//! Integration tests for `rsconstruct product show <path>`.
//!
//! These build a small tera project (template + glob/git_count_files calls)
//! and then exercise the various display paths: lookup by output, lookup by
//! input, JSON shape, and the descriptor key reflecting analyzer-extended
//! config_hash.

use std::fs;
use tempfile::TempDir;
use crate::common::run_rsconstruct_with_env;

/// `product show` resolves a product by its output path and reports every
/// section: processor, primary input, analyzer-attributed inputs, hash
/// pieces, descriptor key, and cache state.
#[test]
fn product_show_resolves_by_output_and_prints_all_sections() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();

    fs::create_dir_all(p.join("tera.templates")).unwrap();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(p.join("data/b.md"), "b").unwrap();
    fs::write(
        p.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n[analyzer.tera]\n",
    ).unwrap();
    fs::write(
        p.join("tera.templates/report.txt.tera"),
        "Total: {{ glob(pattern=\"data/*.md\") | length }}\n",
    ).unwrap();

    // Prime the deps cache.
    let build = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success(),
        "build failed: {}", String::from_utf8_lossy(&build.stderr));

    // Lookup by output path — the user-visible target.
    let out = run_rsconstruct_with_env(
        p,
        &["product", "show", "report.txt"],
        &[("NO_COLOR", "1")],
    );
    assert!(out.status.success(),
        "product show failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("processor:") && stdout.contains("tera"),
        "missing processor section: {}", stdout);
    assert!(stdout.contains("primary") && stdout.contains("tera.templates/report.txt.tera"),
        "missing primary input: {}", stdout);
    assert!(stdout.contains("hash_pieces:") && stdout.contains("glob") && stdout.contains("data/*.md"),
        "missing hash_pieces section: {}", stdout);
    // Default output collapses *_resolved hash pieces to a count summary.
    assert!(stdout.contains("glob_resolved") && stdout.contains("2 files"),
        "expected collapsed resolved summary: {}", stdout);
    assert!(!stdout.contains("data/a.md") && !stdout.contains("data/b.md"),
        "resolved file list should be hidden without --verbose: {}", stdout);
    assert!(stdout.contains("descriptor_key:") && stdout.contains("cache_state:"),
        "missing descriptor/cache footer: {}", stdout);

    // With --verbose the full resolved list is included.
    let out_v = run_rsconstruct_with_env(
        p,
        &["--verbose", "product", "show", "report.txt"],
        &[("NO_COLOR", "1")],
    );
    assert!(out_v.status.success(),
        "product show --verbose failed: {}", String::from_utf8_lossy(&out_v.stderr));
    let stdout_v = String::from_utf8_lossy(&out_v.stdout);
    assert!(stdout_v.contains("data/a.md") && stdout_v.contains("data/b.md"),
        "expected resolved file list under --verbose: {}", stdout_v);
}

/// Lookup falls back to the primary input path when no product owns the
/// path as output. The template file itself is not an output of any
/// product, but it IS the primary input of one.
#[test]
fn product_show_falls_back_to_primary_input() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();

    fs::create_dir_all(p.join("tera.templates")).unwrap();
    fs::write(
        p.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n[analyzer.tera]\n",
    ).unwrap();
    fs::write(
        p.join("tera.templates/report.txt.tera"),
        "Hello\n",
    ).unwrap();

    let build = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success());

    let out = run_rsconstruct_with_env(
        p,
        &["product", "show", "tera.templates/report.txt.tera"],
        &[("NO_COLOR", "1")],
    );
    assert!(out.status.success(),
        "show by primary input failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("outputs:") && stdout.contains("report.txt"),
        "expected output report.txt: {}", stdout);
}

/// An unknown path produces an actionable error, not an empty/zero output.
/// The error wording mentions both lookup paths so the user knows what to
/// try next.
#[test]
fn product_show_unknown_path_errors() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();

    fs::write(
        p.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n",
    ).unwrap();
    fs::create_dir_all(p.join("tera.templates")).unwrap();
    fs::write(p.join("tera.templates/x.txt.tera"), "x\n").unwrap();

    let out = run_rsconstruct_with_env(
        p,
        &["product", "show", "does/not/exist.txt"],
        &[("NO_COLOR", "1")],
    );
    assert!(!out.status.success(), "unknown path must fail");
    let combined = format!("{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr));
    assert!(combined.contains("No product owns or consumes"),
        "expected actionable error: {}", combined);
}

/// JSON output is a single object with stable field names: `processor`,
/// `outputs`, `inputs.{primary,configured,analyzer}`, `config_hash`,
/// `hash_pieces`, `descriptor_key`, `cache_state`.
#[test]
fn product_show_json_shape() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let p = temp_dir.path();

    fs::create_dir_all(p.join("tera.templates")).unwrap();
    fs::create_dir_all(p.join("data")).unwrap();
    fs::write(p.join("data/a.md"), "a").unwrap();
    fs::write(
        p.join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n[analyzer.tera]\n",
    ).unwrap();
    fs::write(
        p.join("tera.templates/report.txt.tera"),
        "Total: {{ glob(pattern=\"data/*.md\") | length }}\n",
    ).unwrap();

    let build = run_rsconstruct_with_env(p, &["build"], &[("NO_COLOR", "1")]);
    assert!(build.status.success());

    let out = run_rsconstruct_with_env(
        p,
        &["--json", "product", "show", "report.txt"],
        &[("NO_COLOR", "1")],
    );
    assert!(out.status.success(),
        "json product show failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {}\n---\n{}", e, stdout));

    assert_eq!(parsed["processor"], "tera");
    assert_eq!(parsed["outputs"][0], "report.txt");
    assert_eq!(parsed["inputs"]["primary"], "tera.templates/report.txt.tera");
    let tera_pieces = parsed["hash_pieces"]["tera"].as_array()
        .unwrap_or_else(|| panic!("hash_pieces.tera missing or not array: {}", stdout));
    let joined = tera_pieces.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("glob:data/*.md"),
        "expected glob piece: {}", joined);
    assert!(parsed["descriptor_key"].is_string(), "descriptor_key must be a string");
    assert!(parsed["cache_state"].is_string(), "cache_state must be a string");
}
