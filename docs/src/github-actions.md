# GitHub Actions

How to run rsconstruct in a GitHub Actions workflow.

## Recommended flags

```yaml
- name: Build
  run: rsconstruct build -q -j0
```

| Flag | Why |
|------|-----|
| `-q` (quiet) | Suppresses the progress bar and status messages. The progress bar uses terminal escape codes that produce garbage in CI logs. Only errors are shown. |
| `-j0` | Auto-detect CPU cores. GitHub-hosted runners have 4 cores (`ubuntu-latest`) — using them all speeds up the build significantly vs the default of `-j1`. |

## Full workflow example

```yaml
name: Build
on: [push, pull_request]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install rsconstruct
        run: cargo install rsconstruct

      - name: Install tools
        run: rsconstruct tools install

      - name: Build
        run: rsconstruct build -q -j0
```

## Runner sizing

| Runner | Cores | RAM | Notes |
|--------|-------|-----|-------|
| `ubuntu-latest` | 4 | 16 GB | Good for most projects. Use `-j0` or `-j4`. |
| `ubuntu-latest` (private repo) | 4 | 16 GB | Same hardware as public repos. |
| Large runners | 8-64 | 32-256 GB | For large projects. `-j0` scales automatically. |

`-j0` always does the right thing — it detects the available cores at runtime.
There is no benefit to setting `-j` higher than the core count.

## Caching

Cache the `.rsconstruct/` directory between runs to skip unchanged products:

```yaml
      - uses: actions/cache@v4
        with:
          path: .rsconstruct
          key: rsconstruct-${{ hashFiles('rsconstruct.toml') }}-${{ github.sha }}
          restore-keys: |
            rsconstruct-${{ hashFiles('rsconstruct.toml') }}-
            rsconstruct-
```

This restores cached build products from previous runs. Only products whose
inputs changed will be rebuilt.

## Tips

- **Don't use `--timings` in CI** unless you need the data. It adds overhead.
- **Use `--json`** instead of `-q` if you want machine-readable output for downstream processing.
- **Use `-k` (keep-going)** to see all failures at once instead of stopping at the first one.
- **Use `--verify-tool-versions`** to catch tool version drift between local and CI environments.
