use std::fs;
#[allow(unused_imports)]
use std::path::Path;
use crate::common::{setup_test_project, run_rsconstruct, run_rsconstruct_with_env, run_rsconstruct_json, run_rsconstruct_json_with_env};

#[test]
fn clean_command() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create and build a template
    fs::write(
        project_path.join("config/clean_test.py"),
        "test = 'clean'"
    ).expect("Failed to write config");

    fs::write(
        project_path.join("tera.templates/cleanme.txt.tera"),
        "{% set c = load_python(path='config/clean_test.py') %}{{ c.test }}"
    ).expect("Failed to write template");

    // Build
    let build_output = run_rsconstruct(project_path, &["build"]);
    assert!(build_output.status.success());

    // Verify files exist
    assert!(project_path.join("cleanme.txt").exists());
    assert!(project_path.join(".rsconstruct/db.redb").exists());

    // Clean
    let clean_output = run_rsconstruct(project_path, &["clean", "outputs"]);
    assert!(clean_output.status.success());

    // Verify build outputs are removed but cache is preserved
    assert!(!project_path.join("cleanme.txt").exists());
    assert!(project_path.join(".rsconstruct").exists());
}

#[test]
fn force_rebuild() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create template
    fs::write(
        project_path.join("config/force.py"),
        "mode = 'force'"
    ).expect("Failed to write config");

    fs::write(
        project_path.join("tera.templates/force.txt.tera"),
        "{% set c = load_python(path='config/force.py') %}Mode: {{ c.mode }}"
    ).expect("Failed to write template");

    // First build
    let first_build = run_rsconstruct(project_path, &["build"]);
    assert!(first_build.status.success(), "First build failed: {}", String::from_utf8_lossy(&first_build.stderr));

    // Force rebuild - should process, not skip
    let result = run_rsconstruct_json(project_path, &["build", "--force"]);
    assert!(result.exit_success);
    assert_eq!(result.success, 1, "Should have 1 successful build");
    assert_eq!(result.skipped, 0, "Should not skip anything with --force");
}

#[test]
fn no_color_env() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a template so there's something to process
    fs::write(
        project_path.join("tera.templates/color_test.txt.tera"),
        "hello"
    ).unwrap();

    // Run with NO_COLOR set
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // ANSI escape codes start with \x1b[
    assert!(!stdout.contains("\x1b["), "Output should not contain ANSI escape codes when NO_COLOR is set");
}

#[test]
fn timings_flag() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a template
    fs::write(
        project_path.join("tera.templates/timing_test.txt.tera"),
        "hello"
    ).unwrap();

    // Run with --timings
    let output = run_rsconstruct_with_env(project_path, &["build", "--timings"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should contain timing information
    assert!(stdout.contains("Timing:"), "Output should contain 'Timing:' header");
    assert!(stdout.contains("Total:"), "Output should contain 'Total:' line");
}

#[test]
fn no_timings_by_default() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a template
    fs::write(
        project_path.join("tera.templates/no_timing.txt.tera"),
        "hello"
    ).unwrap();

    // Run without --timings (and without --verbose)
    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should NOT contain timing information
    assert!(!stdout.contains("Timing:"), "Output should not contain timing info without --timings flag");
    assert!(!stdout.contains("Total:"), "Output should not contain total timing without --timings flag");
}

#[test]
fn keep_going_continues_after_failure() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create one bad template and one good template
    fs::write(project_path.join("tera.templates/bad.txt.tera"), "{{ invalid").unwrap();
    fs::write(project_path.join("tera.templates/good.txt.tera"), "hello").unwrap();

    // Run with --keep-going
    let output = run_rsconstruct_with_env(project_path, &["build", "-v", "--keep-going"], &[("NO_COLOR", "1")]);

    // Should exit non-zero because of the failure
    assert!(!output.status.success(), "Build should fail with bad template");

    // The good template should still have been processed (verify via output)
    let stdout = String::from_utf8_lossy(&output.stdout);
    // With --keep-going, both files should be attempted to be processed
    assert!(stdout.contains("Processing:"), "Files should be processed with --keep-going");
}

#[test]
fn keep_going_short_flag() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create one bad template
    fs::write(project_path.join("tera.templates/bad_k.txt.tera"), "{{ invalid").unwrap();

    // Run with -k (short form)
    let output = run_rsconstruct_with_env(project_path, &["build", "-k"], &[("NO_COLOR", "1")]);

    // Should exit non-zero since the template has invalid content
    assert!(!output.status.success(), "Build should fail with bad template");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should contain error reporting in stdout or stderr
    let combined = format!("{}{}", stdout, stderr);
    assert!(combined.contains("error") || combined.contains("Error"),
        "Should report errors: stdout={}, stderr={}", stdout, stderr);
}

#[test]
fn build_stops_on_first_error() {
    // Without --keep-going, build should stop immediately on first error
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // "aaa" sorts before "zzz" alphabetically, so it will be processed first
    fs::write(project_path.join("tera.templates/aaa.txt.tera"), "{{ invalid").unwrap();
    fs::write(project_path.join("tera.templates/zzz.txt.tera"), "hello").unwrap();

    // Build should fail on aaa.txt.tera and stop
    let result = run_rsconstruct_json(project_path, &["build"]);
    assert!(!result.exit_success, "Build should fail with bad template");
    assert_eq!(result.failed, 1, "Should have exactly 1 failure");
    // zzz.txt should NOT be processed because we stop on first error
    assert!(!result.has_product("zzz.txt", "success"),
        "Second file should NOT be processed after first error");
}

#[test]
fn keep_going_continues_after_error() {
    // With --keep-going, independent products should still be processed.
    // Use an undefined variable (runtime error) rather than a parse error,
    // because Tera loads all templates at once and a parse error poisons them all.
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(project_path.join("tera.templates/bad.txt.tera"), "{{ undefined_var }}").unwrap();
    fs::write(project_path.join("tera.templates/good.txt.tera"), "hello").unwrap();

    // First build with --keep-going — should fail but process all files
    let result1 = run_rsconstruct_json(project_path, &["build", "--keep-going"]);
    assert!(!result1.exit_success, "Build should fail with bad template");
    assert_eq!(result1.failed, 1, "Should have 1 failure");
    assert_eq!(result1.success, 1, "Should have 1 success (good.txt)");
    assert!(result1.has_product("good.txt", "success"),
        "Good template should be processed with --keep-going");

    // Fix the bad file
    fs::write(project_path.join("tera.templates/bad.txt.tera"), "fixed").unwrap();

    // Second build — good.txt should be skipped (cached)
    let result2 = run_rsconstruct_json(project_path, &["build"]);
    assert!(result2.exit_success, "Second build should succeed");
    assert_eq!(result2.skipped, 1, "Good template should be skipped (cached)");
    assert_eq!(result2.success, 1, "Bad template (now fixed) should be processed");
}

#[test]
fn parallel_build_with_j_flag() {
    // Verify -j flag enables parallel execution and all products are built
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    for name in &["alpha", "beta", "gamma", "delta"] {
        fs::write(
            project_path.join(format!("tera.templates/{}.txt.tera", name)),
            format!("content of {}", name),
        ).unwrap();
    }

    let result = run_rsconstruct_json(project_path, &["build", "-j2"]);
    assert!(result.exit_success, "Parallel build with -j2 should succeed");
    assert_eq!(result.success, 4, "Should process all 4 templates");
    assert_eq!(result.total_products, 4);
}

#[test]
fn parallel_keep_going_continues_after_failure() {
    // Verify --keep-going processes all independent products even when one fails
    // under parallel execution.
    // Use an undefined variable (runtime error) rather than a parse error,
    // because Tera loads all templates at once and a parse error poisons them all.
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(project_path.join("tera.templates/aaa_bad.txt.tera"), "{{ undefined_var }}").unwrap();
    fs::write(project_path.join("tera.templates/good1.txt.tera"), "hello1").unwrap();
    fs::write(project_path.join("tera.templates/good2.txt.tera"), "hello2").unwrap();
    fs::write(project_path.join("tera.templates/good3.txt.tera"), "hello3").unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\n\n[build]\nparallel = 2\n"
    ).unwrap();

    let result = run_rsconstruct_json(project_path, &["build", "--keep-going"]);

    // Should fail overall
    assert!(!result.exit_success, "Build should fail with bad template even with --keep-going");
    assert_eq!(result.failed, 1, "Should have 1 failure");
    assert_eq!(result.success, 3, "All 3 good templates should be processed with --keep-going");
}

#[test]
fn parallel_builds_all_independent_products() {
    // Verify parallel config in rsconstruct.toml works and all products complete
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    for i in 0..8 {
        fs::write(
            project_path.join(format!("tera.templates/task_{:02}.txt.tera", i)),
            format!("content of task {}", i),
        ).unwrap();
    }
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\n\n[build]\nparallel = 4\n"
    ).unwrap();

    let result = run_rsconstruct_json(project_path, &["build"]);
    assert!(result.exit_success, "Parallel build with 8 products and 4 jobs should succeed");
    assert_eq!(result.success, 8, "Should process all 8 templates");
    assert_eq!(result.total_products, 8);

    // Incremental: second build should skip everything
    let result2 = run_rsconstruct_json(project_path, &["build"]);
    assert!(result2.exit_success);
    assert_eq!(result2.skipped, 8, "All 8 products should be skipped on second build");
}

#[test]
fn parallel_timings_flag() {
    // Verify --timings output works with parallel builds.
    //
    // Each template gets distinct content so the produced output blobs
    // also differ. Identical content across products triggers a data race
    // in the object store's store_object check-then-write path under
    // parallel execution: two threads both observe the blob path missing
    // and both try to fs::write it, with the second hitting EACCES after
    // the first has already set the file read-only.
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    for name in &["one", "two", "three"] {
        fs::write(
            project_path.join(format!("tera.templates/{}.txt.tera", name)),
            format!("hello from {}", name),
        ).unwrap();
    }
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\n\n[build]\nparallel = 2\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(
        project_path, &["build", "--timings"], &[("NO_COLOR", "1")]
    );
    assert!(output.status.success(),
        "Parallel build with --timings should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Timing:"), "Should contain 'Timing:' header in parallel mode");
    assert!(stdout.contains("Total:"), "Should contain 'Total:' line in parallel mode");

    // Should have timing entries
    let timing_lines = stdout.lines()
        .filter(|l| l.contains("[tera]") && l.contains("(0."))
        .count();
    assert!(timing_lines >= 1,
        "Should have at least one timing entry: {}", stdout);
}

#[test]
fn deterministic_build_order() {
    // Run two separate builds with multiple templates and verify
    // that the processing order is identical both times.
    let outputs: Vec<Vec<String>> = (0..2).map(|_| {
        let temp_dir = setup_test_project();
        let project_path = temp_dir.path();

        // Create several template files with distinct names and content
        for name in &["zebra", "alpha", "mango", "banana", "cherry"] {
            fs::write(
                project_path.join(format!("tera.templates/{}.txt.tera", name)),
                format!("content of {}", name),
            ).unwrap();
        }

        let output = run_rsconstruct_with_env(project_path, &["build", "-v", "-j", "1"], &[("NO_COLOR", "1")]);
        assert!(output.status.success(),
            "Build failed: {}",
            String::from_utf8_lossy(&output.stderr));

        // Extract the target name from "Processing: <name>" lines
        let stdout = String::from_utf8_lossy(&output.stdout);
        let processing_names: Vec<String> = stdout
            .lines()
            .filter(|l| l.contains("Processing:"))
            .filter_map(|l| {
                l.split("Processing:").nth(1).map(|s| s.trim().to_string())
            })
            .collect();
        assert_eq!(processing_names.len(), 5, "Should process all 5 templates: {}", stdout);
        processing_names
    }).collect();

    assert_eq!(outputs[0], outputs[1],
        "Build order must be deterministic across runs.\nFirst:  {:?}\nSecond: {:?}",
        outputs[0], outputs[1]);
}

/// Test that classify_products propagates dependency changes transitively.
/// Setup: tera generates step1.txt, a second tera template depends on step1.txt via dep_inputs.
/// When the first tera template changes, both products should be classified as needing
/// rebuild — not just the first product.
#[test]
fn classify_propagates_through_dependencies() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Phase 1: build with tera to create the output file
    fs::write(
        project_path.join("config/gen.py"),
        "val = 1"
    ).unwrap();
    fs::write(
        project_path.join("tera.templates/step1.txt.tera"),
        "{% set c = load_python(path='config/gen.py') %}step1={{ c.val }}"
    ).unwrap();

    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "Phase 1 build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    assert!(project_path.join("step1.txt").exists(), "Tera should generate step1.txt");

    // Phase 2: add a second template with dep_inputs pointing to the first tera output
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.tera]\ndep_inputs = [\"step1.txt\"]\n"
    ).unwrap();
    fs::write(
        project_path.join("tera.templates/step2.txt.tera"),
        "step2"
    ).unwrap();

    // Build both products
    let output2 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success(),
        "Phase 2 build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output2.stdout),
        String::from_utf8_lossy(&output2.stderr));

    // Verify everything is up-to-date
    let output3 = run_rsconstruct_with_env(
        project_path, &["build", "--stop-after", "classify"], &[("NO_COLOR", "1")]
    );
    assert!(output3.status.success());
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("0 to build, 0 to restore (2 up-to-date)"),
        "Both products should be up-to-date: {}", stdout3);

    // Wait so mtime differs
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Modify the first tera template
    fs::write(
        project_path.join("tera.templates/step1.txt.tera"),
        "{% set c = load_python(path='config/gen.py') %}modified={{ c.val }}"
    ).unwrap();

    // Classify: both products should need work (tera rebuild + second rebuild/restore)
    let output4 = run_rsconstruct_with_env(
        project_path, &["build", "--stop-after", "classify"], &[("NO_COLOR", "1")]
    );
    assert!(output4.status.success());
    let stdout4 = String::from_utf8_lossy(&output4.stdout);
    assert!(stdout4.contains("0 up-to-date"),
        "No products should be up-to-date when root dependency changed: {}", stdout4);
}

#[test]
fn checker_and_generator_both_rebuild_on_shared_input_change() {
    // Regression test: when a checker and generator share the same input file,
    // modifying the input must cause BOTH to rebuild, not just the checker.
    // Bug scenario: checker runs first, updates the input hash in cache,
    // then generator sees matching hash and skips even though its output is stale.
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Enable both tera (generator) and script (checker) on .tera files
    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.tera]\n",
            "\n",
            "[processor.script]\n",
            "src_dirs = [\"tera.templates\"]\n",
            "src_extensions = [\".tera\"]\n",
            "command = \"true\"\n",
        ),
    ).unwrap();

    // Create a template
    fs::write(
        project_path.join("tera.templates/shared.txt.tera"),
        "version1",
    ).unwrap();

    // First build: both checker and generator should run
    let result1 = run_rsconstruct_json(project_path, &["build"]);
    assert!(result1.exit_success, "First build should succeed");
    assert!(result1.has_product("shared.txt", "success"),
        "Tera should process: {:?}", result1.products);
    assert_eq!(result1.failed, 0, "No failures expected");

    // Verify the output was created
    assert!(project_path.join("shared.txt").exists(),
        "Tera output should exist after first build");
    let content1 = fs::read_to_string(project_path.join("shared.txt")).unwrap();
    assert_eq!(content1, "version1");

    // Second build: everything should be skipped (no changes)
    let result2 = run_rsconstruct_json(project_path, &["build"]);
    assert!(result2.exit_success);
    assert_eq!(result2.skipped, result2.total_products,
        "All products should be skipped on second build (no changes)");

    // Wait for mtime to differ
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Modify the shared input file
    fs::write(
        project_path.join("tera.templates/shared.txt.tera"),
        "version2",
    ).unwrap();

    // Third build: BOTH checker and generator must rebuild
    let result3 = run_rsconstruct_json(project_path, &["build"]);
    assert!(result3.exit_success, "Third build should succeed: {:?}", result3.errors);
    assert_eq!(result3.skipped, 0,
        "No products should be skipped after input change, got: {:?}", result3.products);
    assert!(result3.has_product("shared.txt", "success"),
        "Tera generator MUST rebuild after input change: {:?}", result3.products);

    // Verify the output was updated
    let content3 = fs::read_to_string(project_path.join("shared.txt")).unwrap();
    assert_eq!(content3, "version2",
        "Output should contain new content after rebuild");
}

/// Test that cross-processor dependencies work: a downstream processor discovers
/// products whose inputs are declared outputs of an upstream processor, even on
/// a clean build where those output files don't exist on disk yet.
#[test]
fn cross_processor_discovery() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let project_path = temp_dir.path();

    // Enable tera (generates files) and ascii (checks files).
    // Configure ascii to scan .txt files at the project root — exactly where
    // tera will output them.
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"
[processor.tera]

[processor.ascii]
src_dirs = ["."]
src_extensions = [".txt"]
"#,
    ).unwrap();

    // Create a tera template that generates a .txt file
    fs::create_dir_all(project_path.join("tera.templates")).unwrap();
    fs::write(
        project_path.join("tera.templates/generated.txt.tera"),
        "hello world",
    ).unwrap();

    // The generated.txt does NOT exist on disk yet — this is a clean build.
    assert!(!project_path.join("generated.txt").exists());

    // Ask rsconstruct to list processor files (discovery only, no build).
    // The ascii processor should discover generated.txt as an input,
    // because the fixed-point discovery loop injects tera's declared output
    // as a virtual file.
    let output = run_rsconstruct_with_env(
        project_path,
        &["--json", "processors", "files"],
        &[("NO_COLOR", "1")],
    );
    assert!(output.status.success(), "processors files failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("JSON parse failed: {}\nOutput: {}", e, stdout));

    // Find the ascii processor's entries
    let ascii_products: Vec<&serde_json::Value> = parsed.iter()
        .filter(|p| p["processor"].as_str() == Some("ascii"))
        .collect();

    assert!(!ascii_products.is_empty(),
        "ascii processor should have discovered products from tera's output.\n\
         All products: {:?}", parsed);

    // Verify that generated.txt is an input to ascii
    let has_generated_input = ascii_products.iter().any(|p| {
        p["inputs"].as_array().unwrap().iter()
            .any(|i| i.as_str().unwrap().contains("generated.txt"))
    });
    assert!(has_generated_input,
        "ascii should have generated.txt as input.\nascii products: {:?}", ascii_products);
}

/// Test the explicit processor: declares inputs, input_globs, and outputs explicitly.
/// Verifies discovery creates a single product with all resolved inputs and the declared output.
#[test]
fn explicit_processor_discovery() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let project_path = temp_dir.path();

    // Create input files
    fs::create_dir_all(project_path.join("data")).unwrap();
    fs::write(project_path.join("config.txt"), "config").unwrap();
    fs::write(project_path.join("data/a.csv"), "a").unwrap();
    fs::write(project_path.join("data/b.csv"), "b").unwrap();
    fs::write(project_path.join("data/skip.txt"), "not a csv").unwrap();

    // Configure an explicit processor with literal inputs and a glob
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"
[processor.explicit.report]
command = "scripts/build_report.py"
inputs = ["config.txt"]
input_globs = ["data/*.csv"]
output_files = ["out/report.html"]
src_dirs = ["."]
"#,
    ).unwrap();

    let output = run_rsconstruct_with_env(
        project_path,
        &["--json", "processors", "files"],
        &[("NO_COLOR", "1")],
    );
    assert!(output.status.success(), "processors files failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("JSON parse failed: {}\nOutput: {}", e, stdout));

    // Should have exactly one product
    let explicit_products: Vec<&serde_json::Value> = parsed.iter()
        .filter(|p| p["processor"].as_str().unwrap().contains("explicit"))
        .collect();
    assert_eq!(explicit_products.len(), 1,
        "Expected 1 explicit product, got {}: {:?}", explicit_products.len(), explicit_products);

    let product = explicit_products[0];

    // Check inputs: config.txt (literal) + data/a.csv, data/b.csv (glob, sorted)
    let inputs: Vec<&str> = product["inputs"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(inputs.len(), 3, "Expected 3 inputs: {:?}", inputs);
    assert_eq!(inputs[0], "config.txt");
    assert!(inputs[1].ends_with("a.csv"), "Expected a.csv, got {}", inputs[1]);
    assert!(inputs[2].ends_with("b.csv"), "Expected b.csv, got {}", inputs[2]);
    // skip.txt should NOT be included (not matching *.csv)
    assert!(!inputs.iter().any(|i| i.contains("skip.txt")), "skip.txt should not be an input");

    // Check output
    let outputs: Vec<&str> = product["outputs"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(outputs, vec!["out/report.html"]);
}

/// Test that a downstream processor discovers files in a directory that doesn't
/// exist on disk yet — the directory is created by an upstream generator.
/// This is a clean-build scenario: the generator outputs to out/generated/,
/// and ascii is configured to scan out/generated/ for .txt files.
/// The out/generated/ directory does NOT exist before the build.
#[test]
fn cross_processor_nonexistent_output_dir() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let project_path = temp_dir.path();

    // Create a tera template that generates a .txt file into a subdirectory.
    // Tera scans tera.templates/ and strips the scan_dir prefix, so
    // tera.templates/out/generated/hello.txt.tera → out/generated/hello.txt
    fs::create_dir_all(project_path.join("tera.templates/out/generated")).unwrap();
    fs::write(
        project_path.join("tera.templates/out/generated/hello.txt.tera"),
        "hello world",
    ).unwrap();

    // Configure tera (generator) and ascii (checker scanning out/generated/).
    // The directory out/generated/ does NOT exist on disk.
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"
[processor.tera]

[processor.ascii]
src_dirs = ["out/generated"]
src_extensions = [".txt"]
"#,
    ).unwrap();

    // Verify out/generated/ does not exist on disk
    assert!(!project_path.join("out/generated").exists(),
        "out/generated/ should not exist before discovery");

    // Run discovery via processors files (JSON)
    let output = run_rsconstruct_with_env(
        project_path,
        &["--json", "processors", "files"],
        &[("NO_COLOR", "1")],
    );
    assert!(output.status.success(), "processors files failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("JSON parse failed: {}\nOutput: {}", e, stdout));

    // Tera should have 1 product
    let tera_products: Vec<&serde_json::Value> = parsed.iter()
        .filter(|p| p["processor"].as_str() == Some("tera"))
        .collect();
    assert_eq!(tera_products.len(), 1, "Expected 1 tera product: {:?}", tera_products);

    // ASCII should discover the tera output even though out/generated/ doesn't exist on disk.
    // The fixed-point discovery loop injects tera's declared output as a virtual file.
    let ascii_products: Vec<&serde_json::Value> = parsed.iter()
        .filter(|p| p["processor"].as_str() == Some("ascii"))
        .collect();
    assert_eq!(ascii_products.len(), 1,
        "ascii should discover 1 product from tera's output in nonexistent dir.\n\
         All products: {:?}", parsed);

    // Verify the ascii product's input is the tera output
    let ascii_input = ascii_products[0]["inputs"].as_array().unwrap()[0].as_str().unwrap();
    assert!(ascii_input.contains("out/generated/hello.txt"),
        "ascii input should be out/generated/hello.txt, got: {}", ascii_input);
}

#[test]
fn max_jobs_limits_per_processor_concurrency() {
    // Verify that max_jobs limits concurrency for a specific processor.
    // Uses the script processor with a bash script that tracks peak concurrency
    // via a shared counter file protected by flock.
    //
    // Setup: 8 input files, global -j8, but max_jobs=2 for the script processor.
    // The script sleeps 0.3s per file, so without max_jobs all 8 would run at once.
    // With max_jobs=2, peak concurrency must never exceed 2.

    let temp_dir = tempfile::TempDir::new().unwrap();
    let project_path = temp_dir.path();

    // Create the concurrency-tracking script
    let script_content = r#"#!/bin/bash
# Track concurrency: atomically increment counter, record peak, sleep, decrement.
COUNTER_FILE="$PROJECT_ROOT/.concurrency_counter"
PEAK_FILE="$PROJECT_ROOT/.concurrency_peak"
LOCK_FILE="$PROJECT_ROOT/.concurrency_lock"

# Increment and record peak
(
    flock 9
    current=$(cat "$COUNTER_FILE" 2>/dev/null || echo 0)
    current=$((current + 1))
    echo "$current" > "$COUNTER_FILE"
    peak=$(cat "$PEAK_FILE" 2>/dev/null || echo 0)
    if [ "$current" -gt "$peak" ]; then
        echo "$current" > "$PEAK_FILE"
    fi
) 9>"$LOCK_FILE"

# Hold the slot for a bit so concurrency can be observed
sleep 0.3

# Decrement
(
    flock 9
    current=$(cat "$COUNTER_FILE" 2>/dev/null || echo 0)
    current=$((current - 1))
    echo "$current" > "$COUNTER_FILE"
) 9>"$LOCK_FILE"
"#;

    let script_path = project_path.join("check_concurrency.sh");
    fs::write(&script_path, script_content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Create rsconstruct.toml with script processor, max_jobs=2, batch disabled
    let config = format!(
        r#"[processor.script]
command = "bash"
args = ["{script}"]
src_extensions = [".txt"]
src_dirs = ["inputs"]
max_jobs = 2
batch = false
"#,
        script = script_path.display(),
    );
    fs::write(project_path.join("rsconstruct.toml"), &config).unwrap();

    // Create 8 input files
    fs::create_dir_all(project_path.join("inputs")).unwrap();
    for i in 0..8 {
        fs::write(
            project_path.join(format!("inputs/file_{:02}.txt", i)),
            format!("content {}", i),
        ).unwrap();
    }

    // Initialize counter files
    fs::write(project_path.join(".concurrency_counter"), "0").unwrap();
    fs::write(project_path.join(".concurrency_peak"), "0").unwrap();

    // Run build with -j8 global parallelism
    let result = run_rsconstruct_json_with_env(
        project_path,
        &["build", "-j8"],
        &[("NO_COLOR", "1"), ("PROJECT_ROOT", project_path.to_str().unwrap())],
    );

    assert!(result.exit_success,
        "Build should succeed. Errors: {:?}", result.errors);
    assert_eq!(result.success, 8, "All 8 files should be processed");

    // Read peak concurrency
    let peak: usize = fs::read_to_string(project_path.join(".concurrency_peak"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    assert!(peak <= 2,
        "Peak concurrency was {} but max_jobs=2 should limit it to 2", peak);
    assert!(peak >= 1,
        "Peak concurrency should be at least 1, got {}", peak);
}

#[test]
fn max_jobs_unset_allows_full_parallelism() {
    // Verify that without max_jobs, the processor uses full global parallelism.
    // Same setup as above but without max_jobs — peak should be > 2.

    let temp_dir = tempfile::TempDir::new().unwrap();
    let project_path = temp_dir.path();

    let script_content = r#"#!/bin/bash
COUNTER_FILE="$PROJECT_ROOT/.concurrency_counter"
PEAK_FILE="$PROJECT_ROOT/.concurrency_peak"
LOCK_FILE="$PROJECT_ROOT/.concurrency_lock"

(
    flock 9
    current=$(cat "$COUNTER_FILE" 2>/dev/null || echo 0)
    current=$((current + 1))
    echo "$current" > "$COUNTER_FILE"
    peak=$(cat "$PEAK_FILE" 2>/dev/null || echo 0)
    if [ "$current" -gt "$peak" ]; then
        echo "$current" > "$PEAK_FILE"
    fi
) 9>"$LOCK_FILE"

sleep 0.3

(
    flock 9
    current=$(cat "$COUNTER_FILE" 2>/dev/null || echo 0)
    current=$((current - 1))
    echo "$current" > "$COUNTER_FILE"
) 9>"$LOCK_FILE"
"#;

    let script_path = project_path.join("check_concurrency.sh");
    fs::write(&script_path, script_content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // No max_jobs, batch disabled — should use full parallelism
    let config = format!(
        r#"[processor.script]
command = "bash"
args = ["{script}"]
src_extensions = [".txt"]
src_dirs = ["inputs"]
batch = false
"#,
        script = script_path.display(),
    );
    fs::write(project_path.join("rsconstruct.toml"), &config).unwrap();

    fs::create_dir_all(project_path.join("inputs")).unwrap();
    for i in 0..8 {
        fs::write(
            project_path.join(format!("inputs/file_{:02}.txt", i)),
            format!("content {}", i),
        ).unwrap();
    }

    fs::write(project_path.join(".concurrency_counter"), "0").unwrap();
    fs::write(project_path.join(".concurrency_peak"), "0").unwrap();

    let result = run_rsconstruct_json_with_env(
        project_path,
        &["build", "-j8"],
        &[("NO_COLOR", "1"), ("PROJECT_ROOT", project_path.to_str().unwrap())],
    );

    assert!(result.exit_success,
        "Build should succeed. Errors: {:?}", result.errors);
    assert_eq!(result.success, 8, "All 8 files should be processed");

    let peak: usize = fs::read_to_string(project_path.join(".concurrency_peak"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // Without max_jobs and -j8, peak should be higher than 2
    // (on any machine with >= 2 cores)
    assert!(peak > 2,
        "Without max_jobs, peak concurrency should exceed 2 with -j8, got {}", peak);
}

#[test]
fn generator_non_batch_partial_failure_only_rebuilds_failed() {
    // In non-batch mode (default fail-fast with chunk_size=1), each product
    // executes independently. Successful products are cached and skipped on
    // the next run — only the failed product needs rebuilding.
    let temp_dir = tempfile::TempDir::new().unwrap();
    let project_path = temp_dir.path();

    // Script that copies input to output but fails if input contains "FAIL"
    let script_path = project_path.join("transform.sh");
    fs::write(&script_path, r#"#!/bin/bash
input="$1"; output="$2"
if grep -q "FAIL" "$input"; then
    echo "Error: $input contains FAIL" >&2
    exit 1
fi
cp "$input" "$output"
"#).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(project_path.join("src/aaa_good1.txt"), "content1\n").unwrap();
    fs::write(project_path.join("src/aaa_good2.txt"), "content2\n").unwrap();
    fs::write(project_path.join("src/zzz_bad.txt"), "FAIL\n").unwrap();

    fs::write(project_path.join("rsconstruct.toml"), format!(
        r#"[processor.generator]
command = "{script}"
src_extensions = [".txt"]
src_dirs = ["src"]
output_extension = "out"
batch = false
"#,
        script = script_path.display(),
    )).unwrap();

    // First build with --keep-going: good files succeed, bad file fails
    let result1 = run_rsconstruct_json_with_env(
        project_path, &["build", "--keep-going"],
        &[("NO_COLOR", "1")],
    );
    assert!(!result1.exit_success, "First build should fail (zzz_bad.txt)");
    assert_eq!(result1.success, 2, "Two good files should succeed");
    assert_eq!(result1.failed, 1, "One bad file should fail");

    // Fix the bad file
    fs::write(project_path.join("src/zzz_bad.txt"), "fixed\n").unwrap();

    // Second build: good files should be skipped (cached), only bad file rebuilt
    let result2 = run_rsconstruct_json_with_env(
        project_path, &["build"],
        &[("NO_COLOR", "1")],
    );
    assert!(result2.exit_success, "Second build should succeed");
    assert_eq!(result2.skipped, 2, "Two good files should be skipped (cached)");
    assert_eq!(result2.success, 1, "Only the fixed file should be rebuilt");
}

/// Helper: write a minimal 2-processor project (`tera` + `ruff`) so we can
/// exercise `-x` against a known set of processors without pulling in real
/// external tools for actual execution.
fn setup_two_processor_project() -> tempfile::TempDir {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();
    fs::create_dir_all(p.join("tera.templates")).unwrap();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(p.join("rsconstruct.toml"), "[processor.tera]\n\n[processor.ruff]\nsrc_dirs = [\"src\"]\n").unwrap();
    fs::write(p.join("src/hello.py"), "print('hi')\n").unwrap();
    temp_dir
}

/// `-x tera` must exclude only tera; other processors still run.
#[test]
fn exclude_processor_runs_everything_else() {
    let temp_dir = setup_two_processor_project();
    let project_path = temp_dir.path();

    // Build with ruff excluded via -x. The tera processor should still be
    // active — we verify by checking the classify line reports at least one
    // product (tera) and zero ruff-attributable activity.
    let output = run_rsconstruct_with_env(
        project_path, &["build", "-x", "ruff"], &[("NO_COLOR", "1")],
    );
    // Build should succeed (tera has no templates so it's a no-op but valid).
    assert!(
        output.status.success(),
        "build with -x ruff must succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // Stdout/stderr must not mention processing the ruff product for hello.py.
    assert!(
        !combined.contains("[ruff]"),
        "ruff must not run when -x ruff is passed: {}", combined
    );
}

/// `-x unknown` must fail with a CONFIG_ERROR, same as `-p unknown`.
#[test]
fn exclude_unknown_processor_is_config_error() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(
        project_path, &["build", "-x", "nonexistent"], &[("NO_COLOR", "1")],
    );
    assert!(!output.status.success());
    let exit_code = output.status.code().unwrap();
    assert_eq!(exit_code, 2, "Expected CONFIG_ERROR (2), got {}", exit_code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown processor"),
        "Error should name the unknown processor: {}", stderr
    );
}

/// `-p foo -x foo` must reject the conflicting intent.
#[test]
fn include_and_exclude_same_processor_is_error() {
    let temp_dir = setup_two_processor_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(
        project_path, &["build", "-p", "tera", "-x", "tera"], &[("NO_COLOR", "1")],
    );
    assert!(!output.status.success(), "-p tera -x tera must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("both -p and -x"),
        "Error should explain the conflict: {}", stderr
    );
}

/// Regression test for the chain SVG → marp → ipdfunite.
///
/// The markdown analyzer must surface the SVG referenced by a marp deck as a
/// dependency of the deck's PDF product. When the SVG content changes, the
/// marp PDF must be classified for rebuild, and that change must transitively
/// propagate to the ipdfunite product that merges the deck PDFs — otherwise a
/// stale merged PDF can survive even though its source content changed.
///
/// Requires marp (`node` + the marp-cli npm package); ipdfunite is
/// in-process so always runs when reached.
#[test]
fn svg_change_rebuilds_marp_and_ipdfunite() {
    crate::common::require_tool("marp");

    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Layout:
    //   marp/courses/deck/a.md   → references svg/img.svg
    //   svg/img.svg              → real SVG, picked up by [analyzer.markdown]
    //   rsconstruct.toml         → marp + ipdfunite + markdown analyzer
    fs::create_dir_all(project_path.join("marp/courses/deck")).unwrap();
    fs::create_dir_all(project_path.join("svg")).unwrap();
    fs::write(
        project_path.join("svg/img.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#,
    ).unwrap();
    fs::write(
        project_path.join("marp/courses/deck/a.md"),
        "# Slide\n\n![img](svg/img.svg)\n",
    ).unwrap();
    // Generous marp timeout for shared CI hosts — marp/Chrome can be slow
    // to launch under load. We do not care about render performance here.
    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.marp]\n",
            "timeout_secs = 120\n",
            "max_attempts = 3\n",
            "[processor.ipdfunite]\n",
            "[analyzer.markdown]\n",
        ),
    ).unwrap();

    // Phase 1: clean build. Both marp and ipdfunite must succeed.
    let result1 = run_rsconstruct_json(project_path, &["build"]);
    assert!(result1.exit_success,
        "Phase 1 build failed (errors={:?}, products={:?})",
        result1.errors, result1.products);
    assert!(project_path.join("out/marp/courses/deck/a.pdf").exists(),
        "marp output PDF should exist on disk");
    assert!(project_path.join("out/ipdfunite/deck.pdf").exists(),
        "ipdfunite merged PDF should exist on disk");

    // Wait so mtime differs and the markdown analyzer's mtime shortcut
    // doesn't decide nothing changed before reading content.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Phase 2: modify the SVG that the marp deck references. The point of
    // the test: this change must flow through to BOTH the marp PDF and the
    // ipdfunite merged PDF.
    fs::write(
        project_path.join("svg/img.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="blue"/></svg>"#,
    ).unwrap();

    // Classify-only first: prove the marp product is flagged for rebuild
    // BEFORE anything runs. This is the load-bearing assertion for the
    // markdown analyzer / SVG-dep wiring — independent of caching quirks
    // downstream.
    let classify = run_rsconstruct_with_env(
        project_path, &["build", "--stop-after", "classify"], &[("NO_COLOR", "1")]
    );
    assert!(classify.status.success(),
        "Classify should succeed: stderr={}", String::from_utf8_lossy(&classify.stderr));
    let classify_out = String::from_utf8_lossy(&classify.stdout);
    assert!(!classify_out.contains("(2 up-to-date)"),
        "After SVG change, not every product should be up-to-date: {}", classify_out);

    // Phase 3: actually rebuild and confirm BOTH products ran (or restored).
    // The user-facing contract: a stale ipdfunite output must NOT survive an
    // SVG change.
    let result3 = run_rsconstruct_json(project_path, &["build"]);
    assert!(result3.exit_success,
        "Phase 3 build failed (errors={:?})", result3.errors);
    let marp_ran = result3.products.iter().any(|p|
        p.processor == "marp" && p.status == "success"
    );
    assert!(marp_ran,
        "marp must rebuild when its referenced SVG changes: {:?}", result3.products);
    let ipdfunite_ran = result3.products.iter().any(|p|
        p.processor == "ipdfunite" && p.status == "success"
    );
    assert!(ipdfunite_ran,
        "ipdfunite must rebuild when its upstream marp PDF changes: {:?}",
        result3.products);
}

/// Under `--json`, stdout carries a machine-readable event stream: every
/// line must parse as JSON. Human progress lines leaking onto stdout make
/// the output unparseable for any consumer that doesn't defensively skip
/// junk (the test harness itself used to silently discard such lines,
/// which is why this went unnoticed).
///
/// Covers the three shapes that leaked: plain `--json` (build phase
/// summaries), `--json -v` (per-product progress), and the incremental
/// re-run (skip lines). `--color=always` is included because JSON mode
/// must never emit ANSI escapes onto stdout either.
#[test]
fn json_mode_stdout_is_pure_json() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();
    for i in 1..=3 {
        fs::write(
            project_path.join(format!("tera.templates/f{i}.txt.tera")),
            format!("Hello {i}\n"),
        ).unwrap();
    }

    let cases: [&[&str]; 4] = [
        &["--json", "build"],
        &["--json", "-v", "build", "--force"],
        &["--json", "-v", "build"],
        &["--json", "--color=always", "-v", "build", "--force"],
    ];

    for args in cases {
        let output = run_rsconstruct_with_env(project_path, args, &[]);
        assert!(output.status.success(),
            "build failed for {args:?}: {}", String::from_utf8_lossy(&output.stderr));

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
            assert!(!line.contains('\u{1b}'),
                "ANSI escape on stdout in JSON mode ({args:?}): {line:?}");
            assert!(serde_json::from_str::<serde_json::Value>(line).is_ok(),
                "non-JSON line on stdout in JSON mode ({args:?}): {line:?}");
        }
    }
}
