use std::process::Command;
use anyhow::Result;
use serde::Serialize;
use crate::color;
use super::{Builder, sorted_keys};

#[derive(Serialize)]
struct DoctorCheck {
    name: String,
    status: &'static str,
    category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_hint: Option<String>,
}

impl Builder {
    /// Run diagnostic checks on the build environment.
    pub fn doctor(&self, ctx: &crate::build_context::BuildContext) -> Result<()> {
        let json_mode = crate::json_output::is_json_mode();
        let mut ok_count = 0usize;
        let mut fail_count = 0usize;
        let mut warn_count = 0usize;
        let mut checks: Vec<DoctorCheck> = Vec::new();

        let mut record = |name: String, status: &'static str, category: &'static str, detail: Option<String>, install_hint: Option<String>, ok: &mut usize, fail: &mut usize, warn: &mut usize| {
            match status {
                "ok" => *ok += 1,
                "fail" => *fail += 1,
                "warn" => *warn += 1,
                _ => {}
            }
            if !json_mode {
                let detail_str = detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default();
                let hint_str = install_hint.as_deref().map(|h| format!("  install: {h}")).unwrap_or_default();
                let tag: String = match status {
                    "ok" => color::green("[ok]").to_string(),
                    "fail" => color::red("[FAIL]").to_string(),
                    "warn" => color::yellow("[warn]").to_string(),
                    _ => status.to_string(),
                };
                println!("{tag} {name}{detail_str}{hint_str}");
            }
            checks.push(DoctorCheck { name, status, category, detail, install_hint });
        };

        // Check rsconstruct.toml
        if std::path::Path::new("rsconstruct.toml").exists() {
            record("rsconstruct.toml found and valid".to_string(), "ok", "config", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
        } else {
            record("rsconstruct.toml not found".to_string(), "fail", "config", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
        }

        // Check .rsconstructignore
        if std::path::Path::new(".rsconstructignore").exists() {
            record(".rsconstructignore found".to_string(), "ok", "config", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
        } else {
            record(".rsconstructignore not found (optional)".to_string(), "warn", "config", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
        }

        // Check tools for enabled processors
        let processors = self.create_processors()?;
        let mut checked_tools = std::collections::HashSet::new();

        for name in sorted_keys(&processors) {
            let processor = &processors[name];
            for tool in processor.required_tools() {
                if !checked_tools.insert(tool.clone()) {
                    continue;
                }
                if let Some(version) = tool_version(ctx, &tool) {
                    record(format!("{tool} available"), "ok", "tool", Some(version), None, &mut ok_count, &mut fail_count, &mut warn_count);
                } else {
                    let install_hint = crate::tools::tool_install_command(&tool);
                    record(format!("{tool} not found"), "fail", "tool", None, install_hint, &mut ok_count, &mut fail_count, &mut warn_count);
                }
            }
        }

        // Check declared dependencies. Python deps come from both
        // [dependencies].pip and pyproject.toml (the canonical list).
        let deps = &self.config.dependencies;
        let pip_deps = deps.effective_pip(std::path::Path::new("pyproject.toml"))?;
        if !deps.is_empty() || !pip_deps.is_empty() {
            if !json_mode {
                println!();
                println!("{}:", color::bold("Declared dependencies"));
            }

            for pkg in &deps.system {
                // A system dependency is a package, not a tool: ask the
                // package manager whether it is installed. which() is wrong
                // here — packages like aspell-en (a dictionary) ship no
                // binary named after themselves, so which() reported them
                // as missing even right after install-deps put them on.
                if super::tools::is_system_package_installed(ctx, pkg) {
                    record(format!("{pkg} (system)"), "ok", "dependency", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
                } else {
                    record(format!("{pkg} not found"), "fail", "dependency", Some("system".to_string()), Some("rsconstruct tools install-deps".to_string()), &mut ok_count, &mut fail_count, &mut warn_count);
                }
            }

            for pkg in &pip_deps {
                let name = crate::config::normalized_distribution_name(pkg);
                let found = package_installed(ctx, "pip", &["show", "--quiet", name.as_str()]);
                if found {
                    record(format!("{pkg} (pip)"), "ok", "dependency", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
                } else {
                    record(format!("{pkg} not installed"), "fail", "dependency", Some("pip".to_string()), Some(format!("pip install {pkg}")), &mut ok_count, &mut fail_count, &mut warn_count);
                }
            }

            for pkg in &deps.npm {
                let found = package_installed(ctx, "npm", &["list", "--depth=0", pkg]);
                if found {
                    record(format!("{pkg} (npm)"), "ok", "dependency", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
                } else {
                    record(format!("{pkg} not installed"), "fail", "dependency", Some("npm".to_string()), Some(format!("npm install {pkg}")), &mut ok_count, &mut fail_count, &mut warn_count);
                }
            }

            for pkg in &deps.gem {
                let found = package_installed(ctx, "gem", &["list", "--installed", "--exact", pkg]);
                if found {
                    record(format!("{pkg} (gem)"), "ok", "dependency", None, None, &mut ok_count, &mut fail_count, &mut warn_count);
                } else {
                    record(format!("{pkg} not installed"), "fail", "dependency", Some("gem".to_string()), Some(format!("gem install {pkg}")), &mut ok_count, &mut fail_count, &mut warn_count);
                }
            }
        }

        let total = ok_count + fail_count;

        if json_mode {
            let out = serde_json::json!({
                "checks": checks,
                "summary": {
                    "ok": ok_count,
                    "fail": fail_count,
                    "warn": warn_count,
                    "total": total,
                },
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            println!();
            let summary = format!("Summary: {ok_count}/{total} checks passed");
            if fail_count == 0 {
                println!("{}", color::green(&summary));
            } else {
                println!("{}", color::yellow(&summary));
            }
        }

        Ok(())
    }
}

/// Whether a package manager reports `pkg` as installed. One helper for the
/// pip/npm/gem probes, which were three copies of the same block.
fn package_installed(
    ctx: &crate::build_context::BuildContext,
    program: &str,
    args: &[&str],
) -> bool {
    let mut cmd = Command::new(program);
    cmd.args(args);
    crate::processors::run_command_capture(ctx, &cmd)
        .is_ok_and(|o| o.status.success())
}

/// Try to get the version string of a tool by running `tool --version`.
fn tool_version(ctx: &crate::build_context::BuildContext, tool: &str) -> Option<String> {
    let mut cmd = Command::new(tool);
    cmd.arg("--version");
    let output = crate::processors::run_command_capture(ctx, &cmd).ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let version = version.trim();
    if version.is_empty() {
        let version = String::from_utf8_lossy(&output.stderr);
        let first_line = version.lines().next().unwrap_or("").trim();
        if first_line.is_empty() {
            return Some("ok".to_string());
        }
        return Some(first_line.to_string());
    }
    Some(version.lines().next().unwrap_or(version).to_string())
}
