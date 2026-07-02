#!/usr/bin/env python3
"""Differential fuzzer for the unboxed register JIT workers (i64 / f64).

Generates chaotic-but-terminating {int,float}-only recursive Brood programs and
asserts the JIT path (default) produces byte-identical output to the VM
(BROOD_NO_JIT=1). A segfault / panic (negative return code) or a JIT-vs-VM output
mismatch is a bug. Termination is guaranteed: every self-call strictly decreases
the first arg toward a base case.

    scripts/fuzz_unbox.py [N] [--mode int|float] [--brood PATH] [--timeout SECS]

Exits nonzero if any bug is found. Companion to tests/unbox_torture_test.blsp
(the fixed-case CI guard); this is the manual breadth pass for JIT-path changes.
"""
import argparse, os, random, subprocess, sys

CMP_OPS = ["<", "<=", "=", ">", ">="]
INT = dict(
    ops=["+", "-", "*", "bit-and", "bit-or", "bit-xor", "min", "max", "rem", "quot"],
    consts=[0, 1, 2, 3, 7, -1, -2, 10, 100, 1000, 4611686018427387904,
            9223372036854775807, -9223372036854775808, 1000000007, 65535],
    fmt=lambda c: str(c), dec=lambda d: str(d), suffix="",
)
FLOAT = dict(
    ops=["+", "-", "*", "/"],
    consts=[0.0, 1.0, 2.0, 3.0, 0.5, -1.0, 1.5, 10.0, 100.0, 1e100, -0.0, 1e-9, 3.14159],
    fmt=lambda c: repr(float(c)), dec=lambda d: f"{float(d)}", suffix=".0",
)


def gen(seed, K):
    random.seed(seed)
    rc = lambda: random.choice(K["consts"])
    leaf = lambda ps: random.choice(ps) if ps and random.random() < 0.6 else K["fmt"](rc())

    def arith(ps, d):
        if d <= 0 or random.random() < 0.4:
            return leaf(ps)
        return f"({random.choice(K['ops'])} {arith(ps, d-1)} {arith(ps, d-1)})"

    def selfcall(ps, ar):
        args = [f"(- {ps[0]} {K['dec'](random.randint(1,3))})"]
        args += [arith(ps, 1) for _ in range(1, ar)]
        return f"(f {' '.join(args)})"

    arity = random.choice([1, 1, 1, 2, 2, 3])
    ps = ["a", "b", "c"][:arity]
    shape = random.choice(["linear", "linear", "binary", "let"])
    if shape == "linear":
        body = f"({random.choice(K['ops'])} {selfcall(ps,arity)} {arith(ps,1)})"
    elif shape == "binary":
        body = f"({random.choice(K['ops'])} {selfcall(ps,arity)} {selfcall(ps,arity)})"
    else:
        n = random.randint(1, 3)
        binds = " ".join(f"v{i} {arith(ps,2)}" for i in range(n))
        vs = [f"v{i}" for i in range(n)]
        body = f"(let ({binds}) ({random.choice(K['ops'])} {selfcall(ps+vs,arity)} {random.choice(ps+vs)}))"
    thr = K["fmt"](random.randint(0, 3))
    fn = f"(defn f ({' '.join(ps)}) (if (< a {thr}) {arith(ps,2)} {body}))"
    a0 = random.randint(0, 22) if shape == "binary" else random.choice(
        [random.randint(0, 50), random.randint(0, 1500), random.randint(1300, 1600), random.randint(0, 5000)])
    rest = " ".join(K["fmt"](random.choice([random.randint(-5, 5), rc()])) for _ in range(arity - 1))
    return f"{fn}\n(println (f {a0}{K['suffix']} {rest}))\n".replace("  ", " ")


def run(brood, src, no_jit, timeout):
    env = dict(os.environ, **({"BROOD_NO_JIT": "1"} if no_jit else {}))
    try:
        p = subprocess.run([brood, "/dev/stdin"], input=src, env=env,
                           capture_output=True, text=True, timeout=timeout)
        out = p.stdout.strip().splitlines()
        return (out[-1] if out else "", "error" in p.stderr.lower(), p.returncode)
    except subprocess.TimeoutExpired:
        return ("<TIMEOUT>", False, -99)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("n", nargs="?", type=int, default=500)
    ap.add_argument("--mode", choices=["int", "float"], default="int")
    ap.add_argument("--brood", default=os.path.join(
        os.path.dirname(__file__), "..", "target", "release", "brood"))
    ap.add_argument("--timeout", type=float, default=10.0)
    a = ap.parse_args()
    K = INT if a.mode == "int" else FLOAT
    bugs = 0
    for i in range(a.n):
        src = gen(i, K)
        jit = run(a.brood, src, False, a.timeout)
        vm = run(a.brood, src, True, a.timeout)
        crash = (jit[2] < 0 and jit[2] != -99) or (vm[2] < 0 and vm[2] != -99)
        mismatch = jit[0] != vm[0] or jit[1] != vm[1]
        if crash or mismatch:
            bugs += 1
            print(f"\n=== BUG seed={i} crash={crash} mismatch={mismatch} ===\n  jit={jit} vm={vm}")
            print("  " + src.replace("\n", "\n  "))
            if bugs >= 8:
                break
        if i % 100 == 0:
            print(f"[{i}/{a.n}] bugs: {bugs}", flush=True)
    print(f"\nDONE ({a.mode}): {a.n} programs, {bugs} bugs")
    sys.exit(1 if bugs else 0)


if __name__ == "__main__":
    main()
