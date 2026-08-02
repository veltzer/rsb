use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn markdownlint_valid_file() {
    require_tool("markdownlint");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Point command to the system markdownlint, skip npm dependency
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.markdownlint]\ncommand = \"markdownlint\"\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    fs::write(
        project_path.join("doc.md"),
        "# Hello World\n\nThis is a test.\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid markdown: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process markdownlint: {}",
        stdout
    );
}

#[test]
fn markdownlint_nonexistent_src_dir_scans_nothing() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.markdownlint]\nsrc_dirs = [\"mdlint_docs\"]\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build must succeed: a missing src_dirs entry scans nothing. {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
