//! Integration tests for the duplicate_files checker.

use std::fs;
use tempfile::TempDir;
use crate::common::run_rsconstruct_with_env;

fn setup_project(files: &[(&str, &str)]) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();
    fs::write(
        p.join("rsconstruct.toml"),
        "[processor.duplicate_files]\nsrc_dirs = [\".\"]\n",
    ).unwrap();
    for (name, content) in files {
        fs::write(p.join(name), content).unwrap();
    }
    temp_dir
}

#[test]
fn duplicate_files_detects_duplicates() {
    let temp_dir = setup_project(&[
        ("a.md", "same content\n"),
        ("b.md", "same content\n"),
        ("c.md", "different content\n"),
    ]);
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with duplicate files");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("duplicate"), "should mention duplicates: {}", stderr);
    assert!(stderr.contains("a.md") && stderr.contains("b.md"),
        "should name both duplicates: {}", stderr);
}

#[test]
fn duplicate_files_passes_distinct() {
    let temp_dir = setup_project(&[
        ("a.md", "content one\n"),
        ("b.md", "content two\n"),
    ]);
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "build should succeed with distinct files: stderr={}",
        String::from_utf8_lossy(&output.stderr));
}

/// A duplicate of an old, unchanged file added later must still be detected:
/// the whole-set product's input list changes, invalidating the cache.
#[test]
fn duplicate_files_detects_duplicate_added_incrementally() {
    let temp_dir = setup_project(&[
        ("a.md", "original content\n"),
        ("b.md", "other content\n"),
    ]);
    let p = temp_dir.path();

    let first = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(first.status.success(), "initial build should pass: stderr={}",
        String::from_utf8_lossy(&first.stderr));

    // Add a copy of an existing, unchanged file
    fs::write(p.join("copy.md"), "original content\n").unwrap();

    let second = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(!second.status.success(), "rebuild should detect the new duplicate");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("a.md") && stderr.contains("copy.md"),
        "should name the duplicate pair: {}", stderr);
}
