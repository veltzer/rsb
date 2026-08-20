use anyhow::{Context, Result};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output, anchor_display_dir};

/// A single kernel module definition inside linux-module.yaml.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LinuxModuleModuleDef {
    pub name: String,
    pub sources: Vec<String>,
    #[serde(default)]
    pub extra_cflags: Vec<String>,
}

/// Parsed contents of a linux-module.yaml file.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LinuxModuleManifest {
    #[serde(default = "default_make_tool")]
    pub make: String,
    #[serde(default)]
    pub kdir: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub cross_compile: Option<String>,
    #[serde(default = "default_linux_module_v")]
    pub v: u32,
    #[serde(default = "default_linux_module_w")]
    pub w: u32,
    pub modules: Vec<LinuxModuleModuleDef>,
}

fn default_make_tool() -> String {
    "make".into()
}

const fn default_linux_module_v() -> u32 {
    0
}

const fn default_linux_module_w() -> u32 {
    1
}

/// Linux module config. No custom fields.
/// Unused `StandardConfig` fields: command, formats, `output_dir`, args.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct LinuxModuleConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}

pub struct LinuxModuleProcessor {
    config: LinuxModuleConfig,
}

impl LinuxModuleProcessor {
    pub const fn new(config: LinuxModuleConfig) -> Self {
        Self {
            config,
        }
    }

    /// Parse a linux-module.yaml file.
    fn parse_manifest(yaml_path: &Path) -> Result<LinuxModuleManifest> {
        let content = fs::read_to_string(yaml_path)
            .with_context(|| format!("Failed to read {}", yaml_path.display()))?;
        let manifest: LinuxModuleManifest = serde_yml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", yaml_path.display()))?;
        Ok(manifest)
    }

    /// Compute the output directory for a linux-module.yaml file.
    fn output_dir_for(yaml_path: &Path) -> PathBuf {
        let anchor_dir = crate::processors::parent_dir_or_empty(yaml_path);
        if anchor_dir.as_os_str().is_empty() {
            PathBuf::from("out/linux-module")
        } else {
            Path::new("out/linux-module").join(anchor_dir)
        }
    }

    /// Get the default KDIR path from the running kernel. Fails when the
    /// kernel release cannot be determined — commands run without a shell, so
    /// a literal `$(uname -r)` fallback would never expand and `make -C`
    /// would fail with a baffling path.
    fn default_kdir(ctx: &crate::build_context::BuildContext) -> Result<String> {
        let mut cmd = Command::new("uname");
        cmd.arg("-r");
        let output = crate::processors::run_command_capture(ctx, &cmd)
            .context("Failed to run 'uname -r' to locate kernel build directory (set 'kdir' in linux-module.yaml to override)")?;
        if !output.status.success() {
            anyhow::bail!(
                "'uname -r' failed while locating the kernel build directory (set 'kdir' in linux-module.yaml to override): {}",
                String::from_utf8_lossy(&output.stderr).trim_end()
            );
        }
        let release = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(format!("/lib/modules/{release}/build"))
    }

    /// Generate a Kbuild file for building a kernel module.
    fn write_kbuild(module_dir: &Path, module: &LinuxModuleModuleDef) -> Result<()> {
        let mut content = format!("obj-m := {}.o\n", module.name);

        let objs: Vec<String> = module.sources.iter()
            .map(|s| {
                let p = Path::new(s);
                let stem = p.file_stem().unwrap_or_default().to_string_lossy();
                format!("{stem}.o")
            })
            .collect();
        let _ = writeln!(content, "{}-objs := {}", module.name, objs.join(" "));

        if !module.extra_cflags.is_empty() {
            let _ = writeln!(content, "ccflags-y := {}", module.extra_cflags.join(" "));
        }

        fs::write(module_dir.join("Kbuild"), &content)
            .with_context(|| format!("Failed to write Kbuild in {}", module_dir.display()))?;
        Ok(())
    }

    /// Build a single kernel module. Runs make in the module's source directory.
    fn build_module(ctx: &crate::build_context::BuildContext, manifest: &LinuxModuleManifest, anchor_dir: &Path, module: &LinuxModuleModuleDef, output_dir: &Path) -> Result<()> {
        let cwd = std::env::current_dir()
            .context("Failed to get current directory for linux module build")?;
        let module_dir = if anchor_dir.as_os_str().is_empty() {
            cwd
        } else {
            cwd.join(anchor_dir)
        };

        let kdir = match manifest.kdir.clone() {
            Some(kdir) => kdir,
            None => Self::default_kdir(ctx)?,
        };

        Self::write_kbuild(&module_dir, module)?;

        let mut cmd = Command::new(&manifest.make);
        cmd.arg("-C").arg(&kdir);
        if let Some(ref arch) = manifest.arch {
            cmd.arg(format!("ARCH={arch}"));
        }
        if let Some(ref cross) = manifest.cross_compile {
            cmd.arg(format!("CROSS_COMPILE={cross}"));
        }
        cmd.arg(format!("M={}", module_dir.display()));
        cmd.arg(format!("V={}", manifest.v));
        cmd.arg(format!("W={}", manifest.w));
        cmd.arg("modules");
        cmd.current_dir(&module_dir);

        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("make modules for {}", module.name))?;

        // Read the built .ko into memory. A successful make that produced no
        // .ko (e.g. module name mismatch with obj-m) must fail here, not later
        // as a missing declared output. We read the bytes now — before the
        // `make clean` below — because kbuild's clean recursively deletes every
        // .ko under `M=<module_dir>`. When the manifest sits at the repo root,
        // module_dir is the repo root and the output directory (out/…) lives
        // inside it, so cleaning after copying would wipe the copy we just made.
        // Capturing the bytes first makes the output survive clean regardless of
        // where output_dir sits relative to module_dir.
        let ko_name = format!("{}.ko", module.name);
        let ko_src = module_dir.join(&ko_name);
        let ko_bytes = fs::read(&ko_src).map_err(|e| anyhow::anyhow!(
            "make reported success but did not produce {} (check the module name in linux-module.yaml): {}",
            ko_src.display(), e
        ))?;
        let ko_mode = fs::metadata(&ko_src).ok().map(|m| crate::platform::get_mode(&m));

        // Clean up build artifacts from the source directory. Failures leave
        // the source tree polluted — report them. This may delete the source
        // .ko (and any .ko already written under module_dir), which is why the
        // output is written from the in-memory bytes afterwards.
        let mut clean_cmd = Command::new(&manifest.make);
        clean_cmd.arg("-C").arg(&kdir);
        if let Some(ref arch) = manifest.arch {
            clean_cmd.arg(format!("ARCH={arch}"));
        }
        if let Some(ref cross) = manifest.cross_compile {
            clean_cmd.arg(format!("CROSS_COMPILE={cross}"));
        }
        clean_cmd.arg(format!("M={}", module_dir.display()));
        clean_cmd.arg("clean");
        clean_cmd.current_dir(&module_dir);
        let clean_output = run_command(ctx, &clean_cmd)?;
        check_command_output(&clean_output, format_args!("make clean for {}", module.name))?;

        // Remove the Kbuild we generated
        fs::remove_file(module_dir.join("Kbuild"))
            .with_context(|| format!("Failed to remove generated Kbuild in {}", module_dir.display()))?;

        // Write the captured .ko to the output directory, after clean, so it
        // cannot be swept away by the clean above.
        crate::errors::ctx(fs::create_dir_all(output_dir), &format!("Failed to create output dir: {}", output_dir.display()))?;
        let ko_dst = output_dir.join(&ko_name);
        fs::write(&ko_dst, &ko_bytes)
            .with_context(|| format!("Failed to write {ko_name} to output"))?;
        if let Some(mode) = ko_mode {
            crate::platform::set_permissions_mode(&ko_dst, mode)
                .with_context(|| format!("Failed to set mode on {}", ko_dst.display()))?;
        }

        Ok(())
    }

    /// Execute a full linux-module.yaml build.
    fn execute_build(&self, ctx: &crate::build_context::BuildContext, yaml_path: &Path) -> Result<()> {
        let manifest = Self::parse_manifest(yaml_path)?;
        let anchor_dir = crate::processors::parent_dir_or_empty(yaml_path);
        let output_dir = Self::output_dir_for(yaml_path);

        for module in &manifest.modules {
            Self::build_module(ctx, &manifest, anchor_dir, module, &output_dir)?;
        }

        Ok(())
    }
}

impl Processor for LinuxModuleProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        // uname is used to locate the running kernel's build directory when
        // the manifest doesn't set `kdir`. It went undeclared for as long as
        // it bypassed the central runner — exactly the drift the declared-
        // tools check exists to catch.
        vec!["make".to_string(), "uname".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let Some(files) = crate::processors::scan_or_skip(&self.config.standard, file_index) else {
            return Ok(());
        };
        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        for yaml_path in files {
            let manifest = match Self::parse_manifest(&yaml_path) {
                Ok(m) => m,
                Err(e) => {
                    anyhow::bail!("Failed to parse {}: {}", yaml_path.display(), e);
                }
            };

            let anchor_dir = crate::processors::parent_dir_or_empty(&yaml_path);
            let output_dir = Self::output_dir_for(&yaml_path);

            let mut inputs: Vec<PathBuf> = Vec::new();
            inputs.push(yaml_path.clone());

            let mut outputs: Vec<PathBuf> = Vec::new();

            for module in &manifest.modules {
                for source in &module.sources {
                    inputs.push(crate::processors::resolve_anchor_path(anchor_dir, source));
                }
                outputs.push(output_dir.join(format!("{}.ko", module.name)));
            }

            inputs.extend_from_slice(&extra);

            graph.add_product(inputs, outputs, instance_name, hash.clone())?;
        }
        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let yaml_path = product.primary_input();
        let display_dir = anchor_display_dir(yaml_path);
        self.execute_build(ctx, yaml_path)
            .with_context(|| format!("linux_module build failed in {display_dir}"))
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(LinuxModuleProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "linux_module",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        fields: &[],
        omit_standard_fields: &[],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &["linux-module.yaml"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<LinuxModuleConfig>,
        keywords: &["c", "linux", "kernel", "module", "builder"],
        description: "Build Linux kernel modules from linux-module.yaml manifests",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
