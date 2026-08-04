# Mermaid Processor

## Purpose

Converts [Mermaid](https://mermaid.js.org/) diagram files to PNG, SVG, or PDF using `mmdc` (mermaid-cli).

## How It Works

Discovers `.mmd` files in the project and runs `mmdc` on each file, generating
output in the configured formats. Each format produces a separate output file.

## Source Files

- Input: `**/*.mmd`
- Output: `out/mermaid/{format}/{relative_path}.{format}`

## Configuration

```toml
[processor.mermaid]
command = "mmdc"                       # The mmdc command to run
formats = ["png"]                      # Output formats (png, svg, pdf)
args = []                              # Additional arguments to pass to mmdc
output_dir = "out/mermaid"             # Output directory
dep_inputs = []                      # Additional files that trigger rebuilds when changed
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"mmdc"` | The mermaid-cli executable to run |
| `formats` | string[] | `["png"]` | Output formats to generate (`png`, `svg`, `pdf`) |
| `args` | string[] | `[]` | Extra arguments passed to mmdc |
| `output_dir` | string | `"out/mermaid"` | Base output directory |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
