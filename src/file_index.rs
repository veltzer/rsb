use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::config::StandardConfig;

#[derive(Debug, Clone)]
pub struct FileIndex {
    files: Vec<PathBuf>,
}

#[cfg(test)]
impl FileIndex {
    /// Create a `FileIndex` from an explicit list of paths (for testing).
    fn from_paths(mut files: Vec<PathBuf>) -> Self {
        files.sort();
        Self { files }
    }
}

impl FileIndex {
    /// Build a file index by walking the current directory once.
    /// Uses `ignore::WalkBuilder` which natively handles `.gitignore` and
    /// `.rsconstructignore` (via `add_custom_ignore_filename`).
    /// All paths are stored relative to project root (cwd).
    pub fn build() -> Result<Self> {
        Self::build_with_force_dirs(&[])
    }

    /// Build a file index, but also walk the listed directories ignoring
    /// gitignore/rsconstructignore. Use this when the user has explicitly
    /// listed a directory in a processor's `src_dirs` — that listing is an
    /// opt-in to scan it even if it would otherwise be ignored.
    ///
    /// Common case: `src_dirs = ["out/generator"]` where `out/` is in
    /// `.gitignore` because it holds generated files. The user explicitly
    /// wants those generated files scanned (e.g. for terms checking).
    pub fn build_with_force_dirs(force_dirs: &[&str]) -> Result<Self> {
        let walker = ignore::WalkBuilder::new(".")
            .add_custom_ignore_filename(".rsconstructignore")
            .hidden(false) // don't skip hidden files by default (let .gitignore handle it)
            .build();

        let mut files: Vec<PathBuf> = Vec::new();
        for entry in walker {
            let entry = crate::errors::ctx(entry, "Failed to read directory entry during file indexing")?;
            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                let path = entry.into_path();
                // Store relative paths (strip "./" prefix)
                let relative = path.strip_prefix(".")
                    .unwrap_or(&path)
                    .to_path_buf();
                files.push(relative);
            }
        }

        // Walk force_dirs unconditionally — these are user-declared and
        // opt-in, so gitignore should not filter them out.
        for dir in force_dirs {
            if dir.is_empty() {
                continue;
            }
            let path = Path::new(dir);
            if !path.is_dir() {
                continue;
            }
            let walker = ignore::WalkBuilder::new(path)
                .standard_filters(false) // ignore .gitignore, .ignore, hidden, etc.
                .build();
            for entry in walker {
                let entry = crate::errors::ctx(entry, &format!("Failed to read directory entry under forced dir '{dir}'"))?;
                if entry.file_type().is_some_and(|ft| ft.is_file()) {
                    let path = entry.into_path();
                    let relative = path.strip_prefix(".")
                        .unwrap_or(&path)
                        .to_path_buf();
                    files.push(relative);
                }
            }
        }

        files.sort();
        files.dedup();

        Ok(Self { files })
    }

    /// Query the index for files matching the given criteria.
    /// All paths in the index are relative to project root.
    ///
    /// - `root`: only include files under this directory (relative path, e.g., "src" or "")
    /// - `extensions`: file extensions to match (e.g., `[".py", ".pyi"]`)
    /// - `src_exclude_dirs`: directory path segments to skip (e.g., `["/.git/", "/out/"]`)
    /// - `src_exclude_files`: file names to skip (e.g., `["setup.py"]`)
    /// - `src_exclude_paths`: paths relative to project root to skip (e.g., `["Makefile"]`)
    /// - `src_files`: if non-empty, only these paths are matched (allowlist)
    pub fn query(
        &self,
        root: &Path,
        extensions: &[&str],
        src_exclude_dirs: &[&str],
        src_exclude_files: &[&str],
        src_exclude_paths: &[&str],
        src_files: &[&str],
    ) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|path| {
                // src_files: additional explicit files included alongside normal scanning
                // Checked first — these bypass root and extension checks
                if !src_files.is_empty() {
                    let path_str = path.to_string_lossy();
                    if src_files.iter().any(|p| *p == path_str) {
                        return true;
                    }
                }

                // Must be under root (root is relative, e.g., "src" or "")
                // Empty root or "." means match all
                let root_str = root.to_string_lossy();
                if !root_str.is_empty() && root_str != "."
                    && !path.starts_with(root) {
                        return false;
                    }

                // Check exclude dirs
                if !src_exclude_dirs.is_empty() {
                    let path_str = path.to_string_lossy();
                    if src_exclude_dirs.iter().any(|dir| path_str.contains(dir)) {
                        return false;
                    }
                }

                // Check extension match
                // Extensions starting with "." match suffixes (e.g., ".py" matches "foo.py").
                // Extensions without a leading "." are exact filenames (e.g., "Makefile", "requirements.txt").
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !extensions.iter().any(|ext| {
                    if ext.starts_with('.') { name.ends_with(ext) } else { name == *ext }
                }) {
                    return false;
                }

                // Check exclude files
                if !src_exclude_files.is_empty() && src_exclude_files.contains(&name) {
                    return false;
                }

                // Check exclude paths (paths are already relative)
                if !src_exclude_paths.is_empty() {
                    let path_str = path.to_string_lossy();
                    if src_exclude_paths.iter().any(|p| *p == path_str) {
                        return false;
                    }
                }

                true
            })
            .cloned()
            .collect()
    }

    /// Convenience wrapper using `StandardConfig` scan fields.
    /// Returns relative paths.
    ///
    /// - `scan`: processor scan configuration
    /// - `recursive`: if false, only include files at depth 1 from the scan root
    pub fn scan(
        &self,
        scan: &StandardConfig,
        recursive: bool,
    ) -> Vec<PathBuf> {
        let ext_refs: Vec<&str> = scan.src_extensions().iter().map(std::string::String::as_str).collect();
        let exclude_dir_refs: Vec<&str> = scan.src_exclude_dirs().iter().map(std::string::String::as_str).collect();
        let exclude_file_refs: Vec<&str> = scan.src_exclude_files().iter().map(std::string::String::as_str).collect();
        let exclude_path_refs: Vec<&str> = scan.src_exclude_paths().iter().map(std::string::String::as_str).collect();
        let include_path_refs: Vec<&str> = scan.src_files().iter().map(std::string::String::as_str).collect();

        let mut results = Vec::new();
        let src_dirs = scan.src_dirs();
        // Empty src_dirs means scan NOTHING — never "scan the project root".
        // No processor defaults to sweeping the tree: a processor that cannot
        // name where its files live must be told, because guessing means
        // walking node_modules/, .venv/, target/ and vendored code, and
        // linting files the user never intended to own.
        //
        // Scanning the whole project is still available, just never by
        // accident: src_dirs = [""] normalizes to the project root below and
        // walks everything. That is a deliberate, visible opt-in.
        //
        // src_files is the other way to reach the root: those are explicit
        // path allowlists, so query() needs a root to match them against.
        // That is scoped matching of named files, not open-ended discovery.
        let effective_dirs: Vec<&str> = if src_dirs.is_empty() && !include_path_refs.is_empty() {
            vec![""]
        } else {
            src_dirs.iter().map(std::string::String::as_str).collect()
        };
        for dir in &effective_dirs {
            // Normalize "." to "" so depth calculations work correctly
            // (files in the index are stored as relative paths without "./" prefix)
            let root = if *dir == "." || dir.is_empty() { PathBuf::new() } else { PathBuf::from(dir) };
            let mut dir_results = self.query(&root, &ext_refs, &exclude_dir_refs, &exclude_file_refs, &exclude_path_refs, &include_path_refs);

            if !recursive {
                // Filter to depth 1 from scan root: keep only files whose path has
                // exactly one more component than the root.
                let root_depth = root.components().count();
                dir_results.retain(|path| {
                    path.components().count() == root_depth + 1
                });
            }

            results.append(&mut dir_results);
        }
        results.sort();
        results.dedup();
        results
    }

    /// Add virtual files (declared outputs from generators) to the index.
    /// Used by the fixed-point discovery loop so downstream processors can
    /// discover products for files that don't exist on disk yet.
    /// Returns the number of files actually added (not already present).
    pub fn add_virtual_files(&mut self, paths: &[PathBuf]) -> usize {
        // Collect first, insert after: pushing while binary-searching would
        // unsort the vec and make later searches (including duplicates within
        // `paths`) unreliable.
        let mut to_add: Vec<PathBuf> = paths.iter()
            .filter(|p| self.files.binary_search(p).is_err())
            .cloned()
            .collect();
        to_add.sort();
        to_add.dedup();
        let added = to_add.len();
        if added > 0 {
            self.files.extend(to_add);
            self.files.sort();
            self.files.dedup();
        }
        added
    }

    /// Return all files in the index.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Check if the index contains any file with the given extension.
    /// Extension should include the dot, e.g., ".py", ".c".
    pub fn has_extension(&self, ext: &str) -> bool {
        self.files.iter().any(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with(ext))
        })
    }

    /// Check if a specific path exists in the index.
    /// Uses binary search since the file list is sorted.
    pub fn contains(&self, path: &Path) -> bool {
        self.files.binary_search_by(|p| p.as_path().cmp(path)).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_index() -> FileIndex {
        FileIndex::from_paths(vec![
            "src/main.c".into(),
            "src/lib.c".into(),
            "src/util/helper.c".into(),
            "tests/test_main.py".into(),
            "tests/test_lib.py".into(),
            "README.md".into(),
            "Makefile".into(),
            "out/build/app.o".into(),
        ])
    }

    #[test]
    fn contains_finds_existing_path() {
        let idx = sample_index();
        assert!(idx.contains(Path::new("src/main.c")));
        assert!(idx.contains(Path::new("README.md")));
    }

    #[test]
    fn contains_rejects_missing_path() {
        let idx = sample_index();
        assert!(!idx.contains(Path::new("nonexistent.c")));
        assert!(!idx.contains(Path::new("src/missing.c")));
    }

    #[test]
    fn has_extension_finds_existing() {
        let idx = sample_index();
        assert!(idx.has_extension(".c"));
        assert!(idx.has_extension(".py"));
        assert!(idx.has_extension(".md"));
    }

    #[test]
    fn has_extension_rejects_missing() {
        let idx = sample_index();
        assert!(!idx.has_extension(".rs"));
        assert!(!idx.has_extension(".java"));
    }

    #[test]
    fn query_filters_by_extension() {
        let idx = sample_index();
        let results = idx.query(Path::new(""), &[".c"], &[], &[], &[], &[]);
        assert_eq!(results.len(), 3); // main.c, lib.c, helper.c
        assert!(results.iter().all(|p| p.to_string_lossy().ends_with(".c")));
    }

    #[test]
    fn query_filters_by_root() {
        let idx = sample_index();
        let results = idx.query(Path::new("tests"), &[".py"], &[], &[], &[], &[]);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|p| p.starts_with("tests")));
    }

    #[test]
    fn query_excludes_dirs() {
        let idx = sample_index();
        let results = idx.query(Path::new(""), &[".c", ".o"], &["/util/"], &[], &[], &[]);
        assert!(!results.iter().any(|p| p.to_string_lossy().contains("/util/")));
    }

    #[test]
    fn query_excludes_files() {
        let idx = sample_index();
        let results = idx.query(Path::new(""), &[".c"], &[], &["lib.c"], &[], &[]);
        assert!(!results.iter().any(|p| p.file_name().unwrap() == "lib.c"));
        assert!(results.iter().any(|p| p.file_name().unwrap() == "main.c"));
    }

    #[test]
    fn query_excludes_paths() {
        let idx = sample_index();
        let results = idx.query(Path::new(""), &[".c"], &[], &[], &["src/main.c"], &[]);
        assert!(!results.contains(&PathBuf::from("src/main.c")));
        assert!(results.contains(&PathBuf::from("src/lib.c")));
    }

    #[test]
    fn query_empty_root_matches_all() {
        let idx = sample_index();
        let results = idx.query(Path::new(""), &[".md"], &[], &[], &[], &[]);
        assert_eq!(results, vec![PathBuf::from("README.md")]);
    }
}

