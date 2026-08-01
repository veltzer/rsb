use crate::common::{setup_test_project, run_rsconstruct_with_env};

#[test]
fn complete_bash_generates_output() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["complete", "bash"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "complete bash failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Expected completion output for bash");
    assert!(stdout.contains("rsconstruct"), "Expected 'rsconstruct' in bash completion script");

    // Iname injection must have run: the helper calls must be wired into the
    // script, and generation must be warning-free.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.is_empty(), "Expected no warnings from complete bash, got: {stderr}");
    for helper_call in [
        "compgen -W \"$(_rsconstruct_inames)\"",
        "compgen -W \"$(_rsconstruct_analyzer_inames)\"",
        "compgen -W \"$(_rsconstruct_fixer_inames)\"",
    ] {
        assert!(stdout.contains(helper_call), "Expected injected helper call in bash completion script: {helper_call}");
    }
}

#[test]
fn complete_zsh_generates_output() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["complete", "zsh"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "complete zsh failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Expected completion output for zsh");
}

#[test]
fn complete_fish_generates_output() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["complete", "fish"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "complete fish failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Expected completion output for fish");
}

#[test]
fn complete_from_config() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // setup_test_project doesn't set completions config, add it
    let config = "[processor.tera]\n\n[completions]\nshells = [\"bash\"]\n";
    std::fs::write(project_path.join("rsconstruct.toml"), config).expect("Failed to write rsconstruct.toml");

    // Running complete without arguments should use config
    let output = run_rsconstruct_with_env(project_path, &["complete"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "complete from config failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Expected completion output from config");
    assert!(stdout.contains("rsconstruct"), "Expected 'rsconstruct' in completion output");
}
