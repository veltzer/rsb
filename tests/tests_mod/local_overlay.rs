//! Tests for the two shared-config features:
//! - `[build] skip_missing_src_dirs` — missing src_dirs entries deactivate the
//!   scan instead of failing the build.
//! - `rsconstruct.local.toml` — a per-repo overlay deep-merged over the main
//!   config at load time.

use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct, setup_project_with_config, write_file};

#[test]
fn missing_src_dirs_fails_by_default() {
    let temp_dir = setup_project_with_config(
        "[processor.tera]\nsrc_dirs = [\"missing.templates\"]\n",
    );
    let output = run_rsconstruct(temp_dir.path(), &["build"]);
    assert!(!output.status.success(), "build should fail on a missing src_dirs entry");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("src_dirs entry 'missing.templates'"),
        "expected missing-dir error, got: {stderr}"
    );
}

#[test]
fn skip_missing_src_dirs_allows_missing_dirs() {
    let temp_dir = setup_project_with_config(concat!(
        "[build]\n",
        "skip_missing_src_dirs = true\n",
        "\n",
        "[processor.tera.real]\n",
        "src_dirs = [\"tera.templates\"]\n",
        "\n",
        "[processor.tera.ghost]\n",
        "src_dirs = [\"missing.templates\"]\n",
    ));
    let project = temp_dir.path();
    write_file(project, "tera.templates/hello.txt.tera", "hello");

    let output = run_rsconstruct(project, &["build"]);
    assert!(
        output.status.success(),
        "build should skip the missing dir: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The instance with a real directory still produced its output.
    assert!(project.join("hello.txt").exists());
}

#[test]
fn local_overlay_disables_processor() {
    let temp_dir = setup_project_with_config("[processor.tera]\n");
    let project = temp_dir.path();
    write_file(project, "tera.templates/gen.txt.tera", "generated");
    fs::write(
        project.join("rsconstruct.local.toml"),
        "[processor.tera]\nenabled = false\n",
    ).unwrap();

    let output = run_rsconstruct(project, &["build"]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The overlay disabled tera, so nothing was rendered.
    assert!(!project.join("gen.txt").exists());
}

#[test]
fn local_overlay_adds_sections() {
    let temp_dir = setup_project_with_config("[processor.tera]\n");
    let project = temp_dir.path();
    write_file(project, "tera.templates/gen.txt.tera", "generated");
    // The overlay adds a global [build] flag and a whole new processor whose
    // src_dirs doesn't exist — the build only succeeds if both merged in.
    fs::write(
        project.join("rsconstruct.local.toml"),
        concat!(
            "[build]\n",
            "skip_missing_src_dirs = true\n",
            "\n",
            "[processor.zspell]\n",
            "src_dirs = [\"absent_docs\"]\n",
        ),
    ).unwrap();

    let output = run_rsconstruct(project, &["build"]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.join("gen.txt").exists());
}

#[test]
fn local_overlay_field_wins_over_main() {
    let temp_dir = setup_project_with_config(
        "[processor.tera]\ndep_inputs = [\"config/a.py\"]\n",
    );
    let project = temp_dir.path();
    fs::write(
        project.join("rsconstruct.local.toml"),
        "[processor.tera]\ndep_inputs = [\"config/b.py\"]\n",
    ).unwrap();

    let output = run_rsconstruct(project, &["processors", "config", "tera"]);
    assert!(
        output.status.success(),
        "processors config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("b.py"), "local value should win, got: {stdout}");
    assert!(!stdout.contains("a.py"), "main value should be replaced, got: {stdout}");
}

#[test]
fn local_overlay_without_main_config_fails() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::write(
        temp_dir.path().join("rsconstruct.local.toml"),
        "[processor.tera]\n",
    ).unwrap();

    let output = run_rsconstruct(temp_dir.path(), &["build"]);
    assert!(!output.status.success(), "build should fail without a main config");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rsconstruct.local.toml found without rsconstruct.toml"),
        "expected overlay-without-main error, got: {stderr}"
    );
}
