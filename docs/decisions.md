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
`std/prelude.blsp` instead.

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
define every user-facing function in `std/prelude.blsp` on top of it. The kernel
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

**Context.** The test framework (`std/test.blsp`) is written in Brood and runs
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
or not, still `def`s into the *same* live table, so a test's definitions
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
it defines survives. Policy stays in Brood (`std/test.blsp`); Rust supplies only
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

## ADR-019 — Emacs-flat modules: `provide` / `require` / `load-path`, not namespaces

**Status:** accepted; not yet implemented.

**Context.** Today `require` (builtins.rs) is hardcoded to embedded modules — it
knows only `'test`, baked in with `include_str!`; `load` takes a *literal* path,
with no search and no load-once. There is no `provide`, no `*load-path*`, no
feature tracking. As Brood grows a real `std/` and user projects appear, code
must be loadable *by capability name*, once, from configurable locations. The
fork: a flat, Emacs-style namespace, or first-class namespaced modules
(Clojure/Racket-style per-file resolution with explicit imports/exports).

**Decision.** **Flat, Emacs-style modules over the one shared global table.**
- `*features*` (a global list) records what's loaded; `(provide 'name)` adds it,
  `(require 'name)` returns early if present.
- `*load-path*` (a global list of dirs) is searched for `name.blsp`; the first hit
  is `load`ed (evaluated into the shared globals), then `require` checks the
  feature was actually provided.
- Embedded std modules (prelude, `test`, …) stay baked into the binary so it runs
  from any directory; `require` consults the embedded table before the load-path.
- **Mechanism vs policy (ADR-006/008):** the only new Rust is filesystem
  reflection — `file-exists?`, `list-dir`, `cwd` — plus one primitive that hands a
  baked-in module's source to Brood. `provide` / `require` / `load-path` themselves
  are Brood, in `std/prelude.blsp`.
- **Convention, not mechanism:** `foo--internal` (double dash) marks "private",
  Emacs's lightweight interface signal. Unenforced.

**Why.**
- *Matches the architecture as built.* One shared mutable global table per runtime
  (ADR-013); `load` already evals into root. Flat modules add ~no core machinery —
  Brood functions + 3 fs primitives. Namespaces would touch the symbol model
  (`value.rs`: interned `u32`, no namespace axis), the reader (`foo/bar`),
  env/eval (per-namespace resolution), the `RuntimeCode` global table (re-keying),
  and the hot-reload path — the single largest expansion of the core, against
  "keep the language as small as possible" and ADR-011.
- *Right semantics for the goal.* Brood exists to be the language of a
  self-editing, Emacs-like editor, and such an editor is *defined* by a flat,
  openly-redefinable global namespace (advice, monkey-patching, redefining
  anyone's function live). ADR-013's cross-process hot reload is the Brood-native
  form of exactly that. Namespaces would fight the "any code can redefine any
  behaviour at runtime" property the project exists for.
- *Forecloses nothing.* Namespaces can arrive later, additively, along a spectrum
  without revisiting this decision: (1) flat [now]; (2) flat + a pure-Brood
  `defmodule` / `import` macro layer that prefixes names (`text/insert`) in the flat
  table — **zero core change**, since symbols already carry `/` / `-` and lookup
  stays "find the symbol"; (3) first-class per-file resolution [costly core change]
  only if a package ecosystem ever demands it. ADR-011: ship the simple form,
  defer the powerful one.

**Trade-offs accepted.** No isolation — two modules can clobber each other's
names; the only guard is naming convention (prefixes, `--` privates), exactly as
in Emacs Lisp. No machine-checked exports. Fine now (you run only your own code;
no package ecosystem), recoverable later via the macro layer above. A concurrent
re-`require` of the same absent feature can double-load; idempotent like Emacs,
and not worth guarding now (ADR-011).

---

## ADR-020 — Project model: `project.blsp` + a discovery-based test runner

**Status:** accepted; not yet implemented.

**Context.** We want (a) a notion of "a Brood project" — a root, source/test
directories, a name/version — and (b) a tool that *finds and runs* all of a
project's tests, instead of hand-listing cases and calling `(run-tests)` at the
foot of one file. The test framework (ADR-015) already separates **registration**
(`describe` / `test` → `*units*`) from **execution** (`run-tests`) — exactly what
discovery needs. Fork: a project file as Brood *source* (`project.blsp`) or as
inert *data* (`Brood.proj`).

**Decision.** **Convention over configuration** (Mix / Cargo style), with a
manifest for identity.
- **Conventional layout — no config to get the normal case working.** `src/` holds
  the project's Brood source (prepended to `*load-path*`, so its files are
  `require`-able by name); `tests/` holds tests, discovered as `*_test.blsp`
  recursively. A fresh project that puts code in `src/` and tests in `tests/` needs
  no path declarations at all.
- **`project.blsp`** — a Brood-source manifest in the Leiningen `project.clj`
  mould, mainly declaring *identity*: `(project :name … :version …)`. It reads as
  data but is eval'd, so computed config is available when wanted. **Project
  root** = the nearest ancestor directory containing `project.blsp` (like
  Cargo/git).
- **Override, don't enumerate.** The conventional dirs are defaults; the manifest
  *overrides* them (`:source-paths`, `:test-paths`) only when a project deviates —
  you never list paths just to get the standard layout running.
- **Test discovery** — under each test path (default `tests/`), every file matching
  `*_test.blsp`, recursively. A test file only *registers* (`(require 'test)` +
  `describe` / `test`); `nest test` loads them all, then calls `(run-tests)`
  **once**. Test files no longer call `run-tests` themselves.
- Surfaced as a CLI path — `nest test` (and an in-language `(run-project-tests)`)
  — with the discovery/load/run logic written in Brood on the ADR-019 fs
  primitives. Rust stays the thin substrate (CLAUDE.md core principle).

**Why.**
- **Convention over configuration.** Cargo and Python (`src/` + `tests/`), Mix
  (`lib/` + `test/`), Leiningen (`src/` + `test/`): a new project works with zero
  path plumbing, the manifest declares identity not layout, and every project looks
  alike so it's navigable. `src/` + `tests/` are the defaults (matching the Cargo
  workspace Brood lives in), overridable for the rare project that needs to deviate.
- `project.blsp`-as-code is the most Brood-native choice (dogfooding), needs zero
  new core (`load` already evals a file), reads as data yet keeps the
  computed-config escape hatch — the Leiningen model, consistent with Emacs's own
  config-is-code (and with flat modules, ADR-019). Pure-data (`Brood.proj`) buys
  safety (don't eval an untrusted manifest) and external-tool friendliness, but
  both matter only with a package ecosystem (premature — ADR-011), and "data"
  today is a clunky alist because map literals (`{}`) aren't in the language yet.
- Discovery by `*_test.blsp` (Go / ExUnit's `*_test.exs`) lets test files coexist
  with helper files in `tests/`; aggregating into one `run-tests` preserves the
  framework's parallel-by-default scheduling across the *whole* suite (ADR-015)
  rather than per file.

**Trade-offs accepted.** Eval'ing `project.blsp` runs arbitrary code on project
open — fine while you run only your own projects; revisit (a data subset, or a
sandboxed read) if third-party projects arrive. Discovery is convention-bound
(`tests/`, `*_test.blsp`). Migration: the current single `tests/suite.blsp` (which
calls `run-tests` itself) gets reorganised into register-only `*_test.blsp` files,
with `cargo test`'s `suite.rs` invoking the discovery runner.

---

## ADR-021 — Pattern matching: one Brood compiler, reused at every binding site

**Status:** accepted; implemented. Design in `docs/pattern-matching.md`.

**Context.** Erlang/Elixir-style pattern matching subsumes two Tier-2 roadmap
items (destructuring in `let`/`fn`, and `case`) and sets up `receive` clauses. A
Lisp can't copy Elixir's `=`-is-match operator: code is data, so `(:ok x)` is
indistinguishable from a call and `=` is a plain function (ADR-008) that
evaluates both operands. The Lisp-faithful translation is to put **one pattern
grammar at every binding form** and let those binds be refutable.

**Decision.** A single pattern→code compiler, **written in Brood** (`std/prelude.blsp`),
emitting nested `if`/`let` over existing primitives — no Rust matcher, no new
special form (the `try`/`catch` precedent: a macro over primitives, ADR-006/008).

- **Surfaces.** `match` (value dispatch; `case` is just `match` with literal
  patterns); refutable/destructuring `let`; `fn`/`defn` clauses (multi-clause
  Erlang dispatch + pattern parameters). `match*` is the shared engine; each
  surface is a thin layer that picks the failure context.
- **Grammar.** `_` wildcard; a bare symbol **binds** (a repeated one is a
  non-linear equality constraint); literals match by `=`; `'sym` matches a
  symbol; `~expr` is a pin (match the value of `expr`); `(p …)` / `(p & rest)`
  list patterns; `[p …]` fixed-length vector — the **tagged-data idiom**, chosen
  for constructor/pattern symmetry (the same literal builds *and* matches).
- **Clauses are wrapped** `(pattern [:when guard] body…)` — one clause shape for
  `match`/`fn`/`receive`; guards and multi-form bodies fit; misuse is a loud
  compile-time error. (`let` stays flat `pattern value …`.)
- **Failure crashes with a structured, catchable value**
  `[:match-error <context> <value> <patterns>]` (Erlang "let it crash"); add a
  `_` clause to total a match. The macro also raises **compile-time** errors for
  malformed `&`, unreachable clauses after a catch-all, and bad `:when`.
- **`let`/`fn` are lowered in the compile pass** (ADR-022), not the evaluator:
  a non-symbol target / a multi-clause or pattern-param `fn` is desugared to
  `match*` once at definition, so the common case is fast. The evaluator *also*
  keeps the design's Option-A delegation as a **fallback** — if such a binder
  reaches it unlowered (built in a quasiquote unquote, or from a macro expanded
  lazily within its defining form), eval lowers it on the fly via `macroexpand_all`
  and `continue 'tail`. Compile pass = speed; eval fallback = correctness. This
  realises "one matcher, kept in Brood, stays redefinable."

**Why.** Maximum power for one mechanism, all in Brood (redefinable later — map
patterns, custom extractors), the core unchanged. Tail position is preserved
(each chosen body lands in the generated `if`/`let` tail), so match/receive loops
are TCO-safe.

**Trade-offs accepted.** A bare symbol always binds (the one trap — match a known
value with a keyword, `'sym`, or `~pin`). The fn-clause failure context is `:fn`,
not the function's name (the name is attached after closure creation) — a legible
nicety deferred. Pattern destructuring of `&optional` slots is deferred (ambiguous
defaults; rare; additive). The textbook fail-continuation duplication is left as-is
(patterns are shallow; thunk it if measured — see the design doc's code-size note).
The generated code is **unhygienic** (ADR-009): it references the primitives it
emits by bare name, so a local binding could shadow them. Equality uses the kernel
`%eq` (not `=`) by convention to remove the most likely collision; `first`/`rest`/…
remain shadowable until macro hygiene lands.

---

## ADR-022 — A macroexpand-all compile pass (expand once at definition)

**Status:** accepted; implemented.

**Context.** The evaluator expands macros lazily: a function body keeps its macro
calls unexpanded, so each *call* re-expands them. Cheap macros (`when`, `->`)
hardly notice; the pattern matcher's expander is heavy, so a `match` in a loop
cost ~25× a plain `if` (re-running the whole Brood compiler every iteration).
Correct and TCO-safe, but too slow for the receive loops `match` is meant for.

**Decision.** A **compile pass** — `macros::macroexpand_all`, a code walk that
fully expands every macro call (and lowers the pattern binders of ADR-021) —
run **once at each top-level / definition boundary**: `eval_str`, `load`,
`require`, `eval`, and the prelude loader, form-by-form (so a macro a form
defines is visible to the next). The evaluator **still** expands lazily as a
fallback, which covers a macro defined and used within the same top-level form
(not yet defined when the walk ran). `quote`/`quasiquote` are left opaque (their
contents are data; code inside `~unquote` still expands when it runs). For the
same reason, eval's `let`/`fn` keep an on-the-fly lowering fallback (ADR-021) for
a pattern binder that reaches them unlowered (built in a quasiquote unquote, or
from such a lazily-expanded macro).

**Why.** A `match` (or any macro) in a function body now expands once, so the
body runs at plain-`if` speed; it benefits *every* macro, not just `match`. It is
also the natural home for desugaring the `let`/`fn` pattern binders (ADR-021),
keeping the evaluator's core forms small.

**Trade-offs accepted.** Macros are now effectively *early-bound*: a closure
created before a macro is redefined keeps the old expansion (standard Lisp
compile-time-macro semantics; functions still late-bind, so live function
redefinition and cross-process hot reload are unaffected — ADR-013). Further
optimisation (caching, a fuller compile/closure-creation pass) is additive and
deferred.

---

## ADR-023 — First-class type tags; types stay runtime, checking stays advisory

**Status:** accepted; step 1 (reflection + diagnostics) implemented.

**Context.** Brood is dynamically typed: the only "types" are the `Value`
variants, checked ad hoc at the point of use inside primitives (`_ => type_err`).
The discriminant wasn't nameable from the language (no `type-of`), and error
messages dropped the offending value (`first: not a list` — but *what* was it?).
We want better diagnostics now and a path to *limited* compile-time checking
later — without inhibiting the language. The hard constraint is hot reload
(ADR-013): a `def` can rebind any global, including `+`, visible to running
processes. Only **special forms** are immutable (name-dispatched in `eval`
before any binding lookup).

**Decision.**
1. Make the runtime tag first-class: a `Tag` enum + `value::tag` (one mapping),
   and a `(type-of x)` primitive returning the tag as a keyword. Mechanism in
   Rust; the predicates and any richer checking are policy in Brood.
2. Type errors are self-identifying — `LispError::wrong_type(heap, who, expected,
   got)` renders op + wanted type + the actual tag and printed value. The tag
   word is the `type-of` name, so errors and reflection agree. In the same vein,
   every builtin declares an `Arity` enforced at one gate (`eval::call_native`),
   so wrong-count calls are clean arity errors instead of silently-tolerated
   missing/extra args. Both are runtime metadata a later compile pass can read.
3. Types stay **runtime-only**. No annotations, no static gating. Any future
   compile-time analysis (a pass over the ADR-022 expanded forms) is **advisory**
   and **local**: special-form *structure* may be a hard error (special forms
   can't be redefined, so it's always sound); literal misuse is a warning; free
   and global references are treated as `Any` (top of the lattice), which is what
   keeps the analysis from ever fighting hot reload.

**Why.** Reflection + good errors are pure wins with zero language risk and
unlock in-language checks (`assert-type`, optional contracts) written in `std/`.
Pinning "runtime-only, advisory, globals are `Any`" up front means a later
inference pass can't quietly drift into a static type system that would break
the dynamism the project depends on.

**Trade-offs accepted.** `type-of` distinguishes `:fn` (Brood closure) from
`:native` (Rust builtin) — it reports the *concrete* tag rather than collapsing
both to "callable" (`fn?` remains the callability predicate). Reflection is
honest about the implementation seam; `fn?` is the abstraction for code that
shouldn't care. The compile-time tiers beyond special-form structure are
deferred — additive, and gated on a real need.

---

## ADR-024 — Set-theoretic, gradual types: the model and the compatibility contract

**Status:** accepted; step 1 (the `Ty` lattice) implemented. Full plan in
[`types.md`](types.md). Refines ADR-023.

**Context.** ADR-023 made tags first-class and committed to *advisory,
runtime-only* checking, with free/global references treated as `Any`. The open
question was *which* type system. Surveying the field, **Elixir's set-theoretic +
gradual** system is the closest fit: it retrofits types onto a dynamic,
hot-reloadable BEAM language without breaking dynamic code — our exact problem,
solved by people who took the same constraint seriously.

**Decision.** Adopt the **set-theoretic, gradual** model; explicitly reject the
TypeScript-style "pragmatic but unsound" route.
- A **type is a set of values**; the atoms are the runtime `Tag`s. Type
  operations are set operations; **subtyping is set inclusion** (semantic
  subtyping), never syntactic rules.
- **Gradual via `dynamic()`** — the principled replacement for ADR-023's
  "globals are `Any`." `dynamic()` is **integrated into the set-theoretic
  algebra**, not a bolt-on: a bounded type `dynamic(bound)` (pure `dynamic()` =
  `dynamic(ANY)`) whose **consistent subtyping is *derived from* ordinary set
  inclusion** (Castagna & Lanvin, ICFP 2017; Castagna et al., POPL 2019 — the
  reconciliation Elixir uses), *not* the classic Siek–Taha consistency relation
  grafted alongside subtyping. A redefinable global (hot reload) is `dynamic()`,
  so typed/untyped code mixes without spurious errors, and it still composes with
  `∪`/`∩`/`¬`. **This supersedes ADR-023's "globals are `Any`" wording.**
- Checking stays **advisory** (ADR-023): warns and optimises, never rejects a
  runnable program (bar provably-sound special-form structure errors).
- Built in **small, independent steps** (the staircase in `types.md`), each
  shippable on its own; and governed by a **compatibility contract** (also in
  `types.md`) that every future change must honour — several points are
  compiler-enforced (a new `Value` needs a `Tag` + bit; a new primitive will need
  a signature, the way `Arity` is mandatory today).

**Why.** It is sound where it speaks and never inhibits where it can't — the only
combination compatible with a self-editing, hot-reloadable language. Pinning the
model and the contract now stops later work from drifting into a static system
that would break the dynamism the project exists for.

**Trade-offs accepted.** A full set-theoretic checker is a large system; we build
a deliberately small subset (flat tags first; structure and `dynamic()` later)
and stay advisory rather than carrying Elixir's full soundness-proof burden —
borrow the model, not the proof obligation.

---

## ADR-025 — A lossless, span-carrying CST for tooling, separate from the eval `Value`

**Status:** accepted; foundations implemented + the `brood-lsp` crate is live
(Tier 0 landed in commit b724f3f, 2026-05-27). Full plan in [`lsp.md`](lsp.md).
Done: the CST (`syntax::cst`, with shared lexical rules in `syntax::atom`);
leading-string **docstrings** on closures; the introspection primitives `doc` /
`arglist` / `global-names` / `bound?`; and the `crates/lsp` server — stdio
lifecycle, full document sync, and syntactic `publishDiagnostics` off the CST.
Next: the CST scope resolver (shared with the checker), then Tier 1 (completion,
hover + signature help, `documentSymbol`).

**Context.** Brood is meant to be the language of a self-editing editor, so a
language server (LSP) is on the path, not an afterthought (`tooling.md` already
anticipates "Stage 3: richer introspection for eldoc / completion / xref"). The
blocker: every interesting LSP feature — hover, go-to-definition, completion
context, semantic tokens, rename — answers *"what is at this cursor?"*, and the
evaluation `Value` can't say. Symbols are `Value::Sym(u32)`: `Copy`, interned,
deduplicated, **not heap-addressed**, so the same `foo` everywhere is one value.
The `form-pos` side-table is keyed by a heap pair-index, so it positions only
**list** forms, start-only — never the token under the cursor. Making `Value`
carry per-occurrence spans (boxing symbols, wrapping read nodes) would tax every
evaluation forever to serve tooling, and the `Copy` value model + tail-call loop
are load-bearing.

**Decision.** Give tooling its **own** tree: a lossless, span-carrying CST in
`syntax::cst`, separate from the reader's `Value`. It is **heap-free** (owned
`Node`s; no `Heap`, so a server holds many documents cheaply and `Send`s them),
**total / error-tolerant** (`parse` always returns a tree; malformed input
becomes `Error` nodes and parsing resumes), records a `Span { start, end }` of
**byte offsets** on *every* node (including trivia and each symbol token), and
keeps quote sugar *as written*. The eval reader and the CST parser stay separate
functions because they have opposite contracts — the evaluator **rejects** a
half-typed buffer, the LSP **must** parse one on every keystroke — but they
**share** the lexical rules (`is_delimiter`, atom classification, the escape
table) so they can't drift on what a token is. The server is a separate binary,
`crates/lsp` (`brood-lsp`), on `lsp-server` + `lsp-types` (synchronous — the
single-threaded `Interp` is not `Sync`, so a sync request loop owning the
document store avoids `tokio` and `Send`/`Sync` friction). It **never evaluates
user buffers**: syntactic diagnostics come from CST `Error` nodes; semantic ones
from the advisory checker (ADR-024), which is designed to analyse without
running. A small introspection surface (`arglist`, `global-names`, `bound?`)
feeds completion/hover.

**Why.** Deciding *once* how text maps to spans and to meaning lets every feature
read off that substrate instead of each one re-deriving position bookkeeping —
the alternative is a parser's worth of duplication that never agrees with itself.
A separate CST is also the architecturally standard split (execution tree vs.
lossless syntax tree, à la rust-analyzer) and keeps the eval hot path lean.

**Trade-offs accepted.** Two parsers sharing lexical helpers (a managed
divergence risk, bounded by sharing the token rules). The advisory checker today
returns un-located strings over *expanded* forms, so located semantic
diagnostics are a later increment that checks the **un-expanded** CST — which
means not seeing *into* macro-generated code at first (the same macro caveat
`tooling.md` already accepts for runtime-error positions). LSP `Position` is
UTF-16 code units, which neither byte spans nor the char-counting `Pos` match, so
the server owns a UTF-16-aware `LineIndex`. Docstrings (for `doc`/hover) need a
small additive language decision (ADR-011 shape: an optional leading string in a
`def`/`defn` body) and are deliberately deferred — the LSP design does not block
on them. Long-term the CST could subsume the `form-pos` side-table; not required
now.

---

## ADR-026 — Immutability: data is immutable; `def` is the only mutation (no `set!`, no `while`)

**Status:** accepted; implemented.

**Context.** Brood already had *zero* data-mutation primitives — no `set-car!`,
`vector-set!`, `string-set!`, no atoms. The only mutation in the language was
binding mutation: `def` (rebind a global — load-bearing for Erlang-style hot
reload, the project's north star) and `set!` (rebind the nearest existing
binding, local or global). An audit found every real `set!` use targeted a
*global* (`*features*`, the project config vars, the test framework's
registration state) — i.e. it was doing what `def` does — except one: the test
framework's process-local `*fails*` accumulator, `let`-bound and `set!`-mutated
per assertion. So `set!` was, in practice, either a redundant `def` or a local
mutable cell. `while`, the lone iteration special form, is only useful *with*
local mutation to make progress, and had no Brood users.

**Decision.** Commit to immutability and make it an invariant:

- **Lisp data is immutable.** No primitive mutates a `Value`; this stays true.
- **`def` (rebinding a global) is the only mutation in the language** — that is
  exactly what live redefinition / hot reload needs (ADR-013), and it is
  *binding* mutation, not data mutation. `def` inside a function still targets the
  global scope.
- **`set!` is removed** (special form deleted; the now-dead `Heap::env_set` with
  it). Global `set!` uses became `def`; local mutable accumulation is replaced
  (see the test framework, below). A `let`/`fn` binding never changes after it is
  made.
- **`while` is removed.** With no local mutation it can't make progress; loops are
  **recursion** (proper tail calls give O(1) stack) or, for evolving state,
  **processes** (`spawn`/`receive`). Reintroduce a named-`loop`/`recur` macro later
  if ergonomics demand it (ADR-011).
- **Mutable state, when genuinely needed, is expressed two ways — never a mutable
  `Value`:** a **process** holding evolving state in its loop (the Erlang model),
  or a **Rust-backed resource handle** (the coming M2 rope/buffer — an opaque
  mutable resource behind primitives, like a file handle).

**The test-framework consequence.** The per-assertion `*fails*` accumulator can't
survive without local mutation. Replaced with a throw-and-collect scheme that
stays immutable yet keeps multi-failure reporting: a failing assertion **throws** a
tagged record (`(:%test-fail loc details)`), and the `test` macro splits its body
into one thunk per top-level form, running each in its own `try` (`test--run`) and
folding the caught failures into a list. So failures across a test's forms are all
collected (a throw ends only its own form), with no mutable accumulator. The one
limit: multiple assertions nested inside a *single* form stop at the first (the
throw unwinds that whole form) — a process-backed cell could close that later if a
real need appears (ADR-011). A non-assertion error is recorded and stops the test.

**Why.** Immutability reinforces every existing pillar: the planned tracing GC
(no write barriers, no mutable roots), `Send` per-process heaps + copy-on-send
messages (no aliasing hazards), the append-only shared `RUNTIME` code region, and
the safe-Rust guardrail (ADR-001) — it removes the whole shared-mutable-aliasing
bug class. It also shrinks the core: two fewer special forms and a dead heap
method.

**Trade-offs accepted.** Test failures collect per top-level form, not per nested
assertion (above). No imperative loop — fine given TCO recursion and processes,
revisit with `loop`/`recur` only on real need. Repeated immutable `assoc`/`append`
is O(n²) accumulation; mitigations (`reduce`/`fold`, transients, persistent
structures) are deferred per ADR-011.

---

## ADR-027 — Reduction-counted preemption + selective `receive` with timeouts

**Status:** accepted; implemented. Realises `scheduler.md` stage 4 (the fairness
step ADR-018 deferred) and the `receive`-clause surface reserved in
`docs/pattern-matching.md`.

**Context.** The green-process scheduler was **cooperative**: a process yielded
its worker only at `receive`, so a CPU-bound process with no `receive` (a runaway
keybinding, an infinite loop) held its worker until it finished — on an N-worker
pool, N such processes starve everything, including the root. Separately,
`receive` was unconditional FIFO (arity-0, popped the head): no way to wait for a
*specific* message (head-of-line blocking), and no timeout (a process waiting on a
message that never comes suspends forever). Both block the editor milestone — and
both were already designed as *additive* steps.

**Decision.** Two coupled additions, sharing the coroutine yielder and the `match`
compiler; no new special form.

1. **Reduction-counted preemption** (the BEAM's mechanism). `eval`'s `'tail:` loop
   calls `process::tick()` once per iteration — a thread-local `Cell<u32>`
   decrement (budget ≈ 2000, reset by the worker before each `resume`). At zero, a
   green process yields its worker and is re-queued **Ready**. The coroutine now
   yields a `Suspend` reason: `Receive` (park on the mailbox, as before) vs
   `Preempt` (re-queue at the back so peers get a turn). The root thread has no
   yielder, so `tick` just refreshes its budget — the root is never preempted.
   Top-of-loop placement is correct *and* complete: every non-terminating
   computation re-enters the loop infinitely often, and no lock/borrow is held
   there. Proper tail calls are untouched.

2. **Selective `receive`** with patterns, guards, and `after`. `receive` becomes a
   Brood **macro** over a `%receive` primitive (arity 3: a matcher fn, a timeout in
   ms or nil, an on-timeout thunk or nil). The macro reuses `match-build-from` with
   the no-match continuation set to **`nil`** (not the structured throw) and wraps
   each clause body in a **thunk**, producing a matcher that returns the body-thunk
   on a match or `nil` otherwise. `%receive` scans the mailbox in order, **removes
   and runs the first match, leaves non-matching messages queued** (true Erlang
   selective receive). A trailing `(after ms body...)` clause bounds the wait;
   `(after 0 …)` is a non-blocking poll. A green process waiting on a timeout is
   woken by a lazily-started **timer thread** (a `BinaryHeap` of `(deadline, pid)`)
   that re-queues it at the deadline; the root uses `cv.wait_timeout`. Stale timers
   are harmless — `%receive` always re-validates its own deadline. The
   single-consumer mailbox gains a `scanned` cursor so a parked selective receiver
   is only re-run when a *new* (unscanned) message arrives, not for ones it skipped.

   **Catchable timeouts, the Erlang way.** The `after` body runs inline like
   Erlang and, like any clause body, runs through the normal `apply`/`throw` path,
   so it composes with the existing `try`/`catch` (over `%try`). To *propagate* a
   timeout you `throw` from the body — `(after ms (throw [:timeout]))` — and catch
   it; convention is the structured value `[:timeout]`, paralleling `match`'s
   `[:match-error …]`. No separate throwing-timeout construct.

**Why.** Both deliver core capabilities the editor needs (a runaway command can't
freeze the runtime; request/reply and stateful server processes become writable)
by **composing existing machinery** — the yielder and the `match` compiler —
rather than adding language surface. Keeping `receive` a macro over one primitive
honours "as much in Brood as possible" (ADR-006/008) and "keep the core small"
(no new special form). Catchability falls out of the existing error model rather
than a new mechanism (ADR-011).

**Trade-offs accepted.** The per-iteration `tick` is a cost on the hottest path
(a thread-local decrement; benchmark, and if it ever bites, move the tick to the
tail-continue/apply points only — same correctness). Testing a `receive` candidate
rebuilds it into the LOCAL heap, so skipped messages leave short-lived garbage
(reclaimed at the next top-level arena reset, ADR-016) — negligible when the first
message matches. The timer thread is one extra OS thread, started only when a
timed `receive` is first used. `after` is reserved as a final-clause head.

## ADR-028 — Split the CLI: `brood` is the language, `nest` is the project tool

**Status:** accepted (2026-05-27).

**Context.** A single `brood` binary did two unrelated jobs: it *ran the
language* (`brood file.blsp`, REPL) and it was the *project tool* (`brood test`,
`brood new`, user config, scaffolding). These grow in different directions —
the language binary should stay a thin, stable runtime; the project tool will
accrete `build`/`check`/`add`/release commands and eventually the editor's dev
environment. Bolting all of that onto the language entry point conflates two
audiences (run-a-program vs. manage-a-project) and bloats the surface every
language user sees.

**Decision.** Two binaries, the `rustc`/`cargo` (and `elixir`/`mix`) split:

- **`brood`** (`crates/cli`) — the *language* only: `brood <file>`, the REPL,
  `brood --version`, and `brood --test <file>…` (run one or more self-contained
  files as a single in-language suite). No project awareness.
- **`nest`** (`crates/nest`) — the *project tool*: `nest new <name>`,
  `nest test` (walk to `project.blsp`, discover `tests/**/*_test.blsp`, run the
  suite once), the user config, and future `build`/`check`/etc.

`brood --test <file>` (single-file) and `nest test` (project-wide discovery) are
deliberately different commands for different jobs, not aliases.

**`nest` embeds the lib, it does not shell out.** Both binaries depend on the
`brood` lib crate and drive `Interp` directly — no subprocess. (Cargo shells out
to rustc because rustc is not a library; our runtime *is* one, so embedding is
simpler and keeps a single process for the eventual hot-reload/editor story.)
`nest` stays a *thin Rust shell*: it evaluates bootstrap snippets
(`(require 'project) (load-config) (run-project-tests)`) and the policy —
templates, name checks, discovery — lives in `std/project.blsp` (ADR-006). The
small `report_error`/`parse_args` helpers are duplicated across the two bins
rather than coupled through a shared crate; they're tiny and stable.

**Consequences.** `make suite` and `crates/lisp/tests/suite.rs` use the project
runner unchanged (they call the Brood runner, not the binary). Install/uninstall
now cover both binaries. The user config dir stays `~/.config/brood/` — it's the
ecosystem's config, read by `nest`. Self-hosting the tool in Brood remains the
roadmap goal; this split just gives it its own front door first.

## ADR-029 — Module docstrings + `nest doc` (extract by load-and-introspect)

**Status:** accepted (2026-05-27).

**Context.** Function/macro docstrings already exist (ADR-025: a leading string
in a `fn`/`defn` body, stored on the closure, read via `(doc f)`). Two pieces
were missing: a way for a **module** to document itself, and a tool to **extract**
docs into readable output. The flat `provide`/`require` module model (ADR-019)
has no namespace, so nothing records which definitions belong to which module.

**Decision.**

- **Module doc = the file's first top-level form, when it is a bare string** —
  the file-level analogue of the function-docstring rule, no new special form
  (keeps the core small, ADR-011). It's a harmless no-op when the file is loaded;
  the tooling reads it from source.
- **`nest doc [module]` extracts by loading + introspecting**, not by parsing
  source. It snapshots `(global-names)`, loads the module, and the new names are
  what it defined — read back through the existing `(doc f)`/`(arglist f)`. The
  module docstring is read from source (`slurp` + `read-string`), since a leading
  string is discarded on load. Output is Markdown to stdout. Policy lives in
  `std/docs.blsp` (ADR-006); Rust adds only `slurp` (the read counterpart of
  `spit`) and sorts `(global-names)` for deterministic output.
- Documenting one module **loads its code**. That's acceptable for a one-shot CLI
  (as `nest test` already loads files), and is explicitly *not* what the
  continuously-running LSP does — it must never eval user code (`docs/lsp.md`).

**Consequences.** Attribution is load-order dependent: a module already loaded
before the snapshot yields an empty delta and can't be re-documented in the same
process (hence `docs` requires `project` lazily). Definitions that *shadow* a
prelude name, and names pulled in by a transitive `require`, are mis-attributed.
The accurate, order-independent fix is the static CST walk planned in
`docs/lsp.md`; the runtime path ships first because it reuses the canonical doc
machinery and needs almost no new Rust.

## ADR-030 — Maps are immutable values (insertion-ordered assoc vector)

**Context.** A general Lisp needs key→value data; `{ }` was reserved in the
reader but unimplemented. An earlier attempt stalled on the obvious tension:
a *mutable* hash map fights everything the runtime depends on — `Send`
per-process heaps, copy-on-send messages, the append-only shared `RUNTIME` code
region, the (coming) tracing GC that wants no write barriers — and it would
violate the language's core immutability rule (ADR-026). Hashing was the other
snag: keys live in the heap (string contents, list/vector structure), so a
`Hash` over a `Value` needs `&Heap`, which the standard-library `HashMap` API
won't give it.

**Decision.** A map is an **immutable value**, exactly like a vector: a new
`Value::Map` / `Tag::Map`, stored in a slab, deep-copied by `promote` (LOCAL →
shared RUNTIME), retagged by the prelude freeze, and copied across heaps by the
message path — no special-casing, no write barriers. Every operation
(`assoc`/`dissoc`) returns a **fresh** map; nothing mutates in place.

- **Representation:** an **insertion-ordered association vector**
  `Vec<(Value, Value)>`, with no duplicate keys (assoc replaces in place). Keys
  are compared by the existing structural `heap.equal`, which *sidesteps the
  hashing problem entirely* — any value is a valid key, and we never need a
  `Hash` over heap-resident data. O(n) lookup, but maps here are small
  (structured data, error values) and ADR-011 says ship the simple form first.
  It is swappable for a hash-array-mapped trie later **with no surface change**.
- **Semantics:** literals `{k v …}` evaluate their keys and values (like vector
  literals), last-wins on duplicate keys; insertion order is preserved for
  printing and `keys`/`vals`; map `=` is **order-independent** (same
  associations). `contains?` distinguishes a stored `nil` from absence.
- **Kernel vs. Brood:** Rust provides only the irreducible `map-*` primitives
  (`hash-map`, `map-get`, `map-assoc`, `map-dissoc`, `map-keys`, `map-vals`,
  `map-contains?`); the ergonomic surface — `get` (with default), variadic
  `assoc`/`dissoc`, `keys`/`vals`/`contains?`/`map?` — is Brood in
  `std/prelude.blsp` (ADR-006). `count`/`empty?` gained a map case.

**Consequences.** Immutability makes maps "free" to thread through the
concurrency/GC machinery (they're just another `Send` slab of `Copy` handles),
which is the opposite of the mutable-map dead end. The cost is O(n) per
operation and O(n²) for repeated `assoc` in a loop — the same trade-off ADR-026
already accepts for `cons`/`append`, with the same mitigation (a persistent
HAMT) available later behind the unchanged surface. Maps also unblock a
structured error value (a later refactor of `error.rs`).

## Deferred / open questions

- **Macro hygiene:** currently unhygienic `defmacro` + `gensym`; hygienic macros
  (e.g. `syntax-rules`) are possible future work.
- **Nested quasiquote:** not level-tracked in v0.1 (see spec §spec note); fine
  for ordinary macros, revisit if needed.
- **`car`/`cdr` vs `first`/`rest`:** both provided; `first`/`rest` are the
  documented default.
