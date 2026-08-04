# Drawio Processor

## Purpose

Converts [Draw.io](https://www.drawio.com/) diagram files to PNG, SVG, or PDF.

## How It Works

Discovers `.drawio` files in the project and runs `drawio` in export mode on
each file, generating output in the configured formats.

## Source Files

- Input: `**/*.drawio`
- Output: `out/drawio/{format}/{relative_path}.{format}`

## Configuration

```toml
[processor.drawio]
command = "drawio"                     # The drawio command to run
formats = ["png"]                      # Output formats (png, svg, pdf)
args = []                              # Additional arguments to pass to drawio
output_dir = "out/drawio"              # Output directory
dep_inputs = []                      # Additional files that trigger rebuilds when changed
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"drawio"` | The drawio executable to run |
| `formats` | string[] | `["png"]` | Output formats to generate (`png`, `svg`, `pdf`) |
| `args` | string[] | `[]` | Extra arguments passed to drawio |
| `output_dir` | string | `"out/drawio"` | Base output directory |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
