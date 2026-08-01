#!/usr/bin/env bash
# Alpine/musl wrapper: runs the identical measurement script inside a musl
# container so the Linux and Alpine rows are produced by the same code.
#
# Fixes two things the previous version got wrong:
#   - it ran `apk add rust cargo` inside `sh -lc`, whose /etc/profile resets
#     PATH and shadowed the image's rustup toolchain, so the Alpine rows were
#     built by a completely different compiler than every other row;
#   - it built into the bind-mounted ./target as root, leaving root-owned
#     artifacts in the workspace.
set -euo pipefail

RUST_IMAGE="${RUST_IMAGE:-rust:1-alpine}"

docker run --rm \
  -v "$PWD:/work" \
  -w /work \
  -e ALLOCATOR \
  -e TOKIO_MULTI_THREAD \
  -e WORKLOADS \
  -e ALLOC_ENVS \
  -e PLATFORM_LABEL \
  -e RUN_ID \
  -e REPS \
  -e WARMUPS \
  -e CARGO_TARGET_DIR=/work/target-alpine \
  -e GITHUB_STEP_SUMMARY=/work/alpine-summary.md \
  "$RUST_IMAGE" sh -euc '
    apk add --no-cache bash build-base pkgconfig time
    exec bash .github/scripts/allocator_bench_unix.sh
  '

# The container runs as root; hand the results back to the runner user so
# actions/upload-artifact can read them.
sudo chown -R "$(id -u):$(id -g)" . 2>/dev/null || true

cat alpine-summary.md >> "$GITHUB_STEP_SUMMARY"
