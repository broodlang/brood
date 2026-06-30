#!/usr/bin/env python3
"""Metamorphic fuzzer. Generate a base numeric body E(a,b), then emit several
semantics-PRESERVING rewrites of the function body. Every variant must produce the
identical warmed-loop checksum on every engine — any difference is a JIT/VM
lowering bug for that shape (the value logic is unchanged). Each seed writes
variant files mt_<seed>_v<k>.blsp; the runner asserts all variants agree.
"""
import random, sys

def leaf(rng):
    r = rng.random()
    if r < 0.5: return rng.choice(["a", "b"])
    if r < 0.7: return str(rng.choice([0,1,2,-1,3,-2,7]))
    if r < 0.85: return str(rng.choice([2**60, -(2**60), 9007199254740993]))
    return rng.choice(["0.0","1.5","2.0","-2.5","100.0"])

OPS = ["+","-","*","max","min","quot","rem"]
def expr(rng, d):
    if d <= 0 or rng.random() < 0.35: return leaf(rng)
    r = rng.random()
    if r < 0.6: return f"({rng.choice(OPS)} {expr(rng,d-1)} {expr(rng,d-1)})"
    if r < 0.8:
        c = f"({rng.choice(['<','<=','>','=' ])} {expr(rng,d-1)} {expr(rng,d-1)})"
        return f"(if {c} {expr(rng,d-1)} {expr(rng,d-1)})"
    return f"(let (t {expr(rng,d-1)}) ({rng.choice(OPS)} t {expr(rng,d-1)}))"

# semantics-preserving rewrites of a body E (string), all == E
def variants(E):
    return [
        E,                                              # v0 base
        f"(do nil {E})",                                # v1 do-wrap
        f"(let (zz (+ a 1)) {E})",                      # v2 dead let (zz unused)
        f"(let (rr {E}) rr)",                           # v3 bind result
        f"(if (= 0 0) {E} 0)",                          # v4 always-true if
        f"(let (a2 a b2 b) (let (a a2 b b2) {E}))",     # v5 rebind params (identity)
        f"((fn () {E}))",                               # v6 immediately-invoked thunk
    ]

def program(E_variant):
    return (f"(defn f (a b) {E_variant})\n"
            f"(defn lp (i acc)\n"
            f"  (if (= i 0) acc\n"
            f"    (lp (- i 1)\n"
            f"      (str acc \"|\" (try (pr-str (f (- (rem i 11) 5) (- (rem i 7) 3))) (catch e \"E\"))))))\n"
            f"(println (lp 200 \"\"))\n")

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    nv = 0
    for k in range(n):
        seed = base + k
        rng = random.Random(seed)
        E = expr(rng, 4)
        for vi, V in enumerate(variants(E)):
            with open(f"{outdir}/mm_{seed}_v{vi}.blsp", "w") as fh:
                fh.write(program(V))
            nv += 1
    print(f"wrote {nv} variant programs ({n} seeds x 7 variants)")
