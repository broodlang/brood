#!/usr/bin/env python3
"""Pipeline fusion + counted range loop fuzzer (ADR-360 §7, `eval/macros.rs`).

Each program runs a random `seq/l*` stage chain under a `fold`/`reduce` TWICE: once as the
literal pipeline the compiler fuses — stages substituted or called by name, a range base
looped — and once with the view built first into a `def` and folded from there, which the
rewrite cannot see (it fuses only a chain written under the fold). The oracle is in the
program: a difference prints `BAD`. Every engine must agree on the printed result (a value
or an error sentinel), and none may crash.

What varies: the base (a range with random bounds and step — including empty and negative
— a list, a vector, nil), one to four stages of every kind with literal or named functions
(a literal that reads an OUTER variable named like an earlier stage's parameter, to catch
a capture; one that rebinds its own parameter, which must be called rather than
substituted), the reducer (`+`, a literal, a named function), an init that is a number or
a list, and elements that make a stage raise mid-way.
"""
import random, sys

BASES = ["(range 7)", "(range 0)", "(range 2 9)", "(range 9 2 -3)", "(range 0 20 5)", "(range -3 3)",
         "(list 1 2 3 4 5)", "[3 1 4 1 5]", "nil", "(range 40)", "[]", "(list 3)", "{:k 1}"]
# stage function forms; `x` in a body may be shadowed by the stage's own param
STAGE_FNS = {
    "seq/lmap": ["(fn (v) (* v v))", "(fn (v) (+ v x))", "(fn (x) (+ x 1))", "(fn (x) (let (x (* x 2)) x))",
                 "inc", "pf-sq", "(fn (v) (if (= v 3) (str v) v))"],
    "seq/lfilter": ["(fn (v) (> v 1))", "(fn (x) (< x x2))", "pf-even?", "(fn (v) (not= v 4))"],
    "seq/lreject": ["(fn (v) (= v 2))", "pf-even?", "(fn (x) (> x x))"],
    "seq/lkeep": ["(fn (v) (when (> v 2) (* v 10)))", "(fn (x) (if (= 0 (math/rem x 2)) x nil))"],
}
RFS = ["+", "(fn (a v) (+ a v))", "(fn (acc v) (cons v acc))", "pf-add", "(fn (a v) (+ a (* 2 v)))", "conj"]
INITS = ["0", "nil", "[]", "100"]

def program(seed):
    rng = random.Random(seed)
    base = rng.choice(BASES)
    nstages = rng.randint(1, 4)
    chain = base
    for _ in range(nstages):
        kind = rng.choice(list(STAGE_FNS))
        f = rng.choice(STAGE_FNS[kind])
        chain = f"({kind} {chain} {f})"
    rf = rng.choice(RFS)
    init = rng.choice(INITS)
    head = rng.choice(["fold", "reduce"])
    plain = ""
    if rng.random() < 0.4:
        # A plain fold with a literal over the base — the collection dispatch alone —
        # against the same body as a NAMED function, which the rewrite leaves to `fold`.
        body = rng.choice(["(+ a v)", "(cons v a)", "(if (= v 3) (failure \"stop\") (if (int? a) (+ a v) a))", "(+ a (* v x))"])
        plain = f"""(defn pf-step (a v) {body})
(def c (try (show (fold {base} {init} (fn (a v) {body}))) (catch e (str "E:" (error-message e)))))
(def d (try (show (fold {base} {init} pf-step)) (catch e (str "E:" (error-message e)))))
(io/puts (if (= c d) "fold-ok" (str "BAD fold=" c " ref=" d)))"""
    return f"""(def x 7)
(def x2 4)
(defn pf-sq (v) (* v v))
(defn pf-even? (v) (= 0 (math/rem v 2)))
(defn pf-add (a v) (+ a v))
(defn show (r) (pr-str (if (seqview? r) (seq r) r)))
(def a (try (show ({head} {chain} {init} {rf})) (catch e (str "E:" (error-message e)))))
(def view {chain})
(def b (try (show ({head} view {init} {rf})) (catch e (str "E:" (error-message e)))))
(io/puts (if (= a b) a (str "BAD fused=" a " ref=" b)))
{plain}
"""

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/pl_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} pipeline programs")
