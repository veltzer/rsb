use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn libreoffice_valid_file() {
    require_tool("libreoffice");
    require_tool("flock");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.libreoffice]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Create a minimal ODP file (LibreOffice presentation is a ZIP with XML)
    // For a real test, we'd need a proper ODP, but for discovery we just need the extension
    fs::write(
        project_path.join("slides.odp"),
        "placeholder",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    // This may fail if the ODP content is invalid, but it should at least discover and attempt processing
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:") || stdout.contains("1 products"),
        "Should discover libreoffice product: stdout={}, stderr={}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn libreoffice_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.libreoffice]\nsrc_dirs = [\".\"]\n",
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
