# CC Single File Processor

## Purpose

Compiles C (`.c`) and C++ (`.cc`) source files into executables, one source file per executable.

## How It Works

Source files under the configured source directory are compiled into executables
under `out/cc_single_file/`, mirroring the directory structure:

```
src/main.c       →  out/cc_single_file/main.elf
src/a/b.c        →  out/cc_single_file/a/b.elf
src/app.cc       →  out/cc_single_file/app.elf
```

Header dependencies are automatically tracked via compiler-generated `.d` files
(`-MMD -MF`). When a header changes, all source files that include it are rebuilt.

## Source Files

- Input: `{source_dir}/**/*.c`, `{source_dir}/**/*.cc`
- Output: `out/cc_single_file/{relative_path}{output_suffix}`

## Per-File Flags

Per-file compile and link flags can be set via special comments in source files.
This allows individual files to require specific libraries or compiler options
without affecting the entire project.

### Flag directives

```c
// EXTRA_COMPILE_FLAGS_BEFORE=-pthread
// EXTRA_COMPILE_FLAGS_AFTER=-O2 -DNDEBUG
// EXTRA_LINK_FLAGS_BEFORE=-L/usr/local/lib
// EXTRA_LINK_FLAGS_AFTER=-lX11
```

### Command directives

Execute a command and use its stdout as flags (no shell):

```c
// EXTRA_COMPILE_CMD=pkg-config --cflags gtk+-3.0
// EXTRA_LINK_CMD=pkg-config --libs gtk+-3.0
```

### Shell directives

Execute via `sh -c` (full shell syntax):

```c
// EXTRA_COMPILE_SHELL=echo -DLEVEL2_CACHE_LINESIZE=$(getconf LEVEL2_CACHE_LINESIZE)
// EXTRA_LINK_SHELL=echo -L$(brew --prefix openssl)/lib
```

### Backtick substitution

Flag directives also support backtick substitution for inline command execution:

```c
// EXTRA_COMPILE_FLAGS_AFTER=`pkg-config --cflags gtk+-3.0`
// EXTRA_LINK_FLAGS_AFTER=`pkg-config --libs gtk+-3.0`
```

### Command caching

All command and shell directives (`EXTRA_*_CMD`, `EXTRA_*_SHELL`, and backtick substitutions) are cached in memory during a build. If multiple source files use the same command (e.g., `pkg-config --cflags gtk+-3.0`), it is executed only once. This improves build performance when many files share common dependencies.

### Compiler profile-specific flags

When using multiple compiler profiles, you can specify flags that only apply to a specific compiler by adding `[profile_name]` after the directive name:

```c
// EXTRA_COMPILE_FLAGS_BEFORE=-g
// EXTRA_COMPILE_FLAGS_BEFORE[gcc]=-femit-struct-debug-baseonly
// EXTRA_COMPILE_FLAGS_BEFORE[clang]=-gline-tables-only
```

In this example:
- `-g` is applied to all compilers
- `-femit-struct-debug-baseonly` is only applied when compiling with the "gcc" profile
- `-gline-tables-only` is only applied when compiling with the "clang" profile

The profile name matches the `name` field in your `[[processor.cc_single_file.compilers]]` configuration:

```toml
[[processor.cc_single_file.compilers]]
name = "gcc"      # Matches [gcc] suffix
cc = "gcc"

[[processor.cc_single_file.compilers]]
name = "clang"    # Matches [clang] suffix
cc = "clang"
```

This works with all directive types:
- `EXTRA_COMPILE_FLAGS_BEFORE[profile]`
- `EXTRA_COMPILE_FLAGS_AFTER[profile]`
- `EXTRA_LINK_FLAGS_BEFORE[profile]`
- `EXTRA_LINK_FLAGS_AFTER[profile]`
- `EXTRA_COMPILE_CMD[profile]`
- `EXTRA_LINK_CMD[profile]`
- `EXTRA_COMPILE_SHELL[profile]`
- `EXTRA_LINK_SHELL[profile]`

### Excluding files from specific profiles

To exclude a source file from being compiled with specific compiler profiles, use `EXCLUDE_PROFILE`:

```c
// EXCLUDE_PROFILE=clang
```

This is useful when a file uses compiler-specific features that aren't available in other compilers. For example, a file using GCC-only builtins like `__builtin_va_arg_pack_len()`:

```c
// EXCLUDE_PROFILE=clang
// This file uses GCC-specific builtins
#include <stdarg.h>

void example(int first, ...) {
    int count = __builtin_va_arg_pack_len();  // GCC-only
    // ...
}
```

You can exclude multiple profiles by listing them space-separated:

```c
// EXCLUDE_PROFILE=clang icc
```

### Directive summary

| Directive | Execution | Use case |
|---|---|---|
| `EXTRA_COMPILE_FLAGS_BEFORE` | Literal flags | Flags before default cflags |
| `EXTRA_COMPILE_FLAGS_AFTER` | Literal flags | Flags after default cflags |
| `EXTRA_LINK_FLAGS_BEFORE` | Literal flags | Flags before default ldflags |
| `EXTRA_LINK_FLAGS_AFTER` | Literal flags | Flags after default ldflags |
| `EXTRA_COMPILE_CMD` | Subprocess (no shell) | Dynamic compile flags via command |
| `EXTRA_LINK_CMD` | Subprocess (no shell) | Dynamic link flags via command |
| `EXTRA_COMPILE_SHELL` | `sh -c` (full shell) | Dynamic compile flags needing shell features |
| `EXTRA_LINK_SHELL` | `sh -c` (full shell) | Dynamic link flags needing shell features |

### Supported comment styles

Directives can appear in any of these comment styles:

**C++ style:**
```c
// EXTRA_LINK_FLAGS_AFTER=-lX11
```

**C block comment (single line):**
```c
/* EXTRA_LINK_FLAGS_AFTER=-lX11 */
```

**C block comment (multi-line, star-prefixed):**
```c
/*
 * EXTRA_LINK_FLAGS_AFTER=-lX11
 */
```

## Command Line Ordering

The compiler command is constructed in this order:

```
compiler -MMD -MF deps -I... [compile_before] [cflags/cxxflags] [compile_after] -o output source [link_before] [ldflags] [link_after]
```

Link flags come **after** the source file so the linker can resolve symbols correctly.

| Position | Source |
|---|---|
| `compile_before` | `EXTRA_COMPILE_FLAGS_BEFORE` + `EXTRA_COMPILE_CMD` + `EXTRA_COMPILE_SHELL` |
| `cflags/cxxflags` | `[processor.cc_single_file]` config `cflags` or `cxxflags` |
| `compile_after` | `EXTRA_COMPILE_FLAGS_AFTER` |
| `link_before` | `EXTRA_LINK_FLAGS_BEFORE` + `EXTRA_LINK_CMD` + `EXTRA_LINK_SHELL` |
| `ldflags` | `[processor.cc_single_file]` config `ldflags` |
| `link_after` | `EXTRA_LINK_FLAGS_AFTER` |

## Verbosity Levels (`--processor-verbose N`)

| Level | Output |
|-------|--------|
| 0 (default) | Target basename: `main.elf` |
| 1 | Target path + compiler commands: `out/cc_single_file/main.elf` |
| 2 | Adds source path: `out/cc_single_file/main.elf <- src/main.c` |
| 3 | Adds all inputs: `out/cc_single_file/main.elf <- src/main.c, src/utils.h` |

## Configuration

### Single Compiler (Legacy)

```toml
[processor.cc_single_file]
cc = "gcc"                # C compiler (default: "gcc")
cxx = "g++"               # C++ compiler (default: "g++")
cflags = []               # C compiler flags
cxxflags = []             # C++ compiler flags
ldflags = []              # Linker flags
include_paths = []        # Additional -I paths (relative to project root)
src_dirs = ["src"]          # Source directory (required; no default)
output_suffix = ".elf"    # Suffix for output executables (default: ".elf")
dep_inputs = []         # Additional files that trigger rebuilds when changed
include_scanner = "native" # Method for scanning header dependencies (default: "native")
```

### Multiple Compilers

To compile with multiple compilers (e.g., both GCC and Clang), use the `compilers` array:

```toml
[processor.cc_single_file]
src_dirs = ["src"]
include_paths = ["include"]  # Shared across all compilers

[[processor.cc_single_file.compilers]]
name = "gcc"
cc = "gcc"
cxx = "g++"
cflags = ["-Wall", "-Wextra"]
cxxflags = ["-Wall", "-Wextra"]
ldflags = []
output_suffix = ".elf"

[[processor.cc_single_file.compilers]]
name = "clang"
cc = "clang"
cxx = "clang++"
cflags = ["-Wall", "-Wextra", "-Weverything"]
cxxflags = ["-Wall", "-Wextra"]
ldflags = []
output_suffix = ".elf"
```

When using multiple compilers, outputs are organized by compiler name:

```
src/main.c  →  out/cc_single_file/gcc/main.elf
            →  out/cc_single_file/clang/main.elf
```

Each source file is compiled once per compiler profile, allowing you to:
- Test code with multiple compilers to catch different warnings
- Compare output between compilers
- Build for different targets (cross-compilation)

### Configuration Reference

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `cc` | string | `"gcc"` | C compiler command |
| `cxx` | string | `"g++"` | C++ compiler command |
| `cflags` | string[] | `[]` | Flags passed to the C compiler |
| `cxxflags` | string[] | `[]` | Flags passed to the C++ compiler |
| `ldflags` | string[] | `[]` | Flags passed to the linker |
| `include_paths` | string[] | `[]` | Additional `-I` include paths (shared) |
| `src_dirs` | string[] | `[]` | Directories to scan for source files. No default — an unset `src_dirs` scans nothing. |
| `output_suffix` | string | `".elf"` | Suffix appended to output executables |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |
| `include_scanner` | string | `"native"` | Method for scanning header dependencies |
| `compilers` | array | `[]` | Multiple compiler profiles (overrides single-compiler fields) |

### Compiler Profile Fields

Each entry in the `compilers` array can have:

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `name` | string | Yes | Profile name (used in output path) |
| `cc` | string | No | C compiler (default: "gcc") |
| `cxx` | string | No | C++ compiler (default: "g++") |
| `cflags` | string[] | No | C compiler flags |
| `cxxflags` | string[] | No | C++ compiler flags |
| `ldflags` | string[] | No | Linker flags |
| `output_suffix` | string | No | Output suffix (default: ".elf") |

## Batch support

Each input file is processed individually, producing its own output file.

## Include Scanner

The `include_scanner` option controls how header dependencies are discovered:

| Value | Description |
|-------|-------------|
| `native` | Fast regex-based scanner (default). Parses `#include` directives directly without spawning external processes. Handles `#include "file"` and `#include <file>` forms. |
| `compiler` | Uses `gcc -MM` / `g++ -MM` to scan dependencies. More accurate for complex cases (computed includes, conditional compilation) but slower as it spawns a compiler process per source file. |

### Native scanner behavior

The native scanner:
- Recursively follows `#include` directives
- Searches include paths in order: source file directory, configured `include_paths`, project root
- Skips system headers (`/usr/...`, `/lib/...`)
- Only tracks project-local headers (relative paths)

### When to use compiler scanner

Use `include_scanner = "compiler"` if you have:
- Computed includes: `#include MACRO_THAT_EXPANDS_TO_FILENAME`
- Complex conditional compilation affecting which headers are included
- Headers outside the standard search paths that the native scanner misses

The native scanner may occasionally report extra dependencies (false positives), which is safe—it just means some files might rebuild unnecessarily. It will not miss dependencies (false negatives) for standard `#include` patterns.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
