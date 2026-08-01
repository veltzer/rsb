use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn clippy_valid_project() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.clippy]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Create a minimal Rust library project
    fs::create_dir_all(project_path.join("mylib/src")).unwrap();

    fs::write(
        project_path.join("mylib/Cargo.toml"),
        "[package]\nname = \"mylib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    fs::write(
        project_path.join("mylib/src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid Cargo project: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process clippy: {}",
        stdout
    );
}

#[test]
fn clippy_incremental_skip() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.clippy]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    fs::create_dir_all(project_path.join("mylib/src")).unwrap();

    fs::write(
        project_path.join("mylib/Cargo.toml"),
        "[package]\nname = \"mylib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    fs::write(
        project_path.join("mylib/src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
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
        stdout2.contains("[clippy] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}

#[test]
fn clippy_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.clippy]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // No Cargo.toml anywhere — should succeed with nothing to build
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with no Cargo.toml: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 products"),
        "Should discover 0 products: {}",
        stdout
    );
}

#[test]
fn clippy_lint_failure() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Use -D warnings so clippy warnings become errors
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.clippy]\nargs = [\"--\", \"-D\", \"warnings\"]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    fs::create_dir_all(project_path.join("badlib/src")).unwrap();

    fs::write(
        project_path.join("badlib/Cargo.toml"),
        "[package]\nname = \"badlib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Code with a clippy warning (needless return)
    fs::write(
        project_path.join("badlib/src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    return a + b;\n}\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should fail with clippy warnings when -D warnings is set"
    );
}
