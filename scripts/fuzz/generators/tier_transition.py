#!/usr/bin/env python3
"""JIT tier-transition fuzzer. A function's hot arm is fed arguments whose TYPE
cycles every iteration (int -> float -> bignum -> int), so the JIT tiers on one
type then must deopt/retier as the type flips — hammering the as_int/as_f64 tag
guards and the overflow->bignum deopt. All engines (TW / no-jit / JIT / GC-stress)
must produce the identical checksum.
"""
import random, sys

def leaf(rng): return rng.choice(["x","1","2","-1","3","0"])
OPS=["+","-","*","max","min"]
def expr(rng,d):
    if d<=0 or rng.random()<0.4: return leaf(rng)
    r=rng.random()
    if r<0.65: return f"({rng.choice(OPS)} {expr(rng,d-1)} {expr(rng,d-1)})"
    if r<0.85:
        c=f"({rng.choice(['<','<=','>','='])} {expr(rng,d-1)} {expr(rng,d-1)})"
        return f"(if {c} {expr(rng,d-1)} {expr(rng,d-1)})"
    return f"(abs {expr(rng,d-1)})"

def program(seed):
    rng=random.Random(seed)
    body=expr(rng,4)
    # pick cycles the arg type by phase: int, float, bignum (overflow-prone), small int
    return (f"(defn f (x) {body})\n"
            f"(defn pick (i)\n"
            f"  (cond (< (rem i 4) 1) (- (rem i 200) 100)\n"          # int (neg+pos)
            f"        (< (rem i 4) 2) (* 1.0 (- (rem i 200) 100))\n"  # float
            f"        (< (rem i 4) 3) (* 4611686018427387904 (+ 1 (rem i 6)))\n"  # bignum (past i64)
            f"        else (rem i 13)))\n"                            # small int
            f"(defn lp (i acc)\n"
            f"  (if (= i 0) acc\n"
            f"    (lp (- i 1) (str acc \"|\" (try (pr-str (f (pick i))) (catch e \"E\"))))))\n"
            f"(println (lp 4000 \"\"))\n")

if __name__=="__main__":
    n=int(sys.argv[1]); base=int(sys.argv[2]); outdir=sys.argv[3]
    for k in range(n):
        s=base+k
        open(f"{outdir}/tr_{s}.blsp","w").write(program(s))
    print(f"wrote {n} tier-transition programs")
