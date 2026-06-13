# `brood-life` feedback — improving Brood across performance, style, features, and types

*Feedback from the `brood-life` test-bed (a sibling repo, `../../brood-life`), synthesised
from four parallel deep-dives and cross-checked against this repo's kernel + docs + stdlib,
the `brood-type-deferred` worktree, `brood-edit`, and `brood-benchmarks`. Dated 2026-06-13.
Triage tracked in [`roadmap.md`](roadmap.md).*

This document answers the test-bed's founding question: **what should we change in the Brood
language to improve (1) performance, (2) code style, (3) available features, and (4) the
ease of building good abstractions + type checking?** The Game of Life demo is the vehicle —
a small real program pushed hard enough to surface the language's rough edges — and its
`docs/game-of-life-notes.md` flaw log is the primary field evidence.

> **Path convention.** Bare source references like `life.blsp:484`, `bitboard.blsp:18`, and
> `perflog.blsp` are files in the **`brood-life`** repo (`../../brood-life/src/`); its flaw log
> is `../../brood-life/docs/game-of-life-notes.md`. Bare `docname.md` references and
> `crates/…`/`std/…` paths are in **this** (`brood`) repo.

---

## 0. Executive summary

Brood is in much better shape than its "small dynamic Lisp" framing suggests. It already
ships a **bytecode VM** (the tree-walker is now only a fallback), a **Cranelift JIT** (behind
a flag, integer-only), a **generational per-process copying GC**, a **set-theoretic advisory
type checker** with optional runtime contracts, **namespaces**, **transducers**, and a
**distributed green-process runtime**. The gaps are narrower and more specific than "it's an
interpreter, it's slow."

Three findings cut across all four axes and should anchor the roadmap:

1. **Records are the missing keystone.** State today is bare maps threaded through every
   loop (`(get m :key)` appears **35×** in `life.blsp`, `(get …)` **88×** total). Adding a
   `defrecord`-style construct simultaneously fixes the biggest *style* tax, the biggest
   *abstraction* gap, and the most common *type* error (silent map-key typos). It's a prelude
   macro — no kernel change. **Highest single-item leverage.**

2. **Allocation does not yet scale across cores.** A global lock on the allocate/collect path
   serializes every process whenever it allocates; 8 allocation-heavy processes run **8× —
   perfectly serial** (`brood/docs/roadmap.md`). Since immutable data means *everything*
   allocates, this is the deepest perf ceiling, and it's the one with **no design doc yet**.

3. **The pieces meant to relieve the hot paths are shipped-but-broken or off.** User-facing
   transients exist but **corrupt under GC when the value expression allocates**; the JIT is
   **off by default and integer-only**. Both are higher-impact to *finish* than to start
   anything new.

A consolidated, prioritized action table is in [§6](#6-consolidated-priorities).

> **Documentation drift, flagged for the maintainers.** Several design docs are now stale
> relative to the shipped kernel: `transients.md` says user-facing transients were *rejected*
> (they shipped); `spec.md §11` lists map literals / dynamic vars / modules as "not yet
> specified" (all shipped); `value-repr.md` says `Value` is 16 bytes (the JIT layout test pins
> it at 24). Worth a sweep — these mislead anyone planning from the docs.

---

## 1. Performance

### 1.1 What actually exists today

The execution engine is a three-rung ladder, **all rungs shipped**:

| Rung | Status | Gain |
|---|---|---|
| Tree-walker | retained only as `BROOD_VM=0` fallback | baseline |
| Closure-compiling `Node`-VM | superseded | ~1.6–2.3× |
| **Bytecode VM** (flat `Chunk`, non-recursive `exec_chunk`) | **default + sole executor** (ADR-100) | +25–45% over `Node`-VM |
| **Tier-1/2 template JIT** (Cranelift → native) | **shipped behind `--features jit`, OFF by default** | up to **~65×** on int loops |

Supporting facts:
- The VM call stack is **reified heap data** (`Vec<BcFrame>`), not native recursion — this is
  what enables precise mid-eval GC and full live-process migration across worker threads, and
  let the project delete the `corosensei` coroutine dependency entirely.
- **GC**: per-process, generational, semi-space copying collector (ADR-072). No write barrier
  (immutability ⇒ no old→young pointers). Reported ~8× faster / ~9× lower RSS than the prior
  single-space collector. Collects at any eval depth (ADR-061).
- **Scheduler**: M:N green processes, work-stealing, live migration, reduction-counted
  preemption — BEAM-style.
- **Value repr**: a 24-byte tagged enum; NaN-boxing was measured and **deliberately rejected**
  (zero tier-1 upside, would regress the immediate `Pid`/`Ref`/`Socket` paths).

This is the crucial correction to "Brood is a slow interpreter": on the JIT build, integer
compute is near-native; the *default* build is what's 10–50× behind.

### 1.2 Bottlenecks, ranked

**B1 — Allocation serializes across processes (highest impact, no design yet).** A global lock
on the alloc/collect path means allocation-bound work gets *zero* multicore speedup:

| parallel tasks | pure compute | alloc-heavy (build a 3.6k map) |
|---|---|---|
| 1 | 156 ms | 52 ms |
| 8 | 615 ms (4.0×) | 421 ms (**8.1× — fully serial**) |

This directly contradicts the benchmark suite's "the scheduler is not the bottleneck"
conclusion — that holds for *compute*-bound `pfib` but not for the allocation-bound code that
most immutable-Lisp programs actually are. In `brood-life`, splitting SIM (`recolor`) and
RENDERER (`render`) across processes "buys responsiveness but **not** frame-time — pipelining
them was a measured no-op." That no-op is B1.

**B2 — Interpreter dispatch floor on the default (no-JIT) build.** From `brood-benchmarks`
(2026-06-13), Brood vs the fastest runtime: `bintree` 50×, `matmul` 38×, `mandelbrot` 25×
(float — JIT can't help), `wordcount` 23× (map-rebuild). `fib`/`pfib` are now competitive. The
Life demo's "~1µs/op, ~33k ops per 30fps frame" floor is this bottleneck as a frame budget.

**B3 — No read-modify-write transients → persistent-map (CHAMP) churn dominates map-building
loops.** The original Life `step` was ~85% HAMT allocation (275 ms, ~235 ms of it churn). The
internal-transient work accelerates *bulk last-wins* builds (`into {}`, `zipmap`) ~1.6× but
**explicitly does not touch read-modify-write** (`frequencies`, `group-by`, the Conway
neighbour tally) — which is exactly the Life hot loop and the `wordcount` benchmark.

**B4 — Structural map keys hash ~2× slower than integers.** Re-keying the colour layer from
`[x y]` vectors to integer bit-indices cut `recolor` ~24%. Logged lesson: "if a map is rebuilt
every frame, key it on a scalar."

**B5 — Float JIT gap.** `mandelbrot` is a self-tail loop that *should* JIT but bails on the
first float constant; the JIT subset is integer-only.

**B6 — A GC use-after-GC / "slab out of bounds" bug class.** Most historical instances were
stale-binary recurrences (an `nest mcp` server outliving a rebuild — now guarded). Two genuine
rooting bugs were found and fixed in the 2026-06-03 kernel audit. **But a new variant is open**
(see B3's transient corruption and the four crash dumps in `.brood_crash_dump`, all at
`heap.rs:3114`).

### 1.3 Proposals

| # | Proposal | Impact | Effort/risk | Existing plan? |
|---|---|---|---|---|
| P1 | **Per-process / thread-local allocation off the global lock** (or a concurrent collector) | Very high — unlocks multicore for the alloc-bound code that is most real Brood; would let the SIM/RENDERER split actually pipeline | High / high (touches GC core + scheduler) | **No design doc** — only the diagnosis. The highest-value *un-designed* item. |
| P2 | **Fix user-facing-transient use-after-GC corruption, then ship them properly** | High — unblocks ~order-of-magnitude cheaper map building; halves Life `recolor`, fixes `wordcount` | Medium, but same hard rooting-under-moving-GC bug class as B6 | Diagnosis exists; the in-progress transient's mutable node buffer isn't a scannable GC root |
| P3 | **Turn the JIT on by default + add float specialization** | High — ~65× int loops already; float would take `mandelbrot` 75ms → <5ms | Medium-high (float adds a type-specialization axis; needs RUNTIME-compaction survival first) | **Yes, detailed** — `jit-float.md`, `jit-stage1.md`, `jit-tier2.md` |
| P4 | **Bump-pointer nursery for small short-lived objects** | Medium — `bintree` (50×) is walk-bound on short-lived nodes | Medium | Proposed, not designed |
| P5 | **Targeted bulk kernel primitives for RMW** (`frequencies`/`group-by` over an internal transient) | High for the specific kernel | Low effort, **high architectural cost** | `transients.md` sanctions this (option B) but **rejects** a domain-specific `%life-step` as violating "write the language in the language" |

**The strategic question — can pure Brood ever hit interactive perf?** The language's own
answer, demonstrated by this very demo, is *"yes, by choosing a representation whose operations
are already native — not by writing Rust builtins."* The Life `step` went 35.7 ms → 0.6 ms
(**57×**) not via a kernel primitive but via a pure-Brood representation change: pack the whole
board into one bignum so a generation is ~100 native big-int ops instead of 19k interpreter
dispatches. That is the sanctioned escape hatch. The honest caveat: it only reaches interactive
speed for (a) representation-restructured loops that bottom out in native bignum/vector ops and
(b) integer compute once the JIT is on — **not** for allocation-bound or float-heavy pure-Brood
on the default build, and **not in parallel** until B1/P1 lands.

---

## 2. Code style & ergonomics

The code is genuinely well-written and several ergonomic calls are excellent. The friction is
concentrated in **one place**: reading and writing fields of long-lived state maps.

### 2.1 What reads well (keep doing this)

- **Flat sequential `let`** (`(let (a 1 b (+ a 1) [x y] vec) …)`) — the single best readability
  decision. `bitboard/make` (`bitboard.blsp:18`) reads like prose because each binding sees the
  last. Destructuring in binding targets (`[shape s1] (sample s *shapes*)`) makes the
  `[value next-seed]` PRNG idiom about as clean as explicit seed-threading can be.
- **Names carry their role** (`in-footer?`, `hsv->rgb`, `seed--sow`, `colors-refit`) — you know
  the shape before reading the body.
- **`receive` with pattern clauses reads like a protocol** — `sim-loop` (`life.blsp:585`) shows
  the whole actor contract at a glance.
- **Multi-arity / multi-pattern `defn`** in the prelude (`+`, `get`, `assoc`) is exemplary and
  reads better than Clojure's vector clauses because list-headed clauses don't visually collide.

### 2.2 Friction points (every claim grounded in real lines)

- **`(get m :key)` is everywhere — the dominant tax.** In `life.blsp`: **88** `(get …)` calls,
  **78** of them `(get <var> :<key>)` field reads; the state map `m` alone is read **35×**.
  `sim-step` (`life.blsp:484-532`) is the worst — ~17 of its ~30 `let` bindings exist only to
  pull a field out of `m`. `(get m :s)` appears twice on one line (`:503`).
- **The mirror image: wide `assoc m …` updates** — `sim-loop` repacks 9 keys (`life.blsp:523`).
  The function spends ~17 lines unpacking `m` and ~10 repacking it.
- **`[ui redraw?]` / `[model cmd]` tuple threading** — destructure, act on the flag, recurse:
  a 3-line ceremony repeated at `render-mouse`, the key handler, `on-mouse`, `on-key`.
- **Nested clamps are noisy** — `(max 0 (min ox (max 0 (- *w* (quot *w* z2)))))` (`life.blsp:702`)
  appears ~8× and **there is no `clamp` in the prelude**.
- **`(do (send …) [v flag])`** — sequencing an effect before a return adds a paren layer
  throughout `on-key`/`footer-press`.
- **Deep accessors** — `get-in`/`assoc-in`/`update-in` *exist and are documented* but are used
  **0×** in `life.blsp` and **once** in all of `brood-edit`, while `(get (get pane :payload) :top)`
  nesting recurs.

### 2.3 Proposals (before/after)

**A — `{:keys [...]}` map destructuring in `let`/`fn` (highest value).** Kills the 35×-per-file
`(get m :key)` tax in one line:

```lisp
;; before — 17 field-pulls at the top of sim-step
(let (ob (get m :board) held (get m :held)
      hn2 (if held (+ (get m :hn) 1) 0)
      [b1 s1] (if repeat? (apply-clicks board [held] (get m :s)) [board (get m :s)]) …) …)

;; after — name everything once
(let ({:keys [board s gen prev held hn next-id last-inj inject colors renderer logger]} m
      held-age (if held (+ hn 1) 0)
      [b1 s1] (if repeat? (apply-clicks board [held] s) [board s])) …)
```

Pure pattern-matcher work — the matcher already compiles `[a b]` and `(p & rest)`; a
`{:keys [..]}` pattern is a natural extension that mirrors the map literal's own syntax, changes
nothing about immutability, and has clear Clojure precedent. **Worth it.**

**B — `as->` for threading state through conditional transforms (medium).** `->`/`->>` don't fit
when the map isn't the first/last arg; `as->` (a named slot) does, and it's **not in the
prelude**. Cheap to add. Be honest: threading macros are culturally underused (2 sites in 6600
lines of `brood-edit`), so ship it for completeness, don't expect heavy uptake.

**C — a `clamp` prelude fn (small, cheap, obvious).** `(clamp lo hi x)`. The fact that
`life.blsp` hand-rolls it ~8× is the signal it belongs in std. **Worth it.**

**D — promote the existing `get-in`/`update-in`, and consider records.** D1 (docs/skill nudge,
zero language change): when you see `(assoc m :board (f (get m :board)))`, write
`(update m :board f)`. D2 (language): a record/struct — but this is covered better under
[§4](#4-abstractions--the-type-system) since it's primarily an *abstraction* change.

### 2.4 Consistency gaps

- **The stdlib ships nested-map tools the idiom ignores** (`get-in`/`assoc-in`/`update-in`: 0
  uses in `life.blsp`). The docs *say* use these; the canonical demos *don't*. Resolve the
  tension — either lint toward them or accept that line-by-line `(get m :k)` reads "clearly
  enough."
- **`else` vs `:else`** are used interchangeably, sometimes both in one file (`life.blsp:194`
  vs `:716`); the prelude mixes them too. Pick one, lint the other.
- **The docs slightly over-promote threading** — the `(-> person (assoc :born 1816) …)` example
  is not how anyone actually writes Brood.
- **A positive note:** the documented `(defn f ([x y]) …)` trap is respected everywhere — except
  `pack-rgb`/`unpack-rgb` (`life.blsp:214`), which is the exact discouraged form and slipped
  through. Worth fixing as a canary.

---

## 3. Features

### 3.1 Capability map (what ships)

Core types incl. **auto-promoting bignums**, CHAMP maps, vectors, ropes. ~100-primitive kernel;
everything else is Brood. Rich stdlib (`format`/`json`/`csv`/`regex`/`datetime`/`crypto`/`set`/
`queue`/`stats`/`net`/`proc`/`editor/*` …). Green processes + `hatch` + supervisors +
distributed nodes. GUI + terminal display protocol. `nest` CLI + LSP + MCP + formatter. Seedable
PRNG, bitwise + `bit-positions`, transducers, namespaces, pattern matching, dynamic vars.
**User-facing transients are shipped** (superseding the "deferred" doc) — though see B2/P2,
they're currently broken on allocating values.

### 3.2 Gaps surfaced by real use

1. **No atomic file append / streaming / file handles — the headline I/O gap.** `perflog.blsp`
   holds the *entire log in memory and re-spits the whole file every line* because there is no
   append primitive: *"Brood's `spit` overwrites and there's no append / file-handle API yet."*
   `file/append-file` exists but is flagged non-atomic, "pending a kernel append primitive."
2. **`read-string` silently drops trailing forms.** `(read-string "(def a 1) (def b 2)")` →
   `(def a 1)`, no error. The `nest mcp` `eval` tool inherits this, so pasting a multi-form block
   runs only the first form and the rest vanish — surfaced as "unbound symbol" downstream. A
   `read-all` builtin now exists but `read-string`/`eval` aren't wired to error or use it.
3. **No `#{…}` set literal / `set?`** — sets are maps-of-`true`; you test with `map?`.
4. **RMW transient acceleration** missing (the Life `step` tally) — see B3.
5. **No lazy sequences / `iterate`** — every unbounded stream (file lines, frames, undo) reinvents
   a tail-recursive `--at` accumulator.

### 3.3 Prioritized proposals

**Small wins (design notes exist):**

| Feature | Why | Effort |
|---|---|---|
| `read-string` errors on trailing input (or `eval` routes through `read-all`/`do`) | Silent data loss; bites every MCP/agent paste; `read-all` already exists | XS |
| Atomic file `append` primitive (+ file handle) | The only real-use I/O gap; unblocks any logger/streaming output | S (append) / M (handles) |
| `nest format --changed` | Removes whole-tree diff noise | S |
| Expose `argv` to Brood (wrap the existing `%argv`) | Needed for standalone `nest build` binaries | XS |

**Big rocks:**

| Feature | Why | Effort |
|---|---|---|
| **Lazy sequences + `iterate`** | Unbounded streams; full design sketch exists in `deferred.md` | L (new value kind + GC) |
| **Keyword args `&key`** | Editor command-API ergonomics; purely additive | M |
| **RMW transient acceleration** | The Life `step` hot loop; any reduce-into-map workload (needs an ADR-026 amendment or targeted primitives) | L + ADR |
| **First-class `#{…}` set type** | Ergonomics; the lib is already forward-compatible | M (reader+printer+value+hash+checker) |

---

## 4. Abstractions & the type system

**Framing correction:** `brood-type-deferred/` is **not** an abandoned fork — it's a git
*worktree* of `brood/` parked on the type branch, and the type docs are byte-identical to
mainline. **The type system was designed *and shipped* (slices 1–10).** What's "deferred" is a
named subset (full inference, Option-A type variables in primitives, gradual-assignment
checking) — deliberate "ship the simple form, defer the power" calls, not an abandonment.

### 4.1 Abstraction facilities today

Multi-arity / multi-pattern `defn` (one `match` compiler reused at every binding site); macros
with quasiquote + auto-gensym + namespace-robust auto-qualification; modules-as-namespaces
(ADR-065); dynamic vars; the `hatch` process framework (written in Brood); transducers.

### 4.2 The abstraction gaps — and their cost in `life.blsp`

- **No records/structs.** The SIM's entire state is one untyped 14-key map whose schema lives
  only in a *prose comment* (`life.blsp:480`: "see `run-life` for its keys"). Costs: key typos
  are silent (`get` → `nil`), no field types, the shape is invisible to tooling, the schema
  drifts from the comment.
- **No protocols / multimethods.** No open dispatch on value type. `type-matches?`
  (`prelude.blsp:255`) is a hand-written 30-line `cond` ladder over every tag — exactly the
  boilerplate a protocol/multimethod would erase.
- **How people fake it:** maps-as-records (the `m` and `ui` maps), tagged vectors + `match`
  (`[:text row col …]`, `[:set-fps n]`), and hand-packed-int "structs" (`pack-rgb`/`unpack-rgb`
  encode `[r g b]`+count into bit-fields, layout documented only in a comment).

### 4.3 The type checker — substantial, advisory, shipped

Fully dynamic at runtime, but a real **set-theoretic advisory checker** runs automatically at
file/project boundaries (`BROOD_NO_CHECK=1` opts out) and **never gates** a runnable program.
It catches today: provable type misuse via *disjointness* (`(+ 1 "x")`), **arity** on every
call, **unbound symbols** with "did you mean?", **non-tail self-recursion**, macro-hygiene and
function-as-value lints — all flowing through the LSP (per-keystroke) and `nest mcp`.

Annotations (shipped): `(sig name (param… -> ret))` is checker-facing trust (TypeScript-style);
`(sig! …)` *also* installs a runtime contract (a same-arity wrapper that throws on mismatch),
making the function a sound "strong arrow" — all policy in Brood, no new primitive.
`BROOD_CONTRACTS=1` enforces every `sig` for a dev run. Inference is deliberately
*straight-line only*; full body inference is the explicitly deferred piece (it needs
control-flow analysis, "the only false-positive source — so we cut it"). Parametric result
types use per-HOF rules (Option B, shipped) so `(first (map inc xs)) : number | nil` flows;
full type variables in primitives (Option A) wait for a real consumer.

### 4.4 Proposals

1. **`(defrecord …)` as a prelude macro + an auto-generated `(sig …)` per field — THE top
   recommendation.** It expands to a tagged map plus a constructor, named accessors, and field
   contracts. No kernel change (pure ADR-006 policy-in-Brood). It closes the biggest gaps on
   *three* axes at once:
   - **Style:** `(sim-state-board m)` / `{:keys …}` destructuring replaces the 35× `(get m :key)`.
   - **Abstraction:** the state map gets a *name*, a *field list tooling can see*, and a single
     place the schema lives.
   - **Types:** map-key typos become catchable — the accessor is a named global, so the existing
     unbound-symbol check flags `(sim-state-colrs m)` where it can never flag `(get m :colrs)`.

   This is the clear keystone: the highest-leverage item in the whole document.

2. **Multimethods over protocols (`defmulti`/`defmethod`, dispatch on a user fn).** More
   Lisp-idiomatic and lower-surface than Clojure's `defprotocol` (which Brood explicitly
   rejects). Removes the `type-matches?`-style ladders. Defer until a second consumer needs open
   dispatch — follow the documented trigger pattern.

3. **Continue the existing type path; don't pivot.** Gradual `(sig)` + auto-checker + `sig!`
   contracts is exactly the pragmatic "gradual + separate pass + opt-in runtime contracts"
   design the question asks for — it's built and sound. The next increments are records (#1
   above, which hands the checker real schemas) and landing Option-A type variables *only* when
   a user codebase with generic HOFs needs them.

4. **Recategorize the `read-string` bug.** The flaw log says "Brood is dynamically typed, so no
   checker catches this" — but it's a **reader/eval-seam** bug, not a type bug; the type system
   wouldn't catch it. Fix it at the reader (§3.3), not the checker.

---

## 5. Cross-cutting themes

- **Records are the keystone** — they're the top item on the style *and* abstraction *and* type
  axes. Build them first.
- **Finish before you start.** Transients (shipped, broken) and the JIT (shipped, off,
  integer-only) are higher-impact to *complete* than any greenfield work.
- **The deepest ceiling has no plan.** Parallel allocation (B1/P1) is the one thing blocking the
  multicore story for real (allocation-bound) Brood code, and it's the only top-tier perf item
  with no design doc.
- **Trust the "representation, not primitives" doctrine — but document it.** The 57× Life `step`
  win came from a pure-Brood representation change, and that's the language's sanctioned path to
  speed. It should be written up as an explicit performance-engineering guide, because it's
  non-obvious and it's the answer to "why no `%life-step` primitive?"
- **Sweep the stale docs** (transients, spec §11, value-repr byte count) so future planning
  starts from reality.

---

## 6. Consolidated priorities

Ranked by leverage (value ÷ effort), keystones first.

| Rank | Item | Axis | Effort | Has a plan? |
|---|---|---|---|---|
| 1 | **`defrecord` macro + per-field `sig`** | style + abstraction + types | M (prelude macro) | new — sketch here |
| 2 | **`{:keys [...]}` map destructuring** | style | S (matcher extension) | new |
| 3 | **`read-string`/`eval` trailing-form fix** | features | XS (`read-all` exists) | flaw log |
| 4 | **Fix transient GC corruption, then ship** | performance | M (hard rooting bug) | diagnosed |
| 5 | **Atomic file `append` primitive** | features | S | flaw log |
| 6 | **`clamp` + `as->` in prelude** | style | XS | new |
| 7 | **JIT default-on + float specialization** | performance | M-H | `jit-float.md` |
| 8 | **Parallel allocation off the global lock** | performance | H | **none — needs an ADR** |
| 9 | **Lazy sequences + `iterate`** | features | L | `deferred.md` |
| 10 | **`defmulti`/`defmethod`** | abstraction | M | new (defer to a 2nd consumer) |
| — | Doc-drift sweep (transients/spec/value-repr) | hygiene | XS | — |

The single recommendation if only one thing gets done: **records (#1)** — it's the keystone that
pays off across style, abstraction, and type safety, and it costs only a prelude macro.
