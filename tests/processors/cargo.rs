use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

/// Config that builds only the dev profile for faster tests
const SINGLE_PROFILE_CONFIG: &str = "[processor.cargo]\nprofiles = [\"dev\"]\nsrc_dirs = [\".\"]\n";

#[test]
fn cargo_valid_project() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        SINGLE_PROFILE_CONFIG,
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
        "Should process cargo: {}",
        stdout
    );
}

#[test]
fn cargo_incremental_skip() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        SINGLE_PROFILE_CONFIG,
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
        stdout2.contains("[cargo] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}

#[test]
fn cargo_rebuild_on_source_change() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        SINGLE_PROFILE_CONFIG,
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
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());

    // Modify source file
    fs::write(
        project_path.join("mylib/src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n",
    )
    .unwrap();

    // Second build should rebuild (not skip)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("Processing:"),
        "Should rebuild after source change: {}",
        stdout2
    );
}

#[test]
fn cargo_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cargo]\nsrc_dirs = [\".\"]\n",
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
fn cargo_build_failure() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        SINGLE_PROFILE_CONFIG,
    )
    .unwrap();

    fs::create_dir_all(project_path.join("badlib/src")).unwrap();

    fs::write(
        project_path.join("badlib/Cargo.toml"),
        "[package]\nname = \"badlib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Invalid Rust code
    fs::write(
        project_path.join("badlib/src/lib.rs"),
        "this is not valid rust code!!!\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should fail with invalid Rust code"
    );
}

#[test]
fn cargo_check_command() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Use "cargo check" instead of "cargo build", single profile for speed
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cargo]\ncommand = \"check\"\nprofiles = [\"dev\"]\nsrc_dirs = [\".\"]\n",
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
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "cargo check should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process cargo: {}",
        stdout
    );
}

#[test]
fn cargo_multi_profile() {
    require_tool("cargo");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Default profiles: dev + release. Serialize with max_jobs=1 so the two
    // cargo invocations don't race on the shared mylib/target/ directory.
    // This test verifies product discovery (dev + release = 2 products), not
    // concurrent execution — the race is a separate concern.
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cargo]\nsrc_dirs = [\".\"]\nmax_jobs = 1\n",
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
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Multi-profile build should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("2 products"),
        "Should discover 2 products (dev + release): {}",
        stdout
    );
}
