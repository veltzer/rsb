use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn make_valid_makefile() {
    require_tool("make");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Put the Makefile in a subdirectory so .rsconstruct/ cache files
    // (which are sibling-scanned by the make processor) don't
    // cause spurious rebuilds.
    fs::create_dir_all(project_path.join("proj")).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.make]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    fs::write(
        project_path.join("proj/Makefile"),
        ".PHONY: all\nall:\n\t@echo \"hello from make\"\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid Makefile: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process make: {}",
        stdout
    );
}

#[test]
fn make_incremental_skip() {
    require_tool("make");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::create_dir_all(project_path.join("proj")).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.make]\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    fs::write(
        project_path.join("proj/Makefile"),
        ".PHONY: all\nall:\n\t@echo \"hello from make\"\n",
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
        stdout2.contains("[make] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}
