use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output, anchor_display_dir};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CcLibraryDef {
    pub name: String,
    #[serde(default = "default_cc_lib_type")]
    pub lib_type: String,
    pub sources: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
}

fn default_cc_lib_type() -> String {
    "shared".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CcProgramDef {
    pub name: String,
    pub sources: Vec<String>,
    #[serde(default)]
    pub link: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
}

/// Parsed contents of a cc.yaml file, resolved against the `[processor.cc]`
/// config: the six inheritable fields (`cc`, `cxx`, `cflags`, `cxxflags`,
/// `ldflags`, `include_dirs`) default to the config's values, so
/// `rsconstruct.toml` sets project-wide defaults and each cc.yaml overrides
/// per directory. Construct via [`CcManifest::parse`] — the raw serde shape
/// lives in `CcManifestRaw` so "field absent" (inherit) is distinguishable
/// from "field explicitly set".
#[derive(Debug, Clone)]
pub struct CcManifest {
    pub cc: String,
    pub cxx: String,
    pub cflags: Vec<String>,
    pub cxxflags: Vec<String>,
    pub ldflags: Vec<String>,
    pub include_dirs: Vec<String>,
    pub libraries: Vec<CcLibraryDef>,
    pub programs: Vec<CcProgramDef>,
}

/// The raw serde shape of cc.yaml. Inheritable fields are optional; absent
/// means "inherit the `[processor.cc]` config value".
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CcManifestRaw {
    cc: Option<String>,
    cxx: Option<String>,
    cflags: Option<Vec<String>>,
    cxxflags: Option<Vec<String>>,
    ldflags: Option<Vec<String>>,
    include_dirs: Option<Vec<String>>,
    #[serde(default)]
    libraries: Vec<CcLibraryDef>,
    #[serde(default)]
    programs: Vec<CcProgramDef>,
}

impl CcManifest {
    /// Parse cc.yaml content, inheriting unset fields from the config.
    pub fn parse(content: &str, defaults: &CcConfig) -> Result<Self, serde_yml::Error> {
        let raw: CcManifestRaw = serde_yml::from_str(content)?;
        Ok(Self {
            cc: raw.cc.unwrap_or_else(|| defaults.cc.clone()),
            cxx: raw.cxx.unwrap_or_else(|| defaults.cxx.clone()),
            cflags: raw.cflags.unwrap_or_else(|| defaults.cflags.clone()),
            cxxflags: raw.cxxflags.unwrap_or_else(|| defaults.cxxflags.clone()),
            ldflags: raw.ldflags.unwrap_or_else(|| defaults.ldflags.clone()),
            include_dirs: raw.include_dirs.unwrap_or_else(|| defaults.include_dirs.clone()),
            libraries: raw.libraries,
            programs: raw.programs,
        })
    }
}

/// CC (full C/C++ project) config. Custom: cc, cxx, cflags, cxxflags, ldflags, `include_dirs`, `single_invocation`, `cache_output_dir`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CcConfig {
    #[serde(default = "crate::config::default_cc_compiler")]
    pub cc: String,
    #[serde(default = "crate::config::default_cxx_compiler")]
    pub cxx: String,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub single_invocation: bool,
    #[serde(default = "crate::config::default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for CcConfig {
    fn default() -> Self {
        Self {
            cc: "gcc".into(),
            cxx: "g++".into(),
            cflags: Vec::new(),
            cxxflags: Vec::new(),
            ldflags: Vec::new(),
            include_dirs: Vec::new(),
            single_invocation: false,
            cache_output_dir: true,
            standard: StandardConfig::default(),
        }
    }
}

pub struct CcProcessor {
    config: CcConfig,
    /// Compilers the discovered cc.yaml manifests actually resolve to.
    /// Filled during `discover` so `required_tools()` names the real
    /// compilers by execution time (the debug declared-tools assertion
    /// checks against it), not just the config defaults.
    manifest_compilers: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl CcProcessor {
    pub const fn new(config: CcConfig) -> Self {
        Self {
            config,
            manifest_compilers: std::sync::Mutex::new(std::collections::BTreeSet::new()),
        }
    }

    /// Determine whether a source file is C++ based on extension.
    fn is_cxx(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("cc" | "cpp" | "cxx" | "C")
        )
    }

    /// Choose the compiler for a source file.
    fn compiler_for(manifest: &CcManifest, source: &Path) -> String {
        if Self::is_cxx(source) {
            manifest.cxx.clone()
        } else {
            manifest.cc.clone()
        }
    }

    /// Choose cflags or cxxflags for a source file.
    fn lang_flags_for<'a>(manifest: &'a CcManifest, source: &Path) -> &'a [String] {
        if Self::is_cxx(source) {
            &manifest.cxxflags
        } else {
            &manifest.cflags
        }
    }

    /// Parse a cc.yaml file, inheriting unset fields from the
    /// `[processor.cc]` config (project-wide defaults, per-directory
    /// overrides).
    fn parse_manifest(&self, yaml_path: &Path) -> Result<CcManifest> {
        let content = fs::read_to_string(yaml_path)
            .with_context(|| format!("Failed to read {}", yaml_path.display()))?;
        let manifest = CcManifest::parse(&content, &self.config)
            .with_context(|| format!("Failed to parse {}", yaml_path.display()))?;
        Ok(manifest)
    }

    /// Compile a single source file to an object file.
    /// All paths are relative to the project root.
    fn compile_object(ctx: &crate::build_context::BuildContext, manifest: &CcManifest, source: &Path, obj: &Path, extra_cflags: &[String]) -> Result<()> {
        crate::processors::ensure_output_dir(obj)?;
        let compiler = Self::compiler_for(manifest, source);
        let mut cmd = Command::new(&compiler);
        cmd.arg("-c");
        for flag in Self::lang_flags_for(manifest, source) {
            cmd.arg(flag);
        }
        for flag in extra_cflags {
            cmd.arg(flag);
        }
        cmd.arg("-o").arg(obj).arg(source);
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("{} -c {}", compiler, source.display()))
    }

    /// Build a static library from object files.
    fn build_static_lib(ctx: &crate::build_context::BuildContext, lib_path: &Path, objects: &[PathBuf]) -> Result<()> {
        crate::processors::ensure_output_dir(lib_path)?;
        let mut cmd = Command::new("ar");
        cmd.arg("rcs").arg(lib_path);
        for obj in objects {
            cmd.arg(obj);
        }
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("ar rcs {}", lib_path.display()))
    }

    /// Object file path for a source, mirroring the manifest-relative source
    /// path under the target's obj dir so equal stems in different directories
    /// (`src1/util.c`, `src2/util.c`) cannot collide. `..`/`.` components are
    /// dropped to keep the object inside the obj dir.
    fn object_path_for(target_obj_dir: &Path, source_rel: &str) -> PathBuf {
        let mut rel: PathBuf = Path::new(source_rel)
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(p) => Some(p),
                _ => None,
            })
            .collect();
        rel.set_extension("o");
        target_obj_dir.join(rel)
    }

    /// Build a shared library from object files.
    /// `has_cxx` selects the C++ driver, which C++ objects need at link time.
    fn build_shared_lib(ctx: &crate::build_context::BuildContext, manifest: &CcManifest, lib_path: &Path, objects: &[PathBuf], ldflags: &[String], has_cxx: bool) -> Result<()> {
        crate::processors::ensure_output_dir(lib_path)?;
        let compiler = if has_cxx { &manifest.cxx } else { &manifest.cc };
        let mut cmd = Command::new(compiler);
        cmd.arg("-shared").arg("-o").arg(lib_path);
        for obj in objects {
            cmd.arg(obj);
        }
        for flag in &manifest.ldflags {
            cmd.arg(flag);
        }
        for flag in ldflags {
            cmd.arg(flag);
        }
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("{} -shared -o {}", compiler, lib_path.display()))
    }

    /// Link object files into an executable.
    /// `has_cxx` selects the C++ driver, which C++ objects need at link time.
    ///
    /// 8 arguments against clippy's limit of 7 — each is a distinct part of
    /// a linker invocation, and the natural grouping (`CcManifest`) is
    /// already one of them.
    #[allow(clippy::too_many_arguments)]
    fn link_program(ctx: &crate::build_context::BuildContext, manifest: &CcManifest, exe_path: &Path, objects: &[PathBuf], lib_dir: &Path, link_libs: &[String], ldflags: &[String], has_cxx: bool) -> Result<()> {
        crate::processors::ensure_output_dir(exe_path)?;
        let compiler = if has_cxx { &manifest.cxx } else { &manifest.cc };
        let mut cmd = Command::new(compiler);
        cmd.arg("-o").arg(exe_path);
        for obj in objects {
            cmd.arg(obj);
        }
        if !link_libs.is_empty() {
            cmd.arg(format!("-L{}", lib_dir.display()));
            for lib in link_libs {
                cmd.arg(format!("-l{lib}"));
            }
        }
        for flag in &manifest.ldflags {
            cmd.arg(flag);
        }
        for flag in ldflags {
            cmd.arg(flag);
        }
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("{} -o {}", compiler, exe_path.display()))
    }

    /// Single-invocation build for a program (all sources in one command).
    fn single_invocation_program(ctx: &crate::build_context::BuildContext, manifest: &CcManifest, exe_path: &Path, sources: &[PathBuf], lib_dir: &Path, link_libs: &[String], ldflags: &[String]) -> Result<()> {
        crate::processors::ensure_output_dir(exe_path)?;
        let has_cxx = sources.iter().any(|s| Self::is_cxx(s));
        let compiler = if has_cxx { &manifest.cxx } else { &manifest.cc };
        let mut cmd = Command::new(compiler);
        let global_flags = if has_cxx { &manifest.cxxflags } else { &manifest.cflags };
        for flag in global_flags {
            cmd.arg(flag);
        }
        cmd.arg("-o").arg(exe_path);
        for source in sources {
            cmd.arg(source);
        }
        if !link_libs.is_empty() {
            cmd.arg(format!("-L{}", lib_dir.display()));
            for lib in link_libs {
                cmd.arg(format!("-l{lib}"));
            }
        }
        for flag in &manifest.ldflags {
            cmd.arg(flag);
        }
        for flag in ldflags {
            cmd.arg(flag);
        }
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("{} -o {}", compiler, exe_path.display()))
    }

    /// Compute the output directory for a cc.yaml file.
    /// Output goes under out/cc/<relative-path-to-cc.yaml-dir>/.
    fn output_dir_for(yaml_path: &Path) -> PathBuf {
        let anchor_dir = crate::processors::parent_dir_or_empty(yaml_path);
        if anchor_dir.as_os_str().is_empty() {
            PathBuf::from("out/cc")
        } else {
            Path::new("out/cc").join(anchor_dir)
        }
    }

    /// Execute a full cc.yaml build.
    /// All commands run from the project root. Manifest paths are resolved
    /// to project-root-relative paths using the cc.yaml's parent directory.
    fn execute_build(&self, ctx: &crate::build_context::BuildContext, yaml_path: &Path) -> Result<()> {
        let manifest = self.parse_manifest(yaml_path)?;
        let anchor_dir = crate::processors::parent_dir_or_empty(yaml_path);
        let output_dir = Self::output_dir_for(yaml_path);
        let obj_dir = output_dir.join("obj");
        let lib_dir = output_dir.join("lib");
        let bin_dir = output_dir.join("bin");

        // include_dirs are relative to the project root (not the cc.yaml directory)
        let resolved_include_flags: Vec<String> = manifest.include_dirs.iter()
            .map(|dir| format!("-I{dir}"))
            .collect();

        // Build libraries
        for lib in &manifest.libraries {
            let build_shared = matches!(lib.lib_type.as_str(), "shared" | "both");
            let build_static = matches!(lib.lib_type.as_str(), "static" | "both");

            let mut extra_cflags: Vec<String> = lib.cflags.clone();
            if build_shared {
                extra_cflags.push("-fPIC".into());
            }
            for dir in &lib.include_dirs {
                extra_cflags.push(format!("-I{dir}"));
            }
            extra_cflags.extend_from_slice(&resolved_include_flags);

            let target_obj_dir = obj_dir.join(&lib.name);
            let mut objects = Vec::new();
            for source_str in &lib.sources {
                let source = crate::processors::resolve_anchor_path(anchor_dir, source_str);
                let obj = Self::object_path_for(&target_obj_dir, source_str);
                Self::compile_object(ctx, &manifest, &source, &obj, &extra_cflags)?;
                objects.push(obj);
            }
            let has_cxx = lib.sources.iter().any(|s| Self::is_cxx(Path::new(s)));

            if build_static {
                let lib_path = lib_dir.join(format!("lib{}.a", lib.name));
                Self::build_static_lib(ctx, &lib_path, &objects)?;
            }
            if build_shared {
                let lib_path = lib_dir.join(format!("lib{}.so", lib.name));
                Self::build_shared_lib(ctx, &manifest, &lib_path, &objects, &lib.ldflags, has_cxx)?;
            }
        }

        // Build programs
        for prog in &manifest.programs {
            let exe_path = bin_dir.join(&prog.name);

            // Resolve source paths
            let sources: Vec<PathBuf> = prog.sources.iter()
                .map(|s| crate::processors::resolve_anchor_path(anchor_dir, s))
                .collect();

            if self.config.single_invocation {
                Self::single_invocation_program(ctx, &manifest, &exe_path, &sources, &lib_dir, &prog.link, &prog.ldflags)?;
            } else {
                let target_obj_dir = obj_dir.join(&prog.name);
                let mut objects = Vec::new();

                let mut extra_cflags: Vec<String> = prog.cflags.clone();
                for dir in &prog.include_dirs {
                    extra_cflags.push(format!("-I{dir}"));
                }
                extra_cflags.extend_from_slice(&resolved_include_flags);

                for (source_str, source) in prog.sources.iter().zip(&sources) {
                    let obj = Self::object_path_for(&target_obj_dir, source_str);
                    Self::compile_object(ctx, &manifest, source, &obj, &extra_cflags)?;
                    objects.push(obj);
                }
                let has_cxx = sources.iter().any(|s| Self::is_cxx(s));
                Self::link_program(ctx, &manifest, &exe_path, &objects, &lib_dir, &prog.link, &prog.ldflags, has_cxx)?;
            }
        }

        Ok(())
    }
}

impl Processor for CcProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean_output_dir(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        // The config values are project-wide defaults; each cc.yaml may
        // override the compiler per directory. Discovery records what the
        // manifests actually resolve to, so by execution time this names
        // the real compilers, not just the defaults.
        let mut tools: Vec<String> = vec![self.config.cc.clone(), self.config.cxx.clone(), "ar".to_string()];
        for compiler in self.manifest_compilers.lock().unwrap().iter() {
            if !tools.contains(compiler) {
                tools.push(compiler.clone());
            }
        }
        tools
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let Some(files) = crate::processors::scan_or_skip(&self.config.standard, file_index) else {
            return Ok(());
        };
        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        for yaml_path in files {
            let manifest = match self.parse_manifest(&yaml_path) {
                Ok(m) => m,
                Err(e) => {
                    anyhow::bail!("Failed to parse {}: {}", yaml_path.display(), e);
                }
            };

            // Record the compilers this manifest resolves to, so
            // required_tools() covers per-manifest overrides.
            {
                let mut compilers = self.manifest_compilers.lock().unwrap();
                compilers.insert(manifest.cc.clone());
                compilers.insert(manifest.cxx.clone());
            }

            // Source paths in the manifest are relative to the cc.yaml directory.
            // Resolve to project-root-relative paths for the build graph.
            let anchor_dir = crate::processors::parent_dir_or_empty(&yaml_path);

            let mut inputs: Vec<PathBuf> = Vec::new();
            inputs.push(yaml_path.clone());

            for lib in &manifest.libraries {
                for source in &lib.sources {
                    inputs.push(crate::processors::resolve_anchor_path(anchor_dir, source));
                }
            }
            for prog in &manifest.programs {
                for source in &prog.sources {
                    inputs.push(crate::processors::resolve_anchor_path(anchor_dir, source));
                }
            }

            inputs.extend_from_slice(&extra);

            if self.config.cache_output_dir {
                let output_dir = Self::output_dir_for(&yaml_path);
                graph.add_product_with_output_dir(
                    inputs, vec![], instance_name, hash.clone(), output_dir,
                )?;
            } else {
                graph.add_product(inputs, vec![], instance_name, hash.clone())?;
            }
        }
        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let yaml_path = product.primary_input();
        let display_dir = anchor_display_dir(yaml_path);
        self.execute_build(ctx, yaml_path)
            .with_context(|| format!("cc build failed in {display_dir}"))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(CcProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "cc",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "cc", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Default C compiler executable (overridable per cc.yaml)" },
            crate::config::FieldSpec { name: "cxx", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Default C++ compiler executable (overridable per cc.yaml)" },
            crate::config::FieldSpec { name: "cflags", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Default C compiler flags (overridable per cc.yaml)" },
            crate::config::FieldSpec { name: "cxxflags", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Default C++ compiler flags (overridable per cc.yaml)" },
            crate::config::FieldSpec { name: "ldflags", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Default linker flags (overridable per cc.yaml)" },
            crate::config::FieldSpec { name: "include_dirs", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Default header search directories (overridable per cc.yaml)" },
            crate::config::FieldSpec { name: "single_invocation", ty: crate::config::FieldType::Bool,
                affects_output: true, required: false,
                doc: "Compile all sources in one compiler call" },
            crate::config::FieldSpec { name: "cache_output_dir", ty: crate::config::FieldType::Bool,
                affects_output: false, required: false,
                doc: "Cache the entire output directory as a unit" },
        ],
        omit_standard_fields: &["command", "formats", "args", "dep_auto", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &["cc.yaml"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<CcConfig>,
        keywords: &["c", "cpp", "builder", "compiler", "gcc", "clang", "cc", "h", "hpp"],
        description: "Build C/C++ projects from cc.yaml manifests",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
