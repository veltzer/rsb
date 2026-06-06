use crate::common::{setup_test_project, run_rsconstruct_with_env};
use serde_json::Value;

#[test]
fn tools_list_shows_all_registry_tools() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // `tools list` shows the central registry regardless of config, like
    // `processors list`. It lists tools no processor in this project needs.
    let output = run_rsconstruct_with_env(project_path, &["tools", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "tools list failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "tools list should show at least one tool");
    // The registry view is not processor-scoped, so it has no "(...)" column.
    assert!(!stdout.contains("("), "registry list should not show processor names in parentheses");
    // It includes tools the minimal test project does not require.
    assert!(stdout.contains("clojure"), "registry list should include all known tools, e.g. clojure");
}

#[test]
fn tools_list_shows_configured_tools() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["tools", "list-configured"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "tools list-configured failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Template processor requires python3, so list-configured always has output.
    assert!(!stdout.is_empty(), "tools list-configured should show at least one tool");
    assert!(stdout.contains("("), "Expected processor name in parentheses for each tool");
}

#[test]
fn tools_list_configured_all_includes_disabled() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output_default = run_rsconstruct_with_env(project_path, &["tools", "list-configured"], &[("NO_COLOR", "1")]);
    let output_all = run_rsconstruct_with_env(project_path, &["tools", "list-configured", "-a"], &[("NO_COLOR", "1")]);

    assert!(output_default.status.success());
    assert!(output_all.status.success());

    let stdout_default = String::from_utf8_lossy(&output_default.stdout);
    let stdout_all = String::from_utf8_lossy(&output_all.stdout);

    // -a should show at least as many tool entries as the default
    let count_default = stdout_default.lines().count();
    let count_all = stdout_all.lines().count();
    assert!(count_all >= count_default,
        "tools list-configured -a should include at least as many tools as default ({} vs {})",
        count_all, count_default);
}

#[test]
fn tools_list_json() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["--json", "tools", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "tools list --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .expect("Expected valid JSON array");

    // Check that every entry has the expected fields
    for entry in &entries {
        assert!(entry.get("tool").is_some(), "Entry should have 'tool' field");
        assert!(entry.get("processors").is_some(), "Entry should have 'processors' field");
        assert!(entry["processors"].is_array(), "'processors' should be an array");
    }
}

#[test]
fn tools_check_succeeds() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // First create the lock file so check has something to verify against
    let lock_output = run_rsconstruct_with_env(project_path, &["tools", "lock"], &[("NO_COLOR", "1")]);
    assert!(lock_output.status.success(), "tools lock failed: {}", String::from_utf8_lossy(&lock_output.stderr));

    // Now check should succeed since versions match the just-created lock file
    let output = run_rsconstruct_with_env(project_path, &["tools", "check"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "tools check failed: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn tools_stats_shows_summary() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["tools", "stats"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "tools stats failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Tool"), "Expected 'Tool' table header");
    assert!(stdout.contains("Runtime summary:"), "Expected 'Runtime summary:' section");
    assert!(stdout.contains("Total:"), "Expected 'Total:' summary line");
    assert!(stdout.contains("installed"), "Expected 'installed' count");
}

#[test]
fn tools_stats_json() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["--json", "tools", "stats"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "tools stats --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(&stdout).expect("Expected valid JSON");

    // Verify top-level structure
    assert!(parsed.get("tools").is_some(), "Expected 'tools' field");
    assert!(parsed.get("runtimes").is_some(), "Expected 'runtimes' field");
    assert!(parsed.get("summary").is_some(), "Expected 'summary' field");

    // Verify tools array entries
    let tools = parsed["tools"].as_array().expect("'tools' should be an array");
    assert!(!tools.is_empty(), "tools array should not be empty");
    for tool in tools {
        assert!(tool.get("name").is_some(), "Tool entry should have 'name'");
        assert!(tool.get("installed").is_some(), "Tool entry should have 'installed'");
        assert!(tool.get("runtime").is_some(), "Tool entry should have 'runtime'");
        assert!(tool.get("processors").is_some(), "Tool entry should have 'processors'");
    }

    // Verify runtimes array entries
    let runtimes = parsed["runtimes"].as_array().expect("'runtimes' should be an array");
    for rt in runtimes {
        assert!(rt.get("runtime").is_some(), "Runtime entry should have 'runtime'");
        assert!(rt.get("total").is_some(), "Runtime entry should have 'total'");
        assert!(rt.get("installed").is_some(), "Runtime entry should have 'installed'");
        assert!(rt.get("missing").is_some(), "Runtime entry should have 'missing'");
    }

    // Verify summary
    let summary = &parsed["summary"];
    assert!(summary.get("total_tools").is_some(), "Summary should have 'total_tools'");
    assert!(summary.get("installed").is_some(), "Summary should have 'installed'");
    assert!(summary.get("missing").is_some(), "Summary should have 'missing'");

    // Verify consistency: total_tools == tools.len()
    let total_tools = summary["total_tools"].as_u64().unwrap();
    assert_eq!(total_tools as usize, tools.len(), "summary.total_tools should match tools array length");

    // Verify consistency: installed + missing == total_tools
    let installed = summary["installed"].as_u64().unwrap();
    let missing = summary["missing"].as_u64().unwrap();
    assert_eq!(installed + missing, total_tools, "installed + missing should equal total_tools");
}
