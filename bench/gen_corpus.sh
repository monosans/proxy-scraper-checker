#!/usr/bin/env sh
# Regenerates bench/corpus/*.txt.
#
# The generated files are committed, so CI never runs this. It exists so the
# corpus bytes can be audited and reproduced: the run-to-run drift of the live
# proxy lists was the largest confound in the previous allocator benchmarks, and
# a frozen corpus is what removes it.
#
# Deterministic: a fixed-seed LCG, no date, no shuffle, no network.
# Uses only POSIX sh + awk, so it runs identically on Linux, macOS, Alpine and
# Git Bash.
#
# Usage: sh bench/gen_corpus.sh
set -eu

cd "$(dirname "$0")"
mkdir -p corpus

awk '
# The low bits of a power-of-two-modulus LCG have very short periods (the low
# bit alternates), so every draw takes the high bits instead. Without this the
# octets collapse to values like 122.0.0.205 and the dedup set stops being
# representative.
function rnd() { seed = (seed * 1103515245 + 12345) % 2147483648; return int(seed / 2048) }
function pick(lo, hi) { return lo + rnd() % (hi - lo + 1) }
function letters(n,   i, s) {
  s = ""
  for (i = 0; i < n; i++) s = s sprintf("%c", 97 + rnd() % 26)
  return s
}

# --- checking corpus -------------------------------------------------------
# 127.0.0.1 with unbound high ports: connect() is refused in one loopback RTT
# on Linux, macOS and Windows alike, and an RST-ed connection never enters
# TIME_WAIT, so ephemeral ports cannot run out. Every proxy still goes through
# the full per-proxy reqwest::Client construction in Proxy::check, which is the
# allocation site this workload exists to measure.
# Only the port varies: other 127.x.x.x addresses are not assigned on macOS and
# Windows, and their connect behaviour differs per OS, which would make the
# wall-clock column incomparable across platforms.
function gen_check(file, base,   i) {
  for (i = 0; i < 8000; i++) print "127.0.0.1:" (base + i) > file
  close(file)
}

# --- scraping corpus -------------------------------------------------------
# Shapes chosen to cover every branch of parsers.rs and the scraper hot loop:
# bare ipv4:port, CIDR shorthand (the bulk), scheme prefixes, userinfo (the
# Box<ProxyAuth> + auth.clone() path) and letter-only hostnames (the
# HostSortKey::Name path in output.rs, plus the heap CompactString path on
# 32-bit targets). Interleaved HTML/JSON noise keeps the regex scanner honest
# rather than letting it match one clean line after another.
function gen_scrape(file, cidr_block, scheme,   i, a, b, c, d, p) {
  print "<!DOCTYPE html><html><body><table id=\"proxylisttable\">" > file
  print "{\"updated\":\"frozen\",\"note\":\"no timestamps - the corpus must be byte-stable\",\"rows\":[" > file

  # One /16 per protocol file: 65534 hosts from a single line. This is where
  # the dedup HashSet gets big enough for allocator differences to resolve.
  print "  <tr><td>" cidr_block ".0.0/16:8080</td><td>elite</td></tr>" > file

  for (i = 0; i < 2000; i++) {
    a = pick(1, 223); if (a == 10 || a == 127) a = 203
    b = pick(0, 255); c = pick(0, 255); d = pick(1, 254); p = pick(1024, 65535)
    if (i % 7 == 0)
      print "  <tr><td>" scheme "://" a "." b "." c "." d ":" p "</td></tr>" > file
    else if (i % 11 == 0)
      print "    {\"ip\": \"" a "." b "." c "." d "\", \"port\": " p ", \"addr\": \"" a "." b "." c "." d ":" p "\"}," > file
    else
      print a "." b "." c "." d ":" p > file
  }

  for (i = 0; i < 200; i++) {
    a = pick(1, 223); if (a == 10 || a == 127) a = 198
    b = pick(0, 255); c = pick(0, 255); d = pick(1, 254); p = pick(1024, 65535)
    print letters(6) ":" letters(8) "@" a "." b "." c "." d ":" p > file
  }

  # Hostnames: the regex host alternative accepts letters, dots and hyphens
  # only - no digits - so the labels are letter-only by necessity.
  for (i = 0; i < 200; i++)
    print letters(7) ".proxy.example.com:" pick(1024, 65535) > file

  print "</table></body></html>" > file
  close(file)
}

BEGIN {
  seed = 20240817

  gen_check("corpus/check_http.txt",   20000)
  gen_check("corpus/check_socks4.txt", 28000)
  gen_check("corpus/check_socks5.txt", 36000)

  gen_scrape("corpus/scrape_http.txt",   "10.16", "http")
  gen_scrape("corpus/scrape_socks4.txt", "10.80", "socks4")
  gen_scrape("corpus/scrape_socks5.txt", "10.144", "socks5")
}
'

wc -l corpus/*.txt
