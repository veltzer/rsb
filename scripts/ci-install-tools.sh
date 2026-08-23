#!/bin/bash
# Install the external tools this repo's tests shell out to. The canonical
# ci.yml runs this in every repo right after `cargo build`; a repo whose
# tests need no external tools keeps this script as an explicit no-op.
# Keep it strict: a tool that fails to install must fail the build here,
# not surface later as a confusing test failure.
set -euo pipefail

# The whole tool matrix, in one registry-driven command. rsconstruct itself
# is built by the workflow's `cargo build` step; every external tool comes
# from its own registry, so adding a processor never requires touching CI.
#
# Nothing is skipped and nothing is filtered — `--all` treats a tool it
# cannot install automatically as a hard error rather than a warning, so a
# registry entry that loses its install method fails this step instead of
# quietly shrinking the matrix.
./target/debug/rsconstruct tools install --all
