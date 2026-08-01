#!/usr/bin/env python3
"""Emit the allocator-bench job matrix as JSON for GITHUB_OUTPUT.

Expressing the matrix here instead of in YAML keeps it readable: the exclusions
are conditional on both the platform and the allocator, and as `exclude:` lists
they ran to dozens of entries that documented nothing.

Two axes deliberately do NOT appear here, because neither needs a rebuild and
the build is what costs minutes - the scripts loop over them inside one job:

* workload - `check` (24k proxies against refused loopback ports; stresses the
  per-proxy reqwest::Client construction) and `scrape` (~203k proxies from
  frozen CIDR corpora; stresses the dedup HashSet, the sort and serde_json).
* alloc_env - run-time allocator tuning. jemalloc reads MALLOC_CONF as conf
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

# `auto` is the shipped default feature set. Nothing in the previous benchmark
# ever built it, so the configuration users actually get was never measured.
# The `*_override` rows exist to answer the override question with data rather
# than reasoning: mimalloc's `override` and jemalloc's
# `override_allocator_on_supported_platforms` change what happens to the C
# dependencies' allocations (here aws-lc-sys), never to Rust's.
COMMON = ["system", "auto", "mimalloc_v2", "mimalloc_v3", "mimalloc_v3_override"]

# jemalloc-sys shells out to `sh` and `mingw32-make` and has no gnu_target
# mapping for aarch64-pc-windows-msvc, so it cannot be built on Windows at all.
JEMALLOC = ["jemalloc", "jemalloc_override"]

# The RSS side of the tokio-flavor question is already settled (multi_thread
# regressed 35 of 38 measured cells). What was never measured is whether it
# buys any wall clock, so it stays as a small confirmation subset now that time
# is recorded.
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


def alloc_envs(platform: str, family: str, allocator: str) -> str:
    variants = ["default"]
    if allocator.startswith("mimalloc") and family == "linux":
        variants.append("no_thp:MIMALLOC_ALLOW_THP=0")
    if allocator.startswith("jemalloc"):
        variants.append(JE_TUNED)
        # jemalloc-sys lists musl in NO_BG_THREAD_TARGETS, and macOS compiles
        # background threads out entirely, so the knob only exists on glibc.
        if family == "linux" and not platform.startswith("alpine"):
            variants.append(JE_BG)
    return ";".join(variants)


def main() -> None:
    cells = []
    for platform, runner, alpine, family in PLATFORMS:
        allocators = COMMON + ([] if family == "windows" else JEMALLOC)
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
