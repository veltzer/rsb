//! marp generator — custom Processor impl backed by [`MarpConfig`].
//!
//! Concurrency note: marp-cli launches headless Chrome under
//! `$TMPDIR/marp-cli-<random>/`. Multiple marp processes running in parallel
//! create sibling dirs under the same `$TMPDIR`. An earlier version of this
//! file ran a "best effort" cleanup that walked `/tmp` and removed every
//! `marp-cli-*` directory it found — which raced with concurrent invocations,
//! pulling working files out from under a still-running Chrome and producing
//! SIGTRAP crashes that surfaced to puppeteer as `TargetCloseError`.
//!
//! To eliminate the race, each invocation gets its own private `TMPDIR` so
//! marp's `marp-cli-<random>` dir lands in a per-invocation namespace. The
//! whole `TMPDIR` is then removed after marp exits — no shared cleanup.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use serde::{Deserialize, Serialize};

const fn default_marp_timeout_secs() -> u64 { 20 }
const fn default_marp_max_attempts() -> u32 { 3 }

/// Marp config. Adds tunables for the per-invocation timeout and the retry
/// budget, since marp-cli occasionally hangs under chromium-headless and the
/// right values are project-dependent.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MarpConfig {
    /// Per-invocation wall-clock timeout in seconds.
    #[serde(default = "default_marp_timeout_secs")]
    pub timeout_secs: u64,
    /// Total attempts (including the first) when marp times out. Set to 1 to
    /// disable retrying.
    #[serde(default = "default_marp_max_attempts")]
    pub max_attempts: u32,
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl Default for MarpConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_marp_timeout_secs(),
            max_attempts: default_marp_max_attempts(),
            standard: StandardConfig::default(),
        }
    }
}
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{run_command_with_timeout, check_command_output, ensure_output_dir, Processor};

fn is_transient_marp_error(err: &anyhow::Error) -> bool {
    let s = err.to_string();
    // Wall-clock timeout (puppeteer hung) or Chrome's CDP socket dropped
    // (typically because the headless browser crashed). Both are retryable.
    s.contains("Command timed out after") || s.contains("TargetCloseError")
}

/// Allocate a unique, empty temp directory under the system tmpdir. Caller
/// owns the path and is responsible for `fs::remove_dir_all` on completion.
fn make_invocation_tmpdir() -> Result<PathBuf> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let base = std::env::temp_dir();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = base.join(format!("rsc-marp-{pid}-{ns}-{seq}"));
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create marp scratch dir: {}", dir.display()))?;
    Ok(dir)
}

/// Remove a single scratch dir. Errors are intentionally ignored: cleanup is
/// best-effort, and a leaked tmpdir is preferable to surfacing a spurious
/// error after a successful build.
fn remove_scratch_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

pub struct MarpProcessor {
    config: MarpConfig,
}

impl MarpProcessor {
    pub const fn new(config: MarpConfig) -> Self {
        Self { config }
    }
}

impl Processor for MarpProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone(), "node".into()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let params = super::DiscoverParams {
            scan: &self.config.standard,
            dep_inputs: &self.config.standard.dep_inputs,
            config: &self.config,
            output_dir: &self.config.standard.output_dir,
            processor_name: instance_name,
            checksum_fields: crate::config::checksum_fields_of(instance_name),
        };
        super::discover_multi_format(graph, file_index, &params, &self.config.standard.formats)
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let input = product.primary_input();
        let output = product.primary_output();
        let format = output.extension()
            .context("marp output has no extension")?
            .to_string_lossy();
        ensure_output_dir(output)?;
        let command = self.config.standard.require_command("marp")?;
        // Both floored at 1, like each other: timeout_secs = 0 would make
        // every invocation time out instantly (× max_attempts retries).
        let timeout = Duration::from_secs(self.config.timeout_secs.max(1));
        let max_attempts = self.config.max_attempts.max(1);

        for attempt in 1..=max_attempts {
            // Fresh scratch dir per attempt so a failed run can't leave state
            // that confuses the retry, and so concurrent invocations can never
            // touch each other's working files.
            let scratch = make_invocation_tmpdir()?;

            let mut cmd = Command::new(command);
            cmd.env("TMPDIR", &scratch);
            if format != "html" {
                cmd.arg(format!("--{format}"));
            }
            cmd.arg("--output").arg(output);
            for arg in &self.config.standard.args { cmd.arg(arg); }
            cmd.arg(input);
            let result = run_command_with_timeout(ctx, &cmd, timeout)
                .and_then(|out| check_command_output(&out, format_args!("marp {}", input.display())));
            remove_scratch_dir(&scratch);
            match result {
                Ok(()) => return Ok(()),
                Err(err) => {
                    if !is_transient_marp_error(&err) || attempt == max_attempts {
                        return Err(err);
                    }
                    eprintln!(
                        "[marp] {} {} (attempt {}/{}), retrying",
                        input.display(),
                        if err.to_string().contains("TargetCloseError") { "Chrome crashed" } else { "timed out" },
                        attempt, max_attempts
                    );
                }
            }
        }
        unreachable!("loop exits via return on every iteration")
    }
}

fn create_marp(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(MarpProcessor::new(cfg)))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "marp", processor_type: crate::processors::ProcessorType::Generator, create: create_marp,
    fields: &[
        crate::config::FieldSpec { name: "timeout_secs", ty: crate::config::FieldType::Integer,
            affects_output: false, required: false,
            doc: "Per-invocation wall-clock timeout in seconds" },
        crate::config::FieldSpec { name: "max_attempts", ty: crate::config::FieldType::Integer,
            affects_output: false, required: false,
            doc: "Total attempts (including the first) when marp times out" },
    ],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "marp", output_dir: "out/marp", formats: &["pdf"],
        args: &["--html", "--allow-local-files"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<MarpConfig>,
    keywords: &["markdown", "presentation", "slides", "pdf", "html"],
    description: "Convert Marp Markdown presentations to PDF/HTML",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }

/// CI cap for marp: GitHub Actions runners (and similar 2-vCPU CI hosts) OOM
/// when running marp at full project parallelism. Equivalent to the user
/// passing `--pset marp.max_jobs=2`; only applies when the user hasn't set
/// `max_jobs` themselves and `CI=true` is in the environment.
#[allow(clippy::unnecessary_wraps)] // Result<()> is required by PhaseHook::run signature.
fn marp_ci_cap(config: &mut crate::config::Config) -> anyhow::Result<()> {
    if !std::env::var("CI").is_ok_and(|v| v == "true") {
        return Ok(());
    }
    for inst in config.processor.instances.iter_mut().filter(|i| i.type_name == "marp") {
        let Some(table) = inst.config_toml.as_table_mut() else { continue };
        if !table.contains_key("max_jobs") {
            table.insert("max_jobs".to_string(), toml::Value::Integer(2));
        }
    }
    Ok(())
}

inventory::submit! { crate::phases::PhaseHook {
    name: "marp_ci_cap",
    description: "When CI=true and marp.max_jobs is unset, cap it at 2",
    function: concat!(module_path!(), "::marp_ci_cap"),
    location: concat!(file!(), ":", line!()),
    run: marp_ci_cap,
} }
