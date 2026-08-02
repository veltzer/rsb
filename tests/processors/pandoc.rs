use std::fs;
use tempfile::TempDir;
use crate::common::{run_rsconstruct_with_env, require_tool};

#[test]
fn pandoc_valid_file() {
    require_tool("pandoc");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Use HTML format to avoid requiring LaTeX for PDF
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pandoc]\nformats = [\"html\"]\nsrc_dirs = [\"\"]\n",
    )
    .unwrap();

    fs::write(
        project_path.join("doc.md"),
        "# Hello World\n\nThis is a test document.\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with valid markdown for pandoc: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Processing:"),
        "Should process pandoc: {}",
        stdout
    );
}

#[test]
fn pandoc_incremental_skip() {
    require_tool("pandoc");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pandoc]\nformats = [\"html\"]\nsrc_dirs = [\"\"]\n",
    )
    .unwrap();

    fs::write(
        project_path.join("doc.md"),
        "# Hello World\n\nThis is a test.\n",
    )
    .unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());

    // Second build should skip
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("[pandoc] Skipping (unchanged):"),
        "Second build should skip: {}",
        stdout2
    );
}

// A declared src_dirs entry must exist on disk — naming a directory that
// isn't there is a misconfiguration and must fail loudly. (pandoc has no
// default src_dirs: no processor does, so the directory is always named
// explicitly and is always checked.)
#[test]
fn pandoc_declared_src_dir_must_exist() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pandoc]\nsrc_dirs = [\"pandoc\"]\n",
    )
    .unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build must fail when a declared src_dir doesn't exist");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("pandoc") && combined.contains("does not exist"),
        "Error must name the missing 'pandoc' directory: {}", combined
    );
}

/// Markdown containing characters outside the latin-1 range. pdflatex (the
/// pandoc default PDF engine) cannot typeset these without manual package
/// setup; xelatex and lualatex handle them natively. These tests pin both
/// behaviors so a regression in argument plumbing surfaces immediately.
const UNICODE_MD: &str = "---\ntitle: Unicode\n---\n# שלום λ café\n\nΩμέγα. Bonjour à tous.\n";

/// Default engine (pdflatex) on unicode content must fail.
#[test]
fn pandoc_unicode_fails_with_default_engine() {
    require_tool("pandoc");
    require_tool("pdflatex");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // No pdf_engine set → pandoc uses pdflatex.
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pandoc]\nformats = [\"pdf\"]\nsrc_dirs = [\"\"]\n",
    )
    .unwrap();
    fs::write(project_path.join("doc.md"), UNICODE_MD).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should fail with unicode + default engine, but succeeded. stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !project_path.join("out/pandoc/doc.pdf").exists(),
        "PDF should not be produced when pdflatex chokes on unicode",
    );
}

/// xelatex on unicode content must succeed.
#[test]
fn pandoc_unicode_succeeds_with_xelatex() {
    require_tool("pandoc");
    require_tool("xelatex");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pandoc]\nformats = [\"pdf\"]\nsrc_dirs = [\"\"]\npdf_engine = \"xelatex\"\n",
    )
    .unwrap();
    fs::write(project_path.join("doc.md"), UNICODE_MD).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with xelatex on unicode content. stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let pdf = project_path.join("out/pandoc/doc.pdf");
    assert!(pdf.exists(), "Expected PDF at {}", pdf.display());
    let bytes = fs::read(&pdf).unwrap();
    assert!(bytes.len() > 1000, "PDF unexpectedly small: {} bytes", bytes.len());
    assert!(bytes.starts_with(b"%PDF-"), "File is not a PDF (no %PDF- header)");
}

/// lualatex on unicode content must also succeed.
#[test]
fn pandoc_unicode_succeeds_with_lualatex() {
    require_tool("pandoc");
    require_tool("lualatex");

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pandoc]\nformats = [\"pdf\"]\nsrc_dirs = [\"\"]\npdf_engine = \"lualatex\"\n",
    )
    .unwrap();
    fs::write(project_path.join("doc.md"), UNICODE_MD).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed with lualatex on unicode content. stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let pdf = project_path.join("out/pandoc/doc.pdf");
    assert!(pdf.exists(), "Expected PDF at {}", pdf.display());
    let bytes = fs::read(&pdf).unwrap();
    assert!(bytes.starts_with(b"%PDF-"), "File is not a PDF (no %PDF- header)");
}

/// Unknown engine values are rejected at config-load time, before any tool
/// resolution. Doesn't require pandoc to be installed.
#[test]
fn pandoc_rejects_unknown_pdf_engine() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.pandoc]\nformats = [\"pdf\"]\nsrc_dirs = [\"\"]\npdf_engine = \"bogusengine\"\n",
    )
    .unwrap();
    fs::write(project_path.join("doc.md"), "# hi\n").unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        !output.status.success(),
        "Build should fail when pdf_engine is unknown. stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("pdf_engine") && combined.contains("bogusengine"),
        "Error message should name the bad engine. Got: {}",
        combined,
    );
}

/// Build the same markdown file twice with pandoc and verify the PDF output is binary identical.
/// This tests that our pandoc invocation is deterministic (no embedded timestamps, random IDs, etc.).
#[test]
fn pandoc_pdf_deterministic() {
    require_tool("pandoc");
    require_tool("pdflatex");

    let md_content = "---\ntitle: Determinism Test\n---\n# Hello World\n\nThis is a test.\n\n## Outline\n* Chapter one\n* Chapter two\n";
    let config = "[processor.pandoc]\nformats = [\"pdf\"]\nsrc_dirs = [\"\"]\n";

    // Build 1
    let dir1 = TempDir::new().expect("Failed to create temp dir");
    fs::write(dir1.path().join("rsconstruct.toml"), config).unwrap();
    fs::write(dir1.path().join("doc.md"), md_content).unwrap();
    let out1 = run_rsconstruct_with_env(dir1.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(out1.status.success(), "Build 1 failed: {}", String::from_utf8_lossy(&out1.stderr));

    // Build 2
    let dir2 = TempDir::new().expect("Failed to create temp dir");
    fs::write(dir2.path().join("rsconstruct.toml"), config).unwrap();
    fs::write(dir2.path().join("doc.md"), md_content).unwrap();
    let out2 = run_rsconstruct_with_env(dir2.path(), &["build"], &[("NO_COLOR", "1")]);
    assert!(out2.status.success(), "Build 2 failed: {}", String::from_utf8_lossy(&out2.stderr));

    // Compare PDFs
    let pdf1 = fs::read(dir1.path().join("out/pandoc/doc.pdf")).expect("PDF 1 not found");
    let pdf2 = fs::read(dir2.path().join("out/pandoc/doc.pdf")).expect("PDF 2 not found");
    assert_eq!(pdf1, pdf2, "PDFs should be binary identical but differ ({} vs {} bytes)", pdf1.len(), pdf2.len());
}
