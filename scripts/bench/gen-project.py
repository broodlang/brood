#!/usr/bin/env python3
"""Generate a large synthetic project for the ADR-218 / startup measurements.

    scripts/bench/gen-project.py 16300 /tmp/brood-big

N modules of ~180 lines each, plus an entry point that reaches exactly TWO of them —
which is the case the lazy image exists for (an entry should pay for what it reaches,
not for the codebase). Pair with `image-scale.sh`.

Shape matches the calibration in the devlog (a project 10x `~/src/flt/moneyclub`:
16 300 files averaging ~180 lines) — real files average ~180 lines, and synthetic
1000-line modules flatter the runtime, so the per-file constant is what dominates.

The entry point deliberately touches only TWO modules, which is the whole point of
the lazy image: an entry should pay for what it reaches, not for the codebase.
"""
import os
import sys

N = int(sys.argv[1]) if len(sys.argv) > 1 else 16300
ROOT = sys.argv[2] if len(sys.argv) > 2 else "/tmp/brood-big"

SRC = os.path.join(ROOT, "src")
os.makedirs(SRC, exist_ok=True)

with open(os.path.join(ROOT, "project.blsp"), "w") as f:
    f.write('(project\n  :name big\n  :main app)\n')

# The entry point: reaches `mod0` and `helper`, nothing else.
with open(os.path.join(SRC, "app.blsp"), "w") as f:
    f.write(
        '(defmodule app (:use mod0) (:use helper))\n\n'
        '(defn main ()\n'
        '  (println (str "ANSWER: " (+ (mod0-total 3) (helper-scale 4)))))\n'
    )

with open(os.path.join(SRC, "helper.blsp"), "w") as f:
    f.write(
        '(defmodule helper "A small module the entry point reaches.")\n\n'
        '(defn helper-scale (x) (* x 10))\n'
    )


def body(i):
    """~180 lines of ordinary module code: defns, a record, a little dispatch."""
    L = []
    L.append(f'(defmodule mod{i} "Generated module {i} — measurement fixture.")')
    L.append("")
    L.append(f"(defrecord rec{i} (a b))")
    L.append("")
    for k in range(20):
        L.append(f"(defn m{i}-f{k} (x)")
        L.append(f'  "Compute variant {k} of module {i}."')
        L.append(f"  (let (y (+ x {k + 1})")
        L.append(f"        z (* y {i % 7 + 2}))")
        L.append("    (cond")
        L.append("      (< z 0) 0")
        L.append(f"      (= z {k}) (- z 1)")
        L.append("      else (+ z 1))))")
        L.append("")
    L.append(f"(defn mod{i}-total (x)")
    L.append(f"  (fold (fn (acc f) (+ acc (f x))) 0")
    L.append("    (list " + " ".join(f"m{i}-f{k}" for k in range(20)) + ")))")
    L.append("")
    return "\n".join(L)


for i in range(N):
    with open(os.path.join(SRC, f"mod{i}.blsp"), "w") as f:
        f.write(body(i))

files = len(os.listdir(SRC))
lines = sum(1 for _ in open(os.path.join(SRC, "mod0.blsp")))
print(f"{ROOT}: {files} source files, {lines} lines each (module 0)")
