# Binary Releases

RSConstruct publishes pre-built binaries as GitHub releases when a version tag
(`v*`) is pushed.

## Supported Platforms

| Platform | Binary name |
|---|---|
| Linux x86_64 | `rsconstruct-linux-x86_64` |
| Linux aarch64 (arm64) | `rsconstruct-linux-aarch64` |
| macOS x86_64 | `rsconstruct-macos-x86_64` |
| macOS aarch64 (Apple Silicon) | `rsconstruct-macos-aarch64` |

RSConstruct is unix-only. There is no Windows build: the release matrix in
`.github/workflows/release.yml` has never contained a Windows target, and the
codebase assumes unix throughout (`flock`, `/dev/null`, `$HOME`, apt-based
tool installation).

## How It Works

The release workflow (`.github/workflows/release.yml`) has two jobs:

1. **build** — a matrix job that builds the release binary for each platform
   and uploads it as a GitHub Actions artifact.
2. **release** — waits for all builds to finish, downloads the artifacts,
   and creates a GitHub release with auto-generated release notes and all
   binaries attached.

## Creating a Release

1. Update `version` in `Cargo.toml`
2. Commit and push
3. Tag and push: `git tag v0.2.2 && git push origin v0.2.2`
4. The workflow creates the GitHub release automatically

## Release Profile

The binary is optimized for size and performance:

```toml
[profile.release]
strip = true        # Remove debug symbols
lto = true          # Link-time optimization across all crates
codegen-units = 1   # Single codegen unit for better optimization
```
