//! In-process C/C++ dependency analyzer (`icpp`).
//!
//! Uses a pure-Rust regex scanner to find `#include` directives — no external tools.
//! For projects that need compiler-accurate scanning (macros, conditional includes),
//! use the `cpp` analyzer instead.

use anyhow::Result;
use indicatif::ProgressBar;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::IcppAnalyzerConfig;
use crate::errors;
use crate::deps_cache::DepsCache;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};

use super::DepAnalyzer;

const CPP_MATCH_EXTENSIONS: &[&str] = &["c", "cc", "cpp", "cxx"];

/// In-process C/C++ dependency analyzer using a pure-Rust regex scanner.
pub struct IcppDepAnalyzer {
    iname: String,
    config: IcppAnalyzerConfig,
    verbose: bool,
    /// Cached include paths discovered from pkg-config
    pkg_config_include_paths: OnceLock<Vec<PathBuf>>,
    /// Cached include paths from `include_path_commands`
    command_include_paths: OnceLock<Vec<PathBuf>>,
}

impl IcppDepAnalyzer {
    pub fn new(iname: &str, config: IcppAnalyzerConfig, verbose: bool) -> Self {
        Self {
            iname: iname.to_string(),
            config,
            verbose,
            pkg_config_include_paths: OnceLock::new(),
            command_include_paths: OnceLock::new(),
        }
    }

    fn is_excluded(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        self.config.src_exclude_dirs.iter().any(|seg| path_str.contains(seg))
    }

    /// Query pkg-config for include paths (lazy, cached).
    fn get_pkg_config_include_paths(&self, ctx: &crate::build_context::BuildContext) -> &[PathBuf] {
        self.pkg_config_include_paths.get_or_init(|| {
            super::query_pkg_config_include_paths(ctx, "icpp", &self.config.pkg_config, self.verbose)
        })
    }

    /// Run configured `include_path_commands` to get additional include paths (lazy, cached).
    fn get_command_include_paths(&self, ctx: &crate::build_context::BuildContext) -> &[PathBuf] {
        self.command_include_paths.get_or_init(|| {
            super::run_include_path_commands(ctx, "icpp", &self.config.include_path_commands, self.verbose)
        })
    }

    /// Resolve a single `#include` directive to a file, if any.
    /// Searches in order: including file's directory, configured `include_paths`,
    /// pkg-config-discovered include paths, then include paths from configured commands.
    fn resolve_include(&self, ctx: &crate::build_context::BuildContext, include: &str, including_dir: &Path) -> Option<PathBuf> {
        let candidate = including_dir.join(include);
        if candidate.is_file() {
            return Some(candidate);
        }
        for inc_dir in &self.config.include_paths {
            let candidate = Path::new(inc_dir).join(include);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        for inc_dir in self.get_pkg_config_include_paths(ctx) {
            let candidate = inc_dir.join(include);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        for inc_dir in self.get_command_include_paths(ctx) {
            let candidate = inc_dir.join(include);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// Scan a single file for `#include` directives. Returns resolved dep paths.
    /// Errors if a `"quoted"` include can't be resolved (system headers via `<angle>`
    /// are allowed to be unresolved — they may live in system include paths).
    fn scan_file_includes(&self, ctx: &crate::build_context::BuildContext, source: &Path) -> Result<Vec<PathBuf>> {
        let content = errors::ctx(fs::read_to_string(source), &format!("Failed to read {}", source.display()))?;

        static INCLUDE_RE: OnceLock<Regex> = OnceLock::new();
        let re = INCLUDE_RE.get_or_init(|| {
            // Capture group 1: opening delimiter ("" or "<"), group 2: include path
            Regex::new(r#"^\s*#\s*include\s*(["<])([^>"]+)[>"]"#).expect(errors::INVALID_REGEX)
        });

        let parent = crate::processors::parent_dir_or_empty(source);
        let mut deps = Vec::new();
        for line in content.lines() {
            if let Some(caps) = re.captures(line) {
                let is_quoted = &caps[1] == "\"";
                let include = &caps[2];
                if !is_quoted && !self.config.follow_angle_brackets {
                    continue;
                }
                match self.resolve_include(ctx, include, parent) {
                    Some(resolved) => deps.push(resolved),
                    None if is_quoted && !self.config.skip_not_found => {
                        anyhow::bail!(
                            "Include not found: #include \"{}\" in {}",
                            include, source.display()
                        );
                    }
                    None => {}
                }
            }
        }
        Ok(deps)
    }

    /// Recursively scan `source` for transitive includes. Returns the full set
    /// of project-local header files it depends on (excluding the source itself).
    /// Propagates errors from `scan_file_includes` (including "Include not found").
    fn scan_includes(&self, ctx: &crate::build_context::BuildContext, source: &Path) -> Result<Vec<PathBuf>> {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut headers: Vec<PathBuf> = Vec::new();
        let mut queue: Vec<PathBuf> = vec![source.to_path_buf()];

        while let Some(file) = queue.pop() {
            let direct_deps = self.scan_file_includes(ctx, &file)?;
            for dep in direct_deps {
                if seen.insert(dep.clone()) {
                    headers.push(dep.clone());
                    queue.push(dep);
                }
            }
        }

        Ok(headers)
    }
}

impl DepAnalyzer for IcppDepAnalyzer {
    fn description(&self) -> &'static str {
        "Scan C/C++ source files for #include dependencies (in-process, regex-based)"
    }

    fn enabled(&self) -> bool {
        self.config.enabled
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        let extensions = [".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx"];
        for ext in extensions {
            if file_index.has_extension(ext) {
                return true;
            }
        }
        false
    }

    fn match_product(&self, p: &Product) -> Option<PathBuf> {
        if p.inputs.is_empty() {
            return None;
        }
        let source = &p.inputs[0];
        if self.is_excluded(source) {
            return None;
        }
        let ext = source.extension().and_then(|s| s.to_str()).unwrap_or("");
        if CPP_MATCH_EXTENSIONS.contains(&ext) { Some(source.clone()) } else { None }
    }

    fn analyze(
        &self,
        ctx: &crate::build_context::BuildContext,
        graph: &mut BuildGraph,
        deps_cache: &mut DepsCache,
        _file_index: &FileIndex,
        _verbose: bool,
        progress: &ProgressBar,
    ) -> Result<()> {
        super::analyze_with_scanner(
            ctx,
            graph,
            deps_cache,
            &self.iname,
            |p| self.match_product(p),
            |source| self.scan_includes(ctx, source),
            progress,
        )
    }
}

inventory::submit! {
    crate::registries::AnalyzerPlugin {
        name: "icpp",
        description: "Scan C/C++ source files for #include dependencies (in-process, regex-based)",
        is_native: true,
        create: |iname, toml_value, verbose| {
            let cfg: IcppAnalyzerConfig = toml::from_str(&toml::to_string(toml_value)?)?;
            Ok(Box::new(IcppDepAnalyzer::new(iname, cfg, verbose)))
        },
        defconfig_toml: || {
            toml::to_string_pretty(&crate::config::IcppAnalyzerConfig::default()).ok()
        },
        known_fields: crate::registries::typed_known_fields::<crate::config::IcppAnalyzerConfig>,
    }
}
