use std::fs;
use tempfile::TempDir;
use crate::common::run_rsconstruct_with_env;

test_checker!(black, tool: "black", processor: "black",
    files: [("hello.py", "def hello():\n    return \"world\"\n")]);

fn setup_black_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.black]\nsrc_dirs = [\".\"]\n",
    )
    .expect("Failed to write rsconstruct.toml");
    temp_dir
}

#[test]
fn black_badly_formatted_fails() {
    crate::common::require_tool("black");

    let temp_dir = setup_black_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("bad.py"),
        "def hello(  ):\n    x=1\n    return    \"world\"\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should fail with badly formatted Python file"
    );
}
