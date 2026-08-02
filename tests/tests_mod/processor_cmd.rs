use std::fs;
use tempfile::TempDir;
use crate::common::{setup_test_project, run_rsconstruct_with_env, run_rsconstruct_json_with_env};

#[test]
fn processors_list_shows_declared() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["processors", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "processors list failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tera"), "Expected tera processor in list");
}

#[test]
fn processors_files_shows_products() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Write a template so there's at least one product
    fs::write(
        project_path.join("config/test.py"),
        "value = 42"
    ).expect("Failed to write config");
    fs::write(
        project_path.join("tera.templates/output.txt.tera"),
        "{% set c = load_python(path='config/test.py') %}{{ c.value }}"
    ).expect("Failed to write template");

    let output = run_rsconstruct_with_env(project_path, &["processors", "files", "--headers"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "processors files failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[tera]"), "Expected [tera] header in output");
    assert!(stdout.contains("output.txt"), "Expected output file in listing");
}

#[test]
fn processors_files_no_files_message() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // No template files written, so no products
    let output = run_rsconstruct_with_env(project_path, &["processors", "files"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No files discovered") || stdout.contains("(no files)"),
        "Expected empty message, got: {}", stdout);
}

#[test]
fn processors_files_unknown_processor_fails() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["processors", "files", "nonexistent"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Expected failure for unknown processor");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown processor"), "Expected 'Unknown processor' error, got: {}", stderr);
}

#[test]
fn processors_list_shows_descriptions() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["processors", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "processors list failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    // processors list shows processors in a table
    assert!(stdout.contains("tera"), "Expected tera processor");
}

#[test]
fn processors_files_json_output() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Write a template so there's at least one product
    fs::write(
        project_path.join("config/test.py"),
        "value = 42"
    ).expect("Failed to write config");
    fs::write(
        project_path.join("tera.templates/output.txt.tera"),
        "{% set c = load_python(path='config/test.py') %}{{ c.value }}"
    ).expect("Failed to write template");

    let output = run_rsconstruct_with_env(project_path, &["--json", "processors", "files"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "processors files --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .expect("Expected valid JSON array");
    assert!(!entries.is_empty(), "Expected at least one entry");

    let entry = &entries[0];
    assert!(entry.get("processor").is_some(), "Entry should have 'processor' field");
    assert!(entry.get("processor_type").is_some(), "Entry should have 'processor_type' field");
    assert!(entry.get("inputs").is_some(), "Entry should have 'inputs' field");
    assert!(entry.get("outputs").is_some(), "Entry should have 'outputs' field");
    assert_eq!(entry["processor"], "tera");
    assert_eq!(entry["processor_type"], "generator");
}

#[test]
fn processors_files_json_empty() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // No template files written, so no products
    let output = run_rsconstruct_with_env(project_path, &["--json", "processors", "files"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .expect("Expected valid JSON array");
    assert!(entries.is_empty(), "Expected empty JSON array, got: {}", stdout);
}

#[test]
fn processors_list_works_without_config() {
    // Run from a temp dir with no rsconstruct.toml
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let output = run_rsconstruct_with_env(temp_dir.path(), &["processors", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "processors list should work without rsconstruct.toml: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tera"), "Expected tera processor in output");
    assert!(stdout.contains("ruff"), "Expected ruff processor in output");
    assert!(stdout.contains("shellcheck"), "Expected shellcheck processor in output");
}

#[test]
fn no_processor_section_means_no_products() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create tera template directory and file
    fs::create_dir_all(project_path.join("tera.templates")).unwrap();
    fs::write(project_path.join("tera.templates/quick.txt.tera"), "hello").unwrap();

    // No processor sections declared
    fs::write(
        project_path.join("rsconstruct.toml"),
        "\n"
    ).unwrap();

    // Build should produce zero products (no processors declared)
    let result = run_rsconstruct_json_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(result.exit_success, "Build should succeed");
    assert_eq!(result.total_products, 0, "Expected 0 products when no processor is declared");
}

#[test]
fn per_processor_enabled_true_is_default() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create tera template directory and file
    fs::create_dir_all(project_path.join("tera.templates")).unwrap();
    fs::write(project_path.join("tera.templates/quick.txt.tera"), "hello").unwrap();

    // Enable tera in the enabled list without setting per-processor enabled
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n"
    ).unwrap();

    // Build should produce one product (tera defaults to enabled = true)
    let result = run_rsconstruct_json_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(result.exit_success, "Build should succeed");
    assert_eq!(result.total_products, 1, "Expected 1 product when processor defaults to enabled");
}

/// `enabled = false` exists to keep a stanza while its tool is not installed:
/// the tool pre-flight must not fail the build for disabled instances.
#[test]
fn disabled_processor_skips_tool_preflight() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(project_path.join("src/a.txt"), "hello\n").unwrap();

    // src_extensions must match src/a.txt: the tool check runs against
    // processors that actually produced products, so a processor matching
    // nothing would never reach it.
    let config = |enabled: &str| format!(
        "[processor.script]\ncommand = \"definitely-not-a-real-tool-xyz\"\nsrc_dirs = [\"src\"]\nsrc_extensions = [\".txt\"]\nenabled = {enabled}\n"
    );

    // Control: enabled instance with a missing tool must fail pre-flight
    fs::write(project_path.join("rsconstruct.toml"), config("true")).unwrap();
    let result = run_rsconstruct_json_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!result.exit_success, "enabled instance with missing tool must fail");

    // Disabled instance must not trip the pre-flight
    fs::write(project_path.join("rsconstruct.toml"), config("false")).unwrap();
    let result = run_rsconstruct_json_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(result.exit_success, "disabled instance must not fail the tool pre-flight");
    assert_eq!(result.total_products, 0, "disabled instance must produce no products");
}

#[test]
fn processors_list_json() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["--json", "processors", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "processors list --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .expect("Expected valid JSON array");
    assert!(!entries.is_empty(), "Expected at least one entry");

    // Check that every entry has the expected fields
    for entry in &entries {
        assert!(entry.get("name").is_some(), "Entry should have 'name' field");
        assert!(entry.get("processor_type").is_some(), "Entry should have 'processor_type' field");
        assert!(entry.get("enabled").is_some(), "Entry should have 'enabled' field");
        assert!(entry.get("detected").is_some(), "Entry should have 'detected' field");
        assert!(entry.get("batch").is_some(), "Entry should have 'batch' field");
        assert!(entry.get("description").is_some(), "Entry should have 'description' field");
    }

    // list always shows all processors regardless of config
    let tera = entries.iter().find(|e| e["name"] == "tera").expect("Expected tera in list");
    assert!(tera.get("name").is_some());
}

#[test]
fn processors_list_all_json_without_config() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let output = run_rsconstruct_with_env(temp_dir.path(), &["--json", "processors", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "processors list --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .expect("Expected valid JSON array");
    assert!(!entries.is_empty(), "Expected at least one entry");

    // Check that every entry has the expected fields
    for entry in &entries {
        assert!(entry.get("name").is_some(), "Entry should have 'name' field");
        assert!(entry.get("processor_type").is_some(), "Entry should have 'processor_type' field");
        assert!(entry.get("batch").is_some(), "Entry should have 'batch' field");
        assert!(entry.get("description").is_some(), "Entry should have 'description' field");
    }

    let tera = entries.iter().find(|e| e["name"] == "tera").expect("Expected tera in list");
    assert!(tera.get("name").is_some());
}

#[test]
fn removing_processor_section_disables_it() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create tera template directory and file
    fs::create_dir_all(project_path.join("tera.templates")).unwrap();
    fs::write(
        project_path.join("tera.templates/output.txt.tera"),
        "hello",
    ).unwrap();

    // First build with tera declared — should produce 1 product
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n"
    ).unwrap();

    let result = run_rsconstruct_json_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(result.exit_success, "Build should succeed");
    assert_eq!(result.total_products, 1, "Expected 1 product with tera declared");

    // Remove tera section — should produce 0 products
    fs::write(
        project_path.join("rsconstruct.toml"),
        "\n"
    ).unwrap();

    let result = run_rsconstruct_json_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(result.exit_success, "Build should succeed");
    assert_eq!(result.total_products, 0, "Expected 0 products with no processor declared");
}
