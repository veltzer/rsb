use std::fs;
use std::process::Command;
use tempfile::TempDir;
use crate::common::{setup_cc_project, run_rsconstruct, run_rsconstruct_with_env};

#[test]
fn cc_single_file_compile_single_c_file() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    fs::write(
        project_path.join("src/main.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "rsconstruct build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    // Check executable exists
    assert!(project_path.join("out/cc_single_file/src/main.elf").exists(), "Executable should exist");
}

#[test]
fn cc_single_file_incremental_skip() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    fs::write(
        project_path.join("src/main.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success());
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "First build should process: {}", stdout1);

    // Second build - should skip
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[cc_single_file] Skipping (unchanged):"), "Second build should skip: {}", stdout2);
}

#[test]
fn cc_single_file_header_dependency() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Create header and source
    fs::write(
        project_path.join("src/utils.h"),
        "#ifndef UTILS_H\n#define UTILS_H\n#define VALUE 42\n#endif\n"
    ).unwrap();

    fs::write(
        project_path.join("src/main.c"),
        "#include \"utils.h\"\nint main() { return VALUE - 42; }\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));

    // Wait a moment so mtime differs
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Modify header (keep VALUE defined so compilation still succeeds)
    fs::write(
        project_path.join("src/utils.h"),
        "#ifndef UTILS_H\n#define UTILS_H\n#define VALUE 42\n#define OTHER 10\n#endif\n"
    ).unwrap();

    // Rebuild - should recompile files that include utils.h
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success(),
        "Rebuild failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output2.stdout),
        String::from_utf8_lossy(&output2.stderr));
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("Processing:"),
        "Should recompile after header change: {}", stdout2);
}

#[test]
fn cc_single_file_mixed_c_and_cpp() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    fs::write(
        project_path.join("src/helper.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    fs::write(
        project_path.join("src/main.cc"),
        "int main() { return 0; }\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Mixed C/C++ build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/helper.elf").exists(), "C executable should exist");
    assert!(project_path.join("out/cc_single_file/src/main.elf").exists(), "C++ executable should exist");
}

#[test]
fn cc_single_file_clean() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    fs::write(
        project_path.join("src/main.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    // Build
    let build_output = run_rsconstruct(project_path, &["build"]);
    assert!(build_output.status.success());
    assert!(project_path.join("out/cc_single_file/src/main.elf").exists());

    // Clean
    let clean_output = run_rsconstruct(project_path, &["clean", "outputs"]);
    assert!(clean_output.status.success());

    // Verify outputs are removed but .rsconstruct cache is preserved
    assert!(!project_path.join("out/cc_single_file/src/main.elf").exists(), "output should be removed after clean");
    assert!(project_path.join(".rsconstruct").exists(), ".rsconstruct cache should be preserved after clean");
}

#[test]
fn cc_single_file_dry_run() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    fs::write(
        project_path.join("src/main.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    // Dry run
    let output = run_rsconstruct_with_env(project_path, &["build", "--dry-run"], &[("NO_COLOR", "1")]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("BUILD"), "Dry run should show BUILD for cc products: {}", stdout);

    // Verify nothing was built
    assert!(!project_path.join("out/cc_single_file/src/main.elf").exists(), "Dry run should not compile");
}

#[test]
fn cc_single_file_config_change_triggers_rebuild() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    fs::write(
        project_path.join("src/main.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    // First build — should process
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "First build should process: {}", stdout1);

    // Second build — should skip (nothing changed)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[cc_single_file] Skipping (unchanged):"), "Second build should skip: {}", stdout2);

    // Change cflags in rsconstruct.toml
    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.cc_single_file]\nsrc_dirs = [\"src\"]\ncflags = [\"-O2\"]\n"
    ).unwrap();

    // Third build — should rebuild because config changed
    let output3 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output3.status.success(),
        "Third build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output3.stdout),
        String::from_utf8_lossy(&output3.stderr));
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("Processing:"),
        "Build after config change should reprocess, not skip: {}", stdout3);
}

#[test]
fn cc_single_file_per_file_compile_flags() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Source with EXTRA_COMPILE_FLAGS_AFTER defining a macro
    fs::write(
        project_path.join("src/flagtest.c"),
        r#"// EXTRA_COMPILE_FLAGS_AFTER=-DTEST_VALUE=42
#include <stdio.h>
int main() {
    printf("%d\n", TEST_VALUE);
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with per-file compile flags failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/flagtest.elf").exists(),
        "Executable with per-file compile flags should exist");

    // Run the executable and verify it outputs 42
    let run_output = Command::new(project_path.join("out/cc_single_file/src/flagtest.elf"))
        .output()
        .expect("Failed to run flagtest");
    assert!(run_output.status.success(), "flagtest exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "42",
        "Executable should output 42, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_link_flags() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Source that uses math library (sqrt), linked via per-file flag
    fs::write(
        project_path.join("src/mathtest.c"),
        r#"// EXTRA_LINK_FLAGS_AFTER=-lm
#include <stdio.h>
#include <math.h>
int main() {
    printf("%.0f\n", sqrt(144.0));
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with per-file link flags failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/mathtest.elf").exists(),
        "Executable with per-file link flags should exist");

    // Run the executable and verify it outputs 12
    let run_output = Command::new(project_path.join("out/cc_single_file/src/mathtest.elf"))
        .output()
        .expect("Failed to run mathtest");
    assert!(run_output.status.success(), "mathtest exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "12",
        "Executable should output 12, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_backtick_substitution() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Source with backtick command substitution to define a macro
    fs::write(
        project_path.join("src/backtick.c"),
        r#"// EXTRA_COMPILE_FLAGS_AFTER=`echo -DBACKTICK_VAL=99`
#include <stdio.h>
int main() {
    printf("%d\n", BACKTICK_VAL);
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with backtick substitution failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/backtick.elf").exists(),
        "Executable with backtick substitution should exist");

    // Run the executable and verify it outputs 99
    let run_output = Command::new(project_path.join("out/cc_single_file/src/backtick.elf"))
        .output()
        .expect("Failed to run backtick");
    assert!(run_output.status.success(), "backtick exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "99",
        "Executable should output 99, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_no_flags() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Source without any special comments
    fs::write(
        project_path.join("src/plain.c"),
        r#"#include <stdio.h>
int main() {
    printf("hello\n");
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build without per-file flags failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/plain.elf").exists(),
        "Executable without per-file flags should exist");

    let run_output = Command::new(project_path.join("out/cc_single_file/src/plain.elf"))
        .output()
        .expect("Failed to run plain");
    assert!(run_output.status.success(), "plain exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "hello",
        "Executable should output hello, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_compile_cmd() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // EXTRA_COMPILE_CMD runs a command as subprocess; use echo to produce a -D flag
    fs::write(
        project_path.join("src/compilecmd.c"),
        r#"// EXTRA_COMPILE_CMD=echo -DCMD_VAL=77
#include <stdio.h>
int main() {
    printf("%d\n", CMD_VAL);
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with EXTRA_COMPILE_CMD failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/compilecmd.elf").exists(),
        "Executable with EXTRA_COMPILE_CMD should exist");

    let run_output = Command::new(project_path.join("out/cc_single_file/src/compilecmd.elf"))
        .output()
        .expect("Failed to run compilecmd");
    assert!(run_output.status.success(), "compilecmd exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "77",
        "Executable should output 77, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_link_cmd() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // EXTRA_LINK_CMD runs a command; use echo to produce -lm
    fs::write(
        project_path.join("src/linkcmd.c"),
        r#"// EXTRA_LINK_CMD=echo -lm
#include <stdio.h>
#include <math.h>
int main() {
    printf("%.0f\n", sqrt(144.0));
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with EXTRA_LINK_CMD failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/linkcmd.elf").exists(),
        "Executable with EXTRA_LINK_CMD should exist");

    let run_output = Command::new(project_path.join("out/cc_single_file/src/linkcmd.elf"))
        .output()
        .expect("Failed to run linkcmd");
    assert!(run_output.status.success(), "linkcmd exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "12",
        "Executable should output 12, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_block_comment_star_prefix() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Block comment continuation line with * prefix
    fs::write(
        project_path.join("src/blockstar.c"),
        r#"/*
 * EXTRA_LINK_FLAGS_AFTER=-lm
 */
#include <stdio.h>
#include <math.h>
int main() {
    printf("%.0f\n", sqrt(144.0));
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with block comment * prefix failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/blockstar.elf").exists(),
        "Executable with block comment * prefix should exist");

    let run_output = Command::new(project_path.join("out/cc_single_file/src/blockstar.elf"))
        .output()
        .expect("Failed to run blockstar");
    assert!(run_output.status.success(), "blockstar exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "12",
        "Executable should output 12, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_compile_shell() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Use EXTRA_COMPILE_SHELL to define a macro via shell command
    fs::write(
        project_path.join("src/compileshell.c"),
        r#"// EXTRA_COMPILE_SHELL=echo -DSHELL_VALUE=$(echo 77)
#include <stdio.h>
int main() {
    printf("%d\n", SHELL_VALUE);
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with EXTRA_COMPILE_SHELL failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/compileshell.elf").exists(),
        "Executable with EXTRA_COMPILE_SHELL should exist");

    let run_output = Command::new(project_path.join("out/cc_single_file/src/compileshell.elf"))
        .output()
        .expect("Failed to run compileshell");
    assert!(run_output.status.success(), "compileshell exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "77",
        "Executable should output 77, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_per_file_link_shell() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Use EXTRA_LINK_SHELL to add -lm via shell
    fs::write(
        project_path.join("src/linkshell.c"),
        r#"// EXTRA_LINK_SHELL=echo -lm
#include <stdio.h>
#include <math.h>
int main() {
    printf("%.0f\n", sqrt(49.0));
    return 0;
}
"#
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with EXTRA_LINK_SHELL failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    assert!(project_path.join("out/cc_single_file/src/linkshell.elf").exists(),
        "Executable with EXTRA_LINK_SHELL should exist");

    let run_output = Command::new(project_path.join("out/cc_single_file/src/linkshell.elf"))
        .output()
        .expect("Failed to run linkshell");
    assert!(run_output.status.success(), "linkshell exited with error");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.trim() == "7",
        "Executable should output 7, got: {}", stdout.trim());
}

#[test]
fn cc_single_file_direct_header_change_triggers_rebuild() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Create header and source that includes it directly
    fs::write(
        project_path.join("src/direct.h"),
        "#define DIRECT_VAL 10\n"
    ).unwrap();

    fs::write(
        project_path.join("src/main.c"),
        "#include \"direct.h\"\nint main() { return DIRECT_VAL - 10; }\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "First build should process: {}", stdout1);

    // Second build — should skip (nothing changed)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[cc_single_file] Skipping (unchanged):"),
        "Second build should skip: {}", stdout2);

    // Modify the directly included header
    fs::write(
        project_path.join("src/direct.h"),
        "#define DIRECT_VAL 20\n"
    ).unwrap();

    // Third build — should recompile because the direct header changed
    let output3 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output3.status.success(),
        "Rebuild after direct header change failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output3.stdout),
        String::from_utf8_lossy(&output3.stderr));
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("Processing:"),
        "Should recompile after direct header change: {}", stdout3);

    // Verify the new value was compiled in (return 20 - 10 = 10, nonzero exit)
    let run_output = Command::new(project_path.join("out/cc_single_file/src/main.elf"))
        .output()
        .expect("Failed to run main");
    assert!(!run_output.status.success(),
        "Executable should exit nonzero after header change (DIRECT_VAL=20, returns 20-10=10)");
}

#[test]
fn cc_single_file_indirect_header_change_triggers_rebuild() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Create an indirect header, a direct header that includes it, and a source file
    fs::write(
        project_path.join("src/indirect.h"),
        "#define INDIRECT_VAL 5\n"
    ).unwrap();

    fs::write(
        project_path.join("src/middle.h"),
        "#include \"indirect.h\"\n#define MIDDLE_VAL (INDIRECT_VAL + 1)\n"
    ).unwrap();

    fs::write(
        project_path.join("src/main.c"),
        "#include \"middle.h\"\nint main() { return MIDDLE_VAL - 6; }\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "First build should process: {}", stdout1);

    // Verify exit code 0 (MIDDLE_VAL=6, 6-6=0)
    let run_output = Command::new(project_path.join("out/cc_single_file/src/main.elf"))
        .output()
        .expect("Failed to run main");
    assert!(run_output.status.success(), "Executable should exit 0 initially");

    // Second build — should skip (nothing changed)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[cc_single_file] Skipping (unchanged):"),
        "Second build should skip: {}", stdout2);

    // Modify the indirect header (not directly included by source)
    fs::write(
        project_path.join("src/indirect.h"),
        "#define INDIRECT_VAL 100\n"
    ).unwrap();

    // Third build — should recompile because an indirect header changed
    let output3 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output3.status.success(),
        "Rebuild after indirect header change failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output3.stdout),
        String::from_utf8_lossy(&output3.stderr));
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("Processing:"),
        "Should recompile after indirect header change: {}", stdout3);

    // Verify the new value was compiled in (MIDDLE_VAL=101, 101-6=95, nonzero exit)
    let run_output2 = Command::new(project_path.join("out/cc_single_file/src/main.elf"))
        .output()
        .expect("Failed to run main after indirect header change");
    assert!(!run_output2.status.success(),
        "Executable should exit nonzero after indirect header change (MIDDLE_VAL=101, returns 101-6=95)");
}

#[test]
fn cc_single_file_new_include_triggers_dependency_recomputation() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    setup_cc_project(project_path);

    // Step 1: Create source file without any includes
    fs::write(
        project_path.join("src/main.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("Processing:"), "First build should process: {}", stdout1);

    // Step 2: Second build — should skip (nothing changed)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[cc_single_file] Skipping (unchanged):"),
        "Second build should skip: {}", stdout2);

    // Step 3: Create a new header and modify source to include it
    fs::write(
        project_path.join("src/newheader.h"),
        "#define NEW_VAL 55\n"
    ).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(100));

    fs::write(
        project_path.join("src/main.c"),
        "#include \"newheader.h\"\nint main() { return NEW_VAL - 55; }\n"
    ).unwrap();

    // Step 4: Build — should recompile (source changed, deps re-scanned picking up newheader.h)
    let output3 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output3.status.success(),
        "Build after adding include failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output3.stdout),
        String::from_utf8_lossy(&output3.stderr));
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("Processing:"),
        "Should recompile after source changed to add include: {}", stdout3);

    // Step 5: Build again — should skip (nothing changed)
    let output4 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output4.status.success());
    let stdout4 = String::from_utf8_lossy(&output4.stdout);
    assert!(stdout4.contains("[cc_single_file] Skipping (unchanged):"),
        "Build should skip after no changes: {}", stdout4);

    // Step 6: Modify the newly-included header
    std::thread::sleep(std::time::Duration::from_millis(100));

    fs::write(
        project_path.join("src/newheader.h"),
        "#define NEW_VAL 99\n"
    ).unwrap();

    // Step 7: Build — should recompile (newly-tracked header changed)
    let output5 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output5.status.success(),
        "Build after modifying new header failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output5.stdout),
        String::from_utf8_lossy(&output5.stderr));
    let stdout5 = String::from_utf8_lossy(&output5.stdout);
    assert!(stdout5.contains("Processing:"),
        "Should recompile after newly-tracked header changed: {}", stdout5);

    // Verify the new value was compiled in (NEW_VAL=99, 99-55=44, nonzero exit)
    let run_output = Command::new(project_path.join("out/cc_single_file/src/main.elf"))
        .output()
        .expect("Failed to run main");
    assert!(!run_output.status.success(),
        "Executable should exit nonzero after header change (NEW_VAL=99, returns 99-55=44)");
}

#[test]
fn cc_single_file_angle_bracket_include_dependency() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create project with include_paths configured
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::create_dir_all(project_path.join("include")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"[processor.cc_single_file]
src_dirs = ["src"]
include_paths = ["include"]

[analyzer.cpp]
include_paths = ["include"]
"#
    ).unwrap();

    // Create a header in include/ directory
    fs::write(
        project_path.join("include/mylib.h"),
        "#ifndef MYLIB_H\n#define MYLIB_H\n#define MYLIB_VALUE 123\n#endif\n"
    ).unwrap();

    // Create source file that uses angle-bracket include for local header
    fs::write(
        project_path.join("src/main.c"),
        "#include <mylib.h>\nint main() { return MYLIB_VALUE - 123; }\n"
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output1.status.success(),
        "First build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr));

    // Second build - should skip (nothing changed)
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("[cc_single_file] Skipping (unchanged):"),
        "Second build should skip: {}", stdout2);

    // Modify the angle-bracket included header
    fs::write(
        project_path.join("include/mylib.h"),
        "#ifndef MYLIB_H\n#define MYLIB_H\n#define MYLIB_VALUE 456\n#endif\n"
    ).unwrap();

    // Third build - should recompile because the header changed
    let output3 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output3.status.success(),
        "Rebuild after angle-bracket header change failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output3.stdout),
        String::from_utf8_lossy(&output3.stderr));
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("Processing:"),
        "Should recompile after angle-bracket header change: {}", stdout3);
}

#[test]
fn cc_single_file_multiple_compiler_profiles() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create project with multiple compiler profiles
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"[processor.cc_single_file]
src_dirs = ["src"]

[[processor.cc_single_file.compilers]]
name = "gcc"
cc = "gcc"
cxx = "g++"
output_suffix = ".elf"

[[processor.cc_single_file.compilers]]
name = "clang"
cc = "clang"
cxx = "clang++"
output_suffix = ".elf"
"#
    ).unwrap();

    fs::write(
        project_path.join("src/main.c"),
        "int main() { return 0; }\n"
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "rsconstruct build with multiple compilers failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    // Check both executables exist
    assert!(project_path.join("out/cc_single_file/gcc/src/main.elf").exists(),
        "GCC executable should exist");
    assert!(project_path.join("out/cc_single_file/clang/src/main.elf").exists(),
        "Clang executable should exist");

    // Verify both executables run successfully
    let gcc_output = Command::new(project_path.join("out/cc_single_file/gcc/src/main.elf"))
        .output()
        .expect("Failed to run gcc executable");
    assert!(gcc_output.status.success(), "GCC executable should run successfully");

    let clang_output = Command::new(project_path.join("out/cc_single_file/clang/src/main.elf"))
        .output()
        .expect("Failed to run clang executable");
    assert!(clang_output.status.success(), "Clang executable should run successfully");
}

#[test]
fn cc_single_file_missing_include_errors() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create minimal project
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"[processor.cc_single_file]
src_dirs = ["src"]
[analyzer.icpp]
"#
    ).unwrap();

    // Create source file with missing include
    fs::write(
        project_path.join("src/main.c"),
        r#"#include "nonexistent.h"
int main() { return 0; }
"#
    ).unwrap();

    // Build should fail with error about missing include
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(),
        "Build with missing include should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Include not found") && stderr.contains("nonexistent.h"),
        "Error should mention missing include: {}", stderr);
}

#[test]
fn cc_single_file_profile_specific_flags() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create project with multiple compiler profiles
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"[processor.cc_single_file]
src_dirs = ["src"]

[[processor.cc_single_file.compilers]]
name = "gcc"
cc = "gcc"
cxx = "g++"

[[processor.cc_single_file.compilers]]
name = "clang"
cc = "clang"
cxx = "clang++"
"#
    ).unwrap();

    // Create source file with profile-specific flags
    // GCC gets -DCOMPILER_GCC, Clang gets -DCOMPILER_CLANG
    // Both get -DCOMMON from the non-profile-specific directive
    fs::write(
        project_path.join("src/profile_test.c"),
        r#"// EXTRA_COMPILE_FLAGS_BEFORE=-DCOMMON
// EXTRA_COMPILE_FLAGS_BEFORE[gcc]=-DCOMPILER_GCC
// EXTRA_COMPILE_FLAGS_BEFORE[clang]=-DCOMPILER_CLANG
#include <stdio.h>

int main() {
#ifdef COMMON
    printf("COMMON ");
#endif
#ifdef COMPILER_GCC
    printf("GCC\n");
#endif
#ifdef COMPILER_CLANG
    printf("CLANG\n");
#endif
    return 0;
}
"#
    ).unwrap();

    // Build
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build with profile-specific flags failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    // Run GCC executable - should print "COMMON GCC"
    let gcc_exe = project_path.join("out/cc_single_file/gcc/src/profile_test.elf");
    assert!(gcc_exe.exists(), "GCC executable should exist");
    let gcc_output = Command::new(&gcc_exe)
        .output()
        .expect("Failed to run GCC executable");
    let gcc_stdout = String::from_utf8_lossy(&gcc_output.stdout);
    assert!(gcc_stdout.contains("COMMON") && gcc_stdout.contains("GCC") && !gcc_stdout.contains("CLANG"),
        "GCC build should have COMMON and GCC defined, got: {}", gcc_stdout);

    // Run Clang executable - should print "COMMON CLANG"
    let clang_exe = project_path.join("out/cc_single_file/clang/src/profile_test.elf");
    assert!(clang_exe.exists(), "Clang executable should exist");
    let clang_output = Command::new(&clang_exe)
        .output()
        .expect("Failed to run Clang executable");
    let clang_stdout = String::from_utf8_lossy(&clang_output.stdout);
    assert!(clang_stdout.contains("COMMON") && clang_stdout.contains("CLANG") && !clang_stdout.contains("GCC"),
        "Clang build should have COMMON and CLANG defined, got: {}", clang_stdout);
}

#[test]
fn cc_single_file_exclude_profile() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create project with multiple compiler profiles
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(
        project_path.join("rsconstruct.toml"),
        r#"[processor.cc_single_file]
src_dirs = ["src"]

[[processor.cc_single_file.compilers]]
name = "gcc"
cc = "gcc"
cxx = "g++"

[[processor.cc_single_file.compilers]]
name = "clang"
cc = "clang"
cxx = "clang++"
"#
    ).unwrap();

    // Create a file that should be built by both compilers
    fs::write(
        project_path.join("src/both.c"),
        r#"int main() { return 0; }
"#
    ).unwrap();

    // Create a file that should only be built by GCC (excluded from clang)
    fs::write(
        project_path.join("src/gcc_only.c"),
        r#"// EXCLUDE_PROFILE=clang
int main() { return 0; }
"#
    ).unwrap();

    // Create a file that should only be built by Clang (excluded from gcc)
    fs::write(
        project_path.join("src/clang_only.c"),
        r#"// EXCLUDE_PROFILE=gcc
int main() { return 0; }
"#
    ).unwrap();

    // Build
    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(output.status.success(),
        "Build failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    // Check that both.elf exists for both compilers
    assert!(project_path.join("out/cc_single_file/gcc/src/both.elf").exists(),
        "both.elf should exist for gcc");
    assert!(project_path.join("out/cc_single_file/clang/src/both.elf").exists(),
        "both.elf should exist for clang");

    // Check that gcc_only.elf exists only for gcc
    assert!(project_path.join("out/cc_single_file/gcc/src/gcc_only.elf").exists(),
        "gcc_only.elf should exist for gcc");
    assert!(!project_path.join("out/cc_single_file/clang/src/gcc_only.elf").exists(),
        "gcc_only.elf should NOT exist for clang");

    // Check that clang_only.elf exists only for clang
    assert!(!project_path.join("out/cc_single_file/gcc/src/clang_only.elf").exists(),
        "clang_only.elf should NOT exist for gcc");
    assert!(project_path.join("out/cc_single_file/clang/src/clang_only.elf").exists(),
        "clang_only.elf should exist for clang");
}
