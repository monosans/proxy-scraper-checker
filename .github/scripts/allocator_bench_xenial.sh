#!/usr/bin/env bash
# Old-glibc wrapper: builds the binary the way releases are built (cargo-zigbuild
# against glibc 2.17) on the runner, then measures it inside Ubuntu 16.04, whose
# glibc 2.23 has no malloc tcache. The kernel is the runner's either way, so the
# libc is the only thing that differs from the ubuntu-26.04 rows.
#
# Expects cargo-zigbuild on PATH and uv on PATH (the workflow installs both).
set -euo pipefail

target="$(uname -m)-unknown-linux-gnu.2.17"

BENCH_PHASE=build BENCH_CARGO=zigbuild BENCH_TARGET="$target" \
  bash .github/scripts/allocator_bench_unix.sh

# uv supplies a current Python for bench/tls_server.py, which needs 3.7+ while
# 16.04 ships 3.5; the Python only serves the tls workload and is never measured.
# The runner's own uv can't go in: its aarch64 build needs glibc 2.28. The
# official image of the same version carries a static musl build instead.
uv_image="ghcr.io/astral-sh/uv:$(uv --version | cut -d' ' -f2)"
uv_container="$(docker create "$uv_image")"
docker cp "$uv_container:/uv" bench-uv
docker rm "$uv_container" >/dev/null

docker run --rm \
  -v "$PWD:/work" \
  -v "$PWD/bench-uv:/usr/local/bin/uv:ro" \
  -w /work \
  -e ALLOCATOR \
  -e TOKIO_MULTI_THREAD \
  -e WORKLOADS \
  -e ALLOC_ENVS \
  -e PLATFORM_LABEL \
  -e RUN_ID \
  -e REPS \
  -e WARMUPS \
  -e BENCH_PHASE=run \
  -e BENCH_TARGET="$target" \
  -e BENCH_TOOLCHAIN="$(rustc -V | cut -d' ' -f2)" \
  -e GITHUB_STEP_SUMMARY=/work/xenial-summary.md \
  ubuntu:16.04 bash -euc '
    # ca-certificates because rustls-platform-verifier refuses to start
    # without a system trust store, which any real install has.
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      ca-certificates time openssl >/dev/null
    uv venv -q --managed-python --python 3 /opt/py
    PATH="/opt/py/bin:$PATH" exec bash .github/scripts/allocator_bench_unix.sh
  '

# The container runs as root; hand the results back to the runner user so
# actions/upload-artifact can read them.
sudo chown -R "$(id -u):$(id -g)" . 2>/dev/null || true

# Cosmetic, like the summary itself: the measured rows live in
# bench-results.tsv and are uploaded regardless.
if [ -f xenial-summary.md ]; then
  cat xenial-summary.md >> "$GITHUB_STEP_SUMMARY"
else
  echo "::warning::no xenial-summary.md (results are still in bench-results.tsv)"
fi
