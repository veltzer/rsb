use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn pdfunite_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pdfunite]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // No source directory — should succeed with nothing to build
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 products"),
        "Should discover 0 products: {}",
        stdout
    );
}

#[test]
fn pdfunite_discovers_courses() {
    require_tool("pdfunite");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pdfunite]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Create the source directory structure that pdfunite scans
    fs::create_dir_all(project_path.join("marp/courses/intro")).unwrap();
    fs::write(
        project_path.join("marp/courses/intro/lesson1.md"),
        "# Lesson 1\n",
    )
    .unwrap();

    // Run with dry-run to check discovery without needing actual PDFs
    let output = run_rsconstruct_with_env(project_path, &["build", "--dry-run"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Dry run should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
