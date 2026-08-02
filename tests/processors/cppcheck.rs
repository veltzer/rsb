use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn cppcheck_valid_c_file() {
    require_tool("cppcheck");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::create_dir_all(project_path.join("src")).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cppcheck]\nsrc_dirs = [\"src\"]\n",
    )
    .unwrap();

    fs::write(
        project_path.join("src/main.c"),
        "#include <stdio.h>\nint main(void) {\n    printf(\"hello\\n\");\n    return 0;\n}\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid C file: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:") || stdout.contains("Processing batch:"),
        "Should process cppcheck: {}",
        stdout
    );
}

#[test]
fn cppcheck_incremental_skip() {
    require_tool("cppcheck");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::create_dir_all(project_path.join("src")).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cppcheck]\nsrc_dirs = [\"src\"]\n",
    )
    .unwrap();

    fs::write(
        project_path.join("src/main.c"),
        "#include <stdio.h>\nint main(void) {\n    printf(\"hello\\n\");\n    return 0;\n}\n",
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
        stdout2.contains("[cppcheck] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}
