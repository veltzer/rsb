use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

fn setup_pytest_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::create_dir_all(temp_dir.path().join("tests")).expect("Failed to create tests dir");
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.pytest]\nsrc_dirs = [\"tests\"]\n"
    ).expect("Failed to write rsconstruct.toml");
    temp_dir
}

#[test]
fn pytest_passing_test() {
    require_tool("pytest");

    let temp_dir = setup_pytest_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tests/test_example.py"),
        "def test_addition():\n    assert 1 + 1 == 2\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with passing test: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Processing:"), "Should process pytest: {}", stdout);
}

#[test]
fn pytest_incremental_skip() {
    require_tool("pytest");

    let temp_dir = setup_pytest_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tests/test_skip.py"),
        "def test_trivial():\n    assert True\n"
    ).unwrap();

    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());

    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("[pytest] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}

#[test]
fn pytest_failing_test() {
    require_tool("pytest");

    let temp_dir = setup_pytest_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tests/test_fail.py"),
        "def test_failure():\n    assert 1 == 2\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should fail with failing test"
    );
}
