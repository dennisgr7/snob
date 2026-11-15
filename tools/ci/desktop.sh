#!/usr/bin/env bash
# What the desktop app's container runs: the Linux lines of
# .github/workflows/desktop.yml, in the same order. Keep the two in step.
#
# Arguments go to the harness in place of `quick`: `bash tools/ci/run-desktop.sh
# behavior` runs one suite.
set -euo pipefail
cd /src/apps/desktop

cargo fmt --check
cargo clippy --all-targets --features testing --locked -- -D warnings
cargo clippy --all-targets --locked -- -D warnings

# A display that exists nowhere, with Mesa's Vulkan on the CPU under it. What
# the run writes stays in the container's own target directory: a scenario's
# root holds a socket, which a directory shared with a Windows host cannot.
(( $# )) || set -- quick
xvfb-run -a cargo run --quiet --locked --features testing --example harness -- "$@"
