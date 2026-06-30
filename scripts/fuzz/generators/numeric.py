#!/usr/bin/env python3
"""Differential fuzzer for Brood's numeric engines.

Generates randomized, deterministic, number-closed Brood programs. Each defines
`f(a b c)` whose body is a random numeric expression tree, then warms it in a
tail loop (past the JIT tier threshold ~8) accumulating a pr-str checksum of
EVERY call result. The program prints that checksum.

The same program is run under all engines; identical input must yield identical
output. Any mismatch (value, TYPE — 5 vs 5.0 —, or one-errors-other-doesn't) is a
real bug, because the tree-walker is the reference semantics.
"""
import random, sys

# number-closed leaves: params, ints (incl. huge → bignum), floats, negatives
def leaf(rng, params):
    r = rng.random()
    if r < 0.45:
        return rng.choice(params)
    if r < 0.6:
        return str(rng.choice([0, 1, 2, -1, 3, -2, 7, -8, 100, -100]))
    if r < 0.75:
        # huge ints to exercise i64 edges + bignum promotion + float precision loss
        return str(rng.choice([2**62, -2**62, 2**63 - 1, -(2**63), 9007199254740993,
                               2**70, -(2**70), 9223372036854775807]))
    # floats incl. tricky values
    return rng.choice(["0.0", "-0.0", "1.5", "2.0", "0.1", "3.14", "-2.5",
                       "1e308", "-1e16", "0.5", "100.0"])

# number→number and number→bool(→if) ops, all keeping the tree number-closed
BIN = ["+", "-", "*", "max", "min", "quot", "rem", "mod"]
CMP = ["<", "<=", ">", ">=", "=", "not="]

def expr(rng, params, depth):
    if depth <= 0 or rng.random() < 0.3:
        return leaf(rng, params)
    r = rng.random()
    if r < 0.5:
        op = rng.choice(BIN)
        return f"({op} {expr(rng,params,depth-1)} {expr(rng,params,depth-1)})"
    if r < 0.7:
        # comparison folded into an if so the result stays numeric (and exercises
        # the exact spot the float= bug lived: a CMP whose operands may be mixed)
        op = rng.choice(CMP)
        c = f"({op} {expr(rng,params,depth-1)} {expr(rng,params,depth-1)})"
        return f"(if {c} {expr(rng,params,depth-1)} {expr(rng,params,depth-1)})"
    if r < 0.82:
        return f"(abs {expr(rng,params,depth-1)})"
    if r < 0.9:
        # let-binding to exercise slot reuse / float-slot tracking
        return f"(let (t {expr(rng,params,depth-1)}) ({rng.choice(BIN)} t {expr(rng,params,depth-1)}))"
    # division can yield float or error (div-by-zero) — both must match across engines
    return f"(quot {expr(rng,params,depth-1)} (max 1 (abs {expr(rng,params,depth-1)})))"

def program(seed):
    rng = random.Random(seed)
    params = ["a", "b", "c"]
    body = expr(rng, params, 4)
    # args derived from i: a int, b float, c mixed — guarantees float paths warm.
    return f"""(defn f (a b c) {body})
(defn lp (i acc)
  (if (= i 0) acc
    (lp (- i 1)
      (str acc "|"
        (try (pr-str (f (- (rem i 11) 5)
                        (/ (- i 50) 4.0)
                        (if (< (rem i 3) 1) (- i 30) (* 1.5 (- i 20)))))
             (catch e "ERR"))))))
(println (lp 120 ""))
"""

if __name__ == "__main__":
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    base = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    outdir = sys.argv[3] if len(sys.argv) > 3 else "."
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/fz_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} programs (seeds {base}..{base+n-1}) to {outdir}")
