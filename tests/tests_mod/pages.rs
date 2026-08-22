use crate::common::{run_rsconstruct, setup_project_with_config};

#[test]
fn pages_dir_prints_configured_dir() {
    let temp_dir = setup_project_with_config("[pages]\ndir = \"out/web\"\n");
    let output = run_rsconstruct(temp_dir.path(), &["pages", "dir"]);
    assert!(output.status.success(), "pages dir failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "out/web");
}

#[test]
fn pages_dir_prints_nothing_when_not_configured() {
    // No [pages] section: empty stdout and exit 0, so CI can branch on the
    // output being empty instead of parsing exit codes.
    let temp_dir = setup_project_with_config("[build]\nparallel = 1\n");
    let output = run_rsconstruct(temp_dir.path(), &["pages", "dir"]);
    assert!(output.status.success(), "pages dir failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[test]
fn pages_dir_json_reports_configured_state() {
    let temp_dir = setup_project_with_config("[pages]\ndir = \"_site\"\n");
    let output = run_rsconstruct(temp_dir.path(), &["--json", "pages", "dir"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json["configured"], true);
    assert_eq!(json["dir"], "_site");

    let temp_dir = setup_project_with_config("");
    let output = run_rsconstruct(temp_dir.path(), &["--json", "pages", "dir"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json["configured"], false);
    assert_eq!(json["dir"], "");
}

#[test]
fn pages_section_requires_dir() {
    let temp_dir = setup_project_with_config("[pages]\n");
    let output = run_rsconstruct(temp_dir.path(), &["pages", "dir"]);
    assert!(!output.status.success(), "a [pages] section without dir should be a config error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing field `dir`"), "unexpected error: {stderr}");
}

#[test]
fn pages_section_rejects_unknown_fields() {
    let temp_dir = setup_project_with_config("[pages]\ndir = \"out/web\"\npath = \"x\"\n");
    let output = run_rsconstruct(temp_dir.path(), &["pages", "dir"]);
    assert!(!output.status.success(), "unknown fields in [pages] should be a config error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown field"), "unexpected error: {stderr}");
}
