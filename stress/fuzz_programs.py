#!/usr/bin/env python3
"""Random-program differential fuzzer (Fuzzilli-lite, engine-diff flavour).

Generates seeded, DETERMINISTIC, terminating Brood programs from a small
expression grammar — pure arithmetic helpers (ints at the i64/bigint edges,
floats, comparisons, let/if nesting), a self-tail driver loop that calls them
and performs table effects (put/incr — non-idempotent on purpose: this is what
caught the deopt-rerun bug), and a printed digest. Each program runs under
several engine/GC/chaos configs; ANY stdout difference is a bug (Brood's
engines are bit-identical by mandate).

Usage:
    python3 stress/fuzz_programs.py [--seeds N] [--start S] [--keep]
Exit nonzero on any divergence; failing programs are kept in stress/fuzz_out/.
"""
import argparse, os, random, subprocess, sys, tempfile, shutil

BROOD = os.environ.get("BROOD", "target/release/brood")

CONFIGS = [
    ("jit", {}),
    ("no-jit", {"BROOD_NO_JIT": "1"}),
    ("gc-stress", {"BROOD_GC_STRESS": "1", "BROOD_NO_JIT": "1"}),
    ("chaos-preempt", {"BROOD_REDUCTIONS": "97"}),  # tiny prime budget: preempt storms
]

EDGE_INTS = [0, 1, -1, 2, 7, 63, 4096, 2**31, 2**60 - 1, -(2**60), 2**62, -(2**62)]

class Gen:
    def __init__(self, seed):
        self.r = random.Random(seed)
        self.fresh = 0

    def name(self, p):
        self.fresh += 1
        return f"{p}{self.fresh}"

    # -- pure int expression over `params`, depth-bounded, total ---------------
    def expr(self, params, depth):
        r = self.r
        if depth == 0 or r.random() < 0.3:
            if params and r.random() < 0.6:
                return r.choice(params)
            return str(r.choice(EDGE_INTS) if r.random() < 0.4 else r.randint(-999, 999))
        c = r.random()
        if c < 0.45:
            op = r.choice(["+", "-", "*", "max", "min", "bit-and", "bit-or", "bit-xor"])
            return f"({op} {self.expr(params, depth-1)} {self.expr(params, depth-1)})"
        if c < 0.60:  # guarded integer division (nonzero literal divisor)
            op = r.choice(["quot", "rem"])
            d = r.choice([2, 3, 7, 97, -3, -8])
            return f"({op} {self.expr(params, depth-1)} {d})"
        if c < 0.75:
            cmp = r.choice(["<", "<=", "=", ">", ">="])
            return (f"(if ({cmp} {self.expr(params, depth-1)} {self.expr(params, depth-1)}) "
                    f"{self.expr(params, depth-1)} {self.expr(params, depth-1)})")
        if c < 0.88:
            v = self.name("t")
            return f"(let ({v} {self.expr(params, depth-1)}) {self.expr(params + [v], depth-1)})"
        # small vector build + constant-index read (exercises MakeVector/VectorRef)
        n = r.randint(2, 4)
        elems = " ".join(self.expr(params, depth-1) for _ in range(n))
        return f"(nth [{elems}] {r.randint(0, n-1)})"

    def program(self):
        r = self.r
        lines = ["(def t (table))"]
        helpers = []
        for _ in range(r.randint(1, 3)):
            f = self.name("f")
            arity = r.randint(1, 3)
            ps = [self.name("p") for _ in range(arity)]
            lines.append(f"(defn {f} ({' '.join(ps)}) {self.expr(ps, r.randint(2, 4))})")
            helpers.append((f, arity))
        # the effectful driver loop: calls helpers, puts + incrs the table
        # occasionally a float helper too — exercises the float JIT paths and
        # boxed-float flows through calls (printing is IEEE-deterministic)
        if r.random() < 0.3:
            g = self.name("g")
            lines.append(
                f"(defn {g} (x) (+ (* x 1.5) (/ (+ x 1) 4)))")
            helpers.append((g, 1))
        f, arity = r.choice(helpers)
        args = " ".join(["i"] * arity)
        key_mod = r.choice([8, 64, 512, 4095])
        body_bits = [f"(table-put t (rem i {key_mod}) ({f} {args}))",
                     f"(table-incr t {r.randint(0, 7)} 1)"]
        r.shuffle(body_bits)
        n_iters = r.choice([3000, 5000, 9000])
        lines.append(
            f"(defn drive (i n acc)\n"
            f"  (if (>= i n) acc\n"
            f"    (do {body_bits[0]}\n"
            f"      {body_bits[1]}\n"
            f"      (drive (+ i 1) n ({r.choice(["bit-xor", "+"])} acc (bit-and ({f} {args}) 268435455))))))")
        lines.append(f"(def acc (drive 0 {n_iters} 0))")
        # digest: accumulator + table contents
        lines.append(
            "(defn dig (k n s)\n"
            "  (if (>= k n) s\n"
            "    (dig (+ k 1) n (bit-xor s (* (+ k 1) (table-get t k 0))))))")
        lines.append(f'(println "digest" acc (dig 0 {key_mod} 0) (table-count t))')
        return "\n".join(lines) + "\n"

def run_one(path, env_extra):
    env = dict(os.environ)
    env.update(env_extra)
    env["BROOD_NO_CHECK"] = "1"
    try:
        out = subprocess.run([BROOD, path], capture_output=True, text=True,
                             timeout=120, env=env)
        return f"exit={out.returncode}\n{out.stdout}"
    except subprocess.TimeoutExpired:
        return "TIMEOUT"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", type=int, default=25)
    ap.add_argument("--start", type=int, default=1)
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()
    outdir = "stress/fuzz_out"
    os.makedirs(outdir, exist_ok=True)
    bad = 0
    for seed in range(args.start, args.start + args.seeds):
        src = Gen(seed).program()
        path = os.path.join(outdir, f"fuzz_{seed}.blsp")
        with open(path, "w") as fh:
            fh.write(src)
        results = {name: run_one(path, env) for name, env in CONFIGS}
        vals = set(results.values())
        if len(vals) != 1:
            bad += 1
            print(f"DIVERGENCE seed={seed} ({path} kept):")
            for name, res in results.items():
                print(f"  [{name}] {res.strip()[:200]}")
        else:
            if not args.keep:
                os.remove(path)
            sys.stdout.write(f"seed {seed} ok ({next(iter(vals)).splitlines()[-1][:60]})\n")
    if bad:
        print(f"---- fuzz: {bad}/{args.seeds} seeds DIVERGED")
        return 1
    print(f"---- fuzz: {args.seeds} seeds, all configs agree")
    if not args.keep and os.path.isdir(outdir) and not os.listdir(outdir):
        shutil.rmtree(outdir)
    return 0

if __name__ == "__main__":
    sys.exit(main())
