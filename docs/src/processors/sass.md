# Sass Processor

## Purpose

Compiles SCSS and SASS files into CSS using the Sass compiler.

## How It Works

Files matching configured extensions in the `sass/` directory are compiled to CSS.
Output is written to `out/sass/` preserving the directory structure:

```
sass/style.scss  ->  out/sass/style.css
sass/components/button.scss  ->  out/sass/components/button.css
```

## Source Files

- Input: `sass/**/*{src_extensions}`
- Output: `out/sass/` mirroring the source structure with `.css` extension

## Configuration

```toml
[processor.sass]
command = "sass"                          # Sass compiler binary (default: "sass")
src_extensions = [".scss", ".sass"]           # File extensions to process
output_dir = "out/sass"                   # Output directory (default: "out/sass")
dep_inputs = []                         # Additional files that trigger rebuilds
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"sass"` | Path to sass compiler |
| `src_extensions` | string[] | `[".scss", ".sass"]` | File extensions to discover |
| `output_dir` | string | `"out/sass"` | Output directory |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
