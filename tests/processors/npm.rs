use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn npm_valid_project() {
    require_tool("npm");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.npm]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Exclude node_modules from file index so npm's package.json isn't rediscovered
    fs::write(project_path.join(".rsconstructignore"), "node_modules/\n").unwrap();

    // Need at least one dependency so npm creates node_modules
    fs::write(
        project_path.join("package.json"),
        "{\"name\": \"test\", \"version\": \"1.0.0\", \"dependencies\": {\"is-number\": \"^7.0.0\"}}\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid package.json: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process npm: {}",
        stdout
    );
}

#[test]
fn npm_incremental_skip() {
    require_tool("npm");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.npm]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Ignore node_modules and package-lock.json (created by npm install)
    fs::write(project_path.join(".rsconstructignore"), "node_modules/\npackage-lock.json\n").unwrap();

    fs::write(
        project_path.join("package.json"),
        "{\"name\": \"test\", \"version\": \"1.0.0\", \"dependencies\": {\"is-number\": \"^7.0.0\"}}\n",
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
        stdout2.contains("[npm] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}

#[test]
fn npm_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.npm]\nsrc_dirs = [\".\"]\n",
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
