#!/usr/bin/env python3
"""try/catch + JIT error-propagation fuzzer. Generate a JIT-eligible function `f`
whose body can throw on data-dependent edges (div/mod/quot by zero, explicit
error, nth out of range). Warm it past the tier threshold inside a try/catch loop
fed throwing and non-throwing args. Every engine (tree-walker = reference, VM
no-jit, VM+JIT, GC-stress) must agree on the accumulated checksum, and none may
abort — a Brood error raised inside JIT'd code must stay catchable, never crash.
"""
import random, sys

LEAVES = ["a", "b", "0", "1", "2", "-1", "3", "-3"]
# ops that can throw on an edge: quot/rem/mod by zero; otherwise plain arith
THROWY = ["math/quot", "math/rem", "math/mod"]
SAFE = ["+", "-", "*"]

def expr(rng, depth):
    if depth <= 0 or rng.random() < 0.4:
        return rng.choice(LEAVES)
    r = rng.random()
    if r < 0.4:
        # may throw: (math/quot X Y) where Y can be 0
        return f"({rng.choice(THROWY)} {expr(rng,depth-1)} {expr(rng,depth-1)})"
    if r < 0.7:
        return f"({rng.choice(SAFE)} {expr(rng,depth-1)} {expr(rng,depth-1)})"
    if r < 0.85:
        # explicit conditional throw
        return f"(if (< {expr(rng,depth-1)} 0) (error \"neg\") {expr(rng,depth-1)})"
    # nth that can go out of range — it answers nil rather than throwing now, so the
    # throw the leaf exists to exercise is made explicit
    return f"(or (nth [10 20 30] {expr(rng,depth-1)}) (error \"oob\"))"

def program(seed):
    rng = random.Random(seed)
    body = expr(rng, 4)
    # nested try/catch: inner catches with a sentinel, then more arithmetic that
    # itself runs under an outer accumulation — exercises catch-then-continue.
    return f"""(defn f (a b) {body})
(defn lp (i acc)
  (if (= i 0) acc
    (lp (- i 1)
      (+ acc
        (try (math/rem (+ 1000 (f (- (math/rem i 9) 4) (- (math/rem i 5) 2))) 100)
             (catch e (+ 7 (try (f (math/rem i 3) 0) (catch e2 3)))))))))
(io/puts (lp 6000 0))
"""

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/tc_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} try/catch programs")
