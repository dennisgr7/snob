#!/usr/bin/env bash
# Run the desktop app's Linux job locally. From the repository root:
#
#     bash tools/ci/run-desktop.sh            # lints, then `cargo harness quick`
#     bash tools/ci/run-desktop.sh behavior   # lints, then one suite
#
# The image is the command line's (tools/ci/Dockerfile) with what GPUI links
# and a display that exists nowhere (tools/ci/desktop.Dockerfile). The first
# run compiles GPUI from cold; the volumes keep the registry and the target
# directory between runs. CARGO_BUILD_JOBS is bounded as in run.sh, for the
# reason given there.
set -euo pipefail
cd "$(dirname "$0")/../.."

root="$(pwd -W 2>/dev/null || pwd)"
export MSYS_NO_PATHCONV=1

read -r mem_bytes cpus < <(docker info --format '{{.MemTotal}} {{.NCPU}}')
jobs=$(( mem_bytes / (2 * 1024 * 1024 * 1024) ))
(( jobs < 1 )) && jobs=1
(( jobs > cpus )) && jobs=$cpus

docker build -q -t snob-ci tools/ci >/dev/null
docker build -q -t snob-ci-desktop -f tools/ci/desktop.Dockerfile tools/ci >/dev/null
exec docker run --rm \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$jobs}" \
    -v "$root:/src" \
    -v snob-ci-cargo:/cargo \
    -v snob-ci-target-desktop:/target-desktop \
    snob-ci-desktop bash /src/tools/ci/desktop.sh "$@"
