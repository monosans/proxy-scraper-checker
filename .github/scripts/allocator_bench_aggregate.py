#!/usr/bin/env python3
"""Merge the per-job allocator-bench TSVs into one table and one verdict.

Reads every bench-results.tsv under the directory given on the command line
(the downloaded artifacts), writes the concatenated rows to bench-all.tsv, and
renders per-platform tables into GITHUB_STEP_SUMMARY.

Two properties the previous harness lacked and that everything here depends on:

* Rows are only ever compared inside one (platform, workload) group. Linux
  ru_maxrss, macOS maxrss and the Windows peak working set are three different
  quantities, and the page-fault columns are three different counters, so a
  cross-OS ranking is meaningless no matter how tempting the table looks.
* A delta is only called a win if it clears that cell's own measured spread.
  The noise floor is derived from the reps, not assumed: for each comparison it
  is the larger of the two cells' MAD, scaled to a rough 95% interval. With the
  old n=1 data, 21% of pairwise allocator comparisons flipped sign between two
  runs of the identical cell.
"""

from __future__ import annotations

import os
import pathlib
import statistics
import sys
from collections import defaultdict

# MAD -> standard-deviation-ish scale for a normal distribution, then ~2 sigma.
# Deliberately conservative: over-reporting a win is the expensive mistake here,
# since it ends in a C toolchain being added to 41 release targets.
MAD_TO_SIGMA = 1.4826
NOISE_K = 2.0

METRICS = [
    ("peak_rss_kb", "peak RSS KB", "lower"),
    ("wall_ms", "wall ms", "lower"),
    ("cpu_ms", "cpu ms", "lower"),
]


def read_rows(root: pathlib.Path) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for path in sorted(root.rglob("bench-results.tsv")):
        with path.open(encoding="utf-8") as fh:
            header = fh.readline().rstrip("\n").split("\t")
            for line in fh:
                line = line.rstrip("\n")
                if not line:
                    continue
                values = line.split("\t")
                if len(values) != len(header):
                    sys.exit(f"{path}: malformed row: {line!r}")
                rows.append(dict(zip(header, values)))
    if not rows:
        sys.exit(f"no bench-results.tsv found under {root}")
    return rows


def check_consistency(rows: list[dict[str, str]], out: list[str]) -> None:
    """Assert every cell of a platform did the same amount of work.

    If two allocators on one platform disagree about how many proxies were
    checked or written, the corpus was not frozen and no comparison between
    them means anything.
    """
    work: dict[tuple[str, str], set[tuple[str, str]]] = defaultdict(set)
    binaries: dict[tuple[str, str], set[str]] = defaultdict(set)
    for row in rows:
        key = (row["platform"], row["workload"])
        work[key].add((row["proxies_checked"], row["proxies_out"]))
        binaries[(row["platform"], row["exe_sha"])].add(row["allocator"])

    problems = []
    for (platform, workload), seen in sorted(work.items()):
        if len(seen) > 1:
            problems.append(
                f"- **{platform}/{workload}**: cells disagree on work done "
                f"(checked, written) = {sorted(seen)}"
            )
    for (platform, sha), allocators in sorted(binaries.items()):
        if len(allocators) > 1:
            problems.append(
                f"- **{platform}**: {sorted(allocators)} produced the identical "
                f"binary {sha} - those features are not doing anything"
            )

    if problems:
        out.append("## Consistency problems\n")
        out.extend(problems)
        out.append("")


def summarize(rows: list[dict[str, str]]) -> dict:
    cells: dict[tuple, dict[str, list[float]]] = defaultdict(
        lambda: defaultdict(list)
    )
    for row in rows:
        key = (
            row["platform"],
            row["libc"],
            row["arch"],
            row["workload"],
            row["mt"],
            row["allocator"],
            row["alloc_env"],
        )
        cells[key]["peak_rss_kb"].append(float(row["peak_rss_kb"]))
        cells[key]["wall_ms"].append(float(row["wall_ms"]))
        cells[key]["cpu_ms"].append(
            float(row["cpu_user_ms"]) + float(row["cpu_sys_ms"])
        )
    stats = {}
    for key, series in cells.items():
        stats[key] = {
            name: (
                statistics.median(values),
                statistics.median(
                    [abs(v - statistics.median(values)) for v in values]
                ),
                len(values),
            )
            for name, values in series.items()
        }
    return stats


def render(stats: dict, out: list[str]) -> None:
    groups: dict[tuple[str, str, str], list[tuple]] = defaultdict(list)
    for key in stats:
        platform, libc, arch, workload, mt, allocator, alloc_env = key
        groups[(platform, workload, mt)].append(key)

    for (platform, workload, mt), keys in sorted(groups.items()):
        sample = keys[0]
        out.append(
            f"## {platform} ({sample[1]}/{sample[2]}) - "
            f"workload={workload}, tokio-multi-thread={mt}\n"
        )
        baseline = next(
            (
                k
                for k in keys
                if k[5] == "system" and k[6] == "default"
            ),
            None,
        )
        header = "| allocator | env |"
        divider = "| --- | --- |"
        for _, label, _ in METRICS:
            header += f" {label} | MAD | vs system |"
            divider += " ---: | ---: | ---: |"
        out.append(header)
        out.append(divider)

        for key in sorted(keys, key=lambda k: (k[5], k[6])):
            line = f"| `{key[5]}` | {key[6]} |"
            for name, _, _ in METRICS:
                median, mad, _ = stats[key][name]
                line += f" {median:.0f} | {mad:.0f} |"
                if baseline is None or key == baseline:
                    line += " - |"
                else:
                    base_median, base_mad, _ = stats[baseline][name]
                    delta = median - base_median
                    pct = 100.0 * delta / base_median if base_median else 0.0
                    noise = NOISE_K * MAD_TO_SIGMA * max(mad, base_mad)
                    # A zero MAD means every rep landed on the same value; that
                    # is a real possibility for peak RSS and must not turn a
                    # 1 KB difference into a "significant" result.
                    noise = max(noise, 0.01 * base_median)
                    mark = "" if abs(delta) > noise else " (noise)"
                    line += f" {pct:+.1f}%{mark} |"
            out.append(line)
        out.append("")


def main() -> None:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "artifacts")
    rows = read_rows(root)

    with open("bench-all.tsv", "w", encoding="utf-8", newline="\n") as fh:
        header = list(rows[0].keys())
        fh.write("\t".join(header) + "\n")
        for row in rows:
            fh.write("\t".join(row[column] for column in header) + "\n")

    out: list[str] = [f"# Allocator bench - {len(rows)} measured repetitions\n"]
    check_consistency(rows, out)
    render(summarize(rows), out)
    out.append(
        "Deltas are median-vs-median. `(noise)` marks a delta inside "
        f"{NOISE_K:g}x the scaled MAD of the noisier of the two cells, i.e. one "
        "this data cannot distinguish from run-to-run variation.\n"
    )

    text = "\n".join(out)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(text)
    else:
        print(text)


if __name__ == "__main__":
    main()
