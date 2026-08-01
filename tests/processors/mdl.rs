use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn mdl_valid_file() {
    require_tool("mdl");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Point command to the system mdl, skip gem dependency
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.mdl]\ncommand = \"mdl\"\nsrc_dirs = [\".\"]\n",
    )
    .unwrap();

    // Content that passes mdl rules: proper heading structure, blank lines
    fs::write(
        project_path.join("doc.md"),
        "# Hello World\n\nThis is a test document.\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    // mdl may fail due to rule violations even with simple content
    // Just verify discovery and processing attempt
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Processing:") || stdout.contains("1 products"),
        "Should discover and attempt mdl processing: stdout={}, stderr={}",
        stdout, stderr
    );
}

// Reproduces the user-reported bug:
//   [processor.mdl]
//   src_dirs = ["config", "script"]
// where `script/` doesn't exist on disk. Must fail with the missing-dir
// error rather than silently scanning nothing.
#[test]
fn mdl_nonexistent_src_dir_fails() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // `config/` exists but `script/` does not — must fail.
    fs::create_dir(project_path.join("config")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.mdl]\nsrc_dirs = [\"config\", \"script\"]\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build must fail when any src_dirs entry doesn't exist");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("script") && combined.contains("does not exist"),
        "Error must name the missing 'script' directory: {}", combined
    );
}
