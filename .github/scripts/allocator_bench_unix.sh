#!/usr/bin/env bash
set -euo pipefail

TOKIO_FEATURE=""
if [[ "${TOKIO_MULTI_THREAD:-false}" == "true" ]]; then
  TOKIO_FEATURE="tokio-multi-thread"
fi

allocators=(system jemalloc mimalloc_v2 mimalloc_v3)

if [[ "$RUNNER_OS" == "Linux" ]]; then
  time_cmd=(/usr/bin/time -v)
  parse_peak() {
    awk -F': ' '/Maximum resident set size/ {print $2; exit}'
  }
else
  time_cmd=(/usr/bin/time -l)
  parse_peak() {
    awk '/maximum resident set size/ {print $1; exit}'
  }
fi

build_features() {
  local allocator="$1"
  local features=""
  if [[ "$allocator" != "system" ]]; then
    features="$allocator"
  fi
  if [[ -n "$TOKIO_FEATURE" ]]; then
    if [[ -n "$features" ]]; then
      features="$features,$TOKIO_FEATURE"
    else
      features="$TOKIO_FEATURE"
    fi
  fi
  echo "$features"
}

run_one() {
  local allocator="$1"
  local features
  features="$(build_features "$allocator")"

  local feature_args=()
  if [[ -n "$features" ]]; then
    feature_args=(--features "$features")
  fi

  local output
  output="$(${time_cmd[@]} cargo run --release --locked "${feature_args[@]}" 2>&1 >/dev/null)"

  local peak
  peak="$(echo "$output" | parse_peak)"
  if [[ -z "$peak" ]]; then
    echo "Failed to parse peak memory for $allocator" >&2
    exit 1
  fi

  if [[ "$RUNNER_OS" != "Linux" ]]; then
    peak=$((peak / 1024))
  fi

  printf "%s\t%s\n" "$allocator" "$peak" >> results.tsv
}

if [[ "$RUNNER_OS" == "Windows" ]]; then
  allocators=(system mimalloc_v2 mimalloc_v3)
fi

: > results.tsv
for allocator in "${allocators[@]}"; do
  run_one "$allocator"
done

{
  echo "### ${PLATFORM_LABEL:-unknown} (tokio-multi-thread=${TOKIO_MULTI_THREAD:-false})"
  echo ""
  echo "| Allocator | Peak KB |"
  echo "| --- | ---: |"
  sort -n -k2,2 results.tsv | while IFS=$'\t' read -r allocator peak; do
    echo "| $allocator | $peak |"
  done
  best="$(sort -n -k2,2 results.tsv | head -n1)"
  best_allocator="${best%%$'\t'*}"
  best_peak="${best#*$'\t'}"
  echo ""
  echo "**Best:** $best_allocator ($best_peak KB)"
} >> "$GITHUB_STEP_SUMMARY"
