use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn mdbook_valid_project() {
    require_tool("mdbook");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.mdbook]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Create a minimal mdbook project in a subdirectory
    fs::create_dir_all(project_path.join("docs/src")).unwrap();

    fs::write(
        project_path.join("docs/book.toml"),
        "[book]\ntitle = \"Test\"\n",
    )
    .unwrap();

    fs::write(
        project_path.join("docs/src/SUMMARY.md"),
        "# Summary\n\n- [Chapter 1](./chapter1.md)\n",
    )
    .unwrap();

    fs::write(
        project_path.join("docs/src/chapter1.md"),
        "# Chapter 1\n\nHello world.\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid mdbook project: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process mdbook: {}",
        stdout
    );
}

#[test]
fn mdbook_incremental_skip() {
    require_tool("mdbook");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.mdbook]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    fs::create_dir_all(project_path.join("docs/src")).unwrap();

    fs::write(
        project_path.join("docs/book.toml"),
        "[book]\ntitle = \"Test\"\n",
    )
    .unwrap();

    fs::write(
        project_path.join("docs/src/SUMMARY.md"),
        "# Summary\n\n- [Chapter 1](./chapter1.md)\n",
    )
    .unwrap();

    fs::write(
        project_path.join("docs/src/chapter1.md"),
        "# Chapter 1\n\nHello world.\n",
    )
    .unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());

    // Second build should skip
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("[mdbook] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}

#[test]
fn mdbook_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.mdbook]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 products"),
        "Should discover 0 products: {}",
        stdout
    );
}
