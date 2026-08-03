use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, run_rsconstruct_json};

/// Helper: set up a project with the cc processor enabled and a cc.yaml file.
fn setup_cc_project(project_path: &std::path::Path, cc_yaml: &str) {
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc]\nsrc_dirs = [\".\"]\n",
    ).unwrap();
    fs::write(project_path.join("cc.yaml"), cc_yaml).unwrap();
}

#[test]
fn cc_no_project_discovered() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Enable cc processor but don't create cc.yaml
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let result = run_rsconstruct_json(project_path, &["build"]);
    assert!(result.exit_success, "Build should succeed with no cc.yaml");
    assert_eq!(result.total_products, 0, "No products should be discovered");
}

#[test]
fn cc_build_static_library() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path, r#"
libraries:
  - name: mymath
    lib_type: static
    sources: [src/math.c]
"#);

    fs::write(
        project_path.join("src/math.c"),
        "int add(int a, int b) { return a + b; }\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc/lib/libmymath.a").exists(),
        "Static library should exist");
}

#[test]
fn cc_build_shared_library() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path, r#"
libraries:
  - name: mymath
    lib_type: shared
    sources: [src/math.c]
"#);

    fs::write(
        project_path.join("src/math.c"),
        "int add(int a, int b) { return a + b; }\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc/lib/libmymath.so").exists(),
        "Shared library should exist");
}

#[test]
fn cc_build_program_with_library() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path, r#"
libraries:
  - name: mymath
    lib_type: static
    sources: [src/math.c]

programs:
  - name: main
    sources: [src/main.c]
    link: [mymath]
"#);

    fs::write(
        project_path.join("src/math.c"),
        "int add(int a, int b) { return a + b; }\n",
    ).unwrap();

    fs::write(
        project_path.join("src/main.c"),
        r#"extern int add(int, int);
int main() { return add(1, 2) - 3; }
"#,
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc/lib/libmymath.a").exists(),
        "Static library should exist");
    assert!(project_path.join("out/cc/bin/main").exists(),
        "Executable should exist");

    // Run the executable - should return 0 (add(1,2) - 3 == 0)
    let run_output = std::process::Command::new(project_path.join("out/cc/bin/main"))
        .output()
        .expect("Failed to run executable");
    assert!(run_output.status.success(),
        "Executable should return 0");
}

#[test]
fn cc_incremental_skip() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path, r#"
programs:
  - name: hello
    sources: [src/hello.c]
"#);

    fs::write(
        project_path.join("src/hello.c"),
        "int main() { return 0; }\n",
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "First build should process: {}", stdout1);

    // Second build - should skip
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[cc] Skipping (unchanged):"), "Second build should skip: {}", stdout2);
}

#[test]
fn cc_single_invocation_mode() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc]\nsingle_invocation = true\nsrc_dirs = [\".\"]\n",
    ).unwrap();
    fs::write(
        project_path.join("cc.yaml"),
        r#"
programs:
  - name: hello
    sources: [src/hello.c]
"#,
    ).unwrap();
    fs::write(
        project_path.join("src/hello.c"),
        "#include <stdio.h>\nint main() { printf(\"hello\\n\"); return 0; }\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with single_invocation failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc/bin/hello").exists(),
        "Executable should exist");
}

#[test]
fn cc_build_both_library_types() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path, r#"
libraries:
  - name: myutil
    lib_type: both
    sources: [src/util.c]
"#);

    fs::write(
        project_path.join("src/util.c"),
        "int helper() { return 42; }\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc/lib/libmyutil.a").exists(),
        "Static library should exist");
    assert!(project_path.join("out/cc/lib/libmyutil.so").exists(),
        "Shared library should exist");
}

#[test]
fn cc_clean_removes_output_dir() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path, r#"
programs:
  - name: hello
    sources: [src/hello.c]
"#);

    fs::write(
        project_path.join("src/hello.c"),
        "int main() { return 0; }\n",
    ).unwrap();

    // Build
    let build_output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(build_output.status.success());
    assert!(project_path.join("out/cc/bin/hello").exists());

    // Clean
    let clean_output = run_rsconstruct_with_env(project_path, &["clean", "outputs"], &[("NO_COLOR", "1")]);
    assert!(clean_output.status.success());
    assert!(!project_path.join("out/cc/bin/hello").exists(),
        "Output should be removed after clean");
}

/// Two sources with the same stem in different directories must compile to
/// distinct object files — a flat stem-derived name would silently drop one
/// translation unit from the link.
#[test]
fn cc_same_stem_sources_do_not_collide() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path, r#"
programs:
  - name: app
    sources: [src/main.c, src/a/util.c, src/b/util.c]
"#);

    fs::create_dir_all(project_path.join("src/a")).unwrap();
    fs::create_dir_all(project_path.join("src/b")).unwrap();
    fs::write(
        project_path.join("src/main.c"),
        "int from_a(void); int from_b(void);\nint main(void) { return from_a() + from_b(); }\n",
    ).unwrap();
    fs::write(
        project_path.join("src/a/util.c"),
        "int from_a(void) { return 1; }\n",
    ).unwrap();
    fs::write(
        project_path.join("src/b/util.c"),
        "int from_b(void) { return -1; }\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build failed (same-stem object collision?): stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
    assert!(project_path.join("out/cc/bin/app").exists(), "Program should link");
}

/// `[processor.cc]` flags are project-wide defaults for every cc.yaml.
///
/// Regression test: the config-level `cflags`/`cxxflags`/`ldflags`/
/// `include_dirs` fields used to be fully dead — declared, documented, part
/// of the config hash, and read by nothing. The source below only compiles
/// when the config-level define reaches the compiler.
#[test]
fn cc_config_flags_are_manifest_defaults() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc]\nsrc_dirs = [\".\"]\ncflags = [\"-DNEED_FLAG=0\"]\n",
    ).unwrap();
    fs::write(project_path.join("cc.yaml"), r#"
programs:
  - name: app
    sources: [src/main.c]
"#).unwrap();
    fs::write(
        project_path.join("src/main.c"),
        "int main(void) { return NEED_FLAG; }\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "config-level cflags must reach the compiler: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
}

/// A cc.yaml that sets a field explicitly overrides the config default —
/// including setting it to empty.
#[test]
fn cc_manifest_overrides_config_flags() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::create_dir_all(project_path.join("src")).unwrap();
    // The config default is an invalid flag: the build can only succeed if
    // the manifest's explicit empty cflags wins over the config value.
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc]\nsrc_dirs = [\".\"]\ncflags = [\"--definitely-not-a-flag\"]\n",
    ).unwrap();
    fs::write(project_path.join("cc.yaml"), r#"
cflags: []
programs:
  - name: app
    sources: [src/main.c]
"#).unwrap();
    fs::write(
        project_path.join("src/main.c"),
        "int main(void) { return 0; }\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "manifest cflags must override the config default: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
}

/// A compiler overridden per cc.yaml must be part of the processor's
/// declared tools.
///
/// Regression test: `required_tools()` used to return only the config-level
/// compilers, so a manifest override tripped the debug-build declared-tools
/// assertion ("executed undeclared tool") and made `tools check` verify the
/// wrong binary. Test binaries run the debug build, so this test fails by
/// panic if the override is not declared.
#[test]
fn cc_manifest_compiler_is_declared() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // A distinctly-named compiler wrapper on PATH.
    let bin_dir = project_path.join("toolbin");
    fs::create_dir_all(&bin_dir).unwrap();
    let wrapper = bin_dir.join("mycustomcc");
    fs::write(&wrapper, "#!/bin/sh\nexec gcc \"$@\"\n").unwrap();
    crate::common::make_executable(&wrapper);

    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc]\nsrc_dirs = [\".\"]\n",
    ).unwrap();
    fs::write(project_path.join("cc.yaml"), r#"
cc: mycustomcc
programs:
  - name: app
    sources: [src/main.c]
"#).unwrap();
    fs::write(
        project_path.join("src/main.c"),
        "int main(void) { return 0; }\n",
    ).unwrap();

    let path_env = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default());
    let output = run_rsconstruct_with_env(
        project_path, &["build"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(output.status.success(),
        "manifest-overridden compiler must be declared (debug builds assert on undeclared tools): stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
    assert!(project_path.join("out/cc/bin/app").exists(), "Program should link");
}
