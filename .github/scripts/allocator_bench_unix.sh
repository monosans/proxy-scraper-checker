#!/usr/bin/env bash
# Allocator benchmark, Linux/macOS/Alpine. Builds one binary, runs it
# WARMUPS+REPS times against a frozen local corpus, and appends one TSV row per
# measured repetition to bench-results.tsv.
#
# Inputs (env):
#   ALLOCATOR          system | auto | jemalloc | jemalloc_override |
#                      mimalloc_v2 | mimalloc_v2_no_thp | mimalloc_v3 |
#                      mimalloc_v3_no_thp | mimalloc_v3_override
#   TOKIO_MULTI_THREAD true | false
#   WORKLOADS          space-separated subset of "check scrape"
#   ALLOC_ENVS         ';'-separated run-time tuning variants, each
#                      "name:VAR=VAL[ VAR=VAL...]" or just "default"
#   PLATFORM_LABEL     matrix label, recorded verbatim
#   RUN_ID             identifies one dispatch of the whole matrix
#   REPS               measured repetitions (default 5)
#   WARMUPS            discarded leading repetitions (default 1)
#
# Workloads and tuning variants are loops inside the job, not matrix axes:
# neither needs a rebuild, and the build is what costs minutes.
set -euo pipefail

REPS="${REPS:-5}"
WARMUPS="${WARMUPS:-1}"
WORKLOADS="${WORKLOADS:-check scrape}"
ALLOC_ENVS="${ALLOC_ENVS:-default}"
RUN_ID="${RUN_ID:-0}"
PLATFORM_LABEL="${PLATFORM_LABEL:-unknown}"
TOKIO_MULTI_THREAD="${TOKIO_MULTI_THREAD:-false}"

# --- feature selection -----------------------------------------------------
# "system" and "auto" are not features. "system" is --no-default-features with
# nothing added; "auto" is the shipped default feature set, which nothing in
# the previous benchmark ever built even though it is what users actually get.
features=""
no_default="--no-default-features"
case "$ALLOCATOR" in
  system) ;;
  auto) no_default="" ;;
  mimalloc_v3_override) features="mimalloc_v3,mimalloc_override" ;;
  *) features="$ALLOCATOR" ;;
esac
if [ "$TOKIO_MULTI_THREAD" = "true" ]; then
  features="${features:+$features,}tokio-multi-thread"
fi

build_args=(build --release --locked)
[ -n "$no_default" ] && build_args+=("$no_default")
[ -n "$features" ] && build_args+=(--features "$features")

echo "::group::cargo ${build_args[*]}"
cargo "${build_args[@]}"
echo "::endgroup::"

# The Alpine wrapper redirects CARGO_TARGET_DIR so the container's root-owned
# artifacts stay out of the runner's ./target.
exe="${CARGO_TARGET_DIR:-target}/release/proxy-scraper-checker"

# --- platform probes -------------------------------------------------------
# Recorded per row because they are what makes a row interpretable. The THP
# state in particular decides whether mimalloc's no_thp variants can do
# anything at all, and the previous harness never captured it.
uname_s="$(uname -s)"
arch="$(uname -m)"
case "$uname_s" in
  Linux)
    cores="$(nproc --all)"
    page_kb="$(( $(getconf PAGE_SIZE) / 1024 ))"
    thp="$(sed -n 's/.*\[\([a-z]*\)\].*/\1/p' \
             /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || true)"
    if [ -f /etc/alpine-release ]; then libc=musl; else libc=glibc; fi
    pf_kind=linux_getrusage
    peak_extra_kind=none
    ;;
  Darwin)
    cores="$(sysctl -n hw.logicalcpu)"
    page_kb="$(( $(getconf PAGE_SIZE) / 1024 ))"
    thp="n/a"
    libc=darwin
    pf_kind=macos_getrusage
    peak_extra_kind=macos_footprint
    ;;
  *) echo "unsupported platform: $uname_s" >&2; exit 1 ;;
esac
thp="${thp:-unknown}"

toolchain="$(rustc -V | cut -d' ' -f2)"
if command -v sha256sum >/dev/null 2>&1; then
  exe_sha="$(sha256sum "$exe" | cut -c1-16)"
else
  exe_sha="$(shasum -a 256 "$exe" | cut -c1-16)"
fi

results=bench-results.tsv
if [ ! -f "$results" ]; then
  printf 'run_id\tcell_id\trep\tplatform\tlibc\tarch\tcores\ttoolchain\tallocator\talloc_env\tmt\tworkload\twall_ms\tcpu_user_ms\tcpu_sys_ms\tpeak_rss_kb\tpeak_extra_kb\tpeak_extra_kind\tmajor_pf\tminor_pf\tpf_kind\texit_code\tproxies_checked\tproxies_out\texe_sha\tthp\tpage_kb\n' > "$results"
fi

# is_container() forces ./out regardless of output.path, so probe both.
out_dir=bench-out
[ -f /.dockerenv ] && out_dir=out

to_ms() { awk -v v="$1" 'BEGIN { printf "%d", v * 1000 }'; }

cells=""
total=$(( WARMUPS + REPS ))

for WORKLOAD in $WORKLOADS; do
export PROXY_SCRAPER_CHECKER_CONFIG="bench/config.${WORKLOAD}.toml"

old_ifs="$IFS"; IFS=';'
for ALLOC_ENV in $ALLOC_ENVS; do
IFS="$old_ifs"

alloc_env_name="${ALLOC_ENV%%:*}"
alloc_env_vars=""
[ "$ALLOC_ENV" != "$alloc_env_name" ] && alloc_env_vars="${ALLOC_ENV#*:}"

cell_id="${PLATFORM_LABEL}|${ALLOCATOR}|mt=${TOKIO_MULTI_THREAD}|${WORKLOAD}|${alloc_env_name}"
cells="${cells}${cell_id}
"
echo "--- $cell_id (env: ${alloc_env_vars:-none}) ---"

for i in $(seq 1 "$total"); do
  log="bench-${WORKLOAD}-${alloc_env_name}-${i}.log"
  timing="bench-time-${WORKLOAD}-${alloc_env_name}-${i}.txt"

  set +e
  # ALLOC_ENV is deliberately word-split: it carries zero or more VAR=VAL pairs.
  # shellcheck disable=SC2086
  if [ "$uname_s" = "Linux" ]; then
    env $alloc_env_vars /usr/bin/time \
      -f '%e\t%U\t%S\t%M\t%R\t%F\t%x' -o "$timing" \
      -- "$exe" >"$log" 2>&1
  else
    # shellcheck disable=SC2086
    env $alloc_env_vars /usr/bin/time -l -- "$exe" >"$log" 2>"$timing"
  fi
  rc=$?
  set -e

  if [ "$uname_s" = "Linux" ]; then
    IFS=$'\t' read -r wall user sys peak_rss_kb minor major exit_code \
      < "$timing"
    peak_extra_kb=0
  else
    # BSD time has no -f. Line 1 carries the timings, which the previous
    # harness discarded entirely.
    set -- $(awk '/real/ && /user/ && /sys/ { print $1, $3, $5; exit }' \
               "$timing")
    wall="$1"; user="$2"; sys="$3"
    # BSD time reports bytes; GNU time reports KB.
    peak_rss_kb=$(( $(awk '/maximum resident set size/ { print $1; exit }' \
                        "$timing") / 1024 ))
    # phys_footprint: includes compressed memory, excludes clean file-backed
    # pages. Recorded alongside max RSS, never instead of it.
    peak_extra_kb=$(( $(awk '/peak memory footprint/ { print $1; exit }' \
                          "$timing") / 1024 ))
    minor="$(awk '/page reclaims/ { print $1; exit }' "$timing")"
    major="$(awk '/ page faults/ { print $1; exit }' "$timing")"
    exit_code=$rc
  fi

  wall_ms="$(to_ms "$wall")"
  user_ms="$(to_ms "$user")"
  sys_ms="$(to_ms "$sys")"

  # Work-done counters. A clean exit 0 that scraped nothing is the failure mode
  # the previous harness could not see; the aggregator additionally asserts
  # these agree across every rep and every cell of a platform.
  proxies_checked="$(sed -n \
    's/.*Started checking \([0-9][0-9]*\) proxies.*/\1/p' "$log" | head -n 1)"
  proxies_checked="${proxies_checked:-0}"
  if [ -f "$out_dir/proxies/all.txt" ]; then
    proxies_out="$(wc -l < "$out_dir/proxies/all.txt" | tr -d ' ')"
  else
    proxies_out=0
  fi

  if [ "$exit_code" != "0" ]; then
    echo "rep $i exited with $exit_code" >&2
    tail -n 40 "$log" "$timing" >&2
    exit 1
  fi
  if [ "$WORKLOAD" = "check" ] && [ "$proxies_checked" -eq 0 ]; then
    echo "rep $i checked no proxies - corpus or config is wrong" >&2
    tail -n 40 "$log" "$timing" >&2
    exit 1
  fi
  if [ "$WORKLOAD" = "scrape" ] && [ "$proxies_out" -eq 0 ]; then
    echo "rep $i wrote no proxies - corpus or config is wrong" >&2
    tail -n 40 "$log" "$timing" >&2
    exit 1
  fi

  if [ "$i" -le "$WARMUPS" ]; then
    continue
  fi

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$RUN_ID" "$cell_id" "$(( i - WARMUPS ))" "$PLATFORM_LABEL" "$libc" \
    "$arch" "$cores" "$toolchain" "$ALLOCATOR" "$alloc_env_name" \
    "$TOKIO_MULTI_THREAD" "$WORKLOAD" "$wall_ms" "$user_ms" "$sys_ms" \
    "$peak_rss_kb" "$peak_extra_kb" "$peak_extra_kind" "$major" "$minor" \
    "$pf_kind" "$exit_code" "$proxies_checked" "$proxies_out" "$exe_sha" \
    "$thp" "$page_kb" >> "$results"
done

old_ifs="$IFS"; IFS=';'
done
IFS="$old_ifs"
done

echo "$cells" | while read -r cell; do
  if [ -n "$cell" ]; then
    sh .github/scripts/allocator_bench_report.sh "$results" "$cell"
  fi
done
