use std::fs;
use tempfile::TempDir;
use crate::common::run_rsconstruct_with_env;

/// Create a test project with a creator processor that produces two output directories.
/// The script creates dir_a/file_a.txt and dir_b/file_b.txt.
fn setup_creator_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Write the creator script
    let script = project_path.join("create.sh");
    fs::write(&script, concat!(
        "#!/bin/bash\n",
        "set -e\n",
        "mkdir -p dir_a dir_b\n",
        "echo 'content_a' > dir_a/file_a.txt\n",
        "echo 'content_b' > dir_b/file_b.txt\n",
    )).unwrap();
        {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Write the anchor file (the script scans for this)
    fs::write(project_path.join("trigger.manifest"), "trigger\n").unwrap();

    // Configure the creator processor
    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.creator.my_creator]\n",
            "command = \"./create.sh\"\n",
            "src_extensions = [\".manifest\"]\n",
            "src_dirs = [\".\"]\n",
            "output_dirs = [\"dir_a\", \"dir_b\"]\n",
        ),
    ).unwrap();

    temp_dir
}

#[test]
fn creator_produces_two_output_dirs() {
    let temp_dir = setup_creator_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Verify both dirs and files were created
    assert!(project_path.join("dir_a/file_a.txt").exists(), "dir_a/file_a.txt should exist");
    assert!(project_path.join("dir_b/file_b.txt").exists(), "dir_b/file_b.txt should exist");
    assert_eq!(fs::read_to_string(project_path.join("dir_a/file_a.txt")).unwrap().trim(), "content_a");
    assert_eq!(fs::read_to_string(project_path.join("dir_b/file_b.txt")).unwrap().trim(), "content_b");
}

#[test]
fn creator_clean_removes_output_dirs() {
    let temp_dir = setup_creator_project();
    let project_path = temp_dir.path();

    // Build first
    let build = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success());
    assert!(project_path.join("dir_a").exists());
    assert!(project_path.join("dir_b").exists());

    // Clean outputs
    let clean = run_rsconstruct_with_env(project_path, &["clean", "outputs"], &[("NO_COLOR", "1")]);
    assert!(
        clean.status.success(),
        "Clean should succeed: stderr={}",
        String::from_utf8_lossy(&clean.stderr),
    );

    // Verify dirs are gone
    assert!(!project_path.join("dir_a").exists(), "dir_a should be removed after clean");
    assert!(!project_path.join("dir_b").exists(), "dir_b should be removed after clean");
    // Cache should still exist
    assert!(project_path.join(".rsconstruct").exists(), "cache should be preserved");
}

#[test]
fn creator_restores_output_dirs_from_cache() {
    let temp_dir = setup_creator_project();
    let project_path = temp_dir.path();

    // Build to populate cache
    let build = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success());

    // Clean outputs (but preserve cache)
    let clean = run_rsconstruct_with_env(project_path, &["clean", "outputs"], &[("NO_COLOR", "1")]);
    assert!(clean.status.success());
    assert!(!project_path.join("dir_a").exists());
    assert!(!project_path.join("dir_b").exists());

    // Rebuild — should restore from cache
    let restore = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(
        restore.status.success(),
        "Restore build should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr),
    );

    let stdout = String::from_utf8_lossy(&restore.stdout);
    assert!(
        stdout.contains("Restored from cache:"),
        "Should restore from cache: {}",
        stdout,
    );

    // Verify restored content is correct
    assert!(project_path.join("dir_a/file_a.txt").exists(), "dir_a/file_a.txt should be restored");
    assert!(project_path.join("dir_b/file_b.txt").exists(), "dir_b/file_b.txt should be restored");
    assert_eq!(fs::read_to_string(project_path.join("dir_a/file_a.txt")).unwrap().trim(), "content_a");
    assert_eq!(fs::read_to_string(project_path.join("dir_b/file_b.txt")).unwrap().trim(), "content_b");
}

#[test]
fn creator_incremental_skip() {
    let temp_dir = setup_creator_project();
    let project_path = temp_dir.path();

    // First build
    let build1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(build1.status.success());

    // Second build — should skip
    let build2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(
        build2.status.success(),
        "Second build should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&build2.stdout),
        String::from_utf8_lossy(&build2.stderr),
    );
    let stdout = String::from_utf8_lossy(&build2.stdout);
    assert!(
        stdout.contains("Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout,
    );
}
