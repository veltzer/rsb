use std::fs;
use crate::common::{run_rsconstruct, run_rsconstruct_with_env};
use tempfile::TempDir;

fn setup_rust_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::create_dir_all(temp_dir.path().join("src")).expect("Failed to create src dir");
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.rust_single_file]\nsrc_dirs = [\"src\"]\n"
    ).expect("Failed to write rsconstruct.toml");
    temp_dir
}

#[test]
fn rust_single_file_basic_compile() {
    let temp_dir = setup_rust_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("src/hello.rs"),
        "fn main() { println!(\"Hello\"); }\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    let output_file = project_path.join("out/rust_single_file/hello.elf");
    assert!(output_file.exists(), "Output executable was not created");
}

#[test]
fn rust_single_file_incremental_build() {
    let temp_dir = setup_rust_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("src/hello.rs"),
        "fn main() {}\n"
    ).unwrap();

    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "Expected 'Processing:' in first build output: {}", stdout1);

    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[rust_single_file] Skipping (unchanged):"), "Expected skip message in second build: {}", stdout2);
}

#[test]
fn rust_single_file_compile_error_fails() {
    let temp_dir = setup_rust_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("src/bad.rs"),
        "fn main() { let x: i32 = \"not a number\"; }\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build should fail with compile error");
}

#[test]
fn rust_single_file_clean() {
    let temp_dir = setup_rust_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("src/hello.rs"),
        "fn main() {}\n"
    ).unwrap();

    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());
    assert!(project_path.join("out/rust_single_file/hello.elf").exists());

    let output = run_rsconstruct(project_path, &["clean", "outputs"]);
    assert!(output.status.success());
    assert!(!project_path.join("out/rust_single_file/hello.elf").exists());
}

#[test]
fn rust_single_file_multiple_files() {
    let temp_dir = setup_rust_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("src/one.rs"),
        "fn main() { println!(\"one\"); }\n"
    ).unwrap();

    fs::write(
        project_path.join("src/two.rs"),
        "fn main() { println!(\"two\"); }\n"
    ).unwrap();

    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());

    assert!(project_path.join("out/rust_single_file/one.elf").exists());
    assert!(project_path.join("out/rust_single_file/two.elf").exists());
}
