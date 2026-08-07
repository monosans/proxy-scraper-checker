#!/usr/bin/env bash
# Alpine/musl wrapper: runs the identical measurement script inside a musl
# container so the Linux and Alpine rows are produced by the same code.
#
# Two things to keep in mind when touching it. Do not install the toolchain
# with `apk add rust cargo` under `sh -lc`: that profile resets PATH and
# shadows the image's rustup toolchain, so the Alpine rows come out built by a
# different compiler than every other row. And do not build into the
# bind-mounted ./target, or the container leaves root-owned artifacts in the
# workspace.
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
    # python3 and openssl serve the tls workload (bench/tls_server.py); without
    # them that workload skips itself with a warning and check/scrape still run.
    apk add --no-cache bash build-base pkgconfig time python3 openssl
    exec bash .github/scripts/allocator_bench_unix.sh
  '

# The container runs as root; hand the results back to the runner user so
# actions/upload-artifact can read them.
sudo chown -R "$(id -u):$(id -g)" . 2>/dev/null || true

# Cosmetic, like the summary itself: the measured rows live in
# bench-results.tsv and are uploaded regardless.
if [ -f alpine-summary.md ]; then
  cat alpine-summary.md >> "$GITHUB_STEP_SUMMARY"
else
  echo "::warning::no alpine-summary.md (results are still in bench-results.tsv)"
fi
