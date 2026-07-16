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
    ("tree-walk", {"BROOD_VM": "0"}),  # honored at top level again since 2026-07-16
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
        # a closure-through-HOF helper: maps a generated closure over a small
        # range and folds — exercises hof_apply_native / capture slots / the
        # closure-arm gate under every tier
        if r.random() < 0.4:
            h = self.name("h")
            k = r.randint(1, 9)
            op = r.choice(["+", "*", "bit-xor"])
            lines.append("(defn " + h + " (x) (fold + 0 (map (fn (y) (" + op
                         + " y (rem x " + str(k + 1) + "))) (range " + str(k) + "))))")
            helpers.append((h, 1))
        # a match-dispatch helper: builds a shape from its arg and matches it —
        # exercises the fail-thunk match lowering + destructures under every tier
        if r.random() < 0.4:
            m = self.name("m")
            lines.append(
                "(defn " + m + " (x) "
                "(match (if (= (rem x 3) 0) [:a x] (if (= (rem x 3) 1) [:b x (+ x 1)] (list :c x))) "
                "([:a v] :when (> v 100) (+ v 1)) "
                "([:a v] (- v 1)) "
                "([:b v w] (+ v w)) "
                "((:c v) (* v 2)) "
                "(_ 0)))")
            helpers.append((m, 1))
        # a string helper: digest of the decimal rendering — exercises str /
        # string-length across tiers
        if r.random() < 0.3:
            sname = self.name("s")
            lines.append(
                "(defn " + sname + " (x) "
                '(let (t (str x "-" (* x 3))) '
                "(+ (string-length t) (bit-and x 7))))")
            helpers.append((sname, 1))
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
        # optional CONCURRENT phase: fan out workers running a pure helper +
        # commutative shared-table ops (incr / disjoint puts), fan in results.
        # The final digest is deterministic even though scheduling is not —
        # any divergence is a real concurrency bug (lost update, torn value,
        # broken fan-in), not schedule noise.
        if r.random() < 0.6:
            # int-returning helpers only: the worker masks with bit-and, which
            # errors on the float helper's result — a crashed worker never sends
            # :done and the fan-in's timeout turns the whole seed into a hang.
            int_helpers = [(n, a) for (n, a) in helpers if not n.startswith("g")]
            pf, par = r.choice(int_helpers)
            pargs = " ".join(["j"] * par)
            per = r.choice([500, 1500, 3000])
            nworkers = r.choice([4, 8, 16])
            span = r.choice([64, 512])
            lines.append("(def me (self))")
            lines.append(
                "(defn worker (w j n s)\n"
                "  (if (>= j n) (send me [:done s])\n"
                "    (do (table-incr t 999983 1)\n"
                "      (table-put t (+ 100000 (* w " + str(span) + ") (rem j " + str(span) + ")) (bit-and (" + pf + " " + pargs + ") 268435455))\n"
                "      (worker w (+ j 1) n (bit-and (+ s (" + pf + " " + pargs + ")) 268435455)))))")
            lines.append(
                "(defn fan (w) (if (= w " + str(nworkers) + ") nil (do (spawn (worker w 0 " + str(per) + " 0)) (fan (+ w 1)))))")
            lines.append(
                "(defn fan-in (k s) (if (= k " + str(nworkers) + ") s (fan-in (+ k 1) (receive ([:done v] (bit-and (+ s v) 268435455)) (after 10000 -1)))))")
            lines.append(
                "(defn dig2 (k n s)\n"
                "  (if (>= k n) s\n"
                "    (dig2 (+ k 1) n (bit-xor s (* (- k 99999) (table-get t k 0))))))")
            lines.append("(fan 0)")
            lines.append("(def conc (fan-in 0 0))")
            lines.append(
                f'(println "conc" conc (table-get t 999983 0) '
                f"(dig2 100000 {100000 + nworkers * span} 0))")
        # digest: accumulator + table contents
        lines.append(
            "(defn dig (k n s)\n"
            "  (if (>= k n) s\n"
            "    (dig (+ k 1) n (bit-xor s (* (+ k 1) (table-get t k 0))))))")
        lines.append(f'(println "digest" acc (dig 0 {key_mod} 0) (table-count t))')
        return "\n".join(lines) + "\n"

def check_soundness(path):
    """The advisory checker must be SOUND: zero warnings on a program that runs
    cleanly (missed errors are fine — completeness is not required; false
    positives are bugs). Returns None if sound, else the offending output."""
    env = dict(os.environ)
    try:
        out = subprocess.run([BROOD, "--check", path], capture_output=True,
                             text=True, timeout=120, env=env)
    except subprocess.TimeoutExpired:
        return "CHECKER TIMEOUT"
    text = (out.stdout + out.stderr).strip()
    # Style lints are legitimate on generated code (the generator makes unused
    # binders and non-tail recursion on purpose); SOUNDNESS is about TYPE
    # warnings — any remaining warning on a cleanly-running program is a
    # checker false positive.
    style = ("unused let binding", "non-tail position", "unused parameter")
    real = [ln for ln in text.splitlines()
            if "warning" in ln and not any(t in ln for t in style)]
    if real:
        return "\n".join(real)
    return None

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

# ---- auto-shrink (delta debugging over s-expressions) ------------------------

def tokenize(src):
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c in "()[]{}":
            out.append(c); i += 1
        elif c == '"':
            j = i + 1
            while j < n and (src[j] != '"' or src[j-1] == "\\"):
                j += 1
            out.append(src[i:j+1]); i = j + 1
        elif c == ";":
            while i < n and src[i] != "\n":
                i += 1
        elif c.isspace():
            i += 1
        else:
            j = i
            while j < n and not src[j].isspace() and src[j] not in "()[]{};":
                j += 1
            out.append(src[i:j]); i = j
    return out

CLOSE = {"(": ")", "[": "]", "{": "}"}

def parse(tokens):
    """token list -> nested lists; each node is (kind, children|atom)."""
    def one(i):
        t = tokens[i]
        if t in CLOSE:
            kids, i = [], i + 1
            while tokens[i] != CLOSE[t]:
                node, i = one(i)
                kids.append(node)
            return (t, kids), i + 1
        return ("a", t), i + 1
    forms, i = [], 0
    while i < len(tokens):
        node, i = one(i)
        forms.append(node)
    return forms

def render(node):
    kind, v = node
    if kind == "a":
        return v
    return kind + " ".join(render(k) for k in v) + CLOSE[kind]

def render_forms(forms):
    return "\n".join(render(f) for f in forms) + "\n"

def all_nodes(forms):
    """(container, index) pairs for every removable/replaceable child, largest first."""
    out = []
    def walk(node):
        kind, v = node
        if kind == "a":
            return 1
        size = 1
        for i, k in enumerate(v):
            size += walk(k)
            out.append((v, i))
        return size
    for f in forms:
        walk(f)
    out.sort(key=lambda ci: -node_size(ci[0][ci[1]]))
    return out

def node_size(node):
    kind, v = node
    return 1 if kind == "a" else 1 + sum(node_size(k) for k in v)

def shrink(path, still_bad, budget=250):
    """Greedy delta debugging: try dropping top-level forms, then replacing
    subtrees with `0`, then halving int literals — keeping every change that
    preserves `still_bad(src)`. Bounded by `budget` oracle runs."""
    src = open(path).read()
    forms = parse(tokenize(src))
    runs = [0]
    def check(candidate_forms):
        if runs[0] >= budget:
            return False
        runs[0] += 1
        cand = render_forms(candidate_forms)
        with open(path + ".shrink", "w") as fh:
            fh.write(cand)
        return still_bad(path + ".shrink")
    changed = True
    while changed and runs[0] < budget:
        changed = False
        # pass 1: drop whole top-level forms
        for i in range(len(forms) - 1, -1, -1):
            cand = forms[:i] + forms[i+1:]
            if cand and check(cand):
                forms = cand
                changed = True
        # pass 2: replace subtrees with the atom 0 (largest first)
        for container, i in all_nodes(forms):
            saved = container[i]
            if saved == ("a", "0"):
                continue
            container[i] = ("a", "0")
            if check(forms):
                changed = True
            else:
                container[i] = saved
        # pass 3: shrink big int literals toward 0
        for container, i in all_nodes(forms):
            kind, v = container[i]
            if kind == "a" and v.lstrip("-").isdigit() and abs(int(v)) > 8:
                saved = container[i]
                container[i] = ("a", str(int(v) // 2))
                if check(forms):
                    changed = True
                else:
                    container[i] = saved
    out = render_forms(forms)
    with open(path + ".min", "w") as fh:
        fh.write(out)
    os.remove(path + ".shrink") if os.path.exists(path + ".shrink") else None
    return path + ".min", runs[0]

def divergence_oracle(path):
    """True iff the configs still disagree on `path` (the shrink predicate)."""
    results = {run_one(path, env) for _, env in CONFIGS}
    return len(results) != 1

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
        # checker soundness: a program every engine runs CLEANLY must produce
        # zero advisory warnings (sound-but-incomplete contract)
        if all(r.startswith("exit=0") for r in results.values()):
            if (w := check_soundness(path)) is not None:
                bad += 1
                print(f"CHECKER FALSE POSITIVE seed={seed} ({path} kept):")
                print("  " + w[:400].replace(chr(10), chr(10) + "  "))
                continue
        vals = set(results.values())
        if len(vals) != 1:
            bad += 1
            print(f"DIVERGENCE seed={seed} ({path} kept):")
            for name, res in results.items():
                print(f"  [{name}] {res.strip()[:200]}")
            minp, oracle_runs = shrink(path, divergence_oracle)
            print(f"  shrunk -> {minp} ({oracle_runs} oracle runs, "
                  f"{len(open(minp).read())} bytes from {len(open(path).read())})")
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
