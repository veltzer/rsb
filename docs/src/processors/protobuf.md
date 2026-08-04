# Protobuf Processor

## Purpose

Compiles Protocol Buffer (`.proto`) files to generated source code using `protoc`.

## How It Works

Files matching configured extensions in the `proto/` directory are compiled using the
Protocol Buffer compiler. Output is written to `out/protobuf/`:

```
proto/hello.proto  →  out/protobuf/hello.pb.cc
```

The `--proto_path` is automatically set to the parent directory of each input file.

## Source Files

- Input: `proto/**/*.proto`
- Output: `out/protobuf/` with `.pb.cc` extension

## Configuration

```toml
[processor.protobuf]
command = "protoc"                        # Protoc binary (default: "protoc")
src_extensions = [".proto"]                   # File extensions to process
output_dir = "out/protobuf"              # Output directory (default: "out/protobuf")
dep_inputs = []                         # Additional files that trigger rebuilds
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"protoc"` | Path to protoc compiler |
| `src_extensions` | string[] | `[".proto"]` | File extensions to discover |
| `output_dir` | string | `"out/protobuf"` | Output directory |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |

## Batch support

Each input file is processed individually, producing its own output file.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
