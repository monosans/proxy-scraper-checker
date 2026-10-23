#!/usr/bin/env bash
# Interleaved allocator A/B: builds every variant into one directory, then runs
# all of them round-robin (shuffled per rep) on the same machine, so timing
# deltas are not swamped by between-job VM noise.
#
# Env: VARIANTS (space-separated "name" or "name=feat1,feat2"), REPS, WORKLOADS,
#      OUT (results tsv), LABEL.
set -euo pipefail
REPS="${REPS:-20}"
WORKLOADS="${WORKLOADS:-check scrape tls}"
OUT="${OUT:-interleaved.tsv}"
LABEL="${LABEL:-unknown}"
bindir="$PWD/alloc-bins"
mkdir -p "$bindir"

for v in $VARIANTS; do
  name="${v%%=*}"
  case "$v" in
    system) feats="" ;;
    *=*) feats="${v#*=}" ;;
    *) feats="$v" ;;
  esac
  args=(build --release --locked --no-default-features)
  [ -n "$feats" ] && args+=(--features "$feats")
  echo "::group::$name: cargo ${args[*]}"
  cargo "${args[@]}"
  echo "::endgroup::"
  cp "${CARGO_TARGET_DIR:-target}/release/proxy-scraper-checker" "$bindir/$name"
done

uname_s="$(uname -s)"
thp="n/a"
if [ "$uname_s" = Linux ]; then
  thp="$(sed -n 's/.*\[\([a-z]*\)\].*/\1/p' /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || echo unknown)"
fi
[ -f "$OUT" ] || printf 'label\tthp\tworkload\tvariant\trep\twall_ms\tuser_ms\tsys_ms\tmaxrss_kb\tfootprint_kb\n' > "$OUT"

openssl req -x509 -newkey rsa:2048 -keyout il-key.pem -out il-cert.pem -days 1 -nodes -subj "/CN=127.0.0.1" >/dev/null 2>&1
python3 bench/tls_server.py --port 18443 --cert il-cert.pem --key il-key.pem --count-file il-hs.txt >il-tls.log 2>&1 &
tls=$!
trap 'kill $tls 2>/dev/null || true' EXIT
sleep 2

to_ms() { awk -v v="$1" 'BEGIN { printf "%d", v * 1000 }'; }
shuffle() { awk 'BEGIN { srand() } { print rand() "\t" $0 }' | sort -n | cut -f2-; }
names=()
for v in $VARIANTS; do names+=("${v%%=*}"); done

for rep in $(seq 0 "$REPS"); do
  for wl in $WORKLOADS; do
    export PROXY_SCRAPER_CHECKER_CONFIG="bench/config.$wl.toml"
    for b in $(printf '%s\n' "${names[@]}" | shuffle); do
      if [ "$uname_s" = Linux ]; then
        /usr/bin/time -f '%e %U %S %M' -o il-t.txt -- "$bindir/$b" >il-log.txt 2>&1 || { tail -n 30 il-log.txt il-t.txt; exit 1; }
        read -r wall user sys rss < il-t.txt
        fp=0
      else
        /usr/bin/time -l -- "$bindir/$b" >il-log.txt 2>il-t.txt || { tail -n 30 il-log.txt il-t.txt; exit 1; }
        set -- $(awk '/real/ && /user/ && /sys/ { print $1, $3, $5; exit }' il-t.txt)
        wall="$1"; user="$2"; sys="$3"
        rss=$(( $(awk '/maximum resident set size/ { print $1; exit }' il-t.txt) / 1024 ))
        fp=$(( $(awk '/peak memory footprint/ { print $1; f=1; exit } END { if (!f) print 0 }' il-t.txt) / 1024 ))
      fi
      [ "$rep" -eq 0 ] && continue
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$thp" "$wl" "$b" "$rep" \
        "$(to_ms "$wall")" "$(to_ms "$user")" "$(to_ms "$sys")" "$rss" "$fp" >> "$OUT"
    done
  done
  echo "rep $rep done"
done
echo "tls handshakes: $(cat il-hs.txt)"
