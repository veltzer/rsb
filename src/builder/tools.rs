use std::collections::BTreeMap;
// Aliased: `std::io::Write` is also in scope here (for stdout flushing), and
// both traits define `write!`/`writeln!` methods.
use std::fmt::Write as FmtWrite;
use std::io::Write;
use std::process::Command;
use anyhow::{bail, Context, Result};
use crate::cli::{GraphFormat, ToolsAction};
use crate::color;
use crate::tables;
use crate::json_output;
use crate::tool_lock;
use super::{Builder, sorted_keys};

/// Check if a system package is installed using the platform's package manager.
/// Tries dpkg-query (Debian/Ubuntu), rpm (Fedora/RHEL), pacman (Arch), and brew (macOS).
///
/// Probes run through the central runner so a `tools check` over a long
/// package list stays interruptible — `dpkg-query` on a large system is not
/// instant, and one probe runs per declared package.
///
/// Shared with `doctor`: a `[dependencies] system` entry is a package, not a
/// tool, so every probe of one must go through the package manager. Checking
/// it with `which()` misreports in both directions — a binary-less package
/// like aspell-en shows as missing forever, and a binary that happens to share
/// the package's name shows as installed without the package being on.
pub(super) fn is_system_package_installed(ctx: &crate::build_context::BuildContext, pkg: &str) -> bool {
    let probe = |program: &str, args: &[&str]| -> bool {
        let mut cmd = Command::new(program);
        cmd.args(args);
        crate::processors::run_command_capture(ctx, &cmd)
            .is_ok_and(|o| o.status.success())
    };

    if which::which("dpkg-query").is_ok() {
        // Exit status alone is not enough. dpkg-query exits 0 for a package it
        // merely knows about -- one removed earlier, or named as a dependency --
        // printing "unknown ok not-installed" or "deinstall ok config-files".
        // Only "install ok installed" means the files are actually on disk.
        //
        // Trusting the exit code made install-deps silently skip aspell-en on a
        // runner that had neither aspell nor aspell-en, so apt was never asked
        // for it and the spellcheck processor then failed on missing
        // /usr/lib/aspell/en.dat. Packages that ship no binary of their own are
        // the ones this hides, because the which() fallback cannot catch them.
        let mut cmd = Command::new("dpkg-query");
        cmd.args(["-W", "-f", "${Status}", pkg]);
        return crate::processors::run_command_capture(ctx, &cmd).is_ok_and(|o| {
            o.status.success() && dpkg_status_means_installed(&String::from_utf8_lossy(&o.stdout))
        });
    }
    if which::which("rpm").is_ok() {
        return probe("rpm", &["-q", pkg]);
    }
    if which::which("pacman").is_ok() {
        return probe("pacman", &["-Q", pkg]);
    }
    if which::which("brew").is_ok() {
        return probe("brew", &["list", pkg]);
    }
    // Fallback: check if the package name is available as a command
    which::which(pkg).is_ok()
}

/// Whether a `dpkg-query -f '${Status}'` line means the package is on disk.
///
/// dpkg's status is three words: want, error, state. Only the `installed`
/// state means the files are present -- `not-installed` and `config-files`
/// both leave nothing usable behind, yet dpkg-query still exits 0 for them.
fn dpkg_status_means_installed(status: &str) -> bool {
    status.split_whitespace().nth(2) == Some("installed")
}

/// Whether a `required_tools()` entry names something the tool registry could
/// ever install.
///
/// A processor `command` that contains a path separator is a repo-local script
/// (`scripts/check_md.py`), not a binary on `$PATH` — see `ToolInfo::name` in
/// `src/tools.rs` for why a name with a `/` cannot serve as a registry key.
/// Such a command is checked into the repo and needs no installing, so
/// `tools install` must skip it rather than fail with "No install method".
/// The build-time preflight in `builder/build.rs` still requires it to exist,
/// so skipping it here does not weaken strictness.
fn is_installable_tool_name(tool: &str) -> bool {
    !tool.contains('/')
}

/// Whether a package manager reports `pkg` as already installed.
///
/// One helper for the pip/npm/gem probes, which were four copies of the same
/// spawn-and-discard-output block. Routed through the central runner so a
/// long `install-deps` preflight stays interruptible.
fn package_probe_succeeds(
    ctx: &crate::build_context::BuildContext,
    program: &str,
    args: &[&str],
) -> bool {
    let mut cmd = Command::new(program);
    cmd.args(args);
    crate::processors::run_command_capture(ctx, &cmd)
        .is_ok_and(|o| o.status.success())
}

/// Return the language runtime category for a tool.
/// Delegates to the central `TOOLS` registry in `src/tools.rs`.
fn tool_runtime(tool: &str) -> &'static str {
    crate::tools::tool_runtime(tool).unwrap_or_else(|| {
        debug_assert!(false, "tool_runtime: unrecognized tool '{tool}'");
        "system"
    })
}

/// Handle `rsconstruct tools` subcommands without a project config.
/// Uses default processor configs so List, Stats, Install, and Graph work
/// even outside a project directory.
pub fn tools_no_config(ctx: &crate::build_context::BuildContext, action: ToolsAction, verbose: bool) -> Result<()> {
    let processors = super::create_all_default_processors()?;
    run_tools_command(ctx, &processors, &|_name| true, action, verbose, None)
}

impl Builder {
    /// Verify tool versions against .tools.versions lock file.
    /// Called at the start of build unless --ignore-tool-versions is passed.
    pub fn verify_tool_versions(&self, ctx: &crate::build_context::BuildContext) -> Result<()> {
        let processors = self.create_processors()?;
        let tool_commands = tool_lock::collect_tool_commands(
            &processors,
            &|_name| true,
        );
        if tool_commands.is_empty() {
            return Ok(());
        }
        tool_lock::verify_lock_file(ctx, &tool_commands)
    }

    /// Handle `rsconstruct tools` subcommands
    pub fn tools(&self, ctx: &crate::build_context::BuildContext, action: ToolsAction, verbose: bool) -> Result<()> {
        let processors = self.create_processors()?;
        run_tools_command(
            ctx,
            &processors,
            &|_name| true,
            action,
            verbose,
            Some(self),
        )
    }
}

/// Core implementation for `tools` subcommands, shared by `Builder::tools()` and `tools_no_config()`.
/// `builder` is `Some` when running with a project config (needed for `open_file`, `Check`, `Lock`).
fn run_tools_command(
    ctx: &crate::build_context::BuildContext,
    processors: &crate::processors::ProcessorMap,
    is_enabled: &dyn Fn(&str) -> bool,
    action: ToolsAction,
    verbose: bool,
    builder: Option<&Builder>,
) -> Result<()> {
    let show_all = matches!(&action, ToolsAction::ListConfigured { all: true, .. });
    let show_methods = matches!(&action,
        ToolsAction::ListConfigured { methods: true, .. } | ToolsAction::List { methods: true });
    let install_interactive = matches!(&action, ToolsAction::Install { interactive: true, .. });
    let install_no_eatmydata = matches!(&action, ToolsAction::Install { no_eatmydata: true, .. });
    // eatmydata speeds up apt/dnf/pacman by no-op'ing fsync; the trade-off
    // is loss-on-power-cut. Off by default; the post-config phase hook
    // `eatmydata_ci_default` flips it on when CI=true. Users who want it
    // off in CI should unset CI; users who want it on outside CI should
    // run with CI=true. The CLI flag --no-eatmydata always wins.
    // `--all` is config-independent by design, so there may be no Config for
    // the `eatmydata_ci_default` hook to have run against. Apply the same
    // CI=true policy directly, otherwise the one caller that most wants the
    // speedup (a CI runner with no rsconstruct.toml) would never get it.
    let install_all = matches!(&action, ToolsAction::Install { all: true, .. });
    let config_enables_eatmydata = builder.is_some_and(|b| b.config.dependencies.eatmydata)
        || (install_all && crate::config::running_in_ci());
    let install_use_eatmydata = !install_no_eatmydata
        && config_enables_eatmydata
        && which::which("eatmydata").is_ok();

    let mut tool_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in sorted_keys(processors) {
        if !show_all && !is_enabled(name) {
            continue;
        }
        for tool in processors[name].required_tools() {
            let procs = tool_map.entry(tool).or_default();
            if !procs.contains(name) {
                procs.push(name.clone());
            }
        }
    }

    match action {
        ToolsAction::List { .. } => {
            // Registry-wide list: every tool rsconstruct knows how to install,
            // independent of which processors are configured. Mirrors
            // `processors list`, which shows all built-in processors.
            if crate::json_output::is_json_mode() {
                let entries: Vec<json_output::ToolListEntry> = crate::tools::all_tools()
                    .map(|info| json_output::ToolListEntry {
                        tool: info.name.to_string(),
                        installed: which::which(info.name).is_ok(),
                        runtime: info.runtime.to_string(),
                        processors: Vec::new(),
                        install_methods: info.install_methods.iter().map(|m| {
                            json_output::ToolInstallMethodEntry {
                                method: m.method.to_string(),
                                command: m.command(),
                            }
                        }).collect(),
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
                return Ok(());
            }

            let mut tools: Vec<&crate::tools::ToolInfo> = crate::tools::all_tools().collect();
            tools.sort_by_key(|t| t.name);

            // The install command is noise for the common "what tools exist?"
            // question, so it's hidden by default — shown only with --verbose
            // (or --methods for every method). JSON always carries it.
            let show_install = verbose || show_methods;
            let mut headers: Vec<&str> = vec!["Tool", "Status", "Runtime"];
            if show_install {
                headers.push("Install");
            }
            let rows: Vec<Vec<String>> = tools.iter().map(|info| {
                let status = if which::which(info.name).is_ok() {
                    color::green("installed")
                } else {
                    color::red("missing")
                };
                let mut row = vec![info.name.to_string(), status.to_string(), info.runtime.to_string()];
                if show_install {
                    let install_str = if show_methods {
                        let methods: Vec<String> = info.install_methods.iter()
                            .map(|m| format!("{}: {}", m.method, m.command()))
                            .collect();
                        if methods.is_empty() { "?".to_string() } else { methods.join(" | ") }
                    } else {
                        info.install_methods.first().map_or_else(|| "?".to_string(), crate::tools::InstallMethod::command)
                    };
                    row.push(install_str);
                }
                row
            }).collect();
            tables::print_table(&headers, &rows);
        }
        ToolsAction::ListConfigured { .. } => {
            if crate::json_output::is_json_mode() {
                let entries: Vec<json_output::ToolListEntry> = tool_map.iter()
                    .map(|(tool, procs)| {
                        let info = crate::tools::tool_info(tool);
                        let install_methods = info
                            .map(|i| i.install_methods.iter().map(|m| {
                                json_output::ToolInstallMethodEntry {
                                    method: m.method.to_string(),
                                    command: m.command(),
                                }
                            }).collect())
                            .unwrap_or_default();
                        json_output::ToolListEntry {
                            tool: tool.clone(),
                            installed: which::which(tool).is_ok(),
                            runtime: info.map_or("unknown", |i| i.runtime).to_string(),
                            processors: procs.clone(),
                            install_methods,
                        }
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
                return Ok(());
            }

            for (tool, procs) in &tool_map {
                let installed = if which::which(tool).is_ok() {
                    color::green("installed")
                } else {
                    color::red("missing")
                };
                let info = crate::tools::tool_info(tool);
                let runtime = info.map_or("unknown", |i| i.runtime);
                let install_str = if show_methods {
                    let methods: Vec<String> = info
                        .map(|i| i.install_methods.iter()
                            .map(|m| format!("{}: {}", m.method, m.command()))
                            .collect())
                        .unwrap_or_default();
                    if methods.is_empty() { "?".to_string() } else { methods.join(" | ") }
                } else {
                    info.and_then(|i| i.install_methods.first()).map_or_else(|| "?".to_string(), crate::tools::InstallMethod::command)
                };
                println!("{} [{}] [{}] ({}) — {}",
                    tool, installed, runtime, procs.join(", "), color::dim(&install_str));
            }
        }
        ToolsAction::Check => {
            let tool_commands = tool_lock::collect_tool_commands(processors, is_enabled);
            tool_lock::verify_lock_file(ctx, &tool_commands)?;
            let lock = tool_lock::read_lock_file()?;
            if crate::json_output::is_json_mode() {
                let entries: Vec<serde_json::Value> = lock
                    .as_ref()
                    .map(|l| l.tools.iter().map(|(name, info)| {
                        let version = tool_lock::extract_semver(&info.version_output).unwrap_or("?");
                        serde_json::json!({
                            "tool": name,
                            "status": "ok",
                            "version": version,
                        })
                    }).collect())
                    .unwrap_or_default();
                let out = serde_json::json!({
                    "match": true,
                    "tools": entries,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                if verbose
                    && let Some(lock) = &lock {
                        for (name, info) in &lock.tools {
                            let version = tool_lock::extract_semver(&info.version_output).unwrap_or("?");
                            println!("{} {} {}", name, color::green("ok"), color::dim(version));
                        }
                    }
                println!("{}", color::green("Tool versions match lock file."));
            }
        }
        ToolsAction::Lock => {
            let tool_commands = tool_lock::collect_tool_commands(processors, is_enabled);
            let lock = tool_lock::create_lock(ctx, &tool_commands)?;
            tool_lock::write_lock_file(&lock)?;
            if crate::json_output::is_json_mode() {
                let entries: Vec<serde_json::Value> = lock.tools.iter().map(|(name, info)| {
                    let version = tool_lock::extract_semver(&info.version_output).unwrap_or("?");
                    serde_json::json!({
                        "tool": name,
                        "status": "locked",
                        "version": version,
                    })
                }).collect();
                let out = serde_json::json!({
                    "lock_file": ".tools.versions",
                    "tools": entries,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                for (name, info) in &lock.tools {
                    let version = tool_lock::extract_semver(&info.version_output).unwrap_or("?");
                    println!("{} {} {}", name, color::green("locked"), color::dim(version));
                }
                println!("Wrote {}", color::bold(".tools.versions"));
            }
        }
        ToolsAction::Graph { format, view } => {
            if view {
                let html_content = tools_graph_html(&tool_map);
                let html_path = std::env::temp_dir().join("rsconstruct_tools_graph.html");
                std::fs::write(&html_path, &html_content)
                    .with_context(|| format!("Failed to write HTML file: {}", html_path.display()))?;
                if let Some(b) = builder {
                    b.open_file(&html_path)?;
                    println!("Opened tools graph in browser: {}", html_path.display());
                } else {
                    println!("Wrote tools graph to: {}", html_path.display());
                }
            } else {
                let effective_format = if crate::json_output::is_json_mode() {
                    GraphFormat::Json
                } else {
                    format
                };
                let output = match effective_format {
                    GraphFormat::Dot => tools_graph_dot(&tool_map),
                    GraphFormat::Mermaid => tools_graph_mermaid(&tool_map),
                    GraphFormat::Text => tools_graph_text(&tool_map),
                    GraphFormat::Json => tools_graph_json(&tool_map)?,
                    GraphFormat::Svg => tools_graph_svg(ctx, &tool_map)?,
                };
                println!("{output}");
            }
        }
        ToolsAction::Stats => {
            let mut tool_stats: Vec<json_output::ToolStat> = Vec::new();
            for (tool, procs) in &tool_map {
                let installed = which::which(tool).is_ok();
                let runtime = tool_runtime(tool).to_string();
                let install_command = crate::tools::tool_install_command(tool);
                tool_stats.push(json_output::ToolStat {
                    name: tool.clone(),
                    installed,
                    runtime,
                    processors: procs.clone(),
                    install_command,
                });
            }

            let mut runtime_map: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
            for stat in &tool_stats {
                let entry = runtime_map.entry(tool_runtime(&stat.name)).or_insert((0, 0));
                entry.0 += 1;
                if stat.installed {
                    entry.1 += 1;
                }
            }
            let runtime_stats: Vec<json_output::RuntimeStat> = runtime_map
                .iter()
                .map(|(runtime, (total, installed))| json_output::RuntimeStat {
                    runtime: runtime.to_string(),
                    total: *total,
                    installed: *installed,
                    missing: total - installed,
                })
                .collect();

            let total_tools = tool_stats.len();
            let installed_count = tool_stats.iter().filter(|t| t.installed).count();
            let missing_count = total_tools - installed_count;

            if crate::json_output::is_json_mode() {
                let output = json_output::ToolStatsOutput {
                    tools: tool_stats,
                    runtimes: runtime_stats,
                    summary: json_output::StatsSummary {
                        total_tools,
                        installed: installed_count,
                        missing: missing_count,
                    },
                };
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let rows: Vec<Vec<String>> = tool_stats.iter().map(|stat| {
                    let status = if stat.installed {
                        color::green("\u{2713}")
                    } else {
                        color::red("\u{2717}")
                    };
                    let procs = stat.processors.join(", ");
                    let install = stat.install_command.as_deref().unwrap_or("").to_string();
                    vec![stat.name.clone(), status.to_string(), procs, install]
                }).collect();
                tables::print_table(&["Tool", "Status", "Processors", "Install"], &rows);

                println!();
                println!("Runtime summary:");
                let runtime_display: &[(&str, &str)] = &[
                    ("python", "Python"),
                    ("node", "Node.js"),
                    ("ruby", "Ruby"),
                    ("rust", "Rust"),
                    ("perl", "Perl"),
                    ("jvm", "JVM"),
                    ("system", "System"),
                ];
                let rt_rows: Vec<Vec<String>> = runtime_display.iter().filter_map(|(key, label)| {
                    runtime_stats.iter().find(|r| r.runtime == *key).map(|rs| {
                        let status_str = format!("{}/{}", rs.installed, rs.total);
                        let line = if rs.missing > 0 {
                            color::yellow(&status_str)
                        } else {
                            color::green(&status_str)
                        };
                        vec![label.to_string(), line.to_string()]
                    })
                }).collect();
                tables::print_table(&["Runtime", "Installed"], &rt_rows);

                println!();
                let total_line = format!("Total: {installed_count}/{total_tools} tools installed");
                if missing_count > 0 {
                    println!("{}", color::yellow(&total_line));
                } else {
                    println!("{}", color::green(&total_line));
                }
            }
        }
        ToolsAction::Install { name, all, .. } => {
            // Collect missing tools with their install info
            let missing_tools: Vec<(&str, &crate::tools::InstallMethod)> = if all {
                // Registry-wide: every tool rsconstruct knows how to install,
                // independent of any config. Mirrors `tools list`, which is the
                // registry-wide counterpart of `tools list-configured`.
                //
                // Nothing is skipped. A tool with no automatable method is a
                // hard error, not a warning to scroll past: --all claiming
                // success while a tool is absent is exactly the silent-gap
                // failure the strict-by-default rule exists to prevent.
                let mut missing = Vec::new();
                let mut manual_only: Vec<&crate::tools::ToolInfo> = Vec::new();
                for info in crate::tools::all_tools() {
                    if which::which(info.name).is_ok() {
                        continue;
                    }
                    // Prefer any automatable method over a manual one. clojure
                    // lists manual first (the official Linux route) but also
                    // carries a real brew method — taking `.first()` blindly
                    // would strand it as uninstallable on a host that has brew.
                    match info.install_methods.iter().find(|m| m.method != "manual") {
                        Some(method) => missing.push((info.name, method)),
                        None => manual_only.push(info),
                    }
                }
                if !manual_only.is_empty() {
                    eprintln!(
                        "{}: {} tool(s) have no automatable install method:",
                        color::red("Error"),
                        manual_only.len(),
                    );
                    for info in &manual_only {
                        let instructions = info.install_methods.first()
                            .map_or("no install method known", |m| m.package);
                        eprintln!("  {} — {}", color::bold(info.name), instructions);
                    }
                    return Err(crate::exit_code::RsconstructError::new(
                        crate::exit_code::RsconstructExitCode::ToolError,
                        format!(
                            "Cannot install {} tool(s) automatically: {}",
                            manual_only.len(),
                            manual_only.iter().map(|i| i.name).collect::<Vec<_>>().join(", "),
                        ),
                    ).into());
                }
                if missing.is_empty() {
                    println!("{}", color::green("All registry tools are already installed."));
                    return Ok(());
                }
                missing
            } else if let Some(ref name) = name {
                // Install a specific tool
                if let Some(info) = crate::tools::tool_info(name) {
                    if let Some(method) = info.install_methods.first() {
                        vec![(info.name, method)]
                    } else {
                        eprintln!("{}: No install method for '{}'", color::red("Error"), name);
                        return Err(crate::exit_code::RsconstructError::new(
                            crate::exit_code::RsconstructExitCode::ToolError,
                            format!("No install method known for tool '{name}'"),
                        ).into());
                    }
                } else {
                    eprintln!("{}: Unknown tool '{}'", color::red("Error"), name);
                    return Err(crate::exit_code::RsconstructError::new(
                        crate::exit_code::RsconstructExitCode::ToolError,
                        format!("Unknown tool '{name}'"),
                    ).into());
                }
            } else {
                // Build tool list from all enabled processors.
                // Every processor in the config gets its tools installed —
                // auto_detect is not checked because the tool itself may need
                // to be installed before detection can succeed.
                let mut install_tools: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for name in sorted_keys(processors) {
                    if !is_enabled(name) {
                        continue;
                    }
                    for tool in processors[name].required_tools() {
                        let procs = install_tools.entry(tool).or_default();
                        if !procs.contains(name) {
                            procs.push(name.clone());
                        }
                    }
                }

                // Collect missing tools
                let mut missing = Vec::new();
                let mut any_unknown = false;
                for tool_name in install_tools.keys() {
                    if !is_installable_tool_name(tool_name) {
                        continue;
                    }
                    if which::which(tool_name).is_ok() {
                        continue;
                    }
                    match crate::tools::tool_info(tool_name) {
                        Some(info) if !info.install_methods.is_empty() => {
                            missing.push((info.name, &info.install_methods[0]));
                        }
                        _ => {
                            eprintln!("{}: No install method for '{}'", color::red("Error"), tool_name);
                            any_unknown = true;
                        }
                    }
                }
                if any_unknown {
                    return Err(crate::exit_code::RsconstructError::new(
                        crate::exit_code::RsconstructExitCode::ToolError,
                        "Some tools have no known install procedure",
                    ).into());
                }
                if missing.is_empty() {
                    println!("{}", color::green("All tools are already installed."));
                    return Ok(());
                }
                missing
            };

            // Group by install method for batch installation. Methods
            // that don't batch (binary, manual) keep one entry per package.
            let mut by_method: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (_tool_name, method) in &missing_tools {
                by_method.entry(method.method).or_default().push(method.package);
            }

            // Display the install plan
            println!("Missing {} tool(s), grouped by install method:", missing_tools.len());
            println!();

            let method_order = &["pip", "apt", "brew", "npm", "cargo", "gem", "snap", "binary", "manual"];
            let ctx = crate::tools::InstallCtx {
                verbose,
                use_eatmydata: install_use_eatmydata,
            };
            let mut any_failed = false;
            for method in method_order {
                let Some(packages) = by_method.get(method) else { continue };
                println!("  {} {}", color::bold(&format!("[{method}]")), packages.join(", "));
                for argv in crate::tools::describe(method, packages) {
                    println!("       {}", color::dim(&argv.join(" ")));
                }
            }
            println!();

            // Non-interactive by default: CI and scripts are the common
            // callers and neither can answer a prompt. `-i` opts into the
            // confirmation, and declining it is a normal outcome, not an
            // error — the user saw the command list and said no.
            if install_interactive {
                print!("Proceed? [y/N] ");
                std::io::stdout().flush()
                    .context("Failed to flush stdout before install prompt")?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)
                    .context("Failed to read install prompt answer")?;
                let answer = answer.trim().to_lowercase();
                if answer != "y" && answer != "yes" {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            for method in method_order {
                let Some(packages) = by_method.get(method) else { continue };
                match crate::tools::run(method, packages, &ctx) {
                    Ok(()) => println!("{} [{}] {}", color::green("OK"), method, packages.join(", ")),
                    Err(e) => {
                        println!("{} [{}] {}: {}", color::red("FAILED"), method, packages.join(", "), e);
                        any_failed = true;
                    }
                }
            }

            if any_failed {
                return Err(crate::exit_code::RsconstructError::new(
                    crate::exit_code::RsconstructExitCode::ToolError,
                    "Some install commands failed",
                ).into());
            }
            println!("{}", color::green("All tools installed successfully."));
        }
        ToolsAction::InstallDeps { interactive, no_eatmydata } => {
            let config = builder
                .map(|b| &b.config.dependencies)
                .ok_or_else(|| anyhow::anyhow!("install-deps requires a project with rsconstruct.toml"))?;
            // eatmydata: off by default; turned on by the post-config
            // phase hook when CI=true. CLI --no-eatmydata overrides.
            // Wrap also requires eatmydata to be on PATH.
            let use_eatmydata = !no_eatmydata
                && config.eatmydata
                && which::which("eatmydata").is_ok();

            // The Python set comes from uv.lock by default (pyproject.toml
            // in pyproject mode); [dependencies].pip entries merge in front.
            let pip_deps = config.effective_pip(std::path::Path::new("."))?;

            if config.is_empty() && pip_deps.is_empty() {
                println!("No dependencies declared in [dependencies] or pyproject.toml.");
                return Ok(());
            }

            // Install order is FIXED and load-bearing: system → pip → npm → gem.
            // Language-level packages (pip, gem, npm) often build native
            // extensions at install time that link against system libraries
            // via pkg-config — system deps must be present first.
            // See docs/src/configuration.md "Install order".
            let system_method: &str = if which::which("apt-get").is_ok() {
                "apt"
            } else if which::which("dnf").is_ok() {
                "dnf"
            } else if which::which("pacman").is_ok() {
                "pacman"
            } else if which::which("brew").is_ok() {
                "brew"
            } else if !config.system.is_empty() {
                bail!(
                    "No supported package manager found (apt-get, dnf, pacman, brew); install these system packages manually: {}",
                    config.system.join(", ")
                );
            } else {
                "apt" // unused
            };

            let mut skipped: Vec<String> = Vec::new();
            let mut groups: Vec<(&str, Vec<String>)> = Vec::new();

            let system_missing: Vec<String> = config.system.iter()
                .filter(|pkg| {
                    let installed = is_system_package_installed(ctx, pkg);
                    if installed { skipped.push(format!("[system] {pkg}")); }
                    !installed
                })
                .cloned()
                .collect();
            if !system_missing.is_empty() {
                groups.push((system_method, system_missing));
            }

            let pip_missing: Vec<String> = pip_deps.iter()
                .filter(|pkg| {
                    // `pip show` only proves the base distribution is present;
                    // it says nothing about an extras entry like `pkg[extra]`,
                    // whose extra dependencies may still be missing. Always
                    // hand extras entries to pip — the install is idempotent.
                    if pkg.contains('[') {
                        return true;
                    }
                    let name = crate::config::normalized_distribution_name(pkg);
                    let installed = package_probe_succeeds(ctx, "pip", &["show", name.as_str()]);
                    if installed { skipped.push(format!("[pip] {pkg}")); }
                    !installed
                })
                .cloned()
                .collect();
            if !pip_missing.is_empty() {
                groups.push(("pip", pip_missing));
            }

            let npm_missing: Vec<String> = config.npm.iter()
                .filter(|pkg| {
                    let installed = package_probe_succeeds(ctx, "npm", &["ls", "-g", pkg]);
                    if installed { skipped.push(format!("[npm] {pkg}")); }
                    !installed
                })
                .cloned()
                .collect();
            if !npm_missing.is_empty() {
                groups.push(("npm", npm_missing));
            }

            let gem_missing: Vec<String> = config.gem.iter()
                .filter(|pkg| {
                    let installed = package_probe_succeeds(ctx, "gem", &["list", "-i", pkg]);
                    if installed { skipped.push(format!("[gem] {pkg}")); }
                    !installed
                })
                .cloned()
                .collect();
            if !gem_missing.is_empty() {
                groups.push(("gem", gem_missing));
            }

            if !skipped.is_empty() {
                println!("{}:", color::dim("Already installed (skipping)"));
                for s in &skipped {
                    println!("  {} {}", color::green("✓"), s);
                }
                println!();
            }

            if groups.is_empty() {
                println!("{}", color::green("All dependencies already installed."));
                return Ok(());
            }

            println!("{}:", color::bold("Dependencies to install"));
            for (method, packages) in &groups {
                let pkgs_ref: Vec<&str> = packages.iter().map(String::as_str).collect();
                let preview = crate::tools::describe(method, &pkgs_ref);
                let line = preview.first().map(|a| a.join(" ")).unwrap_or_default();
                println!("  {} {}", color::bold(&format!("[{method}]")), color::dim(&line));
            }
            println!();

            // Non-interactive by default; see the note in Install above.
            if interactive {
                use std::io::Write;
                print!("Proceed? [y/N] ");
                std::io::stdout().flush()
                    .context("Failed to flush stdout before dependency install prompt")?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)
                    .context("Failed to read dependency install prompt answer")?;
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let ctx = crate::tools::InstallCtx { verbose, use_eatmydata };
            let mut any_failed = false;
            for (method, packages) in &groups {
                let pkgs_ref: Vec<&str> = packages.iter().map(String::as_str).collect();
                match crate::tools::run(method, &pkgs_ref, &ctx) {
                    Ok(()) => println!("{} [{}] {}", color::green("✓"), method, packages.join(", ")),
                    Err(e) => {
                        println!("{} [{}] {}: {}", color::red("✗"), method, packages.join(", "), e);
                        any_failed = true;
                    }
                }
            }

            if any_failed {
                return Err(crate::exit_code::RsconstructError::new(
                    crate::exit_code::RsconstructExitCode::ToolError,
                    "Some dependency installs failed",
                ).into());
            }
            println!("{}", color::green("All dependencies installed successfully."));
        }
    }

    Ok(())
}

/// Sanitize a name for use as a DOT node identifier
fn sanitize_node_id(prefix: &str, name: &str) -> String {
    format!("{}_{}", prefix, name.replace(['.', '-', ' ', '+'], "_"))
}

fn tools_graph_dot(tool_map: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = String::from("digraph tools {\n    rankdir=LR;\n    node [fontname=\"sans-serif\"];\n");

    // Collect unique processor names
    let mut processors: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for procs in tool_map.values() {
        for p in procs {
            processors.insert(p);
        }
    }

    // Tool nodes
    out.push_str("    // Tools\n");
    for tool in tool_map.keys() {
        let id = sanitize_node_id("tool", tool);
        let _ = writeln!(
            out,
            "    {id} [label=\"{tool}\" shape=box style=filled fillcolor=lightblue];"
        );
    }

    // Processor nodes
    out.push_str("    // Processors\n");
    for proc in &processors {
        let id = sanitize_node_id("proc", proc);
        let _ = writeln!(
            out,
            "    {id} [label=\"{proc}\" shape=box style=filled fillcolor=lightyellow];"
        );
    }

    // Edges
    out.push_str("    // Edges\n");
    for (tool, procs) in tool_map {
        let tool_id = sanitize_node_id("tool", tool);
        for proc in procs {
            let proc_id = sanitize_node_id("proc", proc);
            let _ = writeln!(out, "    {tool_id} -> {proc_id};");
        }
    }

    out.push_str("}\n");
    out
}

fn tools_graph_mermaid(tool_map: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = String::from("graph LR\n");

    for (tool, procs) in tool_map {
        let tool_id = sanitize_node_id("tool", tool);
        for proc in procs {
            let proc_id = sanitize_node_id("proc", proc);
            let _ = writeln!(
                out,
                "    {tool_id}[\"{tool}\"]:::tool --> {proc_id}[\"{proc}\"]:::processor"
            );
        }
    }

    out.push_str("    classDef tool fill:#add8e6\n");
    out.push_str("    classDef processor fill:#ffffe0\n");
    out
}

fn tools_graph_text(tool_map: &BTreeMap<String, Vec<String>>) -> String {
    let mut lines = Vec::new();
    for (tool, procs) in tool_map {
        for proc in procs {
            lines.push(format!("{tool} \u{2192} {proc}"));
        }
    }
    lines.join("\n")
}

fn tools_graph_json(tool_map: &BTreeMap<String, Vec<String>>) -> Result<String> {
    let entries: Vec<crate::json_output::ToolListEntry> = tool_map
        .iter()
        .map(|(tool, procs)| {
            let info = crate::tools::tool_info(tool);
            let install_methods = info
                .map(|i| i.install_methods.iter().map(|m| {
                    crate::json_output::ToolInstallMethodEntry {
                        method: m.method.to_string(),
                        command: m.command(),
                    }
                }).collect())
                .unwrap_or_default();
            crate::json_output::ToolListEntry {
                tool: tool.clone(),
                installed: which::which(tool).is_ok(),
                runtime: info.map_or("unknown", |i| i.runtime).to_string(),
                processors: procs.clone(),
                install_methods,
            }
        })
        .collect();
    Ok(serde_json::to_string_pretty(&entries)?)
}

fn tools_graph_html(tool_map: &BTreeMap<String, Vec<String>>) -> String {
    let mermaid_content = tools_graph_mermaid(tool_map);
    format!(r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>RSConstruct Tools Graph</title>
    <script src="https://cdn.jsdelivr.net/npm/mermaid/dist/mermaid.min.js"></script>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            margin: 40px;
            background: #f5f5f5;
        }}
        h1 {{
            color: #333;
        }}
        .mermaid {{
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }}
    </style>
</head>
<body>
    <h1>RSConstruct Tools Graph</h1>
    <div class="mermaid">
{mermaid_content}
    </div>
    <script>
        mermaid.initialize({{ startOnLoad: true, theme: 'default', maxTextSize: 500000 }});
    </script>
</body>
</html>
"#)
}

fn tools_graph_svg(
    ctx: &crate::build_context::BuildContext,
    tool_map: &BTreeMap<String, Vec<String>>,
) -> Result<String> {
    crate::processors::dot_to_svg(ctx, &tools_graph_dot(tool_map))
}

#[cfg(test)]
mod dpkg_status_tests {
    use super::dpkg_status_means_installed;

    #[test]
    fn installed_package_is_installed() {
        assert!(dpkg_status_means_installed("install ok installed"));
        assert!(dpkg_status_means_installed("install ok installed\n"));
    }

    /// The regression this guards: dpkg-query exits 0 for a package it merely
    /// knows about, so the exit code alone reported aspell-en as satisfied on
    /// a runner that did not have it.
    #[test]
    fn known_but_absent_package_is_not_installed() {
        assert!(!dpkg_status_means_installed("unknown ok not-installed"));
        assert!(!dpkg_status_means_installed("deinstall ok config-files"));
        assert!(!dpkg_status_means_installed("install ok half-configured"));
        assert!(!dpkg_status_means_installed(""));
    }
}
