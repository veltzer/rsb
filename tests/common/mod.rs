#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use serde::Deserialize;

/// Assert that an external tool is available on PATH.
/// Panics if the tool is missing — a missing tool must fail the test,
/// never silently skip it. Only presence is checked (some tools, like
/// pdfunite, have no --version flag); whether the tool actually works
/// is the test body's job.
pub fn require_tool(name: &str) {
    let available = Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    assert!(
        available,
        "required tool '{name}' is not installed — install it; missing tools fail tests, they never skip"
    );
}

/// Helper to create a test project structure (tera processor only)
pub fn setup_test_project() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create directories
    fs::create_dir_all(temp_dir.path().join("tera.templates")).expect("Failed to create tera.templates dir");
    fs::create_dir_all(temp_dir.path().join("config")).expect("Failed to create config dir");

    // Only enable the tera processor so config/*.py files aren't picked up by linters.
    // src_dirs is explicit because no processor defaults to scanning anywhere —
    // a bare [processor.tera] matches no files at all.
    fs::write(
        temp_dir.path().join("rsconstruct.toml"),
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n"
    ).expect("Failed to write rsconstruct.toml");

    temp_dir
}

/// Helper to run rsconstruct command in a directory
pub fn run_rsconstruct(dir: &Path, args: &[&str]) -> std::process::Output {
    let rsconstruct_path = env!("CARGO_BIN_EXE_rsconstruct");
    Command::new(rsconstruct_path)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("Failed to execute rsconstruct")
}

/// Helper to run rsconstruct command with extra environment variables
pub fn run_rsconstruct_with_env(dir: &Path, args: &[&str], env_vars: &[(&str, &str)]) -> std::process::Output {
    let rsconstruct_path = env!("CARGO_BIN_EXE_rsconstruct");
    let mut cmd = Command::new(rsconstruct_path);
    cmd.current_dir(dir).args(args);
    for (key, val) in env_vars {
        cmd.env(key, val);
    }
    cmd.output().expect("Failed to execute rsconstruct")
}

/// Create a temp dir with a rsconstruct.toml containing the given config string.
pub fn setup_project_with_config(config: &str) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    fs::write(temp_dir.path().join("rsconstruct.toml"), config).unwrap();
    temp_dir
}

/// Mark a file executable, so a test can put a script on PATH and have the
/// build actually invoke it. Unconditionally unix (the project targets
/// Linux/macOS only — `#[cfg]` forks are forbidden as untestable dead code).
pub fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

/// Create a file at the given path, creating parent directories as needed.
pub fn write_file(base: &Path, relative: &str, content: &str) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// Helper to set up a C project with the cc processor enabled
pub fn setup_cc_project(project_path: &Path) {
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc_single_file]\nsrc_dirs = [\"src\"]\n[analyzer.icpp]\n"
    ).unwrap();
}

// --- JSON output parsing for tests ---

/// Run rsconstruct with --json flag and return parsed build result
pub fn run_rsconstruct_json(dir: &Path, args: &[&str]) -> BuildResult {
    let mut full_args = vec!["--json"];
    full_args.extend(args);
    let output = run_rsconstruct(dir, &full_args);
    BuildResult::parse(&output)
}

/// Run rsconstruct with --json flag and extra environment variables
pub fn run_rsconstruct_json_with_env(dir: &Path, args: &[&str], env_vars: &[(&str, &str)]) -> BuildResult {
    let mut full_args = vec!["--json"];
    full_args.extend(args);
    let output = run_rsconstruct_with_env(dir, &full_args, env_vars);
    BuildResult::parse(&output)
}

/// JSON event from rsconstruct --json output
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BuildEvent {
    BuildStart {
        total_products: usize,
    },
    ProductComplete {
        product: String,
        processor: String,
        status: String,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        error: Option<String>,
    },
    BuildSummary {
        total: usize,
        success: usize,
        failed: usize,
        skipped: usize,
        restored: usize,
        duration_ms: u64,
        #[serde(default)]
        errors: Vec<String>,
    },
}

/// Parsed build result from rsconstruct --json output
#[derive(Debug, Default)]
pub struct BuildResult {
    pub exit_success: bool,
    pub total_products: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
    pub restored: usize,
    pub duration_ms: u64,
    pub errors: Vec<String>,
    pub products: Vec<ProductResult>,
}

/// Individual product result
#[derive(Debug, Clone)]
pub struct ProductResult {
    pub product: String,
    pub processor: String,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

impl BuildResult {
    /// Parse rsconstruct --json output into structured BuildResult
    pub fn parse(output: &std::process::Output) -> Self {
        let mut result = BuildResult {
            exit_success: output.status.success(),
            ..Default::default()
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<BuildEvent>(line) {
                match event {
                    BuildEvent::BuildStart { total_products } => {
                        result.total_products = total_products;
                    }
                    BuildEvent::ProductComplete { product, processor, status, duration_ms, error } => {
                        result.products.push(ProductResult {
                            product,
                            processor,
                            status,
                            duration_ms,
                            error,
                        });
                    }
                    BuildEvent::BuildSummary { total: _, success, failed, skipped, restored, duration_ms, errors } => {
                        result.success = success;
                        result.failed = failed;
                        result.skipped = skipped;
                        result.restored = restored;
                        result.duration_ms = duration_ms;
                        result.errors = errors;
                    }
                }
            }
        }
        result
    }

    /// Count products with a specific status
    pub fn count_status(&self, status: &str) -> usize {
        self.products.iter().filter(|p| p.status == status).count()
    }

    /// Check if a product with given name was processed with given status
    pub fn has_product(&self, name: &str, status: &str) -> bool {
        self.products.iter().any(|p| p.product.contains(name) && p.status == status)
    }

    /// Get all products with a specific status
    pub fn products_with_status(&self, status: &str) -> Vec<&ProductResult> {
        self.products.iter().filter(|p| p.status == status).collect()
    }
}

/// Generate standard checker processor tests: valid file + incremental skip.
///
/// Usage:
/// ```ignore
/// test_checker!(eslint, tool: "eslint", processor: "eslint",
///     files: [(".eslintrc.json", "{}\n"), ("test.js", "var x = 1;\n")]);
/// ```
///
/// For build tools that don't have simple files to test, use `no_project`:
/// ```ignore
/// test_checker!(cmake, tool: "cmake", processor: "cmake", no_project);
/// ```
macro_rules! test_checker {
    // Full test: valid file + incremental skip
    ($mod_name:ident, tool: $tool:expr, processor: $proc:expr,
     files: [ $( ($fname:expr, $content:expr) ),+ $(,)? ]) => {
        paste::paste! {
            #[test]
            fn [<$mod_name _valid>]() {
                crate::common::require_tool($tool);

                let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
                let project_path = temp_dir.path();

                std::fs::write(
                    project_path.join("rsconstruct.toml"),
                    format!("[processor.{}]\nsrc_dirs = [\".\"]\n", $proc),
                ).unwrap();

                $( std::fs::write(project_path.join($fname), $content).unwrap(); )+

                let output = crate::common::run_rsconstruct_with_env(
                    project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
                assert!(
                    output.status.success(),
                    "Build should succeed: stdout={}, stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }

            #[test]
            fn [<$mod_name _incremental_skip>]() {
                crate::common::require_tool($tool);

                let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
                let project_path = temp_dir.path();

                std::fs::write(
                    project_path.join("rsconstruct.toml"),
                    format!("[processor.{}]\nsrc_dirs = [\".\"]\n", $proc),
                ).unwrap();

                $( std::fs::write(project_path.join($fname), $content).unwrap(); )+

                let output1 = crate::common::run_rsconstruct_with_env(
                    project_path, &["build"], &[("NO_COLOR", "1")]);
                assert!(output1.status.success());

                let output2 = crate::common::run_rsconstruct_with_env(
                    project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
                assert!(output2.status.success());
                let stdout2 = String::from_utf8_lossy(&output2.stdout);
                assert!(
                    stdout2.contains(&format!("[{}] Skipping (unchanged):", $proc)),
                    "Second build should skip: {}", stdout2
                );
            }
        }
    };

    // No-project test: just verify the processor works with no matching files
    ($mod_name:ident, tool: $tool:expr, processor: $proc:expr, no_project) => {
        paste::paste! {
            #[test]
            fn [<$mod_name _no_project_discovered>]() {
                crate::common::require_tool($tool);

                let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
                let project_path = temp_dir.path();

                std::fs::write(
                    project_path.join("rsconstruct.toml"),
                    format!("[processor.{}]\nsrc_dirs = [\".\"]\n", $proc),
                ).unwrap();

                let output = crate::common::run_rsconstruct_with_env(
                    project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
                assert!(
                    output.status.success(),
                    "Build should succeed with no files: stdout={}, stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    };

    // Custom config test: with extra TOML config
    ($mod_name:ident, tool: $tool:expr, processor: $proc:expr,
     config: $config:expr,
     files: [ $( ($fname:expr, $content:expr) ),+ $(,)? ]) => {
        paste::paste! {
            #[test]
            fn [<$mod_name _valid>]() {
                crate::common::require_tool($tool);

                let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
                let project_path = temp_dir.path();

                std::fs::write(project_path.join("rsconstruct.toml"), $config).unwrap();

                $( std::fs::write(project_path.join($fname), $content).unwrap(); )+

                let output = crate::common::run_rsconstruct_with_env(
                    project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
                assert!(
                    output.status.success(),
                    "Build should succeed: stdout={}, stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }

            #[test]
            fn [<$mod_name _incremental_skip>]() {
                crate::common::require_tool($tool);

                let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
                let project_path = temp_dir.path();

                std::fs::write(project_path.join("rsconstruct.toml"), $config).unwrap();

                $( std::fs::write(project_path.join($fname), $content).unwrap(); )+

                let output1 = crate::common::run_rsconstruct_with_env(
                    project_path, &["build"], &[("NO_COLOR", "1")]);
                assert!(output1.status.success());

                let output2 = crate::common::run_rsconstruct_with_env(
                    project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
                assert!(output2.status.success());
                let stdout2 = String::from_utf8_lossy(&output2.stdout);
                assert!(
                    stdout2.contains(&format!("[{}] Skipping (unchanged):", $proc)),
                    "Second build should skip: {}", stdout2
                );
            }
        }
    };
}
