# Chromium Processor

## Purpose

Converts HTML files to PDF using headless Chromium (Google Chrome).

## How It Works

Discovers `.html` files in the configured scan directory (default: `out/marp`) and runs
headless Chromium with `--print-to-pdf` on each file, producing a PDF output.

This is typically used as a post-processing step after another processor (e.g., Marp)
generates HTML files.

## Source Files

- Input: `out/marp/**/*.html` (default scan directory)
- Output: `out/chromium/{relative_path}.pdf`

## Configuration

```toml
[processor.chromium]
command = "google-chrome"                 # The Chromium/Chrome executable to run
args = []                                 # Additional arguments to pass to Chromium
output_dir = "out/chromium"               # Output directory for PDFs
dep_inputs = []                         # Additional files that trigger rebuilds when changed
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"google-chrome"` | The Chromium or Google Chrome executable |
| `args` | string[] | `[]` | Extra arguments passed to Chromium |
| `output_dir` | string | `"out/chromium"` | Base output directory for PDF files |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
