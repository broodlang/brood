#!/usr/bin/env python3
"""Bad-arithmetic fuzzer: edge-case-heavy numeric programs. Each warms `f` past
the JIT tier threshold, accumulating a pr-str-or-error sentinel of every result,
and prints it. All engines must agree (value, TYPE, or identical error sentinel);
none may crash. Targets div/mod-by-zero, overflow, i64::MIN/-1, shifts, mixed
int/float/bignum/decimal, NaN/inf propagation.
"""
import random, sys

LEAVES = [
    "a", "b", "0", "1", "-1", "2",
    "9223372036854775807", "-9223372036854775808",     # i64 MAX/MIN
    "9223372036854775808", "-9223372036854775809",      # just past i64
    "0.0", "-0.0", "1.5", "1e308", "1e400", "-1e400",    # floats incl inf
    "nan", "inf", "-inf",
    "100000000000000000000", "-100000000000000000000",   # bignums
    "1M", "1.5M", "-2.5M", "0M",                          # decimals
    "0.1", "3", "64", "65", "-5",
]
# ops that bite: division family (zero!), shifts (huge/neg!), mixed-tower arith/compare
BIN = ["+", "-", "*", "/", "quot", "rem", "mod", "max", "min",
       "bit-and", "bit-or", "bit-xor", "bit-shl", "bit-shr"]
CMP = ["<", "<=", ">", ">=", "=", "not="]
UN = ["abs", "-", "inc", "dec"]

def expr(rng, depth):
    if depth <= 0 or rng.random() < 0.35:
        return rng.choice(LEAVES)
    r = rng.random()
    if r < 0.55:
        return f"({rng.choice(BIN)} {expr(rng,depth-1)} {expr(rng,depth-1)})"
    if r < 0.72:
        c = f"({rng.choice(CMP)} {expr(rng,depth-1)} {expr(rng,depth-1)})"
        return f"(if {c} {expr(rng,depth-1)} {expr(rng,depth-1)})"
    if r < 0.85:
        return f"({rng.choice(UN)} {expr(rng,depth-1)})"
    return f"(let (t {expr(rng,depth-1)}) ({rng.choice(BIN)} t {expr(rng,depth-1)}))"

def program(seed):
    rng = random.Random(seed)
    body = expr(rng, 4)
    return f"""(defn f (a b) {body})
(defn lp (i acc)
  (if (= i 0) acc
    (lp (- i 1)
      (str acc "|"
        (try (pr-str (f (- (rem i 13) 6) (* 1.0 (- (rem i 7) 3))))
             (catch e "E"))))))
(println (lp 80 ""))
"""

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/ar_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} arithmetic programs")
