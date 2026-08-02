//! Tests for the two shared-config features:
//! - Missing `src_dirs` entries deactivate the scan instead of failing the
//!   build, always — this is unconditional, not behind a flag.
//! - `rsconstruct.local.toml` — a per-repo overlay deep-merged over the main
//!   config at load time.

use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct, setup_project_with_config, write_file};

/// A src_dirs entry that doesn't exist scans nothing rather than failing.
/// src_dirs scans only what it names, so naming an absent directory already
/// means "scan nothing" by another route — and this is what lets one shared
/// rsconstruct.toml list every directory the family of repos might have.
#[test]
fn missing_src_dirs_scans_nothing() {
    let temp_dir = setup_project_with_config(
        "[processor.tera]\nsrc_dirs = [\"missing.templates\"]\n",
    );
    let output = run_rsconstruct(temp_dir.path(), &["build"]);
    assert!(
        output.status.success(),
        "build should succeed with a missing src_dirs entry: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn missing_src_dirs_does_not_disturb_sibling_instances() {
    let temp_dir = setup_project_with_config(concat!(
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
fn tool_check_is_deferred_to_processors_with_products() {
    // In shared-config mode a declared processor whose tool is absent must
    // not fail the build when it also has no products in this repo.
    let temp_dir = setup_project_with_config(concat!(
        "[processor.tera]\n",
        "src_dirs = [\"tera.templates\"]\n",
        "\n",
        "[processor.script.ghost_check]\n",
        "command = \"scripts/does_not_exist.py\"\n",
        "src_dirs = [\"ghost_src\"]\n",
        "src_extensions = [\".md\"]\n",
    ));
    let project = temp_dir.path();
    write_file(project, "tera.templates/out.txt.tera", "ok");

    let output = run_rsconstruct(project, &["build"]);
    assert!(
        output.status.success(),
        "tool-less zero-product processor should not fail the build: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.join("out.txt").exists());
}

/// Deferring the tool check to processors with products must not weaken it:
/// a processor that DOES match files still fails when its tool is absent.
#[test]
fn missing_tool_fails_when_processor_has_products() {
    let temp_dir = setup_project_with_config(concat!(
        "[processor.script.real_check]\n",
        "command = \"scripts/does_not_exist.py\"\n",
        "src_dirs = [\"checked\"]\n",
        "src_extensions = [\".md\"]\n",
    ));
    let project = temp_dir.path();
    write_file(project, "checked/doc.md", "# doc");

    let output = run_rsconstruct(project, &["build"]);
    assert!(!output.status.success(), "missing tool must fail when the processor has products");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing required tools"),
        "expected strict tool preflight, got: {stderr}"
    );
}

#[test]
fn local_overlay_disables_processor() {
    let temp_dir = setup_project_with_config("[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n");
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
    let temp_dir = setup_project_with_config("[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n");
    let project = temp_dir.path();
    write_file(project, "tera.templates/gen.txt.tera", "generated");
    // The overlay adds a global [build] setting and a whole new processor
    // section — both must merge in.
    fs::write(
        project.join("rsconstruct.local.toml"),
        concat!(
            "[build]\n",
            "max_discovery_passes = 7\n",
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

    // Both overlay sections merged: the [build] value is visible in the
    // merged config, and the added processor section is a known instance.
    let cfg = run_rsconstruct(project, &["processors", "config", "zspell"]);
    assert!(
        cfg.status.success(),
        "overlay-added [processor.zspell] should exist in the merged config: {}",
        String::from_utf8_lossy(&cfg.stderr)
    );
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
