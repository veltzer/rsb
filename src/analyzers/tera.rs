//! Tera template dependency analyzer for scanning include and import directives.
//!
//! Scans Tera template files for `{% include %}`, `{% import %}`, and `{% extends %}`
//! directives and adds referenced template files as dependencies to products in the build graph.
//! Also scans for the file-reading template functions (`load_python`/`load_lua`/`load_data`/
//! `load_json`/`load_toml`/`load_csv` and `version_str`) so the files they read are
//! content-tracked inputs, and for `glob`/`git_count_files`/`grep_count`/`shell_output`
//! whose resolved path sets and command literals enter the cache hash.

use anyhow::{Result, bail};
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::TeraAnalyzerConfig;
use crate::deps_cache::DepsCache;
use crate::errors;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};

use super::{DepAnalyzer, ScanResult};

use indicatif::ProgressBar;

/// Tera template dependency analyzer that scans for include/import/extends directives.
pub struct TeraDepAnalyzer {
    iname: String,
    config: TeraAnalyzerConfig,
}

impl TeraDepAnalyzer {
    pub fn new(iname: &str, config: TeraAnalyzerConfig) -> Self {
        Self { iname: iname.to_string(), config }
    }

    /// Scan a Tera template file for all dependency-affecting constructs.
    /// Returns the resolved file paths (added to product.inputs) and the
    /// hash pieces that capture non-content state — the sorted set of paths
    /// matching each glob, plus the literal text of each shell command.
    pub(crate) fn scan_template(&self, source: &Path) -> Result<ScanResult> {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        // Pieces accumulated into the config_hash: sorted paths from each glob,
        // plus literal command strings. Order matters and is determined by the
        // order in which they appear in the template, which is stable.
        let mut hash_pieces: Vec<String> = Vec::new();
        // Templates whose contents we've already scanned, to avoid infinite
        // recursion on cyclic includes.
        let mut scanned: HashSet<PathBuf> = HashSet::new();

        scan_template_recursive(source, &mut paths, &mut seen, &mut hash_pieces, &mut scanned)?;

        Ok(ScanResult {
            deps: paths,
            hash_pieces,
        })
    }
}

/// Scan `source` for dependencies and recurse into any `{% include %}`,
/// `{% import %}`, or `{% extends %}` referenced templates so that
/// glob/git_count_files/shell_output calls in *any* transitively-included
/// template participate in the parent product's dependency set and cache key.
///
/// `paths` and `seen` accumulate the input file set; `hash_pieces` accumulates
/// the config-hash contribution; `scanned` prevents revisiting the same
/// template (cycle guard).
fn scan_template_recursive(
    source: &Path,
    paths: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    hash_pieces: &mut Vec<String>,
    scanned: &mut HashSet<PathBuf>,
) -> Result<()> {
    let canonical = source.canonicalize().unwrap_or_else(|_| source.to_path_buf());
    if !scanned.insert(canonical) {
        return Ok(());
    }

    let content = crate::errors::ctx(
        fs::read_to_string(source),
        &format!("Failed to read template: {}", source.display()),
    )?;

    // {% include "path" %}, {% import "path" %}, {% extends "path" %}
    static INCLUDE_RE: OnceLock<Regex> = OnceLock::new();
    let include_re = INCLUDE_RE.get_or_init(|| {
        Regex::new(r#"\{%[-~]?\s*(?:include|import|extends)\s+["']([^"']+)["']"#)
            .expect(errors::INVALID_REGEX)
    });

    // load_python/load_lua/load_data/load_json/load_toml/load_csv(path="...")
    static LOAD_RE: OnceLock<Regex> = OnceLock::new();
    let load_re = LOAD_RE.get_or_init(|| {
        Regex::new(r#"load_(?:python|lua|data|json|toml|csv)\s*\(\s*path\s*=\s*["']([^"']+)["']"#)
            .expect(errors::INVALID_REGEX)
    });

    // version_str() / version_str(path="...") — reads a `tup` from the file at
    // `path`, defaulting to config/version.py when called without arguments.
    // The default must mirror VersionStrFunction in processors/generators/tera.rs
    // or the analyzer would track a different file than the renderer reads.
    static VERSION_STR_RE: OnceLock<Regex> = OnceLock::new();
    let version_str_re = VERSION_STR_RE.get_or_init(|| {
        Regex::new(r#"version_str\s*\(\s*(?:path\s*=\s*["']([^"']+)["'])?\s*\)"#)
            .expect(errors::INVALID_REGEX)
    });

    // glob(pattern="...") — first-class directory query.
    static GLOB_RE: OnceLock<Regex> = OnceLock::new();
    let glob_re = GLOB_RE.get_or_init(|| {
        Regex::new(r#"glob\s*\(\s*pattern\s*=\s*["']([^"']+)["']\s*\)"#)
            .expect(errors::INVALID_REGEX)
    });

    // git_count_files(pattern="...") — counts git-tracked files matching
    // a pathspec. Semantics differ from glob(): only tracked files count,
    // and .gitignore'd or untracked files are excluded.
    static GIT_COUNT_RE: OnceLock<Regex> = OnceLock::new();
    let git_count_re = GIT_COUNT_RE.get_or_init(|| {
        Regex::new(r#"git_count_files\s*\(\s*pattern\s*=\s*["']([^"']+)["']\s*\)"#)
            .expect(errors::INVALID_REGEX)
    });

    // grep_count(pattern="<regex>", glob="<glob>") — counts lines matching
    // a regex across all files matching a glob. Unlike glob/git_count_files,
    // this consumes the *content* of the matched files, so the analyzer must
    // add them as inputs (mtime/checksum-tracked) AND mix the regex literal
    // and resolved file set into the cache hash.
    static GREP_COUNT_RE: OnceLock<Regex> = OnceLock::new();
    let grep_count_re = GREP_COUNT_RE.get_or_init(|| {
        // Capture both args; both can appear in either order. Two regexes:
        // first locate the call body, then extract pattern= and glob= inside.
        Regex::new(r#"grep_count\s*\(([^)]*)\)"#).expect(errors::INVALID_REGEX)
    });
    static GREP_COUNT_PATTERN_RE: OnceLock<Regex> = OnceLock::new();
    let grep_count_pattern_re = GREP_COUNT_PATTERN_RE.get_or_init(|| {
        Regex::new(r#"pattern\s*=\s*["']([^"']*)["']"#).expect(errors::INVALID_REGEX)
    });
    static GREP_COUNT_GLOB_RE: OnceLock<Regex> = OnceLock::new();
    let grep_count_glob_re = GREP_COUNT_GLOB_RE.get_or_init(|| {
        Regex::new(r#"glob\s*=\s*["']([^"']*)["']"#).expect(errors::INVALID_REGEX)
    });

    // shell_output(...) — full call. We pull out the command and depends_on
    // separately. The full body capture is intentionally lazy; a missing
    // depends_on must be diagnosed (analyzer-time error).
    static SHELL_OUTPUT_RE: OnceLock<Regex> = OnceLock::new();
    let shell_re = SHELL_OUTPUT_RE.get_or_init(|| {
        Regex::new(r#"shell_output\s*\(([^)]*)\)"#).expect(errors::INVALID_REGEX)
    });

    // Inner extraction inside a shell_output(...) body.
    static SHELL_CMD_RE: OnceLock<Regex> = OnceLock::new();
    let shell_cmd_re = SHELL_CMD_RE.get_or_init(|| {
        Regex::new(r#"command\s*=\s*["']([^"']*)["']"#).expect(errors::INVALID_REGEX)
    });
    static SHELL_DEPS_RE: OnceLock<Regex> = OnceLock::new();
    let shell_deps_re = SHELL_DEPS_RE.get_or_init(|| {
        Regex::new(r#"depends_on\s*=\s*\[([^\]]*)\]"#).expect(errors::INVALID_REGEX)
    });
    static QUOTED_STR_RE: OnceLock<Regex> = OnceLock::new();
    let quoted_str_re = QUOTED_STR_RE.get_or_init(|| {
        Regex::new(r#"["']([^"']+)["']"#).expect(errors::INVALID_REGEX)
    });

    let source_dir = source.parent().unwrap_or(Path::new("."));

    // 1) include/import/extends and load_*. For include/import/extends, also
    // recurse into the included template so its glob/shell_output/git_count
    // calls participate in this product's dependency set.
    for caps in include_re.captures_iter(&content) {
        let path_str = &caps[1];
        if path_str.is_empty() {
            continue;
        }
        let candidates = [source_dir.join(path_str), PathBuf::from(path_str)];
        for candidate in &candidates {
            if candidate.is_file() {
                if !seen.contains(candidate) {
                    seen.insert(candidate.clone());
                    paths.push(candidate.clone());
                }
                scan_template_recursive(candidate, paths, seen, hash_pieces, scanned)?;
                break;
            }
        }
    }
    for caps in load_re.captures_iter(&content) {
        let path_str = &caps[1];
        if path_str.is_empty() {
            continue;
        }
        let candidates = [source_dir.join(path_str), PathBuf::from(path_str)];
        for candidate in &candidates {
            if candidate.is_file() && !seen.contains(candidate) {
                seen.insert(candidate.clone());
                paths.push(candidate.clone());
                break;
            }
        }
    }
    for caps in version_str_re.captures_iter(&content) {
        let path_str = caps.get(1).map_or("config/version.py", |m| m.as_str());
        if path_str.is_empty() {
            continue;
        }
        let candidates = [source_dir.join(path_str), PathBuf::from(path_str)];
        for candidate in &candidates {
            if candidate.is_file() && !seen.contains(candidate) {
                seen.insert(candidate.clone());
                paths.push(candidate.clone());
                break;
            }
        }
    }

    // 2) glob(pattern="...") — contribute only the resolved path set to the
    // cache hash. The matched files are NOT added as inputs: the template
    // consumes the list of *names*, not their content, so editing one of
    // those files must not invalidate this product. Adding/removing/renaming
    // a matching file changes the path-set fingerprint and rebuilds.
    for caps in glob_re.captures_iter(&content) {
        let pattern = &caps[1];
        let matched = expand_glob(pattern)?;
        hash_pieces.push(format!("glob:{pattern}"));
        hash_pieces.push(format!("glob_resolved:{}", matched.join("\n")));
    }

    // 3) git_count_files(pattern="...") — same path-set-only semantics as
    // glob. Only the count/identity of tracked files matters, not content.
    for caps in git_count_re.captures_iter(&content) {
        let pattern = &caps[1];
        let matched = git_ls_files(pattern);
        hash_pieces.push(format!("git_count:{pattern}"));
        hash_pieces.push(format!("git_count_resolved:{}", matched.join("\n")));
    }

    // 3b) grep_count(pattern="<regex>", glob="<file_glob>") — content-tracked.
    // Both the regex and the resolved file set go into the hash, AND the
    // matched files are added as inputs so mtime/checksum-tracked content
    // changes invalidate the product.
    for caps in grep_count_re.captures_iter(&content) {
        let body = &caps[1];
        let regex_pat = grep_count_pattern_re.captures(body)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
        let file_glob = grep_count_glob_re.captures(body)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
        let Some(regex_pat) = regex_pat else {
            bail!(
                "[tera] {}: grep_count(...) is missing pattern=\"<regex>\". Found: grep_count({})",
                source.display(), body.trim(),
            );
        };
        let Some(file_glob) = file_glob else {
            bail!(
                "[tera] {}: grep_count(pattern=\"{}\") is missing glob=\"<file_glob>\".",
                source.display(), regex_pat,
            );
        };
        let matched = expand_glob(&file_glob)?;
        hash_pieces.push(format!("grep_count_re:{regex_pat}"));
        hash_pieces.push(format!("grep_count_glob:{file_glob}"));
        hash_pieces.push(format!("grep_count_resolved:{}", matched.join("\n")));
        for p in matched {
            let pb = PathBuf::from(p);
            if !seen.contains(&pb) {
                seen.insert(pb.clone());
                paths.push(pb);
            }
        }
    }

    // 4) shell_output(...): require depends_on, harvest patterns and command
    for caps in shell_re.captures_iter(&content) {
        let body = &caps[1];
        let command = shell_cmd_re.captures(body)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
        let deps_block = shell_deps_re.captures(body)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));

        let Some(command) = command else {
            bail!(
                "[tera] {}: shell_output(...) call has no command= argument. \
                 Found: shell_output({})",
                source.display(), body.trim(),
            );
        };
        let Some(deps_block) = deps_block else {
            bail!(
                "[tera] {}: shell_output(command=\"{}\") is missing depends_on=[...].\n\
                 rsconstruct cannot otherwise tell when its output should be invalidated.\n\
                 Migrate to glob(pattern=\"...\") for directory queries, or pass an explicit \
                 list (e.g. depends_on=[\"marp/**/*.md\"]).\n\
                 If your command genuinely has no file dependencies, pass depends_on=[] \
                 to acknowledge that.",
                source.display(), command,
            );
        };

        hash_pieces.push(format!("shell_cmd:{command}"));

        let mut patterns: Vec<String> = Vec::new();
        for pcap in quoted_str_re.captures_iter(&deps_block) {
            patterns.push(pcap[1].to_string());
        }
        if patterns.is_empty() {
            hash_pieces.push("shell_deps:[]".to_string());
            continue;
        }
        for pattern in &patterns {
            let matched = expand_glob(pattern)?;
            hash_pieces.push(format!("shell_dep:{pattern}"));
            hash_pieces.push(format!("shell_dep_resolved:{}", matched.join("\n")));
            for p in matched {
                let pb = PathBuf::from(p);
                if !seen.contains(&pb) {
                    seen.insert(pb.clone());
                    paths.push(pb);
                }
            }
        }
    }

    Ok(())
}

/// Run `git ls-files -- <pattern>` and return the sorted list of tracked
/// files matching the pathspec. This mirrors the runtime semantics of the
/// `git_count_files` Tera function so the analyzer's invalidation set
/// matches what the renderer actually counts. A failed git invocation
/// (e.g. not a git repository) yields an empty list — the function is
/// best-effort by design.
fn git_ls_files(pattern: &str) -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "--", pattern])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut paths: Vec<String> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(std::string::ToString::to_string)
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Expand a glob pattern into a sorted list of file paths (as strings, relative
/// to project root). Symlinks and directories are skipped — only regular files
/// participate. The sorted order matters for the deterministic hash piece.
fn expand_glob(pattern: &str) -> Result<Vec<String>> {
    let mut paths: Vec<String> = Vec::new();
    for entry in glob::glob(pattern)
        .map_err(|e| anyhow::anyhow!("Invalid glob pattern '{pattern}': {e}"))?
    {
        let path = entry
            .map_err(|e| anyhow::anyhow!("Glob iteration error for '{pattern}': {e}"))?;
        if path.is_file() {
            paths.push(path.to_string_lossy().into_owned());
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

impl DepAnalyzer for TeraDepAnalyzer {
    fn description(&self) -> &'static str {
        "Scan Tera templates for include/import/extends dependencies"
    }

    fn enabled(&self) -> bool {
        self.config.enabled
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        file_index.has_extension(".tera")
    }

    fn match_product(&self, p: &Product) -> Option<PathBuf> {
        if p.inputs.is_empty() {
            return None;
        }
        let source = &p.inputs[0];
        let ext = source.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext == "tera" { Some(source.clone()) } else { None }
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
        super::analyze_with_full_scanner(
            ctx,
            graph,
            deps_cache,
            &self.iname,
            |p| self.match_product(p),
            |source| self.scan_template(source),
            progress,
        )
    }

    fn scan_hash_pieces(&self, source: &Path) -> Result<Option<Vec<String>>> {
        Ok(Some(self.scan_template(source)?.hash_pieces))
    }
}

inventory::submit! {
    crate::registries::AnalyzerPlugin {
        name: "tera",
        description: "Scan Tera templates for include/import/extends dependencies",
        is_native: true,
        create: |iname, toml_value, _| {
            let cfg: TeraAnalyzerConfig = toml::from_str(&toml::to_string(toml_value)?)?;
            Ok(Box::new(TeraDepAnalyzer::new(iname, cfg)))
        },
        defconfig_toml: || {
            toml::to_string_pretty(&TeraAnalyzerConfig::default()).ok()
        },
        known_fields: crate::registries::typed_known_fields::<TeraAnalyzerConfig>,
    }
}
