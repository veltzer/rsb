use std::fs;
use tempfile::TempDir;
use crate::common::{setup_test_project, run_rsconstruct, run_rsconstruct_with_env};

#[test]
fn tera_to_file_translation() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a Python config file
    let config_content = r#"
project_name = "TestProject"
version = "1.2.3"
author = "Test Author"
debug_mode = True
features = ["logging", "caching", "metrics"]
max_connections = 100
"#;
    fs::write(
        project_path.join("config/test_config.py"),
        config_content
    ).expect("Failed to write config file");

    // Create a tera file
    let tera_content = r#"{% set cfg = load_python(path="config/test_config.py") %}
# Generated configuration for {{ cfg.project_name }}
# Version: {{ cfg.version }}
# Author: {{ cfg.author }}

[settings]
project = "{{ cfg.project_name }}"
version = "{{ cfg.version }}"
debug = {{ cfg.debug_mode }}
max_connections = {{ cfg.max_connections }}

[features]
{% for feature in cfg.features -%}
{{ feature }} = enabled
{% endfor %}

# Build information
{% if cfg.debug_mode -%}
build_type = "debug"
optimization = 0
{% else -%}
build_type = "release"
optimization = 3
{% endif -%}
"#;
    fs::write(
        project_path.join("tera.templates/app.config.tera"),
        tera_content
    ).expect("Failed to write tera file");

    // Run rsconstruct build
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Check that the output file was created
    let output_file = project_path.join("app.config");
    assert!(output_file.exists(), "Output file was not created");

    // Read and verify the generated file content
    let generated_content = fs::read_to_string(&output_file)
        .expect("Failed to read generated file");

    // Verify expected content in the generated file
    assert!(generated_content.contains("Generated configuration for TestProject"));
    assert!(generated_content.contains("Version: 1.2.3"));
    assert!(generated_content.contains("Author: Test Author"));
    assert!(generated_content.contains("debug = true"));
    assert!(generated_content.contains("max_connections = 100"));
    assert!(generated_content.contains("logging = enabled"));
    assert!(generated_content.contains("caching = enabled"));
    assert!(generated_content.contains("metrics = enabled"));
    assert!(generated_content.contains("build_type = \"debug\""));
    assert!(generated_content.contains("optimization = 0"));

    println!("Generated file content:\n{}", generated_content);
}

#[test]
fn incremental_build() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a simple config and tera
    fs::write(
        project_path.join("config/simple.py"),
        "name = 'SimpleTest'\ncount = 42"
    ).expect("Failed to write config");

    fs::write(
        project_path.join("tera.templates/simple.txt.tera"),
        "{% set c = load_python(path='config/simple.py') %}Name: {{ c.name }}, Count: {{ c.count }}"
    ).expect("Failed to write tera");

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"));

    // Second build (should skip unchanged tera - use verbose to see skip message)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[tera] Skipping (unchanged):"));

    // Verify cache directory exists
    assert!(project_path.join(".rsconstruct/db.redb").exists());
}

#[test]
fn multiple_templates() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create shared config
    let config = "shared_name = 'MultiTest'\nshared_value = 123";
    fs::write(project_path.join("config/shared.py"), config).unwrap();

    // Create multiple teras
    fs::write(
        project_path.join("tera.templates/first.txt.tera"),
        "{% set c = load_python(path='config/shared.py') %}First: {{ c.shared_name }}"
    ).unwrap();

    fs::write(
        project_path.join("tera.templates/second.conf.tera"),
        "{% set c = load_python(path='config/shared.py') %}[config]\nname={{ c.shared_name }}\nvalue={{ c.shared_value }}"
    ).unwrap();

    fs::write(
        project_path.join("tera.templates/third.json.tera"),
        r#"{% set c = load_python(path='config/shared.py') %}{"name": "{{ c.shared_name }}", "value": {{ c.shared_value }}}"#
    ).unwrap();

    // Build
    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());

    // Check all files were created
    assert!(project_path.join("first.txt").exists());
    assert!(project_path.join("second.conf").exists());
    assert!(project_path.join("third.json").exists());

    // Verify content
    let first = fs::read_to_string(project_path.join("first.txt")).unwrap();
    assert_eq!(first.trim(), "First: MultiTest");

    let third = fs::read_to_string(project_path.join("third.json")).unwrap();
    assert!(third.contains(r#""name": "MultiTest""#));
    assert!(third.contains(r#""value": 123"#));
}

#[test]
fn dep_inputs_triggers_rebuild() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a Python config file used as extra_input
    fs::write(
        project_path.join("config/settings.py"),
        "name = 'Original'"
    ).unwrap();

    // Create a tera
    fs::write(
        project_path.join("tera.templates/output.txt.tera"),
        "{% set c = load_python(path='config/settings.py') %}Name: {{ c.name }}"
    ).unwrap();

    // Configure tera processor with dep_inputs pointing to the config file
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\ndep_inputs = [\"config/settings.py\"]\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "First build should process: {}", stdout1);

    // Second build — should skip (nothing changed)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[tera] Skipping (unchanged):"), "Second build should skip: {}", stdout2);

    // Wait so mtime differs
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Modify the extra input file (but not the tera itself)
    fs::write(
        project_path.join("config/settings.py"),
        "name = 'Modified'"
    ).unwrap();

    // Third build — should rebuild because extra input changed
    let output3 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output3.status.success(),
        "Third build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output3.stdout),
        String::from_utf8_lossy(&output3.stderr));
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("Processing:"),
        "Build after extra_input change should reprocess, not skip: {}", stdout3);

    // Verify the output reflects the new config
    let content = fs::read_to_string(project_path.join("output.txt")).unwrap();
    assert!(content.contains("Modified"),
        "Output should reflect the modified config: {}", content);
}

#[test]
fn dep_inputs_nonexistent_file_fails() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a tera
    fs::write(
        project_path.join("config/simple.py"),
        "val = 'test'"
    ).unwrap();

    fs::write(
        project_path.join("tera.templates/simple.txt.tera"),
        "{% set c = load_python(path='config/simple.py') %}{{ c.val }}"
    ).unwrap();

    // Configure with a nonexistent extra_input — should cause an error
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\ndep_inputs = [\"nonexistent_file.txt\"]\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(),
        "Build should fail with nonexistent extra_input: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("dep_inputs file not found") || stderr.contains("nonexistent_file.txt"),
        "Error should mention missing dep_inputs file: {}", stderr);
}

#[test]
fn subdirectory_output() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a template in a subdirectory
    fs::create_dir_all(project_path.join("tera.templates/sub")).unwrap();
    fs::write(
        project_path.join("tera.templates/sub/output.txt.tera"),
        "Hello from subdirectory"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "rsconstruct build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Output should be at sub/output.txt (tera.templates/ prefix stripped)
    let output_file = project_path.join("sub/output.txt");
    assert!(output_file.exists(), "Output file sub/output.txt was not created");

    let content = fs::read_to_string(&output_file).unwrap();
    assert_eq!(content, "Hello from subdirectory");
}

// ----- glob() and shell_output(depends_on=...) ---------------------------------------------
//
// These tests exercise the design from docs/src/internal/glob-deps.md:
// - glob(pattern=...) is a first-class directory query that participates in
//   dependency tracking (file content + path-set fingerprint).
// - shell_output() requires depends_on=[...] explicitly; missing the argument
//   is an error.
//
// Each test sets up a project with [processor.tera] + [analyzer.tera] declared,
// because the analyzer is what wires globs into the cache key. The harness's
// setup_test_project() only adds the processor, so we build configs explicitly.

/// Build a self-contained tera project with the given template body.
/// Returns a TempDir whose path contains rsconstruct.toml + the template +
/// any extra files the caller writes afterwards.
fn setup_glob_project(template_body: &str) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();
    fs::create_dir_all(project_path.join("tera.templates")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n[analyzer.tera]\n",
    ).unwrap();
    fs::write(
        project_path.join("tera.templates/report.txt.tera"),
        template_body,
    ).unwrap();
    temp_dir
}

#[test]
fn glob_counts_matching_files() {
    let project = setup_glob_project("Total: {{ glob(pattern=\"data/**/*.md\") | length }}\n");
    let p = project.path();

    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();
    fs::write(p.join("data/c.md"), "c").unwrap();
    // A non-matching file (wrong extension) — should not be counted.
    fs::write(p.join("data/ignore.txt"), "x").unwrap();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "build failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    let report = fs::read_to_string(p.join("report.txt")).unwrap();
    assert_eq!(report.trim(), "Total: 3", "Got: {}", report);
}

#[test]
fn glob_invalidates_when_file_added() {
    let project = setup_glob_project("Total: {{ glob(pattern=\"data/**/*.md\") | length }}\n");
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();

    // First build: 2 files.
    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "first build failed: {}", String::from_utf8_lossy(&out1.stderr));
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 2");

    // Add a third file.
    fs::write(p.join("data/c.md"), "c").unwrap();

    // Second build: should rebuild (not skip) and reflect 3 files.
    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success(), "second build failed: {}", String::from_utf8_lossy(&out2.stderr));
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build should NOT have skipped after adding a glob-matched file. stdout={}",
        stdout2,
    );
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 3");
}

#[test]
fn glob_invalidates_when_file_removed() {
    let project = setup_glob_project("Total: {{ glob(pattern=\"data/**/*.md\") | length }}\n");
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();
    fs::write(p.join("data/c.md"), "c").unwrap();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success());
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 3");

    fs::remove_file(p.join("data/c.md")).unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build should NOT have skipped after removing a glob-matched file. stdout={}",
        stdout2,
    );
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 2");
}

#[test]
fn glob_invalidates_when_file_renamed() {
    // Renaming a file with identical content otherwise produces the same
    // content-addressed cache key. The path-set fingerprint mixed into
    // config_hash is what makes this case work.
    let project = setup_glob_project(
        "Sorted: {{ glob(pattern=\"data/**/*.md\") | join(sep=\",\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/old_name.md"), "same-content").unwrap();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success());
    let report1 = fs::read_to_string(p.join("report.txt")).unwrap();
    assert!(report1.contains("old_name.md"), "Got: {}", report1);

    // Rename the file (content unchanged).
    fs::rename(p.join("data/old_name.md"), p.join("data/new_name.md")).unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build should NOT have skipped after rename. stdout={}",
        stdout2,
    );
    let report2 = fs::read_to_string(p.join("report.txt")).unwrap();
    assert!(report2.contains("new_name.md"), "Got: {}", report2);
    assert!(!report2.contains("old_name.md"), "Got: {}", report2);
}

#[test]
fn shell_output_without_depends_on_is_rejected() {
    let project = setup_glob_project(
        "Count: {{ shell_output(command=\"ls data | wc -l\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should have failed for shell_output without depends_on. stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("depends_on"),
        "Error message should mention depends_on. Got: {}",
        combined,
    );
}

#[test]
fn shell_output_with_depends_on_succeeds() {
    let project = setup_glob_project(
        "Count: {{ shell_output(command=\"ls data | wc -l\", depends_on=[\"data/**/*.md\"]) }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build with depends_on should have succeeded. stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let report = fs::read_to_string(p.join("report.txt")).unwrap();
    assert_eq!(report.trim(), "Count: 2", "Got: {}", report);
}

#[test]
fn shell_output_invalidates_on_matching_file_change() {
    // shell_output's depends_on is the only way the analyzer learns which
    // files might affect the command's output. Modifying a depends_on-matched
    // file should bust the cache.
    let project = setup_glob_project(
        "Lines: {{ shell_output(command=\"cat data/*.md | wc -l\", depends_on=[\"data/**/*.md\"]) }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "one\ntwo\n").unwrap();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "first build failed: {}", String::from_utf8_lossy(&out1.stderr));
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Lines: 2");

    // Edit the matching file: 2 lines → 3 lines.
    fs::write(p.join("data/a.md"), "one\ntwo\nthree\n").unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success(), "second build failed: {}", String::from_utf8_lossy(&out2.stderr));
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build should NOT have skipped after editing a depends_on-matched file. stdout={}",
        stdout2,
    );
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Lines: 3");
}

#[test]
fn shell_output_invalidates_when_command_edited() {
    // The literal command string is part of the config_hash, so editing the
    // command should bust the cache even when no depends_on file changed.
    let project = setup_glob_project(
        "Out: {{ shell_output(command=\"echo first\", depends_on=[]) }}\n",
    );
    let p = project.path();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "first build failed: {}", String::from_utf8_lossy(&out1.stderr));
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Out: first");

    // Edit only the command, not the dependency list.
    fs::write(
        p.join("tera.templates/report.txt.tera"),
        "Out: {{ shell_output(command=\"echo second\", depends_on=[]) }}\n",
    ).unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success(), "second build failed: {}", String::from_utf8_lossy(&out2.stderr));
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Out: second");
}

// ----- git_count_files() ---------------------------------------------------
//
// git_count_files(pattern="...") counts only git-tracked files matching the
// pathspec. The analyzer mirrors that semantics by shelling out to
// `git ls-files -- <pattern>` so the path-set fingerprint mixed into the
// cache key matches what the renderer will actually count.

/// Initialize a git repo in `project_path` and stage+commit any tracked files
/// the caller has already written. Configures user.email/user.name so the
/// commit succeeds in CI environments.
fn git_init_and_commit(project_path: &std::path::Path) {
    use std::process::Command;
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(project_path)
            .args(args)
            .output()
            .expect("git invocation failed");
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);
}

#[test]
fn git_count_files_counts_only_tracked() {
    let project = setup_glob_project(
        "Total: {{ git_count_files(pattern=\"data/*.md\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();
    git_init_and_commit(p);

    // An untracked file added after the commit must NOT be counted.
    fs::write(p.join("data/untracked.md"), "x").unwrap();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "build failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    let report = fs::read_to_string(p.join("report.txt")).unwrap();
    assert_eq!(report.trim(), "Total: 2", "Got: {}", report);
}

#[test]
fn git_count_files_invalidates_when_file_committed() {
    let project = setup_glob_project(
        "Total: {{ git_count_files(pattern=\"data/*.md\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();
    git_init_and_commit(p);

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "first build failed: {}", String::from_utf8_lossy(&out1.stderr));
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 2");

    // Add a new file and commit it — `git ls-files` will now return 3.
    fs::write(p.join("data/c.md"), "c").unwrap();
    use std::process::Command;
    Command::new("git").current_dir(p).args(["add", "data/c.md"]).output().unwrap();
    Command::new("git").current_dir(p).args(["commit", "-q", "-m", "add c"]).output().unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success(), "second build failed: {}", String::from_utf8_lossy(&out2.stderr));
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build should NOT have skipped after committing a new tracked file. stdout={}",
        stdout2,
    );
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 3");
}

#[test]
fn git_count_files_skips_when_only_untracked_added() {
    // Adding an untracked file does not change the git ls-files output, so
    // the build should skip on the second run. This is the inverse of
    // glob_invalidates_when_file_added — different semantics, different cache.
    let project = setup_glob_project(
        "Total: {{ git_count_files(pattern=\"data/*.md\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();
    git_init_and_commit(p);

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success());
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 2");

    // Add an untracked file — should NOT trigger a rebuild.
    fs::write(p.join("data/untracked.md"), "x").unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build SHOULD have skipped — untracked files don't affect git_count_files. stdout={}",
        stdout2,
    );
}

#[test]
fn glob_skips_when_matched_file_content_changes() {
    // glob() consumes names, not content. Editing a file that happens to
    // match the glob must NOT invalidate the product — only adding,
    // removing, or renaming a matched file does.
    let project = setup_glob_project("Total: {{ glob(pattern=\"data/**/*.md\") | length }}\n");
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "original").unwrap();
    fs::write(p.join("data/b.md"), "original").unwrap();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success());

    // Edit the content of a matching file (path set unchanged).
    fs::write(p.join("data/a.md"), "edited").unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build SHOULD skip — glob tracks names, not content. stdout={}",
        stdout2,
    );
}

#[test]
fn git_count_files_skips_when_tracked_file_content_changes() {
    // git_count_files() consumes the count of tracked files, not their
    // content. Editing a tracked matching file (without changing the
    // tracked path set) must NOT invalidate the product.
    let project = setup_glob_project(
        "Total: {{ git_count_files(pattern=\"data/*.md\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "original").unwrap();
    fs::write(p.join("data/b.md"), "original").unwrap();
    git_init_and_commit(p);

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success());
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 2");

    // Edit a tracked file's content without changing the tracked set.
    fs::write(p.join("data/a.md"), "edited").unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build SHOULD skip — git_count_files tracks count, not content. stdout={}",
        stdout2,
    );
}

// ----- grep_count() --------------------------------------------------------
//
// grep_count consumes file *content*, so the analyzer must add matched files
// as inputs (mtime/checksum-tracked). This is the key difference from glob
// and git_count_files.

#[test]
fn grep_count_counts_matching_lines() {
    let project = setup_glob_project(
        "TODOs: {{ grep_count(pattern=\"^TODO\", glob=\"src/**/*.txt\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(p.join("src/a.txt"), "TODO: x\nfoo\nTODO: y\n").unwrap();
    fs::write(p.join("src/b.txt"), "no todos here\nTODO: z\n").unwrap();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "build failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
    let report = fs::read_to_string(p.join("report.txt")).unwrap();
    assert_eq!(report.trim(), "TODOs: 3", "Got: {}", report);
}

#[test]
fn grep_count_invalidates_when_matched_file_content_changes() {
    // Editing a file inside the glob must trigger a rebuild — that's the
    // whole point of grep_count vs glob/git_count_files.
    let project = setup_glob_project(
        "TODOs: {{ grep_count(pattern=\"^TODO\", glob=\"src/**/*.txt\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(p.join("src/a.txt"), "TODO: x\n").unwrap();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "first build failed: {}", String::from_utf8_lossy(&out1.stderr));
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "TODOs: 1");

    // Add another TODO line to the same file. Content changed; path set unchanged.
    fs::write(p.join("src/a.txt"), "TODO: x\nTODO: y\n").unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success(), "second build failed: {}", String::from_utf8_lossy(&out2.stderr));
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build should NOT skip — grep_count tracks content. stdout={}",
        stdout2,
    );
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "TODOs: 2");
}

#[test]
fn grep_count_invalidates_when_regex_changes() {
    let project = setup_glob_project(
        "Hits: {{ grep_count(pattern=\"^TODO\", glob=\"src/**/*.txt\") }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(p.join("src/a.txt"), "TODO: x\nFIXME: y\n").unwrap();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success());
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Hits: 1");

    // Change only the regex — same files, same content.
    fs::write(
        p.join("tera.templates/report.txt.tera"),
        "Hits: {{ grep_count(pattern=\"^FIXME\", glob=\"src/**/*.txt\") }}\n",
    ).unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success());
    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Hits: 1");
}

#[test]
fn glob_in_included_snippet_is_tracked() {
    // The function call lives in an included snippet, not the top-level
    // template. The analyzer must recurse into includes so the snippet's
    // glob participates in the parent product's cache key.
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();
    fs::create_dir_all(p.join("tera.templates")).unwrap();
    fs::create_dir_all(p.join("tera.snippets")).unwrap();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(
        p.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n[analyzer.tera]\n",
    ).unwrap();
    fs::write(
        p.join("tera.templates/report.txt.tera"),
        "Header\n{% include \"tera.snippets/main.md.tera\" %}\nFooter\n",
    ).unwrap();
    fs::write(
        p.join("tera.snippets/main.md.tera"),
        "Total: {{ glob(pattern=\"data/**/*.md\") | length }}\n",
    ).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();

    let out1 = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "first build failed: {}", String::from_utf8_lossy(&out1.stderr));
    let report = fs::read_to_string(p.join("report.txt")).unwrap();
    assert!(report.contains("Total: 2"), "report should reflect 2 files: {}", report);

    // Add a third matching file. The parent template body did not change,
    // and the snippet body did not change, so without recursive analysis
    // the build would skip incorrectly.
    fs::write(p.join("data/c.md"), "c").unwrap();

    let out2 = run_rsconstruct_with_env(p, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success(), "second build failed: {}", String::from_utf8_lossy(&out2.stderr));
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("[tera] Skipping (unchanged):"),
        "Second build should NOT skip — glob in included snippet matched a new file. stdout={}",
        stdout2,
    );
    let report2 = fs::read_to_string(p.join("report.txt")).unwrap();
    assert!(report2.contains("Total: 3"), "report should reflect 3 files after add: {}", report2);
}

#[test]
fn glob_no_matches_returns_empty_list() {
    let project = setup_glob_project(
        "Total: {{ glob(pattern=\"nonexistent/**/*.md\") | length }}\n",
    );
    let p = project.path();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "build with empty glob should succeed. stderr={}",
        String::from_utf8_lossy(&output.stderr));

    assert_eq!(fs::read_to_string(p.join("report.txt")).unwrap().trim(), "Total: 0");
}

/// `analyzers show files <path> --hash-pieces` must surface the structured
/// non-content state the analyzer mixes into the cache key. For a tera
/// template that calls `glob(pattern=...)`, the output should include the
/// pattern itself and the resolved file list so the user can see exactly
/// what's being tracked.
#[test]
fn show_files_hash_pieces_surfaces_glob_state() {
    let project = setup_glob_project(
        "Total: {{ glob(pattern=\"data/*.md\") | length }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();

    // Prime the deps cache so `analyzers show files` has an entry to read.
    let build = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let out = run_rsconstruct_with_env(
        p,
        &["analyzers", "show", "files", "tera.templates/report.txt.tera", "--hash-pieces"],
        &[("NO_COLOR", "1")],
    );
    assert!(out.status.success(),
        "show files --hash-pieces failed: {}",
        String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("hash pieces:"),
        "expected 'hash pieces:' header in output: {}", stdout);
    assert!(stdout.contains("glob") && stdout.contains("data/*.md"),
        "expected glob pattern in hash pieces: {}", stdout);
    assert!(stdout.contains("data/a.md") && stdout.contains("data/b.md"),
        "expected resolved file list in hash pieces: {}", stdout);
}

/// Same as the text test but verifies the JSON shape: `hash_pieces` is a
/// list of `kind:body` strings, recomputed live (so the field is present
/// only when --hash-pieces is passed).
#[test]
fn show_files_hash_pieces_json_shape() {
    let project = setup_glob_project(
        "Total: {{ glob(pattern=\"data/*.md\") | length }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();

    let build = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success());

    let out = run_rsconstruct_with_env(
        p,
        &["--json", "analyzers", "show", "files",
          "tera.templates/report.txt.tera", "--hash-pieces"],
        &[("NO_COLOR", "1")],
    );
    assert!(out.status.success(),
        "json show files --hash-pieces failed: {}",
        String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {}\n---\n{}", e, stdout));
    let arr = parsed.as_array().expect("top-level JSON must be an array");
    assert_eq!(arr.len(), 1, "expected one entry, got: {}", stdout);
    let entry = &arr[0];
    let pieces = entry.get("hash_pieces")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("hash_pieces field missing or not array: {}", stdout));
    let joined = pieces.iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("glob:data/*.md"),
        "expected 'glob:data/*.md' piece, got: {}", joined);
    assert!(joined.contains("data/a.md"),
        "expected resolved file list to mention data/a.md, got: {}", joined);
}

/// When `--hash-pieces` is omitted, the JSON shape must NOT include the
/// `hash_pieces` field — keeps the existing JSON contract stable for any
/// caller that doesn't opt in.
#[test]
fn show_files_without_hash_pieces_omits_field() {
    let project = setup_glob_project(
        "Total: {{ glob(pattern=\"data/*.md\") | length }}\n",
    );
    let p = project.path();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();

    let build = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success());

    let out = run_rsconstruct_with_env(
        p,
        &["--json", "analyzers", "show", "files", "tera.templates/report.txt.tera"],
        &[("NO_COLOR", "1")],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {}\n---\n{}", e, stdout));
    let entry = &parsed.as_array().expect("array")[0];
    assert!(entry.get("hash_pieces").is_none(),
        "hash_pieces field must be absent when flag is omitted: {}", stdout);
}

#[test]
fn copyright_years_falls_back_when_no_commits() {
    // A fresh repo (or a non-git directory) must not crash copyright_years().
    // We expect the function to fall back to "{current_year}".
    let project = setup_glob_project("© {{ copyright_years() }}\n");
    let p = project.path();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "build failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let report = fs::read_to_string(p.join("report.txt")).unwrap();
    let current_year = chrono::Local::now().format("%Y").to_string();
    assert_eq!(report.trim(), format!("© {}", current_year), "Got: {}", report);
}

#[test]
fn copyright_years_uses_first_commit_year() {
    // With a real git repo and a commit, copyright_years() should produce a
    // range from the first commit's year to the current year.
    let project = setup_glob_project("© {{ copyright_years() }}\n");
    let p = project.path();
    git_init_and_commit(p);

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "build failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let report = fs::read_to_string(p.join("report.txt")).unwrap();
    let current_year: i32 = chrono::Local::now().format("%Y").to_string().parse().unwrap();
    // Whatever the commit year was, the output must end with the current year
    // and contain it.
    assert!(
        report.trim().ends_with(&current_year.to_string()),
        "Got: {}",
        report
    );
}

#[test]
fn analyzer_tracks_load_python_without_dep_inputs() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Analyzers run only when declared, and the whole point of these tests is
    // the analyzer's own dependency discovery — no dep_inputs anywhere.
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[analyzer.tera]\n\n[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n"
    ).unwrap();

    fs::write(
        project_path.join("config/tracked.py"),
        "name = 'Original'"
    ).unwrap();

    // No dep_inputs anywhere: the analyzer alone must discover that the
    // template reads config/tracked.py through load_python(path="...").
    fs::write(
        project_path.join("tera.templates/tracked.txt.tera"),
        "{% set c = load_python(path='config/tracked.py') %}Name: {{ c.name }}"
    ).unwrap();

    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));

    // Wait so mtime differs
    std::thread::sleep(std::time::Duration::from_millis(100));

    fs::write(
        project_path.join("config/tracked.py"),
        "name = 'Modified'"
    ).unwrap();

    let output2 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let content = fs::read_to_string(project_path.join("tracked.txt")).unwrap();
    assert!(content.contains("Modified"),
        "Editing a load_python config must rebuild the product: {}", content);
}

#[test]
fn analyzer_tracks_version_str_default_path() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Analyzers run only when declared, and the whole point of these tests is
    // the analyzer's own dependency discovery — no dep_inputs anywhere.
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[analyzer.tera]\n\n[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n"
    ).unwrap();

    // version_str() with no arguments reads config/version.py; the analyzer
    // must track that default even though no path appears in the template.
    fs::write(
        project_path.join("config/version.py"),
        "tup = (1, 2, 3)"
    ).unwrap();

    fs::write(
        project_path.join("tera.templates/version.txt.tera"),
        "version: {{ version_str() }}"
    ).unwrap();

    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    let content = fs::read_to_string(project_path.join("version.txt")).unwrap();
    assert!(content.contains("1.2.3"), "Got: {}", content);

    // Wait so mtime differs
    std::thread::sleep(std::time::Duration::from_millis(100));

    fs::write(
        project_path.join("config/version.py"),
        "tup = (1, 2, 4)"
    ).unwrap();

    let output2 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let content = fs::read_to_string(project_path.join("version.txt")).unwrap();
    assert!(content.contains("1.2.4"),
        "Bumping config/version.py must rebuild a version_str() product: {}", content);
}

#[test]
fn analyzer_tracks_version_str_explicit_lua_path() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Analyzers run only when declared, and the whole point of these tests is
    // the analyzer's own dependency discovery — no dep_inputs anywhere.
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[analyzer.tera]\n\n[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n"
    ).unwrap();

    fs::write(
        project_path.join("config/version.lua"),
        "tup = { 2, 0, 0 }"
    ).unwrap();

    fs::write(
        project_path.join("tera.templates/vlua.txt.tera"),
        r#"version: {{ version_str(path="config/version.lua") }}"#
    ).unwrap();

    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    let content = fs::read_to_string(project_path.join("vlua.txt")).unwrap();
    assert!(content.contains("2.0.0"), "Got: {}", content);

    // Wait so mtime differs
    std::thread::sleep(std::time::Duration::from_millis(100));

    fs::write(
        project_path.join("config/version.lua"),
        "tup = { 2, 0, 1 }"
    ).unwrap();

    let output2 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let content = fs::read_to_string(project_path.join("vlua.txt")).unwrap();
    assert!(content.contains("2.0.1"),
        "Bumping the version_str(path=...) file must rebuild the product: {}", content);
}
