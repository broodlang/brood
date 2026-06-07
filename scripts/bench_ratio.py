#!/usr/bin/env python3
"""Parse `cargo bench --bench eval` (engine-grid) output on stdin and print the
load-robust VM ÷ tree-walker ratio per workload.

The eval benches pin each row to an engine via `set_forced_engine`, so every
size N appears as adjacent `(Vm, N)` and `(Tw, N)` rows in the *same* process.
We pair them and report VM/TW — the metric that survives the machine load that
wrecks absolute times (see docs/benchmarking.md). Ratio < 1 ⇒ the VM wins.
"""
import sys
import re

TIME = re.compile(r"([\d.]+)\s*(ns|µs|us|ms|s)\b")
LEAF = re.compile(r"\((Vm|Tw),\s*(\d+)\)")
NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
UNIT = {"ns": 1.0, "µs": 1e3, "us": 1e3, "ms": 1e6, "s": 1e9}


def strip_tree(s: str) -> str:
    return s.lstrip("│├╰└─┬ \t")


def main() -> int:
    cur = None
    # data[bench][n][eng] = (display_str, nanoseconds)
    data: dict = {}
    for raw in sys.stdin:
        s = strip_tree(raw.rstrip("\n"))
        if not s:
            continue
        m = LEAF.search(s)
        if m:
            eng, n = m.group(1), int(m.group(2))
            times = TIME.findall(s)
            if len(times) >= 3:  # fastest, slowest, median, mean[, ...]
                val, u = times[2]  # median
                data.setdefault(cur or "?", {}).setdefault(n, {})[eng] = (
                    f"{val} {u}",
                    float(val) * UNIT[u],
                )
            continue
        # A parent bench-fn line: a bare identifier with no timing columns.
        label = s.split("  ")[0].strip()
        if NAME.match(label) and label != "eval" and not TIME.search(s):
            cur = label

    rows = []
    for bench in data:
        for n in sorted(data[bench]):
            e = data[bench][n]
            if "Vm" in e and "Tw" in e:
                (vm_s, vm_ns), (tw_s, tw_ns) = e["Vm"], e["Tw"]
                ratio = vm_ns / tw_ns if tw_ns else float("nan")
                rows.append((bench, n, tw_s, vm_s, ratio))

    if not rows:
        print("bench-ratio: no paired (Vm, N)/(Tw, N) rows found in input.", file=sys.stderr)
        print("  (run on `cargo bench --bench eval` engine-grid output)", file=sys.stderr)
        return 1

    hdr = f"{'bench':<24}{'size':>11}{'tree-walker':>14}{'VM':>14}{'VM/TW':>9}"
    print(hdr)
    print("-" * len(hdr))
    for bench, n, tw_s, vm_s, ratio in rows:
        if ratio < 0.98:
            mark = f"  {(1 - ratio) * 100:.0f}% faster"
        elif ratio > 1.02:
            mark = f"  {(ratio - 1) * 100:.0f}% slower"
        else:
            mark = "  ~parity"
        print(f"{bench:<24}{n:>11}{tw_s:>14}{vm_s:>14}{ratio:>9.2f}{mark}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
