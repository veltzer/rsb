use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn marp_ci_cap_sets_max_jobs_when_ci_true() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.marp]\nsrc_dirs = [\"marp\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(
        project_path,
        &["--json", "processors", "config", "marp"],
        &[("NO_COLOR", "1"), ("CI", "true")],
    );
    assert!(output.status.success(), "command failed: {}",
        String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {}\nstdout: {}", e, stdout));
    let max_jobs = parsed.get("marp")
        .and_then(|m| m.get("max_jobs"))
        .unwrap_or_else(|| panic!("marp.max_jobs missing under CI=true; full dump: {}", stdout));
    assert_eq!(max_jobs.as_i64(), Some(2),
        "expected max_jobs=2 under CI=true, got {:?}", max_jobs);
}

#[test]
fn marp_ci_cap_absent_when_ci_unset() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.marp]\nsrc_dirs = [\"marp\"]\n",
    ).unwrap();

    // CI=false should behave like unset (the hook only fires on CI=="true").
    let output = run_rsconstruct_with_env(
        project_path,
        &["--json", "processors", "config", "marp"],
        &[("NO_COLOR", "1"), ("CI", "false")],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let max_jobs = parsed.get("marp").and_then(|m| m.get("max_jobs"));
    assert!(
        max_jobs.is_none() || max_jobs == Some(&serde_json::Value::Null),
        "expected max_jobs absent without CI=true, got {:?}", max_jobs
    );
}

#[test]
fn marp_ci_cap_respects_user_set_max_jobs() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.marp]\nmax_jobs = 5\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(
        project_path,
        &["--json", "processors", "config", "marp"],
        &[("NO_COLOR", "1"), ("CI", "true")],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let max_jobs = parsed["marp"]["max_jobs"].as_i64();
    assert_eq!(max_jobs, Some(5),
        "user-set max_jobs must win over CI cap; got {:?}", max_jobs);
}

#[test]
fn marp_valid_file() {
    require_tool("marp");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.marp]\nsrc_dirs = [\"marp\"]\nformats = [\"html\"]\n",
    )
    .unwrap();

    fs::create_dir_all(project_path.join("marp")).unwrap();
    fs::write(
        project_path.join("marp/slides.md"),
        "---\nmarp: true\n---\n\n# Slide 1\n\nHello World\n\n---\n\n# Slide 2\n\nGoodbye\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid Marp file: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process marp: {}",
        stdout
    );
}

#[test]
fn marp_nonexistent_src_dir_fails() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.marp]\nsrc_dirs = [\"marp\"]\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build must fail when src_dirs entry doesn't exist");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("marp") && combined.contains("does not exist"),
        "Error must name the missing directory: {}", combined
    );
}
