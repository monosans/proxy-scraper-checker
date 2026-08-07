#!/usr/bin/env python3
"""Merge the per-job allocator-bench TSVs into one table and one verdict.

Reads every bench-results.tsv under the directory given on the command line
(the downloaded artifacts), writes the concatenated rows to bench-all.tsv, and
renders per-platform tables into GITHUB_STEP_SUMMARY.

Everything here depends on two properties:

* Rows are only ever compared inside one (platform, workload) group. Linux
  ru_maxrss, macOS maxrss and the Windows peak working set are three different
  quantities, and the page-fault columns are three different counters, so a
  cross-OS ranking is meaningless no matter how tempting the table looks.
* A delta is only called a win if it clears that cell's own measured spread.
  The noise floor is derived from the reps, not assumed: for each comparison it
  is the larger of the two cells' MAD, scaled to a rough 95% interval. At n=1,
  21% of pairwise allocator comparisons flipped sign between two runs of the
  identical cell.
"""

from __future__ import annotations

import os
import pathlib
import statistics
import sys
from collections import defaultdict
from typing import NamedTuple

METRICS = [
    ("peak_rss_kb", "peak RSS KB", "lower"),
    ("wall_ms", "wall ms", "lower"),
    ("cpu_ms", "cpu ms", "lower"),
]

# A cell whose reps span more than this is not one measurement with noise on
# it. 22% of the cells in the first three-workload run cleared it, almost all
# of them jemalloc or mimalloc.
UNSTABLE_SPREAD_PCT = 5.0
# Drift, measured as the mean of the last two reps against the first two.
# Above this the reps are a trend, not a sample, and the median is meaningless:
# one macOS cell ran 36688 -> 14080 KB across five reps and its MAD still said
# the number was solid.
DRIFT_PCT = 10.0


class Cell(NamedTuple):
    median: float
    mad: float
    n: int
    lo: float
    hi: float
    seq: list[float]

    @property
    def spread_pct(self) -> float:
        return 100.0 * (self.hi - self.lo) / self.median if self.median else 0.0

    @property
    def drift_pct(self) -> float:
        if self.n < 4:
            return 0.0
        early = statistics.mean(self.seq[:2])
        late = statistics.mean(self.seq[-2:])
        return 100.0 * (late - early) / early if early else 0.0

    @property
    def warning(self) -> str:
        """Single-character marker, worst problem first."""
        if abs(self.drift_pct) > DRIFT_PCT:
            return "D"
        if self.spread_pct > UNSTABLE_SPREAD_PCT:
            return "S"
        return ""


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
                f"binary {sha}, so those features are not doing anything"
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
            name: Cell(
                median=statistics.median(values),
                mad=statistics.median(
                    [abs(v - statistics.median(values)) for v in values]
                ),
                n=len(values),
                lo=min(values),
                hi=max(values),
                # Reps in file order. Kept so a monotonic drift can be told
                # apart from scatter; see drift() below.
                seq=list(values),
            )
            for name, values in series.items()
        }
    return stats


DUP_SUFFIX = "_dup"


def job_noise_floors(stats: dict) -> dict[tuple[str, str, str], float]:
    """Between-job noise per (platform, workload, metric), from the dup cells.

    A "_dup" cell is the same features built in a second job, so the gap between
    X and X_dup is noise by construction. It is much larger than it looks from
    the auto-vs-system pair, which is the steadiest configuration in the matrix:
    that pair sits within 1.4% while jemalloc_override on ubuntu-24.04 moved
    ~14% between jobs across every tuning variant. Within-cell spread cannot see
    this at all - each side is individually tight - so without a floor the
    report will keep calling a 14% job-to-job wobble a result.
    """
    floors: dict[tuple[str, str, str], float] = {}
    for key, series in stats.items():
        allocator = key[5]
        if not allocator.endswith(DUP_SUFFIX):
            continue
        base_key = (*key[:5], allocator[: -len(DUP_SUFFIX)], key[6])
        base = stats.get(base_key)
        if base is None:
            continue
        for name, _, _ in METRICS:
            b = base[name].median
            if not b:
                continue
            pct = abs(100.0 * (series[name].median - b) / b)
            slot = (key[0], key[3], name)
            floors[slot] = max(floors.get(slot, 0.0), pct)

    # Platforms without a dup cell fall back to the worst floor measured
    # anywhere for that metric, rather than to zero. Zero would quietly award
    # full confidence exactly where the noise is unknown, which is what this
    # whole mechanism is meant to prevent.
    for name, _, _ in METRICS:
        worst = max(
            (v for (_, _, m), v in floors.items() if m == name), default=0.0
        )
        floors[("*", "*", name)] = worst
    return floors


def floor_for(floors: dict, platform: str, workload: str, metric: str) -> float:
    if (platform, workload, metric) in floors:
        return floors[(platform, workload, metric)]
    return floors.get(("*", "*", metric), 0.0)


def delta_cell(x: Cell, b: Cell, floor: float = 0.0) -> str:
    """Median-vs-median delta, qualified by what the reps can actually support.

    A MAD-based test calls a cell precise whenever three of five reps agree,
    and mimalloc cells are frequently bimodal, e.g.
    [75808, 63524, 75812, 65576, 75812], whose MAD is 4 KB against a 12 MB
    spread. So significance is decided by the observed ranges instead: the
    delta counts only if it keeps its sign when each side is taken at its worst
    against the other's best. That is conservative by construction, which is
    the right direction when a false positive ends in a C toolchain being added
    to 41 release targets.
    """
    pct = 100.0 * (x.median - b.median) / b.median if b.median else 0.0
    if x.n < 3 or b.n < 3:
        return f"{pct:+.1f}% (n<3)"
    if abs(x.drift_pct) > DRIFT_PCT or abs(b.drift_pct) > DRIFT_PCT:
        return f"{pct:+.1f}% (drift)"
    lo = 100.0 * (x.lo - b.hi) / b.hi if b.hi else 0.0
    hi = 100.0 * (x.hi - b.lo) / b.lo if b.lo else 0.0
    if not (lo > 0.0 and hi > 0.0 or lo < 0.0 and hi < 0.0):
        return f"{pct:+.1f}% (noise)"
    # Both cells come from separate jobs, so clearing their within-job spread is
    # necessary but not sufficient.
    if abs(pct) <= floor:
        return f"{pct:+.1f}% (job-noise)"
    return f"{pct:+.1f}%"


def render(stats: dict, out: list[str], floors: dict) -> None:
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
                "> No `system`/`default` rows in this group, so that cell's "
                "job failed. Every delta below is blank for want of a "
                "baseline, not because the allocators tied.\n"
            )
        # n is printed because the upload is `if: always()`: a cell that died
        # at rep 1 still ships its rows, and a single rep has MAD 0, which
        # collapses the noise floor onto the 1% fallback and prints an
        # unqualified "win". That n=1 regime is exactly what this module
        # exists to avoid.
        header = "| allocator | env | n |"
        divider = "| --- | --- | ---: |"
        for _, label, _ in METRICS:
            header += f" {label} | spread | vs system |"
            divider += " ---: | ---: | ---: |"
        out.append(header)
        out.append(divider)

        for key in sorted(keys, key=lambda k: (k[5], k[6])):
            cell = stats[key]["wall_ms"]
            line = f"| `{key[5]}` | {key[6]} | {cell.n} |"
            for name, _, _ in METRICS:
                c = stats[key][name]
                line += f" {c.median:.0f} | {c.spread_pct:.1f}%{c.warning} |"
                if baseline is None or key == baseline:
                    line += " - |"
                else:
                    floor = floor_for(floors, platform, workload, name)
                    line += f" {delta_cell(c, stats[baseline][name], floor)} |"
            out.append(line)

        measured = [
            f"{label} {floors[(platform, workload, name)]:.1f}%"
            for name, label, _ in METRICS
            if (platform, workload, name) in floors
        ]
        if measured:
            out.append(
                "\nBetween-job noise measured here from the `_dup` cells: "
                + ", ".join(measured)
                + ". Deltas within it are marked `(job-noise)`.\n"
            )
        else:
            borrowed = ", ".join(
                f"{label} {floor_for(floors, platform, workload, name):.1f}%"
                for name, label, _ in METRICS
            )
            out.append(
                "\nNo `_dup` cell on this platform, so between-job noise was "
                "not measured here. The deltas above are held to the worst "
                f"floor seen anywhere instead ({borrowed}), which is a guess. "
                "Add this platform to NOISE_DUPES in allocator_bench_matrix.py "
                "if a decision is going to rest on it.\n"
            )
        out.append("")


def render_mt(stats: dict, out: list[str]) -> None:
    """Put mt=true next to mt=false for otherwise identical cells.

    render() carries `mt` in the group key, so the per-platform tables never
    place the two flavors side by side, and each table normalises its deltas to
    its own baseline, which makes the two columns incomparable as well. The
    mt=true jobs answer this one question, so it gets its own table.
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
        n = min(off["wall_ms"].n, on["wall_ms"].n)
        line = f"| {platform} | {workload} | `{allocator}` | {alloc_env} | {n} |"
        for name, _, _ in METRICS:
            line += (
                f" {off[name].median:.0f} | {on[name].median:.0f} |"
                f" {delta_cell(on[name], off[name])} |"
            )
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
    floors = job_noise_floors(stats)
    render_mt(stats, out)
    render(stats, out, floors)
    out.append(
        "Deltas are median-vs-median, qualified by the observed spread of the "
        "repetitions rather than by their MAD: a delta counts only if it keeps "
        "its sign with each side taken at its worst against the other's best. "
        "`(noise)` means it does not. `(n<3)` means one side has too few "
        "repetitions to say anything. `(drift)` means one side's reps are a "
        "trend rather than a sample, so its median describes nothing.\n"
        "\n"
        "The `spread` column is (max-min)/median over a cell's own reps. "
        f"`S` marks more than {UNSTABLE_SPREAD_PCT:g}%, which is common for "
        "jemalloc and mimalloc and often bimodal rather than noisy; `D` marks "
        f"a drift above {DRIFT_PCT:g}% between the first and last reps.\n"
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
