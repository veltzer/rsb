use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

fn setup_sass_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::create_dir_all(temp_dir.path().join("sass")).expect("Failed to create sass dir");
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.sass]\nsrc_dirs = [\"sass\"]\n",
    )
    .expect("Failed to write rsconstruct.toml");
    temp_dir
}

#[test]
fn sass_basic_compile() {
    require_tool("sass");

    let temp_dir = setup_sass_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("sass/style.scss"),
        "$color: red;\nbody { color: $color; }\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "rsconstruct build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output_file = project_path.join("out/sass/style.css");
    assert!(output_file.exists(), "Output CSS file was not created");

    let content = fs::read_to_string(&output_file).unwrap();
    assert!(content.contains("red"), "CSS should contain the color value");
}

#[test]
fn sass_dry_run() {
    let temp_dir = setup_sass_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("sass/style.scss"),
        "body { margin: 0; }\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "--dry-run"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Dry run should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("BUILD") || stdout.contains("build"),
        "Should discover sass product: {}",
        stdout
    );
}
