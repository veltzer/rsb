# Pytest Processor

## Purpose

Runs Python test files using pytest to verify they pass.

## How It Works

Python test files (`.py`) in the `tests/` directory are run using `pytest`.
Each test file is checked individually — a failing test causes the build to fail.

## Source Files

- Input: `tests/**/*.py`
- Output: none (checker — pass/fail only)

## Configuration

```toml
[processor.pytest]
src_extensions = [".py"]                      # File extensions to process (default: [".py"])
src_dirs = ["tests"]                     # Directories to scan (required; no default)
dep_inputs = []                         # Additional files that trigger rebuilds
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `src_extensions` | string[] | `[".py"]` | File extensions to discover |
| `src_dirs` | string[] | `["tests"]` | Directories to scan for test files |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

The tool accepts multiple files on the command line. When batching is enabled (default), rsconstruct passes all files in a single invocation for better performance.

## Clean behavior

This processor is a Checker — `rsconstruct clean outputs` is a no-op for it (checkers produce no outputs). See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
