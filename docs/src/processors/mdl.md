# Mdl Processor

## Purpose

Lints Markdown files using [mdl](https://github.com/markdownlint/markdownlint) (Ruby markdownlint).

## How It Works

Discovers `.md` files in the project and runs `mdl` on each file. A non-zero
exit code fails the product.

Depends on the gem processor — uses the `mdl` binary installed by Bundler.

## Source Files

- Input: `**/*.md`
- Output: none (checker)

## Configuration

```toml
[processor.mdl]
gem_home = "gems"                      # GEM_HOME directory
command = "mdl"                        # Path to the mdl binary
args = []                              # Additional arguments to pass to mdl
gem_stamp = "out/gem/root.stamp"       # Stamp file from gem processor (dependency)
dep_inputs = []                      # Additional files that trigger rebuilds when changed
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `gem_home` | string | `"gems"` | GEM_HOME directory for Ruby gems |
| `command` | string | `"mdl"` | Path to the mdl executable |
| `args` | string[] | `[]` | Extra arguments passed to mdl |
| `gem_stamp` | string | `"out/gem/root.stamp"` | Stamp file from gem processor (ensures gems are installed first) |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

The tool processes one file at a time. Each file is checked in a separate invocation.

## Clean behavior

This processor is a Checker — `rsconstruct clean outputs` is a no-op for it (checkers produce no outputs). See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
