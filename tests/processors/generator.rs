use std::fs;
use tempfile::TempDir;
use crate::common::run_rsconstruct_with_env;
use std::os::unix::fs::PermissionsExt;

#[test]
fn generator_rebuilds_when_command_file_changes() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    let script_path = project_path.join("gen.sh");
    fs::write(&script_path, "#!/bin/bash\ncp \"$1\" \"$2\"\n").unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        format!(
            "[processor.generator]\ncommand = \"{script}\"\nsrc_extensions = [\".txt\"]\nsrc_dirs = [\".\"]\noutput_extension = \"out\"\n",
            script = script_path.display(),
        ),
    ).unwrap();
    fs::write(project_path.join("input.txt"), "hello\n").unwrap();

    // First build
    let out1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "First build should succeed: stderr={}", String::from_utf8_lossy(&out1.stderr));

    // Second build: unchanged — should skip
    let out2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    assert!(
        String::from_utf8_lossy(&out2.stdout).contains("Skipping"),
        "Second build should skip: {}",
        String::from_utf8_lossy(&out2.stdout),
    );

    // Modify the command script
    fs::write(&script_path, "#!/bin/bash\ncp \"$1\" \"$2\"\n# changed\n").unwrap();

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
