#!/usr/bin/env python3
"""Parse `cargo bench --bench eval` (engine-grid) output on stdin and print each
engine's load-robust ratio against the reference engine, per workload.

The eval benches pin each row to an engine via `set_forced_engine`, so every size N
appears as *adjacent* rows — `(Vm, N)`, `(Tw, N)`, … — in the same process. Adjacency is
the point: under load they slow down together, so the ratio holds where the absolute
times wander (see docs/benchmarking.md §1). Ratio < 1 ⇒ that engine beats the reference.

Engine-agnostic: labels come from `Engine::short()` in
`crates/lisp/src/eval/compile/mod.rs` and the grid is built from `Engine::ALL`, so a third
engine shows up here as an extra column without touching this script. `Tw` (the
tree-walker) is the reference because it is the stable in-process baseline the methodology
rests on — it is not the engine under test.
"""
import math
import re
import sys

TIME = re.compile(r"([\d.]+)\s*(ns|µs|us|ms|s)\b")
# Any engine label the bench emits, not a fixed pair.
LEAF = re.compile(r"\(([A-Za-z][A-Za-z0-9_]*),\s*(\d+)\)")
NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
UNIT = {"ns": 1.0, "µs": 1e3, "us": 1e3, "ms": 1e6, "s": 1e9}

REFERENCE = "Tw"


def strip_tree(s: str) -> str:
    return s.lstrip("│├╰└─┬ \t")


def open_input(argv: list):
    """Return the line source: a named file if one was given, else stdin.

    Reading a bare `sys.stdin` that is a terminal blocks forever waiting for an EOF
    that never arrives — the parser's most common way to "hang". Refuse that up front
    with a usage message instead of stalling, and prefer an explicit file argument so
    the bench output can be captured to disk and parsed as a separate, cheap step.
    """
    if len(argv) > 1:
        path = argv[1]
        if path in ("-h", "--help"):
            print(f"usage: {argv[0]} [bench-output-file]   (reads stdin if no file given)")
            sys.exit(0)
        return open(path, "r", encoding="utf-8", errors="replace")
    if sys.stdin.isatty():
        print(
            f"usage: {argv[0]} [bench-output-file]   (or pipe bench output on stdin)\n"
            "  refusing to read an interactive terminal — it would block forever.",
            file=sys.stderr,
        )
        sys.exit(2)
    return sys.stdin


def main() -> int:
    cur = None
    # data[bench][n][eng] = (display_str, nanoseconds)
    data: dict = {}
    for raw in open_input(sys.argv):
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

    # Every non-reference engine seen anywhere, in first-seen order.
    engines: list = []
    for bench in data:
        for n in data[bench]:
            for eng in data[bench][n]:
                if eng != REFERENCE and eng not in engines:
                    engines.append(eng)

    rows = []
    for bench in data:
        for n in sorted(data[bench]):
            e = data[bench][n]
            if REFERENCE not in e:
                continue
            ref_s, ref_ns = e[REFERENCE]
            cells = []
            for eng in engines:
                if eng in e:
                    got_s, got_ns = e[eng]
                    cells.append((got_s, got_ns / ref_ns if ref_ns else float("nan")))
                else:
                    cells.append(("-", None))
            # Emit the row whenever the REFERENCE was measured, even if a subject engine has
            # no row for this size — that prints as "-" rather than vanishing. The previous
            # behaviour (inherited from the two-engine version, which required both engines
            # present) dropped such a size silently, so an interrupted or partially-failed
            # bench run read as though that size had never been measured.
            rows.append((bench, n, ref_s, cells))

    if not engines:
        print(
            f"bench-ratio: only ({REFERENCE}, N) rows in input — nothing to compare against.",
            file=sys.stderr,
        )
        return 1
    if not rows:
        print(
            f"bench-ratio: no ({REFERENCE}, N) reference rows found in input.",
            file=sys.stderr,
        )
        print("  (run on `cargo bench --bench eval` engine-grid output)", file=sys.stderr)
        return 1

    hdr = f"{'bench':<24}{'size':>11}{'tree-walker':>14}"
    for eng in engines:
        hdr += f"{eng:>14}{eng + '/' + REFERENCE:>9}"
    print(hdr)
    print("-" * len(hdr))
    for bench, n, ref_s, cells in rows:
        line = f"{bench:<24}{n:>11}{ref_s:>14}"
        notes = []
        for eng, (got_s, ratio) in zip(engines, cells):
            if ratio is None:
                line += f"{got_s:>14}{'-':>9}"
                continue
            # A non-finite ratio means the reference timed as 0 — too fast to resolve at this
            # script's millisecond-ish granularity, not "the same speed". Saying "~parity"
            # there (which the comparisons below would, since every comparison against nan is
            # False) reports equality that was never measured.
            if not math.isfinite(ratio):
                line += f"{got_s:>14}{'n/a':>9}"
                notes.append(f"{eng} unresolved (reference measured 0)")
                continue
            line += f"{got_s:>14}{ratio:>9.2f}"
            if ratio < 0.98:
                notes.append(f"{eng} {(1 - ratio) * 100:.0f}% faster")
            elif ratio > 1.02:
                notes.append(f"{eng} {(ratio - 1) * 100:.0f}% slower")
            else:
                notes.append(f"{eng} ~parity")
        print(line + ("  " + ", ".join(notes) if notes else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
