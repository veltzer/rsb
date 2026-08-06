#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
#![deny(warnings)]

// The pedantic/nursery allow list. Every entry below is a decision, not a
// backlog item: each was measured, the alternative was written out, and the
// alternative lost. Lints that were merely noisy have already been removed
// and their hits fixed — see doc/strictness-pass.md for the history.
//
// The bar for adding to this list is: clippy's preferred form is not
// clearly better here, AND the lint fires broadly enough that a per-site
// `#[allow]` with a reason would be worse than one crate-level entry.
// Anything narrower than that belongs at the call site, where several
// already live (search for `#[allow(clippy::` under src/).

// Numeric casts. These fire on progress percentages, byte counts rendered
// for humans, and duration arithmetic — places where the value range is
// known-small and a lossy cast is the intent. None of them cast untrusted
// input, so per-site allows would be ~15 copies of the same sentence.
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]

// Local `static REGEX: OnceLock<Regex>` declarations sit immediately above
// the `get_or_init` that uses them — 8 of them in analyzers/tera.rs alone.
// Hoisting them to the top of the function to satisfy this lint would
// separate each regex from the code explaining what it matches.
#![allow(clippy::items_after_statements)]

// Match arms are kept separate when they mean different things, even where
// they currently share a body. Merging them optimizes for today's
// implementation at the cost of the arm list no longer reading as an
// enumeration of the cases.
#![allow(clippy::match_same_arms)]

// Fires on trait implementations whose signature is fixed by the trait, and
// on `by value` suggestions that would force callers to clone.
#![allow(clippy::needless_pass_by_value)]

// Suggests `map_or`/`map_or_else`, which is less readable than `if let`
// once the branches are more than an expression each. Where the branch
// really is trivial, `is_ok_and`/`is_some_and` is the better form and is
// used directly — clippy's own `unnecessary_map_or` lint agrees.
#![allow(clippy::option_if_let_else)]

// Fires on the CLI dispatch match in main.rs and the per-command handlers.
// These are flat dispatch tables; splitting them produces indirection
// without reducing the amount to read.
#![allow(clippy::too_many_lines)]

// Trait methods that don't happen to read `self` in one implementation.
// The signature belongs to the trait, not the impl.
#![allow(clippy::unused_self)]

// Fires on doc comments containing Rust array/slice literals like
// `["a", "b"]`, which it mistakes for intra-doc links. Every hit in this
// codebase is that false positive, so there is nothing to fix per-site.
#![allow(clippy::doc_link_with_quotes)]

// CLI argument structs and config structs are flag bags by nature — clap
// derives one bool per `--flag`, and the config mirrors the TOML. Grouping
// them into sub-structs to satisfy a 3-bool limit would obscure the
// one-field-per-option mapping that makes them readable.
#![allow(clippy::struct_excessive_bools)]

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
mod display;
mod executor;
mod exit_code;
mod file_index;
mod graph;
mod graph_render;
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
mod stats;
mod tables;
mod tool_lock;
mod tools;
mod watcher;
mod webcache;

use anyhow::{Context, bail, Result};
use cli::{BuildPhase, CleanAction, Commands, WebCacheAction, parse_shell, print_completions};
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
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<()>(0);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect(errors::SIGNAL_HANDLER_RUNTIME);
            rt.block_on(async {
                let mut interrupt = platform::interrupt_signal().expect(errors::SIGNAL_LISTEN);
                // Handler installed — let run() proceed. Before this rendezvous
                // a Ctrl+C would hit the default disposition and kill the
                // process instantly, bypassing interrupt() and every Drop.
                let _ = ready_tx.send(());
                interrupt.recv().await;
                ctx_for_signal.interrupt();
                eprintln!("\nInterrupted. Press Ctrl+C again to force exit.");
                interrupt.recv().await;
                std::process::exit(130);
            });
        });
        // Bounded by thread startup + signal registration: microseconds.
        let _ = ready_rx.recv();
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
                let builder = Builder::new_with_overrides(&ctx, &shared.iset, &shared.pset)?;
                builder.dry_run(&ctx, force, shared.explain)?;
            } else {
                let t = Instant::now();
                let mut builder = Builder::new_with_overrides(&ctx, &shared.iset, &shared.pset)?;
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
            builder::cache_cmd::handle(&ctx, action)?;
        }
        Commands::Clean { action } => {
            let action = action.ok_or_else(|| anyhow::anyhow!(
                "Missing subcommand. Usage: rsconstruct clean <outputs|all|git|unknown>"
            ))?;
            match action {
                CleanAction::Outputs { processors, no_empty_dirs } => {
                    let builder = Builder::new(&ctx)?;
                    let filter = if processors.is_empty() { None } else { Some(processors) };
                    builder.clean(&ctx, cli.verbose, filter.as_deref(), !no_empty_dirs)?;
                }
                CleanAction::All => {
                    let builder = Builder::new(&ctx)?;
                    builder.distclean()?;
                }
                CleanAction::Git => {
                    let builder = Builder::new(&ctx)?;
                    builder.hardclean(&ctx)?;
                }
                CleanAction::Unknown { dry_run, no_gitignore } => {
                    let builder = Builder::new(&ctx)?;
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
                    let builder = Builder::new(&ctx)?;
                    builder.analyzers(&ctx, action, cli.verbose)?;
                }
            }
        }
        Commands::Doctor => {
            let builder = Builder::new(&ctx)?;
            builder.doctor(&ctx)?;
        }
        Commands::Errors => {
            list_exit_codes(cli.verbose)?;
        }
        Commands::Fix { action } => {
            let builder = Builder::new(&ctx)?;
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
            let builder = Builder::new(&ctx)?;
            builder.graph(&ctx, action)?;
        }
        Commands::Info { action } => {
            match action {
                cli::InfoAction::Source => {
                    let builder = Builder::new(&ctx)?;
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
                    let builder = Builder::new(&ctx)?;
                    builder.processor(&ctx, action, cli.verbose)?;
                }
            }
        }
        Commands::Product { action } => {
            let builder = Builder::new(&ctx)?;
            match action {
                cli::ProductAction::Show { ref path } => {
                    builder.product_show(&ctx, path, cli.verbose)?;
                }
            }
        }
        Commands::Sloc { cocomo, salary } => {
            // Generated files are not project code: exclude configured
            // output roots from the count. Config::load falls back to
            // defaults when no rsconstruct.toml exists, so sloc still works
            // outside a project.
            let config = Config::load()?;
            let (exclude_roots, _) = config.file_index_walk_dirs();
            let file_index = file_index::FileIndex::build_with_force_dirs(&[], &exclude_roots, config.build.warn_symlinks)?;
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
                    let builder = Builder::new(&ctx)?;
                    let detected = builder.detected_processors()?;
                    builder::smart::enable_detected(&detected)?;
                }
                cli::SmartAction::Minimal => {
                    let builder = Builder::new(&ctx)?;
                    let detected = builder.detected_processors()?;
                    builder::smart::minimal(&detected)?;
                }
                cli::SmartAction::Reset => {
                    builder::smart::reset()?;
                }
                cli::SmartAction::EnableIfAvailable => {
                    let builder = Builder::new(&ctx)?;
                    let available = builder.detected_and_available_processors()?;
                    builder::smart::enable_if_available(&available)?;
                }
                cli::SmartAction::Only { ref names } => {
                    builder::smart::only(names)?;
                }
                cli::SmartAction::Auto => {
                    let builder = Builder::new(&ctx)?;
                    let detected = builder.detected_and_available_processors()?;
                    builder::smart::auto(&detected)?;
                }
                cli::SmartAction::RemoveNoFileProcessors => {
                    let builder = Builder::new(&ctx)?;
                    let empty = builder.no_file_processors(&ctx)?;
                    builder::smart::remove_no_file_processors(&empty)?;
                }
            }
        }
        Commands::Status { breakdown } => {
            let builder = Builder::new(&ctx)?;
            builder.status(&ctx, cli.verbose, breakdown)?;
        }
        Commands::SymlinkInstall => {
            let config = Config::load()?;
            builder::symlink_install::run(&config.command.symlink_install)?;
        }
        Commands::Terms { action } => {
            let config = Config::load()?;
            let terms_config: processors::terms::TermsConfig =
                config.processor.instance_config_or_default("terms")?;
            match action {
                cli::TermsAction::Fix { remove_non_terms } => {
                    processors::terms::fix_all(&terms_config, remove_non_terms, config.build.warn_symlinks)?;
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
                    let tags_config: processors::tags_cmd::TagsConfig =
                        config.processor.instance_config_or_default("tags")?;
                    processors::tags_cmd::check_tags(&tags_config, config.build.warn_symlinks)?;
                }
                cli::TagsAction::Suggest { path } => {
                    let tags_config: processors::tags_cmd::TagsConfig =
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
                    if json_output::is_json_mode() {
                        println!("{}", serde_json::json!({ "valid": true }));
                    } else {
                        output::info("rsconstruct.toml is valid.");
                    }
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
                let builder = Builder::new(&ctx)?;
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
                    if json_output::is_json_mode() {
                        println!("{}", serde_json::json!({ "removed_entries": count }));
                    } else {
                        output::info(&format!("Removed {count} cached entries."));
                    }
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
        let rsconstructignore_content = r"# .rsconstructignore - Exclude files from rsconstruct processing
# Uses .gitignore syntax (glob patterns, one per line)
# Lines starting with # are comments
#
# Examples:
# /build/           # Exclude a top-level directory
# *.generated.*     # Exclude generated files by pattern
# /src/vendor/**    # Exclude vendored source code
# /experiments/     # Exclude experimental code
# *.bak             # Exclude backup files
";
        crate::errors::ctx(fs::write(&rsconstructignore_path, rsconstructignore_content), "Failed to write .rsconstructignore")?;
        println!("Created .rsconstructignore");
    }

    println!("{}", color::green("Project initialized successfully!"));
    println!("{}", color::dim("Hint: edit .rsconstructignore to exclude files from processing"));
    Ok(())
}

