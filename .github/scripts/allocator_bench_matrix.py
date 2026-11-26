#!/usr/bin/env python3
"""Emit the allocator-bench job matrix as JSON for GITHUB_OUTPUT.

Expressing the matrix here instead of in YAML keeps it readable: the exclusions
are conditional on both the platform and the allocator, and as `exclude:` lists
they ran to dozens of entries that documented nothing.

Two axes deliberately do NOT appear here, because neither needs a rebuild and
the build is what costs minutes. The scripts loop over them inside one job:

* workload: `check` (24k proxies against refused loopback ports; stresses the
  per-proxy reqwest::Client construction) and `scrape` (~203k proxies from
  frozen CIDR corpora; stresses the dedup HashSet, the sort and serde_json).
* alloc_env: run-time allocator tuning. jemalloc reads MALLOC_CONF as conf
  source 3, which overrides the --with-malloc-conf baked in at build time;
  mimalloc reads MIMALLOC_* at process load, and MIMALLOC_ALLOW_THP=0 takes the
  same path as its no_thp build option (prctl(PR_SET_THP_DISABLE)), so turning
  THP off needs no build of its own. glibc reads GLIBC_TUNABLES at startup. The
  unix script interleaves a job's variants rep by rep, so a variant against its
  own `default` is the one comparison here free of between-job noise.
"""

from __future__ import annotations

import json
import os

# (label, runner, wrapper, os_family). The wrapper picks the workflow step:
# `native` runs the bench script on the runner, `alpine` runs it in a musl
# container, and `xenial` builds the release-style binary (glibc 2.17 through
# cargo-zigbuild) on the runner and runs it in an Ubuntu 16.04 container.
#
# The xenial rows exist because the runners only show what the newest systems
# do. musl is linked statically and macOS gets jemalloc, so neither depends on
# the user's OS, but glibc is the user's own, and releases start on glibc 2.17.
# 16.04 ships 2.23, which predates the malloc tcache (2.26), so it stands for
# the old systems where glibc malloc is weakest. Run 45861740593 ranked them as
# on 26.04: the system allocator had the lowest peak RSS, mimalloc cost 46-72%
# more (17-34% without THP), and no time delta cleared the between-job noise.
PLATFORMS = [
    ("ubuntu-26.04", "ubuntu-26.04", "native", "linux"),
    ("ubuntu-26.04-arm", "ubuntu-26.04-arm", "native", "linux"),
    ("ubuntu-16.04", "ubuntu-26.04", "xenial", "linux"),
    ("ubuntu-16.04-arm", "ubuntu-26.04-arm", "xenial", "linux"),
    ("alpine", "ubuntu-26.04", "alpine", "linux"),
    ("alpine-arm", "ubuntu-26.04-arm", "alpine", "linux"),
    ("macos-26", "macos-26", "native", "macos"),
    ("macos-26-intel", "macos-26-intel", "native", "macos"),
    ("windows-2025", "windows-2025", "native", "windows"),
    ("windows-11-vs2026-arm", "windows-11-vs2026-arm", "native", "windows"),
]

# `auto` is the shipped default feature set, so leaving it out would mean never
# measuring the configuration users actually get.
#
# It is knowingly a duplicate: `auto-allocator` only wires an allocator for
# target_os macos (tikv-jemallocator-auto, the same package and features as
# `jemalloc_override`), and the matching #[global_allocator] in src/main.rs is
# gated the same way. Off macOS `auto` is therefore the same program as
# `system`. It is kept anyway, for a reason: every "vs system" delta
# in the report compares two jobs on two different runner VMs, while the MAD
# that sets the noise floor comes from reps inside one job on one machine. The
# auto-vs-system spread on those platforms is therefore a direct measurement of
# the between-job noise the report otherwise has no way to see. Read it first,
# and distrust any delta that is not comfortably larger than it.
#
# The `*_override` rows answer the override question with data rather than
# reasoning: mimalloc's `override` and jemalloc's
# `override_allocator_on_supported_platforms` change what happens to the C
# dependencies' allocations (here aws-lc-sys), never to Rust's.
COMMON = ["system", "auto", "mimalloc", "mimalloc_override"]

# jemalloc-sys shells out to `sh` and `mingw32-make` and has no gnu_target
# mapping for aarch64-pc-windows-msvc, so it cannot be built on Windows at all.
JEMALLOC = ["jemalloc", "jemalloc_override"]

# The RSS side of the tokio-flavor question is already settled (multi_thread
# regressed 35 of 38 measured cells), but whether it buys any wall clock is
# still open, so it stays as a small confirmation subset now that time is
# recorded.
MT_PLATFORMS = {"ubuntu-26.04", "ubuntu-26.04-arm", "macos-26", "windows-2025"}
MT_ALLOCATORS = {"system", "auto"}

# Run-time variants exist only where they can change what ships, so each one is
# a candidate for the default build. Variants that were measured and settled are
# gone: jemalloc's decay tuning, thp:never and background_thread changed nothing
# or cost RSS (background_thread +17..31% on scrape) in every job of run
# 44974453662, and that comparison was within one job, so it was not noise.
# The Windows segment heap, switched per run through the image's IFEO key, cost
# windows-2025 +1..+22% peak working set in runs 45024444992 and 45026762281.
# Windows 11 on arm64 already gives this binary a segment heap, and forcing it
# there still saved 8..14% for reasons nobody worked out, so neither the
# variant nor a manifest opt-in shipped.
#
# Two tunings won and now ship, so their variants are gone too: one glibc arena
# (src/main.rs) and narenas:1,cache_oblivious:false baked into jemalloc on macOS
# (.cargo/config.toml), both in runs 45024444992 and 45026762281. glibc's arena
# limit is set with mallopt, which overrides GLIBC_TUNABLES, so no variant can
# undo it any more.


# Between-job reproducibility is allocator-dependent, and the auto-vs-system
# pair cannot measure it: those two build the same program and are the steadiest
# configuration in the matrix (max 2.6% apart off macOS), which real allocators
# are not. Comparing two full runs showed most cells landing within ~1pp of each
# other, but jemalloc_override on the THP=always x86_64 runners moved 20-26pp:
# alpine went +47.2% -> +21.4% against system with no change but the run. Read
# through the within-job spread alone that looked like "override saves 13%".
#
# These duplicates build a byte-identical binary under a second job, so the
# report shows X against X_dup and the gap is the between-job noise for that
# allocator, measured instead of assumed. The suffix is stripped before the
# feature list is assembled.
NOISE_DUPES = [
    ("ubuntu-26.04", "jemalloc_override"),
    ("alpine", "jemalloc_override"),
    ("ubuntu-26.04", "mimalloc"),
    # The question the xenial rows answer is whether mimalloc beats an old
    # glibc, so that comparison gets its own measured floor.
    ("ubuntu-16.04", "mimalloc"),
    # Both macOS runners: the shipped default is not the system allocator
    # there, and it had no between-job measurement at all.
    # jemalloc rather than jemalloc_override because
    # `auto` resolves to the same package and features as jemalloc_override
    # there, so the override variant already has a second job to compare with.
    ("macos-26", "jemalloc"),
    ("macos-26-intel", "jemalloc"),
]
DUP_SUFFIX = "_dup"


def alloc_envs(family: str, allocator: str) -> str:
    allocator = allocator.removesuffix(DUP_SUFFIX)
    variants = ["default"]
    if allocator.startswith("mimalloc") and family == "linux":
        variants.append("no_thp:MIMALLOC_ALLOW_THP=0")
    return ";".join(variants)


def main() -> None:
    cells = []
    for platform, runner, wrapper, family in PLATFORMS:
        allocators = COMMON + ([] if family == "windows" else JEMALLOC)
        if family == "windows":
            # mimalloc-sys/build.rs ignores `override` when target_os is
            # windows, so the cell compiles identical C to `mimalloc` and would
            # only add a row labelled as an override that overrides nothing.
            # exe_sha will not catch it either: the feature adds a #[used]
            # static regardless of target, so the binaries differ while the
            # behaviour does not.
            allocators = [a for a in allocators if a != "mimalloc_override"]
        allocators = allocators + [
            a + DUP_SUFFIX for p, a in NOISE_DUPES if p == platform
        ]
        for allocator in allocators:
            mts = [False]
            if platform in MT_PLATFORMS and allocator in MT_ALLOCATORS:
                mts.append(True)
            for mt in mts:
                cells.append(
                    {
                        "label": f"{platform} {allocator} mt={str(mt).lower()}",
                        "platform": platform,
                        "runner": runner,
                        "wrapper": wrapper,
                        "allocator": allocator,
                        "mt": str(mt).lower(),
                        "alloc_envs": alloc_envs(family, allocator),
                    }
                )

    payload = json.dumps(cells, separators=(",", ":"))
    out = os.environ.get("GITHUB_OUTPUT")
    line = f"cells={payload}"
    if out:
        with open(out, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")
    print(f"{len(cells)} jobs", flush=True)
    if not out:
        print(line)


if __name__ == "__main__":
    main()
