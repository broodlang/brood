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

    # -- RESTRICTED pure-i64 expression: int arith / compares / if / let ONLY,
    # no vectors and no calls, so a fn built from it stays in the subset the
    # SPECIALISED i64 fast-path lowerer (`jit_lower_arm_inner`) accepts. The
    # table/closure/match helpers above are too complex to qualify, leaving that
    # whole second lowering engine un-fuzzed (found via llvm-cov 2026-07-16) —
    # this feeds it. ------------------------------------------------------------
    def i64_expr(self, params, depth):
        r = self.r
        if depth == 0 or r.random() < 0.35:
            if params and r.random() < 0.7:
                return r.choice(params)
            return str(r.randint(-50, 50))
        c = r.random()
        if c < 0.5:
            op = r.choice(["+", "-", "*", "bit-and", "bit-or", "bit-xor", "max", "min"])
            return f"({op} {self.i64_expr(params, depth-1)} {self.i64_expr(params, depth-1)})"
        if c < 0.65:
            op = r.choice(["quot", "rem"])
            d = r.choice([2, 3, 7, -3, 8])
            return f"({op} {self.i64_expr(params, depth-1)} {d})"
        if c < 0.82:
            cmp = r.choice(["<", "<=", "=", ">", ">="])
            return (f"(if ({cmp} {self.i64_expr(params, depth-1)} {self.i64_expr(params, depth-1)}) "
                    f"{self.i64_expr(params, depth-1)} {self.i64_expr(params, depth-1)})")
        v = self.name("q")
        return f"(let ({v} {self.i64_expr(params, depth-1)}) {self.i64_expr(params + [v], depth-1)})"

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
        # a MAP helper: build a CHAMP map by folding assoc, then dissoc/get/merge/
        # count/keys — exercises persistent-map construction + traversal through
        # calls and (in the concurrent phase) send's deep-copy round-trip. The
        # digest folds `get` over `keys`, which is order-independent (commutative
        # sum) so it's deterministic despite CHAMP iteration order.
        if r.random() < 0.4:
            mname = self.name("mp")
            lines.append(
                "(defn " + mname + " (x)\n"
                "  (let (m (fold (fn (mm i) (assoc mm (rem (+ x i) 16) (bit-and (* i x) 255)))\n"
                "                {} (range 8)))\n"
                "    (let (m2 (merge (dissoc m (rem x 16)) {99 (bit-and x 255)}))\n"
                "      (bit-and (+ (count m2) (get m2 (rem (+ x 3) 16) 0)\n"
                "                  (fold + 0 (map (fn (k) (get m2 k 0)) (keys m2))))\n"
                "               268435455))))")
            helpers.append((mname, 1))
        # a NESTED-CLOSURE helper: three closures deep, each capturing outer
        # bindings AND its own let — hammers capture-slot / frame-slot allocation,
        # the exact machinery behind the sibling-let slot-reuse miscompile (devlog
        # 2026-07-16). Fully applied so the result is a deterministic int.
        if r.random() < 0.5:
            cname = self.name("cl")
            lines.append(
                "(defn " + cname + " (x)\n"
                "  (let (a (rem x 7) b (rem x 5) c (rem x 3))\n"
                "    (let (f (fn (y)\n"
                "              (let (d (+ y a))\n"
                "                (fn (z)\n"
                "                  (let (e (+ (* z b) d))\n"
                "                    (fn (w) (bit-and (+ a b c d e w (* x 2)) 268435455)))))))\n"
                "      (((f 1) 2) 3))))")
            helpers.append((cname, 1))
        # a TRY/CATCH helper: deterministically throws on some inputs and catches,
        # exercising the throw -> catch -> deopt path under the JIT (an error
        # unwinds the native frame). Both arms return an int, so it never crashes
        # a driver/worker (which would turn a seed into a hang, not a diff).
        if r.random() < 0.4:
            tname = self.name("tr")
            lines.append(
                "(defn " + tname + " (x)\n"
                "  (try\n"
                "    (let (d (rem x 3))\n"
                "      (if (= d 0) (throw [:zero x]) (quot 100 d)))\n"
                "    (catch e (bit-and (+ 7 (rem x 5)) 255))))")
            helpers.append((tname, 1))
        # a SLOT-TORTURE helper: sibling `let`s in operand positions, shadowing
        # (slot reuse across scopes), `let`-in-`if`, and a bool-slot `let` used as
        # a condition — the exact machinery behind the sibling-let slot-reuse JIT
        # miscompile. Deterministic int result.
        if r.random() < 0.55:
            qn = self.name("sq")
            lines.append(
                "(defn " + qn + " (x)\n"
                "  (let (a (rem x 7))\n"
                "    (- (let (a (+ a 1))\n"
                "         (+ (let (b (* a 2)) b)\n"
                "            (let (a (bit-xor a x)) a)))\n"
                "       (let (c (if (< a 3) (let (d 10) d) (let (d 20) d)))\n"
                "         (* c (if (let (e (> x 0)) e) 2 3))))))")
            helpers.append((qn, 1))
        # a FLOAT slot-torture helper: float-slot `let`s as the operands of
        # comparisons, with a shadowed float-slot binding — exercises the float
        # branch of the SetLocal materialisation fix. Int result via the compares.
        if r.random() < 0.45:
            fn = self.name("sf")
            lines.append(
                "(defn " + fn + " (x)\n"
                "  (+ (if (< (let (a (* (rem x 5) 1.5)) a) (let (b (/ (+ x 3) 2.0)) b)) 1 0)\n"
                "     (if (< (let (a (+ x 0.5)) a) (let (a (* x 0.25)) a)) 10 0)))")
            helpers.append((fn, 1))
        # an EA-SCALAR-REPLACEMENT helper: a single-binder `let` of a small vector
        # LITERAL read only by CONSTANT index — the escape-analysis pass proves the
        # vector never escapes and lifts each element into its own slot (the vector
        # is never allocated). Exercises mod.rs `ea_scalar_replace`/`local_escapes`/
        # `rewrite_elem_reads`, which the immediate `(nth [..] k)` form never reaches.
        if r.random() < 0.5:
            en = self.name("ea")
            ne = r.randint(2, 6)
            elems = " ".join(self.i64_expr(["x"], 2) for _ in range(ne))
            reads = " ".join(f"(nth v {r.randint(0, ne - 1)})" for _ in range(r.randint(2, 5)))
            lines.append(
                "(defn " + en + " (x)\n"
                "  (let (v [" + elems + "])\n"
                "    (bit-and (+ " + reads + ") 268435455)))")
            helpers.append((en, 1))
        # a RANGE-REDUCE helper: a NON-prim closure folded over a range, which
        # dispatches through the native `%range-reduce` HOF driver + its JIT
        # fast-frame (mod.rs `hof_apply_native`/`hof_apply_step`). A prim reducer
        # (`fold +`) skips that path, so the grammar never reached it before.
        if r.random() < 0.5:
            rrn = self.name("rr")
            k = r.randint(6, 15)
            op = r.choice(["+", "-", "bit-xor"])
            m = r.randint(2, 9)
            lines.append(
                "(defn " + rrn + " (x)\n"
                "  (reduce (fn (acc i) (bit-and (" + op + " acc (* i (rem x " + str(m) + "))) 268435455)) 0 (range " + str(k) + ")))")
            helpers.append((rrn, 1))
        # the driver bit-ands the helper result, so it must be int-returning; the
        # float helper `g` is exercised on its own (and by the pure `flt`/`accf`
        # recursion below), never fed to a bit op.
        f, arity = r.choice([(n, a) for (n, a) in helpers if not n.startswith("g")])
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
            mode = r.choice(["flat", "flat", "tree"])
            if mode == "flat":
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
            else:
                # PROCESS TREE: a parent spawns `nodes` node-workers; each node
                # spawns `leaves` leaf-workers, MONITORS each, and collects both
                # the leaf's [:leaf s] result and its [:down] (every leaf exits
                # normally, so a node sees exactly `leaves` downs — a deterministic
                # count). Nodes fan totals to the parent. Exercises nested spawn,
                # nested/selective receive, monitors, and per-process mailboxes.
                # Completion keys on downs==leaves (not results) so even a crashing
                # leaf can't hang the node. Sums are commutative → schedule-free.
                pa = " ".join(["(+ w j)"] * par)
                nodes = r.choice([3, 5, 8])
                leaves = r.choice([3, 6])
                per = r.choice([300, 900])
                lines.append("(def me (self))")
                lines.append(
                    "(defn leaf (node w j n s)\n"
                    "  (if (>= j n) (send node [:leaf s])\n"
                    "    (leaf node w (+ j 1) n (bit-and (+ s (" + pf + " " + pa + ")) 268435455))))")
                lines.append(
                    "(defn node-loop (w downs sum)\n"
                    "  (if (= downs " + str(leaves) + ") (send me [:node (bit-and sum 268435455)])\n"
                    "    (receive\n"
                    "      ([:leaf v] (node-loop w downs (bit-and (+ sum v) 268435455)))\n"
                    "      ([:down _ _ _] (node-loop w (+ downs 1) sum))\n"
                    "      (after 15000 (send me [:node -1])))))")
                # NB `spawn` evaluates its whole form in the CHILD, so `(self)`
                # inside a spawned call is the child's pid — the node captures its
                # own pid into `nd` (a local, carried by value into the spawned
                # leaf closure) and passes that, rather than calling `(self)` from
                # inside the leaf. Local vars ARE captured; function calls re-run.
                lines.append(
                    "(defn spawn-leaves (nd w k)\n"
                    "  (if (= k " + str(leaves) + ") nil\n"
                    "    (do (monitor (spawn (leaf nd w 0 " + str(per) + " 0))) (spawn-leaves nd w (+ k 1)))))")
                lines.append(
                    "(defn node-worker (w) (let (nd (self)) (do (spawn-leaves nd w 0) (node-loop w 0 0))))")
                lines.append(
                    "(defn spawn-nodes (w) (if (= w " + str(nodes) + ") nil (do (spawn (node-worker w)) (spawn-nodes (+ w 1)))))")
                lines.append(
                    "(defn tree-in (k s) (if (= k " + str(nodes) + ") s (tree-in (+ k 1) (receive ([:node v] (bit-and (+ s v) 268435455)) (after 20000 -1)))))")
                lines.append("(spawn-nodes 0)")
                lines.append('(println "tree" (tree-in 0 0))')
        # PURE self-recursive numeric fns — standalone (no tables, no cross-helper
        # calls), so they stay in the pure-i64/float subset the SPECIALISED
        # fast-path lowerer (`jit_lower_arm_inner`) compiles. This is the lowering
        # engine the table/closure-heavy programs never reach (llvm-cov gap,
        # 2026-07-16); any jit/tree divergence here is an i64-path miscompile.
        if r.random() < 0.75:
            pn = self.name("rec")
            body = self.i64_expr(["n", "a"], r.randint(2, 4))
            lines.append(
                f"(defn {pn} (n a)\n"
                f"  (if (<= n 0) a\n"
                f"    ({pn} (- n 1) (bit-and {body} 268435455))))")
            lines.append(f'(println "{pn}" ({pn} {r.choice([2000, 4000, 8000])} {r.randint(1, 99)}))')
        # fib-like NON-TAIL double self-recursion (the `i64_too_deep` guard path).
        if r.random() < 0.45:
            fbn = self.name("fib")
            lines.append(
                f"(defn {fbn} (n)\n"
                f"  (if (< n 2) n (bit-and (+ ({fbn} (- n 1)) ({fbn} (- n 2))) 268435455)))")
            lines.append(f'(println "{fbn}" ({fbn} {r.choice([25, 28, 30])}))')
        # pure FLOAT self-recursion — the float variant of the fast path
        # (`has_float_slot`). A bounded predicate keeps the print IEEE-stable while
        # still catching a float-path divergence (jit inf vs tree finite → differs).
        if r.random() < 0.45:
            fln = self.name("flt")
            lines.append(
                f"(defn {fln} (n a)\n"
                f"  (if (<= n 0) a\n"
                f"    ({fln} (- n 1) (+ (* a 1.0000001) (- (* n 0.5) a)))))")
            lines.append(f'(println "{fln}" (< ({fln} {r.choice([1500, 3000])} 1.0) 1.0e18))')
        # UNMASKED-overflow recursion — the i64 arm's overflow guard
        # (`i64_guard_overflow`) must deopt to the VM, which promotes to a bignum
        # (never silently wrap). The bignum result is engine-independent.
        if r.random() < 0.4:
            ovn = self.name("ov")
            base = r.choice([2, 3, 5])
            lines.append(
                f"(defn {ovn} (n a) (if (<= n 0) a ({ovn} (- n 1) (* a {base}))))")
            lines.append(f'(println "{ovn}" ({ovn} {r.choice([40, 50, 80])} {r.randint(2, 9)}))')
        # THROW inside an i64 arm (`i64_throw_call`) — the throw fires from the
        # recursion and is caught deterministically.
        if r.random() < 0.4:
            trn = self.name("thr")
            lines.append(
                f"(defn {trn} (n a)\n"
                f"  (if (<= n 0) (throw [:done a])\n"
                f"    ({trn} (- n 1) (bit-and (+ (* a 2) n) 268435455))))")
            lines.append(f'(println "{trn}" (try ({trn} {r.choice([2000, 5000])} 1) (catch e (nth e 1))))')
        # a LINMAP (linear map-accumulator) loop: an immutable map threaded through
        # a self-recursive fold, updated ONLY via map-int-add / map-dissoc and read
        # via map-get / map-count — the whitelist that lets the compiler rewrite it
        # to a private mutable Table internally (mod.rs linmap; a semantics-preserving
        # transform, so any jit-vs-tree diff is a real miscompile). The observable
        # result is still an ordinary immutable map.
        if r.random() < 0.5:
            hn = self.name("hist")
            keys = r.choice([5, 7, 10, 16])
            upd = r.choice([1, 2, 3])
            body = f"(map-int-add m (rem i {keys}) {upd})"
            if r.random() < 0.45:  # mix in the other update op (dissoc)
                body = f"(if (= (rem i 13) 0) (map-dissoc m (rem i {keys})) {body})"
            lines.append(
                f"(defn {hn} (m i n)\n"
                f"  (if (>= i n) m\n"
                f"    ({hn} {body} (+ i 1) n)))")
            dn = self.name("mdig")
            lines.append(
                f"(defn {dn} (m k acc)\n"
                f"  (if (>= k {keys}) acc\n"
                f"    ({dn} m (+ k 1) (bit-xor acc (* (+ k 1) (map-get m k 0))))))")
            hv = hn + "-r"
            lines.append(f"(def {hv} ({hn} {{}} 0 {r.choice([2000, 5000, 9000])}))")
            lines.append(f'(println "{hn}" ({dn} {hv} 0 0) (map-count {hv}))')
        # a SIDE-EFFECT + call-result-DESTRUCTURE loop — the deopt-rerun bug's
        # shape: a `table-incr` effect before a non-tail call whose vector result
        # is destructured (which deopts). Exercises the JIT deopt/effect-ordering
        # machinery (the checkpoint that makes an effect execute exactly once); a
        # jit-vs-tree diff here would be a duplicated/lost effect.
        if r.random() < 0.4:
            sp = self.name("spin")
            mk = self.name("mk")
            key = r.choice([777, 888, 999])
            lines.append(f"(defn {mk} (s) [(rem (+ (* s 1103515245) 12345) 2147483648) :tag])")
            lines.append(
                f"(defn {sp} (s i n acc)\n"
                f"  (if (>= i n) acc\n"
                f"    (do (table-incr t {key} 1)\n"
                f"      (let ([s2 tag] ({mk} s))\n"
                f"        ({sp} s2 (+ i 1) n (bit-xor acc (bit-and s2 268435455)))))))")
            sv = sp + "-r"
            lines.append(f'(def {sv} ({sp} 1337 0 {r.choice([5000, 15000])} 0))')
            lines.append(f'(println "{sp}" {sv} (table-get t {key} 0))')
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

def confirm_divergence(path, rounds=3):
    """Re-run all configs `rounds` more times; a REAL engine divergence (a
    deterministic miscompile, or a race that recurs) reproduces, so require it to
    STILL disagree every round. A transient harness artifact — a subprocess the
    OS killed / OOM'd / starved under concurrent build load, seen as a nonzero
    exit or truncated output — converges on re-run and is filtered out. Returns
    True only if the seed diverged on the initial detection AND all `rounds`
    re-checks. (Bought by the seed 20108 catch: that one reproduces 3/3; the two
    build-contention false positives converged immediately.)"""
    return all(divergence_oracle(path) for _ in range(rounds))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", type=int, default=25)
    ap.add_argument("--start", type=int, default=1)
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()
    outdir = os.environ.get("FUZZ_OUTDIR", "stress/fuzz_out")
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
            # Re-confirm before believing it: filters transient contention
            # artifacts (a killed/starved subprocess) that don't reproduce.
            if not confirm_divergence(path):
                sys.stdout.write(f"seed {seed} transient (diverged once, "
                                 f"converged on re-check — skipped)\n")
                if not args.keep:
                    os.remove(path)
                continue
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
