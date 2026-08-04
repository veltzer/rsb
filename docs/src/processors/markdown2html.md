# Markdown2html Processor

## Purpose

Converts Markdown files to HTML using the [markdown](https://daringfireball.net/projects/markdown/) Perl script.

## How It Works

Discovers `.md` files in the project and runs `markdown` on each file,
producing an HTML output file.

## Source Files

- Input: `**/*.md`
- Output: `out/markdown2html/{relative_path}.html`

## Configuration

```toml
[processor.markdown2html]
command = "markdown"                   # The markdown command to run
args = []                              # Additional arguments to pass to markdown
output_dir = "out/markdown2html"       # Output directory
dep_inputs = []                      # Additional files that trigger rebuilds when changed
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"markdown"` | The markdown executable to run |
| `args` | string[] | `[]` | Extra arguments passed to markdown |
| `output_dir` | string | `"out/markdown2html"` | Output directory for HTML files |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
