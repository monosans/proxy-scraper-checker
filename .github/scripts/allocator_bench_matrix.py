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
  mimalloc reads MIMALLOC_* at process load. This is also what replaces the old
  mimalloc_v*_no_thp build features: MIMALLOC_ALLOW_THP=0 is strictly stronger,
  since it additionally issues prctl(PR_SET_THP_DISABLE), and it covers v2 and
  v3 with the same binary.
"""

from __future__ import annotations

import json
import os

# (label, runner, alpine, os_family)
PLATFORMS = [
    ("ubuntu-24.04", "ubuntu-24.04", False, "linux"),
    ("ubuntu-24.04-arm", "ubuntu-24.04-arm", False, "linux"),
    ("alpine", "ubuntu-24.04", True, "linux"),
    ("alpine-arm", "ubuntu-24.04-arm", True, "linux"),
    ("macos-26", "macos-26", False, "macos"),
    ("macos-26-intel", "macos-26-intel", False, "macos"),
    ("windows-2025", "windows-2025", False, "windows"),
    ("windows-11-arm", "windows-11-arm", False, "windows"),
]

# `auto` is the shipped default feature set, so leaving it out would mean never
# measuring the configuration users actually get.
#
# It is knowingly a duplicate: `auto-allocator` wires tikv-jemallocator-auto
# only under [target.'cfg(target_os = "macos")'.dependencies] (Cargo.toml) and
# the matching #[global_allocator] in src/main.rs is gated on target_os macos,
# so off macOS `auto` is the same program as `system`, and on macOS it is the
# same package and feature set as `jemalloc_override`. It is kept anyway, for a
# reason: every "vs system" delta in the report compares two jobs on two
# different runner VMs, while the MAD that sets the noise floor comes from reps
# inside one job on one machine. The auto-vs-system spread on a non-macOS
# platform is therefore a direct measurement of the between-job noise the
# report otherwise has no way to see. Read it first, and distrust any delta
# that is not comfortably larger than it.
#
# The `*_override` rows answer the override question with data rather than
# reasoning: mimalloc's `override` and jemalloc's
# `override_allocator_on_supported_platforms` change what happens to the C
# dependencies' allocations (here aws-lc-sys), never to Rust's.
COMMON = ["system", "auto", "mimalloc_v2", "mimalloc_v3", "mimalloc_v3_override"]

# jemalloc-sys shells out to `sh` and `mingw32-make` and has no gnu_target
# mapping for aarch64-pc-windows-msvc, so it cannot be built on Windows at all.
JEMALLOC = ["jemalloc", "jemalloc_override"]

# The RSS side of the tokio-flavor question is already settled (multi_thread
# regressed 35 of 38 measured cells), but whether it buys any wall clock is
# still open, so it stays as a small confirmation subset now that time is
# recorded.
MT_PLATFORMS = {"ubuntu-24.04", "ubuntu-24.04-arm", "macos-26", "windows-2025"}
MT_ALLOCATORS = {"system", "auto"}

# Both spellings are set: jemalloc reads the prefixed name when built prefixed
# (always on Apple, and on Linux without the override feature) and the plain
# name when unprefixed. Setting the unused one is harmless.
JE_TUNED = (
    "tuned:MALLOC_CONF=dirty_decay_ms:1000,muzzy_decay_ms:0 "
    "_RJEM_MALLOC_CONF=dirty_decay_ms:1000,muzzy_decay_ms:0"
)
JE_BG = "bg:MALLOC_CONF=background_thread:true _RJEM_MALLOC_CONF=background_thread:true"

# An earlier run split perfectly along THP: jemalloc cost +59..92% peak RSS on
# the x86_64 runners, which report /sys/.../enabled = [always], and only +5..19%
# on the arm64 ones, which report [madvise]. The mechanism is plausible enough
# (jemalloc maps large aligned regions that khugepaged then backs with 2 MB
# pages), but it is confounded with the architecture. opt.thp is a Linux-only
# jemalloc option (it needs MADVISE_HUGE), so this variant tests it directly on
# the always machines instead of inferring it from a correlation.
JE_THP = "thp_never:MALLOC_CONF=thp:never _RJEM_MALLOC_CONF=thp:never"


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
    ("ubuntu-24.04", "jemalloc_override"),
    ("alpine", "jemalloc_override"),
    ("ubuntu-24.04", "mimalloc_v3"),
    # Both macOS runners: this is the only platform whose result actually
    # decides anything, since it is the one place the shipped default is not
    # the system allocator, and it had no between-job measurement at all.
    # jemalloc rather than jemalloc_override because
    # `auto` resolves to the same package and features as jemalloc_override
    # there, so the override variant already has a second job to compare with.
    ("macos-26", "jemalloc"),
    ("macos-26-intel", "jemalloc"),
]
DUP_SUFFIX = "_dup"


def alloc_envs(platform: str, family: str, allocator: str) -> str:
    allocator = allocator.removesuffix(DUP_SUFFIX)
    variants = ["default"]
    if allocator.startswith("mimalloc") and family == "linux":
        variants.append("no_thp:MIMALLOC_ALLOW_THP=0")
    if allocator.startswith("jemalloc"):
        variants.append(JE_TUNED)
        if family == "linux":
            variants.append(JE_THP)
        # jemalloc-sys lists musl in NO_BG_THREAD_TARGETS, and macOS compiles
        # background threads out entirely, so the knob only exists on glibc.
        if family == "linux" and not platform.startswith("alpine"):
            variants.append(JE_BG)
    return ";".join(variants)


def main() -> None:
    cells = []
    for platform, runner, alpine, family in PLATFORMS:
        allocators = COMMON + ([] if family == "windows" else JEMALLOC)
        if family == "windows":
            # libmimalloc-sys/build.rs defines MI_MALLOC_OVERRIDE only when
            # target_family != "windows", and skips -fno-builtin-malloc for
            # MSVC ("overriding malloc is only available on windows in shared
            # mode, but we only ever build a static lib"). The cell therefore
            # compiles identical C to mimalloc_v3 and would only add a row
            # labelled as an override that overrides nothing. exe_sha will not
            # catch it either: the feature adds a #[used] static regardless of
            # target, so the binaries differ while the behaviour does not.
            allocators = [a for a in allocators if a != "mimalloc_v3_override"]
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
                        "alpine": alpine,
                        "allocator": allocator,
                        "mt": str(mt).lower(),
                        "alloc_envs": alloc_envs(platform, family, allocator),
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
