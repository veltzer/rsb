use std::fs;
use crate::common::{make_executable, run_rsconstruct_with_env, setup_project_with_config};
use serde_json::Value;

/// The regression this guards: `doctor` used to probe `[dependencies] system`
/// entries with `which()`, treating packages as if they were tools. That
/// misreports in both directions — a binary-less package like aspell-en (a
/// dictionary) showed as missing even when installed, and any binary that
/// happens to share a package's name showed as installed without the package
/// being on. The probe must go through the package manager, the same one
/// `tools install-deps` uses.
///
/// The test plants a fake binary on PATH whose name is not a real package.
/// The old which()-based doctor reported it ok; the package-manager probe
/// must report it missing. This assumes a supported package manager exists
/// (dpkg/rpm/pacman/brew) — on a machine without one, `install-deps` bails
/// anyway, so that environment is already unsupported.
#[test]
fn doctor_system_dependency_is_probed_as_package_not_tool() {
    let temp_dir = setup_project_with_config(
        "[processor.tera]\nsrc_dirs = [\"tera.templates\"]\n\n[dependencies]\nsystem = [\"rsconstruct-fake-system-package\"]\n",
    );
    let project_path = temp_dir.path();
    fs::create_dir_all(project_path.join("tera.templates")).unwrap();

    // A binary with the package's exact name, on PATH.
    let bin_dir = project_path.join("fakebin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake = bin_dir.join("rsconstruct-fake-system-package");
    fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&fake);
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").expect("PATH must be set")
    );

    let output = run_rsconstruct_with_env(
        project_path,
        &["--json", "doctor"],
        &[("NO_COLOR", "1"), ("PATH", &path)],
    );
    assert!(
        output.status.success(),
        "doctor failed to run: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: Value = serde_json::from_slice(&output.stdout).expect("doctor --json should emit valid JSON");
    let checks = parsed["checks"].as_array().expect("doctor --json should have a checks array");
    let check = checks
        .iter()
        .find(|c| {
            c["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("rsconstruct-fake-system-package"))
        })
        .expect("doctor should report on the declared system dependency");

    assert_eq!(
        check["status"], "fail",
        "a fake binary on PATH is not an installed system package; doctor must ask \
         the package manager, not which(). Check was: {check}",
    );
    assert_eq!(check["category"], "dependency");
    assert_eq!(
        check["install_hint"], "rsconstruct tools install-deps",
        "the fix for a missing system dependency is install-deps, not a tool install",
    );
}
