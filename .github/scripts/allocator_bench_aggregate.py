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
        # utf-8-sig, not utf-8: PowerShell's -Encoding utf8 emits a BOM on
        # Windows PowerShell. It is BOM-less under pwsh, which is what the
        # workflow uses, but a BOM would make the first column "﻿run_id"
        # and every row lookup raise KeyError after all 60 cells had run.
        with path.open(encoding="utf-8-sig") as fh:
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
    # key is (platform, libc, arch, workload, mt, allocator, alloc_env); libc
    # and arch ride along for the heading and come off `sample` below.
    for key in stats:
        groups[(key[0], key[3], key[4])].append(key)

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
        if baseline is None:
            out.append(
                "> No `system`/`default` rows in this group - that cell's job "
                "failed, so every delta below is blank for want of a "
                "baseline, not because the allocators tied.\n"
            )
        # n is printed because the upload is `if: always()`: a cell that died
        # at rep 1 still ships its rows, and a single rep has MAD 0, which
        # collapses the noise floor onto the 1% fallback and prints an
        # unqualified "win" - the exact n=1 regime this module was written
        # to get away from.
        header = "| allocator | env | n |"
        divider = "| --- | --- | ---: |"
        for _, label, _ in METRICS:
            header += f" {label} | MAD | vs system |"
            divider += " ---: | ---: | ---: |"
        out.append(header)
        out.append(divider)

        for key in sorted(keys, key=lambda k: (k[5], k[6])):
            n = stats[key]["wall_ms"][2]
            line = f"| `{key[5]}` | {key[6]} | {n} |"
            for name, _, _ in METRICS:
                median, mad, _ = stats[key][name]
                line += f" {median:.0f} | {mad:.0f} |"
                if baseline is None or key == baseline:
                    line += " - |"
                else:
                    base_median, base_mad, base_n = stats[baseline][name]
                    delta = median - base_median
                    pct = 100.0 * delta / base_median if base_median else 0.0
                    noise = NOISE_K * MAD_TO_SIGMA * max(mad, base_mad)
                    # A zero MAD means every rep landed on the same value; that
                    # is a real possibility for peak RSS and must not turn a
                    # 1 KB difference into a "significant" result.
                    noise = max(noise, 0.01 * base_median)
                    if n < 3 or base_n < 3:
                        mark = " (n<3)"
                    else:
                        mark = "" if abs(delta) > noise else " (noise)"
                    line += f" {pct:+.1f}%{mark} |"
            out.append(line)
        out.append("")


def render_mt(stats: dict, out: list[str]) -> None:
    """Put mt=true next to mt=false for otherwise identical cells.

    render() carries `mt` in the group key, so the per-platform tables never
    place the two flavors side by side, and each table normalises its deltas to
    its own baseline - which makes the two columns incomparable as well. The
    mt=true jobs exist to answer this one question, so it gets its own table.
    """
    pairs: dict[tuple[str, str, str, str], dict[str, dict]] = defaultdict(dict)
    # key is (platform, libc, arch, workload, mt, allocator, alloc_env).
    for key, series in stats.items():
        pairs[(key[0], key[3], key[5], key[6])][key[4]] = series

    rows = [
        (key, sides)
        for key, sides in sorted(pairs.items())
        if "true" in sides and "false" in sides
    ]
    if not rows:
        return

    out.append("## tokio-multi-thread: true vs false\n")
    out.append(
        "Negative means multi_thread won. The flavor is a build-time feature, "
        "so each pair is still two jobs on two runner VMs.\n"
    )
    header = "| platform | workload | allocator | env | n |"
    divider = "| --- | --- | --- | --- | ---: |"
    for _, label, _ in METRICS:
        header += f" {label} mt=false | mt=true | delta |"
        divider += " ---: | ---: | ---: |"
    out.append(header)
    out.append(divider)

    for (platform, workload, allocator, alloc_env), sides in rows:
        off, on = sides["false"], sides["true"]
        n = min(off["wall_ms"][2], on["wall_ms"][2])
        line = f"| {platform} | {workload} | `{allocator}` | {alloc_env} | {n} |"
        for name, _, _ in METRICS:
            base_median, base_mad, _ = off[name]
            median, mad, _ = on[name]
            delta = median - base_median
            pct = 100.0 * delta / base_median if base_median else 0.0
            noise = max(
                NOISE_K * MAD_TO_SIGMA * max(mad, base_mad), 0.01 * base_median
            )
            if n < 3:
                mark = " (n<3)"
            else:
                mark = "" if abs(delta) > noise else " (noise)"
            line += f" {base_median:.0f} | {median:.0f} | {pct:+.1f}%{mark} |"
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
    stats = summarize(rows)
    render_mt(stats, out)
    render(stats, out)
    out.append(
        "Deltas are median-vs-median. `(noise)` marks a delta inside "
        f"{NOISE_K:g}x the scaled MAD of the noisier of the two cells, i.e. one "
        "this data cannot distinguish from run-to-run variation. `(n<3)` marks "
        "a comparison where one side has too few repetitions for its MAD to "
        "mean anything - treat it as unmeasured, not as a result.\n"
        "\n"
        "The MAD is measured *within* one job on one runner, but every delta "
        "compares two jobs on two runner VMs, so timing deltas in particular "
        "are less significant than they look here. Calibrate against the "
        "`auto` vs `system` row: off macOS those two build the same program, "
        "so whatever spread they show is pure between-job noise.\n"
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
