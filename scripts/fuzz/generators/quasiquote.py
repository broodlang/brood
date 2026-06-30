#!/usr/bin/env python3
"""Quasiquote fuzzer with an explicit-constructor oracle. Build a template tree,
render it two ways — as a `quasiquote with ~ / ~@, and as the equivalent explicit
`(concat (list ...) ...)` construction — and assert they are `=`. Literals are
ints/keywords only (self-quoting both ways). Vars in scope: x=7, y=:m, and the
spliceable lists xs/ys/ws. Each program prints OK / BAD; runs under all engines.
"""
import random, sys

PRELUDE_VARS = "(let (x 7 y :m xs (list 1 2 3) ys (list) ws (list :a :b))"

def leaf(rng):
    r = rng.random()
    if r < 0.4:  return ("lit", rng.choice(["1", "2", "0", ":k", ":q", "42"]))
    if r < 0.7:  return ("unq", rng.choice(["x", "y"]))      # ~x
    return ("spl", rng.choice(["xs", "ys", "ws"]))           # ~@xs

def gen(rng, depth):
    # a list node with 0..4 children; children are leaves or (rarely) nested lists
    n = rng.randint(0, 4)
    kids = []
    for _ in range(n):
        if depth > 0 and rng.random() < 0.25:
            kids.append(gen(rng, depth-1))
        else:
            kids.append(leaf(rng))
    return ("list", kids)

def qq(node):
    if node[0] == "lit": return node[1]
    if node[0] == "unq": return "~" + node[1]
    if node[0] == "spl": return "~@" + node[1]
    return "(" + " ".join(qq(k) for k in node[1]) + ")"

def val_seg(node):
    # the segment this node contributes to its parent list's concat
    if node[0] == "spl":
        return node[1]                       # splice: the list itself
    return "(list " + val_elem(node) + ")"   # one element

def val_elem(node):
    # the single value this node denotes (when not splicing)
    if node[0] == "lit": return node[1]
    if node[0] == "unq": return node[1]
    if node[0] == "spl": return node[1]      # (shouldn't be hit via val_elem)
    return "(concat " + " ".join(val_seg(k) for k in node[1]) + ")"

def program(seed):
    rng = random.Random(seed)
    t = gen(rng, 3)
    qq_form = "`" + qq(t)
    explicit = "(concat " + " ".join(val_seg(k) for k in t[1]) + ")"
    return (f"{PRELUDE_VARS}\n"
            f"  (let (a {qq_form}\n"
            f"        b {explicit})\n"
            f"    (println (if (= a b) \"OK\" (str \"BAD a=\" (pr-str a) \" b=\" (pr-str b))))))\n")

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/qq_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} quasiquote programs")
