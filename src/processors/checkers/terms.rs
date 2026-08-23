use anyhow::{Result, bail};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, default_true};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::discover_checker_products;

fn default_dir_terms_unambiguous() -> String {
    "terms/unambiguous".into()
}

fn default_dir_terms_ambiguous() -> String {
    "terms/ambiguous".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TermsConfig {
    #[serde(default = "default_dir_terms_unambiguous")]
    pub dir_terms_unambiguous: String,
    #[serde(default = "default_dir_terms_ambiguous")]
    pub dir_terms_ambiguous: String,
    /// When true (default), backticking an ambiguous term is a build error
    /// and `terms fix` strips those backticks. When false, ambiguous terms
    /// are loaded only to validate the disjoint invariant; their use in
    /// backticks is neither flagged nor changed.
    #[serde(default = "default_true")]
    pub forbid_backticked_ambiguous: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for TermsConfig {
    fn default() -> Self {
        Self {
            dir_terms_unambiguous: "terms/unambiguous".into(),
            dir_terms_ambiguous: "terms/ambiguous".into(),
            forbid_backticked_ambiguous: true,
            standard: StandardConfig::default(),
        }
    }
}

/// Single-meaning terms (must be backticked in prose) and ambiguous terms
/// (must NOT be backticked — using backticks falsely asserts they're the
/// technical term). The sets are guaranteed disjoint by `load_and_validate_terms`.
pub struct LoadedTerms {
    pub single: HashSet<String>,
    pub ambiguous: HashSet<String>,
}

impl LoadedTerms {
    pub fn is_empty(&self) -> bool {
        self.single.is_empty() && self.ambiguous.is_empty()
    }
}

pub struct TermsProcessor {
    config: TermsConfig,
}

impl TermsProcessor {
    pub const fn new(config: TermsConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, product: &Product) -> Result<()> {
        self.check_files(&[product.primary_input()])
    }

    fn check_files(&self, files: &[&Path]) -> Result<()> {
        let terms = load_and_validate_terms(&self.config)?;
        let sorted = sorted_terms(&terms.single);
        let amb_for_check: HashSet<String> = if self.config.forbid_backticked_ambiguous {
            terms.ambiguous.clone()
        } else {
            HashSet::new()
        };
        let mut bad_files: Vec<String> = Vec::new();

        for file in files {
            let issues = check_file_detail(file, &sorted, &amb_for_check)?;
            if !issues.is_empty() {
                bad_files.push(format!("{}: {}", file.display(), issues));
            }
        }

        if bad_files.is_empty() {
            Ok(())
        } else {
            bail!(
                "{} file(s) have term issues (run `rsconstruct terms fix` to fix):\n{}",
                bad_files.len(),
                bad_files.join("\n"),
            )
        }
    }
}

impl crate::processors::Processor for TermsProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        Path::new(&self.config.dir_terms_unambiguous).is_dir()
            && !file_index.scan(&self.config.standard, true).is_empty()
    }

    fn discover(
        &self,
        graph: &mut BuildGraph,
        file_index: &FileIndex,
        instance_name: &str,
    ) -> Result<()> {
        if !Path::new(&self.config.dir_terms_unambiguous).is_dir() {
            return Ok(());
        }
        // Collect all .txt files from both term directories as extra inputs
        let mut dep_inputs = self.config.standard.dep_inputs.clone();
        let mut watched_dirs: Vec<&str> = vec![&self.config.dir_terms_unambiguous];
        if Path::new(&self.config.dir_terms_ambiguous).is_dir() {
            watched_dirs.push(&self.config.dir_terms_ambiguous);
        }
        for dir in watched_dirs {
            for entry in crate::errors::ctx(fs::read_dir(dir), &format!("Failed to read terms directory {dir}"))? {
                let entry = crate::errors::ctx(entry, &format!("Failed to read entry in terms directory {dir}"))?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "txt") {
                    dep_inputs.push(path.to_string_lossy().into_owned());
                }
            }
        }
        discover_checker_products(
            graph, &self.config.standard, file_index, &dep_inputs,
            &self.config.standard.dep_auto, &self.config,
            &crate::config::checksum_fields_of(instance_name),
            instance_name,
        )
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_product(product)
    }

    fn execute_batch(&self, _ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        crate::processors::execute_checker_batch_per_file(products, |file| self.check_files(&[file]))
    }
}

// --- Shared logic used by both the processor and the `rsconstruct terms fix` command ---

/// Load all technical terms from .txt files in the given directory.
/// Each file has one term per line. Errors if any term appears more than once
/// (within the same file or across files).
pub fn load_terms(dir_path: &str) -> Result<HashSet<String>> {
    let dir = Path::new(dir_path);
    if !dir.is_dir() {
        bail!("terms directory `{dir_path}` does not exist or is not a directory");
    }
    // Map each term to (file, line_number) where it first appeared
    let mut seen: std::collections::HashMap<String, (String, usize)> = std::collections::HashMap::new();
    let mut duplicates = Vec::new();

    let mut entries: Vec<_> = crate::errors::ctx(fs::read_dir(dir), &format!("Failed to read terms directory {}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .collect();
    entries.sort_by_key(std::fs::DirEntry::path);

    for entry in &entries {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "txt") {
            let filename = path.file_name().unwrap().to_string_lossy().to_string();
            let content = crate::errors::ctx(fs::read_to_string(&path), &format!("Failed to read terms file: {}", path.display()))?;
            for (line_idx, line) in content.lines().enumerate() {
                let term = line.trim();
                if term.is_empty() {
                    continue;
                }
                if let Some((prev_file, prev_line)) = seen.get(term) {
                    duplicates.push(format!(
                        "  `{}` in {}:{} (first seen in {}:{})",
                        term, filename, line_idx + 1, prev_file, prev_line,
                    ));
                } else {
                    seen.insert(term.to_string(), (filename.clone(), line_idx + 1));
                }
            }
        }
    }

    if !duplicates.is_empty() {
        bail!("Duplicate terms in {}:\n{}", dir_path, duplicates.join("\n"));
    }

    Ok(seen.into_keys().collect())
}

/// Load the unambiguous and ambiguous term lists. If the ambiguous directory
/// exists on disk, also verifies that no term appears in both directories.
/// A missing ambiguous directory is treated as an empty list, not an error —
/// projects without an ambiguous list just get the unambiguous-only behavior.
/// Errors if no terms are loaded at all: a configured terms setup that yields
/// zero terms is a misconfiguration (e.g. `dir_terms_unambiguous` pointing at
/// the wrong directory), and silently checking nothing would hide it.
pub fn load_and_validate_terms(config: &TermsConfig) -> Result<LoadedTerms> {
    let single = load_terms(&config.dir_terms_unambiguous)?;
    let amb_dir = &config.dir_terms_ambiguous;
    let ambiguous = if Path::new(amb_dir).is_dir() {
        let ambiguous = load_terms(amb_dir)?;
        let mut overlap: Vec<&str> = single
            .iter()
            .filter(|t| ambiguous.contains(*t))
            .map(std::string::String::as_str)
            .collect();
        if !overlap.is_empty() {
            overlap.sort_unstable();
            bail!(
                "{} term(s) appear in both `{}` and `{}` (ambiguous terms must not be in the unambiguous list):\n  {}",
                overlap.len(),
                config.dir_terms_unambiguous,
                amb_dir,
                overlap.join("\n  "),
            );
        }
        ambiguous
    } else {
        HashSet::new()
    };
    let loaded = LoadedTerms { single, ambiguous };
    if loaded.is_empty() {
        let amb_note = if Path::new(amb_dir).is_dir() {
            format!(" or `{amb_dir}`")
        } else {
            String::new()
        };
        bail!(
            "no terms found in `{}`{amb_note} — the terms processor has nothing to check; \
             each .txt file there must list one term per line (is `dir_terms_unambiguous` pointing at the right directory?)",
            config.dir_terms_unambiguous,
        );
    }
    Ok(loaded)
}

/// Sort terms longest-first for greedy matching (so "Android Studio" matches before "Android").
fn sorted_terms(terms: &HashSet<String>) -> Vec<&str> {
    let mut sorted: Vec<&str> = terms.iter().map(std::string::String::as_str).collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.len()));
    sorted
}

// --- Text analysis helpers ---

/// Find ranges in the text that should be excluded from term processing:
/// YAML frontmatter (--- ... ---), fenced code blocks (``` ... ```),
/// and HTML comments (<!-- ... -->).
fn excluded_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();

    // YAML frontmatter: must start at the very beginning of the file
    if text.starts_with("---\n") || text.starts_with("---\r\n") {
        let skip = if text.as_bytes().get(3) == Some(&b'\r') { 5 } else { 4 };
        if let Some(end_idx) = text[skip..].find("\n---") {
            let mut end = skip + end_idx + 4; // past the closing ---
            // Skip to end of line
            while end < text.len() && text.as_bytes()[end] != b'\n' {
                end += 1;
            }
            if end < text.len() {
                end += 1;
            }
            ranges.push((0, end));
        }
    }

    // Fenced code blocks
    let mut pos = 0;
    let bytes = text.as_bytes();
    while pos < bytes.len() {
        if bytes[pos] == b'`' && pos + 2 < bytes.len() && bytes[pos + 1] == b'`' && bytes[pos + 2] == b'`' {
            let start = pos;
            pos += 3;
            while pos < bytes.len() && bytes[pos] == b'`' {
                pos += 1;
            }
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            loop {
                if pos >= bytes.len() {
                    ranges.push((start, bytes.len()));
                    break;
                }
                if bytes[pos] == b'\n' || pos == 0 {
                    let line_start = if bytes[pos] == b'\n' { pos + 1 } else { pos };
                    if line_start + 2 < bytes.len()
                        && bytes[line_start] == b'`'
                        && bytes[line_start + 1] == b'`'
                        && bytes[line_start + 2] == b'`'
                    {
                        let mut end = line_start + 3;
                        while end < bytes.len() && bytes[end] != b'\n' {
                            end += 1;
                        }
                        if end < bytes.len() {
                            end += 1;
                        }
                        ranges.push((start, end));
                        pos = end;
                        break;
                    }
                }
                pos += 1;
            }
        } else {
            pos += 1;
        }
    }

    // HTML comments: <!-- ... -->
    let comment_open = b"<!--";
    let comment_close = b"-->";
    pos = 0;
    while pos + 4 <= bytes.len() {
        if &bytes[pos..pos + 4] == comment_open {
            let start = pos;
            pos += 4;
            while pos + 3 <= bytes.len() {
                if &bytes[pos..pos + 3] == comment_close {
                    pos += 3;
                    ranges.push((start, pos));
                    break;
                }
                pos += 1;
            }
        } else {
            pos += 1;
        }
    }

    ranges
}

/// Find all backtick span ranges (start, end) in text, excluding fenced code blocks.
/// Returns positions of the opening and closing backtick (inclusive of backticks).
fn backtick_span_ranges(text: &str, fenced: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip fenced code blocks
        let mut in_fenced = false;
        for &(fs, fe) in fenced {
            if i >= fs && i < fe {
                i = fe;
                in_fenced = true;
                break;
            }
        }
        if in_fenced {
            continue;
        }
        if bytes[i] == b'`' {
            // Find matching closing backtick
            let open = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'`' && bytes[i] != b'\n' {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'`' && i > open {
                spans.push((open, i + 1)); // include both backticks
            }
            if i < bytes.len() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    spans
}

/// Check if a byte position is inside any of the given ranges.
fn inside_ranges(pos: usize, end: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|&(s, e)| pos >= s && end <= e)
}

/// Check if the character at a byte position is a word-boundary character.
/// A term match is valid if the characters immediately before and after it
/// are not alphanumeric (or the match is at the start/end of text).
const fn is_word_boundary(text: &[u8], pos: usize) -> bool {
    if pos >= text.len() {
        return true;
    }
    let ch = text[pos];
    // Not a word character: anything that's not alphanumeric or underscore
    !ch.is_ascii_alphanumeric() && ch != b'_'
}

/// Find all occurrences of a term in text (case-sensitive, word-boundary).
/// Returns (start, end) byte positions for each match.
fn find_term_occurrences(text: &str, term: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut results = Vec::new();
    let mut pos = 0;
    while let Some(rel) = text[pos..].find(term) {
        let start = pos + rel;
        let end = start + term.len();
        // Check word boundaries
        let before_ok = start == 0 || is_word_boundary(bytes, start - 1);
        let after_ok = end >= bytes.len() || is_word_boundary(bytes, end);
        if before_ok && after_ok {
            results.push((start, end));
            pos = end;
        } else {
            // Advance to next UTF-8 char boundary
            pos = start + 1;
            while pos < text.len() && !text.is_char_boundary(pos) {
                pos += 1;
            }
        }
    }
    results
}

// --- Core check/fix logic ---

/// Split a backticked string into individual terms.
/// Handles comma-separated lists like "sed, awk" and word separators "and"/"or".
fn split_backticked(inner: &str) -> Vec<String> {
    let mut results = Vec::new();
    for part in inner.split(',') {
        for tok in part.split(" and ").flat_map(|s| s.split(" or ")) {
            let trimmed = tok.trim();
            if !trimmed.is_empty() {
                results.push(trimmed.to_string());
            }
        }
    }
    results
}

/// Check if a string with `/` looks like a file path rather than a term like CI/CD.
/// File paths have dots (extensions) or multiple segments.
fn is_file_path(s: &str) -> bool {
    if !s.contains('/') {
        return false;
    }
    // Has a dot after a slash → likely a file path (e.g., doc/ai.txt)
    if let Some(last_slash) = s.rfind('/')
        && s[last_slash..].contains('.')
    {
        return true;
    }
    // More than one slash → likely a file path (e.g., syllabi/courses/ai)
    s.matches('/').count() > 1
}

/// Check if a backticked string looks like a term reference (not arbitrary inline code
/// or a file path). Code snippets, file paths, and shell commands should keep their backticks.
fn looks_like_term_reference(inner: &str) -> bool {
    let parts = split_backticked(inner);
    if parts.is_empty() {
        return false;
    }
    if is_file_path(inner) {
        return false;
    }
    let code_chars = ['(', ')', '{', '}', '[', ']', ';', '=', '>', '<', '|', '\\', '"', '\'', '~'];
    for part in &parts {
        if part.contains(' ') || part.chars().any(|c| code_chars.contains(&c)) {
            return false;
        }
    }
    true
}

/// Find unquoted term positions (byte offsets) for the fix command.
/// Returns (start, end, `term_text`) sorted longest-first, non-overlapping.
fn find_unquoted_positions(content: &str, sorted_terms: &[&str]) -> Vec<(usize, usize, String)> {
    let fenced = excluded_ranges(content);
    let backtick_spans = backtick_span_ranges(content, &fenced);
    let mut claimed: Vec<(usize, usize)> = Vec::new();
    let mut results = Vec::new();

    for &term in sorted_terms {
        for (start, end) in find_term_occurrences(content, term) {
            if inside_ranges(start, end, &fenced) {
                continue;
            }
            if inside_ranges(start, end, &backtick_spans) {
                continue;
            }
            if claimed.iter().any(|&(cs, ce)| start < ce && end > cs) {
                continue;
            }
            claimed.push((start, end));
            results.push((start, end, content[start..end].to_string()));
        }
    }
    results
}

/// Find backtick-quoted terms that are NOT in the unambiguous list and
/// NOT in the ambiguous list. Used by `--remove-non-terms`. Spans containing
/// ambiguous terms are excluded here so they're handled by the ambiguous-strip
/// pass instead (which always runs).
fn find_non_tech_backticked_positions(
    content: &str,
    single: &HashSet<String>,
    ambiguous: &HashSet<String>,
) -> Vec<(usize, usize)> {
    let fenced = excluded_ranges(content);
    let spans = backtick_span_ranges(content, &fenced);
    let mut results = Vec::new();
    for &(start, end) in &spans {
        let inner = &content[start + 1..end - 1];
        if !looks_like_term_reference(inner) {
            continue;
        }
        let parts = split_backticked(inner);
        let all_non_tech = parts.iter().all(|p| !single.contains(p) && !ambiguous.contains(p));
        if all_non_tech {
            results.push((start, end));
        }
    }
    results
}

/// Find backtick-quoted spans whose contents include any ambiguous term.
/// These are an error — ambiguous terms must NOT be backticked, since
/// backticks falsely assert the technical reading.
fn find_backticked_ambiguous_positions(
    content: &str,
    ambiguous: &HashSet<String>,
) -> Vec<(usize, usize, Vec<String>)> {
    let fenced = excluded_ranges(content);
    let spans = backtick_span_ranges(content, &fenced);
    let mut results = Vec::new();
    for &(start, end) in &spans {
        let inner = &content[start + 1..end - 1];
        if !looks_like_term_reference(inner) {
            continue;
        }
        let parts = split_backticked(inner);
        let hits: Vec<String> = parts.into_iter()
            .filter(|p| ambiguous.contains(p))
            .collect();
        if !hits.is_empty() {
            results.push((start, end, hits));
        }
    }
    results
}

/// Apply edits to text, right-to-left. Edits must not overlap.
fn apply_edits(content: &str, edits: &mut Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    edits.dedup_by(|a, b| a.1 > b.0);

    let mut result = content.to_string();
    for (start, end, replacement) in edits.iter() {
        result = format!("{}{}{}", &result[..*start], replacement, &result[*end..]);
    }
    result
}

/// Apply term fixes to content. When `forbid_ambiguous_backticks` is true,
/// strips backticks from ambiguous terms (they're an error). Then optionally
/// removes backticks from arbitrary non-terms. Then adds missing backticks
/// around bare unambiguous terms.
fn fix_content(
    original: &str,
    terms: &LoadedTerms,
    sorted_terms: &[&str],
    remove_non_terms: bool,
    forbid_ambiguous_backticks: bool,
) -> String {
    // Step 1: when forbidding, strip backticks around ambiguous terms (`server` → server).
    let after_amb = if forbid_ambiguous_backticks {
        let mut amb_removals: Vec<(usize, usize, String)> = find_backticked_ambiguous_positions(original, &terms.ambiguous)
            .into_iter()
            .map(|(s, e, _)| (s, e, original[s + 1..e - 1].to_string()))
            .collect();
        if amb_removals.is_empty() {
            original.to_string()
        } else {
            apply_edits(original, &mut amb_removals)
        }
    } else {
        original.to_string()
    };

    // Step 2: optionally remove backticks from non-terms (e.g. `CI`/`CD` → CI/CD).
    let cleaned = if remove_non_terms {
        let mut removals: Vec<(usize, usize, String)> = find_non_tech_backticked_positions(&after_amb, &terms.single, &terms.ambiguous)
            .into_iter()
            .map(|(s, e)| (s, e, after_amb[s + 1..e - 1].to_string()))
            .collect();
        if removals.is_empty() {
            after_amb
        } else {
            apply_edits(&after_amb, &mut removals)
        }
    } else {
        after_amb
    };

    // Step 3: add backticks to unquoted unambiguous terms (on the cleaned text,
    // so e.g. CI/CD is now found if its backticks were just stripped).
    let mut additions: Vec<(usize, usize, String)> = find_unquoted_positions(&cleaned, sorted_terms)
        .into_iter()
        .map(|(s, e, m)| (s, e, format!("`{m}`")))
        .collect();
    if additions.is_empty() {
        cleaned
    } else {
        apply_edits(&cleaned, &mut additions)
    }
}

/// Check a file and return a formatted issue summary, or an empty string if clean.
/// Reports both unquoted unambiguous terms and ambiguous terms found inside backticks.
fn check_file_detail(path: &Path, sorted_terms: &[&str], ambiguous: &HashSet<String>) -> Result<String> {
    let content = crate::errors::ctx(fs::read_to_string(path), &format!("Failed to read {}", path.display()))?;

    let unquoted = find_unquoted_positions(&content, sorted_terms);
    let mut unquoted_terms: Vec<String> = unquoted.into_iter().map(|(_, _, t)| t).collect();
    unquoted_terms.sort();
    unquoted_terms.dedup();

    let amb_hits = find_backticked_ambiguous_positions(&content, ambiguous);
    let mut amb_terms: Vec<String> = amb_hits.into_iter().flat_map(|(_, _, hits)| hits).collect();
    amb_terms.sort();
    amb_terms.dedup();

    let mut parts: Vec<String> = Vec::new();
    if !unquoted_terms.is_empty() {
        parts.push(format!("missing backticks: {}", unquoted_terms.join(", ")));
    }
    if !amb_terms.is_empty() {
        parts.push(format!("ambiguous terms must not be backticked: {}", amb_terms.join(", ")));
    }
    Ok(parts.join("; "))
}

/// Auto-fix a single markdown file. Returns true if the file was modified.
pub fn fix_file(
    path: &Path,
    terms: &LoadedTerms,
    sorted_terms: &[&str],
    remove_non_terms: bool,
    forbid_ambiguous_backticks: bool,
) -> Result<bool> {
    let original = crate::errors::ctx(fs::read_to_string(path), &format!("Failed to read {}", path.display()))?;
    let fixed = fix_content(&original, terms, sorted_terms, remove_non_terms, forbid_ambiguous_backticks);
    if fixed == original {
        Ok(false)
    } else {
        crate::errors::ctx(fs::write(path, &fixed), &format!("Failed to write {}", path.display()))?;
        Ok(true)
    }
}

/// Fix all markdown files: called by `rsconstruct terms fix`.
/// Uses the same scan config as the terms processor to find files.
pub fn fix_all(config: &TermsConfig, remove_non_terms: bool, warn_symlinks: bool) -> Result<()> {
    let terms = load_and_validate_terms(config)?;
    let sorted = sorted_terms(&terms.single);

    // Force-walk every src_dir the user listed: they may include generated
    // directories like `out/generator` that are gitignored. The build path
    // sees those via `add_virtual_files` after the discover loop runs;
    // `terms fix` runs standalone so it must walk them itself.
    let force_dirs: Vec<&str> = config.standard.src_dirs().iter().map(std::string::String::as_str).collect();
    let file_index = FileIndex::build_with_force_dirs(&force_dirs, &[], warn_symlinks)?;
    let md_files = file_index.scan(&config.standard, true);

    if md_files.is_empty() {
        println!("No markdown files found");
        return Ok(());
    }

    println!(
        "Checking {} markdown files against {} unambiguous + {} ambiguous terms...",
        md_files.len(), terms.single.len(), terms.ambiguous.len(),
    );

    let mut modified_count = 0;
    for file in &md_files {
        if fix_file(file, &terms, &sorted, remove_non_terms, config.forbid_backticked_ambiguous)? {
            modified_count += 1;
            println!("  Fixed: {}", file.display());
        }
    }

    println!("Done. Modified {} of {} files.", modified_count, md_files.len());
    Ok(())
}

/// Merge terms from another project's terms directory into the current one.
/// For each .txt file in `source_dir`:
///   - If a file with the same name exists in `dir_terms_unambiguous`, merge (union) and sort the terms.
///   - Otherwise, copy the file as-is.
pub fn merge_terms(config: &TermsConfig, source_dir: &str) -> Result<()> {
    let src = Path::new(source_dir);
    if !src.is_dir() {
        bail!("Source directory `{source_dir}` does not exist or is not a directory");
    }
    let dest = Path::new(&config.dir_terms_unambiguous);
    if !dest.is_dir() {
        bail!("Terms directory `{}` does not exist or is not a directory", config.dir_terms_unambiguous);
    }

    let mut merged_count = 0;
    let mut copied_count = 0;

    for entry in crate::errors::ctx(fs::read_dir(src), &format!("Failed to read source directory {}", src.display()))? {
        let entry = crate::errors::ctx(entry, &format!("Failed to read entry in source directory {}", src.display()))?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        let filename = path.file_name().unwrap();
        let dest_path = dest.join(filename);

        let source_content = crate::errors::ctx(fs::read_to_string(&path), &format!("Failed to read terms source: {}", path.display()))?;
        let source_terms: HashSet<String> = source_content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        if dest_path.exists() {
            let dest_content = crate::errors::ctx(fs::read_to_string(&dest_path), &format!("Failed to read terms dest: {}", dest_path.display()))?;
            let dest_terms: HashSet<String> = dest_content
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            let mut all_terms = dest_terms.clone();
            all_terms.extend(source_terms.clone());
            let mut sorted: Vec<String> = all_terms.into_iter().collect();
            sorted.sort();
            let content = sorted.join("\n") + "\n";
            if content != source_content || content != dest_content {
                crate::errors::ctx(fs::write(&dest_path, &content), &format!("Failed to write {}", dest_path.display()))?;
                crate::errors::ctx(fs::write(&path, &content), &format!("Failed to write {}", path.display()))?;
                merged_count += 1;
                let added_to_dest = sorted.len() - dest_terms.len();
                let added_to_src = sorted.len() - source_terms.len();
                println!("  Merged: {} (+{} to dest, +{} to source)",
                    filename.to_string_lossy(), added_to_dest, added_to_src);
            }
        } else {
            let mut sorted: Vec<String> = source_terms.into_iter().collect();
            sorted.sort();
            crate::errors::ctx(fs::write(&dest_path, sorted.join("\n") + "\n"), &format!("Failed to write {}", dest_path.display()))?;
            copied_count += 1;
            println!("  Copied to dest: {}", filename.to_string_lossy());
        }
    }

    // Copy files that exist in destination but not in source back to source
    for entry in crate::errors::ctx(fs::read_dir(dest), &format!("Failed to read destination directory {}", dest.display()))? {
        let entry = crate::errors::ctx(entry, &format!("Failed to read entry in destination directory {}", dest.display()))?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        let filename = path.file_name().unwrap();
        let src_path = src.join(filename);
        if !src_path.exists() {
            crate::errors::ctx(fs::copy(&path, &src_path), &format!("Failed to copy {} to {}", path.display(), src_path.display()))?;
            copied_count += 1;
            println!("  Copied to source: {}", filename.to_string_lossy());
        }
    }

    println!("Done. Merged {merged_count} file(s), copied {copied_count} new file(s).");

    // After merging, enforce the no-overlap-with-ambiguous invariant.
    load_and_validate_terms(config)?;
    Ok(())
}

/// Print term statistics.
pub fn stats(config: &TermsConfig) -> Result<()> {
    // Validate the no-overlap invariant before reporting stats.
    load_and_validate_terms(config)?;
    let (single_files, single_terms) = count_terms_in_dir(&config.dir_terms_unambiguous)?;
    let amb_dir_exists = Path::new(&config.dir_terms_ambiguous).is_dir();
    let (amb_files, amb_terms) = if amb_dir_exists {
        count_terms_in_dir(&config.dir_terms_ambiguous)?
    } else {
        (0usize, 0usize)
    };
    if crate::json_output::is_json_mode() {
        let out = serde_json::json!({
            "term_files": single_files,
            "total_terms": single_terms,
            "ambiguous_term_files": amb_files,
            "total_ambiguous_terms": amb_terms,
        });
        println!("{}", serde_json::to_string_pretty(&out).expect(crate::errors::JSON_SERIALIZE));
    } else {
        println!("{single_files} term file(s), {single_terms} total terms (unambiguous)");
        if amb_dir_exists {
            println!("{amb_files} term file(s), {amb_terms} total terms (ambiguous)");
        }
    }
    Ok(())
}

fn count_terms_in_dir(dir_str: &str) -> Result<(usize, usize)> {
    let dir = Path::new(dir_str);
    if !dir.is_dir() {
        bail!("terms directory `{dir_str}` does not exist or is not a directory");
    }
    let mut file_count = 0;
    let mut total_terms = 0;
    for entry in crate::errors::ctx(fs::read_dir(dir), &format!("Failed to read terms directory {}", dir.display()))? {
        let entry = crate::errors::ctx(entry, &format!("Failed to read entry in terms directory {}", dir.display()))?;
        if entry.path().extension().is_some_and(|e| e == "txt") {
            file_count += 1;
            let content = crate::errors::ctx(fs::read_to_string(entry.path()), &format!("Failed to read {}", entry.path().display()))?;
            total_terms += content.lines().filter(|l| !l.trim().is_empty()).count();
        }
    }
    Ok((file_count, total_terms))
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(TermsProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "terms",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "dir_terms_unambiguous", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Directory containing unambiguous term definition files (must be backticked in prose)" },
            crate::config::FieldSpec { name: "dir_terms_ambiguous", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Optional directory of ambiguous terms; build fails if any term overlaps with dir_terms_unambiguous" },
            crate::config::FieldSpec { name: "forbid_backticked_ambiguous", ty: crate::config::FieldType::Bool,
                affects_output: true, required: false,
                doc: "If true (default), backticking an ambiguous term is a build error and `terms fix` strips those backticks" },
        ],
        omit_standard_fields: &["command", "formats", "args", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<TermsConfig>,
        keywords: &["checker", "terminology", "text", "words"],
        description: "Check that technical terms are backtick-quoted in markdown files",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
