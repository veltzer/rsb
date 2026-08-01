use std::fs;

use crate::common::{run_rsconstruct, setup_project_with_config};

/// Tera requires `tera.templates/` to exist; create it so the build doesn't
/// fail on the src_dirs check.
fn create_tera_templates(temp_path: &std::path::Path) {
    fs::create_dir_all(temp_path.join("tera.templates")).unwrap();
}

/// `--iset` errors when iname doesn't match any declared instance.
#[test]
fn iset_unknown_iname_errors() {
    let temp = setup_project_with_config("[processor.tera]\n");
    let out = run_rsconstruct(temp.path(), &["build", "--iset", "nope.max_jobs=2"]);
    assert!(!out.status.success(), "expected failure for unknown iname");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nope") && stderr.contains("iname"),
        "stderr should mention the bad iname; got: {stderr}"
    );
}

/// `--pset` errors when pname has no matching instances.
#[test]
fn pset_unknown_pname_errors() {
    let temp = setup_project_with_config("[processor.tera]\n");
    let out = run_rsconstruct(temp.path(), &["build", "--pset", "ruff.max_jobs=2"]);
    assert!(!out.status.success(), "expected failure for unknown pname");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ruff") && stderr.contains("pname"),
        "stderr should mention the bad pname; got: {stderr}"
    );
}

/// Override targeting an unknown field on a real instance must error.
#[test]
fn iset_unknown_field_errors() {
    let temp = setup_project_with_config("[processor.tera]\n");
    let out = run_rsconstruct(temp.path(), &["build", "--iset", "tera.bogus_field=1"]);
    assert!(!out.status.success(), "expected failure for unknown field");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bogus_field") && stderr.contains("unknown field"),
        "stderr should report unknown field; got: {stderr}"
    );
}

/// Type mismatch (string for an integer field) must error.
#[test]
fn iset_type_mismatch_errors() {
    let temp = setup_project_with_config("[processor.tera]\n");
    let out = run_rsconstruct(temp.path(), &["build", "--iset", "tera.max_jobs=hello"]);
    assert!(!out.status.success(), "expected failure for type mismatch");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("max_jobs") && stderr.contains("integer"),
        "stderr should report type mismatch; got: {stderr}"
    );
}

/// Malformed entries (no '.', no '=') must error.
#[test]
fn iset_malformed_entries_error() {
    let temp = setup_project_with_config("[processor.tera]\n");

    let out = run_rsconstruct(temp.path(), &["build", "--iset", "no_equals"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing '='"));

    let out = run_rsconstruct(temp.path(), &["build", "--iset", "noDot=2"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing '.'"));
}

/// A valid `--iset` setting an integer field on an existing instance must succeed
/// (the build itself may still be a no-op for tera with no templates, but the
/// override should not block startup).
#[test]
fn iset_valid_max_jobs_accepted() {
    let temp = setup_project_with_config("[processor.tera]\n");
    create_tera_templates(temp.path());
    let out = run_rsconstruct(temp.path(), &["build", "--iset", "tera.max_jobs=2"]);
    assert!(
        out.status.success(),
        "expected success; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--iset` must reach named instances of multi-instance processors: the
/// iname itself contains a dot (`ascii.one`), so the name/field split has to
/// happen at the LAST dot.
#[test]
fn iset_multi_instance_dotted_iname() {
    let temp = setup_project_with_config(
        "[processor.ascii.one]\nsrc_dirs = [\"docs_a\"]\n\n[processor.ascii.two]\nsrc_dirs = [\"docs_b\"]\n",
    );
    fs::create_dir_all(temp.path().join("docs_a")).unwrap();
    fs::create_dir_all(temp.path().join("docs_b")).unwrap();
    let out = run_rsconstruct(temp.path(), &["build", "--iset", "ascii.one.max_jobs=2"]);
    assert!(
        out.status.success(),
        "dotted iname override must resolve; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A valid `--pset` targeting all instances of a type must succeed.
#[test]
fn pset_valid_max_jobs_accepted() {
    let temp = setup_project_with_config("[processor.tera]\n");
    create_tera_templates(temp.path());
    let out = run_rsconstruct(temp.path(), &["build", "--pset", "tera.max_jobs=3"]);
    assert!(
        out.status.success(),
        "expected success; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Overrides must go through the same semantic validation as the config
/// file: max_jobs = 0 would deadlock the build on a zero-permit semaphore,
/// so it must be rejected, not applied.
#[test]
fn iset_max_jobs_zero_rejected() {
    let temp = setup_project_with_config("[processor.tera]\n");
    create_tera_templates(temp.path());
    let out = run_rsconstruct(temp.path(), &["build", "--iset", "tera.max_jobs=0"]);
    assert!(!out.status.success(), "expected failure for max_jobs=0 override");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("max_jobs") && stderr.contains("greater than 0"),
        "stderr should explain the max_jobs guard; got: {stderr}"
    );
    assert_eq!(out.status.code(), Some(2), "config errors exit with 2");
}
