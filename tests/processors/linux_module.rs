use std::fs;
use std::path::Path;

use crate::common::{make_executable, run_rsconstruct, write_file};

/// A fake `make` that mimics the parts of kbuild the linux_module processor
/// relies on, without needing real kernel headers:
///   * `modules` — reads the generated Kbuild in M=<dir>, extracts the module
///     name from `obj-m := <name>.o`, and writes `<dir>/<name>.ko`.
///   * `clean`   — recursively deletes every `*.ko` under M=<dir> (kbuild's
///     clean is destructive over the whole module tree — this is exactly the
///     behavior that used to wipe an output written under the module dir).
/// It ignores `-C <kdir>`, ARCH=, CROSS_COMPILE=, V=, W=.
const FAKE_MAKE: &str = r#"#!/usr/bin/env python3
import os
import sys

m_dir = None
subcommand = None
for arg in sys.argv[1:]:
    if arg.startswith("M="):
        m_dir = arg[2:]
    elif arg in ("modules", "clean"):
        subcommand = arg

if subcommand == "modules":
    kbuild = os.path.join(m_dir, "Kbuild")
    name = None
    with open(kbuild) as handle:
        for line in handle:
            if line.startswith("obj-m"):
                # obj-m := <name>.o
                name = line.split(":=", 1)[1].strip()[:-len(".o")]
    with open(os.path.join(m_dir, name + ".ko"), "wb") as handle:
        handle.write(b"FAKE-KO-BYTES")
elif subcommand == "clean":
    for root, _dirs, files in os.walk(m_dir):
        for filename in files:
            if filename.endswith(".ko"):
                os.unlink(os.path.join(root, filename))
sys.exit(0)
"#;

/// Write the fake make into the project and return its absolute path as a
/// string, so the manifest's `make:` can point straight at it. The file is
/// named `make` so its basename matches the processor's declared `make` tool
/// (the declared-tools guard compares basenames).
fn install_fake_make(project: &Path) -> String {
    let make_path = project.join("bin").join("make");
    fs::create_dir_all(make_path.parent().unwrap()).unwrap();
    fs::write(&make_path, FAKE_MAKE).unwrap();
    make_executable(&make_path);
    make_path.to_string_lossy().into_owned()
}

/// Regression test for the root-level manifest bug: when linux-module.yaml sits
/// at the repo root, the module directory is the repo root, so the output dir
/// (out/linux-module/) lives *inside* it. kbuild's `make clean` recursively
/// deletes .ko files under the module dir — which used to wipe the just-copied
/// output, leaving the declared output missing ("Failed to read output:
/// out/linux-module/<name>.ko"). The processor must capture the .ko before
/// cleaning and write it out afterwards, so a root-level module builds cleanly.
#[test]
fn linux_module_root_level_manifest_output_survives_clean() {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path();

    let make = install_fake_make(project);
    // A dummy kdir — the fake make ignores -C, so it need not be a real one.
    fs::create_dir_all(project.join("kdir")).unwrap();
    let kdir = project.join("kdir").to_string_lossy().into_owned();

    write_file(project, "top.c", "int top;\n");
    write_file(project, "helper.c", "int helper;\n");
    write_file(
        project,
        "linux-module.yaml",
        &format!(
            "make: {make}\nkdir: {kdir}\nmodules:\n  - name: mymod\n    sources: [top.c, helper.c]\n"
        ),
    );
    write_file(project, "rsconstruct.toml", "[processor.linux_module]\nsrc_dirs = [\".\"]\n");

    let output = run_rsconstruct(project, &["build"]);
    assert!(
        output.status.success(),
        "build failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The declared output exists and holds the built module.
    let ko = project.join("out/linux-module/mymod.ko");
    assert!(ko.exists(), "output module not produced at {}", ko.display());
    assert_eq!(fs::read(&ko).unwrap(), b"FAKE-KO-BYTES");

    // The source tree is left clean: no leftover .ko or generated Kbuild.
    assert!(!project.join("mymod.ko").exists(), "source .ko not cleaned up");
    assert!(!project.join("Kbuild").exists(), "generated Kbuild not removed");
}

/// A manifest in a subdirectory keeps working: the module dir is the subdir,
/// the output lands under out/linux-module/<subdir>/, and clean over the subdir
/// does not touch it. This guards the reordered clean/write against a
/// regression in the common (subdir) layout.
#[test]
fn linux_module_subdir_manifest_builds() {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path();

    let make = install_fake_make(project);
    fs::create_dir_all(project.join("kdir")).unwrap();
    let kdir = project.join("kdir").to_string_lossy().into_owned();

    write_file(project, "drivers/hello/main.c", "int hello;\n");
    write_file(
        project,
        "drivers/hello/linux-module.yaml",
        &format!("make: {make}\nkdir: {kdir}\nmodules:\n  - name: hello\n    sources: [main.c]\n"),
    );
    write_file(project, "rsconstruct.toml", "[processor.linux_module]\nsrc_dirs = [\"drivers\"]\n");

    let output = run_rsconstruct(project, &["build"]);
    assert!(
        output.status.success(),
        "build failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let ko = project.join("out/linux-module/drivers/hello/hello.ko");
    assert!(ko.exists(), "output module not produced at {}", ko.display());
    assert_eq!(fs::read(&ko).unwrap(), b"FAKE-KO-BYTES");
    assert!(!project.join("drivers/hello/hello.ko").exists(), "source .ko not cleaned up");
    assert!(!project.join("drivers/hello/Kbuild").exists(), "generated Kbuild not removed");
}
