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

## GitHub Pages deployment

A repo that publishes its build output to GitHub Pages (with the Pages source
set to "GitHub Actions") declares the published directory in `rsconstruct.toml`:

```toml
[pages]
dir = "out/web"
```

The workflow then asks `rsconstruct pages dir` whether to deploy. The command
prints the directory when `[pages]` is configured and prints nothing (exit 0)
when it isn't, so the exact same workflow file works in Pages and non-Pages
repos — the upload step and the deploy job simply skip when the output is
empty:

```yaml
jobs:
  build:
    runs-on: ubuntu-24.04
    permissions:
      contents: read
      # needed by the stale-artifact cleanup below
      actions: write
    outputs:
      pages-dir: ${{ steps.pages.outputs.dir }}
    steps:
    # ... checkout, install rsconstruct, build ...
    - name: pages dir
      id: pages
      run: echo "dir=$(rsconstruct pages dir)" >> "$GITHUB_OUTPUT"
    - name: delete stale pages artifact
      # Artifacts survive re-run attempts, and both the upload and
      # deploy-pages fail hard when a "github-pages" artifact already exists
      # in the run. Delete any leftover from a previous attempt first.
      if: steps.pages.outputs.dir != ''
      run: |
        gh api --paginate "repos/${{ github.repository }}/actions/runs/${{ github.run_id }}/artifacts" \
          --jq '.artifacts[] | select(.name == "github-pages") | .id' |
        while read -r artifact_id; do
          echo "deleting stale artifact $artifact_id"
          gh api -X DELETE "repos/${{ github.repository }}/actions/artifacts/$artifact_id"
        done
      env:
        GH_TOKEN: ${{ github.token }}
    - uses: actions/upload-pages-artifact@v5
      if: steps.pages.outputs.dir != ''
      with:
        path: ${{ steps.pages.outputs.dir }}

  deploy:
    needs: build
    if: needs.build.outputs.pages-dir != ''
    runs-on: ubuntu-24.04
    permissions:
      pages: write
      id-token: write
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
    - uses: actions/deploy-pages@v5
      id: deployment
```

Permissions are scoped at the job level, so non-Pages repos running this
workflow never grant `pages: write` or `id-token: write` to anything that
executes, and the skipped `deploy` job does not create a `github-pages`
environment.

## Tips

- **Don't use `--timings` in CI** unless you need the data. It adds overhead.
- **Use `--json`** instead of `-q` if you want machine-readable output for downstream processing.
- **Use `-k` (keep-going)** to see all failures at once instead of stopping at the first one.
- **Use `--verify-tool-versions`** to catch tool version drift between local and CI environments.
