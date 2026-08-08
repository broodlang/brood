#!/usr/bin/env python3
"""KI-38 dose-response: does COLD-cache boot contention scale into the 20-30 s
boot-wait deadlines, or does it plateau well below them?

The suite's opening window was measured at 27-29 concurrent `brood` processes on
12 cores. A cold boot costs ~1.23 s alone (expand=1.10 s of it). This sweeps the
herd size against a shared COLD cache and reports the worst boot in each herd --
the number the deadline is actually racing.

Each herd gets a fresh XDG_CACHE_HOME so every child in it misses, which is the
post-rebuild state. Samples total brood RSS while the herd runs and aborts the
sweep before the box is put at risk.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BROOD = os.path.join(REPO, "target", "debug", "brood")
WORK = tempfile.mkdtemp(prefix="ki38-dose-")
PROG = os.path.join(WORK, "up.blsp")
# Mirror what child_cleanup's child does: boot, then announce by writing a file.
open(PROG, "w").write('(spit "%s/marker" "up")\n' % WORK)

# Stop escalating if the box gets tight. 30 GB box; leave a wide margin.
MIN_AVAIL_KB = 8 * 1024 * 1024


def mem_avail():
    for line in open("/proc/meminfo"):
        if line.startswith("MemAvailable"):
            return int(line.split()[1])
    return 0


def brood_rss_kb():
    total = 0
    n = 0
    nd = 0
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            cmd = open(f"/proc/{pid}/cmdline").read()
            if "/target/debug/brood" not in cmd:
                continue
            total += int(open(f"/proc/{pid}/statm").read().split()[1]) * 4
            n += 1
            st = open(f"/proc/{pid}/stat").read()
            if st.rsplit(")", 1)[1].split()[0] == "D":
                nd += 1
        except (OSError, IndexError, ValueError):
            continue
    return total, n, nd


def run_herd(n, cache, label):
    """Launch n children at once; return per-child wall times in ms."""
    peak = {"rss": 0, "n": 0, "d": 0, "avail": 1 << 40}
    stop = threading.Event()

    def sampler():
        while not stop.is_set():
            rss, cnt, nd = brood_rss_kb()
            peak["rss"] = max(peak["rss"], rss)
            peak["n"] = max(peak["n"], cnt)
            peak["d"] = max(peak["d"], nd)
            peak["avail"] = min(peak["avail"], mem_avail())
            time.sleep(0.1)

    t = threading.Thread(target=sampler, daemon=True)
    t.start()

    env = dict(os.environ, XDG_CACHE_HOME=cache)
    times = [None] * n
    procs = []
    t_start = time.time()
    for i in range(n):
        procs.append((i, time.time(), subprocess.Popen(
            [BROOD, PROG], env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)))
    for i, t0, p in procs:
        p.wait()
        times[i] = (time.time() - t0) * 1000
    wall = (time.time() - t_start) * 1000
    stop.set()
    t.join(timeout=1)

    times.sort()
    print(
        f"  {label:>6s} n={n:<4d} min={times[0]:7.0f}ms  med={times[len(times)//2]:7.0f}ms  "
        f"max={times[-1]:8.0f}ms  wall={wall:8.0f}ms   "
        f"peak: {peak['n']} procs, {peak['rss']/1048576:.1f} GB RSS, "
        f"{peak['d']} in D, avail-min {peak['avail']/1048576:.1f} GB"
    )
    sys.stdout.flush()
    return times[-1], peak


print(f"cold vs warm boot herds — 12 cores; deadlines are 20 s / 30 s / 30 s")
print(f"(the suite's opening window was measured at 27-29 concurrent brood)\n")

try:
    for n in (8, 16, 24, 32, 48, 64, 96, 128):
        if mem_avail() < MIN_AVAIL_KB:
            print(f"  stopping: only {mem_avail()/1048576:.1f} GB available")
            break
        cold = os.path.join(WORK, f"cold{n}")
        os.makedirs(cold, exist_ok=True)
        worst, peak = run_herd(n, cold, "COLD")
        if peak["avail"] < MIN_AVAIL_KB:
            print(f"  stopping escalation: herd n={n} pulled available memory to "
                  f"{peak['avail']/1048576:.1f} GB")
            break
    print()
    warm = os.path.join(WORK, "warm")
    os.makedirs(warm, exist_ok=True)
    subprocess.run([BROOD, PROG], env=dict(os.environ, XDG_CACHE_HOME=warm),
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for n in (16, 32, 64):
        run_herd(n, warm, "warm")
finally:
    shutil.rmtree(WORK, ignore_errors=True)
