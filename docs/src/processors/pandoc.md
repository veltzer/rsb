# Pandoc Processor

## Purpose

Converts documents between formats using [pandoc](https://pandoc.org/).

## How It Works

Discovers `.md` files in the project (default `pandoc/` subdirectory) and runs
`pandoc` on each one, producing one output per format listed in `formats`.

## Source Files

- Input: `pandoc/**/*.md` (configurable via `src_dirs` / `src_extensions`)
- Output: `out/pandoc/{relative_path}.{format}` for each `format`

## Configuration

```toml
[processor.pandoc]
command = "pandoc"             # Path to the pandoc executable
formats = ["pdf", "html", "docx"]
args = []                      # Extra arguments passed to pandoc
output_dir = "out/pandoc"
dep_inputs = []                # Additional files that trigger rebuilds when changed
pdf_engine = ""                # PDF engine; empty = pandoc default (pdflatex)
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"pandoc"` | Path to the pandoc executable |
| `formats` | string[] | `["pdf", "html", "docx"]` | Output formats to generate |
| `args` | string[] | `[]` | Extra arguments passed to pandoc |
| `output_dir` | string | `"out/pandoc"` | Base output directory |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |
| `pdf_engine` | string | `""` | Forwarded to `--pdf-engine=` when generating PDFs. See below. |

## PDF engine selection

Pandoc's default engine for PDF output is **pdflatex**, which only handles
8-bit-encoded text. Source files containing characters outside latin-1
(Hebrew, Greek, Cyrillic, CJK, accented Unicode like `λ`, `ש`, `Ω`, `é`) will
fail to compile under pdflatex without manual `\usepackage{...}` setup.

Set `pdf_engine` to use a Unicode-aware engine instead:

```toml
[processor.pandoc]
formats = ["pdf"]
pdf_engine = "xelatex"   # or "lualatex"
```

Recognized values:

| Value | When to use |
|-------|-------------|
| `""` (default) | Pandoc's default (pdflatex). ASCII / latin-1 only. |
| `pdflatex` | Same as default; explicit form. |
| `xelatex` | Unicode-aware. Best general-purpose choice for non-ASCII text. |
| `lualatex` | Unicode-aware, embeds a Lua interpreter for advanced typesetting. |
| `tectonic` | Self-contained engine that downloads packages on demand. |
| `wkhtmltopdf`, `weasyprint`, `prince` | HTML-based PDF rendering pipelines. |
| `context` | ConTeXt-based typesetting. |

Notes:

- `pdf_engine` only affects products whose output format is `pdf`. For other
  formats (html, docx, etc.) the field is silently ignored.
- The configured engine is added to the processor's required-tools list, so
  `rsconstruct doctor` and `rsconstruct tools list-configured` will flag it as missing if
  it isn't installed.
- Changing `pdf_engine` invalidates the cache for affected products (it is
  part of the config-change checksum), so a rebuild is triggered automatically.
- An unknown value is rejected at config-load time with a clear error; the
  build never starts.
- The pdflatex-specific `\pdftrailerid{}` reproducibility hint is only
  injected when the engine is pdflatex (or the empty default). Other engines
  would error on it.

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

Pandoc is a Generator — see [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
