use std::fs;
use crate::common::{run_rsconstruct, run_rsconstruct_with_env};
use tempfile::TempDir;

/// Set up a test project with the jinja2 processor enabled
fn setup_jinja2_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::create_dir_all(temp_dir.path().join("templates.jinja2")).expect("Failed to create templates.jinja2 dir");
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.jinja2]\nsrc_dirs = [\"templates.jinja2\"]\n"
    ).expect("Failed to write rsconstruct.toml");
    temp_dir
}

#[test]
fn jinja2_basic_render() {
    let temp_dir = setup_jinja2_project();
    let project_path = temp_dir.path();

    // Create a simple jinja2 template
    fs::write(
        project_path.join("templates.jinja2/hello.txt.j2"),
        "Hello, {{ 'World' }}!\nCount: {{ 2 + 3 }}\n"
    ).expect("Failed to write jinja2 template");

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    let output_file = project_path.join("hello.txt");
    assert!(output_file.exists(), "Output file was not created");

    let content = fs::read_to_string(&output_file).expect("Failed to read output");
    assert!(content.contains("Hello, World!"));
    assert!(content.contains("Count: 5"));
}

#[test]
fn jinja2_subdirectory_output() {
    let temp_dir = setup_jinja2_project();
    let project_path = temp_dir.path();

    fs::create_dir_all(project_path.join("templates.jinja2/config")).unwrap();
    fs::write(
        project_path.join("templates.jinja2/config/app.conf.j2"),
        "[app]\nname = {{ 'TestApp' }}\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    let output_file = project_path.join("config/app.conf");
    assert!(output_file.exists(), "Output file config/app.conf was not created");

    let content = fs::read_to_string(&output_file).unwrap();
    assert!(content.contains("name = TestApp"));
}

#[test]
fn jinja2_multiple_templates() {
    let temp_dir = setup_jinja2_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("templates.jinja2/first.txt.j2"),
        "First: {{ 1 + 1 }}\n"
    ).unwrap();

    fs::write(
        project_path.join("templates.jinja2/second.conf.j2"),
        "[section]\nvalue = {{ 'hello' }}\n"
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
fn jinja2_incremental_build() {
    let temp_dir = setup_jinja2_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("templates.jinja2/simple.txt.j2"),
        "Value: {{ 42 }}\n"
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
    assert!(stdout2.contains("[jinja2] Skipping (unchanged):"));
}

#[test]
fn jinja2_clean() {
    let temp_dir = setup_jinja2_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("templates.jinja2/output.txt.j2"),
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
