# Libreoffice Processor

## Purpose

Converts LibreOffice documents (e.g., `.odp` presentations) to PDF or other formats.

## How It Works

Discovers `.odp` files in the project and runs `libreoffice` in headless mode
to convert each file to the configured output formats. Uses `flock` to serialize
invocations since LibreOffice only supports a single running instance.

## Source Files

- Input: `**/*.odp`
- Output: `out/libreoffice/{format}/{relative_path}.{format}`

## Configuration

```toml
[processor.libreoffice]
command = "libreoffice"                # The libreoffice command to run
formats = ["pdf"]                      # Output formats (pdf, pptx)
args = []                              # Additional arguments to pass to libreoffice
output_dir = "out/libreoffice"         # Output directory
dep_inputs = []                      # Additional files that trigger rebuilds when changed
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"libreoffice"` | The libreoffice executable to run |
| `formats` | string[] | `["pdf"]` | Output formats to generate (`pdf`, `pptx`) |
| `args` | string[] | `[]` | Extra arguments passed to libreoffice |
| `output_dir` | string | `"out/libreoffice"` | Base output directory |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
