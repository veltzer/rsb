#!/bin/bash
# Install the system libraries this repo's build links against and the
# external tools its tests shell out to. The canonical ci.yml runs this in
# every repo before `cargo build`; a repo that needs neither keeps this
# script as an explicit no-op. Keep it strict: anything that fails to
# install must fail the build here, not surface later as a confusing
# build or test failure.
set -euo pipefail

# The whole tool matrix, in one registry-driven command. `cargo run` builds
# rsconstruct first (the workflow's later Build step then reuses the same
# artifacts); every external tool comes from its own registry, so adding a
# processor never requires touching CI.
#
# Nothing is skipped and nothing is filtered — `--all` treats a tool it
# cannot install automatically as a hard error rather than a warning, so a
# registry entry that loses its install method fails this step instead of
# quietly shrinking the matrix.
#
# drawio's .deb URL is resolved from the GitHub releases API at install time,
# so this inherits GITHUB_TOKEN from the workflow step to stay under the
# authenticated rate limit (see the comment on that step in ci.yml). Unset
# locally, where one machine never approaches the anonymous 60/hour.
cargo run -- tools install --all
