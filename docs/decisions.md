# Design decisions (ADR log)

Short records of *why* we chose what we chose, so we don't accidentally
relitigate settled questions. Newest at the bottom.

---

## ADR-001 — Implement the runtime in Rust (not C or Zig)

**Status:** accepted.

**Context.** We need a host language for the interpreter. The realistic
candidates on this machine were Rust, C, and Zig. A key constraint: a lot of
this codebase will be written with heavy AI assistance ("vibe-coded").

**Decision.** Rust.

**Why.**
- **Memory safety is the highest-value property when AI writes a lot of code.**
  The failure mode to avoid is *silent* corruption (use-after-free, UB). Rust
  turns most of that into compile errors — "if it compiles, the shape is
  probably right" is exactly the guardrail we want.
- **Best AI training corpus of the three**, so generated code is more likely to
  be idiomatic and current. Zig is pre-1.0 and changes fast, so models often
  emit outdated syntax; C is fine to generate but its mistakes are dangerous.
- **The roadmap is paved with mature crates:** `ropey` (text rope), `tokio` +
  `serde` (the server and display protocol), `crossterm` (terminal frontend). C
  and Zig would mean hand-rolling these.
- **Tight feedback loop:** one toolchain, `cargo test`/`cargo run`,
  rust-analyzer.

**Trade-off accepted.** The borrow checker is awkward for graph-shaped data
(environments, closures). We mitigate with the standard `Rc`/`RefCell`-now,
tracing-GC-later pattern (see ADR-002), which is well-represented in training
data (Piccolo, other Rust Lisps).

**Considered & rejected.** Elixir/BEAM is philosophically great for hot-reload
and distribution, but unnecessary here: because the editor is written in Brood,
re-evaluating definitions already gives hot-reload, regardless of host language.

---

## ADR-002 — `Rc`/`RefCell` now, tracing GC later

**Status:** accepted.

**Decision.** Use `Rc<…>` for heap values and `RefCell` for environment
mutation in v0.1. Plan a migration to `gc-arena` before editor sessions become
long-lived.

**Why.** Simplest correct thing; gets us moving. The known cost is that
reference cycles (a closure capturing an environment that reaches it) leak —
irrelevant for a REPL and the early milestones.

**Containment.** All heap construction goes through helpers in `value.rs`, so the
GC migration is localised.

---

## ADR-003 — Lists are cons cells; `[ ]` vectors are separate

**Status:** accepted.

**Decision.** The fundamental list is the cons cell (`Pair`), proper lists end
in `nil`, and `()` reads as `nil`. Vectors `[ ]` are a distinct type that
evaluates its elements.

**Why.** Cons-cell lists keep the language homoiconic, which is what makes
macros and "code is data" natural — essential for a self-editing editor.
Vectors give a clean, modern surface for parameter lists (`(fn [x y] …)`) and
data, matching the Clojure-ish aesthetic.

---

## ADR-004 — Clojure-style truthiness and flat `cond`

**Status:** accepted.

**Decision.** Only `nil` and `false` are falsy. `cond` uses flat `test expr`
pairs with `else`/`:else` as the catch-all, rather than Scheme/CL clause-lists.

**Why.** Consistency with the modern/Clojure-leaning surface already chosen
(vectors, keywords). Flat `cond` is simpler and reads well; multi-expression
branches can use `do`.

---

## ADR-005 — v0.1 has zero external dependencies

**Status:** accepted.

**Decision.** The whole v0.1 (language + CLI) uses only the Rust standard
library. The REPL reads line-buffered stdin rather than pulling in a line-editor
crate.

**Why.** Hermetic builds, nothing to break, and a first version that's trivial
to read end-to-end. Dependencies arrive with the features that justify them
(`ropey`, `tokio`, `serde`, a line editor).

---

## ADR-006 — As much of the language as possible lives in Brood

**Status:** accepted.

**Decision.** Anything that doesn't *need* to be a Rust builtin goes in
`std/prelude.lisp` instead.

**Why.** Whatever is written in Brood is redefinable at runtime. Maximising
that surface is the entire point of the project. Rust provides mechanism;
policy lives in the language.

---

## ADR-007 — Brood is a Lisp-1

**Status:** accepted.

**Decision.** A single namespace shared by functions and variables (like
Scheme/Clojure), not the separate function/value namespaces of Common Lisp or
Emacs Lisp.

**Why.** The operator position of a combination is resolved with the same lookup
as any variable, so functions are ordinary first-class values. This is what lets
higher-order code read naturally (`(map f xs)`, `(reduce %add 0 xs)`) and is a
prerequisite for ADR-008 — defining `+` and friends as plain `def`s only works
because a function is just a value in the one namespace.

**Trade-off accepted.** A local binding can shadow a global function of the same
name. That's the well-understood Lisp-1 cost and matches the Clojure-leaning
aesthetic already chosen.

---

## ADR-008 — Rust is a primitive kernel; the language is written in Brood

**Status:** accepted. Supersedes the original "builtins live in Rust" approach.

**Context.** The core principle (ADR-006) is to write as much of the system in
Brood as possible. Initially the math/list functions (`+`, `-`, `map`, `reduce`,
…) were Rust loops.

**Decision.** Reduce the Rust surface to an **irreducible primitive kernel** and
define every user-facing function in `std/prelude.lisp` on top of it. The kernel
is the 2-argument numeric ops (`%add`/`%sub`/`%mul`/`%div`/`%lt`/`%eq`, plus
`mod`/`rem`), pair/vector constructors and accessors, type-tag predicates,
value↔text and I/O, and the self-hosting hooks (`eval`/`read-string`/`load`/`apply`).
`+ - * / < > = map filter reduce list …` are now Brood `def`s. (See spec §9.)

**Why.** Uniformity (`+` is defined exactly like a user function), and maximal
runtime editability — the whole arithmetic/sequence library can be redefined
live. It also exercises the language hard, surfacing gaps early.

**Trade-off accepted.** Brood-defined arithmetic is materially slower than a
native loop (the tail-recursion test went from ~5s to ~50s at 1,000,000
iterations; we right-sized it to 100,000). This is acceptable for now and
reversible: a future compiler/specialiser, or selectively re-promoting hot ops to
Rust, can recover the speed without changing the surface language.

---

## ADR-009 — Clojure-style quasiquote; commas are whitespace

**Status:** accepted. Resolves the previously-deferred quasiquote question.

**Decision.** Quasiquote uses `` ` `` (quasiquote), `~` (unquote), and `~@`
(unquote-splicing). The comma `,` is treated as whitespace.

**Why.** Consistency with the rest of the Clojure-leaning surface (vectors,
truthiness, `def`/`defn`, flat `cond`). Choosing `~` for unquote frees `,` to be
insignificant whitespace, which is a small but real ergonomic win. Macros are
unhygienic with `gensym` for hygiene-by-convention (CL/elisp style); hygienic
macros remain possible future work.

---

## ADR-010 — Code is cons-lists; vectors are a data type

**Status:** accepted. Refines ADR-003 (reverses its "vectors as the parameter-list
surface" stance).

**Context.** ADR-003 introduced `[ ]` vectors and used them, Clojure-style, for
parameter lists and `let` bindings. Revisiting this against the project's north
star — a *self-editing editor* that constantly rewrites Brood source — the
homoiconic argument won: if code is uniformly cons-lists, macros and the editor's
own code-manipulation never have to special-case "vector vs list".

**Decision.** *Code* (parameter lists, `let` bindings) is written as **lists**:
`(defn f (x y) …)`, `(let (a 1 b 2) …)`. **Vectors `[ ]` remain a first-class
data type** for when O(1) indexing/length matters (`vector-ref`,
`vector-length`). Vectors are still *accepted* in parameter/binding positions for
leniency, but lists are idiomatic and the prelude is written entirely in list
form.

**Why.**
- Homoiconic code is the whole point of a self-editing Lisp: one structure, one
  set of operations, uniform metaprogramming.
- Keeping vectors as *data* preserves fast random access without compromising the
  "code is lists" property — best of both (the analysis behind this is recorded
  for posterity: lists win for code/sequences, vectors win only for indexing).

**Trade-off accepted.** A mild inconsistency — code uses `( )`, some data uses
`[ ]` — and the small readability cost that a parameter list `(x y)` looks like a
call form. Worth it for homoiconic code.

---

## ADR-011 — Favor the simplest user-facing design; defer power features

**Status:** accepted.

**Decision.** When a language feature has a simple form and a powerful-but-complex
form, ship the simplest one the user can hold in their head, and defer the rest
until a concrete need justifies the added complexity.

**First application — the parameter grammar.** We designed the full CL-grade
space (`&optional`, `&key`, required-keywords-via-lazy-defaults, supplied-p
flags) and then cut it to **`required` + `&optional` (with defaults) + `& rest`**.
`&key` (named args) and supplied-p are deferred — they are additive (no migration
cost to add later) and not yet needed. See spec §7.4.

**Why.** Every knob is a tax on everyone who reads or writes the language, paid
forever; a deferred feature costs nothing until added. This keeps the surface
learnable and the implementation small. It complements ADR-006/008 (a small
kernel) on the *ergonomics* side: small kernel, small surface.

---

## ADR-012 — A process-wide byte-counting allocator for memory introspection

**Status:** accepted.

**Decision.** Install a `#[global_allocator]` (`crates/lisp/src/alloc.rs`) that
wraps the system allocator and maintains two relaxed atomics — live bytes and a
peak high-water mark — exposed to Brood as the `mem-bytes` / `mem-peak`
primitives. It is declared in the `brood` library (not the CLI binary) so the
CLI and every integration-test binary share one allocator.

**Why.** Reading the process's memory use genuinely needs Rust (you can't
bootstrap it on top of cons/`+`), so it belongs in the kernel — like `now`
(ADR-008). A wrapping allocator is the simplest accurate option: it counts
*every* Rust allocation, which is exactly the "how much memory did this use"
number, and needs no `/proc` parsing or extra crate (stays dependency-free,
ADR-005). The alternative — instrumenting `Heap`'s `alloc_*` — would miss
allocations behind std collections and only counts objects, not bytes; revisit
it when the tracing GC lands (ADR-002), where an arena reports live bytes for
free.

**Trade-offs.** The allocator is *always on*: two atomic ops per (de)allocation
process-wide (negligible, but real), and declaring it in the library forces it
on all downstream binaries (fine for this workspace; could be feature-gated if
that ever bites). The counters surfaced their value immediately — the test
suite peaks at ~300 MB because there is no reclamation yet (ADR-002), making
`mem-bytes` ≈ `mem-peak`; the two will diverge once the GC exists.

**Deferred — CPU time.** Wall-clock (`now`) covers the common case. True
user+sys CPU time would need `getrusage`/`libc` (against ADR-005) or
Linux-specific `/proc` parsing; deferred until a concrete need (e.g. attributing
cost across the thread-backed processes).

---

## ADR-013 — A runtime's inner processes share live code; separate runtimes don't

**Status:** accepted. Supersedes the earlier "instances are independent / no
shared mutable global" decision (commit 081fda9, which dropped shared-code steps
4–5).

**Context.** Two requirements that first looked contradictory: (a) updating a
function in one runtime must *not* propagate to other connected runtimes/nodes;
(b) a long-running **spawned** process — e.g. a web server — must pick up a
redefinition *without being restarted*. The earlier reading collapsed both into
"every process is independent," which satisfies (a) but fails (b): a snapshot
process never sees updates. The resolution is a matter of **scope**, and it's
exactly Erlang's: a code server holds the *current* code, and every call
re-dispatches through it (Brood, being a late-binding Lisp-1, re-dispatches on
*every* call — no `Module:fun` needed). Code is shared and live; data is not.

**Decision.** A **runtime** owns one mutable, shared code region + global table
(`RuntimeCode`, behind `Arc`). **All processes it `spawn`s share that same
`Arc`**, so a `def` is visible to a running inner process on its next lookup
(cross-process hot reload). **Separate runtimes (future nodes) each get their own
`RuntimeCode`**, so updates never cross between them. Data stays per-process: each
process has its own LOCAL heap; messages cross as deep copies.

**How.** A 2-bit handle region tag — `LOCAL` (per-process data) / `PRELUDE`
(immutable, shared by all runtimes) / `RUNTIME` (mutable, per-runtime, shared by
inner processes). `RUNTIME` code is **append-only** (a redefinition adds a new
version; in-flight calls finish on the old one). The global scope is a sentinel
(`EnvId::GLOBAL`) routing to a `RwLock<HashMap>`; `def` **promotes** the bound
value's reachable code (and any captured environment) from LOCAL into RUNTIME
before rebinding. See `docs/shared-code.md`.

**Why.** It's the only model that gives editor-style hot reload *across* a
runtime's processes (the project's north star) while keeping nodes independent
for safe deployment. Late binding + append-only code gives the Erlang semantics
(in-flight calls keep old code, new calls get new) for free.

**Trade-offs accepted.** Global reads take a brief `RwLock` read; `def` deep-copies
code into the shared region (append-only, never reclaimed yet — same GC debt as
ADR-002). A closure that captured a *local* scope and is then shared has that
scope promoted too; `set!` on such a promoted (now shared) frame is a no-op — a
rare, documented limitation. Cross-runtime/node code distribution is deliberately
out of scope (a future, explicit deploy step, not silent propagation).

---

## ADR-014 — Runtime crates are allowed when they remove real complexity

**Status:** accepted. Relaxes ADR-005 (which had already been superseded on the
CLI side by `rustyline`).

**Decision.** The `brood` library may depend on a well-scoped crate when it
genuinely cuts complexity or unsafe code, rather than hand-rolling substrate. The
bar is **infrastructure that helps build the runtime**, not Lisp-callable
behaviour: functions the *language* exposes are still written in Brood (`std/`),
per ADR-006/008 — we don't pull a crate to provide a builtin users could write in
Brood.

**First application.** `boxcar` backs the shared `RUNTIME` code region (ADR-013):
a lock-free, append-only vector whose references stay valid across concurrent
pushes. It removes a hand-rolled `unsafe` lifetime-extension *and* gives lock-free
reads on the hottest path (every process thread reading closure bodies while a
`def` appends). The global bindings table stays a std `RwLock<HashMap>`.

**Why.** Getting the concurrency substrate correct by hand is exactly where bugs
hide; a purpose-built, audited crate is lower-risk than our own `unsafe`. "Get it
working, then decide" — and the decision is: take the crate where it earns its
keep.

**Trade-off accepted.** A dependency in the runtime crate (build time, supply
chain). Mitigated by the high bar above and by keeping Lisp-level behaviour in
Brood.

---

## ADR-015 — Share-safe, parallel-by-default test framework

**Status:** accepted.

**Context.** The test framework (`std/test.lisp`) is written in Brood and runs
tests as processes. Under ADR-013 those processes **share** the global table, so
the original design — workers tallying into shared mutable globals (`*passed*`,
`*failed*`) — raced and miscounted (failures attributed to the wrong test, double
counts).

**Decision.** Make tallying **share-safe** and adopt an ExUnit / `mix test`
surface:
- `describe` groups, `test` cases (string-named); `deftest` kept as an alias.
- Assertions are **macros that push onto a process-local `*fails*`** (a `let` the
  `test` macro establishes); each test **yields its failures as a value**. The
  runner aggregates from returns/messages into its own local state — no shared
  counters.
- **Parallel by default** (each test its own process), with opt-in serialisation:
  `:serial` (a group's tests run one-at-a-time in a single worker, alongside other
  groups) and `:isolated` (a group/test runs alone, in an exclusive phase after
  the parallel batch).

**Why.** Sharing code but not tally state is the only way concurrent tests don't
clobber each other. `:serial`/`:isolated` give tests that *do* touch shared global
state (a `def`, a hot-reload) a way to opt out of the race, mirroring ExUnit's
`async` model. See `docs/testing.md`.

**Trade-off accepted.** Assertions, being macros over `*fails*`, must be used
lexically inside a test body, not from unrelated top-level helpers — acceptable,
and the normal way tests are written.

---

## ADR-016 — Arena-reset reclamation at top-level boundaries (first GC step)

**Status:** accepted. First concrete step of memory reclamation; revises (does not
yet fulfil) ADR-002's "tracing GC later."

**Context.** The heap arenas only grew — a long REPL session or a long-running
process leaked every cons/env it ever allocated. Spawned processes already free
their whole `Heap` on thread exit, so the leak is specifically *long-lived*
processes. A full tracing GC hits a wall: our `eval` is a native recursive
tree-walker, so live `Value`s sit on the *Rust* call stack where a collector
can't find them as roots. Worse, a mark-sweep rooted only from the current env is
**unsafe mid-evaluation** — sibling sub-expressions strand live values in local
`argv`s reachable from no scannable root.

**Decision.** Reclaim by **arena reset at top-level boundaries**, not tracing.
`Heap::checkpoint()` snapshots the LOCAL slab lengths; `Heap::reset_local_to(cp)`
truncates them back. `eval_str` resets between top-level forms (keeping the
final result); the REPL resets to a baseline after each command. This is safe
precisely because **globals live in the PRELUDE/RUNTIME regions and never point
into a process's LOCAL heap** (a top-level `def` *promotes* its value out, ADR-013)
— so at a quiescent boundary the only live LOCAL value is the form's result, which
is consumed/printed before the reset. O(1), no tracing, no mark bits.

**Why.** It's the simplest thing that's *provably* safe and reclaims the real
leak (the suite/REPL demo: ~712 MB growing → ~78 MB flat across heavy forms). It
needs no eval rewrite and touches nothing shared or concurrent.

**Limits / what's deferred.**
- It does **not** help a single never-returning loop (a server `(loop)` with no
  top-level boundary) — that needs reclamation *during* evaluation.
- Safe mid-eval GC needs the evaluator's roots to be scannable, i.e. an explicit
  value-stack VM — which is also what **4b** (green-process coroutine suspension)
  needs. So general GC and 4b share that prerequisite and should likely be done
  together; `gc-arena` (ADR-002's original target) fits our native recursive eval
  and shared multi-thread RUNTIME region poorly and is no longer the presumed path.
- `truncate` retains Vec capacity (bounded by the largest single form), so steady
  state is the peak form's footprint, not zero — fine, and avoids realloc churn.

---

## ADR-017 — Isolated tests roll back the globals via a private copy (`%isolate`)

**Status:** accepted. Strengthens the `:isolated` mode of the test framework
(ADR-015) from *scheduled-alone* to *state-isolated*.

**Context.** A runtime's processes share one mutable global table (ADR-013), so
the test framework offered `:serial`/`:isolated` to avoid *races* on it. But
`:isolated` only meant "no other test runs concurrently" — every test, isolated
or not, still `def`/`set!`s into the *same* live table, so a test's definitions
persisted and were visible to later tests. That's not true per-test independence.

True isolation wants a fresh runtime per test, but the model rules that out
cheaply: a test thunk is a closure whose handle is region-tagged to *its* runtime
(it indexes that runtime's append-only code slabs), so it cannot be executed in a
different runtime — cross-runtime code sharing is deliberately unsupported (ADR-013).
Re-evaluating each test's *source* in a fresh `Interp` would work but moves test
execution out of the in-language framework and reloads the framework per test.

**Decision.** Isolate **bindings**, not the whole runtime, with one small Rust
mechanism. `Heap::snapshot_globals()` clones the global table (values are `Copy`
handles — cheap); `Heap::restore_globals()` puts a snapshot back. The `%isolate`
primitive wraps a thunk: snapshot → run → restore (even on error). The framework
runs the `:isolated` phase **first** and calls each isolated test through
`%isolate`, so every isolated test sees the clean post-load baseline and nothing
it defines survives. Policy stays in Brood (`std/test.lisp`); Rust supplies only
the snapshot/restore mechanism (ADR-006/008).

**Why.** Proportionate (ADR-011): it delivers the property that matters — a test's
defs can't leak to another test — with one primitive and no eval changes, instead
of a fresh-runtime machinery the architecture doesn't cheaply allow.

**Limits / what's deferred.**
- Rolls back **bindings** only. The append-only code slabs and the global symbol
  interner still grow (memory, not behaviour; there's no GC yet — ADR-016).
- The LOCAL data heap isn't reset by `%isolate` (it carries no cross-test state).
- Sound only because the isolated phase runs alone: `restore_globals` is a
  wholesale swap, unsafe if another process were writing globals concurrently.
- If a genuine fresh-runtime-per-test need appears, source re-eval in a new
  `Interp` remains the fuller (heavier) option.

---

## ADR-018 — Green M:N scheduler via stackful coroutines (step 4b)

**Status:** accepted. Implementation plan in `docs/scheduler.md`.

**Context.** Step 4a runs one OS thread per process and blocks the thread at
`receive` — it oversubscribes cores, needs the `Gate` cap, and can deadlock when
more processes block than the cap allows. Step 4b makes processes cheap green
threads on a small worker pool, with `receive` suspending rather than blocking.

**Decision.** **Path A — stackful coroutines (`corosensei`).** Each process runs
in a coroutine with its own parkable stack, so the native recursive `eval` runs
unchanged; `receive` on an empty mailbox yields the coroutine. A worker pool
(≈ `nproc`, a *setting* — never a magic number; `-j` overrides) runs ready
processes off a shared run queue; `send` wakes a waiting process. `Heap` is
already `Send`, so processes migrate between workers freely.

- **Not** the explicit-value-stack VM (Path B) — that's a far bigger rewrite,
  only needed for precise mid-eval GC, and deferred.
- **Cooperative to start** (yield only at `receive`); reduction-counted
  preemption (the BEAM's fairness mechanism — decrement a counter in `eval`'s
  loop, yield at zero) and work-stealing are **additive later**, not a redesign.
- `corosensei` does the stack-switching `unsafe` we'd otherwise hand-roll
  (ADR-014). Swappable if we later want to slim dependencies.

**Why.** It delivers cheap green processes + bounded OS threads + suspending
`receive` with no evaluator rewrite — the lowest-risk path to finishing 4b. It's
"BEAM-minus-preemption-minus-migration," both of which are additive.

**Trade-offs accepted.** Per-coroutine stacks cost memory (tunable). Cooperative
scheduling lets a CPU-bound process with no `receive` hold its worker until done
(bounded by pool size; preemption is the deferred fix). A dependency in the
runtime crate (justified per ADR-014).

---

## Deferred / open questions

- **Macro hygiene:** currently unhygienic `defmacro` + `gensym`; hygienic macros
  (e.g. `syntax-rules`) are possible future work.
- **Nested quasiquote:** not level-tracked in v0.1 (see spec §spec note); fine
  for ordinary macros, revisit if needed.
- **`car`/`cdr` vs `first`/`rest`:** both provided; `first`/`rest` are the
  documented default.
