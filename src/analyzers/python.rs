//! Python dependency analyzer for scanning import statements.
//!
//! Scans Python source files for import statements and adds dependencies
//! to products in the build graph.

use anyhow::Result;
use indicatif::ProgressBar;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::PythonAnalyzerConfig;
use crate::deps_cache::DepsCache;
use crate::errors;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};

use super::DepAnalyzer;

/// Scan a Python source file for `import` / `from X import ...` statements and
/// return the top-level module names referenced. Comments are skipped. The
/// caller decides how to classify each name (local, stdlib, third-party).
pub fn scan_python_imports(source: &Path) -> Result<Vec<String>> {
    let content = crate::errors::ctx(
        fs::read_to_string(source),
        &format!("Failed to read Python source: {}", source.display()),
    )?;
    let mut modules = Vec::new();

    static IMPORT_RE: OnceLock<Regex> = OnceLock::new();
    let import_re = IMPORT_RE.get_or_init(|| {
        Regex::new(r"^\s*(?:from\s+(\S+)\s+import|import\s+(\S+))").expect(errors::INVALID_REGEX)
    });

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }

        let Some(caps) = import_re.captures(line) else { continue };
        let module_path = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str());
        let Some(module) = module_path else { continue };

        // Handle multiple imports: `import foo, bar, baz`
        for part in module.split(',') {
            let module_name = part.split_whitespace().next().unwrap_or("");
            if !module_name.is_empty() {
                modules.push(module_name.to_string());
            }
        }
    }

    Ok(modules)
}

/// Python dependency analyzer that scans source files for import statements.
pub struct PythonDepAnalyzer {
    iname: String,
    config: PythonAnalyzerConfig,
}

impl PythonDepAnalyzer {
    pub fn new(iname: &str, config: PythonAnalyzerConfig) -> Self {
        Self { iname: iname.to_string(), config }
    }

    /// Scan a Python file for import statements and return paths to local
    /// Python files that are imported. Stdlib and third-party modules are
    /// filtered out by `resolve_module`.
    fn scan_imports(&self, source: &Path, file_index: &FileIndex) -> Result<Vec<PathBuf>> {
        let modules = scan_python_imports(source)?;
        let mut imports = Vec::new();
        let mut seen = HashSet::new();
        for module_name in modules {
            if let Some(path) = self.resolve_module(source, &module_name, file_index)
                && !seen.contains(&path) {
                    seen.insert(path.clone());
                    imports.push(path);
                }
        }
        Ok(imports)
    }

    /// Try to resolve a Python module name to a local file path.
    /// Returns None for stdlib/external modules.
    fn resolve_module(&self, source: &Path, module: &str, file_index: &FileIndex) -> Option<PathBuf> {
        // Convert module.path to module/path
        let module_path = module.replace('.', "/");

        // Get the directory containing the source file
        let source_dir = crate::processors::parent_dir(source);

        // Try various resolution strategies:
        // 1. Relative to source file: source_dir/module.py
        // 2. Relative to source file: source_dir/module/__init__.py
        // 3. From project root: module.py
        // 4. From project root: module/__init__.py

        let candidates = [
            // Relative paths
            source_dir.join(format!("{module_path}.py")),
            source_dir.join(&module_path).join("__init__.py"),
            // Project root paths
            PathBuf::from(format!("{module_path}.py")),
            PathBuf::from(&module_path).join("__init__.py"),
        ];

        for candidate in &candidates {
            // Check if this file exists in the file index
            if file_index.contains(candidate) {
                return Some(candidate.clone());
            }
            // Also check if the file exists on disk (cwd is project root)
            if candidate.is_file() {
                return Some(candidate.clone());
            }
        }

        None
    }
}

impl DepAnalyzer for PythonDepAnalyzer {
    fn description(&self) -> &'static str {
        "Scan Python source files for import dependencies"
    }

    fn enabled(&self) -> bool {
        self.config.enabled
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        // Check if there are any Python files
        file_index.has_extension(".py")
    }

    fn match_product(&self, p: &Product) -> Option<PathBuf> {
        if p.inputs.is_empty() {
            return None;
        }
        let source = &p.inputs[0];
        let ext = source.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext == "py" { Some(source.clone()) } else { None }
    }

    fn analyze(
        &self,
        ctx: &crate::build_context::BuildContext,
        graph: &mut BuildGraph,
        deps_cache: &mut DepsCache,
        file_index: &FileIndex,
        _verbose: bool,
        progress: &ProgressBar,
    ) -> Result<()> {
        super::analyze_with_scanner(
            ctx,
            graph,
            deps_cache,
            &self.iname,
            |p| self.match_product(p),
            |source| self.scan_imports(source, file_index),
            progress,
        )
    }
}

inventory::submit! {
    crate::registries::AnalyzerPlugin {
        name: "python",
        description: "Scan Python files for local import dependencies",
        is_native: true,
        create: |iname, toml_value, _| {
            let cfg: PythonAnalyzerConfig = toml::from_str(&toml::to_string(toml_value)?)?;
            Ok(Box::new(PythonDepAnalyzer::new(iname, cfg)))
        },
        defconfig_toml: || {
            toml::to_string_pretty(&PythonAnalyzerConfig::default()).ok()
        },
        known_fields: crate::registries::typed_known_fields::<PythonAnalyzerConfig>,
    }
}
