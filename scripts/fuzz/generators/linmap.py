#!/usr/bin/env python3
"""Linear-map rewrite fuzzer (LINMAP, `eval/compile/inline.rs` + `eval/macros.rs`).

Each program folds a random tally over a random key stream TWICE: once as a self-tail
`defn` — the shape the rewrite turns into an in-place `Table` loop — and once as a
`fold` over a closure, which the rewrite never touches. Both are the same source
arithmetic, so the oracle is in the program: a difference prints `BAD`. The engines
must also agree on the printed result (value or error sentinel), and none may crash.

What varies: the spelling (`%map-int-add`, the idiomatic `(assoc m k (+ (get m k 0) e))`
in either operand order, and the near-misses that must NOT fuse — a non-zero default, a
2-arity `get`, a different read key), the key expression (a local, `(first xs)`, a
keyword, an arithmetic expression), the addend (int, float, bignum-sized, a read of the
map), the seed map (empty; ints; a float or a string under a key that is hit — the float
must add as `+` does and the string must be `+`'s error), and a base case that returns
the map, a read of it, or its count.
"""
import random, sys

KEYS = [
    ("k", "(math/rem i 5)"),           # a let-bound local
    ("(first xs)", None),              # an inlined prelude prim over a local
    (":hot", None),                    # a constant key
    ("(math/rem i 3)", None),          # an arithmetic key expression, twice
]
ADDENDS = ["1", "2", "-1", "0.5", "9223372036854775807", "(get m :other 0)", "(+ 1 (math/rem i 2))"]
SEEDS = ["{}", "{:hot 10 :other 3}", "{0 1.5 :hot 2.5}", "{1 \"s\"}", "{:hot 9223372036854775807}", "{2 100000000000000000000}", "{:hot -9223372036854775808}",
         # seeds a table cannot stand in for: the wrapper must run the loop as written
         "{:r (text/from-string \"x\") :hot 1}", "{:f string/length}", "{:v (seq/lmap (list 0 1 2) inc) :hot 2}",
         "(lm-rec 5)"]
BASES = ["m", "(get m :hot :none)", "(count m)", "(get m 0)"]

def update(rng, key):
    """The map-update form for one step, on accumulator `m` with key form `key`."""
    e = rng.choice(ADDENDS)
    r = rng.random()
    if r < 0.15:
        return f"(%map-int-add m {key} {e if e not in ('0.5',) else '1'})" if e != "(get m :other 0)" else f"(%map-int-add m {key} 1)"
    if r < 0.45:
        return f"(assoc m {key} (+ (get m {key} 0) {e}))"
    if r < 0.75:
        return f"(assoc m {key} (+ {e} (get m {key} 0)))"
    if r < 0.80:
        return rng.choice([
            f"(assoc m {key} (inc (get m {key} 0)))",
            f"(assoc m {key} (dec (get m {key} 0)))",
            f"(assoc m {key} (- (get m {key} 0) {e}))",
            f"(assoc m {key} (- {e} (get m {key} 0)))",       # e minus the count: no fuse
        ])
    if r < 0.85:
        return f"(assoc m {key} (+ (get m {key} 1) {e}))"      # non-zero default: no fuse
    if r < 0.93:
        return f"(assoc m {key} (+ (get m {key}) {e}))"        # 2-arity get: raises on a miss
    return f"(assoc m {key} (+ (get m :other 0) {e}))"         # different read key: no fuse

def program(seed):
    rng = random.Random(seed)
    key, let_init = rng.choice(KEYS)
    upd = update(rng, key)
    base = rng.choice(BASES)
    seedmap = rng.choice(SEEDS)
    n = rng.choice([7, 40, 300])
    binding = f"(let (k {let_init}) " if let_init else ""
    close = ")" if let_init else ""
    step = f"{binding}{upd}{close}"
    # xs is a list of ints so `(first xs)` is a key; `i` counts down.
    return f"""(defrecord lm-rec (hits))
(impl Lookup lm-rec (lookup-get [c k] (if (= k :hits) (get c :hits) 100)) (lookup-keys [c] (list :hits)))
(defn go (i xs m)
  (if (= i 0)
    {base}
    (go (- i 1) (rest xs) {step})))
(defn ref-step (m i xs) {step})
(defn ref-go (i xs m)
  (if (= i 0)
    {base}
    (ref-go (- i 1) (rest xs) (ref-step m i xs))))
(def xs (map (range 0 {n}) (fn (i) (math/rem (* i 7) 4))))
(defn show (m) (if (map? m) (pr-str (sort (map (seq m) (fn (kv) (str (nth kv 0) "=" (let (v (nth kv 1)) (cond (rope? v) (text/->string v) (seqview? v) (pr-str (seq v)) (fn? v) "fn" else (pr-str v)))))))) (pr-str m)))
(def a (try (show (go {n} xs {seedmap})) (catch e (str "E:" (error-message e)))))
(def b (try (show (ref-go {n} xs {seedmap})) (catch e (str "E:" (error-message e)))))
(io/puts (if (= a b) a (str "BAD split=" a " ref=" b)))
"""

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/lm_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} linmap programs")
