use std::fs;
use tempfile::TempDir;
use crate::common::{setup_test_project, run_rsconstruct, run_rsconstruct_with_env};

#[test]
fn cache_operations() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a simple template
    fs::write(
        project_path.join("config/cache_test.py"),
        "value = 'cached'"
    ).unwrap();

    fs::write(
        project_path.join("tera.templates/cached.txt.tera"),
        "{% set c = load_python(path='config/cache_test.py') %}{{ c.value }}"
    ).unwrap();

    // Build to populate cache
    let output = run_rsconstruct(project_path, &["build"]);
    assert!(output.status.success());
    assert!(project_path.join("cached.txt").exists());
    assert!(project_path.join(".rsconstruct/db.redb").exists());
    assert!(project_path.join(".rsconstruct/objects").exists());

    // Check cache size reports objects
    let size_output = run_rsconstruct(project_path, &["cache", "size"]);
    assert!(size_output.status.success());
    let size_stdout = String::from_utf8_lossy(&size_output.stdout);
    assert!(size_stdout.contains("Cache size:"));
    assert!(size_stdout.contains("objects"));
    // Should have at least 1 object
    assert!(!size_stdout.contains("0 objects"), "Cache should have objects after build");

    // Delete the output file, then rebuild — should restore from cache
    fs::remove_file(project_path.join("cached.txt")).unwrap();
    assert!(!project_path.join("cached.txt").exists());

    let restore_output = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(restore_output.status.success());
    let restore_stdout = String::from_utf8_lossy(&restore_output.stdout);
    assert!(restore_stdout.contains("Restored from cache:"));
    assert!(project_path.join("cached.txt").exists());

    // Verify restored content is correct
    let content = fs::read_to_string(project_path.join("cached.txt")).unwrap();
    assert_eq!(content.trim(), "cached");

    // Trim cache (nothing unreferenced, so 0 removed)
    let trim_output = run_rsconstruct(project_path, &["cache", "trim"]);
    assert!(trim_output.status.success());
    let trim_stdout = String::from_utf8_lossy(&trim_output.stdout);
    assert!(trim_stdout.contains("0 unreferenced objects"));

    // Clear cache entirely
    let clear_output = run_rsconstruct(project_path, &["cache", "clear"]);
    assert!(clear_output.status.success());
    // .rsconstruct/ exists (fresh db) but objects dir is gone
    assert!(!project_path.join(".rsconstruct").join("objects").exists());

    // Cache size after clear should be 0
    let size_after = run_rsconstruct(project_path, &["cache", "size"]);
    assert!(size_after.status.success());
    let size_after_stdout = String::from_utf8_lossy(&size_after.stdout);
    assert!(size_after_stdout.contains("0 B"));
    assert!(size_after_stdout.contains("0 objects"));
}

#[test]
fn cache_list_shows_entries() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tera.templates/list_test.txt.tera"),
        "hello"
    ).unwrap();

    // Build to populate cache
    let build = run_rsconstruct(project_path, &["build"]);
    assert!(build.status.success());

    // List cache — output is JSON
    let output = run_rsconstruct_with_env(project_path, &["cache", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Cache list should be valid JSON: {}\nOutput: {}", e, stdout));
    let arr = entries.as_array().expect("Cache list should be a JSON array");
    assert!(!arr.is_empty(), "Cache list should have entries after build");
    let first = &arr[0];
    assert!(first["cache_key"].as_str().is_some(),
        "Cache entry should have a cache_key: {}", first);
}

#[test]
fn cache_list_empty() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "\n"
    ).unwrap();

    // Empty cache should produce an empty JSON array
    let output = run_rsconstruct_with_env(project_path, &["cache", "list"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Cache list should be valid JSON: {}\nOutput: {}", e, stdout));
    let arr = entries.as_array().expect("Cache list should be a JSON array");
    assert!(arr.is_empty(), "Empty cache should produce an empty JSON array: {}", stdout);
}

#[test]
fn cache_stats_empty() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["cache", "stats"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Cache is empty"), "Expected 'Cache is empty' message, got: {}", stdout);
}

#[test]
fn cache_stats_after_build() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tera.templates/stats_test.txt.tera"),
        "hello"
    ).unwrap();

    // Build to populate cache
    let build = run_rsconstruct(project_path, &["build"]);
    assert!(build.status.success());

    // Check stats
    let output = run_rsconstruct_with_env(project_path, &["cache", "stats"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("entries"), "Expected 'entries' in stats, got: {}", stdout);
}

#[test]
fn cache_stats_json() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tera.templates/json_test.txt.tera"),
        "hello"
    ).unwrap();

    // Build to populate cache
    let build = run_rsconstruct(project_path, &["build"]);
    assert!(build.status.success());

    // Verify cache stats outputs valid JSON
    let output = run_rsconstruct_with_env(project_path, &["--json", "cache", "stats"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Cache stats JSON should be valid: {}\nOutput: {}", e, stdout));
    assert!(parsed.is_object(), "Expected JSON object, got: {}", stdout);
    assert!(parsed.get("all").is_some(), "Expected 'all' key in stats JSON, got: {}", stdout);
}

#[test]
fn cache_clear_removes_everything() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tera.templates/clear_test.txt.tera"),
        "hello"
    ).unwrap();

    // Build to populate cache
    let build = run_rsconstruct(project_path, &["build"]);
    assert!(build.status.success());
    assert!(project_path.join(".rsconstruct").exists(), "Cache dir should exist after build");
    assert!(project_path.join(".rsconstruct/objects").exists(), "Objects dir should exist after build");
    assert!(project_path.join(".rsconstruct/descriptors").exists(), "Descriptors dir should exist after build");

    // Clear cache
    let clear = run_rsconstruct(project_path, &["cache", "clear"]);
    assert!(clear.status.success());

    // Entire .rsconstruct directory should be gone
    assert!(!project_path.join(".rsconstruct").exists(),
        "Entire .rsconstruct dir should be removed after cache clear");
    assert!(!project_path.join(".rsconstruct/objects").exists(),
        "Objects dir should not exist after cache clear");
    assert!(!project_path.join(".rsconstruct/descriptors").exists(),
        "Descriptors dir should not exist after cache clear");

    // Rebuild should work from scratch (full rebuild, no restore)
    let rebuild = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(rebuild.status.success());
    let stdout = String::from_utf8_lossy(&rebuild.stdout);
    assert!(stdout.contains("Processing:"), "Should do a full rebuild after cache clear: {}", stdout);
    assert!(!stdout.contains("Restored from cache:"), "Should not restore after cache clear: {}", stdout);
}

/// `cache remove-stale` must keep entries for the current project state.
///
/// Regression test: `valid_cache_keys` used to collect plaintext
/// `Product::cache_key()` strings while `remove_stale` compared them against
/// hashed descriptor keys reconstructed from on-disk paths. Nothing ever
/// matched, so `remove-stale` deleted the entire cache and `cache stale`
/// reported 100% of entries as stale.
#[test]
fn remove_stale_keeps_current_entries() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("tera.templates/stale_test.txt.tera"),
        "stale test content"
    ).unwrap();

    // Build to populate cache
    let build = run_rsconstruct(project_path, &["build"]);
    assert!(build.status.success());
    assert!(project_path.join("stale_test.txt").exists());

    // Everything in the cache corresponds to the current project state
    let stale = run_rsconstruct_with_env(project_path, &["cache", "stale"], &[("NO_COLOR", "1")]);
    assert!(stale.status.success());
    let stale_stdout = String::from_utf8_lossy(&stale.stdout);
    assert!(!stale_stdout.lines().any(|l| l.starts_with("stale ")),
        "Fresh build must have no stale entries: {}", stale_stdout);
    assert!(stale_stdout.lines().any(|l| l.starts_with("current ")),
        "Fresh build's entries must be recognized as current: {}", stale_stdout);

    // remove-stale must not remove anything
    let remove = run_rsconstruct_with_env(project_path, &["cache", "remove-stale"], &[("NO_COLOR", "1")]);
    assert!(remove.status.success());
    let remove_stdout = String::from_utf8_lossy(&remove.stdout);
    assert!(remove_stdout.contains("Removed 0 stale index entries"),
        "remove-stale after a fresh build must remove nothing: {}", remove_stdout);

    // The cache must still work: delete the output and restore from cache
    fs::remove_file(project_path.join("stale_test.txt")).unwrap();
    let rebuild = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(rebuild.status.success());
    let rebuild_stdout = String::from_utf8_lossy(&rebuild.stdout);
    assert!(rebuild_stdout.contains("Restored from cache:"),
        "Cache must survive remove-stale: {}", rebuild_stdout);

    // Now actually make the entry stale: change the input content. The old
    // descriptor no longer matches any current product state.
    fs::write(
        project_path.join("tera.templates/stale_test.txt.tera"),
        "changed content"
    ).unwrap();
    let remove2 = run_rsconstruct_with_env(project_path, &["cache", "remove-stale"], &[("NO_COLOR", "1")]);
    assert!(remove2.status.success());
    let remove2_stdout = String::from_utf8_lossy(&remove2.stdout);
    assert!(!remove2_stdout.contains("Removed 0 stale index entries"),
        "Changing input content must make the old entry stale: {}", remove2_stdout);
}

#[test]
fn cache_survives_input_rename() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Create a template and build it
    fs::write(
        project_path.join("tera.templates/original.txt.tera"),
        "rename test content"
    ).unwrap();

    let build1 = run_rsconstruct(project_path, &["build"]);
    assert!(build1.status.success());
    assert!(project_path.join("original.txt").exists());

    // Remove the output and rename the input (same content, different name)
    fs::remove_file(project_path.join("original.txt")).unwrap();
    fs::rename(
        project_path.join("tera.templates/original.txt.tera"),
        project_path.join("tera.templates/renamed.txt.tera"),
    ).unwrap();

    // Build again — the content is identical, so the cache should hit
    // and restore the output (under the new name) from cache
    let build2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(build2.status.success(),
        "Build after rename should succeed: stderr={}",
        String::from_utf8_lossy(&build2.stderr));

    let stdout = String::from_utf8_lossy(&build2.stdout);
    assert!(stdout.contains("Restored from cache:"),
        "Renamed input with same content should restore from cache: {}", stdout);
    assert!(project_path.join("renamed.txt").exists(),
        "Output under new name should exist after restore");

    // Verify content is correct
    let content = fs::read_to_string(project_path.join("renamed.txt")).unwrap();
    assert_eq!(content.trim(), "rename test content");
}

/// Upgrading a build tool must invalidate everything that tool produced.
///
/// This used to be opt-in: `processor_tool_hashes` returned an empty map
/// unless a `.tools.versions` lock file existed, so in the common case (no
/// lock file) a tool upgrade left every cached result valid — the build
/// would happily reuse output produced by the previous version.
///
/// The tool here is a shell script the test owns, so "upgrading" it is a
/// file write. `script` declares its `command` as a required tool, which is
/// what feeds the tool component of the cache key.
#[test]
fn tool_upgrade_invalidates_cached_results() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // A "tool" we can upgrade, on PATH via a bin dir we control.
    let bin_dir = project_path.join("toolbin");
    fs::create_dir_all(&bin_dir).unwrap();
    let tool = bin_dir.join("faketool");
    fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    crate::common::make_executable(&tool);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.script]\ncommand = \"faketool\"\nsrc_dirs = [\"src\"]\nsrc_extensions = [\".txt\"]\n",
    ).unwrap();
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(project_path.join("src/a.txt"), "content\n").unwrap();

    let path_env = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default());

    // First build populates the cache.
    let build1 = run_rsconstruct_with_env(
        project_path, &["build", "-v"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(build1.status.success(),
        "first build failed: {}", String::from_utf8_lossy(&build1.stderr));

    // Second build with the tool unchanged: cached, nothing re-run.
    let build2 = run_rsconstruct_with_env(
        project_path, &["build", "-v"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(build2.status.success(),
        "second build failed: {}", String::from_utf8_lossy(&build2.stderr));
    let stdout2 = String::from_utf8_lossy(&build2.stdout);
    assert!(!stdout2.contains("Processing:"),
        "unchanged tool + unchanged inputs must not re-run anything: {}", stdout2);

    // "Upgrade" the tool. Inputs and config are untouched.
    fs::write(&tool, "#!/bin/sh\n# v2\nexit 0\n").unwrap();
    crate::common::make_executable(&tool);

    let build3 = run_rsconstruct_with_env(
        project_path, &["build", "-v"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(build3.status.success(),
        "build after tool upgrade failed: {}", String::from_utf8_lossy(&build3.stderr));
    let stdout3 = String::from_utf8_lossy(&build3.stdout);
    assert!(stdout3.contains("Processing:"),
        "a tool upgrade must invalidate cached results: {}", stdout3);
}

/// `[build] hash_tool_versions = false` restores the old behavior for
/// projects that must not have tool identity in their cache keys.
#[test]
fn hash_tool_versions_false_ignores_tool_upgrade() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    let bin_dir = project_path.join("toolbin");
    fs::create_dir_all(&bin_dir).unwrap();
    let tool = bin_dir.join("faketool2");
    fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    crate::common::make_executable(&tool);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[build]\nhash_tool_versions = false\n\n\
         [processor.script]\ncommand = \"faketool2\"\nsrc_dirs = [\"src\"]\nsrc_extensions = [\".txt\"]\n",
    ).unwrap();
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(project_path.join("src/a.txt"), "content\n").unwrap();

    let path_env = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default());

    let build1 = run_rsconstruct_with_env(
        project_path, &["build", "-v"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(build1.status.success(),
        "first build failed: {}", String::from_utf8_lossy(&build1.stderr));

    fs::write(&tool, "#!/bin/sh\n# v2\nexit 0\n").unwrap();
    crate::common::make_executable(&tool);

    let build2 = run_rsconstruct_with_env(
        project_path, &["build", "-v"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(build2.status.success(),
        "second build failed: {}", String::from_utf8_lossy(&build2.stderr));
    let stdout2 = String::from_utf8_lossy(&build2.stdout);
    assert!(!stdout2.contains("Processing:"),
        "with hash_tool_versions=false a tool upgrade must not invalidate: {}", stdout2);
}

/// Two processors over byte-identical input must not share a cache entry.
///
/// The descriptor key hashes input *content*, not input *path* (see
/// `cache_survives_input_rename`), so the input checksum cannot tell two
/// processors apart when they read the same bytes. What separates them is the
/// processor name, hashed alongside the version, config digest, and input
/// checksum (`CacheKey::descriptor_key`).
///
/// Isolating that name is the whole difficulty here. Giving the two instances
/// different commands would separate them by *config hash* instead — `command`
/// and `args` are checksum fields — and the test would pass even with the
/// processor name removed from the key. So both instances run the identical
/// command over the identical input and differ only in `output_dir`, which is
/// deliberately NOT a checksum field for generators. Config digest, version,
/// and input checksum are then all equal, and the processor name is the only
/// remaining discriminator.
///
/// The command writes its own output path into the file, so a collision is
/// visible as content: a shared key restores the first product's blob into the
/// second product's path, and the recorded path inside it is the wrong one.
#[test]
fn distinct_processors_do_not_share_cache_entries() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // One tool, used by both instances — identical `command` and `args`.
    // The generator contract is `command [args...] <input> <output>`.
    let bin_dir = project_path.join("toolbin");
    fs::create_dir_all(&bin_dir).unwrap();
    let tool = bin_dir.join("stamp");
    fs::write(&tool, "#!/bin/sh\nprintf 'built:%s' \"$2\" > \"$2\"\nexit 0\n").unwrap();
    crate::common::make_executable(&tool);

    // gen_a and gen_b differ ONLY by instance name and output_dir.
    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[build]\nhash_tool_versions = false\n\n",
            "[processor.generator.gen_a]\n",
            "command = \"stamp\"\n",
            "output_dir = \"out/a\"\n",
            "output_extension = \"txt\"\n",
            "batch = false\n",
            "src_extensions = [\".src\"]\n",
            "src_dirs = [\"src\"]\n",
            "\n",
            "[processor.generator.gen_b]\n",
            "command = \"stamp\"\n",
            "output_dir = \"out/b\"\n",
            "output_extension = \"txt\"\n",
            "batch = false\n",
            "src_extensions = [\".src\"]\n",
            "src_dirs = [\"src\"]\n",
        ),
    ).unwrap();

    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(project_path.join("src/input.src"), "identical content\n").unwrap();

    let path_env = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default());

    let build = run_rsconstruct_with_env(
        project_path, &["build", "-v"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(build.status.success(),
        "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let out_a = project_path.join("out/a/input.txt");
    let out_b = project_path.join("out/b/input.txt");
    assert!(out_a.exists(), "gen_a produced no output");
    assert!(out_b.exists(), "gen_b produced no output");

    // Each file records the path it was generated for. If the two products
    // shared a descriptor key, one of these carries the other's path.
    assert_eq!(fs::read_to_string(&out_a).unwrap(), "built:out/a/input.txt",
        "gen_a's output came from another processor's cache entry");
    assert_eq!(fs::read_to_string(&out_b).unwrap(), "built:out/b/input.txt",
        "gen_b's output came from another processor's cache entry");

    // Restore path: delete both outputs and rebuild from cache. This is where
    // a shared key does its damage — the blob is content-addressed and
    // path-free, so the wrong bytes land in the right path and the build
    // still reports success.
    fs::remove_file(&out_a).unwrap();
    fs::remove_file(&out_b).unwrap();

    let build2 = run_rsconstruct_with_env(
        project_path, &["build", "-v"],
        &[("NO_COLOR", "1"), ("PATH", &path_env)],
    );
    assert!(build2.status.success(),
        "rebuild failed: {}", String::from_utf8_lossy(&build2.stderr));

    let stdout2 = String::from_utf8_lossy(&build2.stdout);
    assert!(stdout2.contains("Restored from cache:"),
        "second build should restore from cache, not rebuild: {}", stdout2);

    assert_eq!(fs::read_to_string(&out_a).unwrap(), "built:out/a/input.txt",
        "gen_a restored the wrong processor's cached blob");
    assert_eq!(fs::read_to_string(&out_b).unwrap(), "built:out/b/input.txt",
        "gen_b restored the wrong processor's cached blob");
}
