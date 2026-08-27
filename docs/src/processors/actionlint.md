# Actionlint Processor

## Purpose

Lints GitHub Actions workflow files using
[actionlint](https://github.com/rhysd/actionlint).

## How It Works

Discovers `.yml`/`.yaml` files in the configured directories, runs
`actionlint` on each file, and records success in the cache. A non-zero exit
code from actionlint fails the product.

actionlint validates workflow syntax against the GitHub Actions schema,
type-checks `${{ }}` expressions, checks `runs-on` labels and cron syntax,
flags untrusted-input injection in `run:` steps, and (when `shellcheck` is
installed) lints the shell scripts inside `run:` blocks.

This processor supports batch mode.

## Source Files

- Input: `**/*.yml`, `**/*.yaml` (point `src_dirs` at `.github/workflows`)
- Output: none (checker)

## Configuration

```toml
[processor.actionlint]
src_dirs = [".github/workflows"]
args = []
dep_inputs = []
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `src_dirs` | string[] | `[]` | Directories to scan — set to `[".github/workflows"]` |
| `args` | string[] | `[]` | Extra arguments passed to actionlint |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

Scoping `src_dirs` to `.github/workflows` matters: actionlint treats every
file it is given as a workflow file, so pointing it at a directory of
ordinary YAML would report false errors.

## Batch support

The tool accepts multiple files on the command line. When batching is enabled (default), rsconstruct passes all files in a single invocation for better performance.

## Clean behavior

This processor is a Checker — `rsconstruct clean outputs` is a no-op for it (checkers produce no outputs). See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
