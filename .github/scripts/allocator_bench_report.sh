#!/usr/bin/env sh
# Per-job summary: median and MAD over the measured repetitions of one cell.
#
# Median rather than mean, MAD rather than stddev: with 5 reps a single
# descheduled run would drag a mean far enough to invent an allocator
# difference. Cross-cell and cross-platform comparison is the aggregate job's
# problem, not this one's - a job only ever sees its own cell.
#
# Usage: allocator_bench_report.sh <results.tsv> <cell_id>
set -eu

results="$1"
cell_id="$2"

{
  echo "### ${cell_id}"
  echo ""
  awk -F'\t' -v cell="$cell_id" '
    function median(a, n,   m) {
      # a is 1-indexed and already sorted
      m = int(n / 2)
      return (n % 2) ? a[m + 1] : (a[m] + a[m + 1]) / 2
    }
    function isort(a, n,   i, j, t) {
      for (i = 2; i <= n; i++) {
        t = a[i]
        for (j = i - 1; j >= 1 && a[j] > t; j--) a[j + 1] = a[j]
        a[j + 1] = t
      }
    }
    function stat(src, n, out,   i, s[1], d[1], med) {
      for (i = 1; i <= n; i++) s[i] = src[i]
      isort(s, n); med = median(s, n)
      for (i = 1; i <= n; i++) d[i] = (s[i] > med) ? s[i] - med : med - s[i]
      isort(d, n)
      out[1] = med; out[2] = median(d, n)
    }
    NR == 1 { next }
    $2 == cell {
      n++
      wall[n] = $13; cpu[n] = $14 + $15; rss[n] = $16; extra[n] = $17
      pf_major = $19; pf_minor = $20; pf_kind = $21
      checked = $23; out_n = $24; sha = $25; thp = $26
      cores = $7; tc = $8; extra_kind = $18; pg = $27
    }
    END {
      if (!n) { print "no measured repetitions"; exit }
      stat(wall, n, w); stat(cpu, n, c); stat(rss, n, r); stat(extra, n, e)
      printf "reps: %d &nbsp;&nbsp; cores: %s &nbsp;&nbsp; rustc: %s", n, cores, tc
      printf " &nbsp;&nbsp; THP: %s &nbsp;&nbsp; page: %s KB", thp, pg
      printf " &nbsp;&nbsp; exe: %s\n\n", sha
      print "| metric | median | MAD |"
      print "| --- | ---: | ---: |"
      printf "| wall ms | %d | %d |\n", w[1], w[2]
      printf "| cpu ms (user+sys) | %d | %d |\n", c[1], c[2]
      printf "| peak RSS KB | %d | %d |\n", r[1], r[2]
      if (extra_kind != "none")
        printf "| peak %s KB | %d | %d |\n", extra_kind, e[1], e[2]
      printf "| major PF (%s) | %s | |\n", pf_kind, pf_major
      printf "| minor PF (%s) | %s | |\n", pf_kind, pf_minor
      printf "\nwork done: checked=%s written=%s\n", checked, out_n
    }
  ' "$results"
  echo ""
} >> "${GITHUB_STEP_SUMMARY:-/dev/stdout}"
