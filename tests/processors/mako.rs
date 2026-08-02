use std::fs;
use crate::common::{run_rsconstruct, run_rsconstruct_with_env};
use tempfile::TempDir;

/// Set up a test project with the mako processor enabled
fn setup_mako_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::create_dir_all(temp_dir.path().join("templates.mako")).expect("Failed to create templates.mako dir");
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.mako]\nsrc_dirs = [\"templates.mako\"]\n"
    ).expect("Failed to write rsconstruct.toml");
    temp_dir
}

#[test]
fn mako_basic_render() {
    let temp_dir = setup_mako_project();
    let project_path = temp_dir.path();

    // Create a simple mako template
    fs::write(
        project_path.join("templates.mako/hello.txt.mako"),
        "Hello, ${'World'}!\nCount: ${2 + 3}\n"
    ).expect("Failed to write mako template");

    // Run rsconstruct build
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Check that the output file was created
    let output_file = project_path.join("hello.txt");
    assert!(output_file.exists(), "Output file was not created");

    let content = fs::read_to_string(&output_file).expect("Failed to read output");
    assert!(content.contains("Hello, World!"));
    assert!(content.contains("Count: 5"));
}

#[test]
fn mako_subdirectory_output() {
    let temp_dir = setup_mako_project();
    let project_path = temp_dir.path();

    // Create a template in a subdirectory
    fs::create_dir_all(project_path.join("templates.mako/config")).unwrap();
    fs::write(
        project_path.join("templates.mako/config/app.conf.mako"),
        "[app]\nname = ${'TestApp'}\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Output should be at config/app.conf (templates.mako/ prefix stripped)
    let output_file = project_path.join("config/app.conf");
    assert!(output_file.exists(), "Output file config/app.conf was not created");

    let content = fs::read_to_string(&output_file).unwrap();
    assert!(content.contains("name = TestApp"));
}

#[test]
fn mako_multiple_templates() {
    let temp_dir = setup_mako_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("templates.mako/first.txt.mako"),
        "First: ${1 + 1}\n"
    ).unwrap();

    fs::write(
        project_path.join("templates.mako/second.conf.mako"),
        "[section]\nvalue = ${'hello'}\n"
    ).unwrap();

    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());

    assert!(project_path.join("first.txt").exists());
    assert!(project_path.join("second.conf").exists());

    let first = fs::read_to_string(project_path.join("first.txt")).unwrap();
    assert!(first.contains("First: 2"));

    let second = fs::read_to_string(project_path.join("second.conf")).unwrap();
    assert!(second.contains("value = hello"));
}

#[test]
fn mako_incremental_build() {
    let temp_dir = setup_mako_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("templates.mako/simple.txt.mako"),
        "Value: ${42}\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"));

    // Second build (should skip unchanged)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[mako] Skipping (unchanged):"));
}

#[test]
fn mako_clean() {
    let temp_dir = setup_mako_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("templates.mako/output.txt.mako"),
        "content\n"
    ).unwrap();

    // Build
    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());
    assert!(project_path.join("output.txt").exists());

    // Clean
    let output = run_rsconstruct(project_path, &["clean", "outputs"]);
    assert!(output.status.success());
    assert!(!project_path.join("output.txt").exists(), "Output should be removed after clean");
}
