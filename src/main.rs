#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
#![deny(warnings)]

// Pedantic / nursery lints triggered by the existing codebase.
// Each one is a tracked cleanup target — see doc/strictness-pass.md.
// Remove the allow once the lint produces zero hits.
// Numeric/cast lints — pervasive in places like progress percentage
// computation; fixing is mostly cosmetic with `#[allow]` at call sites.
// Leave broad until we decide on policy.
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]

// Stylistic / debatable. These either prefer a specific code shape that
// is not always clearer, or warn about patterns the codebase uses
// deliberately.
#![allow(clippy::case_sensitive_file_extension_comparisons)]
#![allow(clippy::doc_link_with_quotes)]
#![allow(clippy::format_push_string)]
#![allow(clippy::implicit_clone)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::module_inception)]
#![allow(clippy::naive_bytecount)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::needless_raw_string_hashes)]
#![allow(clippy::option_if_let_else)]
#![allow(clippy::option_option)]
#![allow(clippy::permissions_set_readonly_false)]
#![allow(clippy::ref_as_ptr)]
#![allow(clippy::similar_names)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::unused_self)]
#![allow(clippy::wildcard_imports)]

// Mutex/lock guard tightening. The lint flags every guard whose scope
// extends past the last use, even by one statement. In practice our
// guards are held for short cache lookups and ad-hoc tightening
// produces busier code without a measurable contention win. Keep
// allowed as a policy choice; revisit only when a hot-path profile
// shows real contention.
#![allow(clippy::significant_drop_tightening)]


mod registries;
mod errors;
mod analyzers;
mod build_context;
mod builder;
mod cache_key;
mod checksum;
mod cli;
mod color;
mod config;
mod db;
mod deps_cache;
mod executor;
mod exit_code;
mod file_index;
mod graph;
mod json_output;
mod object_store;
mod output;
mod phases;
mod platform;
mod processors;
mod progress;
pub(crate) mod word_manager;
mod runtime_flags;
mod remote_cache;
mod tables;
mod tool_lock;
mod watcher;
mod webcache;

use anyhow::{Context, bail, Result};
use cli::{BuildPhase, CacheAction, CleanAction, Commands, WebCacheAction, parse_shell, print_completions};
use config::Config;
use builder::Builder;
use exit_code::{RsconstructExitCode, RsconstructError, classify_error};
use std::env;
use std::fs;
use std::sync::Arc;
use std::time::Instant;

fn main() -> std::process::ExitCode {
    platform::reset_sigpipe();

    let (result, show_status) = run();
    let exit_code = match result {
        Ok(()) => RsconstructExitCode::Success,
        Err(err) => {
            let exit_code = classify_error(&err);
            if json_output::is_json_mode() {
                let error_event = serde_json::json!({
                    "event": "error",
                    "exit_code": exit_code.code(),
                    "exit_code_name": exit_code.name(),
                    "message": format!("{:#}", err),
                });
                eprintln!("{error_event}");
            } else {
                eprintln!("Error [{}]: {:#}", exit_code.name(), err);
            }
            exit_code
        }
    };

    // Final status line — only for build/watch/clean where pass/fail matters.
    // Suppressed in quiet mode, JSON mode, and for informational commands.
    if show_status && !runtime_flags::quiet_or_default() && !runtime_flags::json_mode_or_default() {
        let line = format!(
            "Exited with {} ({})",
            exit_code.name(),
            exit_code.code(),
        );
        if exit_code == RsconstructExitCode::Success {
            eprintln!("{}", color::green(&line));
        } else {
            eprintln!("{}", color::red(&line));
        }
    }

    std::process::ExitCode::from(exit_code.code())
}

/// Returns (result, `show_status_line`). The status line is only shown for
/// build, watch, and clean — commands where pass/fail matters to the user.
fn run() -> (Result<()>, bool) {
    let t_start = Instant::now();
    let cli = cli::parse_cli();
    let cli_parse_dur = t_start.elapsed();

    // Initialize runtime flags from CLI arguments (once, before any reads)
    let t = Instant::now();
    // JSON mode never emits color: stdout must be machine-readable, and
    // --color=always must not smuggle ANSI escapes into it.
    let color_enabled = !cli.json && match cli.color {
        cli::ColorMode::Always => true,
        cli::ColorMode::Never => false,
        cli::ColorMode::Auto => {
            // Disable if NO_COLOR is set to a non-empty value (per the
            // no-color.org spec, an empty value does NOT disable color)
            // or if stdout is not a tty.
            std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
                && std::io::IsTerminal::is_terminal(&std::io::stdout())
        }
    };
    runtime_flags::init(runtime_flags::RuntimeFlags {
        show_child_processes: cli.show_child_processes,
        show_output: cli.show_output,
        phases_debug: cli.phases,
        graph_stats: cli.graph_stats,
        json_mode: cli.json,
        quiet: cli.quiet,
        color_enabled,
    });

    // Create the build context before the signal handler so the handler can
    // call ctx.interrupt() to signal all running subprocesses.
    let ctx = Arc::new(build_context::BuildContext::new());

    // Apply CLI override for mtime cache
    if cli.no_mtime_cache {
        ctx.set_mtime_check(false);
    }

    // Set up Ctrl+C handler. `BuildContext::interrupt()` is the single
    // source of truth: it sets the flag the executor polls between products
    // AND broadcasts on the watch channel that wakes every waiting
    // subprocess. A second standalone `Arc<AtomicBool>` used to be threaded
    // through the executor and watcher in parallel with this, set on the
    // same line and only ever read OR'd with it — pure redundancy.
    {
        let ctx_for_signal = Arc::clone(&ctx);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect(errors::SIGNAL_HANDLER_RUNTIME);
            rt.block_on(async {
                tokio::signal::ctrl_c().await.expect(errors::SIGNAL_LISTEN);
                ctx_for_signal.interrupt();
                eprintln!("\nInterrupted. Press Ctrl+C again to force exit.");
                tokio::signal::ctrl_c().await.expect(errors::SIGNAL_LISTEN);
                std::process::exit(130);
            });
        });
    }
    let init_dur = t.elapsed();

    let show_status = matches!(cli.command,
        Commands::Build { .. } | Commands::Watch { .. } | Commands::Clean { .. } | Commands::Fix { .. }
    );

    // Wrap the body in a closure so `?` works naturally inside, then pair the
    // result with show_status at the end.
    let result = (|| -> Result<()> {

    match cli.command {
        Commands::Build { force, dry_run, verify_tool_versions, stop_after, ref shared } => {
            if dry_run {
                let builder = Builder::new_with_overrides(&shared.iset, &shared.pset)?;
                builder.apply_config_to_context(&ctx);
                builder.dry_run(&ctx, force, shared.explain)?;
            } else {
                let t = Instant::now();
                let mut builder = Builder::new_with_overrides(&shared.iset, &shared.pset)?;
                builder.apply_config_to_context(&ctx);
                let builder_new_dur = t.elapsed();
                let t = Instant::now();
                if verify_tool_versions {
                    builder.verify_tool_versions(&ctx)?;
                }
                let verify_tools_dur = t.elapsed();
                let init_timings = vec![
                    ("cli_parse".to_string(), cli_parse_dur),
                    ("init".to_string(), init_dur),
                    ("builder_new".to_string(), builder_new_dur),
                    ("verify_tools".to_string(), verify_tools_dur),
                ];
                let opts = shared.to_build_options(&cli, force, stop_after);
                builder.build(&ctx, &opts, init_timings)?;
            }
        }
        Commands::Cache { action } => {
            match action {
                CacheAction::Clear => {
                    // Delete .rsconstruct directory directly — must work even if the
                    // database is corrupted and Builder::new() would fail.
                    let rsconstruct_dir = std::path::Path::new(".rsconstruct");
                    if rsconstruct_dir.exists() {
                        crate::errors::ctx(fs::remove_dir_all(rsconstruct_dir), "Failed to remove .rsconstruct directory")?;
                    }
                    println!("Cache cleared.");
                }
                CacheAction::Size => {
                    let builder = Builder::new()?;
                    let (bytes, count) = builder.object_store().size();
                    if json_output::is_json_mode() {
                        let out = serde_json::json!({ "bytes": bytes, "objects": count });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Cache size: {} ({} objects)", humansize::format_size(bytes, humansize::BINARY), count);
                    }
                }
                CacheAction::Trim => {
                    let builder = Builder::new()?;
                    builder.apply_config_to_context(&ctx);
                    let (bytes, count) = builder.object_store().trim()?;
                    // The object store is not the only thing that grows
                    // without bound: mtime rows for deleted files and expired
                    // HTTP responses accumulate forever otherwise.
                    let mtime_pruned = checksum::prune_mtime_cache(&ctx)?;
                    let web_pruned = webcache::prune(ctx.webcache_ttl_secs())?;
                    if json_output::is_json_mode() {
                        let out = serde_json::json!({
                            "removed_bytes": bytes,
                            "removed_objects": count,
                            "removed_mtime_entries": mtime_pruned,
                            "removed_webcache_entries": web_pruned,
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Removed {bytes} bytes ({count} unreferenced objects)");
                        println!("Removed {mtime_pruned} mtime entries for deleted files");
                        println!("Removed {web_pruned} expired webcache entries");
                    }
                }
                CacheAction::RemoveStale => {
                    let builder = Builder::new()?;
                    let valid_keys = builder.valid_cache_keys(&ctx)?;
                    let stale_count = builder.object_store().remove_stale(&valid_keys)?;
                    let (bytes, trim_count) = builder.object_store().trim()?;
                    if json_output::is_json_mode() {
                        let out = serde_json::json!({
                            "stale_index_entries_removed": stale_count,
                            "orphaned_bytes_removed": bytes,
                            "orphaned_objects_removed": trim_count,
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Removed {stale_count} stale index entries");
                        if trim_count > 0 {
                            println!("Removed {bytes} bytes ({trim_count} orphaned objects)");
                        }
                    }
                }
                CacheAction::List => {
                    let builder = Builder::new()?;
                    let entries = builder.object_store().list();
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                }
                CacheAction::Stats => {
                    let builder = Builder::new()?;
                    let stats = builder.object_store().stats_by_processor();

                    if stats.is_empty() {
                        println!("Cache is empty.");
                    } else if json_output::is_json_mode() {
                        println!("{}", serde_json::to_string_pretty(&stats)?);
                    } else {
                        let mut total_entries = 0usize;
                        let mut total_outputs = 0usize;
                        let mut total_bytes = 0u64;
                        for (processor, proc_stats) in &stats {
                            total_entries += proc_stats.entry_count;
                            total_outputs += proc_stats.output_count;
                            total_bytes += proc_stats.output_bytes;
                            println!("{}: {} entries, {} outputs, {}",
                                color::bold(processor),
                                proc_stats.entry_count,
                                proc_stats.output_count,
                                humansize::format_size(proc_stats.output_bytes, humansize::BINARY));
                        }
                        println!();
                        println!("{}: {} entries, {} outputs, {}",
                            color::bold("Total"),
                            total_entries,
                            total_outputs,
                            humansize::format_size(total_bytes, humansize::BINARY));
                    }
                }
                CacheAction::Stale => {
                    let builder = Builder::new()?;
                    let valid_keys = builder.valid_cache_keys(&ctx)?;
                    let entries = builder.object_store().list();
                    let mut current_count = 0usize;
                    let mut stale_count = 0usize;
                    if json_output::is_json_mode() {
                        let mut rows = Vec::with_capacity(entries.len());
                        for entry in &entries {
                            let is_current = valid_keys.contains(&entry.cache_key);
                            if is_current { current_count += 1; } else { stale_count += 1; }
                            rows.push(serde_json::json!({
                                "cache_key": entry.cache_key,
                                "status": if is_current { "current" } else { "stale" },
                            }));
                        }
                        let out = serde_json::json!({
                            "entries": rows,
                            "summary": { "current": current_count, "stale": stale_count },
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        for entry in &entries {
                            if valid_keys.contains(&entry.cache_key) {
                                println!("{} {}", color::green("current"), entry.cache_key);
                                current_count += 1;
                            } else {
                                println!("{} {}", color::yellow("stale"), entry.cache_key);
                                stale_count += 1;
                            }
                        }
                        println!();
                        println!("{}: {} current, {} stale",
                            color::bold("Summary"), current_count, stale_count);
                    }
                }
            }
        }
        Commands::Clean { action } => {
            let action = action.ok_or_else(|| anyhow::anyhow!(
                "Missing subcommand. Usage: rsconstruct clean <outputs|all|git|unknown>"
            ))?;
            match action {
                CleanAction::Outputs { processors, no_empty_dirs } => {
                    let builder = Builder::new()?;
                    let filter = if processors.is_empty() { None } else { Some(processors) };
                    builder.clean(&ctx, cli.verbose, filter.as_deref(), !no_empty_dirs)?;
                }
                CleanAction::All => {
                    let builder = Builder::new()?;
                    builder.distclean()?;
                }
                CleanAction::Git => {
                    let builder = Builder::new()?;
                    builder.hardclean(&ctx)?;
                }
                CleanAction::Unknown { dry_run, no_gitignore } => {
                    let builder = Builder::new()?;
                    builder.clean_unknown(&ctx, !dry_run, cli.verbose, !no_gitignore)?;
                }
            }
        }
        Commands::Complete { shells } => {
            let shells_to_generate = if shells.is_empty() {
                // Load from config file
                let config = Config::load()?;
                let mut parsed_shells = Vec::new();
                for shell_name in &config.completions.shells {
                    match parse_shell(shell_name) {
                        Some(shell) => parsed_shells.push(shell),
                        None => return Err(crate::exit_code::config_error(
                            format!("Unknown shell in config: {shell_name}"))),
                    }
                }
                parsed_shells
            } else {
                shells
            };

            for shell in shells_to_generate {
                print_completions(shell)?;
            }
        }
        Commands::Analyzers { action } => {
            match &action {
                cli::AnalyzersAction::List => {
                    builder::analyzers::list_analyzers(cli.verbose);
                }
                cli::AnalyzersAction::Defconfig { pname } => {
                    builder::analyzers::analyzer_defconfig(pname.as_deref())?;
                }
                cli::AnalyzersAction::Add { pname, dry_run } => {
                    builder::add_analyzer(pname, *dry_run)?;
                }
                cli::AnalyzersAction::Delete { iname } => {
                    builder::smart::delete_analyzer(iname)?;
                }
                cli::AnalyzersAction::Disable { iname } => {
                    builder::smart::disable_analyzer(iname)?;
                }
                cli::AnalyzersAction::Enable { iname } => {
                    builder::smart::enable_analyzer(iname)?;
                }
                _ => {
                    let builder = Builder::new()?;
                    builder.analyzers(&ctx, action, cli.verbose)?;
                }
            }
        }
        Commands::Doctor => {
            let builder = Builder::new()?;
            builder.doctor(&ctx)?;
        }
        Commands::Errors => {
            list_exit_codes(cli.verbose)?;
        }
        Commands::Fix { action } => {
            let builder = Builder::new()?;
            builder.apply_config_to_context(&ctx);
            match action {
                cli::FixAction::Run { processors } => {
                    if processors.is_empty() {
                        bail!("No processors specified. Usage: rsconstruct fix run <processor1,processor2,...>");
                    }
                    builder.fix(&ctx, Some(&processors))?;
                }
                cli::FixAction::List => {
                    builder.fix_list()?;
                }
            }
        }
        Commands::Graph { action } => {
            let builder = Builder::new()?;
            builder.graph(&ctx, action)?;
        }
        Commands::Info { action } => {
            match action {
                cli::InfoAction::Source => {
                    let builder = Builder::new()?;
                    builder.info_source(&ctx)?;
                }
            }
        }
        Commands::Init => {
            init_project()?;
        }
        Commands::Hooks => {
            list_hooks(cli.verbose)?;
        }
        Commands::Processors { action } => {
            let has_config = std::path::Path::new("rsconstruct.toml").exists();
            match action {
                cli::ProcessorAction::List { ref processor_type } => {
                    builder::processors::list_processors_no_config(cli.verbose, processor_type.as_deref())?;
                }
                cli::ProcessorAction::Types => {
                    builder::processors::list_processor_types(cli.verbose)?;
                }
                cli::ProcessorAction::Recommend => {
                    builder::processors::list_recommendations();
                }
                cli::ProcessorAction::Defconfig { ref pname } => {
                    builder::processors::processor_defconfig(pname, cli.verbose)?;
                }
                cli::ProcessorAction::Add { ref pname, dry_run } => {
                    builder::add_processor(pname, dry_run)?;
                }
                cli::ProcessorAction::Search { ref query } => {
                    builder::processors::search_processors(query)?;
                }
                cli::ProcessorAction::Delete { ref iname } => {
                    builder::smart::delete_processor(iname)?;
                }
                cli::ProcessorAction::Disable { ref iname } => {
                    builder::smart::disable_processor(iname)?;
                }
                cli::ProcessorAction::Enable { ref iname } => {
                    builder::smart::enable_processor(iname)?;
                }
                cli::ProcessorAction::Config { .. } if !has_config => {
                    bail!("No rsconstruct.toml found. Use 'processors defconfig <name>' to see default config without a project.");
                }
                action => {
                    let builder = Builder::new()?;
                    builder.processor(&ctx, action, cli.verbose)?;
                }
            }
        }
        Commands::Product { action } => {
            let builder = Builder::new()?;
            builder.apply_config_to_context(&ctx);
            match action {
                cli::ProductAction::Show { ref path } => {
                    builder.product_show(&ctx, path, cli.verbose)?;
                }
            }
        }
        Commands::Sloc { cocomo, salary } => {
            let file_index = file_index::FileIndex::build()?;
            builder::sloc::run_sloc(&file_index, cocomo, salary)?;
        }
        Commands::Smart { action } => {
            match action {
                cli::SmartAction::DisableAll => {
                    builder::smart::disable_all()?;
                }
                cli::SmartAction::EnableAll => {
                    builder::smart::enable_all()?;
                }
                cli::SmartAction::Disable { ref name } => {
                    builder::smart::disable(name)?;
                }
                cli::SmartAction::Enable { ref name } => {
                    builder::smart::enable(name)?;
                }
                cli::SmartAction::EnableDetected => {
                    let builder = Builder::new()?;
                    let detected = builder.detected_processors()?;
                    builder::smart::enable_detected(&detected)?;
                }
                cli::SmartAction::Minimal => {
                    let builder = Builder::new()?;
                    let detected = builder.detected_processors()?;
                    builder::smart::minimal(&detected)?;
                }
                cli::SmartAction::Reset => {
                    builder::smart::reset()?;
                }
                cli::SmartAction::EnableIfAvailable => {
                    let builder = Builder::new()?;
                    let available = builder.detected_and_available_processors()?;
                    builder::smart::enable_if_available(&available)?;
                }
                cli::SmartAction::Only { ref names } => {
                    builder::smart::only(names)?;
                }
                cli::SmartAction::Auto => {
                    let builder = Builder::new()?;
                    let detected = builder.detected_and_available_processors()?;
                    builder::smart::auto(&detected)?;
                }
                cli::SmartAction::RemoveNoFileProcessors => {
                    let builder = Builder::new()?;
                    let empty = builder.no_file_processors(&ctx)?;
                    builder::smart::remove_no_file_processors(&empty)?;
                }
            }
        }
        Commands::Status { breakdown } => {
            let builder = Builder::new()?;
            builder.apply_config_to_context(&ctx);
            builder.status(&ctx, cli.verbose, breakdown)?;
        }
        Commands::SymlinkInstall => {
            let config = Config::load()?;
            builder::symlink_install::run(&config.command.symlink_install)?;
        }
        Commands::Terms { action } => {
            let config = Config::load()?;
            let terms_config: config::TermsConfig =
                config.processor.instance_config_or_default("terms")?;
            match action {
                cli::TermsAction::Fix { remove_non_terms } => {
                    processors::terms::fix_all(&terms_config, remove_non_terms)?;
                }
                cli::TermsAction::Merge { path } => {
                    processors::terms::merge_terms(&terms_config, &path)?;
                }
                cli::TermsAction::Stats => {
                    processors::terms::stats(&terms_config)?;
                }
            }
        }
        Commands::Tags { action } => {
            let config = Config::load()?;
            let db_path = config.processor.instance_field_str("tags", "output")
                .unwrap_or_else(|| "out/tags/tags.db".into());
            let tags_dir = config.processor.instance_field_str("tags", "tags_dir")
                .unwrap_or_else(|| "tags".into());
            match action {
                cli::TagsAction::Files { tags, or } => processors::tags_cmd::files_for_tags(&db_path, &tags, or)?,
                cli::TagsAction::Grep { text, ignore_case } => processors::tags_cmd::grep_tags(&db_path, &text, ignore_case)?,
                cli::TagsAction::List => processors::tags_cmd::list_tags(&db_path)?,
                cli::TagsAction::Count => processors::tags_cmd::count_tags(&db_path)?,
                cli::TagsAction::Tree => processors::tags_cmd::tree_tags(&db_path)?,
                cli::TagsAction::Stats => processors::tags_cmd::stats_tags(&db_path)?,
                cli::TagsAction::ForFile { path } => processors::tags_cmd::tags_for_file(&db_path, &path)?,
                cli::TagsAction::Frontmatter { path } => processors::tags_cmd::frontmatter_for_file(&db_path, &path)?,
                cli::TagsAction::Unused { strict } => processors::tags_cmd::unused_tags(&db_path, &tags_dir, strict)?,
                cli::TagsAction::Validate => processors::tags_cmd::validate_tags(&db_path, &tags_dir)?,
                cli::TagsAction::Matrix => processors::tags_cmd::matrix_tags(&db_path)?,
                cli::TagsAction::Coverage => processors::tags_cmd::coverage_tags(&db_path)?,
                cli::TagsAction::Orphans => processors::tags_cmd::orphan_files(&db_path)?,
                cli::TagsAction::Check => {
                    let tags_config: config::TagsConfig =
                        config.processor.instance_config_or_default("tags")?;
                    processors::tags_cmd::check_tags(&tags_config)?;
                }
                cli::TagsAction::Suggest { path } => {
                    let tags_config: config::TagsConfig =
                        config.processor.instance_config_or_default("tags")?;
                    processors::tags_cmd::suggest_tags(&db_path, &path, &tags_config)?;
                }
                cli::TagsAction::Merge { path } => processors::tags_cmd::merge_tags(&tags_dir, &path)?,
                cli::TagsAction::Collect => processors::tags_cmd::collect_tags(&db_path, &tags_dir)?,
            }
        }
        Commands::Toml { action } => {
            match action {
                cli::TomlAction::Check => {
                    config::Config::require_config()?;
                    // Config::load() validates all fields — unknown fields, types, required fields.
                    // If it succeeds, the config is valid.
                    let _config = config::Config::load()?;
                    println!("rsconstruct.toml is valid.");
                }
            }
        }
        Commands::Functions { action } => {
            use cli::FunctionsAction;
            use processors::generators::tera::TERA_FUNCTIONS;
            match action {
                FunctionsAction::List => {
                    if json_output::is_json_mode() {
                        let arr: Vec<serde_json::Value> = TERA_FUNCTIONS.iter().map(|f| {
                            serde_json::json!({
                                "name": f.name,
                                "summary": f.summary,
                                "args": f.args,
                                "returns": f.returns,
                                "dep_tracking": f.dep_tracking,
                                "example": f.example,
                            })
                        }).collect();
                        println!("{}", serde_json::to_string_pretty(&serde_json::Value::Array(arr))?);
                    } else {
                        for f in TERA_FUNCTIONS {
                            println!("{}({})", f.name, f.args);
                            println!("  {}", f.summary);
                            println!("  returns:       {}", f.returns);
                            println!("  dep tracking:  {}", f.dep_tracking);
                            println!("  example:       {}", f.example);
                            println!();
                        }
                    }
                }
            }
        }
        Commands::Tools { action } => {
            // Fall back to default config only if no config file exists.
            // If config exists but is broken, fail — don't silently use defaults.
            if std::path::Path::new("rsconstruct.toml").exists() {
                let builder = Builder::new()?;
                builder.tools(&ctx, action, cli.verbose)?;
            } else {
                builder::tools::tools_no_config(&ctx, action, cli.verbose)?;
            }
        }
        Commands::Version => {
            if json_output::is_json_mode() {
                let info = serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "authors": env!("CARGO_PKG_AUTHORS"),
                    "git_describe": env!("GIT_DESCRIBE"),
                    "git_sha": env!("GIT_SHA"),
                    "git_branch": env!("GIT_BRANCH"),
                    "git_dirty": env!("GIT_DIRTY"),
                    "rustc_semver": env!("RUSTC_SEMVER"),
                    "rust_edition": env!("RUST_EDITION"),
                    "build_timestamp": env!("BUILD_TIMESTAMP"),
                });
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("rsconstruct {} by {}", env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_AUTHORS"));
                println!("GIT_DESCRIBE: {}", env!("GIT_DESCRIBE"));
                println!("GIT_SHA: {}", env!("GIT_SHA"));
                println!("GIT_BRANCH: {}", env!("GIT_BRANCH"));
                println!("GIT_DIRTY: {}", env!("GIT_DIRTY"));
                println!("RUSTC_SEMVER: {}", env!("RUSTC_SEMVER"));
                println!("RUST_EDITION: {}", env!("RUST_EDITION"));
                println!("BUILD_TIMESTAMP: {}", env!("BUILD_TIMESTAMP"));
            }
        }
        Commands::WebCache { action } => {
            match action {
                WebCacheAction::Clear => {
                    let count = webcache::clear()?;
                    println!("Removed {count} cached entries.");
                }
                WebCacheAction::Stats => {
                    let (bytes, count) = webcache::stats()?;
                    if json_output::is_json_mode() {
                        let out = serde_json::json!({ "bytes": bytes, "entries": count });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Web cache: {} ({} entries)",
                            humansize::format_size(bytes, humansize::BINARY), count);
                    }
                }
                WebCacheAction::List => {
                    let entries = webcache::list()?;
                    if json_output::is_json_mode() {
                        let rows: Vec<serde_json::Value> = entries.iter().map(|e| {
                            serde_json::json!({
                                "url": e.url,
                                "size": e.size,
                                "age_secs": e.age_secs,
                                "expired": e.expired,
                            })
                        }).collect();
                        println!("{}", serde_json::to_string_pretty(&rows)?);
                    } else if entries.is_empty() {
                        println!("Web cache is empty.");
                    } else {
                        let rows: Vec<Vec<String>> = entries.iter().map(|entry| vec![
                            entry.url.clone(),
                            humansize::format_size(entry.size, humansize::BINARY),
                            format_age(entry.age_secs),
                            if entry.expired { "expired" } else { "fresh" }.to_string(),
                        ]).collect();
                        tables::print_table(&["URL", "Size", "Age", "State"], &rows);
                    }
                }
            }
        }
        Commands::Watch { ref shared } => {
            let opts = shared.to_build_options(&cli, false, BuildPhase::Build);
            watcher::watch(&ctx, &opts)?;
        }
    }

    Ok(())

    })(); // end closure
    (result, show_status)
}

/// Render an age in seconds as a compact human string ("3d", "5h", "12m").
/// Coarse on purpose — this exists to answer "is this entry old?", and the
/// exact second a schema was fetched is never the question.
fn format_age(secs: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if secs >= DAY {
        format!("{}d", secs / DAY)
    } else if secs >= HOUR {
        format!("{}h", secs / HOUR)
    } else if secs >= MINUTE {
        format!("{}m", secs / MINUTE)
    } else {
        format!("{secs}s")
    }
}

/// List all exit codes and their meanings.
fn list_exit_codes(verbose: bool) -> Result<()> {
    use strum::IntoEnumIterator;
    use exit_code::RsconstructExitCode;

    if json_output::is_json_mode() {
        #[derive(serde::Serialize)]
        struct Entry { code: u8, name: &'static str, description: &'static str }
        let entries: Vec<Entry> = RsconstructExitCode::iter()
            .map(|e| Entry { code: e.code(), name: e.name(), description: e.description() })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if verbose {
        let rows: Vec<Vec<String>> = RsconstructExitCode::iter()
            .map(|e| vec![e.code().to_string(), e.name().to_string(), e.description().to_string()])
            .collect();
        tables::print_table(&["Code", "Name", "Description"], &rows);
    } else {
        let rows: Vec<Vec<String>> = RsconstructExitCode::iter()
            .map(|e| vec![e.code().to_string(), e.name().to_string()])
            .collect();
        tables::print_table(&["Code", "Name"], &rows);
    }
    Ok(())
}

fn list_hooks(verbose: bool) -> Result<()> {
    let hooks: Vec<&phases::PhaseHook> = phases::all_hooks().collect();
    if json_output::is_json_mode() {
        #[derive(serde::Serialize)]
        struct Entry {
            name: &'static str,
            description: &'static str,
            function: &'static str,
            location: &'static str,
        }
        let entries: Vec<Entry> = hooks.iter()
            .map(|h| Entry {
                name: h.name,
                description: h.description,
                function: h.function,
                location: h.location,
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if verbose {
        let rows: Vec<Vec<String>> = hooks.iter()
            .map(|h| vec![
                h.name.to_string(),
                h.function.to_string(),
                h.location.to_string(),
                h.description.to_string(),
            ])
            .collect();
        tables::print_table(&["Hook", "Function", "Location", "Description"], &rows);
    } else {
        let rows: Vec<Vec<String>> = hooks.iter()
            .map(|h| vec![h.name.to_string(), h.description.to_string()])
            .collect();
        tables::print_table(&["Hook", "Description"], &rows);
    }
    Ok(())
}

/// Initialize a new rsconstruct project in the current directory
fn init_project() -> Result<()> {
    let cwd = env::current_dir()
        .context("Failed to get current directory for project init")?;
    let config_path = cwd.join("rsconstruct.toml");

    if config_path.exists() {
        return Err(RsconstructError::new(
            RsconstructExitCode::ConfigError,
            "rsconstruct.toml already exists in the current directory",
        ).into());
    }

    // Create rsconstruct.toml with commented defaults
    let config_content = r#"# RSConstruct Build Tool Configuration
# Uncomment [processor.NAME] sections to enable processors.
# Each section declares a processor instance; removing it disables the processor.
# For multiple instances: [processor.pylint.core] and [processor.pylint.tests]

[build]
# Number of parallel jobs (0 = auto-detect CPU cores, 1 = sequential)
# parallel = 0
# Max files per batch for batch-capable processors (0 = no limit, omit to disable batching)
# batch_size = 0

[cache]
# restore_method = "auto"  # auto (default: copy in CI, hardlink otherwise), hardlink, or copy

# Uncomment processors you want to use:

# [processor.tera]
# strict = true
# src_dirs = ["tera.templates"]
# src_extensions = [".tera"]

# [processor.ruff]
# command = "ruff"
# args = []

# [processor.pylint]
# args = []

# [processor.cc_single_file]
# cc = "gcc"
# cxx = "g++"
# src_dirs = ["src"]
# src_extensions = [".c", ".cc"]

# [processor.cppcheck]
# args = ["--error-exitcode=1", "--enable=warning,style,performance,portability"]

# [processor.shellcheck]
# args = []

# [processor.make]
# make = "make"

# [processor.cargo]
# cargo = "cargo"

[graph]
# viewer = "google-chrome"

[completions]
# shells = ["bash"]

# [plugins]
# dir = "plugins"  # directory containing .lua processor plugins
"#;
    crate::errors::ctx(fs::write(&config_path, config_content), &format!("Failed to write {}", config_path.display()))?;
    println!("Created {}", config_path.display());

    // Create .rsconstructignore if it doesn't exist
    let rsconstructignore_path = cwd.join(".rsconstructignore");
    if !rsconstructignore_path.exists() {
        let rsconstructignore_content = r#"# .rsconstructignore - Exclude files from rsconstruct processing
# Uses .gitignore syntax (glob patterns, one per line)
# Lines starting with # are comments
#
# Examples:
# /build/           # Exclude a top-level directory
# *.generated.*     # Exclude generated files by pattern
# /src/vendor/**    # Exclude vendored source code
# /experiments/     # Exclude experimental code
# *.bak             # Exclude backup files
"#;
        crate::errors::ctx(fs::write(&rsconstructignore_path, rsconstructignore_content), "Failed to write .rsconstructignore")?;
        println!("Created .rsconstructignore");
    }

    println!("{}", color::green("Project initialized successfully!"));
    println!("{}", color::dim("Hint: edit .rsconstructignore to exclude files from processing"));
    Ok(())
}

