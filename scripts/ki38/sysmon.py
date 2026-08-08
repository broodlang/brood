#!/usr/bin/env python3
"""KI-38: a 1 Hz system-state timeline, written beside a suite run.

`support::stall_report` gives one snapshot at the instant a boot wait gives up.
That answers "what state was the child in", but not "what was the box doing in
the 30 s leading up to it" -- which is what separates the release_bundle-spike
hypothesis from everything else. This fills that in.

Reads /proc only. It spawns no `brood`, so it cannot re-create the KI-29 orphan
leak the previous session's sampler did.
"""
import os
import sys
import time

OUT = sys.argv[1]
INTERVAL = 1.0


def meminfo():
    d = {}
    with open("/proc/meminfo") as f:
        for line in f:
            k, _, v = line.partition(":")
            d[k] = int(v.split()[0])
    return d


def vmstat():
    d = {}
    with open("/proc/vmstat") as f:
        for line in f:
            k, _, v = line.partition(" ")
            d[k] = int(v)
    return d


def procs():
    """(#brood/nest, #of those in D state, #test binaries running, rss_kb_total)."""
    nbrood = nd = ntest = 0
    rss = 0
    interesting = []
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            with open(f"/proc/{pid}/cmdline") as f:
                cmd = f.read().replace("\0", " ").strip()
            if not cmd:
                continue
            is_brood = "/brood" in cmd or "/nest" in cmd
            is_test = "/target/debug/deps/" in cmd
            if not (is_brood or is_test):
                continue
            with open(f"/proc/{pid}/stat") as f:
                stat = f.read()
            state = stat.rsplit(")", 1)[1].split()[0]
            with open(f"/proc/{pid}/statm") as f:
                rss_pages = int(f.read().split()[1])
            rss += rss_pages * 4
            if is_brood:
                nbrood += 1
                if state == "D":
                    nd += 1
            if is_test:
                ntest += 1
                # name the running test binary so the timeline shows WHICH
                # region of the schedule we are in (release_bundle et al).
                base = cmd.split("/target/debug/deps/")[1].split()[0]
                interesting.append(base.split("-")[0])
        except (OSError, IndexError, ValueError):
            continue
    return nbrood, nd, ntest, rss, ",".join(sorted(set(interesting)))


prev = vmstat()
with open(OUT, "w", buffering=1) as out:
    out.write(
        "ts,load1,mem_avail_kb,swap_free_kb,shmem_kb,dirty_kb,"
        "d_pgmajfault,d_pswpin,d_pswpout,n_brood,n_brood_D,n_testbin,testbin_rss_kb,running\n"
    )
    while True:
        try:
            mi = meminfo()
            vm = vmstat()
            load1 = open("/proc/loadavg").read().split()[0]
            nbrood, nd, ntest, rss, running = procs()
            out.write(
                f"{time.time():.1f},{load1},{mi['MemAvailable']},{mi['SwapFree']},"
                f"{mi['Shmem']},{mi['Dirty']},"
                f"{vm['pgmajfault'] - prev['pgmajfault']},"
                f"{vm['pswpin'] - prev['pswpin']},{vm['pswpout'] - prev['pswpout']},"
                f"{nbrood},{nd},{ntest},{rss},{running}\n"
            )
            prev = vm
        except Exception as e:  # never let the instrument kill the hunt
            out.write(f"{time.time():.1f},ERR,{e}\n")
        time.sleep(INTERVAL)
