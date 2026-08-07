#!/usr/bin/env bash
# Allocator benchmark, Linux/macOS/Alpine. Builds one binary, runs it
# WARMUPS+REPS times against a frozen local corpus, and appends one TSV row per
# measured repetition to bench-results.tsv.
#
# Inputs (env):
#   ALLOCATOR          system | auto | jemalloc | jemalloc_override |
#                      mimalloc_v2 | mimalloc_v3 | mimalloc_v3_override,
#                      optionally with a "_dup" suffix, which builds the same
#                      binary under a second job so the report can show the
#                      between-job noise for that allocator. The suffix is
#                      recorded but never reaches the feature list.
#   TOKIO_MULTI_THREAD true | false
#   WORKLOADS          space-separated subset of "check scrape tls"
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
# nothing added; "auto" is the shipped default feature set, which is what users
# actually get and so has to be measured.
features=""
no_default="--no-default-features"
# A "_dup" cell builds the identical binary under a second job so the report can
# show the between-job noise for that allocator rather than assume it. Only the
# recorded label carries the suffix; strip it once, here, and use the stripped
# name for every feature decision below. Stripping it in the `case` subject
# alone is not enough, because the default branch has to use it too.
allocator_features="${ALLOCATOR%_dup}"
case "$allocator_features" in
  system) ;;
  auto) no_default="" ;;
  mimalloc_v3_override) features="mimalloc_v3,mimalloc_override" ;;
  *) features="$allocator_features" ;;
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
# Recorded per row because a row cannot be interpreted without them. The THP
# state in particular decides whether mimalloc's no_thp variants can do
# anything at all.
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

# --- tls workload support --------------------------------------------------
# Must match check_url in bench/config.tls.toml and the port baked into
# bench/corpus/tls_http.txt.
TLS_PORT=18443
tls_pid=""

tls_server_stop() {
  if [ -n "$tls_pid" ]; then
    kill "$tls_pid" 2>/dev/null || true
    wait "$tls_pid" 2>/dev/null || true
    tls_pid=""
  fi
}
trap tls_server_stop EXIT INT TERM

# Returns non-zero to SKIP the tls workload rather than fail the job: a missing
# python3 or openssl on one platform must not throw away that job's check and
# scrape rows, which is the same reason the summary step is non-fatal.
tls_server_start() {
  tls_pid=""
  for tool in python3 openssl; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "::warning::$tool not found, skipping the tls workload"
      return 1
    fi
  done

  if [ ! -f bench-tls-cert.pem ]; then
    # Self-signed on purpose: the client is meant to reject it, and that is
    # what makes the outcome deterministic. No SAN and no -addext, so this
    # works on LibreSSL (macOS) as well as OpenSSL.
    if ! openssl req -x509 -newkey rsa:2048 -keyout bench-tls-key.pem \
        -out bench-tls-cert.pem -days 1 -nodes -subj "/CN=127.0.0.1" \
        >bench-tls-openssl.log 2>&1; then
      echo "::warning::openssl could not create a cert, skipping tls"
      cat bench-tls-openssl.log >&2
      return 1
    fi
  fi

  python3 bench/tls_server.py --port "$TLS_PORT" \
    --cert bench-tls-cert.pem --key bench-tls-key.pem \
    --count-file bench-tls-handshakes.txt \
    >bench-tls-server.log 2>&1 &
  tls_pid=$!

  # Probe the port rather than the log, matching the Windows script: a readiness
  # signal that depends on reading a file another process is writing is one more
  # way to lose a cell. The probe sends nothing and closes, so the server drops
  # it before the handshake counter.
  i=0
  while [ "$i" -lt 150 ]; do
    if (exec 3<>"/dev/tcp/127.0.0.1/$TLS_PORT") 2>/dev/null; then
      exec 3<&- 3>&-
      return 0
    fi
    if ! kill -0 "$tls_pid" 2>/dev/null; then
      break
    fi
    sleep 0.1
    i=$(( i + 1 ))
  done

  echo "::warning::tls server never became ready, skipping the tls workload"
  cat bench-tls-server.log >&2
  tls_server_stop
  return 1
}

cells=""
total=$(( WARMUPS + REPS ))

for WORKLOAD in $WORKLOADS; do
export PROXY_SCRAPER_CHECKER_CONFIG="bench/config.${WORKLOAD}.toml"

# IFS is at its default here, so skipping the whole workload is safe.
if [ "$WORKLOAD" = "tls" ] && ! tls_server_start; then
  continue
fi

# Repetitions outside, tuning variants inside. Running all six reps of one
# variant before starting the next makes every variant comparison a
# before-and-after across several minutes, so any drift in the machine lands
# entirely on whichever variant ran later. That is not hypothetical: in that
# order a macOS cell reported jemalloc/tuned at -56% peak RSS purely because
# its reps ran last and slid from 36688 KB to 14080 KB. Interleaving spreads
# such a trend evenly over the variants instead.
for i in $(seq 1 "$total"); do

old_ifs="$IFS"; IFS=';'
for ALLOC_ENV in $ALLOC_ENVS; do
IFS="$old_ifs"

alloc_env_name="${ALLOC_ENV%%:*}"
alloc_env_vars=""
[ "$ALLOC_ENV" != "$alloc_env_name" ] && alloc_env_vars="${ALLOC_ENV#*:}"

cell_id="${PLATFORM_LABEL}|${ALLOCATOR}|mt=${TOKIO_MULTI_THREAD}|${WORKLOAD}|${alloc_env_name}"
if [ "$i" -eq 1 ]; then
  cells="${cells}${cell_id}
"
  echo "--- $cell_id (env: ${alloc_env_vars:-none}) ---"
fi

  log="bench-${WORKLOAD}-${alloc_env_name}-${i}.log"
  timing="bench-time-${WORKLOAD}-${alloc_env_name}-${i}.txt"

  hs_before=0
  if [ "$WORKLOAD" = "tls" ]; then
    hs_before="$(cat bench-tls-handshakes.txt 2>/dev/null || echo 0)"
  fi

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

  # Bail before the timing parse, not after. On the unproven Darwin path an
  # unparseable $timing dies at an unbound-variable or arithmetic error tens of
  # lines before the diagnostic dump below, and on Linux GNU time prepends
  # "Command exited with non-zero status" to the file, which corrupts the
  # tab-delimited read. Either way the log never reaches the job output.
  if [ "$rc" != "0" ]; then
    echo "rep $i exited with $rc" >&2
    tail -n 40 "$log" "$timing" >&2
    exit 1
  fi

  if [ "$uname_s" = "Linux" ]; then
    IFS=$'\t' read -r wall user sys peak_rss_kb minor major exit_code \
      < "$timing"
    peak_extra_kb=0
  else
    # BSD time has no -f, and line 1 is where it puts the timings.
    set -- $(awk '/real/ && /user/ && /sys/ { print $1, $3, $5; exit }' \
               "$timing")
    wall="$1"; user="$2"; sys="$3"
    # BSD time reports bytes; GNU time reports KB.
    # The END guards keep an absent line from collapsing to $(( / 1024 )), a
    # bash syntax error that would kill a macOS cell after its fat-LTO build.
    # Insurance on a branch that has never run, not a known trigger.
    peak_rss_kb=$(( $(awk '/maximum resident set size/ { print $1; f=1; exit }
                           END { if (!f) print 0 }' "$timing") / 1024 ))
    # phys_footprint: includes compressed memory, excludes clean file-backed
    # pages. Recorded alongside max RSS, never instead of it.
    peak_extra_kb=$(( $(awk '/peak memory footprint/ { print $1; f=1; exit }
                             END { if (!f) print 0 }' "$timing") / 1024 ))
    minor="$(awk '/page reclaims/ { print $1; exit }' "$timing")"
    major="$(awk '/ page faults/ { print $1; exit }' "$timing")"
    exit_code=$rc
  fi

  wall_ms="$(to_ms "$wall")"
  user_ms="$(to_ms "$user")"
  sys_ms="$(to_ms "$sys")"

  # Work-done counters, so a clean exit 0 that scraped nothing cannot pass as a
  # measurement. The aggregator additionally asserts these agree across every
  # rep and every cell of a platform.
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
  # config.rs raises RLIMIT_NOFILE itself and silently clamps
  # max_concurrent_checks to whatever it got, so EMFILE can never be measured -
  # but a platform that clamps runs a different concurrency than the rest of
  # the matrix, and nothing in the TSV would say so. Per-platform comparisons
  # stay valid either way; this only refuses to let it pass unnoticed.
  if grep -q "max_concurrent_checks config value is too high" "$log"; then
    echo "::warning::$cell_id rep $i: concurrency was clamped below the" \
      "configured 512; this platform is not running the same workload"
  fi

  # Per repetition, not just per job. A job-level total hid the real thing that
  # went wrong: two macos-26 jobs lost a third of their handshakes to ephemeral
  # port exhaustion, yet their totals still cleared a job-level threshold, and
  # the affected rows looked like a 2.5x allocator win. The counter flushes
  # every 10, hence the 1900 rather than 2000.
  if [ "$WORKLOAD" = "tls" ]; then
    hs_after="$(cat bench-tls-handshakes.txt 2>/dev/null || echo 0)"
    hs_done=$(( hs_after - hs_before ))
    if [ "$hs_done" -lt 1900 ]; then
      echo "::warning::$cell_id rep $i completed only $hs_done of 2000" \
        "handshakes, so this row measures failed connects, not TLS"
    fi
  fi

  # tls shares the check guard: every one of its 2000 proxies must reach
  # Proxy::check, and a proxies_checked of 0 means the corpus, the config or
  # the local server is wrong rather than that the allocator was fast.
  if { [ "$WORKLOAD" = "check" ] || [ "$WORKLOAD" = "tls" ]; } &&
     [ "$proxies_checked" -eq 0 ]; then
    echo "rep $i checked no proxies: corpus or config is wrong" >&2
    tail -n 40 "$log" "$timing" >&2
    exit 1
  fi
  if [ "$WORKLOAD" = "scrape" ] && [ "$proxies_out" -eq 0 ]; then
    echo "rep $i wrote no proxies: corpus or config is wrong" >&2
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

old_ifs="$IFS"; IFS=';'
done
IFS="$old_ifs"

done

if [ "$WORKLOAD" = "tls" ]; then
  # Prove the tunnel actually carried TLS. "Started checking 2000 proxies" is
  # logged before the first check runs, so proxies_checked cannot tell a run of
  # 2000 handshakes from a run of 2000 refused connects, and the second one
  # would quietly be a duplicate of the check workload wearing the tls label.
  # Counted by the server, so the measured process is untouched.
  handshakes="$(cat bench-tls-handshakes.txt 2>/dev/null || echo 0)"
  n_envs="$(printf '%s' "$ALLOC_ENVS" | awk -F';' '{ print NF }')"
  expected=$(( (WARMUPS + REPS) * 2000 * n_envs ))
  # 99%, not 50%. At 50% a job that lost 7560 of its 24000 handshakes to port
  # exhaustion still passed silently, and its rows were indistinguishable from
  # a large allocator win.
  if [ "$handshakes" -lt $(( expected * 99 / 100 )) ]; then
    echo "::warning::tls workload completed only $handshakes handshakes," \
      "expected about $expected, so those rows measure failed connects, not TLS"
  else
    echo "tls workload: $handshakes handshakes (expected ~$expected)"
  fi
  tls_server_stop
fi
done

# The summary is cosmetic; bench-results.tsv is the artifact that matters and
# the aggregate job reads that, not this. A formatting bug here must never
# discard a cell that already ran to completion, which is exactly what an awk
# syntax error did once, throwing away every measured repetition of the job.
echo "$cells" | while read -r cell; do
  if [ -n "$cell" ]; then
    sh .github/scripts/allocator_bench_report.sh "$results" "$cell" \
      || echo "::warning::summary failed for $cell (results are still in $results)"
  fi
done
