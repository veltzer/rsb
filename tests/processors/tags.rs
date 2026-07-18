use std::fs;
use crate::common::run_rsconstruct_with_env;
use tempfile::TempDir;

/// Helper: create a tags test project with given .md files and optional tags directory.
/// `tag_lists` is a slice of (filename, content) pairs for files in tags/.
fn setup_tags_project(md_files: &[(&str, &str)], tag_lists: &[(&str, &str)]) -> TempDir {
    setup_tags_project_with_config(md_files, tag_lists, "[processor.tags]\nsrc_dirs = [\".\"]\n")
}

fn setup_tags_project_with_config(md_files: &[(&str, &str)], tag_lists: &[(&str, &str)], config: &str) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();

    fs::write(p.join("rsconstruct.toml"), config).unwrap();

    for (name, content) in md_files {
        if let Some(parent) = std::path::Path::new(name).parent()
            && !parent.as_os_str().is_empty() {
            fs::create_dir_all(p.join(parent)).unwrap();
        }
        fs::write(p.join(name), content).unwrap();
    }

    if !tag_lists.is_empty() {
        let tag_dir = p.join("tags");
        fs::create_dir_all(&tag_dir).unwrap();
        for (name, content) in tag_lists {
            fs::write(tag_dir.join(name), content).unwrap();
        }
    }

    temp_dir
}

/// Helper: build the project and assert success.
fn build_project(p: &std::path::Path) {
    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "build failed: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn tags_basic_build_and_query() {
    // Use category:value format in frontmatter lists, matching real-world usage
    let temp_dir = setup_tags_project(
        &[
            ("course1.md", "---\nlevel: beginner\ntags:\n  - tools:python\n  - tools:docker\n---\n# Course 1\n"),
            ("course2.md", "---\nlevel: advanced\ntags:\n  - tools:rust\n  - tools:docker\n---\n# Course 2\n"),
        ],
        &[
            ("level.txt", "beginner\nadvanced\n"),
            ("tools.txt", "python\nrust\ndocker\n"),
        ],
    );
    let p = temp_dir.path();

    // Build should succeed and create the tags database
    build_project(p);
    assert!(p.join("out/tags/tags.db").exists(), "tags database should be created");

    // `rsconstruct tags list` should show all tags sorted
    let list_output = run_rsconstruct_with_env(p, &["tags", "list"], &[("NO_COLOR", "1")]);
    assert!(list_output.status.success());
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    let tags: Vec<&str> = stdout.lines().collect();
    assert!(tags.contains(&"tools:docker"));
    assert!(tags.contains(&"tools:python"));
    assert!(tags.contains(&"tools:rust"));
    assert!(tags.contains(&"level:beginner"));
    assert!(tags.contains(&"level:advanced"));

    // `rsconstruct tags files tools:docker` should return both files
    let files_output = run_rsconstruct_with_env(p, &["tags", "files", "tools:docker"], &[("NO_COLOR", "1")]);
    assert!(files_output.status.success());
    let files_stdout = String::from_utf8_lossy(&files_output.stdout);
    assert!(files_stdout.contains("course1.md"));
    assert!(files_stdout.contains("course2.md"));

    // `rsconstruct tags files tools:docker tools:rust` (AND) should return only course2
    let and_output = run_rsconstruct_with_env(p, &["tags", "files", "tools:docker", "tools:rust"], &[("NO_COLOR", "1")]);
    assert!(and_output.status.success());
    let and_stdout = String::from_utf8_lossy(&and_output.stdout);
    assert!(!and_stdout.contains("course1.md"));
    assert!(and_stdout.contains("course2.md"));

    // `rsconstruct tags files --or tools:python tools:rust` (OR) should return both files
    let or_output = run_rsconstruct_with_env(p, &["tags", "files", "--or", "tools:python", "tools:rust"], &[("NO_COLOR", "1")]);
    assert!(or_output.status.success());
    let or_stdout = String::from_utf8_lossy(&or_output.stdout);
    assert!(or_stdout.contains("course1.md"));
    assert!(or_stdout.contains("course2.md"));
}

#[test]
fn tags_validation_rejects_unknown_tags() {
    let temp_dir = setup_tags_project(
        &[
            ("course.md", "---\nlevel: beginner\ntags:\n  - tools:python\n  - tools:dockker\n---\n# Course\n"),
        ],
        &[
            ("tools.txt", "python\ndocker\n"),
            ("level.txt", "beginner\n"),
        ],
    );
    let p = temp_dir.path();

    // Build should fail because "tools:dockker" is not in tag_lists
    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with unknown tag");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("dockker"), "error should mention the unknown tag: {}", stderr);
    // Should suggest "tools:docker" as a typo correction
    assert!(stderr.contains("docker"), "error should suggest 'docker': {}", stderr);
}

#[test]
fn tags_for_file_path_matching() {
    let temp_dir = setup_tags_project(
        &[
            ("sub/foo.md", "---\ntags:\n  - concepts:alpha\n---\n# Foo\n"),
            ("sub/barfoo.md", "---\ntags:\n  - concepts:beta\n---\n# Barfoo\n"),
        ],
        &[
            ("concepts.txt", "alpha\nbeta\n"),
        ],
    );
    let p = temp_dir.path();

    build_project(p);

    // Querying for "sub/foo.md" should return alpha, NOT beta
    let for_file = run_rsconstruct_with_env(p, &["tags", "for-file", "sub/foo.md"], &[("NO_COLOR", "1")]);
    assert!(for_file.status.success());
    let stdout = String::from_utf8_lossy(&for_file.stdout);
    assert!(stdout.contains("alpha"), "should find tag 'alpha' for sub/foo.md: {}", stdout);
    assert!(!stdout.contains("beta"), "should NOT match barfoo.md's tag 'beta': {}", stdout);
}

#[test]
fn tags_count_and_tree() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\nlevel: beginner\ntags:\n  - tools:python\n  - tools:docker\n---\n"),
            ("b.md", "---\nlevel: advanced\ntags:\n  - tools:docker\n---\n"),
        ],
        &[
            ("level.txt", "beginner\nadvanced\n"),
            ("tools.txt", "python\ndocker\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    // Count should show docker with count 2
    let count = run_rsconstruct_with_env(p, &["tags", "count"], &[("NO_COLOR", "1")]);
    assert!(count.status.success());
    let stdout = String::from_utf8_lossy(&count.stdout);
    assert!(stdout.contains("docker"), "count should list docker: {}", stdout);

    // Tree should group level= tags
    let tree = run_rsconstruct_with_env(p, &["tags", "tree"], &[("NO_COLOR", "1")]);
    assert!(tree.status.success());
    let tree_stdout = String::from_utf8_lossy(&tree.stdout);
    assert!(tree_stdout.contains("level="), "tree should show level= group: {}", tree_stdout);
    assert!(tree_stdout.contains("beginner"), "tree should show beginner value: {}", tree_stdout);
    assert!(tree_stdout.contains("advanced"), "tree should show advanced value: {}", tree_stdout);
}

#[test]
fn tags_stats() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\nlevel: beginner\ntags:\n  - tools:python\n---\n"),
            ("b.md", "---\ntags:\n  - tools:docker\n---\n"),
        ],
        &[
            ("level.txt", "beginner\n"),
            ("tools.txt", "python\ndocker\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    let stats = run_rsconstruct_with_env(p, &["tags", "stats"], &[("NO_COLOR", "1")]);
    assert!(stats.status.success());
    let stdout = String::from_utf8_lossy(&stats.stdout);
    assert!(stdout.contains("Files indexed:"), "stats should show file count: {}", stdout);
    assert!(stdout.contains("2"), "should index 2 files: {}", stdout);
    assert!(stdout.contains("Unique tags:"), "stats should show unique tags: {}", stdout);
}

#[test]
fn tags_grep() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags:\n  - tools:python\n  - tools:python-advanced\n  - tools:docker\n---\n"),
        ],
        &[
            ("tools.txt", "python\npython-advanced\ndocker\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    // Grep for "python" should match both python tags but not docker
    let grep = run_rsconstruct_with_env(p, &["tags", "grep", "python"], &[("NO_COLOR", "1")]);
    assert!(grep.status.success());
    let stdout = String::from_utf8_lossy(&grep.stdout);
    assert!(stdout.contains("python"), "grep should find 'python': {}", stdout);
    assert!(stdout.contains("python-advanced"), "grep should find 'python-advanced': {}", stdout);
    assert!(!stdout.contains("docker"), "grep should NOT find 'docker': {}", stdout);

    // Case-insensitive grep
    let grep_i = run_rsconstruct_with_env(p, &["tags", "grep", "-i", "PYTHON"], &[("NO_COLOR", "1")]);
    assert!(grep_i.status.success());
    let stdout_i = String::from_utf8_lossy(&grep_i.stdout);
    assert!(stdout_i.contains("python"), "case-insensitive grep should find 'python': {}", stdout_i);
}

#[test]
fn tags_frontmatter() {
    let temp_dir = setup_tags_project(
        &[
            ("course.md", "---\ntitle: My Course\nlevel: beginner\ntags:\n  - tools:python\n---\n# Content\n"),
        ],
        &[
            ("level.txt", "beginner\n"),
            ("tools.txt", "python\n"),
            ("title.txt", "My Course\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    let fm = run_rsconstruct_with_env(p, &["tags", "frontmatter", "course.md"], &[("NO_COLOR", "1")]);
    assert!(fm.status.success());
    let stdout = String::from_utf8_lossy(&fm.stdout);
    assert!(stdout.contains("title"), "frontmatter should show title: {}", stdout);
    assert!(stdout.contains("My Course"), "frontmatter should show title value: {}", stdout);
    assert!(stdout.contains("level"), "frontmatter should show level: {}", stdout);
}

#[test]
fn tags_unused_strict_fails() {
    // Build should fail when tag_lists contains unused tags and check_unused is enabled
    let temp_dir = setup_tags_project_with_config(
        &[
            ("a.md", "---\ntags:\n  - tools:active\n---\n"),
        ],
        &[
            ("tools.txt", "active\nobsolete\n"),
        ],
        "[processor.tags]\ncheck_unused = true\nsrc_dirs = [\".\"]\n",
    );
    let p = temp_dir.path();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with unused tags");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("obsolete"), "should report 'obsolete' as unused: {}", stderr);
}

#[test]
fn tags_validate_standalone() {
    // Build with all tags allowed, then remove an allowlist entry and validate
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags:\n  - tools:python\n  - tools:dockker\n---\n"),
        ],
        &[
            ("tools.txt", "python\ndockker\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    // Now update tag_lists to not include "dockker" but include "docker"
    fs::write(p.join("tags/tools.txt"), "python\ndocker\n").unwrap();

    // validate should fail and suggest the correct tag
    let validate = run_rsconstruct_with_env(p, &["tags", "validate"], &[("NO_COLOR", "1")]);
    assert!(!validate.status.success(), "validate should fail with unknown tags");
    let stderr = String::from_utf8_lossy(&validate.stderr);
    assert!(stderr.contains("dockker"), "should mention unknown tag: {}", stderr);
    assert!(stderr.contains("docker"), "should suggest correction: {}", stderr);
}

#[test]
fn tags_inline_yaml_list() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags: [tools:alpha, tools:beta, tools:gamma]\nlevel: beginner\n---\n# Content\n"),
        ],
        &[
            ("tools.txt", "alpha\nbeta\ngamma\n"),
            ("level.txt", "beginner\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    let list = run_rsconstruct_with_env(p, &["tags", "list"], &[("NO_COLOR", "1")]);
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("alpha"), "should parse inline list item 'alpha': {}", stdout);
    assert!(stdout.contains("beta"), "should parse inline list item 'beta': {}", stdout);
    assert!(stdout.contains("gamma"), "should parse inline list item 'gamma': {}", stdout);
}

#[test]
fn tags_colon_in_yaml_value() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\nurl: https://example.com/path\ntime: 10:30\ntags:\n  - tools:web\n---\n"),
        ],
        &[
            ("tools.txt", "web\n"),
            ("url.txt", "https://example.com/path\n"),
            ("time.txt", "10:30\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    let fm = run_rsconstruct_with_env(p, &["tags", "frontmatter", "a.md"], &[("NO_COLOR", "1")]);
    assert!(fm.status.success());
    let stdout = String::from_utf8_lossy(&fm.stdout);
    assert!(stdout.contains("https://example.com/path"), "URL value should be preserved: {}", stdout);
    assert!(stdout.contains("10:30"), "time value should be preserved: {}", stdout);

    // Also check tags list for key=value indexing
    let list = run_rsconstruct_with_env(p, &["tags", "list"], &[("NO_COLOR", "1")]);
    assert!(list.status.success());
    let list_stdout = String::from_utf8_lossy(&list.stdout);
    assert!(list_stdout.contains("url:https://example.com/path"), "URL should be indexed correctly: {}", list_stdout);
    assert!(list_stdout.contains("time:10:30"), "time should be indexed correctly: {}", list_stdout);
}

#[test]
fn tags_numeric_and_boolean_values() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ndifficulty: 3\npublished: true\ntags:\n  - tools:test\n---\n"),
        ],
        &[
            ("tools.txt", "test\n"),
            ("difficulty.txt", "3\n"),
            ("published.txt", "true\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    let list = run_rsconstruct_with_env(p, &["tags", "list"], &[("NO_COLOR", "1")]);
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("difficulty:3"), "numeric value should be indexed: {}", stdout);
    assert!(stdout.contains("published:true"), "boolean value should be indexed: {}", stdout);
}

#[test]
fn tags_stale_entries_cleared_on_rebuild() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags:\n  - tools:alpha\n  - tools:beta\n---\n"),
        ],
        &[
            ("tools.txt", "alpha\nbeta\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    // Verify both tags exist
    let list1 = run_rsconstruct_with_env(p, &["tags", "list"], &[("NO_COLOR", "1")]);
    let stdout1 = String::from_utf8_lossy(&list1.stdout);
    assert!(stdout1.contains("alpha"));
    assert!(stdout1.contains("beta"));

    // Remove "beta" tag from the file and tag_lists, then force rebuild
    fs::write(p.join("a.md"), "---\ntags:\n  - tools:alpha\n---\n").unwrap();
    fs::write(p.join("tags/tools.txt"), "alpha\n").unwrap();
    let rebuild = run_rsconstruct_with_env(p, &["build", "--force"], &[("NO_COLOR", "1")]);
    assert!(rebuild.status.success(), "rebuild failed: {}", String::from_utf8_lossy(&rebuild.stderr));

    // "beta" should no longer appear
    let list2 = run_rsconstruct_with_env(p, &["tags", "list"], &[("NO_COLOR", "1")]);
    let stdout2 = String::from_utf8_lossy(&list2.stdout);
    assert!(stdout2.contains("alpha"), "alpha should still exist: {}", stdout2);
    assert!(!stdout2.contains("beta"), "beta should be gone after rebuild: {}", stdout2);
}

#[test]
fn tags_empty_inline_list() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags: []\nlevel: beginner\n---\n# Content\n"),
        ],
        &[
            ("level.txt", "beginner\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);

    // The empty list should not produce any bare tags (no phantom empty-string tag)
    let list = run_rsconstruct_with_env(p, &["tags", "list"], &[("NO_COLOR", "1")]);
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    // Should only have level:beginner, no empty tags
    assert!(stdout.contains("level:beginner"), "should have level:beginner: {}", stdout);
    let tags: Vec<&str> = stdout.lines().collect();
    assert_eq!(tags.len(), 1, "should have exactly 1 tag (level:beginner), got: {:?}", tags);
}

#[test]
fn tags_duplicate_within_file_fails() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags:\n  - tools:docker\n  - tools:docker\n---\n"),
        ],
        &[
            ("tools.txt", "docker\n"),
        ],
    );
    let p = temp_dir.path();

    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with duplicate tags");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Duplicate"), "should mention duplicate: {}", stderr);
    assert!(stderr.contains("docker"), "should mention the duplicate tag: {}", stderr);
}

#[test]
fn tags_duplicate_across_tag_lists_different_categories_ok() {
    // tools:docker and infra:docker are different tags — no conflict
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags:\n  - tools:docker\n  - infra:docker\n---\n"),
        ],
        &[
            ("tools.txt", "docker\n"),
            ("infra.txt", "docker\n"),
        ],
    );
    let p = temp_dir.path();
    build_project(p);
}

#[test]
fn tags_required_fields_pass() {
    let config = r#"
[processor.tags]
required_fields = ["level", "tags"]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[
            ("a.md", "---\nlevel: beginner\ntags:\n  - tools:docker\n---\n"),
        ],
        &[
            ("level.txt", "beginner\n"),
            ("tools.txt", "docker\n"),
        ],
        config,
    );
    build_project(temp_dir.path());
}

#[test]
fn tags_required_fields_missing_fails() {
    let config = r#"
[processor.tags]
required_fields = ["level", "tags", "category"]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[
            ("a.md", "---\nlevel: beginner\ntags:\n  - tools:docker\n---\n"),
        ],
        &[
            ("level.txt", "beginner\n"),
            ("tools.txt", "docker\n"),
        ],
        config,
    );
    let p = temp_dir.path();
    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with missing required field");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("category"), "should mention missing field 'category': {}", stderr);
    assert!(stderr.contains("a.md"), "should mention the file: {}", stderr);
}

#[test]
fn tags_required_fields_empty_list_fails() {
    let config = r#"
[processor.tags]
required_fields = ["tags"]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[
            ("a.md", "---\ntags: []\nlevel: beginner\n---\n"),
        ],
        &[
            ("level.txt", "beginner\n"),
        ],
        config,
    );
    let p = temp_dir.path();
    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail when required list field is empty");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tags"), "should mention missing field 'tags': {}", stderr);
}

#[test]
fn tags_required_fields_no_frontmatter_fails() {
    let config = r#"
[processor.tags]
required_fields = ["level"]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[
            ("a.md", "# No frontmatter at all\nJust content.\n"),
        ],
        &[
            ("level.txt", "beginner\n"),
        ],
        config,
    );
    let p = temp_dir.path();
    let output = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail when file has no frontmatter");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("level"), "should mention missing field 'level': {}", stderr);
}

// --- required_field_groups ---

#[test]
fn tags_required_field_groups_first_group() {
    let config = r#"
[processor.tags]
required_field_groups = [["duration_hours"], ["duration_hours_long", "duration_hours_short"]]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\nduration_hours: 24\ntags:\n  - tools:docker\n---\n")],
        &[("tools.txt", "docker\n"), ("duration_hours.txt", "24\n")],
        config,
    );
    build_project(temp_dir.path());
}

#[test]
fn tags_required_field_groups_second_group() {
    let config = r#"
[processor.tags]
required_field_groups = [["duration_hours"], ["duration_hours_long", "duration_hours_short"]]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\nduration_hours_long: 40\nduration_hours_short: 16\ntags:\n  - tools:docker\n---\n")],
        &[("tools.txt", "docker\n"), ("duration_hours_long.txt", "40\n"), ("duration_hours_short.txt", "16\n")],
        config,
    );
    build_project(temp_dir.path());
}

#[test]
fn tags_required_field_groups_none_satisfied_fails() {
    let config = r#"
[processor.tags]
required_field_groups = [["duration_hours"], ["duration_hours_long", "duration_hours_short"]]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\ntags:\n  - tools:docker\n---\n")],
        &[("tools.txt", "docker\n")],
        config,
    );
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail when no group is satisfied");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("duration_hours"), "should mention the groups: {}", stderr);
}

#[test]
fn tags_required_field_groups_partial_second_fails() {
    // Only has duration_hours_long but not duration_hours_short — neither group satisfied
    let config = r#"
[processor.tags]
required_field_groups = [["duration_hours"], ["duration_hours_long", "duration_hours_short"]]
src_dirs = ["."]
"#;
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\nduration_hours_long: 40\ntags:\n  - tools:docker\n---\n")],
        &[("tools.txt", "docker\n"), ("duration_hours_long.txt", "40\n")],
        config,
    );
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with partial group");
}

// --- Feature 1: required_values ---

#[test]
fn tags_required_values_pass() {
    let config = "[processor.tags]\nrequired_values = [\"level\"]\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\nlevel: beginner\ntags:\n  - tools:docker\n---\n")],
        &[("level.txt", "beginner\n"), ("tools.txt", "docker\n")],
        config,
    );
    build_project(temp_dir.path());
}

#[test]
fn tags_required_values_invalid_fails() {
    let config = "[processor.tags]\nrequired_values = [\"level\"]\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\nlevel: begginer\ntags:\n  - tools:docker\n---\n")],
        &[("level.txt", "beginner\nadvanced\n"), ("tools.txt", "docker\n")],
        config,
    );
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with invalid value");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("begginer"), "should mention invalid value: {}", stderr);
}

// --- Feature 2: unique_fields ---

#[test]
fn tags_unique_fields_pass() {
    let config = "[processor.tags]\nunique_fields = [\"title\"]\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[
            ("a.md", "---\ntitle: Course A\ntags:\n  - tools:docker\n---\n"),
            ("b.md", "---\ntitle: Course B\ntags:\n  - tools:rust\n---\n"),
        ],
        &[("tools.txt", "docker\nrust\n"), ("title.txt", "Course A\nCourse B\n")],
        config,
    );
    build_project(temp_dir.path());
}

#[test]
fn tags_unique_fields_duplicate_fails() {
    let config = "[processor.tags]\nunique_fields = [\"title\"]\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[
            ("a.md", "---\ntitle: Same Title\ntags:\n  - tools:docker\n---\n"),
            ("b.md", "---\ntitle: Same Title\ntags:\n  - tools:rust\n---\n"),
        ],
        &[("tools.txt", "docker\nrust\n"), ("title.txt", "Same Title\n")],
        config,
    );
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with duplicate title");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Same Title"), "should mention the duplicate value: {}", stderr);
}

// --- Feature 3: field_types ---

#[test]
fn tags_field_types_pass() {
    let config = "[processor.tags]\nsrc_dirs = [\".\"]\n[processor.tags.field_types]\ntags = \"list\"\nlevel = \"scalar\"\nduration_hours = \"number\"\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\ntags:\n  - tools:docker\nlevel: beginner\nduration_hours: 24\n---\n")],
        &[("tools.txt", "docker\n"), ("level.txt", "beginner\n"), ("duration_hours.txt", "24\n")],
        config,
    );
    build_project(temp_dir.path());
}

#[test]
fn tags_field_types_wrong_type_fails() {
    let config = "[processor.tags]\nsrc_dirs = [\".\"]\n[processor.tags.field_types]\nlevel = \"list\"\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\nlevel: beginner\ntags:\n  - tools:docker\n---\n")],
        &[("tools.txt", "docker\n"), ("level.txt", "beginner\n")],
        config,
    );
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with wrong field type");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("level"), "should mention the field: {}", stderr);
    assert!(stderr.contains("list"), "should mention expected type: {}", stderr);
}

// --- Feature 9: sorted_tags ---

#[test]
fn tags_sorted_tags_pass() {
    let config = "[processor.tags]\nsorted_tags = true\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\ntags:\n  - tools:alpha\n  - tools:beta\n---\n")],
        &[("tools.txt", "alpha\nbeta\n")],
        config,
    );
    build_project(temp_dir.path());
}

#[test]
fn tags_sorted_tags_unsorted_fails() {
    let config = "[processor.tags]\nsorted_tags = true\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\ntags:\n  - tools:beta\n  - tools:alpha\n---\n")],
        &[("tools.txt", "alpha\nbeta\n")],
        config,
    );
    let output = run_rsconstruct_with_env(temp_dir.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build should fail with unsorted tags");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sorted"), "should mention sorting: {}", stderr);
}

// --- Feature 4, 5, 6: matrix, coverage, orphans ---

#[test]
fn tags_matrix_shows_categories() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\nlevel: beginner\ntags:\n  - tools:docker\n---\n"),
            ("b.md", "---\ntags:\n  - tools:rust\n---\n"),
        ],
        &[("level.txt", "beginner\n"), ("tools.txt", "docker\nrust\n")],
    );
    build_project(temp_dir.path());

    let output = run_rsconstruct_with_env(temp_dir.path(), &["tags", "matrix"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("level"), "matrix should show level category: {}", stdout);
    assert!(stdout.contains("tools"), "matrix should show tools category: {}", stdout);
}

#[test]
fn tags_coverage_shows_percentages() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\nlevel: beginner\ntags:\n  - tools:docker\n---\n"),
            ("b.md", "---\ntags:\n  - tools:rust\n---\n"),
        ],
        &[("level.txt", "beginner\n"), ("tools.txt", "docker\nrust\n")],
    );
    build_project(temp_dir.path());

    let output = run_rsconstruct_with_env(temp_dir.path(), &["tags", "coverage"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tools"), "coverage should show tools: {}", stdout);
    assert!(stdout.contains("100%"), "tools should be 100%: {}", stdout);
    assert!(stdout.contains("50%"), "level should be 50%: {}", stdout);
}

#[test]
fn tags_orphans_no_orphans() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags:\n  - tools:docker\n---\n"),
            ("b.md", "---\ntags:\n  - tools:rust\n---\n"),
        ],
        &[("tools.txt", "docker\nrust\n")],
    );
    build_project(temp_dir.path());

    let output = run_rsconstruct_with_env(temp_dir.path(), &["tags", "orphans"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("All files have tags"), "should report no orphans: {}", stdout);
}

// --- Feature 8: suggest ---

#[test]
fn tags_suggest_shows_suggestions() {
    let temp_dir = setup_tags_project(
        &[
            ("a.md", "---\ntags:\n  - tools:docker\n  - tools:python\nlevel: beginner\n---\n"),
            ("b.md", "---\ntags:\n  - tools:docker\n  - tools:rust\nlevel: advanced\n---\n"),
            ("c.md", "---\ntags:\n  - tools:docker\n---\n"),
        ],
        &[("tools.txt", "docker\npython\nrust\n"), ("level.txt", "beginner\nadvanced\n")],
    );
    build_project(temp_dir.path());

    let output = run_rsconstruct_with_env(temp_dir.path(), &["tags", "suggest", "c.md"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // c.md only has docker, so it should suggest tags from similar files (a.md, b.md)
    assert!(stdout.contains("Suggested"), "should show suggestions: {}", stdout);
}

// --- Feature 7: check ---

#[test]
fn tags_check_passes_clean_project() {
    let config = "[processor.tags]\nrequired_fields = [\"tags\"]\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\ntags:\n  - tools:docker\n---\n")],
        &[("tools.txt", "docker\n")],
        config,
    );
    build_project(temp_dir.path());

    let output = run_rsconstruct_with_env(temp_dir.path(), &["tags", "check"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(), "check should pass: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn tags_check_reports_issues() {
    let config = "[processor.tags]\nrequired_fields = [\"level\"]\nsrc_dirs = [\".\"]\n";
    let temp_dir = setup_tags_project_with_config(
        &[("a.md", "---\ntags:\n  - tools:docker\n---\n")],
        &[("tools.txt", "docker\n")],
        config,
    );

    let output = run_rsconstruct_with_env(temp_dir.path(), &["tags", "check"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "check should fail with missing required field");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("level"), "should report missing level: {}", stderr);
}
