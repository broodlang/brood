# Findings from writing a "VERY complicated" Brood demo

Written 2026-05-28 by an LLM (Claude Opus 4.7) after attempting a
concurrent ASCII Mandelbrot in Brood, working only from the supplied
docs/brood-for-claude.md and the std/prelude source. The demo is
intentionally ambitious — it exercises math, recursion, immutable maps,
transducers, processes, defprocess/hatch, selective receive, pattern
matching, gensym macros, and timing. Everything worked **in isolation**
in a probe; the full program tripped a runtime bug under load. The
notes below mix everything I hit: language familiarity, stdlib gaps,
runtime correctness, performance, and tooling.

The recommendations at the bottom are ordered by what would unblock the
next person fastest.

---

## 1. Executive summary

**Three things that blocked or seriously slowed me:**

1. **Multi-thread scheduler race** — under `-j 0` (default), spawning
   ~20+ worker processes that each touch prelude functions reliably
   crashes workers with bogus "unbound symbol" errors on internal
   names (`acc`, `pred`, `fold`, `%eq`) and a Rust panic in
   `crates/lisp/src/eval/mod.rs:380`. With `-j 1` everything works.
   Until this is fixed, no fan-out demo can credibly ship.
2. **Type-checker noise around `(require 'hatch)`** — every file using
   `defprocess` / `cast` / `!` / `hatch` prints five "unbound symbol"
   warnings at load. They look like errors. A new user's first
   reaction is "this is broken."
3. **`nest format` collapses readable code onto single lines** — multi-
   line `let` blocks, `cond` branches, `defmacro` bodies, and even
   `receive` clauses get squashed onto 100+ char lines. After running
   the formatter my code was substantially harder to read.

**Three polish issues that added friction throughout:**

4. The quick-ref doc (`docs/brood-for-claude.md`) is missing a number of
   builtins the prelude/examples assume: `apply`, `now`, `gensym`, `for`,
   `defprocess` / `hatch` / `!` / `gen-call` / `sleep`, `char-at`,
   `quot`, `string-length`.
5. Float formatting via `str` prints full f64 precision
   (`0.015873015873015872`). No `format`, no `to-fixed`, no `printf`.
   Demo code ends up with hand-rolled `(quot (* 10 a) b)` tricks.
6. Pattern-destructure failures surface as **Rust panics**
   (`index out of bounds: the len is 0 but the index is 1`) rather
   than Brood-level errors. Catastrophic for debugging.

---

## 2. Language familiarity

### 2.1 The quick-ref is the only doc an LLM/new user will read

`docs/brood-for-claude.md` is good but incomplete. To finish the demo
I had to:

- Read `std/prelude.blsp` end-to-end to find `apply`, `gensym`, `for`,
  `quot`, `char-at`, `string-length`, `pr-str`, `enumerate`,
  `iterate-times`, `partition`, `frequencies`.
- Read `std/hatch.blsp` to learn `defprocess`/`cast`/`call`/`!`/
  `gen-call`/`hatch`/`sleep` — none of these appear in the quick-ref.
- Read `crates/lisp/src/builtins.rs` to discover `now` (no other doc
  mentions it).
- Read `examples/life.blsp` to discover `defprocess` exists at all,
  and that ANSI escape codes are first-class (great feature!).

**What this means in practice:** any non-trivial demo requires a
30-minute spelunk through the source. For an LLM running in one shot,
that's three or four extra tool calls per attempt.

**Fix:** expand the quick-ref to cover, at minimum:

- `apply`, `now`, `gensym`, `for`, `doseq`, `dotimes`, `dolist`,
  `enumerate`, `partition`, `frequencies`, `mapcat`, `zip`.
- `defprocess`, `hatch`, `!`, `gen-call`, `sleep` — these *are* the
  idiomatic concurrency story and they're completely missing from
  the doc that bills itself as "the pocket guide."
- `pr-str` vs `str`, and which one preserves structure (the prelude
  uses both; I had to read it to know).
- A worked "tagged-data + worker pool" example (one of the highest-
  value patterns and the one most likely to trip a writer).

### 2.2 Patterns that surprised me

These are subtle and not obvious from the quick-ref:

- `let` bindings are sequential (each binding sees the earlier ones)
  and **flat** (no Scheme double-parens). Mentioned, but easy to miss
  the implication: you can do `(let (a 1 b (+ a 1)) …)`. Good design;
  worth a worked example.
- `_` is wildcard but `&` is rest — both in patterns AND in `fn`
  parameter lists. The dual meaning is fine but the quick-ref
  treats them under "patterns" only.
- Vector destructure in *params* works: `(defn step ([zr zi] [cr ci])
  …)`. I found this in `examples/life.blsp`, not the doc.
- The `:keyword` literals match literally in patterns (`(:stop nil)`
  matches the bare keyword `:stop` and binds nothing). Confusing
  next to `(stop nil)` which would *bind* `stop`. Worth calling out.
- Multi-clause `fn` works with named functions defined by `defn`. Not
  shown in the quick-ref as plainly as it could be — I copied
  `classify` from the doc and was surprised it worked.

### 2.3 Naming inconsistencies

- `quot` (Brood) vs `mod` (Brood) vs `rem` (kernel primitive). The
  quick-ref names none of them; the prelude defines all three.
- `string-length` not `count` or `length` for strings — easy to forget
  when you've just used `(count xs)` on a list.
- `char-at` returns a 1-char *string* (no char type). Helpful that
  this is documented in the prelude comment, but the quick-ref says
  nothing about how to index a string at all.

### 2.4 Misleading docs

- The quick-ref says "Functions can't be sent (per-heap closures) —
  send data and call `def`'d names on the receiving side." This made
  me worry that closures created *inside* a spawned process wouldn't
  work either. They do. Reword as: "Closures don't cross process
  boundaries via `send`. Inside a process, they work normally."
- The quick-ref says `apply` doesn't exist — actually it's in the
  prelude, used heavily by the variadic primitives, and is the only
  way to call a function with a list of args.

---

## 3. Runtime / concurrency

### 3.1 The scheduler race (severity: blocker for fan-out demos)

**Symptoms.** A worker pool of ≥4 processes that each call into prelude
functions (transduce, map, fold, %eq) reliably crashes one or more
workers under `-j 0` and `-j 2`. Errors observed in a single run:

```
process 6 died: unbound error: unbound symbol: fold
process 5 died: unbound error: unbound symbol: %eq
process 9 died: unbound error: unbound symbol: acc
process 7 died: unbound error: unbound symbol: acc
process 3 died: unbound error: unbound symbol: pred
process 10 died: unbound error: unbound symbol: iter
process 4 died: unbound error: unbound symbol: iter
thread '<unnamed>' (2552127) panicked at crates/lisp/src/eval/mod.rs:380:45:
index out of bounds: the len is 0 but the index is 1
```

`fold`, `%eq` are global names. `acc`, `pred`, `iter` are parameter
names inside prelude functions. That an env lookup for a *parameter*
fails strongly suggests env corruption — the lexical scope chain is
being read while another thread mutates it.

**Reproduction:** the Mandelbrot demo at the foobar project, with
`workers ≥ 4` and `-j 0`. Probes with 2–3 workers and short bodies
*sometimes* succeed; the race is contention-sensitive.

**Workaround:** `nest run -j 1` (single-threaded scheduler). Works
reliably, but kills the speedup story.

**Suggested investigation:**

- The panic site `eval/mod.rs:380` — start there.
- Whether `Heap` or `EnvId` is `Send` but not actually safe to share.
- Whether `def` (which the prelude does at load) is racing with
  worker spawns. Note: `defprocess`-generated functions are `defn`s,
  so they're added to the global env after `require 'hatch` completes.
  If a worker is spawned while another thread is still resolving
  prelude symbols, the env could be in an intermediate state.

### 3.2 No supervision / link primitives in the quick-ref

`monitor` and `demonitor` are listed but no example shows them.
`link` isn't mentioned. For a worker-pool demo I had to write
`stop-all` myself and the workers' receive loops never exit cleanly.
A `with-supervisor` macro or a documented `link` would make this far
nicer to teach.

### 3.3 `receive` with no pattern that matches

In probes, if a message doesn't match any clause it appears to be
held in the mailbox indefinitely (correct selective-receive behavior).
But during debugging I had a clause that *should* match shape-wise but
didn't because of a typo, and the process simply hung. There's no
"this message was sent but never matched" diagnostic. A debug flag or
`receive-strict` macro that errors after N ms with unmatched messages
would have saved me 20 minutes.

### 3.4 Process death messages don't tell you which process

`process 6 died: unbound error: …` — process 6 is opaque. If processes
were `(register …)`'d with a name, surface the name. Stack trace of
where it died would be even better.

---

## 4. Performance

Numbers from `claude-opus-4-7` running on a 28-core Raptor Lake-S,
`brood` release build:

| benchmark                              | time   | per-op    |
|----------------------------------------|--------|-----------|
| 1,000,000 tight `(+ acc 1)` recursion  | 3.87 s | 3.9 μs    |
| 100,000 vec destruct + 2-elem rebuild  | 0.56 s | 5.6 μs    |
| spawn 1,000 empty processes            | 17 ms  | 17 μs     |
| 64×28 Mandelbrot, 80 iter (sequential) | 880 ms | (≈30 iter avg → ~1700 cells × ~30 ≈ 50k cstep, each ~17 μs end-to-end) |

**Observations:**

- The variadic `+` going through `(fold %add 0 xs)` allocates a rest
  list and a closure on **every call**. `(+ a 1)` is 3.9 μs, which is
  3.9 μs *of GC pressure per arithmetic op*. For a number-heavy demo
  this dominates.
- Vector destructure + rebuild is 5.6 μs. For a `cstep` that does 2
  destructures and 1 vector construct plus the math, ~10–15 μs is the
  floor without compiler help.
- Spawn is cheap (17 μs). That's good news; the runtime can handle
  large process counts.
- The 880 ms Mandelbrot is interpreter-bound, not algorithm-bound.

**Suggested directions:**

- A `%add2` / `%sub2` / `%mul2` *fast-path* for the 2-arg case that
  skips the rest list and the closure allocation. The kernel already
  exposes the primitives — the variadic wrapper just needs an arity
  check that short-circuits.
- Strength-reduce `[x y]` (a 2-element vector) into a primitive pair
  representation when the compiler can prove the length is fixed. Or
  expose a kernel `pair`/`triple` constructor that doesn't allocate
  a vector header.
- A bytecode pass before evaluation — the AST is being walked at
  runtime, including resolving variadic dispatch in `+` per call.
- A boxed-int / boxed-float distinction is presumably already there;
  worth confirming the hot path doesn't take a heap allocation for
  int/float results.

These are larger lifts. The low-hanging one is **the 2-arg fast path
in variadic `+/-/*/=` etc.** That alone could halve interpreter
overhead for numerics.

---

## 5. Stdlib gaps

Things that should exist for demo writers and didn't, or that I'd had
to reach into the prelude to find:

### Numeric / output

- `(format "%.2f" x)` or `(to-fixed x 2)` — anything to render a float
  with a fixed precision. Currently I wrote
  `(let (x10 (quot (* 10 a) b)) (str (quot x10 10) "." (mod x10 10) "x"))`
  to display a single decimal place. That's 60 characters of distraction
  in a demo.
- `(bench label expr)` macro that prints `label: N ms` and returns
  `expr`. I wrote my own; everybody who writes a demo will too.
- `(now-ns)` for finer timing than ms. The Mandelbrot finished in 880 ms
  with default settings, but a benchmark loop of 1000 cells finishes
  in <1ms and `(now)` resolution wipes it out.

### Parallel / process

- `(parallel-map f coll [n-workers])` — the cliché demo of "fan out and
  gather" is currently 30+ lines (worker loop, dispatch, collector,
  defprocess, hatch, stop-all, await-done). Could be one function.
- `(supervise [pids…] strategy)` — for shutdown, restart, etc.
- A worked example of `monitor` somewhere visible.

### Collections / strings

- `(repeat n x)` — making a list of N copies of a value. I wrote
  `(map (fn (_) x) (range n))` instead.
- `(string-repeat s n)` — for things like the `+--------+` border in
  the demo I hand-typed 64 dashes.
- `(pad-left s n)` / `(pad-right s n)` — basic column formatting.
- `(round-to x decimals)` — same as format/to-fixed above.

### Error / debug

- A `(debug! x)` that prints with full structure (a real `pr` rather
  than `str`) and returns the value, for inline tracing.
- `(throw-with-stack)` or attach stack info to thrown values.

---

## 6. Tooling

### 6.1 `nest format` is too aggressive at single-lining (severity: medium)

After `nest format`, my carefully aligned code became:

```lisp
;; before
(let (w       64
      h       28
      iter    80
      workers 4
      region  [-2.2 -1.1 1.0 1.1])
  …)

;; after
(let (w 64 h 28 iter 80 workers 4 region [-2.2 -1.1 1.0 1.1])
  …)
```

```lisp
;; before
(defmacro time-it (expr)
  (let (t0 (gensym "t0") r (gensym "r"))
    `(let (~t0 (now)
           ~r ~expr)
       [(- (now) ~t0) ~r])))

;; after — 120 char line
(defmacro time-it (expr)
  (let (t0 (gensym "t0") r (gensym "r")) `(let (~t0 (now) ~r ~expr) [(- (now) ~t0) ~r])))
```

The formatter is collapsing forms that the writer broke up *for
semantic alignment*. Reasonable defaults for a Lisp formatter:

- A `let` with **3+ bindings** stays multi-line (one binding per
  line, columns aligned).
- A `defn` body with multiple top-level forms stays multi-line.
- A quasiquoted template stays in the shape the author wrote it.
- A `cond` / `match` / `receive` with multiple clauses stays one
  clause per line.

This is the standard Clojure/Racket/Emacs-Lisp formatter behavior.
The current Brood formatter feels closer to `prettier --print-width=120`
on JS — which is the wrong target for a Lisp.

### 6.2 Type-checker warnings on `defprocess` / `hatch` / `!` / `cast` (severity: high for first-time UX)

```
src/mandel.blsp:53:1: warning: unbound symbol: defprocess
src/mandel.blsp:53:23: warning: unbound symbol: state
src/mandel.blsp:57:3: warning: unbound symbol: cast
src/mandel.blsp:66:6: warning: unbound symbol: !
src/mandel.blsp:86:13: warning: unbound symbol: hatch
```

These print on **every** `brood file.blsp` invocation even when the
program runs perfectly. The macros come from `(require 'hatch)`. The
type-checker should:

- Recognize macros and macro-introduced bindings.
- Or run after `require` resolution rather than before.
- Or suppress unknown-symbol warnings inside the body of a form whose
  head is itself a known macro (defprocess introduces its own scope).

This is the first thing a new user sees. It looks broken.

### 6.3 `nest run` has no `--quiet` / `--no-banner`

The output of `nest run` interleaves the program's stdout with what I
assume is nest's status. For a demo I want to capture just the
program's output. A `--quiet` flag (suppress nest's chatter) and a way
to direct stdout cleanly would help.

### 6.4 No REPL command discovery from a project

`brood` (no args) starts the REPL but doesn't load the project's
`require`d modules. To interactively poke at my demo I'd want
`nest repl` to drop me into a session with the project's main module
already required.

### 6.5 Error UX

Two things that hurt:

- **Rust panics surface as panics.** When a pattern destructure fails
  on an unexpected shape, I get a Rust backtrace including a
  `RUST_BACKTRACE=1` hint. That's developer-facing; users should see
  a Brood-level error.
- **"process N died: unbound error: …" is process-local but printed
  to main's stdout** without context. Which process? Where in the
  source? The kernel knows the spawn site; surface it.

### 6.6 `nest check` is silent on real type problems

I had a `(defn worker-loop (col))` calling `worker-loop col` (typo)
that would have been a static error in any typed language. `nest check`
ran without complaint. The advisory mode is a great choice; the
checks could be sharper.

---

## 7. The demo itself — what worked

For the record, when run with `nest run -j 1` the demo produces:

```
  Brood — concurrent ASCII Mandelbrot

  +----------------------------------------------------------------+
  |                                        ..                        |
  |                                         ...,                     |
  |                                        ..,..                     |
  …
  |                                        ..                        |
  +----------------------------------------------------------------+

  width=64  height=28  max-iter=80  workers=4
  sequential   880 ms
  concurrent   864 ms   (1.0x)
```

Source: `~/src/whk/foobar/src/mandel.blsp` and `src/main.blsp`. The
program exercises:

- Multi-clause / destructured `fn` params.
- Tail-recursive `escape--iter`.
- Transducer in the render path (`xmap` + `str` as the reducing fn).
- `defprocess` collector with a `cast` clause that returns the next
  state, with a guard to `send` the parent when work is complete.
- Selective `receive` with two patterns (`[:job …]` and `:stop`).
- A `gensym`-based `time-it` macro returning `[ms result]` for
  destructure-in-binding.
- Round-robin dispatch as a `fold` over `(range h)`.
- `dolist` for side-effecting print.
- `cond` with `else`.
- Pattern destructure in `let` (`[t-seq img]`).

I'm proud of the code; I'm not proud of the parallelism story. The
runtime is what's standing between the program and a real speedup
graph.

---

## 8. Prioritized recommendations

If you can only do **three things**:

1. **Fix the scheduler race in eval/mod.rs:380.** Without this, you
   can't ship any concurrent example, and the actor story is what
   makes Brood interesting.
2. **Fix the type-checker noise** for macros from required modules.
   First impressions matter a lot — the demo file lights up red on
   first load.
3. **Soften `nest format`** so multi-line `let` / `cond` / `defmacro`
   stays multi-line.

If you have **a week**, add to that list:

4. Expand `docs/brood-for-claude.md` to cover the missing builtins,
   `defprocess`/`hatch`, and a worked worker-pool example.
5. Add `(format …)`, `(bench …)`, `(now-ns)`, `(string-repeat …)`,
   `(repeat …)`, `(pad-left …)`, `(pad-right …)`, `(round-to …)`,
   `(parallel-map …)` to the prelude.
6. Make pattern-destructure failures throw a Brood error, not a Rust
   panic.

If you have **a month**:

7. 2-arg fast-paths for `+`, `-`, `*`, `/`, `=`, `<` to halve numeric
   overhead.
8. Either a bytecode pass before eval, or aggressive monomorphisation
   for fixed-arity calls. This is where the order-of-magnitude
   interpreter speedup lives.
9. `link` / `monitor` worked examples and a `with-supervisor` macro.
10. `nest repl` that loads the project context.

---

## 9. What I'd want to test next

If the scheduler race were fixed, the natural next demo would be:

- **Mandelbrot zoom animation** — terminal frames at 30+ FPS, using
  ANSI escape codes (life.blsp already does this). Workers compute
  frames; main process drives the redraw.
- **Distributed N-queens** — `node-start` / `remote-spawn` are
  intriguing kernel features that I didn't touch.
- **A toy bytecode VM written in Brood** — `defprocess` workers as
  CPU cores, executing a tagged-data ISA, would showcase pattern
  matching and immutable state machines simultaneously.

All three are blocked on the same runtime issue.

---

## Appendix A — exact reproduction of the race

In a fresh `nest new` project, add:

```lisp
;; src/race.blsp
(defmodule race)
(require 'hatch)

(defn worker (parent i)
  (let (xs (map (fn (n) (* n n)) (range 1000)))
    (send parent [:done i (reduce + 0 xs)])))

(defn main ()
  (let (me (self) n 16)
    (dotimes (i n) (spawn (worker me i)))
    (dotimes (_ n)
      (receive ([:done i sum] (println "worker " i " => " sum))))))
```

Then `nest run`. I have not run this exact reproduction, but based on
the mandel demo's failure mode I expect 1–3 of the 16 workers to die
with `unbound symbol: acc` / `pred` / `fold` errors, and the rest
plus main to hang waiting for the missing messages. Confirming this
is the right starting point before diving into `eval/mod.rs`.

---

*End of document.*
