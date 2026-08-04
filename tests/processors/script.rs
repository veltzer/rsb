use std::fs;
use tempfile::TempDir;
use crate::common::run_rsconstruct_with_env;
use std::os::unix::fs::PermissionsExt;

#[test]
fn script_valid_file() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // script is disabled by default, so we must explicitly enable and configure it
    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.script]\n",
            "command = \"true\"\n",
            "src_extensions = [\".txt\"]\n",
            "src_dirs = [\".\"]\n",
        ),
    )
    .unwrap();

    fs::write(
        project_path.join("test.txt"),
        "hello world\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with script using 'true': stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process script: {}",
        stdout
    );
}

#[test]
fn script_incremental_skip() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.script]\n",
            "command = \"true\"\n",
            "src_extensions = [\".txt\"]\n",
            "src_dirs = [\".\"]\n",
        ),
    )
    .unwrap();

    fs::write(
        project_path.join("test.txt"),
        "hello world\n",
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
        stdout2.contains("[script] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}

#[test]
fn script_misspelled_linter_fails_immediately() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.script]\n",
            "command = \"no_such_command_xyzzy\"\n",
            "src_extensions = [\".txt\"]\n",
            "src_dirs = [\".\"]\n",
        ),
    )
    .unwrap();

    fs::write(
        project_path.join("test.txt"),
        "hello world\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should fail when linter does not exist"
    );

    let exit_code = output.status.code().unwrap();
    assert_eq!(exit_code, 3, "Expected exit code 3 (TOOL_ERROR), got {}", exit_code);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing required tools") || stderr.contains("TOOL_ERROR"),
        "Should report missing tool error: {}",
        stderr
    );
}

#[test]
fn script_multi_instance_both_discover_files() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.script.lint_a]\n",
            "command = \"true\"\n",
            "src_extensions = [\".txt\"]\n",
            "src_dirs = [\".\"]\n",
            "\n",
            "[processor.script.lint_b]\n",
            "command = \"true\"\n",
            "src_extensions = [\".txt\"]\n",
            "src_dirs = [\".\"]\n",
        ),
    )
    .unwrap();

    fs::write(project_path.join("test.txt"), "hello\n").unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[script.lint_a]"),
        "Should process script.lint_a: {}",
        stdout
    );
    assert!(
        stdout.contains("[script.lint_b]"),
        "Should process script.lint_b: {}",
        stdout
    );
}

#[test]
fn script_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Without configuring extensions or checker, script should discover nothing
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.script]\nsrc_dirs = [\".\"]\n",
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

#[test]
fn script_rebuilds_when_command_file_changes() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    let script_path = project_path.join("check.sh");
    fs::write(&script_path, "#!/bin/bash\nexit 0\n").unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        format!(
            "[processor.script]\ncommand = \"{script}\"\nsrc_extensions = [\".txt\"]\nsrc_dirs = [\".\"]\n",
            script = script_path.display(),
        ),
    ).unwrap();
    fs::write(project_path.join("test.txt"), "hello\n").unwrap();

    // First build
    let out1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "First build should succeed");

    // Second build: unchanged — should skip
    let out2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    assert!(
        String::from_utf8_lossy(&out2.stdout).contains("Skipping"),
        "Second build should skip: {}",
        String::from_utf8_lossy(&out2.stdout),
    );

    // Modify the command script
    fs::write(&script_path, "#!/bin/bash\nexit 0\n# changed\n").unwrap();

    // Third build: command changed — must rebuild
    let out3 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out3.status.success());
    let stdout3 = String::from_utf8_lossy(&out3.stdout);
    assert!(
        stdout3.contains("Processing:"),
        "Third build must rebuild after command file change: {}",
        stdout3,
    );
}
