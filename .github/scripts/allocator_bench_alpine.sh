#!/usr/bin/env bash
set -euo pipefail

TOKIO_FEATURE=""
if [[ "${TOKIO_MULTI_THREAD:-false}" == "true" ]]; then
  TOKIO_FEATURE="tokio-multi-thread"
fi

ALPINE_SCRIPT=$(cat <<'EOF'
set -eu
apk add --no-cache build-base pkgconfig time

build_features() {
  allocator="$1"
  features=""
  if [ "$allocator" != "system" ]; then
    features="$allocator"
  fi
  if [ -n "$TOKIO_FEATURE" ]; then
    if [ -n "$features" ]; then
      features="$features,$TOKIO_FEATURE"
    else
      features="$TOKIO_FEATURE"
    fi
  fi
  echo "$features"
}

: > /work/alpine-results.tsv
for allocator in system jemalloc mimalloc_v2 mimalloc_v3; do
  features="$(build_features "$allocator")"
  if [ -n "$features" ]; then
    output="$(/usr/bin/time -v cargo run --release --locked --features "$features" 2>&1 >/dev/null)"
  else
    output="$(/usr/bin/time -v cargo run --release --locked 2>&1 >/dev/null)"
  fi
  peak="$(echo "$output" | awk -F': ' '/Maximum resident set size/ {print $2; exit}')"
  if [ -z "$peak" ]; then
    echo "Failed to parse peak memory for $allocator" >&2
    exit 1
  fi
  printf "%s\t%s\n" "$allocator" "$peak" >> /work/alpine-results.tsv
done
EOF
)

docker run --rm \
  -v "$PWD:/work" \
  -w /work \
  -e TOKIO_FEATURE="$TOKIO_FEATURE" \
  -e PLATFORM_LABEL="${PLATFORM_LABEL:-unknown}" \
  rust:alpine sh -lc "$ALPINE_SCRIPT"

{
  echo "### ${PLATFORM_LABEL:-unknown} (tokio-multi-thread=${TOKIO_MULTI_THREAD:-false})"
  echo ""
  echo "| Allocator | Peak KB |"
  echo "| --- | ---: |"
  sort -n -k2,2 alpine-results.tsv | while IFS=$'\t' read -r allocator peak; do
    echo "| $allocator | $peak |"
  done
  best="$(sort -n -k2,2 alpine-results.tsv | head -n1)"
  best_allocator="${best%%$'\t'*}"
  best_peak="${best#*$'\t'}"
  echo ""
  echo "**Best:** $best_allocator ($best_peak KB)"
} >> "$GITHUB_STEP_SUMMARY"
