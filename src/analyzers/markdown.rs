//! Markdown dependency analyzer for scanning image and file references.
//!
//! Scans Markdown source files for image references (`![alt](path)`) and
//! adds referenced local files as dependencies to products in the build graph.

use anyhow::Result;
use indicatif::ProgressBar;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::MarkdownAnalyzerConfig;
use crate::deps_cache::DepsCache;
use crate::errors;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};

use super::DepAnalyzer;

/// Markdown dependency analyzer that scans source files for image and link references.
pub struct MarkdownDepAnalyzer {
    iname: String,
    config: MarkdownAnalyzerConfig,
}

impl MarkdownDepAnalyzer {
    pub fn new(iname: &str, config: MarkdownAnalyzerConfig) -> Self {
        Self { iname: iname.to_string(), config }
    }

    /// Scan a Markdown file for local file references.
    /// Returns paths to local files referenced via `![alt](path)` or `[text](path)` syntax.
    fn scan_references(&self, source: &Path) -> Result<Vec<PathBuf>> {
        let content = crate::errors::ctx(fs::read_to_string(source), &format!("Failed to read markdown: {}", source.display()))?;
        let mut refs = Vec::new();
        let mut seen = HashSet::new();

        // Match ![alt](path) and [text](path) — capture the path portion
        // Excludes URLs (http://, https://, ftp://, data:, #anchors)
        static REF_RE: OnceLock<Regex> = OnceLock::new();
        let ref_re = REF_RE.get_or_init(|| {
            Regex::new(r"!?\[(?:[^\]]*)\]\(([^)]+)\)").expect(errors::INVALID_REGEX)
        });

        let source_dir = crate::processors::parent_dir(source);

        for caps in ref_re.captures_iter(&content) {
            let path_str = caps[1].trim();

            // Skip URLs, anchors, and data URIs
            if path_str.starts_with("http://")
                || path_str.starts_with("https://")
                || path_str.starts_with("ftp://")
                || path_str.starts_with("data:")
                || path_str.starts_with('#')
            {
                continue;
            }

            // Strip optional title: ![alt](path "title")
            let path_str = path_str.split_whitespace().next().unwrap_or(path_str);
            // Strip anchor fragments: path#section
            let path_str = path_str.split('#').next().unwrap_or(path_str);

            if path_str.is_empty() {
                continue;
            }

            // Try resolving relative to the source file's directory first,
            // then relative to the project root (cwd)
            let candidates = [
                source_dir.join(path_str),
                PathBuf::from(path_str),
            ];
            for candidate in &candidates {
                if candidate.is_file() && !seen.contains(candidate) {
                    seen.insert(candidate.clone());
                    refs.push(candidate.clone());
                    break;
                }
            }
        }

        Ok(refs)
    }
}

impl DepAnalyzer for MarkdownDepAnalyzer {
    fn description(&self) -> &'static str {
        "Scan Markdown files for local file dependencies"
    }

    fn enabled(&self) -> bool {
        self.config.enabled
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        file_index.has_extension(".md")
    }

    fn match_product(&self, p: &Product) -> Option<PathBuf> {
        if p.inputs.is_empty() {
            return None;
        }
        let source = &p.inputs[0];
        let ext = source.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext == "md" { Some(source.clone()) } else { None }
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
            |source| self.scan_references(source),
            progress,
        )
    }
}

inventory::submit! {
    crate::registries::AnalyzerPlugin {
        name: "markdown",
        description: "Scan Markdown files for local file dependencies",
        is_native: true,
        create: |iname, toml_value, _| {
            let cfg: MarkdownAnalyzerConfig = toml::from_str(&toml::to_string(toml_value)?)?;
            Ok(Box::new(MarkdownDepAnalyzer::new(iname, cfg)))
        },
        defconfig_toml: || {
            toml::to_string_pretty(&MarkdownAnalyzerConfig::default()).ok()
        },
        known_fields: crate::registries::typed_known_fields::<MarkdownAnalyzerConfig>,
    }
}
