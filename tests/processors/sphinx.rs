use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn sphinx_valid_project() {
    require_tool("sphinx-build");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.sphinx]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Create a minimal Sphinx project in a "sphinx" subdirectory.
    // sphinx-build runs from project root: `sphinx-build sphinx docs`
    fs::create_dir_all(project_path.join("sphinx")).unwrap();

    fs::write(
        project_path.join("sphinx/conf.py"),
        "project = 'Test'\nextensions = []\n",
    )
    .unwrap();

    fs::write(
        project_path.join("sphinx/index.rst"),
        "Test\n====\n\nHello world.\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid Sphinx project: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process sphinx: {}",
        stdout
    );
}

#[test]
fn sphinx_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.sphinx]\nsrc_dirs = [\".\"]\n",
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
