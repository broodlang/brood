#!/usr/bin/env python3
"""Characterise one hunt round: how close did the box get to memory pressure,
and where in the schedule did the release_bundle region fall?

The KI-38 hypothesis under test is that the release_bundle tests (407 MB of
tmpfs each, two concurrently) drive the spike that stalls a booting child. This
prints the numbers that support or refute it, per round, on GREEN runs too --
which is the point: a green run still shows whether the pressure exists.
"""
import csv
import sys

path = sys.argv[1]
rows = []
with open(path) as f:
    for r in csv.DictReader(f):
        if r.get("load1") in (None, "ERR"):
            continue
        try:
            rows.append(
                {
                    "ts": float(r["ts"]),
                    "load": float(r["load1"]),
                    "avail": int(r["mem_avail_kb"]),
                    "swapfree": int(r["swap_free_kb"]),
                    "shmem": int(r["shmem_kb"]),
                    "majflt": int(r["d_pgmajfault"]),
                    "swpin": int(r["d_pswpin"]),
                    "swpout": int(r["d_pswpout"]),
                    "nbrood": int(r["n_brood"]),
                    "nd": int(r["n_brood_D"]),
                    "rss": int(r["testbin_rss_kb"]),
                    "running": r["running"] or "",
                }
            )
        except (ValueError, KeyError):
            continue

if not rows:
    print("no samples")
    sys.exit(1)

t0 = rows[0]["ts"]
dur = rows[-1]["ts"] - t0
print(f"{path}: {len(rows)} samples over {dur:.0f}s\n")


def col(k):
    return [r[k] for r in rows]


def show(label, k, unit="", scale=1.0, lo=True):
    v = sorted(col(k))
    mn, mx = v[0] / scale, v[-1] / scale
    med = v[len(v) // 2] / scale
    print(f"  {label:22s} min {mn:10.1f}  med {med:10.1f}  max {mx:10.1f} {unit}")


print("system envelope over the round:")
show("MemAvailable", "avail", "GB", 1024 * 1024)
show("SwapFree", "swapfree", "GB", 1024 * 1024)
show("Shmem (tmpfs)", "shmem", "GB", 1024 * 1024)
show("loadavg1", "load")
show("test-binary RSS", "rss", "GB", 1024 * 1024)
print(f"  {'total major faults':22s} {sum(col('majflt')):d}")
print(f"  {'total swap-in pages':22s} {sum(col('swpin')):d}")
print(f"  {'total swap-out pages':22s} {sum(col('swpout')):d}")
print(f"  {'samples with a D-state brood':22s} {sum(1 for r in rows if r['nd'])}")

# The release_bundle region.
rb = [r for r in rows if "release_bundle" in r["running"]]
if rb:
    s, e = rb[0]["ts"] - t0, rb[-1]["ts"] - t0
    print(f"\nrelease_bundle region: t+{s:.0f}s .. t+{e:.0f}s ({len(rb)} samples)")
    print(
        f"  MemAvailable during: min {min(r['avail'] for r in rb)/1048576:.1f} GB"
        f"  (round min {min(col('avail'))/1048576:.1f} GB)"
    )
    print(f"  Shmem during:        max {max(r['shmem'] for r in rb)/1048576:.1f} GB")
    print(f"  loadavg during:      max {max(r['load'] for r in rb):.1f}")
else:
    print("\nrelease_bundle never seen running (sampling missed it, or it is fast)")

# Where the round was tightest on memory, and what was running there.
tight = min(rows, key=lambda r: r["avail"])
print(f"\ntightest memory at t+{tight['ts']-t0:.0f}s: {tight['avail']/1048576:.1f} GB avail,")
print(f"  swapfree {tight['swapfree']/1048576:.2f} GB, load {tight['load']}, running: {tight['running']}")

# Any sample where a brood sat in D.
dstate = [r for r in rows if r["nd"]]
if dstate:
    print(f"\nD-state brood sightings ({len(dstate)}):")
    for r in dstate[:20]:
        print(
            f"  t+{r['ts']-t0:6.0f}s  nD={r['nd']}/{r['nbrood']}  "
            f"avail {r['avail']/1048576:.1f} GB  majflt {r['majflt']}  running: {r['running']}"
        )
