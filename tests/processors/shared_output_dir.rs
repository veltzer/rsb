//! Tests for the "shared output directory" scenario.
//!
//! A Creator processor (e.g. mkdocs) writes many files into a directory (_site/),
//! while another processor (e.g. pandoc modelled here with `explicit`) writes a
//! specific file into the same directory. The Creator's cache must NOT claim
//! ownership of files declared as outputs by other products — so that restore
//! never clobbers another processor's output.

use std::fs;
use tempfile::TempDir;
use crate::common::run_rsconstruct_with_env;

/// Set up a project with:
///   - `explicit.pandoc` owns `_site/about.html`
///   - `creator.mkdocs`  owns the `_site/` directory (writes index.html + assets/style.css)
///
/// Both processors contribute to the shared `_site/` folder.
fn setup_shared_site_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // ---- mkdocs: a creator that builds the whole site, except about.html ----
    let mkdocs_script = project_path.join("mkdocs.sh");
    fs::write(&mkdocs_script, concat!(
        "#!/bin/bash\n",
        "set -e\n",
        "mkdir -p _site/assets\n",
        "echo 'mkdocs-index' > _site/index.html\n",
        "echo 'mkdocs-css'   > _site/assets/style.css\n",
        // Intentionally DO NOT create about.html here; pandoc owns it.
    )).unwrap();

    // ---- pandoc-like explicit: produces a specific file inside _site/ ----
    let pandoc_script = project_path.join("pandoc.sh");
    fs::write(&pandoc_script, concat!(
        "#!/bin/bash\n",
        "set -e\n",
        "mkdir -p _site\n",
        "echo 'pandoc-about' > _site/about.html\n",
    )).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mkdocs_script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&pandoc_script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Anchor files the processors scan for
    fs::write(project_path.join("site.manifest"), "mkdocs\n").unwrap();
    fs::write(project_path.join("about.manifest"), "pandoc\n").unwrap();

    // Configure both processors targeting _site/
    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.creator.mkdocs]\n",
            "command = \"./mkdocs.sh\"\n",
            "src_extensions = [\"site.manifest\"]\n",
            "src_dirs = [\".\"]\n",
            "output_dirs = [\"_site\"]\n",
            "\n",
            "[processor.explicit.pandoc]\n",
            "command = \"./pandoc.sh\"\n",
            "inputs = [\"about.manifest\"]\n",
            "output_files = [\"_site/about.html\"]\n",
            "src_dirs = [\".\"]\n",
        ),
    ).unwrap();

    temp_dir
}

#[test]
#[cfg(unix)]
fn shared_dir_both_build_successfully() {
    let temp_dir = setup_shared_site_project();
    let project_path = temp_dir.path();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // All three files produced by the two processors should exist.
    assert!(project_path.join("_site/index.html").exists(),
        "mkdocs should have created _site/index.html");
    assert!(project_path.join("_site/assets/style.css").exists(),
        "mkdocs should have created _site/assets/style.css");
    assert!(project_path.join("_site/about.html").exists(),
        "pandoc (explicit) should have created _site/about.html; \
         when the Creator runs after the Generator, it must NOT wipe the shared _site/ dir.");

    assert_eq!(fs::read_to_string(project_path.join("_site/index.html")).unwrap().trim(),
        "mkdocs-index");
    assert_eq!(fs::read_to_string(project_path.join("_site/about.html")).unwrap().trim(),
        "pandoc-about");
}

#[test]
#[cfg(unix)]
fn shared_dir_clean_and_restore_preserves_ownership() {
    let temp_dir = setup_shared_site_project();
    let project_path = temp_dir.path();

    // Initial build
    let build = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success(),
        "Initial build failed: stderr={}", String::from_utf8_lossy(&build.stderr));

    // Clean outputs (preserves cache)
    let clean = run_rsconstruct_with_env(project_path, &["clean", "outputs"], &[("NO_COLOR", "1")]);
    assert!(clean.status.success(),
        "Clean failed: stderr={}", String::from_utf8_lossy(&clean.stderr));
    assert!(!project_path.join("_site").exists(), "_site should be gone after clean");

    // Rebuild — should restore from cache
    let restore = run_rsconstruct_with_env(
        project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(restore.status.success(),
        "Restore build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr));

    // All three files should be back, regardless of restore order.
    assert!(project_path.join("_site/index.html").exists(),
        "mkdocs file should be restored");
    assert!(project_path.join("_site/assets/style.css").exists(),
        "mkdocs nested file should be restored");
    assert!(project_path.join("_site/about.html").exists(),
        "pandoc file should be restored");
    assert_eq!(fs::read_to_string(project_path.join("_site/about.html")).unwrap().trim(),
        "pandoc-about",
        "about.html must still have pandoc's content — the Creator must not have claimed it");
}

/// Regression test for the core invariant: a Creator's tree descriptor must
/// not include files declared as outputs of other products.
///
/// We simulate this by: running the build, then deleting the explicit (pandoc)
/// output and blowing away pandoc's cache entry. Now `_site/about.html` is
/// gone. We then do `clean outputs` + `build` — the Creator restore must
/// NOT recreate about.html (because it's not in mkdocs's tree), and the
/// explicit processor must re-run to produce it.
#[test]
#[cfg(unix)]
fn creator_tree_does_not_include_foreign_outputs() {
    let temp_dir = setup_shared_site_project();
    let project_path = temp_dir.path();

    // Initial build — caches both processors
    let build = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success(),
        "Initial build failed: stderr={}", String::from_utf8_lossy(&build.stderr));

    // Clean only the cache for the explicit processor's product by wiping
    // .rsconstruct (simplest way — forces all processors to rebuild their work)
    // and the _site directory. This is a round-trip through cache restore for mkdocs.
    // More targeted: do "clean outputs" and then verify restore works.
    let clean = run_rsconstruct_with_env(project_path, &["clean", "outputs"], &[("NO_COLOR", "1")]);
    assert!(clean.status.success());

    // Corrupt pandoc's about.html by making mkdocs restore happen WITHOUT pandoc
    // ever running. We can't easily disable pandoc via CLI, so we rely on:
    // if the creator tree wrongly included about.html, then restoring mkdocs's
    // cache alone would recreate about.html. The actual test is:
    //
    //   1. restore mkdocs from cache (via a build that selects only mkdocs)
    //   2. check that _site/about.html was NOT restored (because it belongs to pandoc)
    let restore_mkdocs_only = run_rsconstruct_with_env(
        project_path,
        &["build", "-p", "creator.mkdocs", "--verbose"],
        &[("NO_COLOR", "1")],
    );
    assert!(restore_mkdocs_only.status.success(),
        "Partial build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&restore_mkdocs_only.stdout),
        String::from_utf8_lossy(&restore_mkdocs_only.stderr));

    // mkdocs's own files must be back.
    assert!(project_path.join("_site/index.html").exists(),
        "mkdocs's index.html should be restored");
    assert!(project_path.join("_site/assets/style.css").exists(),
        "mkdocs's style.css should be restored");

    // The critical invariant: about.html is NOT in mkdocs's tree, so it
    // must NOT have been restored when only mkdocs ran.
    assert!(!project_path.join("_site/about.html").exists(),
        "_site/about.html must NOT be restored by the Creator alone \
         — it is owned by the explicit.pandoc processor and must not \
         appear in the Creator's tree descriptor.");
}

/// Ownership is exclusive: if two processors declare the same literal output path,
/// the graph-build phase MUST fail with an "Output conflict" error. Without this
/// guarantee the shared-folder logic would be unsound (two "owners" of one path).
#[test]
#[cfg(unix)]
fn two_processors_declaring_same_output_file_errors() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Two explicit processors, both claiming _site/index.html as an output.
    fs::write(project_path.join("a.manifest"), "a\n").unwrap();
    fs::write(project_path.join("b.manifest"), "b\n").unwrap();

    let script = project_path.join("touch.sh");
    fs::write(&script, "#!/bin/bash\ntouch \"$1\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fs::write(
        project_path.join("rsconstruct.toml"),
        concat!(
            "[processor.explicit.a]\n",
            "command = \"./touch.sh\"\n",
            "args = [\"_site/index.html\"]\n",
            "inputs = [\"a.manifest\"]\n",
            "output_files = [\"_site/index.html\"]\n",
            "src_dirs = [\".\"]\n",
            "\n",
            "[processor.explicit.b]\n",
            "command = \"./touch.sh\"\n",
            "args = [\"_site/index.html\"]\n",
            "inputs = [\"b.manifest\"]\n",
            "output_files = [\"_site/index.html\"]\n",
            "src_dirs = [\".\"]\n",
        ),
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build must fail when two products declare the same output path. \
         stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Error must mention the conflict. The exact classified exit code is GraphError (4).
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.to_lowercase().contains("output conflict"),
        "Expected an 'Output conflict' error in the output, got: {}",
        combined,
    );
    assert_eq!(output.status.code(), Some(4),
        "Expected GraphError exit code (4), got {:?}. Output: {}",
        output.status.code(), combined);
}
