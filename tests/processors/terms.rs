use std::fs;
use crate::common::{setup_test_project, run_rsconstruct_with_env};

fn write_terms_dirs(project_path: &std::path::Path, unambiguous: &[&str], ambiguous: &[&str]) {
    let una_dir = project_path.join("terms.unambiguous");
    let amb_dir = project_path.join("terms.ambiguous");
    fs::create_dir_all(&una_dir).unwrap();
    fs::create_dir_all(&amb_dir).unwrap();
    fs::write(una_dir.join("words.txt"), unambiguous.join("\n") + "\n").unwrap();
    fs::write(amb_dir.join("words.txt"), ambiguous.join("\n") + "\n").unwrap();
}

#[test]
fn terms_two_dirs_disjoint_passes() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &["Kubernetes", "Docker"], &["server", "client"]);

    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nWe deploy on `Kubernetes` with `Docker`.\n",
    ).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with disjoint dirs: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn terms_two_dirs_overlap_fails() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // "Docker" appears in both -> must fail
    write_terms_dirs(project_path, &["Kubernetes", "Docker"], &["Docker", "client"]);

    fs::write(project_path.join("README.md"), "# Doc\n").unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build should fail when terms overlap");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Docker") && combined.contains("ambiguous"),
        "Error should mention overlapping term and ambiguous: {}",
        combined
    );
}

#[test]
fn terms_backticked_ambiguous_fails_by_default() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &["Kubernetes"], &["server"]);

    // Backticked `server` should fail — server is ambiguous, so backticks
    // falsely assert the technical reading.
    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nThe `server` runs on `Kubernetes`.\n",
    ).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Backticked ambiguous term must fail by default");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("server") && combined.contains("ambiguous"),
        "Error must mention 'server' and 'ambiguous': {}",
        combined
    );
}

#[test]
fn terms_backticked_ambiguous_allowed_when_flag_off() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &["Kubernetes"], &["server"]);

    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nThe `server` runs on `Kubernetes`.\n",
    ).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nforbid_backticked_ambiguous = false\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "With forbid_backticked_ambiguous=false, backticked ambiguous term must pass: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn terms_fix_strips_ambiguous_backticks() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &["Kubernetes"], &["server"]);

    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nThe `server` runs on `Kubernetes`.\n",
    ).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["terms", "fix"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "terms fix must succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let after = fs::read_to_string(project_path.join("README.md")).unwrap();
    assert!(
        !after.contains("`server`") && after.contains("`Kubernetes`"),
        "fix must strip `server` backticks but keep `Kubernetes`: {after}",
    );
}

#[test]
fn terms_fix_fails_when_no_terms_found() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Both term directories exist but contain zero terms — a misconfigured
    // setup (e.g. wrong dir_terms_unambiguous) must fail loudly, not
    // silently report success while checking nothing.
    write_terms_dirs(project_path, &[], &[]);

    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nWe deploy on Kubernetes.\n",
    ).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["terms", "fix"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "terms fix must fail when no terms are loaded");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("no terms found") && combined.contains("terms.unambiguous"),
        "Error must say no terms were found and name the directory: {combined}",
    );
}

#[test]
fn terms_build_fails_when_no_terms_found() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &[], &[]);

    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nWe deploy on Kubernetes.\n",
    ).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "build must fail when the terms processor loads zero terms");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("no terms found"),
        "Error must say no terms were found: {combined}",
    );
}

#[test]
fn terms_matching_is_case_sensitive() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    // Single-meaning list has "Docker" with capital D.
    write_terms_dirs(project_path, &["Docker"], &[]);

    // Prose uses lowercase "docker" — must NOT be flagged because the
    // scanner is case-sensitive and the list says "Docker".
    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nWe run docker on every host.\n",
    ).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Lowercase 'docker' should not match list entry 'Docker': stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Now write the term with the canonical casing — must be flagged.
    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nWe run Docker on every host.\n",
    ).unwrap();
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Canonical casing 'Docker' must be flagged");
}

#[test]
fn terms_ambiguous_terms_are_not_flagged() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &["Kubernetes"], &["server"]);

    // "server" is ambiguous - must NOT be required to be backticked.
    // "Kubernetes" is unambiguous and IS backticked - should pass.
    fs::write(
        project_path.join("README.md"),
        "# Doc\n\nThe server runs on `Kubernetes`.\n",
    ).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Ambiguous terms should not be required to be backticked: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn terms_inside_urls_are_ignored() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &["bash", "jq"], &["server"]);

    // The terms appear only inside URLs (a markdown link target and a bare
    // URL). Backticking them there would corrupt the address, so the checker
    // must not ask for it.
    fs::write(
        project_path.join("README.md"),
        "# Doc\n\n\
         * [Bash Manual](https://www.gnu.org/software/bash/manual/)\n\
         * [Manual](https://stedolan.github.io/jq/manual/)\n\
         * https://example.com/bash/jq/index.html\n",
    ).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Terms inside URLs must not be flagged: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn terms_in_prose_still_flagged_alongside_urls() {
    let temp_dir = setup_test_project();
    let project_path = temp_dir.path();

    write_terms_dirs(project_path, &["bash"], &["server"]);

    // Same term in a URL (ignored) and in prose (must still be flagged).
    fs::write(
        project_path.join("README.md"),
        "# Doc\n\n\
         See [manual](https://www.gnu.org/software/bash/manual/) for bash usage.\n",
    ).unwrap();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.terms]\ndir_terms_unambiguous = \"terms.unambiguous\"\ndir_terms_ambiguous = \"terms.ambiguous\"\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Unbackticked term in prose must still fail"
    );
}
