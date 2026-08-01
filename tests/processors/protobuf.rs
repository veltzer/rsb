use std::fs;
use crate::common::{run_rsconstruct, run_rsconstruct_with_env, require_tool};
use tempfile::TempDir;

fn setup_protobuf_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::create_dir_all(temp_dir.path().join("proto")).expect("Failed to create proto dir");
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.protobuf]\n"
    ).expect("Failed to write rsconstruct.toml");
    temp_dir
}

#[test]
fn protobuf_basic_compile() {
    require_tool("protoc");

    let temp_dir = setup_protobuf_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("proto/hello.proto"),
        r#"syntax = "proto3";
message Hello {
  string name = 1;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    let output_file = project_path.join("out/protobuf/hello.pb.cc");
    assert!(output_file.exists(), "Output protobuf C++ file was not created");
}

#[test]
fn protobuf_incremental_build() {
    require_tool("protoc");

    let temp_dir = setup_protobuf_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("proto/test.proto"),
        r#"syntax = "proto3";
message Test {
  int32 id = 1;
}
"#
    ).unwrap();

    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());

    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("Skipping (unchanged):"), "Expected skip message in incremental build: {}", stdout2);
}

#[test]
fn protobuf_clean() {
    require_tool("protoc");

    let temp_dir = setup_protobuf_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("proto/clean.proto"),
        r#"syntax = "proto3";
message Clean {
  string value = 1;
}
"#
    ).unwrap();

    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());
    assert!(project_path.join("out/protobuf/clean.pb.cc").exists());

    let output = run_rsconstruct(project_path, &["clean", "outputs"]);
    assert!(output.status.success());
    assert!(!project_path.join("out/protobuf/clean.pb.cc").exists());
}

#[test]
fn protobuf_no_files_discovered() {
    require_tool("protoc");
    let temp_dir = setup_protobuf_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 products"),
        "Should discover 0 products: {}",
        stdout
    );
}
