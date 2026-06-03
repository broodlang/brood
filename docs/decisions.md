# Design decisions (ADR log)

An **ADR** is an *Architecture Decision Record* — a short, dated note capturing
one design choice and *why* we made it, so we don't accidentally relitigate
settled questions. Newest at the bottom.

## Index

This file holds the **in-force** ADRs. To jump to one, search for its
`## ADR-NNN` header; the per-entry **Status** line is the source of truth for
current state (the table below is a navigation aid, not a status report).

**Four superseded/reverted/rejected ADRs have been moved out** to keep this log
focused on current design — their full text (with retrospectives) lives in
[archive/decisions-superseded.md](archive/decisions-superseded.md): ADR-002
*(superseded by the tracing/copying GC)*, ADR-035 *(superseded/disabled)*,
ADR-039 *(reverted → ADR-044)*, ADR-057 *(rejected as scoped)*. They're still
listed (italicised) in the index below so the numbering stays complete.
**Still proposed, not built:** ADR-071 *(WASM extensions)*.

| ADR | Title |
|----:|-------|
| 001 | Implement the runtime in Rust (not C or Zig) |
| 002 | `Rc`/`RefCell` now, tracing GC later *(superseded — archived)* |
| 003 | Lists are cons cells; `[ ]` vectors are separate |
| 004 | Clojure-style truthiness and flat `cond` |
| 005 | v0.1 has zero external dependencies *(relaxed by ADR-014)* |
| 006 | As much of the language as possible lives in Brood |
| 007 | Brood is a Lisp-1 |
| 008 | Rust is a primitive kernel; the language is written in Brood |
| 009 | Clojure-style quasiquote; commas are whitespace |
| 010 | Code is cons-lists; vectors are a data type |
| 011 | Favor the simplest user-facing design; defer power features |
| 012 | A process-wide byte-counting allocator for memory introspection |
| 013 | A runtime's inner processes share live code; separate runtimes don't |
| 014 | Runtime crates are allowed when they remove real complexity |
| 015 | Share-safe, parallel-by-default test framework |
| 016 | Arena-reset reclamation at top-level boundaries (first GC step) |
| 017 | Isolated tests roll back the globals via a private copy (`%isolate`) |
| 018 | Green M:N scheduler via stackful coroutines (step 4b) |
| 019 | Emacs-flat modules: `provide`/`require`/`load-path` (pre-namespaces) |
| 020 | Project model: `project.blsp` + a discovery-based test runner |
| 021 | Pattern matching: one Brood compiler, reused at every binding site |
| 022 | A macroexpand-all compile pass (expand once at definition) |
| 023 | First-class type tags; types stay runtime, checking stays advisory |
| 024 | Set-theoretic, gradual types: the model and the compatibility contract |
| 025 | A lossless, span-carrying CST for tooling, separate from the eval `Value` |
| 026 | Immutability: data is immutable; `def` is the only mutation (no `set!`/`while`) |
| 027 | Reduction-counted preemption + selective `receive` with timeouts |
| 028 | Split the CLI: `brood` is the language, `nest` is the project tool |
| 029 | Module docstrings + `nest doc` (extract by load-and-introspect) |
| 030 | Maps are immutable values (insertion-ordered assoc vector) |
| 031 | Cross-file xref is an image query, not a static index |
| 032 | Dynamic variables: a per-process binding stack, declared with `defdyn` |
| 033 | `spawn` takes an expression; closures are sendable as data |
| 034 | Distributed nodes (slice 1): node-tagged pids + a TCP link |
| 035 | Tracing GC: per-process mark-sweep at the outermost-eval safepoint *(superseded — archived)* |
| 036 | `nest mcp`: a per-project Model Context Protocol server |
| 037 | Packages: git deps + project-local cache + lock file |
| 038 | Single-binary bundling (`nest release`) |
| 039 | Supervised processes with mode-gated resume checkpoints *(reverted → ADR-044 — archived)* |
| 040 | Maps: CHAMP (16-way) instead of an entries-vec + index |
| 041 | Shared, refcounted blobs for large immutable byte data |
| 042 | Live-editing hardening: `defonce`, reload-defs detection, dedup, macro-staleness |
| 043 | Runaway-resource backstops: memory limits (E0043) + eval-depth ceiling (E0044) |
| 044 | Supervision is a userland Brood library, not a kernel feature |
| 045 | Text ropes as an opaque, immutable heap value (`Value::Rope`) |
| 046 | The display/input seam: a frontend is a protocol of render-op data |
| 047 | Native multi-arity closure dispatch |
| 048 | Self-hosted REPL (the read-eval-print loop in Brood) |
| 049 | Reader `INCOMPLETE_INPUT` as the multi-line continuation signal |
| 050 | Randomness is a pure, threaded PRNG (bitwise ops the only new primitives) |
| 051 | `(process-info pid)` as the kernel introspection snapshot |
| 052 | Interactive REPL line editor in Brood (inline `term-*` seam) |
| 053 | Remote attach: observe a running runtime over the node link |
| 054 | Generational handles: a debug tripwire for use-after-GC |
| 055 | Stage B: automatic copying collection at the eval safepoint |
| 056 | A windowed (GUI) frontend + mouse input, on the same display seam |
| 057 | Lexical addressing: O(1) variable lookup *(rejected as scoped — archived)* |
| 058 | Automatic GC reaches every entry path; `(hibernate)` removed |
| 059 | Blocking work delivers to a mailbox; it never pins a worker |
| 060 | Sets are a library over maps; the `#{…}` literal is deferred |
| 061 | Collect at any eval depth via an operand stack |
| 062 | TCP sockets: thin kernel, mailbox-delivered, over a reusable IO seam |
| 063 | `(exit pid reason)`: Erlang-style process termination |
| 064 | Rust primitives are single-shot w.r.t. eval re-entry |
| 065 | Namespaces: expand-time resolution over the flat table, soft privacy |
| 066 | Auto-gensym (`x#`): opt-in macro binding hygiene |
| 067 | Process links + `trap_exit` (the supervisor's structural orphan fix) |
| 068 | Node-connect ergonomics: default-cookie file, name-addressed Unix transport |
| 069 | Evaluator dispatch performance: cache the analysis, not the behaviour |
| 070 | Namespace-name collisions: detect-and-reject, not mandatory prefixes |
| 071 | Native extensions are WASM components, built on fetch and wrapped in Brood *(proposed)* |
| 072 | Stage C: a generational nursery + tenured old generation |
| 073 | Node names are `name@host` (Erlang short/long names) |
| 074 | Dual-listen: one node, several transports (`node-also-listen`) |
| 075 | Undo lives in the buffer value (per-buffer undo/redo stacks) |
| 076 | The execution engine becomes a closure-compiling VM (now the default) |
| 077 | Mouse `:drag` and `:release`, at cell granularity |
| 078 | Structured types: arrow + element refinements on the flat lattice |
| 079 | Per-op font scale on the GUI `Face` |
| 080 | Cursor zones: pointer-shape hints carried by the frame |
| 081 | Node-link security: pre-auth DoS hardening + authenticated-encrypted channel |
| 082 | Opt-in type annotations & runtime contracts (`sig`/`sig!`) |
| 083 | Output ports (`*out*`/`*err*`) and an async, safe logger |
| 084 | Quasiquote is a compile/eval-time code transform, not a runtime walker |
| 085 | `std/` is the basic-language core; frameworks are packages; hierarchical names |
| 086 | GUI keys are press/release transitions, not an OS-repeat flood |
| 087 | Expose O(1) kernel facts (`map-count`) as primitives |

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

---

## ADR-031 — Cross-file xref is an image query, not a static index: record def sites at load time

**Status:** accepted (direction); not yet implemented. Foundation primitive
(`source-location`) is the first step. Extends [`lsp.md`](lsp.md) §Cross-file;
builds on the CST decision (ADR-025) and the shared-code / hot-reload model
(ADR-013, [`shared-code.md`](shared-code.md)).

**Context.** Tier-1 `brood-lsp` (ADR-025) is **single-file**: it knows names from
the open buffer's CST and from the interpreter's globals — which are the *prelude
+ Rust builtins only*, because the server **never evaluates the buffer** (a
half-typed file can't be run: side effects, non-termination). So a name another
module `provide`s resolves as `Free` — no goto, no hover. The obvious next step
looked like the **rust-analyzer model**: statically walk the `require` graph off
`*load-path*` (ADR-019/020) and index every file's `def`s. But that makes the
tool an outside observer forever *re-deriving* what the program means, and it
can't see through macros.

Brood is the wrong shape for that model. It is an **image-based, self-editing,
hot-reloadable** Lisp (ADR-013): the running runtime already holds every loaded
module's globals in one shared, mutable code region (`global-names` enumerates
them today). The endgame (M2–M5) is *an editor that is a running Brood image
editing Brood source* — at which point the editor literally is the image and
"xref" is self-reflection. The idiomatic answer is the **SLIME/CIDER/Emacs-xref
model**: the image recorded *where each thing was defined as it loaded*, and
`M-.` is a hash lookup against it, not a re-analysis. The only missing piece is
that the global table doesn't record a definition's birthplace — `Closure` has
`name` and `doc` but no source location, and `form_pos` (top-level form starts)
is LOCAL-only, line/col, and reset on arena reclamation.

**Decision.** Cross-file navigation is answered by **querying the live image**,
not by a parallel static indexer.

1. **Record def sites at load/`def` time.** When a global is defined, store
   `name → (file, span)` into the **runtime's** code region (`RuntimeCode`, the
   shared, mutable, hot-reloadable one — so a redefinition updates it and spawned
   processes see it, consistent with ADR-013). `file` comes from the existing
   `current-file`; `span` from the form's recorded position. This is span-accurate
   for definitions *through macros*, because the site is captured at read/`def`
   time, before macroexpansion (ADR-022) discards spans.
2. **Expose one primitive:** `(source-location 'foo) → (file . span)` (or `nil`).
   Mechanism in Rust; any policy on top is Brood (ADR-006). Useful standalone —
   better runtime-error provenance, `nest`, a self-hosted REPL `M-.` — independent
   of the LSP.
3. **The server stays a hybrid, not a replacement:**
   - the **live buffer** (half-typed, what you're editing) → CST + scope walker
     (ADR-025), span-accurate for the file in front of you;
   - **everything loaded** (other modules, prelude) → image lookup. A name that
     resolves `Free` locally falls back to `source-location`, yielding a
     cross-file goto/hover (LSP `Location` already carries a target `Uri`).
4. **Definitions go image-based; references stay static.** "Find references"
   through macro-generated code has no faithful spans, so it remains CST-level
   source occurrences aggregated across files (`scope::references` per file).
   "Go to definition" becomes a name→site lookup. This is also where SLIME lands.

**Why.** The image is the only source of truth that is *already correct* about
cross-file names and macro-expanded defs; a static indexer can only approximate
it. Investing in def-site recording pays off the eventual self-hosted editor
directly (it needs exactly this), whereas a static workspace-index is throwaway
scaffolding. It is additive: nothing in Tier-1 changes, and `source-location`
earns its keep before any LSP wiring consumes it.

**Trade-offs accepted.**
- **Needs a loaded image.** Cross-file answers require the project to have been
  *run* (top-level side effects on load) — the very line ADR-025 drew at Tier
  0–1. SLIME accepts this (you start a Lisp and load your system); Brood's nature
  leans the same way. The LSP will either own a project image it loads explicitly,
  or talk to a running one — a deliberate, opt-in step, gated so the safe
  single-file features never depend on it.
- **Staleness.** After editing a file you haven't reloaded, the image is stale
  until that `def` is re-evaluated (SLIME's `C-c C-c` workflow). The CST always
  covers the *current* buffer, so staleness mostly bites cross-file lookups.
- **References don't see into macros** — the same caveat ADR-025/`tooling.md`
  already accept.

**Considered & rejected.** A purely static workspace-indexer (walk `require`,
parse every file's CST, never run anything). Safe and image-free, but it
permanently re-derives what the running image already knows, can't follow
computed/conditional `require`s, and is discarded once the self-hosted editor
makes the image authoritative. Kept only as the *fallback* shape if an image is
unavailable (e.g. a project that won't load) — not the primary path.

## ADR-032 — Dynamic variables: a per-process binding stack, declared with `defdyn`

**Status:** accepted.

**Context.** Brood needs Lisp "special variables" — globals temporarily
overridable for a dynamic extent (a print depth, a current sink) that deep
callees read without threading the value through every call. The constraints are
sharp: the language is immutable (ADR-026, so no mutable cell holds the current
value) and concurrent (green processes that migrate between worker threads, so a
Rust thread-local can't hold the binding), and the core should stay small
(ADR-011 — prefer a macro over a primitive over a new special form).

**Decision.**
- **A per-process binding stack lives in the `Heap`.** Each `binding` pushes its
  `(symbol, value)` pairs and pops them when the body returns. Reads consult it
  in `env_get` *at the `EnvId::GLOBAL` step only, and only when the stack is
  non-empty* — so the ordinary lookup path is unchanged, and a dynamic var
  shadows exactly where it resolves (it's never lexically bound).
- **Per-process, not inherited.** Because the stack is in the process's own heap,
  a `binding` is invisible to other processes and a `spawn`ed child starts from
  the declared defaults. This is the right default under share-nothing (data
  isn't shared, so neither is dynamic scope) and means a crash mid-`binding`
  drops the stack with the heap, disturbing no one. (Clojure-style binding
  *conveyance* across threads can be added later as opt-in if a need appears.)
- **Declared, not implicit.** `defdyn` marks the symbol dynamic in a process-wide
  `static` registry (a monotonic declaration fact, like the symbol interner — not
  per-runtime state) and `def`s its default. `binding` rejects a var that wasn't
  declared (almost always a typo; silently shadowing a plain global would
  mislead). `dynamic?` reports the mark.
- **Macros over a tiny kernel, no new special form.** Kernel: `%declare-dynamic`,
  `%binding` (push → `apply` thunk → pop, restoring on `Err` too — the `%isolate`
  shape), `dynamic?`. Surface: the `defdyn`/`binding` macros in the prelude. This
  follows the `try`/`catch` precedent (ADR-011) and keeps the evaluator's special
  forms untouched.

**Why.** Restoration-on-unwind and per-process isolation fall out of the design
rather than needing extra machinery; the read path stays free when no `binding`
is active; and `binding` mutating its stack is *binding* mutation (like `def`),
never data mutation, so the immutability and GC invariants (no write barriers)
hold. The whole feature adds three primitives and two macros — the last open
Tier-1 language gap, closed without growing the core.

**`let` stays lexical.** Resolution consults the dynamic stack only at the
global-lookup step, *after* the lexical frame chain — so a `let`/`fn` binding of a
dynamic var's name is an ordinary lexical shadow, and `binding` is the only form
that binds dynamically. This follows Clojure (lexical `let`, explicit `binding`),
not Common Lisp (where `let` on a `special` var binds dynamically). The CL route
would couple the `let` special form to the dynamic registry for no real gain; the
cost is that `let`-binding an earmuffed name hides a later `binding` of it (a
documented convention: don't — see `docs/language.md`).

**Considered & rejected.**
- *Undeclared `binding` (rebind any global).* Smallest kernel, but `defdyn`
  becomes a pointless alias for `def` and a typo'd `binding` silently "works".
  Declaration is cheap and catches the bug.
- *Temporarily rebinding the shared global table.* Globals are shared across a
  runtime's processes (ADR-013/014), so this would make one process's `binding`
  clobber another's — wrong for concurrency, and it fights hot-reload.
- *A Rust thread-local stack.* Breaks the moment a coroutine migrates workers or
  suspends at `receive`; the binding must travel with the process, i.e. its heap.

## ADR-033 — `spawn` takes an expression; closures are sendable as data

**Decision.** Two coupled changes that together let a *computation* be spawned and
shipped to another node:

1. **`spawn` takes one unevaluated expression**, not a function + args. `(spawn e)`
   is a prelude macro expanding to `(%spawn (fn () e))` — the `try`/`%try` pattern
   (ADR-011: a macro over a primitive, no new special form). The Rust kernel keeps
   only `%spawn`, which runs a 0-arg thunk. `(spawn (* (+ 1 1)))` and
   `(spawn (worker me))` both read naturally, and the thunk **captures free locals
   lexically** instead of taking them as positional args.

2. **A closure serialises into a `Message`** (reversing the old "you can't send a
   function"). A closure's body and its optionals' defaults are *S-expression forms*
   — plain data — so they travel as ordinary messages; the **free locals it actually
   references** are copied (only those — not the whole lexical frame, so unrelated
   siblings don't ride along and a closure capturing a sibling closure can't form a
   serialisation cycle); and its **free globals are not copied at all** — they
   re-resolve on the receiver against that runtime's own global table. So a closure
   runs on any node that has the same definitions (Erlang's "the module must be loaded
   on both nodes"). A self-referential *local* closure can't be sent (define it at top
   level — global recursion resolves by name, captures nothing).

**Why.** The project's reason to exist is a self-editing, remotely-hostable editor;
"run this computation over there" is the primitive that makes the remote half real.
Homoiconicity makes it nearly free: code *is* data, so a `(spawn e)` thunk is already
serialisable once we copy its captured environment. Spawning an expression (not a
pre-built fn) is also the more general, more Lisp-like surface — the fn-and-args form
was a strictly weaker special case.

**Consequences.**
- **`(self)` moved.** It used to be evaluated in the parent (`(spawn worker (self))`);
  now the body runs in the child, so `(self)` *inside* `spawn` is the child's pid.
  Capture the parent's first: `(let (me (self)) (spawn (worker me)))`. Every callsite
  updated to match.
- **A sent closure is a frozen copy.** Redefining *that* function later doesn't reach
  an already-sent copy; globals it *references* still hot-reload (ADR-013). Correct
  for cross-node, where there's no shared code region to track.
- **Builtins still can't be sent** (a Rust fn pointer has no portable form); reference
  one by the symbol naming it. **Macros aren't sendable** either (deferred; no need yet).
- **Local spawn is unchanged in cost:** it still `promote`s the thunk into the shared
  RUNTIME region (O(1), hot-reloadable) rather than serialising — serialisation is the
  *node* path, exercised locally by `send`ing a closure between processes.

**Scope.** This ADR covers the language surface (sendable closures + spawn-the-expr).
**Node identity and the wire transport** — node-tagged pids (`Value::Pid { node, id }`),
the codec that re-encodes a node `Symbol` by name across interners, and `send` dispatch
across a link — live in `crate::dist` and are decided separately.

**Considered & rejected.**
- *Ship the unevaluated form and `eval` it remotely (code-as-data only).* Simpler —
  the form is already messageable — but it gives no lexical capture: `(spawn (f x))`
  couldn't see a local `x` without quasiquote-splicing. Real closures subsume it.
- *Keep `(spawn f arg...)`.* Can't express `(spawn (* (+ 1 1)))` without a wrapper, and
  args-as-data is just the no-capture special case of a captured thunk.

---

## ADR-034 — Distributed nodes (slice 1): node-tagged pids + a TCP link

**Status:** accepted. Realises the node identity + wire transport that ADR-033
deferred; implements the §Distribution sketch in `concurrency.md`. See
`docs/distribution.md` for the full design.

**Context.** Two runtimes must be able to connect and message each other — the
foundation of the project's "backend hosted remotely by a frontend" premise (M4).
Erlang showed the shape: share-nothing + copy-on-send means *the network is just a
longer copy*. The question was how much to build now and how pids should carry
location.

**Decision.** The smallest useful slice (ADR-011):

1. **Pids are a first-class value carrying node identity** — `Value::Pid { node,
   id }` (a `Tag::Pid`), replacing bare-`Int` pids everywhere. `self`/`spawn`
   return one; it prints `#<pid node/id>`. A *local* pid carries this node's name,
   a *remote* one the peer's, so **the same value addresses a process anywhere** —
   `send` dispatches on the node part (local → in-process `deliver`; remote → over
   the link). Before `node-start`, the node is `:nonode` (always local).

2. **An authenticated TCP link.** `(node-start name "host:port" cookie)` names the
   runtime and listens; `(connect "name@host:port")` dials. Both sides exchange a
   `Hello` and check a **shared cookie** (Erlang-style — *not* real security;
   placeholder for auth/TLS). Each connection runs two plain OS threads (reader +
   writer), entirely off the green-process scheduler; an inbound message lands in
   a local mailbox via the same `deliver` an in-process `send` uses.

3. **Bootstrap by registered name.** `(register name pid)` binds a local name;
   a peer reaches it with a `{:name name :node node}` address before it holds any
   pid. The first reply carries `(self)` as a pid, and every later `send` targets
   that **remote pid** directly — location-transparency.

4. **Hand-rolled, length-prefixed wire codec** reusing `Message`'s deep-copy, with
   one cross-process detail: **symbols (incl. a pid's node, keywords) travel by
   name and re-intern on arrival**, because separate runtimes have independent
   interners. No new dependency (std `net` + threads; ADR-014).

**Why a value, not an int.** Routing off-node needs location *on the handle*, and
making local and remote pids the same kind of value keeps `send` uniform — you
never special-case "is this remote?" at the call site. Pids are used opaquely in
Brood (send targets, message payloads, `[:down …]`), so the change is mechanical.

**Scope / deferred.** One node per OS process (node identity + tables + interner
are process-global). The original "deferred to later slices" set has now
landed, in increments tracked in `docs/distribution.md`:

- **Node-down detection** (slice 2) — heartbeat ping/pong + generation-checked
  teardown; `[:nodedown name]` to `monitor-node` watchers.
- **Closure-as-data path from ADR-033** — `M_CLOSURE` wire codec ships every
  `ClosureMsg` field; source positions ride along via `Message::List`'s
  optional `Pos` trailer; `(remote-spawn node expr)` (Brood macro) is the
  surface convenience over the `[:run f x reply]` pattern.
- **Distributed pid monitors** — `(monitor remote-pid)` routes through a
  `Frame::Monitor` to the peer, which reuses the **same** `process::add_monitor`
  core and `MONITORS` table the local monitor uses (one `Watcher` enum with
  `Local` / `Remote` variants — no parallel implementation). Net-split fires
  `[:down mref pid :noconnection]` via a sender-side `PENDING_REMOTE` table
  and `handle_node_down`.
- **Auto-reconnect** — `(ensure-link "name@host:port")` (Brood policy in
  `std/prelude.blsp`) maintains a peer link across restarts: synchronous
  initial connect, supervisor watches via `monitor-node`, retries on each
  `[:nodedown …]` with a 200ms backoff until success.
- **Handshake v2 (real auth)** — 4-byte magic+version prefix (`b"BRD\x02"`),
  nonce-based `Hello`s, HMAC-SHA256 `Auth` frames. The cookie is **never on
  the wire** — it's an HMAC key, so an eavesdropper can't replay either it
  or a captured handshake. A non-brood peer / wrong cookie aborts before the
  link enters `NODES`. Uses the RustCrypto `hmac` + `sha2` crates (the
  "don't roll your own crypto" exception to ADR-005); nonces come from
  `getrandom` (OS RNG). Wire format break from v1, deliberate (greenfield).

**Still deferred.** Erlang OTP-style **supervision trees** with `link` +
restart strategies (today's `monitor` is unidirectional and one-shot — useful,
but not the full OTP guarantee). Optional **TLS** as a transport substrate
*under* the HMAC layer, for over-the-internet links (HMAC alone proves
shared-cookie possession but doesn't encrypt traffic).

## ADR-036 — `nest mcp`: a per-project Model Context Protocol server, tools surface in Brood

**Status:** proposed (2026-05-28). Design recorded in [`mcp.md`](mcp.md).

**Context.** Brood has a Tier-1 language server (`brood-lsp`, ADR-025) that
gives editors hover/completion/diagnostics/goto-def/signature-help on the
buffer under a cursor. But an *AI agent* doing development against the project
asks different questions than an editor: not "what is at this offset?", but
"eval this", "run that test", "expand this macro", "what is `map`'s arglist".
Routing those through the LSP requires a buffer and a cursor; through the
shell, parsing GNU-line output per request. Both miss the thing this Lisp
already does well — hot reload (ADR-013, `docs/shared-code.md`): the running
runtime is the project, `def` mutates it in place, and running processes see
the new binding on the next lookup. That makes a *long-lived per-session image*
the natural shape for agent-driven work, the same way SLIME/CIDER are for
humans. The Model Context Protocol (MCP, JSON-RPC over stdio, the same shape
as LSP) is the standard agent surface — Claude Code attaches MCP servers per
workspace via `.mcp.json` — so the question is just what to expose and where it
lives in the tree.

**Decision.** Add **`nest mcp`** — a subcommand on the project tool (ADR-028)
that speaks MCP over stdio, scoped strictly to the project rooted at cwd.
Outside a project root it errors loudly; there is no "language-only" MCP
flavour, matching the `nest test` / `nest doc` shape rather than `brood
file.blsp`. Concretely:

- **One `Interp` per MCP session, long-lived across tool calls.** State *is*
  the feature: a `def` in one `eval` call is visible to the next and to any
  green process spawned in between. Two `claude` sessions over the same project
  get two `nest mcp` processes, each with its own image — no cross-session
  sharing.
- **A shared introspection layer.** Pull the existing
  `crates/lsp/src/introspect.rs` (`global_names` / `signature` /
  `arglist_tokens`) up to `crates/lisp/src/introspect.rs` and widen it with the
  operations both surfaces need (`source_location`, `macroexpand_to_string`,
  `check_project`, `run_tests`, `format_source`, `eval_in_session`). LSP and
  MCP each become genuinely thin shells over it, so hover and `lookup` cannot
  drift on what `map`'s signature is.
- **The tool *surface* is declared in Brood**, not Rust (ADR-006). The Rust
  side is a JSON-RPC dispatcher; `std/mcp.blsp` lists the tools (name, JSON
  schema, handler fn) and each handler is Brood. A project's own `mcp.blsp`
  can extend the catalogue — registering a project-specific verb is a `defn`,
  not a new Rust release. The initial set (ADR-011, ship the simple shape) is
  eight tools — `eval`, `load`, `lookup`, `macroexpand`, `run-tests`, `check`,
  `format`, `processes` — plus resources for the docs (`brood-for-claude`,
  `language`, `decisions`, `types`), the prelude, and the project manifest.
- **Transport: a sync JSON-RPC loop we own**, the same shape `lsp-server` gives
  the LSP. MCP's surface is small (initialize, tools/{list,call},
  resources/{list,read}, prompts/{list,get}); a direct implementation stays
  under a few hundred lines, avoids an async runtime, and matches the `!Sync`
  `Heap` constraint (one `Interp`, one request at a time, no `tokio`). Same
  calculus as ADR-025 picking `lsp-server` over `tower-lsp`.
- **Scaffold the attach config.** `nest new foo` drops `foo/.mcp.json` pointing
  at `nest mcp`, so `cd foo && claude` auto-attaches. Combined with the
  `%builtin-doc`-baked `brood-for-claude.md` (commit `d650bcb`, also exposed as
  an MCP resource), a freshly scaffolded project is ready for agent-assisted
  development from its first commit.

**Why.** Three forces line up:

1. **ADR-006 — write the language in the language.** Rust supplies transport
   and dispatch; *what tools exist and what they do* is Brood. This is the only
   architecture that lets a project extend its own agent surface without
   forking the binary.
2. **ADR-028 — nest is the project tool.** MCP is project-shaped: per-project
   image, per-project tests, per-project extensions. It belongs in `nest`. A
   "raw language" MCP would just be a REPL behind JSON-RPC — that's what
   `brood` is.
3. **Hot reload is the agent fit.** The same property that makes Brood a good
   editor language — `def` is the only mutation, and it propagates to running
   processes — makes it a good *agent* language: the agent iterates the way a
   Lisper iterates, not the way a Rust dev iterates.

**Trade-offs accepted.**

- **`eval` is arbitrary code execution.** Local, single-session, behind the
  user's own `.mcp.json` it's the same authority as Bash from Claude Code —
  acceptable. Network/multi-tenant exposure would need a `:safe` allowlist; out
  of scope here.
- **One `Interp` per connection, no sharing.** `Heap` is `!Sync`; sharing
  would force a redesign we don't want. Two parallel sessions on a single
  image (an agent and a human REPL at once) is explicitly not a goal yet.
- **Per-project only.** Outside `project.blsp`, `nest mcp` errors. Considered a
  language-only mode and rejected: every nontrivial tool wants project context
  (tests, sources, `mcp.blsp` extensions), and the LSP's project-aware
  bootstrap already proved the shape.
- **Drift risk with the LSP** if the shared `brood::introspect` extraction is
  half-done — the LSP must move onto it as part of the same change, not after.

**Consequences.** `crates/lsp/src/introspect.rs` moves to the lib crate as
`brood::introspect` and the LSP consumes it from there. `crates/nest/` grows
an `mcp.rs` module (promote to a `crates/mcp/` lib only when something else
needs to embed it — the move is mechanical). `std/mcp.blsp` is a new module
the dispatcher loads at startup. `nest new` templates gain a `.mcp.json`.
The editor work later (M2/M3) inherits the same dispatcher — when the editor
is itself a Brood image, `nest mcp` becomes a long-running thread inside it,
no protocol change.


## ADR-037 — Packages: git deps + project-local cache + lock file

**Status:** **accepted / implemented** (v1 scope complete 2026-05-30; proposed
2026-05-28). Design recorded in [`packages.md`](packages.md).

**Context.** The module system (ADR-019) resolves `(require 'foo)` by walking
`*load-path*`, with embedded std modules baked into the binary. That's enough
for a single project (`src/` is on `*load-path*` automatically — ADR-020) and
for the stdlib (embedded via `%builtin-module`). It is **not** enough for
third-party Brood code: there's no way to declare "this project depends on
`parser` version *X*", no place for that code to live, no way to reproduce a
build. As soon as the editor (M2+) starts inviting plugins / modes /
syntax-highlighters, the absence of a package story stops a real ecosystem
from forming.

The choices that defined the ecosystem-shape of every language with a
package manager — central registry vs. URL imports, SAT-solver constraints
vs. pinned refs, project-local vs. global cache — are baked in once and hard
to walk back. Better to pick early, ship the simplest thing that fits the
project's grain, and grow from real pressure rather than speculation.

**Decision.** A **git-deps + project-local cache + lock file** package manager,
designed around the project's existing constraints — language-as-policy
(ADR-006), `nest` as the project tool (ADR-028), `project.blsp` as the
manifest (ADR-020), Brood's module system (ADR-019). The decisions, in
order from most to least committed:

- **Manifest extension.** `project.blsp` gains an optional `:dependencies`
  vector. Each entry is `[name :git URL :ref REF]` or `[name :path PATH]`
  — the local name (the symbol `require` will see), a source kind, and
  source-specific opts. No registry name resolution: **the source URL *is*
  the package identity**. Go's `name = URL` model — pre-1.0-friendly,
  no central infrastructure, no registry to host or pay for.
- **Project-local cache.** Fetched deps live in `_deps/<name>/` under the
  project root (gitignored). One copy per project, no global cache — keeps
  each project hermetic and avoids the "did `cargo clean` clobber something
  I needed" class of issue. Disk is cheap; correctness is not.
- **Lock file.** `nest fetch` writes `project.lock.blsp` with the resolved
  commit, the SHA-256 of the working-tree tarball, and the dep's own
  transitive `:dependencies`. Re-running `fetch` is a no-op unless the
  manifest or a `--update` flag asks otherwise. Reproducible builds without
  inventing a binary lock format — the lock file is just Brood data, read
  by the same reader/printer everything else uses.
- **`*load-path*` integration.** `nest fetch` (and any `nest test`/`run`/
  `check` that triggers an implicit fetch) extends `*load-path*` to include
  each `_deps/<name>/src/`. The existing `(require 'foo)` machinery resolves
  through that — *no change to the require semantics or surface*. Packages
  are just code on the load path.
- **No constraint solver — direct refs only.** Each dep pins an exact Git
  ref (tag or commit). Two deps requiring different versions of the same
  package is a **loud error** at `nest fetch` time; the user resolves it by
  pinning explicitly in their root manifest. No SAT solver, no MVS, no
  semver matching. The pain point this avoids is real and a recurring time
  sink in other ecosystems; the cost is the user has to think about
  conflicts manually until v2 (when, if it comes, an explicit resolver
  gets designed against real data).
- **No install scripts.** Packages are pure Brood source. Loading one runs
  its `(provide …)` / top-level forms via the normal evaluator — no
  privileged install-time hook, no `package.json`-style `postinstall`. A
  package that wants to ship native code does it the standard Rust way (a
  separate `cargo` crate, distributed via crates.io); the Brood side just
  `require`s a wrapper. The npm-style supply-chain attack surface stays
  closed by construction.
- **Policy in Brood (`std/package.blsp`), mechanism in Rust.** The fetch
  primitives are small: `%git-clone url dest ref` (shell out to `git`),
  `%sha256-file path`, `%http-get url` (for future tarball deps —
  primitive added now, used later). Manifest parsing, lock-file format,
  cache layout, conflict detection, transitive resolution — all Brood.
  Standard pattern (ADR-006/008).
- **Subcommand surface on `nest`.** `nest fetch` / `nest update [<name>]` /
  `nest add <name> <source> [opts]` / `nest remove <name>` / `nest tree`.
  All Brood entry points dispatched from the existing `nest` Rust shell
  (ADR-028). Existing subcommands (`test`, `run`, `check`, `format`, `doc`,
  `mcp`) auto-fetch missing deps on first run.

**Why.** Five forces line up:

1. **The simplest thing that could possibly work.** Go's "URL = name" model
   ships a working package manager in a weekend. Cargo's design is excellent
   but borderline-impossible to fit in scope; Hex/Mix needs a registry; npm
   needs a registry *and* unsolvable security work. Git deps + lock file
   gets 90% of the value for 5% of the engineering.
2. **ADR-006 — write the language in the language.** The package manager is
   exactly the kind of policy that should be Brood. The only Rust the design
   adds is "shell out to git" + "compute a SHA-256" — primitives the editor
   will want for unrelated reasons anyway.
3. **ADR-011 — ship the simple form, defer the powerful one.** No constraint
   solver, no registry, no signing — each adds knobs forever. Add when a
   concrete pain shows up.
4. **The editor wants this.** M2+ introduces user-extensible pieces (modes,
   syntax highlighters, keymaps). "How does a plugin arrive in my editor?"
   has to have an answer before the editor lands; a package system that
   already works for ordinary Brood code drops in naturally as the plugin
   loader.
5. **It changes project management — once.** The `_deps/` directory,
   `project.lock.blsp`, the auto-fetch behaviour, the load-path extension —
   they all affect how `nest test` / `nest run` / `nest check` work. Better
   to design them in early than retrofit. (Specifically, this is why we're
   landing the design *before* M2: the editor work shouldn't define its own
   one-off plugin loader.)

**Scope / deferred.**

- **Registry, semver, constraint solving** — deferred. Direct git refs are
  enough until a concrete need emerges.
- **Tarball / HTTP deps** — deferred. `%http-get` lands now for future use;
  no `:tarball` source kind in v1.
- **Signed packages** — deferred. SHA-256 in the lock file gives bit-for-bit
  reproducibility; trust still flows from "do you trust this URL". Signed
  packages need a key infrastructure that's its own problem.
- **Per-dep overrides** (`[:patch]`-style Cargo syntax) — deferred. A `:path`
  source on a dep already gives you "I want to hack on this dep locally".
- **A global / shared cache** — explicitly rejected for v1. Per-project
  `_deps/` is simpler and avoids the "is my install reproducible across
  machines" class of subtle bug. Cost: more disk usage. Acceptable.

**Open questions / answer-on-implementation.**

- *Where does the lockfile sit relative to the manifest?* Alongside in the
  project root, like Cargo. Committed to the user's repo.
- *How are vendored / mirrored deps modelled?* `:path` sources cover the
  internal-mirror case; a separate `:tarball-cache` flag could later cache
  HTTPS fetches in a local directory for offline builds.
- *Does the auto-checker walk dep source?* No, by default. Dep source is
  treated as stable (the package's author already passed `nest check`).
  Override: `nest check --include-deps`.

**Consequences.** `std/package.blsp` is a new module. `std/project.blsp`
grows a `:dependencies` clause in its `(project …)` form and an
`(ensure-deps)` step in `project-setup`. `nest`'s Rust shell gains
`fetch`/`update`/`add`/`remove`/`tree` subcommands (each a one-liner that
calls into `std/package.blsp`). The Rust kernel grows `%git-clone`,
`%sha256`, `%git-resolve-ref`, `%rm-rf` primitives (`%http-get` deferred with
tarball deps — refinement 5 below). `.gitignore`
templates from `nest new` get `_deps/` added. `nest mcp` gets a
`packages.list` tool surface later (separate ADR if needed). No change to
the require/load semantics — the existing module system is the runtime;
packages are just a source provisioner above it.

**Implementation refinements (2026-05-29).** Four decisions taken when the
build started, refining the original sketch (full rationale in
[`packages.md`](packages.md)):

1. **Hash primitive is `%sha256` over a *string*, not `%sha256-file` over a
   directory.** One irreducible primitive (hash a byte string → hex); the
   canonical tree walk + per-file `(%sha256 (slurp p))` + combine is Brood
   (`std/package.blsp`), over the existing `list-dir`/`dir?`/`slurp`. Smaller
   kernel, more in-language (ADR-006), and the same primitive hashes the lock
   manifest. Replaces `%sha256-file` in the original kernel list.
2. **`:path` deps load *in place*.** A path dep's `src/` goes straight onto
   `*load-path*`; it is **not** copied into `_deps/`. So `_deps/` only appears
   once git deps land — and edits to a path-dep's tree are live (the intended
   local-dev workflow). Path deps are still tree-hashed into the lock for
   change detection.
3. **`(project …)` is a quoting macro.** It treats its arguments as literal
   data (expands to `(project--apply '(…))`), so a manifest writes dep names
   and the `:main` pair as **bare symbols** — `[parser :git … :ref …]`, not
   `'[parser …]`. Manifests are pure static data; nothing in them is ever
   evaluated. *(Shipped 2026-05-29 with the `:dependencies` parser; the rest
   of these are Slice-1/2 commitments.)*
4. **Clone-then-checkout the resolved commit.** `git clone --depth 1 --branch
   <ref>` only accepts a branch/tag name, but the lock file always pins a
   commit SHA — so the sketch's `ensure_cache` clone would fail on a pinned
   dep. `%git-clone` instead clones the ref shallowly then checks out the exact
   commit (fetching it where the server allows).

Implementation landed in vertical slices (all done): **Slice 0** (2026-05-29) —
manifest `:dependencies` parsing + the `project` macro; **Slice 1** (2026-05-29)
— `:path` deps end-to-end (`%sha256`, tree hashing, lock-file I/O, `ensure-deps`
load-path integration), no git/network; **Slice 2** (2026-05-30) — `:git` deps
(`%git-resolve-ref`/`%git-clone`/`%rm-rf`, the `_deps/<name>/` cache + a
`.brood-pkg.blsp` stamp, lock commit-reuse on a cache hit, the direct-beats-
transitive conflict rule); **Slice 3** (2026-05-30) — the
`fetch`/`update`/`add`/`remove`/`tree` verbs + auto-fetch on every project-aware
subcommand.

**Further refinements taken at Slice 2 (2026-05-30):**

5. **`%http-get` deferred, not added-unused.** The original plan added it early
   "for future tarball deps." With no caller until the `:tarball` source kind
   (itself deferred), adding it now would be unused kernel surface — so per
   ADR-011 it's deferred *with* tarball deps. The git path needs only
   `%git-resolve-ref`/`%git-clone`/`%rm-rf`.
6. **Clone folded into resolution, not a separate `ensure_cache` pass.** The
   resolution sketch returned `deps: TBD` and filled it in a later `ensure_cache`.
   But the depth-first walk needs each git dep's own `:dependencies` *immediately*
   to queue them, and those live in the dep's `project.blsp` — which only exists
   after the clone. So `package--resolve-git` clones (on a cache miss) and reads
   sub-deps in the same step, mirroring `:path` resolution. A **cache hit** (the
   `.brood-pkg.blsp` stamp records the wanted commit) skips both the clone and the
   tree-hash and reuses the locked SHA — necessary because `ensure-deps` runs on
   every project-aware `nest` subcommand and must stay cheap.
7. **`nest update` = re-resolve with the lock dropped.** Rather than a `--update`
   flag threaded through resolution, `resolve-deps` takes the prior lock and
   `update` simply passes `nil` (all deps) or a lock with the named deps filtered
   out (those re-resolve; the rest keep their pins). Moving refs advance; the
   "network-free on a cache hit" property is just "the lock still matches."

## ADR-038 — Single-binary bundling (`nest release`)

**Status:** **implemented** (2026-05-31; proposed/deferred 2026-05-28). Built as
designed — append-to-binary. See [`release.md`](release.md) for the as-built
reference; the implementation note at the end of this ADR records what shipped.

**Context.** "Run my Brood app as one executable, no `brood` interpreter on
the host" matters for end-user distribution (the editor, eventually) but
adds no value to the project's current loop (CLI + tests + REPL on dev
machines that have `cargo`). The cheapest, most portable bundling approach
is **append-to-binary**: take the built `brood` executable, append a zip of
the project's source + `_deps/`, write a small magic-footer record, chmod
+x. The interpreter's `main` checks for the footer on its own path
(`/proc/self/exe` / `_NSGetExecutablePath` / `GetModuleFileNameW`) and, if
present, mounts the embedded archive and runs the project's `:main` instead
of the default REPL.

**Decision.** Land this when the editor's distribution story actually needs
it — likely late M3 or M4 (server / daemon mode). Two design points worth
recording so the eventual implementation isn't rediscovered:

- **Append-to-binary, not re-link.** Rebuilding via `cargo` on the user's
  machine works but takes a minute and needs the Rust toolchain installed.
  Appending a zip + footer to a pre-built binary takes milliseconds and
  needs nothing on the user's machine.
- **`nest bundle [--target <triple>]`** is the surface. Static linking for
  Linux uses `--target x86_64-unknown-linux-musl`; cross-compilation to
  macOS/Windows uses `cross` or a build host. Out of scope on the bundler
  side; the user provides a pre-built `brood` for the target.

**Why deferred.** Stage 1 has no end-user distribution; the editor
(M2/M3) is the first thing that does. Implementing it now would mean
maintaining a wire format that no real user exercises. Better to wait for
the editor's deployment shape to settle, then design once.

**What's already in our favour for when we land it.** The prelude is
already bundled via `include_str!`; `EMBEDDED_MODULES` is the established
pattern. `project.blsp` already declares the entry point (`:main`).
`(load …)` is the right hook for "load from inside the binary" — extend
to look in the embedded archive before falling through to disk.

**Implementation note (2026-05-31).** Shipped as **`nest release`**, append-to-binary
as designed, with two refinements from building it:

- **Surface is `nest release`** (not `nest bundle`) — it produces the release
  artifact. `nest release [-o PATH] [--runtime PATH] [--target TRIPLE]`.
- **Wire format** (`crates/lisp/src/bundle.rs`): `[brood][archive][20-byte footer]`,
  footer = magic `b"BRDBNDL1"` + `u32` version + `u64` archive-len, read
  last-bytes-first via `std::env::current_exe()` (not hand-rolled `/proc/self/exe`).
  The archive is a flat length-prefixed store of the manifest + each module's
  source keyed by **filename stem** — the exact name `require--find` searches for
  as `<stem>.blsp`, so an app's modules resolve through the *existing* require
  path with no load-path change.
- **The hook is `%builtin-module`, not `load`.** A mounted bundle is just *more
  embedded modules*: `builtin_module` consults the bundle after `EMBEDDED_MODULES`,
  so `require`/`:use` resolve an app's own modules (and bundled deps) transparently.
  Thin new primitives `%bundled?` / `%bundle-manifest` / `%bundle-module-names`
  expose the rest; boot policy is Brood (`project/run-bundle` + `bundle-collect`
  in `std/project.blsp`), per ADR-006.
- **Code-only + deps bundled.** v1 embeds `project.blsp` + `src/**/*.blsp` +
  resolved `_deps/` (so a `:path`/`:git`-dep app is self-contained); it does *not*
  virtualize the filesystem, so runtime asset reads (`(slurp "data.txt")`) still
  hit disk. A self-extracting **FS** is the obvious next increment if an app needs
  it. `tests/` is excluded.
- **Re-release is idempotent.** `nest release` strips an existing footer off the
  base before appending, so releasing from an already-released `brood` can't nest
  archives. macOS code-signing (appended bytes invalidate a signature) is a
  documented re-sign step; cross-targets supply a prebuilt `brood` via `--runtime`.
- **Lean runtime (2026-05-31 follow-on).** A release does *not* append to the dev
  `brood`. A `dev-tools` cargo feature (default on) gates the dev/debug surface;
  `nest release` builds a runtime with `--no-default-features` (cached under
  `target/release-lean/`, profile `release-lean` = strip + LTO + 1 codegen unit),
  so a shipped app carries **no** test framework, process observer, MCP/doc/
  hot-reload tooling, interactive REPL, or GC debug builtins — and they "never
  compile in" (the `include_str!`s are `#[cfg]`'d out, not runtime-hidden). Kept
  in CORE: the prelude, `project` (boots the bundle), and the UI/editor toolkit
  incl. `lineedit` (an editor's minibuffer reuses it). Net ~13 MB → ~6 MB. The
  runtime is built once; changing the app only re-appends the archive. This forced
  one structural fix: `project` no longer `(:use test)` at load (it `require`s +
  qualifies `test/` only inside the test runner), so a lean runtime can load
  `project` to boot a bundle without the test framework present.
- **Still a full evaluator.** A bundled binary keeps `load`/`slurp`/`require`/
  `eval-string` over the real filesystem and `def`-rebind hot reload, so a shipped
  app reads external `.blsp` (an editor's `init.blsp`: add layers/keymaps/modes,
  redefine commands) against the live runtime — only the stripped modules are
  unavailable to it.
- **No Rust at release time (2026-05-31 follow-on).** The lean runtime is built
  *once* at `make install` and **baked into `nest`** (`crates/nest/build.rs` reads
  `BROOD_EMBED_RUNTIME` and `include_bytes!`s it; `Makefile` builds it first).
  `nest release` appends the app to that embedded copy — pure file-ops, verified
  to run with an empty `PATH` (no cargo/rustc). A plain `cargo build` of `nest`
  embeds nothing and falls back to building the runtime from source. **One variant
  for now: lean + `gui`** (the embedded runtime includes the windowing backend
  when GUI is configured, so windowed apps just work; a non-gui app pays ~4 MB it
  doesn't use). A future opt-in terminal-only variant is the planned next step.
  The brief gui-feature *detection* that drove a per-app variant was removed in
  favour of the single embedded variant.
- **Cross-targets via a local runtime cache (2026-06-03 follow-on).** `--target
  TRIPLE` is now **repeatable and functional**: each triple resolves a prebuilt
  lean runtime from `$XDG_CACHE_HOME/brood/runtimes/<triple>/brood` (`~/.cache`
  fallback; `brood.exe` for Windows triples), which the user populates by
  building the lean runtime on/for each target once. The host's own triple
  (baked in as `NEST_HOST_TRIPLE` by `build.rs`) needs no cache entry — the
  embedded runtime serves it. Outputs get friendly suffixes (`app-macos-arm64`,
  `app-windows-x86_64.exe`; musl keeps the libc visible so a gnu+musl matrix
  can't collide), so one invocation emits a whole release matrix. Considered and
  rejected for now: *downloading* runtimes from GitHub releases (the Deno model
  — needs CI + published artifacts we don't have yet; the cache layout is
  exactly what such a fetcher would fill, so it layers on later without a
  breaking change) and *cross-compiling* on demand (Linux→macOS needs the Apple
  SDK; still out of scope). `--runtime PATH` stays as the explicit one-off
  escape hatch, valid with at most one `--target`.

## ADR-040 — Maps: CHAMP (16-way) instead of an entries-vec + index

**Status:** accepted, implemented 2026-05-29 (see devlog).

**Context.** ADR-030 shipped maps as insertion-ordered association vectors,
explicitly flagged "swappable for a hash-array-mapped trie later **with no
surface change**." That has now started to hurt: `assoc`/`dissoc` are O(n)
each because every op copies the whole entries vector (immutability — ADR-026
— forbids the in-place update Clojure's `transient!` uses), so `(fold assoc
{} (range N))` is O(n²). `get` is also O(n) on a linear `equal` scan. An
intermediate attempt — keep the vector, add a hash-keyed bucket index
alongside — moves lookup to O(1) but does nothing about build cost (the
index itself has to be cloned per assoc), and on Brood's current
small-to-medium map workloads the constant-factor regression (`HashMap::clone`
per op) outweighs the lookup win. The right move is to fix both at once with
structural sharing.

**Decision.** Replace the entries-vector representation with a **CHAMP** trie
(*Compressed Hash-Array Mapped Prefix-tree* — Steindorfer & Vinju, 2015).
Surface (`assoc`, `dissoc`, `get`, `contains?`, `keys`, `vals`, `map-pairs`,
order-independent `=`) is unchanged — the kernel API in `builtins.rs` and
every `std/prelude.blsp` wrapper stay byte-for-byte the same. **No new
ADR-030 contract is broken.**

**Why CHAMP, not vanilla Clojure HAMT.** Same big-O (O(log₁₆ N) ≈ effectively
O(1) up to billions of entries), but:
- **Two bitmaps per node** (`dataMap` for inline (k,v) entries, `nodeMap` for
  child subtries) instead of Clojure's combined slot array with type
  discrimination. Half the slots in the common case → smaller nodes, better
  cache use, less GC traffic.
- **Canonical form** under structural equality (no equivalent map has two
  representations), so `equal?` is a recursive walk that bails on the first
  shape mismatch — no need to fall back to "iterate one map, look every key
  up in the other" like ADR-030 does today.
- **Faster iteration** (entries first, then children, then collision nodes —
  CHAMP authors measured ~2× over Clojure's HAMT). Matters for `keys`/`vals`
  in long-running editor processes that fold over thousands of entries.

**16-way branching** (4 bits per level, 8 levels deep at max). 32-way nodes
allocate too much for small maps; 4-way pushes the tree too deep. Steindorfer
& Vinju measure 16 as the sweet spot on modern caches, and it matches our
existing `SmallVec<[Value; 16]>` instinct for inline storage.

**Storage shape.** A new heap slab type, `MapNode`, joins `Slabs` /
`CodeSlabs` next to the existing `maps` slab (which keeps its place as the
root handle — the existing `Value::Map(MapId)` *handle* is unchanged; only
the slot's contents become a CHAMP root node). The trie is built out of
those `MapNode` slots, addressed by `MapId` index-into-slab, so the GC
already knows how to mark/sweep/promote them (one new variant in the
`TraceItem` enum + one `mark_methods!` line). Collision nodes are a separate
small variant (≤ 8 entries before the canonical CHAMP fallback path); above
that the next hash level continues. Bitmaps are `u16` (one bit per child
slot — 4-bit slice → 16 children → fits exactly).

**Hashing.** Adopts the structural `hash_value` introduced by the abandoned
ADR-030-index attempt — consistent with `heap.equal` (0.0/-0.0 identical,
NaN canonical, recursive Pair/Vector/Map walks, region bits ignored). The
function stays in `heap.rs` (it needs `&Heap`); no `Hash`-trait impl on
`Value` (CHAMP nodes call `heap.hash_value(k)` directly).

**Immutability discipline (no regression).** Every `assoc`/`dissoc`
returns a fresh root via **path copying**: only the O(log N) nodes on the
path from root to the touched leaf are cloned; the rest is structurally
shared. Path-copy is the entire point of the ADR-030 trade-off finally
paying out. Frozen PRELUDE / shared RUNTIME maps stay safe because every
op allocates new LOCAL nodes — the shared regions are never mutated, just
referenced.

**Threading-safety & concurrency.** Trie nodes are `Send` once allocated
(every field is `Copy`). Promotion (LOCAL → RUNTIME) walks the trie depth-
first, allocating new RUNTIME slots and replacing handles — same shape as
`promote` for existing data structures. Cross-process message copy goes
through the same recursion. The append-only RUNTIME slab handles
concurrent reads of shared maps without locking, just as it does for
strings and vectors today.

**Consequences.**
- `assoc`/`dissoc` become O(log N) instead of O(n). For small maps this is a
  *constant-factor improvement* (one bitmap test + one slot copy) — no
  small-map regression like the bucket-index attempt had. For large maps
  this is the win we wanted (1000-entry build drops from ~31 ms to single
  digits).
- `get` becomes O(log N), and for the common case (key found in inline
  data, ~1 bitmap test + 1 `equal`) often faster than the old linear scan
  even at N=5.
- `equal?` between two maps drops from O(n²) to O(n) thanks to CHAMP's
  canonical form (compare bitmaps then walk in lock-step).
- One new ADR-030 contract clause: **iteration order is no longer
  insertion order.** `keys`/`vals`/`map-pairs` give a deterministic order
  per map shape, but it's hash-driven. ADR-030 promised insertion order;
  this ADR walks that back. (The current users — `pr-str`, `=`, tests —
  don't depend on it; the only test that asserts insertion-order
  iteration is `tests/maps_test.blsp:215` and would be rewritten as a set
  comparison. Equality is still order-independent.)
- Code volume: ~500 lines of new node logic in a new `core/map_champ.rs`
  module, plus ~30 lines in `heap.rs` for the slab + GC integration. The
  existing `map_*` functions in `heap.rs` shrink to thin handle-router
  wrappers over the trie ops.

**Pre-requisites.** Needs `hash_value(&Heap, Value) -> u64` in `heap.rs`
(the function the ADR-030-index attempt built, salvageable). Needs one
new `Tag` reservation (`MapNode`) and one bit in `types.rs`. Needs the
maps test suite to be updated to use set comparisons for iteration
(`tests/maps_test.blsp` lines that fix order).

## ADR-041 — Shared, refcounted blobs for large immutable byte data

**Status:** accepted, implemented 2026-05-29 (see devlog).

**Context.** ADR-026 made data immutable. ADR-033 proved that closure
*handles* can cross processes without copying — `(spawn …)` ships a closure
via tag-retag for PRELUDE/RUNTIME pointers, only deep-copying the captured
local frame. The bump-only LOCAL allocator (commit `f90f0de`, 2026-05-29)
made every allocation a single bump; combined with `(hibernate fn & args)`
that resets the arena at a controlled point, that gives bounded memory
without a tracing GC. What remained as the next throughput cliff was
**`to_message` deep-copying every string**: a 10 KB error string sent
from one worker to another paid 10 KB of memcpy on `send` *and* another
10 KB on `from_message` (alloc + copy). Closures already escape this via
ADR-033's closure-as-data path; strings should too.

**Decision.** Add a **per-runtime, refcounted blob heap** (`Arc<BlobHeap>`)
sibling to `Arc<RuntimeCode>` and `Arc<SharedCode>`. The LOCAL string slab
becomes a `LocalString` enum:

- `LocalString::Inline(String)` for strings below
  `SHARED_BLOB_THRESHOLD` (256 B) — the atomic-refcount overhead would
  dominate the per-byte memcpy at this size.
- `LocalString::Shared(Arc<SharedBlob>)` for strings at or above the
  threshold — the bytes live in the shared heap (immutable, freed when
  the last `Arc` drops). Both PRELUDE and RUNTIME stay `Vec<String>` /
  `boxcar::Vec<String>` unchanged — the prelude builder's freeze
  inline-extracts any `Shared` entries so the cross-runtime PRELUDE
  region holds no runtime-scoped `Arc`s.

`Heap::alloc_string` is the **single chokepoint** that routes by threshold;
no other path materialises a `Value::Str`. `to_message` (process/message.rs)
calls `local_shared_blob` first — for a LOCAL Shared string it returns the
`Arc::clone` (atomic incr, no byte copy) into a new `Message::StrShared`
variant; otherwise it falls back to the deep-copying `Message::Str`.
`from_message` for `Message::StrShared` calls `alloc_string_from_shared`,
which installs the cloned `Arc` into the receiver's LOCAL slab — same
SharedBlob identity, no bytes copied. Process exit drops the Heap → the
slot drops the `Arc` → the blob is freed at zero. Hibernate flush
(`flush_string`) clones the `Arc` into the new slab; the old slab's drop
decrements; net unchanged across the flush (survivors keep blob identity).

Cross-node sends never share the `Arc` — the wire codec (`dist::wire`)
encodes `Message::StrShared` as inline bytes (`M_STR`), so the receiving
runtime allocates a fresh blob through its own `alloc_string`. Within one
runtime, every spawned green process shares the same `Arc<BlobHeap>` (via
`Arc::clone` on construction), so a blob's identity is preserved across
every cross-process send.

**Why plain `Arc<T>`, not a hand-rolled raw-ptr + atomic.** ADR-026's
immutability guarantee means data can't form cycles — a `cons` can only
point at things allocated *before* it, so an `Arc<SharedBlob>` is sound
without `Weak`/cycle-collector machinery. The standard library does the
atomic incr/decr and `Drop` for us; safe code; one extra word (`Arc`'s
strong/weak counter) per blob, which is negligible against blob sizes
that justify the threshold. The receiver-side extra `Arc::clone` (we have
`&Message`, not owned) is one atomic op per send and can be moved later
if a refactor of the mailbox API lets `from_message` consume the message.

**UTF-8 invariant.** Every entry to `SharedBlob` is via `&str.as_bytes()`
(in `Heap::alloc_string`) or via the wire decoder's pre-validated UTF-8
buffer. Blobs are immutable. So `LocalString::as_str` reads with
`from_utf8_unchecked` in release builds (zero overhead). Debug builds
keep the validating `from_utf8` as a tripwire — a missed entry point
would trip the assertion at the call site.

**Threshold (256 B).** A 256-B memcpy is ~30 ns on modern CPUs; an atomic
incr is ~5–10 ns. Below 256 B, the indirection through the heap + atomic
is in the noise but adds an L1 miss; above it, the per-byte cost
dominates. One `const SHARED_BLOB_THRESHOLD: usize = 256` in
`core/blob.rs`; retunable from one place once profiling warrants it.

**Out of scope (Phase 1).**
- **Spawn-captured strings.** `(spawn (fn () (use big-string)))` runs
  `Heap::promote` on the captured frame; promote currently extracts
  bytes from any `LocalString` into a fresh `String` in RUNTIME's
  `boxcar::Vec<String>` (so the bytes are still shared — RUNTIME is
  shared — but through a different mechanism). Routing promote through
  the blob heap is a follow-up.
- **Vectors of large byte content.** Vectors hold `Value`s which may
  themselves be handles, so the byte-flat sharing model needs more design.
- **Cross-node content-addressing.** The wire codec inlines the bytes;
  a Phase 2 could dedupe blobs that arrive twice from the same peer.
- **Blob interning by content.** No global hash-set of blob bytes; two
  separately-allocated 10-KB identical strings get two `Arc<SharedBlob>`s.
- **PRELUDE retag unification.** The prelude crosses processes by handle
  retag today (its strings are read-only). Unifying it with the blob
  mechanism would be a code-cleanup, not a perf win.

**Consequences.**

- The 10-KB-string send path drops from O(N) bytes to one atomic incr.
- Strings travel cross-process between green processes (via `(send …)`)
  without copying. Spawn-capture still copies — see above.
- A new `Value` *kind* was **not** introduced — the existing `Tag::Str`
  is unchanged. The Inline/Shared split lives in the LOCAL slab entry
  type, so the surface language (and the type checker) see strings
  exactly as before.
- The wire format is unchanged: `Message::StrShared` encodes as `M_STR`,
  so the dist protocol remains backwards-compatible.
- A pair of debug-only primitives — `(%blob-ptr s)` returning the
  `SharedBlob` address as an integer for identity checks, and
  `(%blob-strong-count s)` returning the current refcount — ship under
  `#[cfg(debug_assertions)]` (parallel to the existing `%force-panic`)
  and are guarded with `(bound? …)` in tests so release runs skip them.
- Code volume: ~80 lines of new `core/blob.rs`, ~150 lines of changes in
  `core/heap.rs` (LocalString enum + alloc/string/sweep/flush/freeze
  updates), ~20 lines in `process/message.rs`, ~15 in `dist/wire.rs`,
  ~50 in `builtins.rs` for the two debug primitives. Coverage: ~10 new
  in-language tests in `tests/blob_share_test.blsp` (cross-process
  identity for ≥ 256 B; nil for inline / RUNTIME; 8-worker fan-out;
  hibernate flush preserves identity); a new benchmark
  `concurrency::big_string_fanout` comparing 128 B vs 10 000 B payload
  fan-out.

**References.** ADR-026 (immutability → no cycles → safe rc), ADR-033
(closure-as-data established cross-process handle retag), commit
`f90f0de` (Phase 1 bump-only LOCAL allocator — this design preserves
"no slot reuse"; a Shared slot's handle still grows monotonically, only
the *bytes* are shared), commit `dee0814` (Phase 2 hibernate — flush
must Arc::clone survivors to maintain blob identity).


## ADR-042 — Live-editing hardening: `defonce`, reload-defs detection, dedup, macro-staleness warning

**Status:** accepted, implemented 2026-05-29 (see devlog).

**Context.** The hot-reload *mechanism* is built and documented in
[`shared-code.md`](shared-code.md) (shared RUNTIME region, late-bound globals,
append-only code). [`live-editing.md`](live-editing.md) is the *next* layer —
the handful of things still missing before you can edit the running editor all
day the way you edit a running Emacs. This ADR lands the cheap, high-value
subset of that plan (its Stages 1, 2, 5-dedup, 7); the rest stays planned.

It also **reverses a planned removal.** ADR-039 (supervised-by-default
processes) was *tried and reverted* on 2026-05-29 (roadmap M-process; the
kernel-side supervisor was the bulk of the scheduler-race surface). ADR-039 had
scheduled `defonce`'s deletion "in the same change that adds named-spawn" —
but named-spawn never shipped, and even if it had, it only covers the
*process-singleton* case. The *global state cell* case it does not. So the
planned removal is **void**; `defonce` is the chosen tool, not a transitional
shim.

**Decision.** Four small hardening pieces, all Brood or thin Rust:

1. **`defonce` (prelude macro) — kept and blessed.** Evaluate the init form
   *only if the symbol is not already bound*; otherwise leave the existing
   binding untouched (Emacs `defvar` / Clojure `defonce`). Reload re-runs every
   `def…` form, which would otherwise reset global cells
   (`(defonce *registry* {})`) and re-spawn singletons/reopen resources
   (`(defonce *server* (spawn (serve)))`, leaking the old one). A **pure prelude
   macro over existing primitives** — `(unless (bound? '~name) (def ~name ~val))`
   — zero kernel surface. `bound?` checks *any* binding in scope; it's correct at
   top level (the only place reload re-evaluates), which is where `defonce`
   belongs.

2. **`reload-defs` detection tightened.** A top-level form is treated as a
   definition iff its head symbol starts with `def` **and** is actually a definer
   — a core `def`/`defmacro` special form, or a symbol currently bound to a
   `Macro` (so `defn`/`defmodule`/`defdyn`/`defonce` and any user `def…` macro
   qualify). This drops the false positive where a plain top-level *call* whose
   name starts with `def` (e.g. `(default-config)`) was re-run on every reload:
   it resolves to a `Fn`, not a macro, so it's now correctly skipped. **Known
   limitation:** a definer macro *not* named `def…` (e.g. `(register-handler …)`
   expanding to a `def`) is skipped — workaround: prefix definer macros with
   `def`, the Lisp convention anyway. No dependency graph, no registry.

3. **`reload-defs` atomicity (cheap 90%).** The whole file is read and parsed
   before any form is evaluated, so a syntactically broken / half-saved file
   applies *zero* defs (the read fails first). The residual non-atomic window — a
   *runtime* error while evaluating form N, after forms 1..N-1 already landed —
   is accepted and documented; full snapshot/rollback of the affected bindings is
   deferred (it's rare and the leak it prevents is "some defs newer than others,"
   not corruption).

4. **Dedup-on-identical redefinition.** A `def` of structurally-identical code
   (a save-without-change, or `nest format` rewriting the file) is **not**
   appended as a new version to the append-only RUNTIME region; a genuine change
   still appends and is live immediately. This is the cheap half of
   [`live-editing.md`](live-editing.md) Stage 5 (bounded RUNTIME memory); the real
   compacting collector for superseded versions is deferred to its own stage.

5. **Macro-redefinition staleness warning.** When `defmacro` *rebinds* an
   existing macro, print `[reload] macro X redefined; callers expanded before now
   keep the old expansion — re-eval them`. Silent on first definition (prelude /
   first file load). Mirrors the existing `def` arity-change diagnostic. A true
   reverse-dependency index (who expanded X) is deferred — the warning is 90% of
   the value at 5% of the cost.

**Out of scope / deferred** (tracked in [`live-editing.md`](live-editing.md)):
editor-driven eval via LSP commands (Stage 3), single-process watcher +
optional `notify` (Stage 4), the long-lived-process upgrade hook /
`*code-version*` (Stage 6), and the true RUNTIME collector (Stage 5's later
half). Schema/record migration is **not applicable** — data is structurally
typed immutable maps, so there's no nominal type whose field set can drift out
of sync with live instances.

**References.** [`shared-code.md`](shared-code.md) and
[`live-editing.md`](live-editing.md) (the mechanism and the plan), ADR-013
(redefinable globals / hot reload), ADR-026 (immutability — state lives in
processes, so reload doesn't touch process-threaded state), ADR-039 (reverted;
its scheduled `defonce` removal is void).


## ADR-043 — Runaway-resource backstops: memory limits (E0043) + eval-depth ceiling (E0044)

**Status:** accepted, implemented 2026-05-29 (see devlog).

**Context.** The runtime hosts code it doesn't trust to be well-behaved: the
in-language suite includes [`tests/adversarial_test.blsp`](../tests/adversarial_test.blsp),
and the editor's whole point is to `eval` code you're editing. Two runaway
patterns take down the *host* rather than failing cleanly:

- **Unbounded allocation** (`(cons …)` loop, `(string-repeat "x" huge)`)
  exhausts host RAM and can freeze the machine.
- **Unbounded non-tail recursion** (`(defn boom (n) (+ 1 (boom (+ n 1))))`)
  overflows the coroutine stack — a SIGSEGV the host can't `catch_unwind`, so it
  aborts the whole REPL / `nest mcp` server, not just the offending process.

Both should become clean, catchable Lisp errors.

**Decision.** Two backstops, both **off by default**, both **process-wide**
(per-process accounting is deferred — ADR-011):

**Memory (`E0043`).** A counting `#[global_allocator]` (`core/alloc.rs`,
std-only per ADR-005) tallies live + peak bytes for the *whole* process, with
two tiers:

- **Hard limit** — enforced in `alloc`/`realloc`: an allocation that would cross
  it returns null, so Rust's OOM handler aborts the process. Ungraceful (kills
  every green process) but it is the backstop that guarantees the *host* survives
  any pattern, including a single huge allocation *between* eval safepoints.
- **Soft limit** — *not* enforced in the allocator; polled at the eval safepoint
  (`eval/mod.rs`, gated on `gc_block_depth() == 1`, the same outermost-eval gate
  as the GC safepoint, ADR-035) and raised as a catchable `E0043`. Set below the
  hard limit so a runaway *loop* fails gracefully (only the offending process
  dies; `try`/`catch` can recover) long before the hard abort.

Configured via `BROOD_MEM_LIMIT` (hard) / `BROOD_MEM_SOFT_LIMIT` (soft); soft is
derived as ¾·hard when only the hard is given. Plain `brood`, the REPL, and
`nest run`/`mcp` stay **unlimited** unless the user opts in (the live image edits
all day). The **test runners** (`brood --test`, `nest test`, the `cargo test`
Brood suite) default a ceiling on (`TEST_DEFAULT_HARD`/`TEST_DEFAULT_SOFT`) so an
adversarial test can't OOM the machine; an explicit env var still wins.
`(mem-limit)` / `(mem-soft-limit)` expose the ceilings; `(mem-bytes)` /
`(mem-peak)` the counters.

**Eval depth (`E0044`).** `GC_BLOCK` already counts nested `eval`/`macroexpand`
frames — i.e. *non-tail* recursion depth (a tail call re-enters the `'tail:`
loop without a new frame, so it doesn't bump the counter). At the top of `eval`,
if that depth exceeds the ceiling, raise a catchable `E0044` *before* the
coroutine stack overflows. Default `MAX_EVAL_DEPTH_DEFAULT = 3500`, tuned for the
tightest case (a debug build on the 2 MiB coroutine stack, `CORO_STACK_BYTES`);
the root thread and release builds have far more headroom. Tune with
`BROOD_MAX_DEPTH`. This only ever bites runaway non-tail recursion — Brood loops
are tail recursion (O(1) stack), which doesn't grow `GC_BLOCK`.

**Why two tiers for memory.** The soft limit is the graceful, catchable, common
path. The hard limit covers the one case the soft path *cannot*: a single giant
allocation inside one builtin (`string-repeat` of a huge count) with no
intervening safepoint to poll. The soft check between safepoints can't see it
coming; the allocator can.

**Test-runner default sizing.** Started at 2 GiB hard / 1.5 GiB soft; **lowered
to 512 MiB / 384 MiB on 2026-05-29.** Per-process heaps are `Rc`-reclaimed when a
green process exits, so the suite's footprint is the *concurrent* peak across
~`nproc` workers plus the shared baseline — not a cumulative total — which 512
MiB covers with headroom while making a genuine runaway trip in a fraction of a
second instead of chewing through gigabytes first.

**Known gaps / deferred.**
- **Per-process limits** — only process-wide accounting today (ADR-011: ship the
  simple form, defer the powerful one).
- **Soft check only at `gc_block_depth() == 1`** — a runaway happening entirely
  inside one builtin reaches only the hard limit (abort), not the catchable soft
  path. Accepted: the hard limit protects the host, and builtins that can
  allocate unboundedly are few.
- **The 3500 depth ceiling is empirical headroom, not a proof** against the 2 MiB
  debug coroutine stack; a genuinely deep non-tail algorithm raises
  `BROOD_MAX_DEPTH`.
- **`mem_limit.rs`'s runaway test is `#[ignore]`d** — it drives an unbounded
  allocation by construction (to prove the soft limit catches it), so it's not
  run unattended in a routine `cargo test`; run it with `--ignored` when you can
  watch it.

**References.** ADR-035 (per-process tracing GC — same `gc_block_depth() == 1`
outermost-eval safepoint the soft-memory check rides on), ADR-018 (green
processes and their coroutine stacks), ADR-011 (favour the simple design; defer
per-process limits), ADR-005 (dependency-free, std-only allocator).

---

## ADR-044 — Supervision is a userland Brood library, not a kernel feature

**Status:** accepted (2026-05-29). Supersedes the kernel-supervisor direction of
ADR-039 (tried and reverted; see [`supervision.md`](supervision.md)).

**Context.** ADR-039's kernel supervisor was reverted because its RESUME_SLOT +
safepoint-rooting machinery was the bulk of the multi-thread scheduler race
(KI-1). The building blocks it was built over — `spawn` / `monitor` / `receive`
— were never the problem and remain. The roadmap calls for supervisor trees;
the question was *where* they live.

**Decision.** Supervision is a require-able Brood module, `std/supervisor.blsp`
(`(require 'supervisor)`), built entirely on `spawn` / `monitor` / `receive`. A
supervisor is an ordinary green process carrying immutable state through a
receive loop (the `hatch.blsp` idiom); it `monitor`s each child and reacts to the
kernel's `[:down ref pid reason]`. **No new kernel surface** — this is the
mechanism-in-Rust / policy-in-Brood rule (ADR-006) applied to fault tolerance,
and it adds *zero* scheduler-race surface, the decisive property after KI-1.

**Scope.**
- **All three strategies ship** (update 2026-05-30, once `exit/2` landed —
  ADR-063): `:one-for-one`, `:one-for-all`, `:rest-for-one`. The group strategies
  must *terminate healthy siblings* on a sibling's death; the `(exit pid :kill)`
  primitive (untrappable hard kill, fires the target's `[:down]`) supplies exactly
  that, and `receive` being selective lets the supervisor drain just the killed
  sibling's `[:down]` so a deliberate kill isn't mistaken for a crash. The crashed
  child's `:restart` type gates whether the procedure runs; within a group restart
  each member is restarted only if its own type permits (`:temporary` → terminated
  and dropped). *Originally `:one-for-one`-only* — the group strategies were
  deferred for want of a kill primitive (ADR-011); that deferral is now closed.
- **Restart types:** `:permanent` (always), `:transient` (only on abnormal exit,
  reason ≠ `:normal`), `:temporary` (never).
- **Restart intensity:** `:max-restarts` within `:max-seconds` (defaults 3/5);
  exceeding it exits the supervisor abnormally so a watcher's monitor fires.
- **Introspection:** `(which-children sup)` → `[{:id :pid :restart}]`.
- **Managed names + reverse-order shutdown** (update 2026-05-30): a `:name`
  keyword in a child spec is `register`ed to the fresh pid on every (re)start, so
  callers address a stable name via `whereis` across restarts; and `terminate-many`
  tears children down in **reverse start order** (OTP's dependency-safe order).
- **`:shutdown` policy + nested-tree cascade** (update 2026-05-30): a child spec's
  `:shutdown` is `:brutal-kill` (default — `exit … :kill`), `:infinity` (send
  `[:$stop]`, wait), or an integer ms (graceful, then a hard-kill backstop).
  Marking a supervisor child `:shutdown :infinity` makes teardown **cascade
  depth-first** into the sub-tree (the child supervisor runs its own
  `terminate-many` on `[:$stop]`) instead of orphaning grandchildren — Erlang's
  exact rule. Opt-in per child because broadcasting `[:$stop]` to an arbitrary
  worker is unsafe (it could match and consume it as data). A child whose `:start`
  spawns a supervisor composes as a sub-tree; crash *escalation* through it already
  worked, this closes deliberate *teardown*.
- **Still deferred (ADR-011):** `link`/bidirectional exit propagation — termination
  is one-directional and supervisor-driven; the `:shutdown` cascade covers the
  shutdown direction, not automatic *upward* propagation from a linked peer's crash.

**Consequences.**
- `stop-supervisor` and an intensity-exceeded shutdown both **terminate the
  children** now (no orphans) — the same `(exit … :kill)`. (Pre-`exit/2` they left
  children running; that limitation is gone.)
- A child spec carries a `:start` *closure* (`(fn () (spawn …))`), shipped across
  the spawn boundary by the closure-as-data path (ADR-033); restart re-invokes it
  for a from-scratch incarnation.
- Tests: `tests/supervisor_test.blsp` (restart, all three restart types,
  intensity give-up via a monitor on the supervisor, introspection, strategy
  rejection), `:isolated` per the process-test convention.

**References.** ADR-039 (reverted kernel supervisor), ADR-006 (policy in Brood),
ADR-011 (defer power features), ADR-033 (closures as data),
[`supervision.md`](supervision.md), [`concurrency-v2.md`](concurrency-v2.md) §4.

## ADR-045 — Text ropes as an opaque, immutable heap value (`Value::Rope`)

**Status:** accepted (2026-05-29). The first M2 (editor data model) substrate —
the one new `Value` kind the editor's buffer text needs.

**Context.** The editor stores buffer text. A flat `String` is O(n) per edit and
can't index lines cheaply; the editor needs O(log n) insert/delete and char↔line
mapping over files. That's a B-tree rope — a structure Brood can't bootstrap over
its existing primitives, so it's the one irreducible piece of *text mechanism*
that belongs in Rust (the "Rust is mechanism, Brood is policy" rule, ADR-006).
The open question was how to expose it without breaking the immutability
invariant (ADR-026: no data mutation; every op returns a fresh value), the
tracing-GC assumptions (no write barriers), or the share-nothing process model.

**Decision.** Add a single new heap value, `Value::Rope(RopeId)` / `Tag::Rope`,
backed by a `ropey::Rope`, with a ~10-primitive kernel (`string->rope`,
`rope->string`, `rope-length`, `rope-line-count`, `rope-insert`, `rope-delete`,
`rope-slice`, `rope-line`, `rope-char->line`, `rope-line->char`; all
character-indexed). Everything above it — points, marks, regions, search, undo,
the buffer itself — is Brood.

- **Immutable, for free.** `ropey::Rope` is an `Arc`-shared B-tree: `clone()` is
  O(1) (bump refcounts) and edits are copy-on-write on touched nodes only. So
  `rope-insert`/`rope-delete` *clone-then-edit* and return a **fresh** rope; the
  input is untouched and shares all unchanged structure. The ADR-026 contract
  holds with no special-casing — a rope behaves like every other immutable value.
- **Process-local.** A rope lives in exactly one process's LOCAL heap and **never
  crosses in a message** (`to_message` errors with a hint to send `rope->string`
  and rebuild). This matches the buffer-as-process design (the rope stays put in
  the buffer process; only edit commands and rendered string slices cross) and
  keeps copy-on-send from ever deep-copying a whole file. A rope `def`'d to a
  global *is* promoted into the shared RUNTIME region (mirrors `Str`): immutable
  + `Send`+`Sync`, so sibling processes read it concurrently and safely.
- **GC.** The rope slab is wired into every reclamation site — the live arena-flip
  `flush` path (clone forwards the rope, structural sharing intact), the dormant
  mark/sweep, the poison tripwire, checkpoint/reset, and `local_live_count`. A
  rope is an opaque leaf (no `Value` children) so marking it is a one-liner.

**Compatibility contract (types.md #1).** `Tag::Rope` is the 16th tag, filling the
`Ty(u16)` lattice exactly (`UNIVERSE` now computes in `u32` then narrows, to dodge
the `1u16 << 16` const-overflow); a 17th tag must widen `Ty` to `u32`. `rope?` is
a prelude predicate over `type-of`, and `Ty::tested_by` narrows on it.

**Consequences.**
- One new dependency (`ropey`) in the `brood` lib — squarely the "runtime
  substrate that removes real complexity" case the dependency rule allows; the
  Lisp-callable surface is still Brood.
- This is the *only* new `Value` kind M2 needs; buffers, cursors, and keymaps are
  all Brood values built from existing kinds. It's also the template for any
  future opaque resource (a GPU texture, an OS handle), should one ever be
  justified (deferred per ADR-011 — a concrete rope beats a general FFI-resource
  system until a second resource type exists).

**References.** ADR-006 (mechanism/policy split), ADR-026 (immutability), ADR-005
relaxation (runtime-substrate crates), ADR-011 (ship the simple form),
[`roadmap.md`](roadmap.md) M2, [`types.md`](types.md) compatibility contract.

## ADR-046 — The display/input seam: a frontend is a protocol of render-op data

**Status:** accepted (2026-05-29). The first M3 substrate — the seam between the
runtime and any frontend (local terminal today, a socket peer later).

**Context.** The editor must feel native locally *and* serve remote/web frontends,
from one codebase (architecture.md). The way to get that for free is to make the
display layer a **protocol, not a library**: the runtime emits a serialisable
stream of "render this" operations and consumes input events; the local frontend
implements that protocol in-process (the fast path), and a remote/web frontend
implements the *identical* protocol over a socket. The open question was how thin
the Rust surface should be, and where the protocol's meaning should live.

**Decision.** The render frame is **Brood data** — a vector of tagged render ops
(`[:clear]`, `[:text row col s]`, `[:text row col s face]`, `[:cursor row col]`)
— and Rust supplies only the *frontend that paints it* plus the input source:
five `term-*` primitives over `crossterm` (`term-enter`, `term-leave`,
`term-size`, `term-poll`, `term-draw`). Plus one process-introspection accessor,
`mailbox-size`, that the first app needs and Brood can't reach (the mailbox queue
lives behind the scheduler registry).

- **Protocol meaning is policy (Brood); painting is mechanism (Rust).**
  `std/display.blsp` defines the op vocabulary as pure constructors; `term-draw`
  is a ~40-line interpreter of that vector. So the op set is redefinable Lisp data
  and a remote frontend re-implements the same ops elsewhere — exactly the seam
  architecture.md promised. This is the "drawing, I/O" Rust-primitive category the
  architecture already anticipated (ADR-006).
- **Observer-as-proof, not editor-first.** `std/observer.blsp` + `nest observe` is
  a tiny Erlang-observer-style process viewer — the *smallest real app* on the
  seam. It needs no rope/buffer, so it validates the render protocol + key loop
  end-to-end in isolation, before the editor rides on it. A node-stats panel +
  navigable process list (`↑`/`↓` select, `space` pause, `q` quit). Split into a
  pure `observe-frame` (node + process data → frame, unit-testable without a TTY)
  and a thin `observe-run` IO loop. **Interactivity without mutation:** the UI
  state (selection, freeze) is a plain map threaded through the tail-recursive
  loop — each keypress recurses with a fresh state; selection is tracked *by pid*,
  not row index, so it stays on the same process as the list reorders. Node stats
  reuse existing primitives (`node-name`/`worker-threads`/`mem-bytes`/…); the only
  new Rust is `mailbox-size`.
- **Scheduler safety.** The observer runs in the **root process** (the binary's
  dedicated thread, which is *not* in the scheduler worker pool), so its blocking
  `term-poll` blocks only that thread — never a worker running the processes it
  observes. Poll timeouts are always finite: preemption can't interrupt a process
  parked in a native crossterm call, so an infinite poll on a *green* process
  would pin a worker. (Future async input — a reader thread feeding a mailbox —
  would lift even the root-thread block; deferred per ADR-011.)
- **Terminal-restore is belt-and-suspenders.** The normal teardown is the Brood
  `term-leave` (on quit); a Rust RAII guard in `nest observe` (`brood::builtins::
  restore_terminal`) is the abnormal-path backstop, firing on a panic unwind and
  scoped to drop *before* an error-exit (since `process::exit` skips `Drop`), so a
  crash never leaves the terminal in raw mode / the alternate screen.

**Consequences.**
- One new dependency (`crossterm`) in the `brood` lib — the runtime-substrate
  case the dependency rule allows; the Lisp-callable surface (`display`/`observe`)
  is Brood. `display`/`observe` are embedded opt-in modules, never in the prelude.
- The op vocabulary is intentionally minimal (text + cursor + clear + a small face
  map of fg/bg/bold/reverse). Faces beyond that, mouse/resize events, scroll, and
  attaching the observer to a *remote* live image are additive and deferred
  (ADR-011). The same `term-draw`/`term-poll` shape is what the M3 editor frontend
  and the M4/M5 socket frontends will speak.

**References.** ADR-006 (mechanism/policy), ADR-045 (the rope, the other editor
substrate), ADR-005 relaxation (runtime-substrate crates), ADR-011 (ship the
simple form), ADR-043 (the root-vs-worker thread + stack model),
[`architecture.md`](architecture.md) (the seam), [`roadmap.md`](roadmap.md) M3.

## ADR-047 — Native multi-arity closure dispatch

**Status:** accepted (2026-05-29). Closes the variadic-arithmetic performance gap
without moving `+`/`-`/`=` out of Brood.

**Context.** The prelude's variadic arithmetic and comparison operators (`+`, `*`,
`-`, `/`, `<`, `=`, …) are written *in Brood*, as `defn`s over `fold` and a
rest-list. That is the project's core principle in action (ADR-006: write the
language in the language) — but it was costing **~40× a direct primitive call**.
Each `(+ a b)` allocated a `& xs` rest-list, then a `fold`, then a
`fold--loop`/`empty?`/`first`/`rest` chain ≈ 15 env frames — none of which the
(no-op) GC reclaims. `(sum-to 100000)` spent **497 MB** purely on this per-call
overhead. The naïve fix — make `+`/`-`/`=` Rust builtins — is fast but reverses
the whole reason the project exists and teaches us nothing. CLAUDE.md's "dogfood
first; optimize only by building the language up, not around it" sets the bar: an
optimization must (1) improve language performance *broadly* and (2) build up a
*primitive/capability* so Brood code gets faster — not move behaviour into a Rust
escape hatch. Variadic `+` was the worked example of a missing capability:
**efficient arg-count dispatch**.

**Decision.** Give the evaluator **Clojure-style multi-arity dispatch**. A closure
holds a `Vec<ClosureArm>` (was a flat `params/optionals/rest/body`); each arm is
one arity clause. The call's argument count selects the arm, which then binds its
parameters **directly** — no rest-list, no `match*`, just one env frame for the
common small call. `+` stays Brood; `(+ a b)` is now ~one env frame instead of
~15.

- **Arity clauses vs. pattern clauses — a split, not a replacement.** A clause
  whose head is *arity-only* (plain-symbol params plus optional `&optional`/`&`
  rest) becomes a `ClosureArm` and dispatches natively by count. A clause whose
  head contains *patterns* (literals/destructuring, e.g. `((0) 1)`, `((3 _) …)`)
  still lowers to the existing `match*` engine (`eval::macros::lower_fn`). So the
  pre-existing Erlang-style **same-arity pattern dispatch** (ADR-010) is untouched;
  multi-arity is a second, orthogonal dispatch axis layered cleanly in front of it.
  `fn_is_arity_multi_clause` decides which a given `defn` is.
- **`select_arm(argc)` semantics.** Among arms that `accept(argc)`, prefer an
  **exact fixed-arity** arm (no `&` rest) over a variadic one; among those, the
  **most specific** (most required params). A single-arm closure always returns its
  sole arm when `argc` fits, else an arity error listing the accepted arities.
- **One representation, threaded everywhere.** `arms` replaces the flat fields
  through the whole closure lifecycle: `make_closure`/`bind_params`/`apply_closure`
  and the inline TCO call path (`eval/mod.rs`), `promote_closure`/`flush`/GC
  trace/structural-dedup (`heap.rs`), `to_message`/`from_message` (cross-process
  spawn) and the dist wire codec (cross-node), and the type checker (`infer_sig`
  only fires for single-arm closures — sound: no false inference for an
  overloaded fn; `arity_of` spans all arms).

**Consequences.**
- **`(sum-to 100000 0)` = 61 MB, was 497 MB → 8.1×**; `basic.rs` runtime 29 s → 5 s.
  This is the floor for a fixed-arity arm (≈1 env frame, ~0.6 KB/call) vs. the old
  variadic path (~5 KB/call). The win is *per-op*; it does **not** change the no-GC
  *cumulative* accumulation that still bounds the full in-language suite (that is a
  GC problem — see [`memory/no-gc-suite-memory.md`](../memory/no-gc-suite-memory.md)
  and roadmap M1).
- `+ * - / < > <= >= = not=` are rewritten in the prelude with fast 0/1/2-arg arms
  and a variadic 3+ fallback — still Brood, now cheap.
- **Two things you cannot mix in one `defn`:** arity-overloaded clauses and
  pattern/`&optional` heads. A head is read as *either* an arity arm *or* a pattern
  clause; an `&optional` inside a multi-clause head is treated as a literal symbol
  (it doesn't make that arm variadic). This matches the pre-existing rule that
  `&optional`/patterns/multi-clause don't nest (see `docs/language.md`).

**References.** ADR-006 (write the language in the language), ADR-010 (parameter
lists are lists; Erlang-style same-arity pattern dispatch), ADR-002 (`Rc`→`gc-arena`,
why heap construction stays funnelled), CLAUDE.md "Dogfood first; optimize only by
building the language up", [`language.md`](language.md) (`fn`/`defn` clauses),
[`roadmap.md`](roadmap.md) M1 ("Memory reclamation" — the cumulative-memory story
multi-arity helps but doesn't fully solve).

## ADR-048 — Self-hosted REPL (the read-eval-print loop in Brood)

**Status:** accepted (2026-05-29). Moves the REPL out of Rust (`crates/repl`) and
into Brood (`std/repl.blsp`); the `rustyline` dependency leaves the tree with it.

**Context.** The REPL was Rust from day one — a bootstrap (`crates/repl`, shared
by `brood` and `nest repl`) doing `rustyline` line editing, multi-line balance
detection, per-command heap reset, and error printing. The roadmap always carried
"self-host the CLI/REPL in Brood" as M1 work (the core principle, ADR-006: Rust is
mechanism, Brood is policy — and a read-eval-print loop is pure policy). Three
prerequisites had to land first, and now all have:
- **`eval-string`** is the whole evaluator, callable from Brood (read-all →
  macroexpand-all → eval).
- a never-returning Brood loop can be **memory-bounded** — the design target the
  per-process tracing GC (ADR-035) was meant to hit. ⚠️ That mark-sweep is
  currently **disabled** (`Heap::collect` is a no-op — see ADR-035), so the
  reclamation that actually works today is `(hibernate fn & args)` (arena flip),
  plus the wholesale free of a process's LOCAL heap when it *exits*. `repl--loop`
  therefore recurs via `(hibernate repl--loop tty)`: each command flips the arena,
  keeping only the loop fn + `tty`. Measured: 50 000 allocating commands went from
  **~15 GB** peak RSS (plain recursion) to **flat** with the hibernate flip. The
  Rust `checkpoint`/`reset_local_to` is gone from the Brood loop regardless.
  Because `hibernate` is caught only by the **spawned-process** scheduler loop, not
  the root `eval_str`, `repl-run` runs the loop in a spawned process and `monitor`s
  it to await EOF (the root parks in `receive`).
- **`try`/`catch`** surfaces a built-in error to Brood as a structured map
  (`{:kind :message [:line :col] …}`, ADR + `docs/llm-native.md` §4), so the loop
  can format errors without parsing strings.

**Decision.** Write the loop in `std/repl.blsp` (opt-in module, `(require 'repl)`),
add **one** irreducible Rust primitive, and shrink the binaries to a bootstrap.
- **New primitive: `(read-line)`** — a blocking read of one line from stdin,
  returning the line (trailing newline stripped) or `nil` at EOF. Blocking stdin
  I/O is genuine mechanism the language can't bootstrap; everything else is Brood.
- **Multi-line input rides the reader, not a hand-rolled scanner.** An unclosed
  form or string makes `eval-string` raise the reader's `INCOMPLETE_INPUT` error
  (code `E0002`, ADR-049) — the signal to read another line; any *other* error is
  a real error to report. Because `eval-string` reads *all* forms before evaluating
  any, an incomplete buffer throws at read time with nothing evaluated, so retrying
  the growing buffer as lines arrive has no partial/double side effects. (An earlier
  draft hand-scanned delimiters in Brood; matching the stable error code is simpler
  and more correct — it tracks the reader's own notion of "complete," strings and
  comments included.)
- **Line editing comes free from the terminal's cooked mode** (backspace, `^U`,
  `^W`), so `read-line` stays a plain read — no raw-mode key handling needed for
  v1. Arrow-key history/recall is a later additive layer over the `term-*` raw-key
  seam (M3) + the buffer framework (M2); the point of self-hosting is that it's now
  a Brood function to add, not a Rust dependency to carry.
- **`brood` (no args) and `nest repl` bootstrap into `(require 'repl) (repl-run)`**;
  the `repl` module is baked into the binary (`EMBEDDED_MODULES`) like the prelude.
- **`crates/repl` and `rustyline` are deleted.** Greenfield: no compatibility shim
  (CLAUDE.md). Reads work piped too (`echo '(+ 1 2)' | brood` → `3`); prompts and
  the banner gate on `(stdout-tty?)` so they never pollute a redirected stdout.

**Consequences.**
- The REPL is now redefinable at runtime like the rest of the system — prompts are
  the dynamic vars `*repl-prompt*` / `*repl-cont-prompt*`; the loop, error
  rendering, and incomplete-input detection are ordinary Brood functions.
- **Lost (for now):** arrow-key history recall and Emacs keybindings that
  `rustyline` provided. Cooked-mode editing covers in-line correction; history is
  the first thing to add back over the raw-key seam. Acceptable per the dogfooding
  trade (CLAUDE.md): surface the gap rather than carry a Rust escape hatch.
- One less crate and one fewer third-party dependency; the LSP never depended on
  the REPL, so nothing there changes.
- `tests/repl_test.blsp` covers the pure pieces (datum detection, incomplete-input
  detection, error rendering) incl. a cross-process error-map round-trip; the IO
  loop is exercised manually via `brood` / piped input.

**References.** ADR-006 (write the language in the language), ADR-035 (the
per-process tracing GC meant to bound a never-returning Brood loop — currently
disabled; reclamation today is `(hibernate)` + process-exit), ADR-049 (the reader
`INCOMPLETE_INPUT` signal that drives multi-line reads), ADR-028 (`brood`/`nest`
split — both bootstrap the same Brood REPL), ADR-046 (the `term-*` seam a future
raw-mode line editor rides on), CLAUDE.md "Dogfood first" and "Greenfield".

## ADR-049 — Reader `INCOMPLETE_INPUT` as the multi-line continuation signal

**Status:** accepted (2026-05-29). Formalises a use for an error code the reader
already carried; first consumer is the self-hosted REPL (ADR-048).

**Context.** A REPL — or an editor's interactive evaluator — reading a line at a
time must tell two failures apart: **"input ended mid-form"** (an unclosed `(`,
`[`, `{`, or string → *keep reading*) versus a **genuine syntax error** (e.g. an
unexpected `)` → *report it now*). The naive approach re-scans the text for
balanced delimiters in the consumer, which duplicates the reader's lexing and gets
the corner cases wrong (delimiters inside strings, inside `;` comments, escaped
quotes). The reader already knows precisely when it hit EOF mid-form.

**Decision.** The reader tags exactly the *ended-too-early* parse errors — EOF
inside a form, EOF inside a string — with the stable code
`error_codes::INCOMPLETE_INPUT` (`"E0002"`), via `err_incomplete` /
`err_at_incomplete` (`syntax/reader.rs`). Every other parse error keeps its own
code. Consumers match the **code**, not the message, to decide "needs more input":
- a structured caught error is a map `{:kind :message :code …}` (per `try`/`catch`,
  `docs/llm-native.md` §4), so `(= (get e :code) "E0002")` is the whole test;
- `eval-string` reads *all* forms before evaluating any, so an incomplete buffer
  throws at read time with **nothing evaluated** — the consumer can safely retry
  the whole growing buffer as more lines arrive, with no partial/double effects.

`std/repl.blsp` uses this for line-at-a-time multi-line entry (`repl--incomplete?`).
The same signal is what a future editor's eval-region / structured-editing layer
will read; keeping it a reader-owned, code-tagged fact (not consumer-side
delimiter counting) is what makes those reuses correct for free.

**Consequences.**
- Multi-line REPL input needs no delimiter scanner in Brood; correctness (strings,
  comments, escapes) is the reader's, single-sourced.
- `INCOMPLETE_INPUT` is now a **contract**: the reader must keep tagging only the
  genuinely-incomplete cases with it, and must not reuse `E0002` for other parse
  errors. (It predates this ADR — the code and the `err_incomplete` helper were
  already there "so a REPL / editor can distinguish"; this records the decision and
  its first real consumer.)

**References.** ADR-048 (the self-hosted REPL, first consumer), `docs/error-codes.md`
(the stable code registry), `docs/llm-native.md` §4 (structured caught errors as
maps), CLAUDE.md "Keep the language as small as possible" (a reader fact reused, not
a scanner re-implemented).

## ADR-050 — Randomness is a pure, threaded PRNG (bitwise ops are the only new primitives)

**Status:** accepted (2026-05-29). Prompted by `docs/feedback-retro-game-of-life.md`
§1/§4 — "no randomness anywhere in the language" was the single biggest ergonomic
gap an AI assistant hit building a simulation.

**Context.** Almost every language ships a global, stateful RNG: `rand()` mutates a
hidden seed. Brood is immutable (ADR-026) — there is no global mutable cell to hold a
PRNG state, and adding one would be a mutation primitive we've sworn off. The
feedback author hand-rolled a glibc LCG and *threaded the seed through the game
state* — and noted that's "the idiomatically-correct immutable answer." So the
language already pointed at the right shape; it was just missing the batteries.

**Decision.** Randomness is a **pure, seedable, threaded** facility, written in Brood
(`std/prelude.blsp`), not a Rust builtin and not a process-backed mutable `*rng*`:
- Every step takes a seed and returns `[value next-seed]`; the caller threads
  `next-seed` into the next call (in loop state, process state, wherever). `rng`,
  `rand-int`, `rand-float`, `shuffle`, `sample`, `rand-seed`.
- The generator is Marsaglia **xorshift32**. xorshift32 specifically, because Brood
  integer `+`/`*` **error on overflow** (they don't wrap, ADR — see `num_bin`): a
  64-bit PRNG (SplitMix64, PCG) needs wrapping multiply/add we don't have, whereas
  xorshift32's shifts stay well within i64 and mask back to 32 bits, so it composes
  from the primitives we *do* have.
- The **only** new Rust primitives are the **bitwise ops** (`bit-and`/`-or`/`-xor`/
  `-not`/`-shift-left`/`-shift-right`). These are genuinely irreducible (can't be
  bootstrapped from the numeric ops) and are independently table-stakes (hashing,
  flags). Everything stochastic is then Brood on top — exactly the ADR-006 split.

**Rejected alternatives.**
- *A Rust `rand` builtin / global PRNG.* Fast, familiar, but reintroduces hidden
  mutable state (violates ADR-026) and moves behaviour into Rust that the language
  can express itself (violates ADR-006). A non-starter on both counts.
- *A process-backed `*rng*`* (a green process holding the seed, queried by `send`).
  This *is* the immutable way to get an ambient generator, and may come later for
  scripts that don't want to thread — but it's the powerful-but-complex form;
  ADR-011 says ship the simple threaded form first and defer the rest until a
  concrete need justifies it.
- *A cryptographic generator.* Out of scope — xorshift32 is for simulations,
  sampling, shuffling, jitter, and ids; the docstrings say so explicitly.

**Consequences.**
- Determinism for free: same seed → same stream, which makes stochastic code
  **testable** (the PRNG suite asserts exact streams, including across a `send`
  deep-copy) and reproducible — a property a hidden global RNG can't offer.
- The threading is visible in the types (`[value next-seed]` everywhere), which is
  more ceremony than `(rand)` but is the honest cost of purity, and reads naturally
  once state is already threaded (as it is in any Brood loop/process).
- If a future need for an ambient generator appears, the process-backed `*rng*` is
  additive over this — it would *use* these same pure steppers internally.

**References.** ADR-006 (write the language in the language — bitwise primitive,
stochastic policy in Brood), ADR-026 (immutability — no global mutable PRNG),
ADR-011 (ship the simple form, defer the process-backed one),
`docs/feedback-retro-game-of-life.md` §1/§4, `docs/language.md` (Bitwise, Randomness).

## ADR-051 — `(process-info pid)` as the kernel introspection snapshot

**Status:** accepted (2026-05-29). The introspection surface a process observer /
debugger / supervisor reads; first consumer is `nest observe`.

**Context.** The observer (and any process-management tool) needs per-process
state — status, registered name, mailbox depth, memory, parent, who's monitoring
it. None of it is reachable from Brood: a `Process` lives inside its coroutine (or
the mailbox `waiter` slot when parked), not in any Lisp value; the registry,
name, and monitor tables are all Rust internals. So this is irreducibly kernel
*mechanism* (the ADR-006 split puts it in Rust), but the *shape* exposed to Brood
is a plain immutable map the language manipulates freely.

**Decision.** One primitive, `(process-info pid)`, returns a snapshot **map** for a
live local process (Erlang's `process_info/1` shape), or `nil` for a remote/dead
pid (a non-pid is a type error — same contract as `mailbox-size`):

```
{:id <int> :node <kw> :name <kw|nil> :status <kw> :mailbox <int> :monitored-by <int>}
```

- A **single map primitive**, not granular accessors. The fields are all
  kernel-internal and naturally read together; a map is the Erlang-idiomatic,
  one-call shape, and the cheap-snapshot semantics (read now, immutable copy) suit
  it. (`mailbox-size` stays as the one-field fast path it already was.)
- **Built from independent one-lock reads.** Each field comes from a `process.rs`
  accessor that takes exactly one lock and releases it before the next
  (`mailbox_len`, `process_status`, `monitored_by`, `dist::name_for_pid`,
  `is_alive`); `process-info` calls them in sequence holding no two at once, so it
  adds no lock-ordering risk and tolerates a process changing state mid-read
  (a stale-but-coherent snapshot, fine for display).
- **`:status` is inferred, for now, with no new bookkeeping:** parked in `receive`
  (the mailbox holds it in its `waiter` slot) → `:waiting`, else `:running`; dead →
  the whole call is `nil`. An explicit per-process state enum (in-flight kernel
  work) will replace the inference and may widen the vocabulary (`:runnable`).
- **Incrementally extensible — now full.** The map's key *set* grew monotonically
  as the kernel exposed more; all fields are backed via **registry-reachable cells
  on the `Mailbox`** (the `Process` itself isn't reachable while it runs):
  - `:parent` — a `pid → parent` side table (spawner recorded at `spawn`, dropped
    at `deregister`).
  - `:status` — a real enum (`:running` / `:runnable` / `:waiting`) read from an
    `AtomicU8` the scheduler sets at each transition (`enqueue` → runnable,
    `run_one` → running, `wait_for_message` → waiting; covers root and green),
    replacing the earlier `waiter`-slot inference (which couldn't see `:runnable`).
  - `:memory` — the process's LOCAL heap footprint (`Heap::local_bytes`, an
    estimate from slab `len × size_of`), republished to an `AtomicUsize` each time
    the process enters `receive`. Bump-allocated, so it shows allocation since the
    last reset / `hibernate` (an *accumulation* signal, not a GC live set — there
    is no tracing GC; ADR-016/048). A process that never `receive`s reports `0`.

**Consequences.**
- The numeric `:id` is monotonic (it's the spawn counter), so it doubles as a
  **stable sort key** — the observer now lists processes in spawn order
  deterministically (it previously had no orderable pid handle and fell back to
  busiest-mailbox-first).
- A pid's numeric id is now reachable from Brood (via `:id`) without string-parsing
  its printed form — useful beyond the observer.
- Keeping the snapshot a map (not a process-backed query object) means it's
  `send`-able, comparable, and testable like any value; the `:isolated` tests
  assert it across spawned processes.

**References.** ADR-006 (mechanism in Rust, the map is policy-shaped data), ADR-046
(the observer, first consumer), ADR-026 (the snapshot is an immutable value),
`std/observer.blsp`, `docs/primitives.md` (the `process-info` entry).

## ADR-052 — Interactive REPL line editor in Brood (inline `term-*` seam)

**Status:** accepted (2026-05-29). The syntax-highlighting, bracket-matching,
signature-hinting, completing, emacs-keyed REPL editor — `std/lineedit.blsp` +
`std/highlight.blsp` over a thin new inline `term-*` seam.

**Context.** ADR-048 made the REPL a Brood loop over `read-line`, with line editing
left to the terminal's cooked mode and an explicit note that richer editing was "now
a Brood function to add, not Rust," over the `term-*` raw-key seam. This ADR adds it:
tree-sitter-style lexical highlighting, matching-bracket emphasis, function signature
hints, Tab completion, and the core emacs/readline keys + ↑/↓ history. The existing
`term-*` primitives (ADR-046) were built for a *full-screen* TUI (`nest observe`):
`term-enter` takes the **alternate screen** and `term-draw` paints **absolute** cells
— both wrong for a REPL, which must render **inline** and keep scrollback.

**Decision.**
- **A thin inline seam in Rust, the editor in Brood** (the ADR-006 split). Three new
  primitives: `term-raw-enter` / `term-raw-leave` (raw mode *only* — no alternate
  screen, cursor stays visible, scrollback preserved; `restore_raw` is the
  panic-path backstop, and unlike `restore_terminal` it emits no escape sequences so
  a piped stdout stays clean) and `term-emit` (a vector of *relative*-motion ops —
  `:print`/`:cr`/`:nl`/`:up`/`:down`/`:col`/`:clear-eol`/`:clear-below` — queued then
  flushed once, sharing `term-draw`'s `apply_face`). `key_to_value` also learns the
  ALT modifier (`:alt-f` …, for M-f/M-b) and `BackTab` (`:back-tab`). Everything an
  editor *does* — keymap, kill-ring, history, completion, layout, highlighting —
  lives in Brood (`std/lineedit.blsp` + the pure `std/highlight.blsp`), redefinable.
- **Lexical highlighting, written in Brood.** `std/highlight.blsp` is a pure
  source→data lexer (the `observe-frame` discipline): it classifies tokens by shape +
  head-position (the first symbol after a `(` is a call / special form), not by
  resolving bindings — cheap, robust on incomplete input, and unit-testable without a
  terminal. The special-forms set comes from the `(special-forms)` primitive — the
  canonical Rust `SPECIAL_FORMS` (moved into the `brood` lib), which the LSP
  (`semantic_tokens`/`completion`) now imports too, so the runtime, the highlighter,
  and the LSP share one list and can't drift.
- **Single-line editing, whole-form analysis.** The editor edits one physical line
  and returns it — a `read-line` drop-in — so multi-line forms keep coming from the
  REPL's existing reader-driven accumulation (ADR-049), with no second incomplete-
  detector in Brood. The already-typed accumulator threads in as read-only `:prefix`
  context, so highlighting, bracket matching, and signature hints analyse the *whole*
  form (`prefix + line`) even on a continuation line, while cursor math stays
  one-dimensional. A long line **horizontally scrolls** rather than wrapping (wrapping
  would turn one logical line into many rows and break that math); the signature hint
  renders on the line *below*, and because all motion is relative a bottom-of-screen
  scroll moves the input and hint together (no absolute-row assumptions).
- **The keymap is data; commands are redefinable functions.** `*lineedit-keymap*` is
  a plain map of `key → command-symbol`; each command is a public global
  `(fn (state key) -> state)` (`lineedit-beginning-of-line`, `lineedit-kill-line`, …).
  `lineedit--handle` looks the key up and resolves the symbol *late* (`(eval sym)`), so
  **both** override paths work from a running REPL: rebind a key
  (`(lineedit-bind :ctrl-x 'cmd)` / re-`def` the map) or redefine a command's function
  — each takes effect on the next keystroke (the project's hot-reload model). Keeping
  the keymap symbols-not-closures keeps it pure data (promotable/sendable); a buggy
  binding is caught so it can't crash the read. This is the editor's keymap seam: the
  same shape the full editor's keymaps will use. Common emacs/readline keys are bound —
  C-a/C-e, C-f/C-b, M-f/M-b, C-k/C-u/C-w, M-d, C-y, C-t, C-h, C-d, C-l, Tab, ↑/↓ and
  C-p/C-n. Ctrl-D on an empty line signals EOF; mid-line it deletes forward; Ctrl-C
  abandons the form and re-prompts.
- **Pure keymap + thin IO loop.** Commands and `lineedit--handle` are pure
  `(state, key) → state` (the late symbol resolution aside), so the whole keymap is
  tested without a TTY; only `lineedit--loop` polls keys and paints (exercised
  manually, like `repl`/`observe`). C-l is the one command needing IO (a screen
  clear): its command just sets a `:clear` flag that the loop honours via a new
  `term-emit` `[:clear-screen]` op, keeping the command itself pure.

**Where the editor runs (and why the worker cost is a non-issue).** The editor polls
keys with `term-poll` from inside the *spawned* `repl--loop` process — the process that
`hibernate`s between forms to bound memory (ADR-048). `term-poll` natively blocks its
worker thread for the poll timeout, so the REPL's one worker is unavailable while it
idles at the prompt. Given the scheduler (`scheduler.rs`: ≈`nproc` workers, processes
pinned to a worker for life, per-worker queues, **no work stealing**), this is benign:
(1) only the REPL's *one* worker is involved; (2) a blocked worker only affects
processes pinned to *that* worker, and `assign_worker` is least-loaded, so fresh spawns
steer to idle workers — usually nothing else is co-located; (3) the finite (250 ms)
timeout yields the worker periodically, so even a co-located process still gets slices
(no deadlock); and (4) it's *better* than the old `read-line`, which blocked the same
worker **indefinitely** until a full line arrived — the editor yields every ≤250 ms.
Only the degenerate single-worker pool (`-j 1`) is meaningfully affected, and even
there background work proceeds in slices. **Rejected:** a root↔spawned round-trip that
moves the read to the (never-blocking) root process — it removes the already-benign
block but pushes the editor's per-keystroke transients onto the root arena, which
*cannot* `hibernate` → unbounded growth over a long session; a real cost for an
imaginary one. A **scheduler-parking key read** (suspend the green process until a key
is ready, like `receive`) would make the block truly zero-cost — a nicety, not a fix.

**Consequences.**
- The REPL is now a genuinely modern prompt, entirely in Brood — the editor for the
  coming text editor (M2+) starts here, on the same seam.
- `term-emit`'s relative ops are the inline counterpart to `term-draw`'s absolute
  frame; both share `apply_face`, so a future remote/web frontend interprets one more
  small op set.
- Piped (non-TTY) input is untouched: the editor is gated on **stdin** being a TTY
  (`(and (stdin-tty?) (stdout-tty?))` — a new `stdin-tty?` primitive), so
  `echo … | brood` *in a terminal* (piped stdin, TTY stdout) correctly takes the
  plain `read-line` path instead of blocking the editor on key events; cosmetic
  prompts/banner stay gated on `stdout-tty?`.
- Follow-ups since shipped: `(special-forms)` de-drift (done — above); **persistent
  history** (`$BROOD_HISTORY`/`~/.brood_history`, loaded on start, saved capped per
  submit — `std/repl.blsp`); and **reverse incremental search** (C-r, a `:search`
  sub-mode in `std/lineedit.blsp`). The keymap was also generalised into a shared
  `std/keymap.blsp` (`keymap-dispatch`), the input-side counterpart to the display
  seam, now used by both the editor and `observe`.
- Remaining limits (all additive follow-ups): a scheduler-parking key read (makes the
  benign worker block above truly zero-cost); lexical (not scope-aware) highlighting;
  completion from globals only (no locals-in-scope); display width approximated as one
  column per char (wide CJK/emoji may misposition the cursor).

**References.** ADR-048 (the self-hosted REPL this extends), ADR-049 (the reader's
INCOMPLETE_INPUT multi-line signal the single-line model relies on), ADR-046 (the
full-screen `term-*` seam this adds an inline counterpart to), ADR-006 (mechanism in
Rust, policy in Brood), ADR-025 (`arglist`/`global-names` introspection the hints +
completion read; `semantic_tokens.rs` SPECIAL_FORMS the highlighter mirrors),
`std/lineedit.blsp`, `std/highlight.blsp`, `std/repl.blsp`, `docs/primitives.md`.

## ADR-053 — Remote attach: observe a running runtime over the node link

**Status:** accepted (2026-05-29). The way to watch *existing executing code* — the
real use for the process observer, since one terminal can't show app + observer.

**Context.** `observe-attach` watches *this* runtime; to watch a separately-running
program you must attach from a second terminal, which means IPC between two OS
processes. Brood's only cross-runtime channel is the **distributed node link**
(`dist.rs`: TCP + shared-cookie handshake) — and it's the right one: it gives
location-transparent `send`/`receive`, and `process-info` already returns a
**send-able immutable map**. A bespoke socket would mean new Rust primitives +
re-doing the node wire codec for nothing.

**Decision.** Remote attach is the **same observer loop with a remote data source**
— no kernel changes, no new wire format.
- **Target side, `(observe-serve)`:** spawn an agent and `register` it as
  `:observe`; it replies to each `[:snapshot from _]` with `(observe--local-snapshot)`
  (`{:node :procs}`) — the *same* snapshot the inline observer renders — sent to the
  requester's pid, which routes back over the link. Opt-in (errors unless the program
  has `node-start`ed), exactly like Erlang's `-name`: a program isn't observable
  unless it opens itself up.
- **Observer side, `(observe-connect spec cookie)` / `nest observe --connect`:**
  `node-start` a unique transient node, `connect` the peer *before* `term-enter` (so a
  refused / wrong-cookie / bad-spec error — all clean `LispError`s — surfaces without a
  wrecked screen), `monitor-node` it, then run `observe--loop` with a source that
  requests a snapshot per frame. The **node panel shows the peer's** stats because the
  snapshot now carries `:node` (the source unification — the loop reads node + procs
  from the snapshot, not from a local call).
- **Pluggable source + link status.** A source returns a snapshot map, or a status
  keyword. `observe--apply-result` folds it into `{:last :link}`: a map → `:ok`;
  `:timeout` (stalled link / no agent) → `:stale` keeping the last snapshot;
  `:down` (link dropped, via `[:nodedown]` or socket close) → **sticky** `DISCONNECTED`
  frozen on the last snapshot until the user quits. So the UI never hangs on the
  network and never crashes on disconnect — it shows the state.
- **Cookie (decided): explicit.** `--cookie` → `$BROOD_COOKIE` → a clean error; no
  baked-in default (a default cookie on a listening node is a footgun). A short
  per-frame request timeout (`*observe-timeout*` ≈ 800 ms) keeps a slow link showing
  `stale` rather than blocking the key loop; stale replies are drained so a flaky link
  can't grow the mailbox.

**Consequences.**
- Watching a running CLI/server is now "open a second terminal and
  `nest observe --connect`," the Erlang-observer model. Same `observe-frame`, same
  `process-info` — the observer renders identically whether the data is local or a
  peer's, which is the protocol-not-library property the display seam (ADR-046) set up.
- **Trust model is dev-grade** (inherited from `dist.rs`): shared cookie, **no
  encryption**, no per-message auth — LAN/trusted networks only; an internet-facing
  attach needs TLS on top. Read-only: the observer reads snapshots, it can't control
  the peer's processes (kill/inspect is a deliberate non-goal for now).
- Cross-node coverage in `crates/cli/tests/observe_attach.rs` (two real runtimes:
  attach → snapshot of the peer's processes → kill target → `:down`).

**References.** ADR-046 (the display seam / observer this extends), ADR-051
(`process-info`, the send-able snapshot maps), ADR-034 (the node handshake/cookie),
ADR-006 (mechanism in Rust, the agent + loop are Brood), `std/observer.blsp`,
`docs/roadmap.md` M3.

## ADR-054 — Generational handles: a debug tripwire for use-after-GC

**Status:** accepted (2026-05-29). The debugging/safety foundation for re-enabling
automatic collection (Stage B, `docs/memory-review.md`). Representation +
per-process epoch wiring landed; the deref check is debug-only.

**Context.** A Brood handle is an index into a per-process typed slab `Vec`, not a
raw pointer (for `Send` + the planned arena migration, ADR-002). That makes a
*stale* handle — one held across an arena flip (`(hibernate)` → `Heap::flush`
today; the future safepoint `collect`) without being re-rooted — pathological to
debug: the slab memory is still valid, so the bad access is either an
out-of-bounds index that panics **far from the cause** (e.g. deep in `pair()` with
"len 143 index 274"), or, worse, a **silent read of the wrong object** once the
slab has regrown past that index. Valgrind/heaptrack can't see it (no native
invalid read). A prototype copying collector at the eval safepoint surfaced
exactly this, repeatedly, as the dominant cost of doing GC work. The boolean
`PoisonBits` tripwire can't catch it either: it's cleared on flush and can't
distinguish a reused slot from its previous occupant (no ABA detection).

**Decision.** Carry a **generation stamp** in every handle and check it at the
LOCAL deref.
- **Representation.** Handles widened `u32 → u64` (free — `Value` already has
  8-byte payloads via `Int`/`Float`/`Ref`): region (2 bits) + **generation
  (30 bits)** + index (32 bits). `EnvId::GLOBAL` = `u64::MAX`. **Equality and
  hashing mask the generation** (`canonical()`), so a handle is still "the same
  object" across epochs — the stamp only gates *derefs*, never identity.
- **Per-heap epoch, not per-slot.** The allocator is bump-only (it never reuses a
  slot), so the *only* event that invalidates a LOCAL handle is a whole-arena
  flip. A single `Heap::local_epoch` therefore suffices: `arena_flip` bumps it
  before copying, every `alloc_*` stamps the current epoch, and the flush helpers
  re-mint survivors with the new epoch (carried on `FlushForward`, not threaded).
  Forward-compatible: when a future collector reuses slots, the stamp becomes a
  per-slab generation table (the `slotmap` pattern) with no handle-shape change.
- **Debug-only check.** A `debug_assert!` in each LOCAL accessor compares
  `handle.generation()` against `local_epoch` and panics **at the bad deref** with
  the slot and both epochs. Release builds carry the stamp but skip the check
  (zero cost — same philosophy as the `PoisonBits` it supersedes).

**Consequences.**
- Use-after-flip is now a precise, located panic, not a far-away bounds error or a
  silent wrong-slot read — the tool that makes Stage B (and `(hibernate)` misuse)
  tractable to debug. Proven by `gen_handle_tests` (the tripwire fires; a flushed
  *root* stays valid) and by the full suite (746 tests, which hibernate per step →
  thousands of flips) green under `debug_assertions` with **no** false positive.
- Natives and the `global` sentinel need no stamping: natives are PRELUDE at
  runtime (LOCAL only during the builder, epoch 0), and `Heap.global` is the
  `EnvId::GLOBAL` sentinel at runtime (the `local(0)` initializer is builder-only,
  which never flips).
- **Limitation:** per-heap granularity catches use-after-flip, not per-slot reuse
  (there is none yet); and `reset_local_to` deliberately doesn't bump the epoch
  (it would false-positive below-checkpoint survivors), so the rare reset-regrow
  ABA stays a documented gap until per-slot generations land.

**References.** ADR-002 (`Rc`→arena migration, why handles are indices),
ADR-035 (the disabled mark-sweep this helps revive), ADR-026 (immutability — but
`letrec` cycles mean we still need tracing, not pure refcounting),
[`docs/memory-review.md`](memory-review.md) (the full memory model review + the
staged GC plan), [`roadmap.md`](roadmap.md) M1.

## ADR-055 — Stage B: automatic copying collection at the eval safepoint

**Status:** accepted (2026-05-29). Re-enables automatic per-process GC, on the
generational-handle foundation (ADR-054). The "slow-and-stable" memory the brief
asked for; supersedes the disabled mark-sweep (ADR-035) and the manual-only
`(hibernate)` reclamation.

**Context.** `docs/memory-review.md` mapped the fork: **copying** at the safepoint
(reuses the proven `(hibernate)` `arena_flip` + the per-heap epoch; one unified
collector; but *moves* every object, so any Rust frame holding a handle across a
collection goes stale) vs. **non-moving mark-sweep** (live handles don't move, but
needs new per-slot generation tables and a two-collector design). With the
generational tripwire (ADR-054) now making a stale handle a *precise, located*
panic, copying's footgun became a bounded, test-caught fix list rather than a
silent landmine — so copying won.

**Decision.** When `gc_due()` and `gc_block_depth() == 1` (outermost eval), fire a
semi-space **copying** collection via the shared `arena_flip`: relocate everything
reachable from `expr`/`env`/dynamics/the explicit root stack into fresh slabs, drop
the rest, bump the epoch. The adaptive threshold (`max(floor, 2×live)`) is the
slow/stable dial; `BROOD_GC_STRESS=1` collects maximally (correctness fuzz).

The "everything moves" footgun was closed at its (few, enumerable) sites:
- **`eval` loop** writes back the relocated `expr`/`env` after `collect`.
- **`eval_str`/`eval_source`** re-fetch each form from the relocated root stack
  (`root_at`) instead of their own now-stale `Vec`, and **skip the per-form arena
  reset when GC is on** (a copy invalidates the `checkpoint`; GC reclaims instead).
- **the type checker** brackets itself in `GcBlockGuard` so its `(require …)` evals
  never collect mid-walk (it holds Rust-`Vec` handles across them).
- **`flush_pair` made iterative** down the cdr spine — a long list must not recurse
  its length deep in the collector (an uncatchable SIGABRT); mirrors `promote_list`.
- **`form_pos` re-keyed** through the pair forwarding table on every flip, so a
  collection mid-file-load doesn't drop the reader positions error messages need.

**Consequences.**
- A never-returning, non-hibernating loop is now memory-bounded automatically (a
  100k-iteration allocating loop: ~10 MB, was unbounded). Hot reload is unaffected
  — GC only touches the per-process LOCAL heap, never the shared RUNTIME code/global
  region where `def`s live (and it *reclaims* the LOCAL transient a `def` builds
  before `promote` copies it to RUNTIME). Node connections are unaffected — messages
  cross as serialized deep copies, reconstructed via `alloc_*` (correctly stamped).
- **Immutability shortcut already banked:** no write barriers (data never mutates).
  The generational nursery (Stage C, **now landed** — ADR-072) builds on this: a
  minor GC copies just the nursery survivors and never traces the old generation,
  because immutability ⇒ no old→young pointers. *Almost* no barrier — the one
  exception is a frame tenured **mid-bind** (a collection during a `let`'s rhs,
  then bound further), which `env_define` records in a one-entry remembered set; the
  next minor scans it. (Cycles still exist via `letrec`, so tracing — not pure
  refcounting — remains required; ADR-026/054.)
- A debug-only diagnostic (`debug_walk_env_chain`, the poison-era env walk
  superseded by the tripwire) was found mis-walking RUNTIME indices into the LOCAL
  slab and made debug builds pathologically slow; gated behind `BROOD_ENV_DEBUG=1`.
- Validated: suite 765/765 + `gc.rs` (collector active); `basic.rs` 75/75 under
  `BROOD_GC_STRESS=1`; release bounded + fast.

**References.** ADR-054 (generational handles — the tripwire this relies on),
ADR-035 (the disabled mark-sweep this replaces), ADR-016 (the arena reset it
supersedes under GC), ADR-026 (immutability — no write barriers; but `letrec`
cycles), [`docs/memory-review.md`](memory-review.md) (the full plan + the fork),
[`roadmap.md`](roadmap.md) M1. Stage C (generational nursery) deferred.

## ADR-056 — A windowed (GUI) frontend + mouse input, on the same display seam

**Status:** accepted (2026-05-29). The second frontend for the ADR-046 seam, and
the realisation of its deferred mouse/scroll input. (The window itself first
landed in the same commit as ADR-055 without its own ADR; this records both the
GUI decision and the input completion.)

**Context.** ADR-046 made the display layer a *protocol of render-op data*, not a
library, and deferred "mouse/resize events" and additional frontends as additive.
The claim that a frontend is "just another implementer of the protocol" was only
ever exercised by one frontend (the `crossterm` terminal), so it was unproven. And
the observer was keyboard-only — fine for a TUI, but a window invites a pointer.

**Decision.** Add a **native window frontend** as a peer of `term-*`, and extend
the seam's *input* half with a mouse event — both as additive `gui-*` primitives
and a new render-op-protocol input shape, with zero change to the frame protocol.

- **A frontend is five primitives, again.** `gui-open`/`gui-close`/`gui-size`/
  `gui-draw`/`gui-poll` mirror `term-*` and paint the *identical* frame vector
  (`crate::gui`, behind the `gui` cargo feature: `winit` owns the event loop,
  `softbuffer` a CPU framebuffer, `fontdue` a monospace glyph grid). The same pure
  `observe-frame` therefore paints to a window or a terminal unchanged; a
  `display-broadcast` can still drive several frontends from one frame. Without
  `--features gui` the primitives return a clear "rebuild with --features gui"
  error, so the symbols exist uniformly either way.
- **Many windows, one event loop.** winit allows only *one* event loop per process,
  so a single GUI thread owns it and multiplexes a *registry* of windows. `gui-open`
  returns an integer window id and the other primitives take it (vs the single
  terminal's 0-arg `term-*`); `*gui-display*` is therefore a `(gui-display)`
  *constructor* that opens a window and closes the `gui-*` over its id. This is what
  lets `(observe)` open several independent windows. The id keeps the Brood side
  from depending on winit's opaque `WindowId`; the thread maps between them.
- **Mouse is one new input value, shared by both frontends.** `term-poll`/
  `gui-poll` may now also yield `[:mouse action button row col]` (`action`:
  `:press :scroll-up :scroll-down`; `button`: `:left :right :middle` or nil;
  `row`/`col` 0-based cells) — the same encoding from both, so one keymap/handler
  drives either. The crossterm frontend enables mouse capture in `term-enter` only
  (not the inline REPL `term-raw-enter` seam, which must keep the terminal's own
  text selection). The GUI thread reports it from winit's button/wheel events,
  translated to the same cell coordinates (it tracks the pointer on cursor-move but
  does not *emit* bare motion — see below).
- **A deliberately minimal vocabulary** — exactly what a consumer needs today: a
  click and the wheel. Release / drag / bare motion are dropped at *both* backends
  (crossterm maps them to a nil poll; the GUI tracks the cursor on move but emits
  nothing), so the two frontends surface an identical set, and the observer never
  wakes for an event it would ignore. This avoids a real footgun: winit's
  `CursorMoved` fires per pixel, and since the observer refetches+redraws on every
  poll result, *emitting* motion would turn a mouse wiggle into a redraw storm.
  Release/drag are additive when a consumer (drag-select) needs them (ADR-011).
- **The observer acts on two.** `std/observer.blsp` reacts to left-press (select the
  clicked process row) and the wheel (scroll the selection); a right/middle click,
  a click off the list, or any future action is a no-op. The mapping is **pure**
  (`observe--mouse-row->sel`, `observe--apply-mouse`) and unit-tested without a
  window, consistent with the keyboard commands being pure `(state key) → state`.
- **`(observe)` is non-blocking; one process per window.** To open several windows
  by calling `(observe)` repeatedly it can't be modal, so it `spawn`s a process that
  opens a window and runs the loop, returning that pid. Each window is independent
  state in its own process. The trade-off vs ADR-046's root-process observer: a
  spawned observer blocks on `gui-poll` in a *green* process, pinning a scheduler
  worker for the poll interval (native blocking can't be preempted). Fine for a
  handful of windows (≈`nproc` workers); opening as many observers as workers would
  starve other processes for up to a poll interval. Acceptable now (ADR-011);
  `(observe-attach …)` stays modal for the single-window/terminal case.
- **Same GUI-thread bridge as ADR-046.** Only `Send` plain data (`Op`/`Input`)
  crosses the channels; the windows/surfaces/glyph caches never leave the GUI
  thread. Clicking a window's close button surfaces as a dedicated `:close`
  message to that window's input — distinct from the Escape *key* (`:escape`) so
  an app can quit on the X without conflating it with Escape (which an editor binds
  to cancel/normal-mode) — so its Brood loop tears down (and calls `gui-close`) on
  its own terms. `ui-run` quits on `:close` automatically; a raw `receive` loop
  matches it (or uses `ui/quit-request?`). (Earlier this was delivered as `:escape`;
  the conflation made any Escape-binds-cancel app uncloseable by its X button.)

**Consequences.**
- Three optional deps (`winit`/`softbuffer`/`fontdue`), all gated behind `gui`; a
  default build links none. They're runtime-substrate (the "drawing, I/O" Rust
  category ADR-006/046 anticipated) — the Lisp-callable surface stays Brood.
- `back-tab` (Shift+Tab) is now translated by the GUI too, matching the terminal,
  so the key vocabularies are aligned across frontends.
- The `gui-*` primitives gained a window-id argument (a breaking change from the
  initial 0-arg shape — fine pre-1.0); `*gui-display*` became the `(gui-display)`
  constructor. `(observe)` now returns a pid instead of blocking.
- Still deferred (ADR-011): a `gui-raw-*` inline seam (so the self-hosted REPL can
  run in a window, not just the observer), runtime font sizing, and attaching a
  frontend to a *remote* live image. A spawned observer pins a worker while polling
  (above). No automated GUI test (it needs a live display); the pure input mapping
  is tested, the backend is smoke-tested by hand (two windows at once).

**References.** ADR-046 (the display/input seam this extends — and whose mouse
deferral this closes), ADR-011 (ship the simple form), ADR-006 (drawing/I-O as a
Rust-primitive category), ADR-043 (root-vs-worker thread + finite-poll model),
[`roadmap.md`](roadmap.md) M3.

## ADR-058 — Automatic GC reaches every entry path; `(hibernate)` removed

**Status.** Accepted (2026-05-29). Completes ADR-055 (Stage B) and supersedes the
Stage-A `(hibernate)` expedient from `docs/memory-review.md`.

**Context.** Stage B (ADR-055) made copying collection automatic at the
`gc_block_depth() == 1` eval safepoint. But "done" hid a trap: the safepoint only
fires at depth 1, and how a program is *entered* decides its depth. `nest run
<file>` launched the program via the `(load "path")` builtin, which re-enters
`eval` for each form while the `(load …)` frame is still on the stack — so the
whole program ran at `gc_block_depth >= 2`, the safepoint never fired, and a
long-running loop climbed ~100 MB/s (the Game-of-Life §8 leak,
`feedback-retro-game-of-life.md`). `brood <file>` never leaked because its
`eval_source` form loop runs each top-level form at depth 1. So identical code
leaked or didn't depending purely on the launcher — a violation of the project
rule that **a Brood author must never have to reason about GC**.

**Decision.**
1. **Make `load` bounded in the core, not per-tool.** When `load` is the outermost
   eval (`gc_block_depth() == 1` — a top-level form or a spawned-process body) it
   evaluates the file's forms through the same depth-1 rooted form-loop as
   `Interp::eval_source`: a `GcBlockReset` guard drops the block depth to 0 so each
   form re-enters at the safepoint, and the unevaluated forms are rooted across
   each collection (re-fetched via `root_at`). Called deeper (`(cons (load …) xs)`)
   it falls back to inline eval — a library load that doesn't loop, so it never
   crosses the threshold. Because the fix lives in `load`, *every* entry path —
   `brood`, `nest run`/`--watch`/`--for`, MCP `eval`, the future editor — inherits
   the bound for free; no launcher special-cases it. (`nest run`'s short-lived
   `eval_source` workaround was reverted.)
2. **Remove the `(hibernate)` primitive entirely.** With automatic collection now
   reaching every normal entry path (every long-lived loop is a top-level form or
   a spawned-process body, both at depth 1), the manual flush is redundant. Gone:
   the `hibernate` builtin, the `ErrorKind::Hibernate` unwinding sentinel +
   `hibernate_args` carrier (shrinking `LispError` on the hot `Result` path), and
   the scheduler's catch-and-flush loop. `std/test.blsp`'s runner and
   `std/repl.blsp`'s loop became plain tail calls; the `gc.rs` / `blob_share_test`
   cases that asserted hibernate semantics now drive Stage B directly.
   `Heap::flush` survives as a tested arena-flip helper.

**Safety.** Resetting `GC_BLOCK` inside `load` is sound only at depth 1: the sole
outer frame is the `(load …)` combination, whose `expr`/`call_form` are read only
by `or_form_pos` via `id.index()` (a bit-extract, no slab deref → no tripwire) and
only when the error lacks a position, which it never does here. Validated under
`BROOD_GC_STRESS=1` + `debug_assertions` (every-safepoint fuzz, generational
tripwire armed): `--for` loop and require/load-heavy suites stay green; a
life-style loop went from 0 collections / 1.16 GB to 166 / ~5 MB.

**Known limit.** A loop running several eval frames deep (e.g. invoked from a
non-tail position inside `load`-ed non-entry code) still won't be collected — the
depth-1 safepoint can't reach it. The general fix is the deferred operand-stack VM
(collect at any depth, `memory-review.md` §6); it is not reachable by any normal
program structure, so no escape hatch is retained.

**References.** ADR-055 (Stage B), ADR-054 (generational handles — the tripwire
this leans on), ADR-035 (the per-process GC model), ADR-048 (the REPL loop that
dropped its `(hibernate)`), [`memory-review.md`](memory-review.md) §6,
[`memory-model.md`](memory-model.md), and the §8 resolution in
[`feedback-retro-game-of-life.md`](feedback-retro-game-of-life.md).

## ADR-059 — Blocking work delivers to a mailbox; it never pins a worker

**Status:** accepted (2026-05-29). Phase 1 (GUI observer input) implemented; the
general pattern (terminal, sockets, an offload pool) is planned —
[`handoff-blocking-io.md`](handoff-blocking-io.md).

**Context.** The green scheduler has a small worker pool (≈`nproc`); green
processes are cheap but workers are scarce. A process that makes a **native
blocking call** — `recv_timeout`, a blocking `read`, a synchronous FFI call —
holds its worker for the whole call, since the scheduler can't preempt a thread
parked in a syscall. With multiple windows (ADR-056), each observer blocked in
`gui-poll` pinned a worker; enough of them would block the whole pool while
thousands of other processes starve. The same hazard applies to any future
network or interop call.

A process parked in `(receive)` on an empty mailbox is the opposite: it is
*descheduled* (the mailbox `waiter`), holding **no** worker, until
`mailbox::deliver` wakes it.

**Decision.** Anything that blocks runs on a **non-worker thread** and **delivers a
message to the owning process's mailbox**; the process parks in `(receive)`. This
is not new architecture — it is already the runtime's *network* model (`dist`
reads each `TcpStream` on a dedicated thread and injects via `mailbox::deliver`).
We extend it to GUI input, and adopt it as the rule for blocking work generally.

- **Phase 1 — GUI input (done).** `gui-open` registers the *calling process* as the
  window's subscriber. The GUI thread turns each key/mouse event into a `Message`
  (built off-heap — `Message` is a plain enum, symbols are a global interner) and
  `deliver`s it to that mailbox. `(gui-display)`'s `:poll` becomes
  `(fn (ms) (receive (m m) (after ms nil)))` — park for the next input message, or
  time out for the live-refresh tick. The observer loop is otherwise unchanged
  (same key/mouse shapes), but an idle window now holds **no** worker, so hundreds
  can run at once. `gui-poll` (the blocking primitive) is removed.
- **Already had what we needed**: `mailbox::deliver` (inject + wake from any
  thread), `receive` with `(after ms …)` (the tick — no core change), and a plain
  `Message` enum (off-heap construction). The scheduler pins each process to one
  worker for life with **no migration**, which is exactly why deliver-to-mailbox is
  the right shape — a BEAM-style migrate-to-dirty-scheduler design would be far
  more invasive, while this needs no migration.
- **Phases 2–3 (planned).** Terminal input via a reader thread (lifting even the
  root-thread block ADR-046 predicted); sockets via a `mio` reactor; and a blocking
  *offload pool* (`(blocking (fn () …))`) for unavoidable synchronous calls — all
  the same deliver-to-mailbox shape. See the handoff doc.

**Consequences.**
- The observer's input path is uniform with the rest of the system (it's just
  `receive`), and `(observe)`'s multi-window cost (ADR-056's worker-pinning
  trade-off) is **removed** — idle observers cost nothing.
- `gui-*` no longer has a `poll`; input is a mailbox message. A non-process script
  that wants raw window input opens a window and `receive`s in its own process (the
  root counts).
- `deliver` is unbounded — fine for keys/scroll; sockets will want flow control
  (Phase 2). `%receive` is selective (scans per match) — fine at input rates.

**References.** ADR-056 (multi-window GUI — whose worker-pinning trade-off this
removes), ADR-046 (the display/input seam; predicted async-input-to-mailbox),
ADR-043 (root-vs-worker thread + finite-poll model), ADR-033/034 (the dist
reader-thread → mailbox precedent), [`handoff-blocking-io.md`](handoff-blocking-io.md),
[`roadmap.md`](roadmap.md) M3/M4.

## ADR-060 — Sets are a library over maps; the `#{…}` literal is deferred

**Status:** accepted (2026-05-30). `std/set.blsp` implemented.

**Context.** Building cellular automata / editor code surfaced the want for a set
of values (a Game-of-Life live-cell set is the canonical case). The workaround —
a map `{[x y] true}` whose values are meaningless filler — works but is a *tell*:
it doesn't read as "a set," and there's no `union`/`intersection`/`difference`.

**Decision.** Ship sets as an **opt-in Brood library** (`(require 'set)`), not a
kernel value kind. A set *is* a map of `element → true`. This follows the repo's
prime directive (write the language in the language — ADR-006) and "defer power
features" (ADR-011):

- Because a set is a map, **every existing map/sequence operation already applies**
  — membership is `(contains? s x)`, elements `(keys s)`, size `(count s)`,
  iteration via `fold`/`map`/`into`. The library adds *only* the genuine gaps: a
  deduping constructor `set`, single-element `conj`/`disj`, and the algebra
  `union`/`intersection`/`difference`/`subset?`. Structural equality and vector
  keys come for free from the CHAMP map underneath (ADR-040).
- **Deferred to the kernel, deliberately:** a `#{…}` reader literal, `#{…}`
  printing, and a distinct `set?`/`Tag::Set`. Those need reader, printer, and a new
  `Value` variant (and the type-system bit, GC trace, copy-on-send arms — the full
  compatibility contract in `docs/types.md`). Until a concrete need pulls them in,
  a set is a map, so test it with `map?`. The library API is forward-compatible:
  the function names/meanings survive the eventual literal.

**Consequences.**
- Zero kernel surface, zero new `Value` match arms — the feature lands without
  touching the exhaustive `Value` matches (notably not colliding with in-flight
  kernel work).
- A set and the equivalent `{… true}` map are *indistinguishable* (no `set?`).
  That's the accepted cost of the deferral; it's revisited if/when the literal
  lands.

**References.** ADR-006 (write the language in the language), ADR-011 (defer power
features), ADR-040 (CHAMP map the set rides on), [`roadmap.md`](roadmap.md)
(deferred-features list).

## ADR-061 — Collect at any eval depth via an operand stack

**Status:** accepted (2026-05-30). Implemented.

**Context.** Stage B's automatic copying GC (ADR-055) fired **only at the
outermost eval** (`gc_block_depth() == 1`). The reason was a rooting invariant: a
moving (semi-space) collector must relocate *every* live LOCAL handle, and at the
loop top of the outermost eval the only live transients are the rooted `expr`/`env`
— every inner eval frame's `argv`/`scope`/accumulators sit unrooted on the Rust
stack, so collecting while one is live would strand them. ADR-058 worked around
this for `load` by resetting the block depth so each top-level form re-enters at
depth 1.

But any loop running *below* the outermost eval never reached a safepoint and grew
unbounded (bounded only by the ADR-043 host cap):

- a loop in **argument position** — `(println (gen 0))` runs `gen` at depth 2;
- a **`try`-wrapped** loop — `(try (loop) (catch e …))`, the supervised-server
  shape (the thunk runs via `apply` at depth ≥ 2);
- the **Game-of-Life-via-supervisor** case from the retro: a spawned generation
  loop whose per-generation `mapcat`/`frequencies` churn (all at depth ≥ 2) could
  only be reclaimed *between* generations, spiking RSS to ~1.1 GB.

Measured: a heavy per-iteration loop at depth 1 peaked **131 MB** (collected every
iteration); the identical loop at depth 2 hit **3.5 GB / 0 collections**.

**Decision.** Give the evaluator an **operand stack** so the collector can root
every in-flight LOCAL transient and therefore run at **any** eval depth. The
existing explicit root stack (`Heap::roots`) gains an `EnvId` sibling
(`Heap::env_roots`); both are relocated in place by the copying collector
(`arena_flip`). Every recursive-eval site in `eval/mod.rs` pushes the values it
still needs *after* a nested `eval` — the accumulating `argv`, the cons-spine
cursor, the `callee`, the `call_form`, literal accumulators, `scope`, body forms —
onto these stacks, then re-reads the relocated handles afterwards. The same
discipline covers `bind_params` (`&optional` defaults), `apply_closure`,
`tail_of_cons`, `let`/`letrec` bindings, and the re-entrant builtins (`try`'s
handler; `load`/`eval-string`'s form lists). The safepoint gate changes from
`gc_block_depth() == 1` to "**not in the macro-expansion compile pass**".

The **compile pass opts out instead of being rooted.** `macroexpand_all` holds
partially-built LOCAL forms in unrooted Rust locals; rooting all of `macros.rs`
would be a large, error-prone surface for a path that runs once per top-level form
and allocates little. So a new thread-local `MACRO_BLOCK` (a `MacroBlockGuard`,
saved/restored across coroutine suspend exactly like `GC_BLOCK`/`STACK_BASE`)
suppresses collection during expansion — the brief growth is reclaimed at the next
runtime safepoint, as before. `GC_BLOCK` survives only to feed the stack-overflow
byte guard; it no longer gates GC, and the now-vestigial `GcBlockReset`/`load`
depth-1 trick (ADR-058) is removed.

**Consequences.**
- A loop at *any* depth is now memory-bounded with no author intervention — the
  depth-2 leak repro drops from **3.5 GB → 28 MB**. The retro's spawned-vs-top-level
  spike is gone for the same reason (the mid-generation churn is reclaimable).
- Every function call now pays a few `Vec` push/re-read/truncate operations to
  maintain the operand stack. Correctness over speed for now (ADR-006 dogfooding);
  the hot path can later skip rooting for handles already known non-LOCAL
  (RUNTIME/PRELUDE forms never move) if benchmarks demand it.
- Safety rests on the generational use-after-GC tripwire (ADR-054): a missed root
  panics at the bad deref under `RUSTFLAGS="-C debug-assertions=on"
  BROOD_GC_STRESS=1`. The full suite and a shape battery run clean under it.
- Supersedes the depth-1-only safepoint of ADR-055 and the `load` depth-1 reset of
  ADR-058. `docs/memory-review.md` called this "Model b, the operand-stack VM."

**References.** ADR-055 (Stage B automatic GC), ADR-058 (bounded `load`), ADR-054
(use-after-GC tripwire), ADR-043 (host memory cap), `docs/memory-model.md`,
`docs/memory-review.md`.

## ADR-062 — TCP sockets: thin kernel, mailbox-delivered, over a reusable IO seam

**Status:** accepted (2026-05-30). Implemented (client + server; TLS is a planned
follow-up).

**Context.** Brood needs network I/O — first as a genuine language capability
(an HTTP client, eventually the M4 server listening on a socket), and to dogfood
the package-loading story with a real third-party-style package. The kernel had
no Brood-callable sockets (the `dist` node link reads `TcpStream`s in Rust,
private). The question was *how thin* the native layer is and *how* a socket
interacts with the green scheduler.

**Decision.**

- **Thin kernel mechanism, policy in Brood (ADR-006).** Five primitives —
  `tcp-connect` / `tcp-listen` / `tcp-send` / `tcp-close` / `tcp-local-port` —
  wrap `std::net`. Framing, request/response draining, and protocols (HTTP next)
  are Brood (`std/tcp.blsp`).
- **Mailbox delivery, not polling (ADR-059).** An early non-blocking-poll design
  (Brood loops over a `tcp-recv` that returns would-block) was built and then
  **replaced**: it busy-polls and pins no worker only by luck. Instead a socket
  follows the blocking-IO → mailbox rule: a dedicated **non-worker reader thread**
  blocks on `read` and `deliver`s events to the **owning process's mailbox**, and
  Brood consumes them with plain `receive`. Shapes: `[:tcp sock data]`,
  `[:tcp-closed sock]`, `[:tcp-accept lsock client]`. `connect`/`listen` register
  the *calling* process as owner; an accepted client is wired to the listener's
  owner. A socket waiting for data costs zero workers.
- **A reusable IO seam.** The thread-plus-`deliver` pattern is extracted into one
  place — `process::spawn_io_source(subscriber, name, |sink| …)` + `MailboxSink`
  — so sockets are its first consumer and `gui` / `dist` / terminal input migrate
  onto it later (they hand-roll the same pattern today). This is the concrete
  form of ADR-059's principle.
- **`Value::Socket(u64)` — a scalar handle.** Unlike the heap-bound rope, a socket
  is an id into a global registry, so the GC treats it as a leaf (never traced or
  moved) and it is valid across this runtime's processes (a spawned handler can
  own one). It is **not** node-portable: the dist wire codec rejects
  `Message::Socket`. Adding the 17th `Tag` widened `Ty` from `u16` to `u32`
  (32-atom cap; the documented widen point).

**Consequences / scope.**

- No polling, no `tcp--yield`; `std/tcp.blsp` shrank to `socket?` + `tcp-drain`
  (collect a response until the peer closes). `tests/tcp_test.blsp` drives a full
  loopback echo in a single process via `receive` (so it passes without depending
  on cross-process spawn).
- **Blocking corners (v1):** `tcp-connect` and `tcp-send` block their worker
  briefly (a connect handshake / a `write_all`); the *accept* loop polls on its
  own dedicated thread. Fine at the dozens-of-connections scale; a `mio` reactor
  (ADR-059 Phase 2) is the later scale path, under the same primitives.
- **TLS (done, client) — 2026-05-30.** `https` via `rustls` (the one non-thin,
  crate-backed exception; aws-lc-rs provider + bundled `webpki-roots`, no system
  OpenSSL/trust store). rustls connections can't be split read/write across
  threads like a raw fd, and an HTTPS client call is request→response anyway, so
  TLS is a **one-shot `tls-request host port request`**: a non-worker thread
  connects, handshakes, writes the request, and streams the response back as the
  *same* `[:tcp id data]` / `[:tcp-closed id]` (and `[:tcp-error id msg]`)
  messages — so `tcp-drain` and the HTTP parser are unchanged. `std/http.blsp`'s
  `http-get` picks `tls-request` for `https://`, `tcp-connect`+`tcp-send` for
  `http://`; verified against `https://api.github.com`. ⬜ Still deferred:
  *streaming/persistent* TLS sockets (needs a non-blocking rustls integration or
  a `mio` reactor), and **server-side** TLS (cert+key).
- **`tcp-controlling-process` (done — 2026-05-30):** hand a passive accepted
  socket to a per-connection handler; accepted sockets are passive until claimed.
- **Deferred:** binary-safe bytes (recv is UTF-8-lossy today — fine for
  text/HTTP); a bytes type is a separate future decision.
- **Streaming-response seam (done — 2026-05-31).** The HTTP server's
  read→one-response→close shape gained one protocol-agnostic escape hatch: a
  handler may return `(stream-response status headers stream-fn)` instead of a
  `{:status :headers :body}` map. `http--serve-conn` then renders only the head
  (`render-head`, no Content-Length / `Connection: close`) and hands the **live
  socket** to `stream-fn`, *not* closing it — the handler owns the connection from
  there and `tcp-send`s over time in its own per-connection worker process. This is
  the general seam, not an SSE feature: SSE server push (`std/sse`'s `sse-headers`
  / `sse-frame` / `sse-send`), long-poll, chunked downloads, and a WebSocket upgrade
  are all just `stream-fn`s on top of it — the kernel adds nothing, consistent with
  ADR-006 (mechanism in Rust, policy in Brood) and ADR-011 (ship the simple seam,
  defer the power features to consumers).

**References.** ADR-059 (blocking work → mailbox; the seam this builds on),
ADR-006 (language-in-the-language), ADR-026 (immutability — sockets are the
Rust-backed mutable-resource escape hatch, like the rope), ADR-045 (rope, the
other opaque handle), `docs/handoff-blocking-io.md`.

## ADR-063 — `(exit pid reason)`: Erlang-style process termination

**Status:** accepted (2026-05-30). Implemented: the `exit` primitive + the
`Suspend::Kill` scheduler path.

**Context.** Green processes could only end on their own (return, throw, or the
stack-overflow guard). Nothing could terminate *another* process — needed for a
test-runner per-test timeout, an MCP-tool watchdog, and supervision generally. A
green coroutine is pinned to one worker and **cannot be aborted mid-computation
from another thread** (the KI-1b cross-thread-resume hazard), so termination has
to happen at the target's own yield points.

**Decision.** `(exit pid reason)`, modelled on Erlang `exit/2`:

- `reason = :kill` — the **untrappable hard** kill. Checked in `preempt()` (the
  reduction-boundary yield, hit ≤2000 reductions), so it stops even a tight CPU
  loop that never `receive`s. Untrappable **by construction**: it fires at the
  scheduler level via a new `Suspend::Kill(reason)` the coroutine yields, which
  `run_one` turns into `deregister(reason)` + drop — *below* Brood's `%try`, so no
  `catch` can intercept it.
- any other `reason` — the **soft** signal. Checked at the top of `receive_match`'s
  loop (a server's natural per-iteration boundary), so the target finishes its
  current iteration, then dies with `reason`. A tight non-`receive` loop won't
  honour a soft exit — inherent to cooperative termination (use `:kill`).

**Mechanism (no cross-thread resume).** A per-`Mailbox` `kill_pending: AtomicBool`
+ `MailboxState.kill: Option<Message>`, set by `exit` via the registry from any
thread. The target observes it at its own `preempt`/`receive` and self-terminates
on its **own** worker (where dropping the coroutine force-unwinds safely —
corosensei force-unwinds a suspended coroutine on drop, running destructors). A
**parked** target (in `receive`, not running) is woken by re-`enqueue`ing it onto
its own worker — never dropped by the caller, which would resume the coroutine on
the wrong thread. The state lock serialises `exit`'s waiter-take with `run_one`'s
park, so a just-parking process can't end up parked-with-a-pending-kill (stuck):
exactly one of the two wins. Monitors fire `[:down ref pid reason]`. Exit of a
dead/unknown pid is a no-op (idempotent); remote pids error for now (defer dist).

**Consequences.** Unblocks the test-runner 30s per-test timeout and the MCP-tool
10s watchdog (both `(exit pid :kill)` a slow worker). Self-exit takes effect at the
caller's next yield (not instantaneous) — acceptable; revisit if needed. A
trap-exit (`exit` delivered as a *message* to a process that opted in) is deferred
(ADR-011) until a supervisor needs it.

**References.** ADR-059 (blocking-work→mailbox; the deliver-and-self-handle shape),
KI-1b (cross-thread-resume hazard this design avoids), ADR-051 (`process-info`),
ADR-011 (defer trap-exit), [`todo.md`] (the test/MCP timeouts built on this).

## ADR-064 — Rust primitives are single-shot w.r.t. eval re-entry

**Status:** accepted (2026-05-30). `macroexpand` moved to Brood; rule adopted.

**Context.** Collect-at-any-depth (ADR-061) made the copying collector fire at any
eval depth. That turned a whole class of Rust code into a hazard: a `&mut Heap`
function that holds a LOCAL handle (`Value`/`EnvId`) in a Rust local **across a
call that re-enters `eval`/`apply`** can have that handle relocated out from under
it (the collector moves it; the Rust local isn't updated). The closing sweep found
**six** such sites (`reload_defs`, `receive_match`, `check_file`, `try_catch`,
`quasiquote`, `macroexpand`) and hand-rooted each on the operand stack — tedious
and easy to reintroduce.

**The key asymmetry:** **Brood code is structurally immune.** A Brood function's
"locals" are environment bindings, and the evaluator already roots the active
scope across every nested eval (the ADR-061 operand stack). So a loop or
accumulator written in Brood is GC-safe *by construction* — there is no unrooted
Rust local to go stale. The hazard exists *only* at the Rust↔eval boundary, and
only when a Rust frame **loops or accumulates** across eval.

**Decision.** A Rust primitive must be **single-shot with respect to eval
re-entry**: it may call `eval`/`apply`, but must not hold a LOCAL handle across
that call — and in particular must not *loop* over eval or *build a structure from
eval results*. Anything that does belongs in **Brood** (ADR-006), where the
evaluator roots it for free. Corollaries:

- A primitive that **never** re-enters eval can't trigger a collection at all (GC
  only runs at the eval safepoint), so its `&[Value]` args and locals are always
  valid — **I/O primitives are safe by construction** (`net`/`tls`/`file`/the
  `io_source` mailbox seam: do the syscall, return a Value or *deliver to a
  mailbox*; never `apply` a Brood callback inline holding a handle).
- The irreducible kernel that *must* re-enter eval and hold state — `%try`,
  `receive_match`, `apply`, `load`/`eval-string`, the compile-pass
  `macros::macroexpand_all` — stays in Rust, hand-rooted, and is the small,
  auditable exception set. (The compile pass additionally opts out of collection
  via `MACRO_BLOCK` — ADR-061.)

**First application.** `macroexpand` (the fixpoint loop) moved to a Brood prelude
`defn` over the single-shot `macroexpand-1` primitive — its loop state is now an
env-bound local, auto-rooted. The user-facing Rust `macroexpand` builtin is gone;
`macros::macroexpand` (Rust) remains only for the compile pass.

**Deferred (same rule, bigger moves).** `quasiquote` → a Brood macro over
`cons`/`list`/`eval` (the worst offender, but a bootstrap refactor: `defn` itself
uses backtick, so the expander must be raw Brood before `defn`, and the compile
pass must expand rather than skip `quasiquote`). `reload-defs` → Brood (needs
`note-definition` / read-file-forms primitives exposed). Both tracked as their own
tasks; the Rust versions are correctly rooted in the meantime.

**References.** ADR-061 (collect at any depth — the operand stack that makes Brood
loops safe), ADR-006 (write the language in the language), ADR-059 (the
mailbox-delivery seam that keeps I/O primitives callback-free), CLAUDE.md "Debug
tooling" (`BROOD_GC_VERIFY` — how the six sites were found).

## ADR-065 — Namespaces: expand-time resolution over the flat table, soft privacy

**Status:** accepted; **increments 1–3 + α implemented** (2026-05-30). Inc-1: the
substrate (resolver pass, per-process `compile_ns`, forward-ref pre-scan, qualified
def-site keying, ns-aware advisory checker). Inc-2: `(:use …)` imports + auto-require
— a per-file `Heap.imports` table the resolver consults after the current namespace
and before root; `%refer` enumerates a module's public (non-`--`) names or a `:refer`
subset; `:use` emits `(require …)` so the module auto-loads (never *fetches*).
Own-namespace defs shadow imports. The **macroexpand pass resolves the head through
that same table** (`macroexpand_1`, 2026-05-30): a `(:use …)`-imported (or
same-namespace) macro expands during the compile walk, not only a directly-bound one
— without it an imported macro head (e.g. hatch's `defprocess`) stayed unexpanded and
the advisory checker flagged its raw body. **Inc-3 (the big-bang):** `defmodule` *is* the
single namespace form — the `ns` macro was renamed to `defmodule` and `ns` dropped (a
module *is* a namespace); all of `std/` + every test file migrated in one pass
(leaf-out), with `test` namespaced and `(:use test)` added throughout.
**α** shipped in the same pass: the resolver descends quasiquote templates and
auto-qualifies free reference-position symbols to the *defining* namespace, so
namespaced macros are correct across namespaces without hand-qualifying (the
β-interim wall, e.g. `test/describe`'s bare helper emission, is closed). The
**earmuff rule** (`*foo*` names are ambient/root, never namespaced) was added so
`defdyn` vars / `*load-path*` / `*features*` stay reachable unqualified. Full design
in [`namespaces.md`](namespaces.md). Supersedes the "deferred, point-2-only" stance
of ADR-019. **Left open** (additive, don't block anything): LSP Tier 2 and ns-name
collision policy.

**Context.** ADR-019 chose Emacs-flat modules and deferred namespaces, betting
they'd fight the editor's "any code can redefine any behaviour live" grain
(ADR-013 hot reload). Four pressures now arrive together and force the question:
the package manager (ADR-037) loads third-party `name = URL` code into the one
flat global table (silent clobbering — the package manager is unsafe without an
answer); first-party `std/` crowds the flat namespace; M2+ editor plugins from
many authors must coexist; and the LSP needs qualified names for completion /
cross-file nav / rename. ADR-019 left a spectrum: (1) flat [done]; (2) a Brood
prefix-macro layer; (3) first-class per-file resolution. This commits the
*substrate* and most of the surface of (3), built like (2).

**The reframe.** Surveying Lisps, "namespaces" is two languages. Clojure and CL
are namespaced **and** openly redefinable; Racket is sealed **and** not
redefinable. **Hard privacy and hot reload are the same trade-off seen from two
sides.** ADR-019's worry holds only for the Racket end. So Brood takes the
**Clojure/CL position — namespaced with *soft* privacy**: "private" = not
auto-imported + `--` convention + a checker lint, *never* erased from the runtime.
A fully-qualified name (`observer/observe--internal`, like CL `::`) stays reachable
and live-redefinable. The grain is preserved.

**Decision.**
- **Expand-time resolution over the existing flat table — no namespace axis in
  the core.** `/` is already a legal symbol char and lookup is "find the full
  symbol," so `text/insert` is one interned symbol that already works. `(ns …)`
  sets a current namespace (`*ns*`, a `defdyn` the compile pass reads); `(defn
  observe …)` inside `(ns observer)` defines `observer/observe`; a **resolver
  pass** in the compile pipeline rewrites reference-position symbols
  (bare → ns-qualified → imported → root/prelude fall-through). The **runtime,
  `def`-rebinding, ADR-013 hot reload, `send`/promote/freeze, and the GC are all
  unchanged** — resolution emits a plain late-bound global. Rejected: partitioning
  the `value.rs` interner into `(ns, name)` (touches reader/eval/env/RuntimeCode/
  dist/hot-reload for a result the flat substrate already gives at the surface —
  the big core change ADR-019 argued against).
- **One shared resolver for eval *and* the LSP.** The evaluator and the language
  server run the *same* pass, so the editor can never disagree with the runtime.
  Requires the `ns`/`:use`/`:refer` forms be statically analyzable from the CST
  (they are — keyworded data).
- **Data symbols are inviolate.** The resolver rewrites only resolved
  variable/operator positions, never `quote`d content — symbols travel by name and
  re-intern across runtimes (ADR-034); rewriting a message tag or map key would
  break cross-process protocols. `resolve`/computed-symbol/`apply` are the runtime
  escape hatches.
- **Auto-require resolves + loads from the load-path; it never *fetches*.** Deps
  stay explicit in `project.blsp` so the lock file (ADR-037) stays computable.
- **Migration.** Prelude = the root namespace (unqualified `map`/`+`/`cons`,
  ergonomic macros `describe`/`test`/`is` stay root). `defmodule` evolves into
  `ns`; `provide`/`require`/`*load-path*` become the loader underneath. std
  namespaces gradually; user/package code is namespaced from birth. No hard
  sealing, ever.

**Open (don't block the substrate; see `namespaces.md` §7–8).**
- **Macro hygiene.** Brood macros are unhygienic (bare-symbol `quasiquote` +
  manual `gensym`); use-site rewriting breaks cross-ns macros. *Lean:* α —
  Clojure-style auto-qualifying `quasiquote` (template symbols qualify to the
  macro's defining ns; `~'foo` escapes to a bare symbol) — but it's the biggest
  semantic change and interacts with the ADR-064 quasiquote→Brood refactor.
  Alternative β: stay unhygienic, hand-qualify cross-ns refs.
- **Namespace-name collision across packages.** Namespacing relocates collision
  from symbol level to ns level (two packages declaring `(ns parser)`). Free-for-all
  short names vs. package-local-name-prefixed. Best decided against ADR-037's shape.

**References.** ADR-019 (the flat-modules decision this supersedes + its spectrum),
ADR-037/`packages.md` (the collision pressure + the no-fetch line), ADR-013
(hot reload — the grain soft-privacy preserves), ADR-034 (symbols re-intern by
name across runtimes — why data symbols can't be rewritten), ADR-064 (the
quasiquote→Brood refactor that the hygiene decision rides on), ADR-011 (ship the
simple form), ADR-025/`lsp.md` (Tier 2 — the resolver is shared with the LSP).

## ADR-066 — Auto-gensym (`x#`): opt-in macro binding hygiene

**Status:** accepted (2026-05-30). Implemented in `eval/macros.rs`.

**Context.** Brood macros are unhygienic: a `defmacro` template that introduces a
binder with a plain literal symbol (`(let (tmp …) …)`) shares one flat namespace
with the caller's code, so the binder can **capture** a spliced argument (or be
captured by it). The standing fix is a manual `(gensym)` — verbose, easy to forget
(the `types/check/hygiene.rs` lint exists precisely because forgetting is a real
bug). Solving this *before* namespaces (ADR-065) was chosen deliberately: "macro
hygiene" is two separable concerns — **(#1)** free-reference transparency (a
template's `helper`/`map` resolving to the def site — the namespacing-coupled one)
and **(#2)** introduced-binding capture (this, pre-existing and independent). #2
should not be entangled with the namespace work.

**The roads not taken.** Full Scheme/Racket automatic hygiene (syntax objects /
sets-of-scopes) makes capture impossible without author effort, but requires
identifiers that carry per-occurrence lexical context — fattening `Value::Sym`
(taxes every eval + the GC) or a parallel syntax-object representation, and it
fights two Brood invariants: symbols ship **by name** across runtimes (ADR-034 — no
meaning to a local scope set) and code is ordinary data (homoiconic; syntax objects
need `datum->syntax`/`syntax->datum` bridges). That's the large, core-deep,
"sweeping" change we declined. Elixir-style context-tagging is lighter but still
touches the symbol representation and the cross-process question. **Clojure** — the
closest sibling (Lisp-1, namespaces over a mutable var table, live redefinition) —
deliberately declined full hygiene for these same reasons and shipped auto-gensym;
we follow it.

**Decision.** Clojure-style **auto-gensym**: inside one backtick expansion, a
*literal* template symbol whose name ends in `#` (e.g. `tmp#`) is rewritten to a
**fresh** `gensym`, the **same** one for every occurrence within that expansion and
a **distinct** one per expansion (per call site — macros expand at compile time, so
two runtime calls of one compiled body reuse the baked symbol, as in Clojure).
- **Smallest possible change.** One interception in the quasiquote walker's leaf
  arm (`maybe_autogensym`), threading a per-expansion `HashMap<Symbol, Value>`. **No
  change to the reader** (`#` is already symbol-legal), `value.rs`, `eval`, or the
  symbol model. `value::gensym` already existed.
- **GC-safe by construction.** The table holds only interned `Value::Sym`/`u32` —
  which the copying collector never relocates and which ship by name — so it needs
  none of quasiquote's operand-stack rooting; it sits outside the GC-sensitive path.
- **Correct by the walker's structure.** Only literal template symbols reach the
  leaf arm; a `x#` inside `~unquote` goes through `eval` and is left alone (it's
  user code). The escape for a deliberately-literal/anaphoric binding is `~'it`
  (unquote a quoted symbol).
- **Non-breaking.** No existing `std/` or test symbol ends in `#`; manual-`gensym`
  macros are unaffected. The hygiene lint now treats a `#`-binder as safe and
  suggests `x#` as the lighter alternative to `(gensym)`.

**Scope.** This is concern **#2** only — *binding* capture. Concern **#1** (free
references resolving at the def site across namespaces) is the α decision left open
in ADR-065/`namespaces.md` §7; it is *not* addressed here. Full automatic
(Scheme-grade) hygiene remains deferrable indefinitely — `x#` is forward-compatible
with adding scopes later if a real need ever appears.

**References.** ADR-009 (quasiquote), ADR-065/`namespaces.md` §7 (the two-concerns
split; #1 still open), ADR-064 (the quasiquote→Brood move this rides alongside),
ADR-034 (symbols ship by name — why scope-bearing identifiers are costly here),
ADR-006/011 (Brood-first, smallest core), `types/check/hygiene.rs` (the lint).

## ADR-067 — Process links + `trap_exit` (the supervisor's structural orphan fix)

**Status:** accepted, 2026-05-30. Implemented in a worktree (`links-trap-exit`).

**Context.** `monitor` (ADR-035) is a *one-directional* death notification — it
never affects the watched process. That's the wrong tool for the one thing the
userland supervisor couldn't do: when the **supervisor itself** dies (crash,
intensity-exceeded, or an external `(exit sup …)`), its children kept running —
orphaned. `(exit pid reason)` (ADR-063) added termination but not *coupling*: the
supervisor still had to explicitly kill each child, which a dead/crashing
supervisor can't do. The deep-dive vs Erlang (`supervision.md`) named this the
single biggest gap, and named the fix: Erlang's **links** (symmetric) + `trap_exit`.

**Decision.** Add the general Erlang primitives, not a supervision-specific hook
(the ADR-039 lesson — a narrow "kill my dependents" kernel feature was rejected in
favour of the general one):

- **`link`/`unlink`** — symmetric coupling in a `LINKS` table (`process/links.rs`),
  the structural cousin of `MONITORS`. Same race-free discipline (liveness checked
  inside the table critical section; `deregister` takes tables sequentially, never
  holding REGISTRY while reaching for LINKS).
- **`trap-exit`** — a per-mailbox `AtomicBool`. When set, a linked peer's death
  arrives as a trappable `[:EXIT pid reason]` *message* instead of killing this
  process.
- **`deregister` hook** — after firing monitors, walk the dying pid's links: a
  trapping peer gets `[:EXIT]`; a non-trapping peer with an **abnormal** reason is
  killed (propagation, cascading through *its* links); `:normal` never propagates
  to a non-trapping peer.
- **`spawn-link`** — a prelude macro (`(let (p# (spawn …)) (link p#) p#)`); no
  kernel surface (linking a child that dies in the gap is safe — link-to-dead
  fires `[:EXIT … :noproc]`).

**Propagation hardness — D-simple.** Brood couples "untrappable/immediate" to
`reason == :kill`. A non-trapping peer must die *immediately* (even mid-CPU-loop),
so propagation routes through the hard `(exit peer :kill)`: the peer dies promptly
but reports `:kill` to its own monitors rather than the originating reason. That's
immaterial for supervision (a torn-down worker isn't monitored by anyone but its
dead supervisor). A future "hard kill carrying an arbitrary reason" (a `hard` bit
on the mailbox kill-state) would make it exact; deferred (ADR-011).

**Supervisor rewrite.** `std/supervisor.blsp` switched from `monitor`/`[:down]`/
`:ref` to `trap-exit` + `link` + `[:EXIT]`/`:pid`. A child crash now arrives as
`[:EXIT child reason]`; a supervisor's *own* death propagates to its children
(workers die by propagation; a child **sub-supervisor** traps, recognises its
parent's `[:EXIT]` — it records the caller as `:parent` at `start-supervisor` — and
tears its own subtree down). The `:shutdown :infinity` cascade (ADR-044) still
governs *graceful* teardown (a deliberate hard kill is untrappable, so a
sub-supervisor must opt into the cooperative `[:$stop]` path).

**Why this doesn't reopen ADR-039 (KI-1).** Links add no per-call scheduler-global
state and no cross-thread coroutine resume; the teardown walk runs on the cold
`deregister` path (where monitors already fan out), is a general primitive (any
process links any process), and propagation reuses the existing `exit` path.
Validated: full worktree `cargo test` green, the 17-test supervisor suite + new
7-test `link_test.blsp` clean 3× under `BROOD_GC_STRESS=1` and once under
`BROOD_GC_VERIFY=1`.

**Runtime child API (DynamicSupervisor).** Rides on the same rewrite:
`start-child`/`terminate-child`/`restart-child`/`count-children` (synchronous
request/reply messages the loop handles). A supervisor started with `[]` children
and grown at runtime is Elixir's DynamicSupervisor; a dynamically-added child is a
full member (linked, restarted per its type, torn down on shutdown). No dedicated
`simple_one_for_one` mode — the API works under any strategy.

**Distributed links (cross-node, update 2026-05-30).** Links span nodes, mirroring
the remote-monitor machinery: `link`/`unlink`/`exit` accept a remote pid and route
over the dist link. Three wire frames — `Frame::Link`/`Frame::Unlink` (each node
records its half of the symmetric link in `links::REMOTE_LINKS`, keyed
`local_pid → (node, remote_pid)`) and `Frame::Exit { link }` (a `link`-death goes
through the trap-or-propagate path carrying the *remote* pid; a non-`link` exit is
the explicit remote `(exit pid reason)`, routed to `scheduler::exit`). A net-split
fires `:noconnection` to every local peer of a process on the dropped node — the
exact `:noconnection`-on-net-split semantics monitors have (wired into
`dist::fire_nodedown` alongside `handle_node_down`). This makes **cross-node
supervision** work: a supervisor links a remote child (its `:start` must return a
remote pid — `remote-spawn` is fire-and-forget, so obtain it via a roundtrip), a
remote crash arrives as a link `[:EXIT]` and restarts, and the supervisor's own
death tears the remote child down. Verified by `crates/cli/tests/distribution.rs`
(remote link death → `[:EXIT]`, remote `(exit :kill)`, and a B-supervises-A child
restart). The race-safety mirrors `monitor_remote`: record the half before
consulting `NODES`, so net-split and the wire send can't orphan an entry.

**Synchronous `remote-spawn` (done — 2026-05-30).** `(remote-spawn-sync node
expr)` ships the thunk to the peer's `:remote-spawn` server with the caller's pid
+ a fresh `(ref)`, the server spawns it and replies `[:spawned ref child-pid]`,
and the macro blocks in `receive` for that pid (5s timeout). The returned remote
pid carries the peer's `name@host` (ADR-073), so it's directly `monitor`/`link`-able
— remote-child specs are now turnkey, not roundtrip-by-hand. Pure Brood in
`std/prelude.blsp`; `remote--spawn-server` gained a `[:run-sync …]` clause beside
`[:run …]`. See `remote_spawn_sync_returns_a_usable_remote_pid`.

**Still deferred (ADR-011).** Exact propagated reason for a non-trapping peer (the
`hard` bit above); a `terminate/2`-style worker cleanup hook (the last OTP-parity
item — cleanup on an *external* kill needs the trappable-shutdown path, only
`[:$stop]`-cooperative today).

**References.** ADR-035 (monitors — the one-way cousin), ADR-063 (`exit/2`),
ADR-044 (`:shutdown` cascade), ADR-033/034 (the dist wire codec links extend),
ADR-039 (the reverted kernel supervisor — why general primitives), `supervision.md`
(the vs-OTP deep dive that motivated this), `tests/link_test.blsp`,
`crates/cli/tests/distribution.rs`.

## ADR-068 — Node-connect ergonomics: default-cookie file, name-addressed Unix transport, `nest run --name`

**Status.** Accepted, implemented 2026-05-30. Extends ADR-033/034 (distributed
nodes); the wire protocol, HMAC handshake, pid routing, links/monitors and
ADR-067 supervision are unchanged. See [`node-connect.md`](node-connect.md),
[`distribution.md`](distribution.md).

**Context.** Connecting nodes was all hand-wired: `(node-start :a "127.0.0.1:9001"
"cookie")` + `(connect "a@127.0.0.1:9001")`. Three frictions, all incidental to
the share-nothing model: you invented a cookie per program (every example
hardcoded `"demo-cookie"`), you picked an IP+port even for two runtimes on one
machine (the common dev case *and* the editor-daemon case), and bringing a node
up was in-program ceremony. The destination — an Emacs-like editor "runnable
locally as a native app and remotely as a server", M4's "`--daemon`/`emacsclient`
model" — wants the opposite: address a local peer by name, share one secret,
start a node from the command line.

**Decision.**
1. **A per-user shared cookie**, Erlang-style: `~/.config/brood/cookie`
   (honoring `$XDG_CONFIG_HOME`), one line of hex, mode `0600`, auto-generated on
   first use. Resolution: `$BROOD_COOKIE` → the file → mint + persist — on the
   *connecting* side too, not just `node-start`, so "just connect" works.
2. **A name-addressed Unix-domain transport.** A local node binds
   `$XDG_RUNTIME_DIR/brood/<name>.sock` (fallback `/tmp/brood-<user>/`); peers
   reach it with `(connect "name")` — no port, no IP. `(connect "name@host:port")`
   still means TCP. Dispatch reuses the existing `@` split. Handshake/framing/
   heartbeat run unchanged over both carriers via a single `Stream { Tcp | Unix }`
   seam in `dist.rs`. The `0700` socket dir gates other users; the cookie
   handshake still runs over Unix too, for one uniform protocol.
3. **`nest run --name NAME`** brings up a local node before the program runs (the
   `--daemon` model), so the file is pure app logic.

**Policy in Brood, mechanism in Rust** (ADR-006). The friendly `node-start` /
`connect` / `node-cookie` live in `std/prelude.blsp` (always on, no `require`);
they compute the socket path, resolve the cookie, and pick the transport, over
four thin Rust primitives: `%node-listen`, `%node-connect`, `random-token`
(CSPRNG → hex), `spit-private` (atomic `0600` write). The kernel only carries
bytes and does the I/O it must (sockets, perms, RNG) — which `nest observe` can
reach via its `Interp`, so none of the policy needs to be Rust.

**Scope / deferred.** One transport per node for now (arity-1 `node-start` =
Unix; an addr = TCP); **dual-listen** (a node serving Unix *and* TCP at once —
the eventual editor-daemon end-state) is cleanly additive later, needs no
protocol change (ADR-011). Windows (no `$XDG_RUNTIME_DIR` convention) is out of
scope; TCP works everywhere. Connecting requires a prior `node-start` (no
implicit ephemeral client node) — explicit over magic.

**Consequences.** The 3-arg `(node-start name "host:port" cookie)` and
`(connect "name@host:port")` forms are unchanged, so the existing TCP
`distribution.rs` suite passes as-is; the change is almost entirely additive. The
M3 observer's remote-attach (`nest observe --connect name`) gains Unix addressing
+ the cookie-file fallback for free — the first consumer, today. New tests:
`two_unix_nodes_connect_by_name_and_message`, `wrong_cookie_rejected_over_unix`,
`cookie_file_autogen_and_reuse`.

**References.** ADR-033/034 (distributed nodes), ADR-006 (policy in Brood),
ADR-011 (defer the powerful form — dual-listen), `node-connect.md`,
`distribution.md`, `crates/cli/tests/distribution.rs`, `std/prelude.blsp`.

## ADR-069 — Evaluator dispatch performance: cache the analysis, not the behaviour

**Status:** partially accepted (2026-05-30). Increments 1–2 **implemented** (branch
`perf-eval-dispatch`); increments 3–4 **deferred** (recorded here, gated on need).

**Context.** Cross-language benchmarks put Brood ~50–220× behind Node/BEAM on
interpreted hot loops (collatz, fib, loop, reduce). The project's bar (ADR-006,
`CLAUDE.md`) is explicit: close that gap by making the **evaluator** more capable —
a general mechanism that keeps `+`/`rem`/`fold`/`sum` written in Brood — **not** by
moving hot functions into Rust builtins (an escape hatch that hides the gap and
teaches us nothing). The stated goal is "at least in Elixir's range, but it doesn't
have to be there; using as much Brood as possible matters more — we'll even accept
some slowdown for a lighter Rust footprint." So the question isn't "how do we beat
Node," it's "what evaluator capabilities remove dispatch cost without moving
behaviour out of Brood." Tracing one hot inner op (`(+ a b)`) found the tax is
**symbol resolution and re-deriving immutable facts**, not the arithmetic:

1. two global lookups (`+`, then `%add`), each an `RwLock` acquire + hash on the
   shared `globals` table — plus cross-core contention under fan-out;
2. a wasted full local-env-chain *name scan* for `+` before it ever reaches the
   global table (it's never locally bound);
3. the thin-wrapper passthrough analysis (`(+ a b)` → forward to `%add`) **rebuilt
   from scratch on every call** — an immutable property of the closure;
4. ~5 thread-local reads per combination (gc-due / macro-block / soft-limit / tick /
   deadline).

**Decision (done — increments 1 & 2).**

- **Inc-1: precompute the passthrough analysis.** `ClosureArm` gains a
  `passthrough: Option<Passthrough>` field, computed once at the single
  closure-construction choke point (`Heap::compute_passthrough` in `alloc_closure`)
  and carried verbatim across promote/freeze/message copies (the forwarding head is
  an interned symbol, the arg-map plain indices — region-independent). The hot-path
  `eval::passthrough_arm` becomes an arm-select + field clone. Hot-reload-safe: a
  `def` rebuilds the closure, recomputing the field.
- **Inc-2: per-process global inline cache.** `RuntimeCode` gains a monotonic
  `version: AtomicU64`, bumped on every binding change (`def` rebind,
  `restore_globals`). Each `Heap` holds a `global_ic: symbol -> (version, value)`
  cache, consulted in `env_get` **only after** the local chain and dynamics miss
  (so it can never shadow a lexical or dynamic binding). A version match returns the
  cached handle with no `RwLock`; any `def` makes every stamped entry stale at once,
  so late binding stays exact. GC-safe with no rooting — globals are `promote`d to
  immovable PRELUDE/RUNTIME before binding, so a cached handle can't dangle across a
  local collection; unbound names aren't cached.

  Measured (release, best-of-2, vs `main` @ 59ae226): fib(32) 4.78→4.24s, loop(3M)
  3.18→2.86s, collatz(30k) 4.50→4.13s, reduce(1M) 3.60→3.37s — a consistent
  **6–11%**, no behaviour moved into Rust.

**Deferred (increments 3 & 4 — recorded, not yet justified).**

- **Inc-3: lexical addressing.** A resolution step in the existing compile pass
  (`eval::macros::compile`) rewrites each *local* variable reference to its
  `(depth, index)` frame coordinate, replacing the assoc-list **name scan** in
  `env_get` (cost 2 above) with a direct index. Biggest remaining win for
  param-heavy bodies (fib/loop). **Why deferred:** it's the largest change and bumps
  the type-system compatibility contract (`docs/types.md`) — a new first-class
  `Value` kind needs a `Tag` + type bit + GC/printer/message support, which is
  heavyweight for what is really an internal IR node. Likely wants a *side
  representation* (a resolved-ref encoding that isn't a public `Value`) rather than a
  new tag; that design isn't settled. Also interacts with `letrec`'s
  last-write-wins frame and macro-introduced bindings, which must resolve
  consistently.
- **Inc-4: fold the per-combination TLS reads** (cost 4) into one counter check.
  Low-risk, low-reward; only moves the "pure overhead floor" (the `loop` bench).

**Should we still do 3 & 4? (the gate.)** Not now. Inc-1/2 banked the cheap,
low-risk dispatch wins. The residual gap is dominated by two things Inc-3 addresses
(the env-chain name scan, and per-call env-frame allocation) — but the *honest*
fix for a tree-walker's structural ~50–220× tax is a bytecode / closure-compiling VM
(already flagged in `devlog.md`'s perf follow-ups), and lexical addressing is a
down payment on exactly that compile step. So the decision is: **revisit Inc-3 when
we commit to the compilation step** (it becomes a natural sub-task of building the
resolver/IR), rather than as a standalone `Value`-kind change now. Inc-4 rides along
with whatever next touches the eval loop's safepoint. Until then, neither is on the
critical path — the goal was "Elixir-range is nice-to-have; stay in Brood is the
priority," and the banked wins move us toward it without any Rust escape hatch.

**Why (the shape).** Both shipped increments follow the ADR-006 worked example
(multi-arity dispatch): a general evaluator capability that makes *every* Brood
global reference / operator wrapper cheaper, so the prelude stays in Brood and gets
faster — the opposite of moving `+`/`sum` into `builtins.rs`. The version-counter
inline cache is the standard late-binding-safe monomorphic cache; it preserves the
hot-reload contract (`docs/shared-code.md`) exactly.

**Consequences.** `ClosureArm` carries a derived field (copied by every arm-rebuild
site — `alloc_closure` computes, promote/freeze/message carry it). `RuntimeCode`
carries a version atomic bumped by the two global-table writers; `Heap` carries a
per-process `RefCell` cache (keeps `Heap: Send`, never shared across threads).
`eval::is_special_form` is exposed `pub(crate)` so the precompute can exclude
special-form heads. No language-visible change; no new primitive; no Rust builtin.

**References.** ADR-006 (write the language in the language — the governing
principle), ADR-013 / `docs/shared-code.md` (late binding / hot reload — why the
inline cache is version-guarded), ADR-035/054/055 (moving/generational GC — why a
cached global handle is safe but a local one wouldn't be), ADR-023/024 +
`docs/types.md` (the compatibility contract Inc-3 must clear), `docs/devlog.md`
(the original thin-wrapper elision this caches, and the bytecode-VM follow-up).

## ADR-070 — Namespace-name collisions: detect-and-reject, not mandatory prefixes

**Status:** accepted **and implemented**, 2026-05-30. Closes the one open policy
question from ADR-065 (`namespaces.md` §8). The detect-and-reject check is wired
into the package manager's resolution step (ADR-037 Slices 2–3 having landed) —
`std/package.blsp` `package--check-namespace-collisions`, run from
`fetch`/`add`/`ensure-deps`. **Package-rooted namespaces remain the eventual
upgrade, deliberately deferred** (see *Future direction* below).

**Context.** Namespacing (ADR-065) solves *symbol* collision but raises a *namespace*
collision: two third-party packages can both declare `(defmodule parser)`, and the
flat global table would merge their `parser/…` defs. Prior art: Clojure's
reverse-domain names (`com.foo.parser` — safe, verbose, author-controlled); CL has
no real answer; ADR-037 gives each dependency a project-local name the *importing*
project controls.

**Decision.** Keep namespace names **flat and short** (`parser`, `observer`) — no
mandatory prefix — and **detect-and-reject** collisions at dependency-resolution
time rather than prevent them structurally:

- Namespace names are free-for-all; the common case (descriptive names) has no
  collision, and short names keep call sites ergonomic (`parser/parse`, not
  `com.foo.parser/parse`).
- When the package manager resolves the dependency graph (ADR-037 `nest
  fetch`/lock), it **errors** if two reachable providers declare the same namespace
  name — surfaced loudly at lock time with both sources named, not silently merged.
  *(As implemented, "providers" includes the **importing project's own modules**,
  not just deps — a dep that shadows one of your own modules is the same silent
  clobber and is caught the same way. A provider's namespaces are read from each
  source file's `(defmodule …)` name, so a file whose name differs from its module
  is still checked by the name that actually clobbers.)*
- The heavier escape hatches — a mandatory per-dependency prefix, or an
  import-site **alias** (`(:use [parser :as p])`) — are **deferred** (ADR-011)
  until a real collision in the wild justifies them. The project-local dep name
  (ADR-037) is the natural authority for an alias when that day comes.

**Rationale.** This is the ADR-011 "ship the simple form, defer the powerful one"
call applied to names: flat names are the ergonomic default; a *detected* collision
is a clear, actionable error (rename, or — later — alias), which beats taxing every
call site with a verbose prefix forever to prevent a rare event. It also keeps the
substrate (ADR-065 §3) and the soft-privacy/hot-reload story untouched — collision
policy is purely a package-resolution concern.

**Consequences.** The check is cheap (list each provider's source dir, read each
file's leading `defmodule`, reject a name two providers share) and adds no language
surface, no call-site change, and no migration. The LSP/runtime need no change: they
already resolve a fully-qualified `ns/name` and don't care how the name was made
unique.

**Future direction — package-rooted namespaces (deferred, not rejected).** We
explored the stronger model where a dependency's local manifest name becomes a
**load-time prefix** (foo's `(defmodule b)` → `foo/b/…`), making collisions
*impossible* rather than merely detected — plus author-declared `:exports` (soft
module privacy) and import-site `[mod :as alias]`. It's the Cargo/Go shape
(consumer-controlled rooting; your *own* project stays bare — no Elixir-style
self-prefixing). We **deferred it** (ADR-011) for three reasons: (1) there are no
third-party packages yet, so it's collision-proofing an ecosystem that doesn't
exist; (2) it touches the just-landed ADR-065 substrate (multi-segment namespaces,
a package-scope-aware loader, sibling-alias resolution) — high risk on fresh code;
(3) it adds two permanent knobs (`:exports`, `:as`) to prevent a problem the cheap
check already surfaces loudly. The key de-risking insight that makes deferral nearly
free: **rooting is a loader decision, not a source decision** — because intra-package
references stay short (sibling resolution) regardless, a package's *source* is
identical whether its modules are filed under `b/` or `foo/b/`. So rooting can be
added later, when M2 editor-plugins create real multi-author pressure, with the
loader keeping the flat form working — no package-source churn. The cheap check is
the interim; rooting is the destination.

**References.** ADR-065 (`namespaces.md` §8), ADR-037 (`packages.md`, the dep
local-name model + the lock/resolution step that enforces this), ADR-011 (defer the
powerful form), ADR-068/071 (the *other* ADR-071 — native extensions — is unrelated;
rooting is recorded here, not as its own ADR).

## ADR-071 — Native extensions are WASM components, built on fetch and wrapped in Brood

**Status:** proposed (2026-05-30). Design recorded in [`interop.md`](interop.md).
Nothing implemented yet.

**Context.** ADR-037 closed the native-code door: a package wanting native code
"does it the standard Rust way (a separate crate, baked into the kernel); the
Brood side just `require`s a wrapper." That keeps the supply chain safe but makes
**every native capability kernel-blessed** — adding one means a PR against the
core, a recompile, and a new binary tied to one kernel build and host triple. As
the editor (M2+) invites plugins (highlighters, codecs, a regex engine), that's a
wall: third parties can't ship native code at all, and the kernel accretes every
capability anyone ever wants. The requirement is native extensions that (1) ship
and version *with the package*, (2) require **zero kernel recompilation**, (3)
are portable across kernels/platforms, (4) keep ADR-037's supply-chain door shut,
and (5) don't break the moving GC / per-process-heap / immutability / no-worker-
pinning invariants.

**Decision.** A package may ship a **WebAssembly component** as a native
extension. The package manager **builds it from source at `nest fetch` time** (or
fetches a prebuilt artifact), pins it in the lock file, and caches the `.wasm`
under `_deps/`. The runtime instantiates it **sandboxed** via an embedded
`wasmtime` host and surfaces its exports through a **Brood wrapper module**. The
committed decisions:

- **WASM, not a native dlopen ABI.** A `.wasm` is portable across kernel versions
  and host architectures (its only ABI is the **WIT interface**, decoupled from
  the kernel's `Value`/GC layout) and **sandboxed** (linear-memory isolation — a
  buggy/hostile guest can't segfault the runtime or scribble the Brood heap, so
  fault isolation survives) and **metered** (`wasmtime` fuel/epoch — fits ADR-043
  and the scheduler). A native `.so` fails all three. `wasmtime` is a runtime
  crate alongside `boxcar`/`ropey` — infrastructure, not Lisp-callable behaviour.
- **Zero kernel recompilation.** The `wasmtime` host is compiled into the kernel
  *once*; thereafter a native extension is **hash-pinned `.wasm` data, never
  kernel code**. Adding/updating/removing one never rebuilds the runtime; the
  same shipped binary runs extensions written after it was built, in languages
  the kernel never heard of. The recompile boundary becomes exactly the
  kernel/package boundary.
- **Built on fetch (the Rustler model), wrapped in Brood (the `use Rustler`
  model).** Native code is compiled from source when the package is pulled —
  `mix deps.compile` runs `cargo`, we run the manifest's declared
  `:wasm-build` toolchain — **for that package only; the kernel binary is
  untouched.** The Brood side gets a `use-native` macro (the `use Rustler`
  analog) that binds every WIT-exported function as a namespace function. Because
  the contract is WIT, the bindings are *generated*, not hand-stubbed per
  function (better than Rustler's manual stub list). A prebuilt `:wasm-artifact`
  (the `rustler_precompiled` analog) is the escape hatch for consumers without
  the toolchain.
- **The boundary marshals; it never shares handles.** The moving GC forbids
  passing a `Value` handle across a safepoint, so values cross as the **`Message`
  enum** (Brood's existing copy-on-send serialization boundary), large bytes ride
  the **blob heap (ADR-041)**, and stateful guest objects are **opaque resource
  handles** (the rope precedent, ADR-045). A **WASM instance is mutable state**,
  so it is modelled the only two ways Brood allows — an opaque handle behind
  primitives, or owned by a process — **never a `Value`** (not sendable, not
  map-able). No new state concept.
- **No worker pinning.** A guest call is CPU-bound; short calls run inline
  fuel-capped, long calls run on the Phase-3 **blocking offload pool** and
  **deliver to the mailbox** (`handoff-blocking-io.md`) — the same rule as TCP,
  GUI input, and dist.
- **Supply-chain door stays shut, reframed.** ADR-037 banned arbitrary install
  hooks; build-on-fetch keeps that because the build is a **declared toolchain
  invocation** (not a free-form `postinstall`) and the **output is sandboxed
  regardless** — strictly stronger than today's "bake an opaque crate into the
  kernel with full host privileges." Capabilities are **deny-by-default** (WASI
  imports granted per-manifest). Honest cost (shared with Rustler): build-on-fetch
  needs the wasm toolchain present and pays compile time — hence `:wasm-artifact`.

**Why.** It's the *only* shape that gives per-package native code with zero kernel
recompile **without** reopening the supply-chain hole — the sandbox is what makes
"run untrusted native code" compatible with "don't trust it." It reuses machinery
already built: the `Message` marshalling boundary, the blob heap, the opaque-handle
pattern, the deliver-to-mailbox offload seam, and ADR-037's manifest/lock/cache.
And it tracks a proven trajectory (Elixir: Rustler build-on-fetch → `rustler_precompiled`).

**Scope / deferred (ADR-011).** Component Model + WIT as the ABI (vs. core WASM +
a hand-rolled ABI) — recommended but revisit if wasmtime's component support is
too green; async guests (WASI 0.3) composing with the offload pool; zero-copy blob
read-mapping into linear memory; sandboxing the *build* toolchain (v1: trust the
declared toolchain); a richer per-extension capability/permission UI (the editor
will want it). Cross-node: a WASM instance is local mutable state, so it doesn't
travel in `send`/closure-ship — cross-node use is "talk to the owning process."

**Consequences.** `project.blsp` gains a `:native` clause; `project.lock.blsp`
gains a per-dep `:native` artifact hash + build provenance; `std/package.blsp`
grows build orchestration + the WASM cache layout; a new `use-native` wrapper
macro lands (likely `std/native.blsp`). The kernel embeds `wasmtime` and grows a
small primitive set (`%wasm-instantiate`/`%wasm-call`/`%wasm-build` + resource-drop
wiring), mirroring ADR-037's `%git-clone`/`%sha256`. No change to `require`/load
semantics — a native extension is code on the load path whose wrapper calls a
primitive.

**References.** [`interop.md`](interop.md) (the full design), ADR-037 (packages —
the manifest/lock/cache extended, the "no install scripts" line reframed),
ADR-041 (blob heap), ADR-045 (opaque immutable resource handle), ADR-043
(resource backstops — fuel/epoch), ADR-059/062 + `handoff-blocking-io.md`
(deliver-to-mailbox offload), ADR-054/055 (moving/generational GC — why the
boundary marshals), ADR-006 (write the language in the language — wrapper + policy
in Brood), ADR-011 (defer power features).

---

## ADR-072 — Stage C: a generational nursery + tenured old generation

**Status:** accepted (2026-05-30). The "make copying fast as well as stable"
refinement deferred by `docs/memory-review.md` §6 and ADR-055; the last GC item on
the `handoff-gc.md` list. Builds directly on the single-space copying collector
(ADR-055/061) and the generational-handle epoch (ADR-054).

**Context.** Stage B's safepoint collector did a **full semi-space copy** every
time: every *live* object was relocated on each collection, including long-lived
data that never dies. For a process holding a large working set across churn (a
`receive` server, the editor's buffer state) the per-collection cost tracked
*total live*, not *garbage* — so a stateful loop paid to recopy its entire state
on every minor reclamation. The young-death hypothesis (most allocations die
almost immediately) says the *survivors* of any one collection are a tiny
fraction of what was allocated — which is exactly the workload a generational
split optimizes.

**Decision.** Split the per-process LOCAL heap into a **nursery** (every `alloc_*`
bumps into it) and a **tenured old generation**. The handle's age is one bit
stolen from the generation field (`AGE_OLD`), so a handle still says where its
object lives; LOCAL accessors route young vs. old by that bit, against two epochs
(`local_epoch` for the nursery, `old_epoch` for old).

- A **minor collection** copies the nursery's survivors and drops the rest whole.
  Destination depends on *aging*: if the nursery grew past `min_tenure` (real
  allocation pressure ⇒ survivors are probably long-lived) survivors are **tenured**
  into old; otherwise they stay young via a **semi-space flip**. The flip is what
  keeps `BROOD_GC_STRESS=1` (a minor at *every* safepoint, tiny nursery) from
  prematurely tenuring transient garbage and bloating old.
- A minor **never traces or recopies the old generation** — the generational win.
  Sound because Brood data is immutable: an old object can never come to point at a
  young one, so old is not a root set for a minor. The lone exception is a frame
  tenured **mid-bind** (a collection during a `let` rhs, then bound further), which
  is the language's only data mutation (`env_define`); it's recorded in a one-entry
  **remembered set** the next minor scans. So: *almost* no write barrier, one site.
- A **major collection** compacts old (a semi-space copy of old → fresh old,
  dropping dead tenured objects), fired only when old doubles past `major_floor` —
  rare, so tenured garbage is still reclaimed without recopying old on every minor.

**Consequences.**
- On a stateful workload (a process holding ~20k live across heavy churn):
  **~8× faster, ~9× lower RSS, ~70× less copy volume** than the single-space copy;
  compute-bound (young-death-only) workloads are neutral. A 200k-iteration churn
  loop holding ~20k live runs flat at ~29 MB RSS.
- **`:copied` in `(gc-stats)` now counts promotions** (minor: nursery→old; major:
  old compaction), not "survivors of a flip" — so on a healthy young-death loop
  `reclaimed` dwarfs `copied`, and under `GC_STRESS` premature tenuring can push
  `copied` up (the gc.rs assertion accounts for both).
- **Thresholds are env-tunable** — `BROOD_GC_FLOOR` (adaptive minor trigger),
  `BROOD_GC_TENURE` (nursery pressure to tenure vs. flip), `BROOD_GC_MAJOR` (old
  size to trigger a major); object counts, `K`/`M` suffixes accepted. The shipped
  defaults (64K / 16K / 256K objects) measured well across a sweep of alternatives;
  the knobs are for workload-specific tuning and experimentation, not a default
  anyone must set (ADR-011 — the language asks nothing of the author).
- The heap verifier (`BROOD_GC_VERIFY`) was made generation-aware: it no longer
  re-walks immutable old-gen internals, only the live young graph + the cross-gen
  roots. Found along the way: a `flush_map` bug where a CHAMP node shared across a
  tenure boundary was copied into the wrong generation (OOB/SIGSEGV), and a
  release-only `cfg` slip.

**References.** `docs/memory-review.md` §5–6 (the design space; Stage C as the
"copying gets fast" point), `docs/memory-model.md`, `docs/handoff-gc.md` (item #5),
ADR-054 (generational handles — the epoch this reuses per-generation), ADR-055
(Stage B copying — the collector this refines), ADR-061 (collect at any depth —
the operand-stack roots both minor and major relocate), ADR-026 (immutability — why
there's no general write barrier), ADR-011 (defer power features — the tuning knobs
are opt-in).

---

## ADR-073 — Node names are `name@host` (Erlang short/long names)

**Status.** Accepted, implemented 2026-05-30. Refines ADR-034/068 (node identity);
the wire protocol, handshake, transports, and cookie are unchanged. See
[`distribution.md`](distribution.md), [`node-connect.md`](node-connect.md).

**Context.** A node's identity was a **bare keyword** (`:server`), and the host
lived only in the *transport* address (`server@host:port`). So `:server` on
machine A and `:server` on machine B had **identical identity**, and a pid
`{node: :server, id: 5}` is ambiguous once you're linked to two of them. Erlang
fixed this in 1998: a node *is* `name@host`, globally unique, carried in every
pid. The editor-server goal (remote frontends, cross-node supervision) needs
unambiguous remote pids.

**Decision.** A node's identity is the keyword **`name@host`** (`@` is a legal
symbol char, so `:server@whkbus` reads/prints fine). Qualification, Erlang's
short/long split:
- **Bare name** → qualified automatically (a **short** name). For a **local**
  Unix node the host is this machine's short `(hostname)` (`:a@whkbus`); for a
  **TCP** node it's the *listen address's host* (`:a@127.0.0.1`) — so a peer
  dialing `a@127.0.0.1:9001` and `ensure-link` derive the *same* name the node
  declares. That consistency is the load-bearing reason TCP qualifies from the
  address, not from `hostname`.
- **Already-qualified `name@host`** (passed explicitly) → used verbatim — this is
  how you get a **long**/FQDN name (`(node-start :a@a.example.com "0.0.0.0:9001")`).

There is no epmd, so the **port stays explicit** in `connect` (`name@host:port`);
`name@host` is the identity, `:port` the transport. `connect` returns the peer's
**authoritative** `name@host` (from the handshake) — you address peers with that
value, not a literal.

**Policy in Brood, mechanism in Rust** (ADR-006). The only kernel addition is
`(hostname)` (reads `/proc/sys/kernel/hostname`). All qualification — short vs
verbatim, local-hostname vs listen-address-host, the `name@host:port` parsing —
lives in `std/prelude.blsp` (`node--qualify`, `node-start`, `connect`,
`ensure-link--peer-name`). The node-name Symbol flows through `%node-listen` and
the handshake unchanged.

**Consequences (breaking, greenfield).** Node names are no longer bare literals:
`(node-name)`, `(nodes)`, and pid prints now show `name@host`, and `{:name …
:node X}` addressing needs the qualified value (from `connect` / `(node-name)` /
`nodes`), not `:a`. Migrated the `distribution.rs` suite (capture `connect`'s
return, or use the deterministic `:a@127.0.0.1` for loopback tests) and the
node examples. `remote-spawn`/`ensure-link` already take a node *value*, so they
needed no change beyond `ensure-link--peer-name` now returning `name@host`.

**Scope / deferred (ADR-011).** No FQDN *resolution* in the kernel — a long name
is had by passing it explicitly (matches how Erlang `-name` is usually given). No
epmd-style name→port registry. Short and long names interoperate freely (Brood
compares full `name@host` strings; it doesn't enforce Erlang's short-vs-long
connection ban).

**References.** ADR-034 (distributed nodes), ADR-068 (connect ergonomics — the
transport this qualifies), ADR-033 (closure shipping — remote pids carry the
node), ADR-006 (policy in Brood), ADR-011 (defer FQDN resolution / epmd),
`distribution.md`, `crates/cli/tests/distribution.rs`, `std/prelude.blsp`.

## ADR-074 — Dual-listen: one node, several transports (`node-also-listen`)

**Status.** Accepted, implemented 2026-05-30. Builds on ADR-068 (transports) and
ADR-073 (`name@host` identity); wire protocol, handshake, and cookie unchanged.
See [`distribution.md`](distribution.md).

**Context.** A node bound *one* transport: `(node-start :a)` → a local Unix
socket, or `(node-start :a "host:port")` → TCP. But the editor-daemon end-state
(M4) wants **one core reachable both ways at once** — local frontends by name
over a Unix socket (the `emacsclient` case) *and* remote frontends over TCP. That
needs a single node serving multiple listeners.

**Decision.** Add **`(node-also-listen [addr])`** — add another listener to an
already-started node, sharing its identity + cookie. No arg opens the local Unix
socket (keyed by the node's name-part); `"host:port"` opens a TCP endpoint. So
dual-listen is composed, not a special start mode:

```lisp
(node-start :ed@host "0.0.0.0:9001")   ; identity ed@host, TCP endpoint
(node-also-listen)                     ; + local Unix socket "ed"
;; now: (connect "ed") locally, (connect "ed@host:9001") remotely — same node.
```

The node keeps **one** identity (set once at `node-start`); extra listeners are
just more front doors. A peer reaching it via any transport completes the same
handshake and learns the same authoritative `name@host`; the de-dup/tie-break in
`establish` already collapses two links to one peer, so connecting via both
transports is harmless. Pairs naturally with an **explicit** `:name@host` start
(ADR-073) so the TCP dial host matches the identity.

**Why composable, not "TCP nodes are always dual."** Auto-binding a Unix socket
for every TCP node would pollute `$XDG_RUNTIME_DIR` and make same-name TCP nodes
on one host collide on the socket file (and silently churn the test suite, which
doesn't sandbox `$XDG_RUNTIME_DIR` for the TCP cases). Opt-in keeps the simple
single-transport `node-start` unchanged and lets the daemon ask for what it wants.

**Mechanism in Rust, policy in Brood** (ADR-006). `node_listen`'s bind+acceptor
was extracted into `start_listener(addr)` (identity-agnostic — the handshake
reads `NODE` at accept time), shared by the first listener and by the new
`%node-also-listen` primitive. `node-start` rolls identity back if its first bind
fails (still retryable). The prelude `node-also-listen` derives the Unix path and
picks the scheme; the kernel just binds and accepts.

**Scope / deferred.** Listeners can only be *added*, not removed (no
`node-stop-listening` — no need yet, ADR-011). Server-side TLS as a third
transport is still open (`rustls` is client-only). Many listeners are allowed but
the expected shape is one Unix + one TCP.

**References.** ADR-068 (transports + the `Stream` seam), ADR-073 (`name@host`),
ADR-034 (distributed nodes), ADR-006 (policy in Brood), ADR-011 (defer listener
removal), `crates/cli/tests/distribution.rs` (`dual_listen_serves_tcp_and_unix_at_once`),
`std/prelude.blsp`.

## ADR-075 — Undo lives in the buffer value (per-buffer undo/redo stacks)

**Status.** Accepted, implemented 2026-05-30. Extends ADR-045 (the immutable,
rope-backed buffer framework) and ADR-026 (immutability). See
[`devlog.md`](devlog.md) (2026-05-30) and `std/buffer.blsp`.

**Context.** The editor app (`~/src/whk/myedit`) needs undo, and — with multiple
buffers — undo must be **per-buffer** (Emacs keeps an undo list per buffer). The
question was *where* it lives: in the editor app (a stack of buffers in the app's
model) or in the buffer value itself (`std/buffer.blsp`). The prime directive
(ADR-006) says general capabilities belong in the language toolkit; keybindings
and the kill-ring/minibuffer UX are app policy and stay in the app.

**Decision.** A buffer **carries its own history**: `:undo` and `:redo` stacks of
`{:rope :point :mark}` snapshots. Each editing op pushes a pre-edit snapshot onto
`:undo` (clearing `:redo`) **only when it actually changes the text**; `undo`/`redo`
are pure stack moves restoring the snapshot triple. A snapshot deliberately
**excludes** the history fields, so snapshots don't nest or grow geometrically.

Rationale:
- **Per-buffer for free.** History lives in the buffer value, so switching buffers
  (just moving the app's `:current`) preserves each buffer's undo without any app
  bookkeeping — the immutable-value payoff.
- **Cheap.** A snapshot is `{:rope :point :mark}`; the rope is an Arc-shared B-tree
  (ADR-045), so a snapshot is O(1) and stacks share structure.
- **No no-op steps.** Guarding the push on a real text change keeps undo from
  having dead steps (delete at end-of-buffer, backspace at 0, empty-region delete).
- **Restoring a region delete brings the mark back**, since the snapshot is taken
  before the delete clears it — a small nicety over Emacs.

**Deferred (ADR-011).** No coalescing in v1 — one keystroke is one undo step.
Coalescing consecutive self-inserts needs last-command tracking, which is *command*
identity (app policy), not buffer state; pull it into the app when the editor wants
it. The `spawn-buffer` actor ships text+point+mark and rebuilds, so history doesn't
cross a process boundary (process-local view state) — acceptable.

**References.** ADR-045 (buffer framework), ADR-026 (immutability), ADR-006 (policy
in Brood), ADR-011 (defer coalescing), `std/buffer.blsp`,
`tests/buffer_test.blsp` (the `buffer undo / redo` block).

---

## ADR-076 — The execution engine becomes a closure-compiling VM

**Status:** accepted; **Stage 0–2b built** behind `BROOD_VM` (2026-05-30) — off by
default. **~2–2.3×**: Stage 0–1 (mechanism + ADR-069 passthrough redirect, ~2× on
fib/loop), **2a** (`let`/`letrec` via flatten-scope addressing, ~2.3× on let-loops),
**2b** (multi-arity, exact-arm dispatch). **Next: 2c** — local-capturing closures
(the GC-critical unlock; see `lexical-addressing-gotchas.md`). The performance "big
lever". Long-form companion + as-built numbers/finding:
[`bytecode-vm.md`](bytecode-vm.md). Supersedes the deferral in ADR-069 (which
banked the cheap dispatch wins and named the VM as the honest fix for the
tree-walker's structural tax).

**As-built note (Stage 0–1).** The bounded slice (top-level single-arm exact-arity
global-capturing closures; frame slots on `Heap::roots`; lexical-addressed
`Node::Local`; TCO) is correct and de-risks the GC-rooting crux (R1) — green under
`BROOD_VM=1 BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` — and full-suite parity holds. A
sharp lesson landed: the mechanism *alone* was ~10 % **slower**, because it
delegated every primitive op back to the tree-walker via `eval::apply`; the ~2× win
only appeared once `dispatch` reached primitives directly via the ADR-069
passthrough redirect (`(< n 2)` → `call_native(%lt)`). The takeaway — *a VM frame
that delegates primitives can't win; the speedup is in keeping the hot loop off the
tree-walker* — shapes Stage 2 (depth>0 lexical addressing for local-capturing
closures, multi-arity, more special forms, call-site inline caches).

**Context.** The tree-walker (`eval::eval`) re-pays per call: a special-form lookup,
an env-chain **name scan** per variable reference (`env_get`'s assoc-list walk), a
fresh frame allocation, cons-spine walking, and operand-stack rooting — all by
*interpreting the tree*. ADR-069 measured the structural tax at ~50–220× and
deferred lexical addressing partly because a `(depth,index)` reference as a runtime
`Value` would bump the type-system compatibility contract (new `Tag` + `Ty` bit +
GC/printer/wire support).

**Decision.** Replace the tree-walker with a **closure-compiling engine over a
lexically-addressed IR** (not flat bytecode). Each form compiles once into a `Node`
tree run by a trampoline structurally identical to today's `'tail:` loop; tail
positions compile to a `TailCall` outcome the trampoline loops on. Chosen over
bytecode for four codebase-specific reasons:

1. **GC rooting for free (the crux).** Frame slots are allocated as regions of the
   **existing** `Heap::roots` operand stack and addressed via `root_at(base+index)`,
   so `arena_flip` already relocates every live frame slot — **no new root set**. A
   bytecode VM would need its own root-array operand stack, forcing a rewrite of the
   most subtle correct code we have (`eval_arguments`' rooting).
2. **Keeps the invariant-enforcing trampoline** — the loop's `tick()` /
   `deadline_exceeded()` / `gc_due()` checks stay; the body just runs a compiled node.
3. **Lexical addressing needs no new `Value` tag** — the `(depth,index)` coordinate
   is compiled-node state, never a runtime value, dissolving ADR-069's objection.
4. **Multi-arity / passthrough / macros already key off the closure structures** —
   compile per `ClosureArm`; `select_arm` is unchanged.

Lexical addressing lands as a `lex_resolve` sub-pass in `eval::macros::compile`
(after `macroexpand_all` + `resolve`), turning the per-reference name scan into a
dense `Vec<Value>` frame-slot index — the single biggest win, and the deferred
ADR-069 Inc-3.

**Consequences.** Purely an execution-engine swap — the language, reader, `Value`,
primitives, and `std/*.blsp` are unchanged (invariant). Rollout is staged behind a
`BROOD_VM` flag with the tree-walker as a one-flag fallback and a **differential
test mode** (both engines must agree) guarding the transition: Stage 0 scaffolding
+ benchmarks → Stage 1 lexical addressing (the first milestone, de-risks GC rooting)
→ Stage 2 full compiler/trampoline → Stage 3 cutover. Invariants preserved
explicitly: proper TCO (frame-reuse), generational GC + operand-stack rooting (no
new root set), preemption/deadline (per-iteration checks), hot-reload (globals via
the version-stamped inline cache — never hard-bind a `ClosureId`), multi-arity,
immutability. Top risk is R1 (the VM stack as GC roots), mitigated by reusing
`Heap::roots` and gating on `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

**References.** [`bytecode-vm.md`](bytecode-vm.md) (the full plan, risk register,
data structures), ADR-069 (the deferral this resolves), ADR-061 (the operand stack
the VM reuses), ADR-054/055/072 (the generational copying GC `arena_flip` relocates),
ADR-047 (multi-arity), ADR-022 (the compile pass), ADR-026 (immutability), ADR-011.

**As-built update (2026-05-31): Stage 0–2c done.** One refinement to the plan
emerged in 2c. The merged VM does *not* address locals by `(depth,index)` and does
*not* rewrite bodies; it keeps a single flat frame on `Heap::roots` and resolves all
free names through `genv` (`Node::Global` → `env_get`). So a local-capturing closure
needed only: (a) running it with `genv = its own captured env` (`dispatch` reads
`closure.env`; `Step::Tail` carries it so tail calls can cross envs); (b) keying the
compile cache by the **body-code handle** (RUNTIME-stable), since a LOCAL
`ClosureId`'s index is recycled by the collector (`VmCacheKey::{Runtime,LocalBody}`);
(c) rooting the movable captured `EnvId` on `env_roots` in `vm_apply` and re-reading
it via an `EnvRoot` across collections (R1, the crux — gated green under the full
stress flags). Creating one (`Node::MakeClosure`) snapshots the enclosing lexicals
by value into a fresh frame (sound because Brood bindings are immutable), reusing
`eval::make_closure` to parse arms; the one case a value snapshot can't express —
capturing a not-yet-finalized `letrec` binder (recursive late-binding) — **defers to
the tree-walker**. The GC tracer was left unchanged: the cached `Node` tree is gated
to hold only immovable handles, so it's never a movable root (the simpler outcome
the original R1 note flagged as the alternative to walking `CompiledArm` bodies).
Then **source positions** were threaded through the IR (`Node::Call` carries a
compile-time `Pos` from `Heap::form_pos`; `exec_node` tags errors innermost-wins like
the tree-walker), closing the last divergence — the full suite is now green under
both engines. **Stage 3 cutover done (2026-05-31):** `vm_enabled` defaults the VM
*on*; `BROOD_VM=0` is the tree-walker escape hatch (kept ≥1 release); the transitional
`vm-default` cargo feature was removed. Also done (2026-05-31): the **differential
test mode** (`crates/lisp/tests/differential.rs` + `make test-both` — a corpus run
through both engines, asserting identical results; the standing CI guard); **variadic
arms** (`&rest` + nil-default `&optional`, with a full arity table so selection
reproduces `select_arm`); and **prelude-closure compilation** (stdlib `map`/`fold`/
`sort` etc. now VM-run — `sort_brood` ~1.0×→1.77×). The last required closing a latent
hole — `compile_node` defers a call whose head is an **unexpanded (forward-referenced)
macro** (via `macros::macro_head_id`), since the VM runs only expanded forms.
**Still open (pure perf, deferrals already correct):** pattern/`match*` and
real-default `&optional` coverage; bytecode lowering is premature (no profiling shows
node-dispatch dominating); retiring the tree-walker is infeasible until the VM is a
complete engine (it depends on the tree-walker for every deferred form). Unrelated:
the GC **RUNTIME-region collector** (hot-reload code churn) stays deferred (ADR-072
finished the LOCAL-heap GC; live-editing.md "Stage 5 later half").

## ADR-077 — Mouse `:drag` and `:release`, at cell granularity

**Status:** accepted (2026-05-30). Extends ADR-056's mouse vocabulary; resolves
the deferral ADR-056 itself flagged ("Release/drag are additive when a consumer …
needs them").

**Context.** ADR-056 gave both display frontends (crossterm `term-poll`, the GUI
`gui-open`) a deliberately minimal mouse vocabulary — `:press`, `:scroll-up`,
`:scroll-down` — and explicitly dropped release / drag / bare motion at *both*
backends, for one good reason: winit's `CursorMoved` fires per pixel, and a
consumer that refetches+redraws on every input would turn a mouse wiggle into a
redraw storm. There was also no consumer. The editor (`myedit`) now has one:
**Emacs-style split windows whose dividers you resize by dragging** — a gesture
that is exactly press → (track motion while held) → release, none of which the
vocabulary could express.

**Decision.** Add two actions to the shared `[:mouse action button row col]`
shape, identical across both frontends:

- **`:release`** — the held button coming back up (carries the button + cell).
- **`:drag`** — pointer motion *with a button held*, carrying that button + the
  new cell. **Throttled to cell granularity**: emitted only when the pointer
  crosses into a new character cell, never per pixel. This is the move that makes
  it safe where ADR-056 balked — a divider drag produces at most one event per
  cell of travel, not per pixel, so the redraw-storm footgun is gone.

**Bare motion (no button) is still not emitted** at either backend — no consumer,
and it would reintroduce the flood. So the vocabulary grows by exactly what the
drag gesture needs and no more (ADR-011).

**Mechanism.**
- *GUI* (`gui.rs`): each `Win` tracks the currently-held button (`held`, set on
  press, cleared on release). `CursorMoved` updates the tracked cell and, only on
  a cell change *while a button is held*, emits `:drag`. `MouseInput{Released}`
  emits `:release` and clears `held`.
- *Crossterm* (`builtins.rs::mouse_to_value`): crossterm already reports
  `Drag(button)` and `Up(button)` per-cell — mapped straight to `:drag`/`:release`;
  bare `Moved` still falls through to a nil poll.

One encoding from both frontends, so a single keymap/handler drives either — the
ADR-056 invariant holds. Rust tests (`mouse_event_tests`) lock the crossterm
mapping (incl. bare-motion-is-nil); the GUI path is the same `Mouse`→`Message`
shape.

**Consequences.** Purely additive to the input half of the seam: existing
`:press`/`:scroll-*` consumers (the observer) are untouched. Unlocks divider
drag-resize in `myedit`, and drag-select / drag-scroll generally, with no further
kernel change. **Mouse capture caveat unchanged:** the crossterm side reports
these only under `term-enter` (full-screen), not the inline REPL `term-raw-enter`
seam, which must keep the terminal's own text selection.

**References.** ADR-056 (the mouse vocabulary this extends, and whose deferral it
resolves), ADR-046 (the display/input seam), ADR-058 (GUI input as mailbox
messages), ADR-011 (ship the minimal form; additive features wait for a consumer).

**Addendum (2026-05-31) — held modifiers ride on the event.** The mouse event grew
a sixth element: `[:mouse action button row col mods]`, where `mods` is a vector of
the held modifier keywords in a stable `:ctrl :alt :shift` order (`[]` when none).
Both frontends fill it — the GUI from the window's tracked `ModifiersState`, the
terminal from crossterm's `KeyModifiers` — so an app can bind Ctrl+wheel (font
zoom via the per-window `gui-font!`, ADR-079), Ctrl+drag, etc. **This is a breaking
change, not additive:** Brood vector patterns are *fixed-length* and forbid a `&`
rest, so a consumer destructuring the old 5-vector (`[_ a b r c]`) silently stops
matching. The fix is positional access (`(nth ev n)`) or a 6-binder pattern —
`std/observer.blsp`'s `observe--apply-mouse` was migrated to `nth` (length-agnostic,
robust to any future element). Chose appending to the vector (over a `:ctrl-scroll`
action keyword or reshaping the event to a map) for generality across all actions
with the smallest shape change; the silent-break cost was accepted as the
greenfield norm (break + update callers).

## ADR-078 — Structured types: arrow + element refinements on the flat lattice

**Status:** accepted; **shipped 2026-05-30** (the first slice of Step 5+ in
[`types.md`](types.md)). Function-arrow types are in the `Ty` lattice and the
advisory checker uses them to flag callbacks of the wrong arity passed to the
higher-order combinators (`map`/`filter`/`reduce`/`fold`). Advisory throughout —
contract #5 holds (warns, never gates). Refines the Step 5+ sketch in ADR-024.

**Context.** Steps 0–4 left `Ty` a flat `u32` bitset over the runtime tags —
expressive enough for `int | string` or `not nil`, but it can't say *what kind of
function* a value is. So the biggest blind spot was higher-order functions: `(map
(fn (a b) …) xs)` (a 2-arg callback where `map` calls it with one) or `(map 5 xs)`
sailed through. [`types.md`](types.md) named the next move as Step 5+ "structured
types", sketched as an `enum { Set(u16), Arrow(..), Vec(elem) }` that *replaces* the
bitset.

**Decision.** Add structure as a **refinement on the flat bitset**, not as a
replacing enum. `Ty` becomes a struct `{ tags: u32, arrow: Option<Arc<Sig>> }`: the
tag bitset stays the coarse set (carrying the entire pre-Step-5 behaviour verbatim),
and `arrow`, when present, refines the function members (`Fn`/`Native`) to those
matching a specific signature. An arrow type *is* a [`Sig`] (params + rest + ret),
so the refinement reuses `Sig` rather than a parallel type. `(int) -> int` is
`{tags: Fn|Native, arrow: Some((int)->int)}`; a bare "any function" is the same tags
with `arrow: None`.

**Why a refinement struct over the sketched enum.**
1. **Union across kinds is natural.** `int ∪ (string -> int)` is just
   `{tags: Int|Fn|Native, arrow: …}` — the bitset already unions the tags, and the
   refinement attaches per-kind. A replacing enum would need a `Union(Vec<Ty>)`
   variant (a DNF of type frames), which is the bulk of the set-theoretic-algebra
   complexity ADR-011 says to defer until a consumer needs it.
2. **The flat case is unchanged.** Every existing `Ty` is `{tags, arrow: None}`, so
   the lattice ops degrade to exactly today's bitset algebra — proven by the
   pre-existing lattice-law unit tests still passing untouched.
3. **Advisory-soundness by construction.** The set operations may only ever *widen*
   the refinement toward `None` (= "any function") when they can't represent the
   exact result (union of two distinct arrows; negation; intersection of two known
   arrows). Widening over-approximates the set, so it can only ever *suppress* a
   warning, never manufacture a false one. `is_disjoint` is decided on tags alone
   and never inspects arrows — so an arrow mismatch can't be mistaken for
   disjointness (contract #5). The precise arrow check is a **dedicated step in the
   checker**, not something the generic lattice infers.

**Trade-off accepted.** `Ty` is no longer `Copy` (the `Arc` refinement), so it is
`Clone` (a `u32` + a refcount bump; the flat case is a null pointer). The churn was
contained by making the builtin/curated **type shorthands `const` items** (a `const`
mention re-materialises a fresh value, so the ~170 sig-table sites need no `.clone()`)
and by the compiler flagging the handful of real reuse sites. `Arc` (not `Rc`)
because `Sig` rides on `NativeFn` inside the `Arc<RuntimeCode>` region shared across
scheduler threads, which must stay `Send + Sync`.

**Arrow algebra.** Subtyping is **contravariant in parameters, covariant in the
result** (`Sig::is_subtype`), with arities required compatible — the standard
function-subtyping rule, kept as set inclusion (contract #3). A specific arrow `<:`
"any function"; "any function" is *not* `<:` a specific arrow.

**The checker payoff (this slice).** The curated sigs for `map`/`filter` carry a
1-ary callback arrow, `reduce`/`fold` a 2-ary one. When a parameter is a fixed-arity
arrow and the argument's callback arity is **knowable unambiguously** — a named
*global* function (arity from the heap) or a simple single-clause lambda literal —
the checker flags a callback that can't accept that count: `map: argument 1 is a
callback called with 1 argument, but cons takes 2`. Conservative by design: a local
variable, a variadic/`&optional` or multi-clause lambda, or a file-local name on the
read-only `--check` path all yield "unknown arity → skip", so there are **zero false
positives** (audited across the whole `std/` + `tests/` tree). The arrow's tags are
still `fn | native`, so the existing "non-function argument" check (`(map 5 xs)`) is
unchanged — the arrow only *adds* the arity refinement.

**Element types (second slice, shipped).** `Ty` gained the second refinement the
struct was designed for — `elem: Option<Arc<Ty>>`, refining the sequence members
(`pair`/`vector`) to their element type (`vector<int>` = `{tags: Vector, elem:
Some(int)}`). **Sources:** a vector literal `[1 2 3]` and the `(list …)`/`(vector …)`
constructors take the union of their element types (any unknown element → unrefined,
never wrong). **Sinks:** `(first xs)`/`(last xs)`/`(nth xs i)` flow the element type
out — widened with `nil` for the empty/out-of-range case — so `(+ 1 (first ["a"
"b"]))` is flagged (`string | nil` disjoint from `number`) while `(first [1 2 3])`
stays numeric. Element subtyping is covariant (sound — sequences are immutable);
union widens on a mismatch; `is_disjoint` stays tags-only (same advisory-soundness
rule as `arrow`). The refinements share the generic `merge_union`/`merge_intersect`
helpers. **Latent gap surfaced + fixed:** typing `(list …)` precisely meant the
`match` compiler's vector-pattern lowering `(if (and (vector? m) (= (vector-length m)
2)) (… (vector-ref m i) …) …)` tried to flag the guarded `vector-ref` against a
`list<int>` scrutinee. The root cause was occurrence typing not seeing through the
`and` short-circuit — so `guard_assertion` now narrows through the post-expansion
shape `(let (g E) (if g _ g))` (a truthy `and` ⟹ first conjunct `E` holds; `or`'s
`(if g g _)` deliberately doesn't match). General win beyond this case: any `(if (and
(pred? x) …) …)` now narrows `x` in the then-branch.

**Parametric HOF results (third slice, shipped).** Element types flow *through* the
higher-order functions: `(map f vector<A>) : nil | list<B>` (`B` = the callback's
return), `(filter pred coll)` preserves `coll`'s element, and `(reduce f init coll)`
/ `(fold f init coll)` give an accumulator typed `ty(init) | B` (`B` = the 2-arg
callback's return, accumulator over-approximated as `any` — a sound superset). Done
as **per-HOF result rules** in `check/guards.rs::seq_aware_call_ty` (Option B), *not*
type variables — no lattice change, the same mechanism `first`/`list` already use.
The one new inference is `callback_ret`: a named fn's sig result, or a straight-line
lambda's body typed with its params bound to the input types — *forward* result
typing only, never a body check, so it doesn't reopen the deferred guarded-use FP
class. Sound: uncertain callback/element/init → flat fallback. See
[`parametric-result-types.md`](parametric-result-types.md).

**Still deferred (⬜, ADR-011).** Arrow/element types in the straight-line
`infer_sig`; intersections for overloaded fns; **type variables** for user-defined
generics (Option A — no consumer yet).

**References.** [`types.md`](types.md) (Step 5+, the compatibility contract), ADR-024
(the set-theoretic/gradual model this extends), ADR-023, ADR-011 (ship the simple
form, defer power), ADR-006 (mechanism in Rust, the arrow/element algebra; policy
stays Brood). Lives in `crates/lisp/src/types/mod.rs` (the lattice) and
`crates/lisp/src/types/check/{sigs,walk,guards}.rs` (callback check, element flow,
and the `and`-guard narrowing).

## ADR-079 — Per-op font scale on the GUI `Face`

**Status:** accepted; **shipped 2026-05-31.** The GUI `Face` carries an integer
`:scale` (≥1, default 1, capped at 16); the renderer draws that op's text
`scale`× larger, occupying a `scale`×`scale` block of base cells anchored at the
op's `(row, col)`. The terminal frontend ignores it (renders 1×).

**Context.** On the GUI frontend there was exactly **one font size for everything**
— `Face` carried `fg`/`bg`/`bold`/`italic`/`underline`/`reverse`/`family` but no
size, and the grid is one global `cell_w`/`cell_h`. A "big heading", a larger
status strip, or a per-pane / per-buffer font was inexpressible except by a
hand-rolled "block font" magnified out of many cells (what the foobar Game-of-Life
demo's status strip did by hand). Recorded as **GG-1** in
[`known-issues.md`](known-issues.md); `gui-font!`'s `:height` only resizes the
*whole window*, not an op. `std/pane.blsp` (ADR-077/078) already supplies the
pane layout + clip-rect mechanism, so the only missing piece for per-pane fonts was
a per-op size.

**Decision.** Add the size to the existing per-op styling hook — the face — rather
than a new render op or a std block-font generator. `Face` gains `scale: u16`
(`gui_face` parses `:scale`, clamping to `1..=16`); the renderer rasterises the
glyph at `px * scale`, fills a `scale`×`scale` cell block, and advances `scale`
columns per char. Positions stay in **base-cell units**, so the uniform grid is
unchanged and an app lays a scaled region out by leaving `scale`-cell gaps —
"per-buffer font" is then pure Brood policy (a pane's text drawn with a face
carrying that buffer's scale). This resolves GG-1 and the per-pane-font remainder
of GG-3, and reduces the foobar block-font workaround to `[:text … {:scale n}]`.

**Why integer scale, not arbitrary `:height px`.** A faces-already-flow-end-to-end
addition over a new op (ADR-011: extend the existing hook, don't grow the protocol
shape — a new optional face key is forward-compatible, and the terminal + old
frames ignore it). Integer multiples keep the **single uniform grid**: text still
lands on base cells, so no new metrics-query primitive and no per-pane grid math is
needed. Arbitrary per-pixel sizing (14px vs 18px buffers) would break the single
grid and force a `gui-font-metrics` query into Brood for layout — deferred until a
concrete need justifies it (ADR-011).

**GG-2 follow-up (same day).** `gui-font!` was global across *all* windows — the
`UserEvent::Font` handler retuned every open window, so a second window couldn't
differ. Folded into this ADR (same font-seam surface, a small additive change):
`gui-font!` now takes an **optional leading window id** — `(gui-font! spec)` stays
the global default, `(gui-font! id spec)` retunes *just that window* and does not
touch the global default. The event carries `id: Option<u64>`; `id: Some(w)` looks
up the one window, `None` keeps the old "set defaults + apply to all" path (both
share an `apply_font` helper). So two windows can run different fonts side by side.

**Still deferred.** Arbitrary `:height px` per buffer (see above) — needs a
metrics-query primitive and breaks the single grid.

**References.** ADR-046 (the display-protocol seam + frame-as-data), ADR-011 (ship
the simple form, defer power), ADR-006 (mechanism in Rust, policy in Brood — the
pane/buffer font choice stays Brood). Lives in `crates/lisp/src/gui.rs` (`Face` +
the renderer) and `crates/lisp/src/builtins.rs` (`gui_face` parsing); documented in
`std/face.blsp`.

## ADR-080 — Cursor zones: pointer-shape hints carried by the frame

**Status:** accepted (2026-05-31). Adds a render op to ADR-046's protocol so an app
can show a resize cursor over a window divider (the affordance the editor's
drag-to-resize, ADR-077, was missing).

**Context.** The OS pointer shape can only be set by the GUI thread (it owns the
window), but `ui-run`'s `view`/`update` are pure and never hold the window handle —
only the `:draw` step does. And bare pointer motion is deliberately *not* delivered
to apps (ADR-056/077: it would flood the loop), so an app can't react to hover to set
the cursor itself. We need a way to say "show a resize cursor over this region"
without either plumbing the window handle into `update` or streaming motion events.

**Decision.** A new render op **`[:cursor-zone x y w h shape]`** (cells), where
`shape` is `:col-resize` (↔) or `:row-resize` (↕). It rides the **frame** — the data
the app already produces and that already reaches the right window via `:draw`. The
GUI frontend stores the zones from each frame and, on `CursorMoved` (which it already
tracks per-cell internally), sets the matching `CursorIcon` — or `Default` off every
zone — calling `set_cursor` only when the shape *changes*. The **terminal frontend
ignores it** (an unknown op, skipped), so one frame drives both. Constructor:
`std/display.blsp`'s `(cursor-zone x y w h shape)`.

**Consequences.**
- **Hover *and* drag for free, no new events, no flood.** The pointer sits on the
  divider while dragging, so a hover zone covers both; the GUI handles it locally,
  delivering nothing to the app loop (no redraw churn). This is why it's a *zone*
  (kernel-hit-tested) rather than a `:move` event stream.
- **Additive + frontend-neutral.** Existing apps/ops are untouched; the shape enum
  (`gui::CursorShape`) is mapped to winit's `CursorIcon` only inside the backend
  (`EwResize`/`NsResize`), so the shared `Op` stays dependency-free.
- The editor's `view` emits one zone per `std/pane.blsp` divider (`:col`→
  `:col-resize`, `:row`→`:row-resize`); resizing then has a real cursor affordance.

**References.** ADR-046 (the render-op protocol this extends), ADR-077 (the drag this
affords), ADR-056 (why bare motion isn't delivered — sidestepped by hit-testing zones
in the frontend), ADR-079 (the sibling GUI-`Face` work this lands alongside). Lives in
`crates/lisp/src/gui.rs` (`Op::CursorZone`, hit-test on `CursorMoved`) +
`crates/lisp/src/builtins.rs` (`gui-draw` parsing) + `std/display.blsp`.

## ADR-083 — Output ports (`*out*`/`*err*`) and an async, safe logger

**Status:** accepted, shipped (2026-05-31).

**Context.** `print`/`println` wrote straight to stdout via a Rust primitive.
A host like the editor (myedit) needs output to land somewhere *other* than
stdout — an in-editor buffer (`*Messages*`) — and the project needs a real
**logger** that is *async* (a log call must not block the caller) and *safe* (no
interleaved/garbled lines, no shared mutable state, an isolated failure). Both are
the same underlying need: *write a string to a sink*, where the sink might be a
process that owns a buffer.

**Decision.**
- **A port is a one-argument function `(fn (s) …)`** that consumes a ready string
  — nothing more (ADR-011: the simplest thing; a richer named/introspectable port
  value can come later behind `io-write` without changing callers). The prelude
  declares dynamic vars **`*out*`/`*err*`** holding the current ports and routes
  `print`/`println`/`eprint`/`eprintln` through them.
- **Rust keeps only mechanism, split in two** (ADR-006): `%render` (args → the
  space-joined display string, the exact text stdout would show) and
  `%write-out`/`%write-err` (a ready string → stdout/stderr, the former honouring
  the `with-out-str` capture stack). `*out*` defaults to `%write-out`, so
  `with-out-str` is unaffected and the default path is unchanged. Everything else
  — `std/io.blsp` (port constructors + `with-out`/`with-err`) and `std/log.blsp`
  (the logger) — is Brood.
- **The logger is one `hatch` process** (ADR-006) carrying `{:level :backends}`.
  A log call is a fire-and-forget **cast** → async; the single process serialises
  every write → safe (no interleaving) and isolates a crashing backend. A
  *backend* is an `io` port + a min level + a formatter, so the logger **reuses
  ports** rather than inventing a sink. The default logger is addressed via the
  kernel name registry (`register`/`whereis :logger`), with a stderr fallback when
  none runs so a log is never lost.
- **A buffer sink is a `process-port`/`process-backend`**: the string is *sent* to
  the buffer-owning process as `[:io-write s]` (copied, share-nothing), never a
  mutated value — consistent with immutability (ADR-026) and why it is safe.

**Rejected / deferred.** A tagged-map port value (named/introspectable — deferred
until `nest observe` wants it); a `*logger*` dynamic override (additive, deferred
until a consumer needs per-scope loggers); a string-collecting port (`with-out-str`
already covers capture). Building the logger on `std/task` (one-shot thunk+timeout,
wrong shape) or a hand-rolled receive loop (duplicates `hatch`).

**Consequences.** `print` now goes through a dynamic var + indirect call (a small
cost on a cold path, broadly worth the capability). Dynamic bindings don't reach a
`spawn`ed child, so `with-out` + `spawn` does not redirect the child — pass it a
port explicitly. `nest new`'s default scaffold starts a logger and documents the
buffer route. Lives in `crates/lisp/src/builtins.rs` (`%render`/`%write-out`/
`%write-err`), `std/prelude.blsp` (`*out*`/`*err*` + the four print fns),
`std/io.blsp`, and `std/log.blsp`; tested in `tests/io_test.blsp` +
`tests/log_test.blsp`.

## ADR-082 — Opt-in type annotations & runtime contracts (`sig` / `sig!`)

**Status:** accepted, shipped (2026-05-31). (ADR-081 is the concurrent dist
security-hardening decision; this work took the next free number.)

**Context.** Brood's type system is set-theoretic and **advisory** (ADR-023/024):
it warns on a provably-wrong call, never gates, and is engineered for zero false
positives. The Elixir paper (Castagna/Duboc/Valim, *The Design Principles of the
Elixir Type System* — notes in `docs/research/`) shows how such a system can be
made *sound* without inserting casts or changing compilation: the **strong arrow**
— a function that checks its arguments at run time can be trusted statically. We
want that soundness *available on demand* without compromising Brood's parameters
(greenfield, editor-serving, hot-reload, never-gate, policy-in-Brood) and without
ever forcing a user to write a type.

**Decision.** Two opt-in declaration **macros** — no new special form, no new
primitive:

- `(sig name (params… -> ret))` declares a signature the advisory checker reads
  *first* (ahead of primitive / curated / inferred sigs), so it flags a provably
  wrong argument or a wrong result against the declaration. A pure declaration —
  a runtime no-op. This closes the multi-clause / branchy gap that the
  straight-line `infer_sig` can't reach. Type grammar: base names (the `type-of`
  spellings + the named unions `number`/`list`/`fn`), arrows `(p… -> r)`,
  `(list E)` / `(vector E)`, and `(or A B …)`. An unrecognised type-expression is
  dropped, never guessed.
- `(sig! …)` declares the **same** signature *and* installs a runtime contract: it
  rebinds `name` to a **same-arity** wrapper that checks each argument and the
  result and **throws** on a mismatch. That makes `name` a strong arrow — applied
  off-domain it returns an in-codomain value, fails a runtime check, or diverges;
  it can never silently return an off-type value — so the checker's trust is now
  *sound*. All policy in Brood: `type-matches?` + `contract--check-args` + the
  `sig!` macro in `std/prelude.blsp`. Place it **after** the definition (it
  rebinds the name); the preserved arity keeps introspection and the reload-arity
  diagnostic undisturbed.

The spelling is `sig`, not the `::` first sketched, because a leading `:` lexes as
a keyword in Brood (so `(:: …)` is a keyword-headed list). Enforcement is
**separate and opt-in**: writing a *type* never changes behaviour or cost; opting
into a *runtime check* (`sig!`) does.

**Why this, not the alternatives.** Static gating / inserted casts would break
never-gate and hot reload. A sound-by-default checker would force annotations and
reintroduce false positives. Leaning on a runtime check the programmer opts into
is the only route that stays sound *and* additive *and* never in the way of live
redefinition — and it doubles as real declared types for the editor
(hover/completion). Unknown type-expressions accept any value, so a contract can
never throw on a type it can't interpret (no spurious runtime failure).

**Consequences.** `arglist` of a `sig!`-wrapped fn reflects the wrapper (minor
introspection cost). Re-`def`ing a name drops its contract (re-run `sig!` to
reinstall). The static checker also gained, alongside this: **soundness-oracle
tests** (every `expr_ty` over-approximates the runtime value; a clean-running
program draws no disjointness warning) and **curated sigs** for common predicates
(`even?`/`abs`/`count`/…). Deferred (ADR-011): a `BROOD_CONTRACTS=1` switch to
enforce *every* `sig`; element-level `(list E)` / `(vector E)` runtime checks;
intersections / rest params in the grammar; a noise-free dead-clause lint (a naive
version flags compiler-generated guard plumbing).

**References.** ADR-023/024 (set-theoretic advisory types), ADR-078 (the
structured `arrow`/`elem` refinements the checker reuses), ADR-011 (defer power
features), ADR-006 (policy in Brood). Design: `docs/type-annotations.md`. Review +
applied model: `docs/research/set-theoretic-types-in-brood.md`,
`docs/research/elixir-set-theoretic-types.md`. Lives in
`crates/lisp/src/types/check/annot.rs` (parser) + the `check/` walk + the contract
macros in `std/prelude.blsp`; tests in `tests/contract_test.blsp` and the
`soundness_oracle` module.

## ADR-081 — Node-link security: pre-auth DoS hardening now, authenticated-encrypted channel required for network nodes

**Status:** accepted (2026-05-31). The hardening half is implemented. **Update
(2026-06-01): the channel-encryption half (gap #1) is now implemented — see
ADR-089** (a Noise-style X25519 + ChaCha20-Poly1305 session over the `Stream`
seam, chosen over TLS because the reader/writer thread split can't drive one TLS
connection). (ADR-082 is the concurrent opt-in type-annotations work; these two
landed in the same session and split the numbers.)

**Context.** A security review of the distributed-node layer (`dist/`) — the only
surface that parses untrusted network bytes, and one that *ships closures* (code)
between runtimes, so it is RCE-by-design gated on authentication. The crypto and
deserialization held up well: HMAC-SHA256 handshake over a fresh CSPRNG nonce
(replay-resistant, constant-time compare, cookie never on the wire), a 256-bit
OS-CSPRNG cookie (`0600`), a wire decoder with a depth cap and remaining-bytes-
bounded allocations, no shell-based command injection, and identity keyed on the
*authenticated* node name rather than the wire's `from_node`. Three real gaps
surfaced — all confined to `dist/`; **none touch the language kernel**
(eval/heap/GC/value model unchanged):

1. **No channel confidentiality or per-frame integrity.** The cookie
   authenticates the *handshake*; steady-state frames are cleartext and carry no
   MAC. Over TCP, an on-path attacker who lets the handshake complete can inject
   forged frames afterward — including a `Send` carrying a closure → RCE —
   *without knowing the cookie*, and can read every message passively.
2. **Pre-auth resource exhaustion.** The acceptor spawned an unbounded OS thread
   per inbound connection, and the handshake read frames at the full 64 MiB
   `MAX_FRAME` ceiling, so an 8-byte probe (magic + length prefix) could commit a
   64 MiB allocation, and a connection flood could exhaust threads/FDs/memory
   before authenticating.
3. **Blast radius.** The machine-wide shared cookie (`~/.config/brood/cookie`)
   plus the documented `0.0.0.0` dual-listen example means one cookie leak grants
   RCE on every node on the host, reachable from the whole network.

**Decision.**
- **Fix #2 now (localized hardening, no kernel change).** A `HandshakeSlot` RAII
  permit over an `AtomicUsize` caps concurrent in-flight handshakes
  (`MAX_IN_FLIGHT_HANDSHAKES = 128`); the acceptor takes a slot *before* spawning
  a thread or reading a byte, and sheds past the cap by closing the socket — no
  thread, no log (a per-shed log would itself be a flood vector). Handshake reads
  use a tiny `MAX_HANDSHAKE_FRAME = 4 KiB` ceiling (`read_frame_capped`) instead
  of the 64 MiB steady-state one. The slot is held only for the pre-auth window
  and released on thread end (success/failure/timeout); steady-state links hold
  none.
- **Treat #1 as required, not optional, for network-facing nodes.** The
  long-deferred "optional TLS" is reframed: a node exposed on TCP over an
  untrusted network *requires* an authenticated-encrypted channel (TLS, or a
  Noise-style session over the existing `Stream { Tcp | Unix }` seam) that gives
  per-frame integrity + confidentiality, not just handshake auth. This closes
  both the passive-read and the post-handshake-injection holes in one move.
  Until it lands, the supported posture is: TCP nodes on trusted networks/VPN
  only; the Unix transport (in a `0700` dir) is fine locally.
- **Document #3 as policy.** Recommend binding to loopback/a specific interface
  unless network exposure is intended, and per-node cookies for network-exposed
  nodes; keep the machine-wide shared cookie as the local-convenience default.

**Why not encrypt the channel in this change?** Inbound/server-side TLS is a
separate, larger piece (`rustls` streams don't split read/write across threads
like a raw fd — the same blocker tracked under M4's server-side-TLS item), and it
belongs with the daemon/serving work. The DoS hardening is independent, cheap, and
worth shipping immediately; conflating them would stall the easy win on the hard
one.

**Consequences.**
- Pre-auth memory/threads are now bounded by a constant regardless of connection
  rate; legitimate peers are unaffected (128 is far above any real
  simultaneous-peer fan-in, and the 4 KiB cap is generous over any real
  handshake frame). All 24 real-TCP `distribution.rs` cases stay green.
- The security model is now explicit: **authentication ≠ a secure channel.** The
  cookie proves "you knew the secret at handshake time"; it does not protect the
  bytes after. Network deployments must wait for the encrypted channel.
- Closure-shipping remains RCE-by-design between *trusting* nodes — that is the
  Erlang model and the basis of hot code mobility, not a bug. If the hosted-editor
  threat model ever includes mutually-distrusting nodes (multi-tenant server),
  that is a *separate* design decision (no inbound code from untrusted peers, or a
  capability/sandbox boundary on inbound closures) and needs its own ADR before
  M4's multi-client server mode ships.

**References.** ADR-033/034 (closure shipping + handshake v2 this hardens),
ADR-068 (the `Stream` transport seam an encrypted carrier would slot into),
ADR-074 (dual-listen, whose `0.0.0.0` example motivates the #3 policy note),
ADR-043 (the memory cap that is a backstop, not a substitute for this bound).
Lives in `crates/lisp/src/dist.rs` (`HandshakeSlot`, `MAX_IN_FLIGHT_HANDSHAKES`,
`MAX_HANDSHAKE_FRAME`), `dist/handshake.rs` (capped handshake reads), and
`dist/wire.rs` (`read_frame_capped`).

## ADR-084 — Quasiquote is a compile/eval-time code transform, not a runtime walker

**Context.** A moving collector relocates LOCAL handles; a Rust frame that holds a
LOCAL handle across an `eval` call (which can collect at any safepoint, ADR-061)
must root it on the operand stack or it dangles. The historically worst offender
was the **runtime quasiquote walker** (`macros::quasiquote_depth`/`expand_seq`): it
evaluated each `~unquote` / `~@unquote-splicing` *inline* while accumulating the
partially-built result, the remaining template, and the env as LOCAL transients —
so it needed a hand-written `push_root`/`truncate_roots`/`teardown_err` rooting
dance around every recursion (the kind of bespoke discipline that is easy to get
subtly wrong, and the class of bug the GC audit kept circling).

**Decision.** Quasiquote is now a **pure structural transform that emits builder
code**, run at compile time (in the `compile` pass, after `resolve`) and as the
`eval` fallback for dynamically-constructed forms — never as a runtime walker.
`` `(a ~b ~@c) `` rewrites to `(append (list 'a) (list b) c)`; the *normal*
evaluator then runs that, so the unquoted sub-forms are ordinary `list`/`append`
operands the evaluator already roots. The transform itself calls **no `eval`**, so
it hits no safepoint and needs **no operand-stack rooting** — the entire bespoke
rooting dance is deleted. `expand_quasiquote` in `eval/macros.rs`.

- **Auto-gensym (`x#`)** resolves to a fresh symbol *in* the transform, once per
  template symbol per expansion. Because a macro body is re-expanded on every
  application, each expansion still gets distinct gensyms (Clojure-style hygiene).
- **Namespace qualification (ADR-065 §7) is unaffected.** `resolve` still descends
  the *template* and qualifies free refs at macro-definition time, before the
  transform turns the (already-qualified) template into builder code — so no
  pass-order change was needed.
- **Builder primitives.** The transform emits `list`/`append`/`vector`/`hash-map`/
  `apply`. `vector`/`hash-map`/`apply` are kernel builtins; `list` and `append` are
  prelude functions — but the first macro in the prelude (`defn`) uses a backtick
  template, so minimal seed `list`/`append` are defined at the very top of
  `std/prelude.blsp` (raw `def`/`fn`, no backtick), `def`-rebound by the full
  seq-generic versions further down.

**The general rule (the reason this is an ADR, not just a refactor).** The GC
hazard lives only at a **Rust frame that loops or accumulates LOCAL handles across
an `eval`**. Brood code is immune (the evaluator roots its own transients). So the
standing guidance: a Rust primitive that re-enters `eval` should be **single-shot**
— one bounded step that returns to the evaluator — rather than a loop that drives
evaluation while holding heap handles. When a primitive *would* need to accumulate
across `eval`, prefer expressing it as a transform-to-code (expand, then let the
evaluator run the code) or move it into Brood (ADR-006). Remaining rooted-Rust
re-entry points (the macroexpand fixpoint, `reload-defs`) are candidates to shrink
the same way.

**Status.** Done — `expand_quasiquote` + the prelude seed; the runtime walker,
`expand_seq`, and `teardown_err` are deleted. Verified: VM≡tree-walker differential,
the full in-language suite, and a quasiquote-heavy loop (runtime backtick with
unquote + splice + autogensym) green under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

## ADR-085 — `std/` is the basic-language core; frameworks are packages; hierarchical module names

**Context.** `std/` has grown to ~38 `.blsp` modules, and most of them are *not*
what a normal language's standard library ships. They're three other things wearing
the `std/` coat:

- **An editor / display framework** — `buffer`, `display`, `face`, `highlight`,
  `keymap`, `layers`, `pane`, `ui`, `lineedit`, `ansi`.
- **A net / web library** — `http`, `sse`, `tcp`; **a concurrency framework** —
  `hatch`, `supervisor`.
- **The project toolchain** — `project`, `package`, `test`, `docs`, `reload`,
  `mcp`, `observer`, `repl`, `sexp` (what `nest` is built from).

Everything is a **flat module name** (ADR-019/065): `(require 'buffer)` resolves the
bare stem `buffer`, and `defmodule buffer` qualifies its defs to `buffer/insert`
(one interned symbol; `/` is the namespace-qualified-name separator). Resolution is
two-tier — the embedded `BUILTIN_MODULES` table in `builtins.rs` (hand-maintained
`include_str!("../../../std/X.blsp")` lines, keyed by bare stem), then `<stem>.blsp`
searched across `*load-path*`. A flat name table means no shared prefix, no way to
say "the editor's `buffer`" vs "some package's `buffer`", and a directory listing
that doesn't reflect any structure.

The user's framing: **"std must be very basic functions for a normal language."**

**Decision.** Three coupled moves.

1. **Curate `std/` down to the basic-language core.** `std/` keeps only what any
   normal language ships — `prelude` (always loaded), plus the opt-in basics:
   `io`, `file`, `set`, `regex`, `json`, `fuzzy`, the `format` string formatter,
   `task`, `log`. Everything else leaves `std/`. The bundled stdlib should stay
   *small* ("keep the language as small as possible"); growth pushes **outward** to
   packages, never into more `std/` modules. (The exact in/out line per module —
   e.g. is `json` or `format` core? — is finalized when the move is done; the
   principle is settled, the boundary list above is the working proposal.)

2. **Frameworks and libraries ship as external packages**, installed through the
   package manager (ADR-037), not baked into the `brood` binary. The editor
   framework, the web/net library, the concurrency framework, and the future **GUI
   framework** are all *packages a project depends on* — not always-shipped core.
   (The project **toolchain** — `test`/`project`/`package`/… — is a separate
   category: it's what `nest` is built from, so it stays bundled, but it's
   *toolchain*, not stdlib, and is a candidate for a `tool/` namespace prefix.)

3. **Hierarchical module names** — the enabling language change, amending
   ADR-019/065. `(require 'gui/window)` loads the `gui/window` module/namespace,
   resolved from `gui/window.blsp` (and the embedded table keys on the full
   `"gui/window"` stem). The wrinkle: `/` already separates a qualified name's
   module from its def (`buffer/insert`), so module `gui/window` with a def `draw`
   produces the three-segment symbol `gui/window/draw`. **Split rule: on the *last*
   `/`** — module = everything before, name = the last segment. Touch points:
   the reader/resolver in `eval/mod.rs` + `eval/macros.rs` (`name.contains('/')`
   currently assumes one separator), `require--find` + `*load-path*` (search nested
   dirs), the `%builtin-module` table, and `unbound_namespace_hint`.

**Consequences.**
- The GUI question that started this is answered structurally: a GUI framework is
  *one external package* (`gui/window`, `gui/layout`), not a `std/gui/` subfolder.
- The package manager (ADR-037) becomes the primary distribution path for
  everything above the language core — which is why it was deliberately built
  before M2 (ADR-037 context).
- Migrating a framework out of `std/` is a breaking move (callers switch from a
  bundled `(require 'buffer)` to a package dependency). That's fine here
  (greenfield, no external users), but it wants a migration order that keeps the
  build/tests green at each step — and it depends on hierarchical names landing
  first, so the editor app (`~/src/whk/myedit`) and the in-tree test suite can
  follow.

**Status.** Decided (direction + the three moves). **Move 3 (hierarchical module
names) is done** (2026-06-01): `(defmodule gui/window)` qualifies defs to
`gui/window/draw` (split on the last `/`), loads from a nested `gui/window.blsp`,
imports via `(:use gui/window)`, and round-trips across processes — verified end
to end (`nest check`/`run`) and in the *hierarchical module names* block of
`tests/namespace_test.blsp`. The capability was almost entirely already there:
because a qualified name is one interned symbol over the flat table, the loader
(`require--find` path-joins the stem), `qualify_name` (formats `{ns}/{name}`), and
the resolver's `contains('/')` "already qualified?" guards are all
separator-count-agnostic. The only fixes were the two sites that *split* a
qualified name back into module + name: `semantic_tokens.rs` (`find('/')` →
`rfind('/')`) and `unbound_namespace_hint` (dropped the `!contains('/')` filter so
a hierarchical module is suggested). See [`namespaces.md`](namespaces.md) §3.
**Move 1 (curate `std/`) — the in-tree reorganization is done** (2026-06-01).
`std/` is now grouped on disk, and the framework modules are namespaced:

- **Core stays bare in `std/`** (a normal language's stdlib): `prelude`, `io`,
  `file`, `set`, `regex`, `json`, `fuzzy`, `format`, `task`, `log`.
- **Frameworks are namespaced** (the things Move 2 will externalize): `editor/*`
  (`ansi buffer display face highlight keymap layers lineedit pane ui`), `net/*`
  (`http sse tcp`), `proc/*` (`hatch supervisor`) — files under
  `std/{editor,net,proc}/`, modules `(defmodule editor/buffer …)`, referenced
  `editor/buffer/insert` / imported `(:use editor/buffer)`.
- **Toolchain is grouped but *not* namespaced.** `test project package docs
  reload mcp observer proctree repl sexp` moved to `std/tool/` **on disk**, but
  keep **bare module names** (`(defmodule test …)`, `(:use test)`,
  `test/run-tests`). This honours the rule that the *internal* toolchain stays at
  root (namespaces.md §10: the ergonomic `describe`/`test`/`is` macros stay root)
  — the directory groups them without namespacing their identity. The embedded
  `%builtin-module` table keys them bare (`"test"`) while pointing `include_str!`
  at `std/tool/test.blsp`, so `require` resolves the bare name to the grouped file.

The mechanical rewrite was a token-aware pass (skips comments/strings, leaves
`:keyword` face names like `:ui/header` untouched, rewrites only `defmodule`/
`require`/`:use`/`provide` module positions + non-keyword `mod/name` symbols);
the Rust eval-string bootstraps in the binaries + the embedded table were updated
to match. Full suite green.

**Move 2 (lift frameworks into external packages) — the clean slice is done**
(2026-06-01), and surfaced a structural limit worth recording: *most of the
framework can't actually leave the binary*, because the **bundled toolchain is
built on it**. The dependency walk found: `tool/observer` (`nest observe`) →
`editor/{display,face,highlight,keymap,lineedit,ui}`; `tool/repl` (the REPL) →
`editor/lineedit`; `tool/sexp` → `editor/buffer`; and core `log` → `proc/hatch`.
Those bundled features must work in a fresh runtime with no packages fetched, so
the modules they need **stay bundled**. Only modules with **zero bundled
dependents** externalize cleanly — and those are exactly what shipped:

- **`brood-net`** (`net/tcp`, `net/http`, `net/sse`) — removed from
  `CORE_MODULES`, now an internal package. Built on the kernel `tcp-*` primitives
  + bundled `file`.
- **`brood-supervisor`** (`proc/supervisor`) — likewise; `proc/hatch` stays bundled
  (core `log` needs it). The cross-node `supervisor_restarts_a_remote_child`
  distribution test, which shipped `(require 'proc/supervisor)` into a *bare*
  runtime, was reworked to inline the equivalent userland `monitor`-respawn.

**Internal packages skip the package manager.** An in-workspace package is not
fetched, hashed, or locked (ADR-037 is for *external/distributed* deps). It's
just a sibling `src/` directory put on `*load-path*`: a consumer adds it via
`:source-paths` (e.g. `brood-edit`'s `:source-paths ["src" "../brood-net/src"
"../brood-supervisor/src"]`), which `project-setup` appends to the load-path for
`run`/`test`/`check` alike, so `(require 'net/http)` resolves under it. No
`:dependencies`, no `project.lock.blsp`, no `_deps/`. (Externalizing *into the
package manager* — git deps, lock, distribution — only matters once a package
is shared across workspaces.)

Each package took its modules **and its tests** (`tests/*_test.blsp`) and, for
net, the `webserver` example. The takeaway (an ADR-085 refinement): the
"editor framework" is largely **shared UI the toolchain consumes**, not a
detachable app framework — so `editor/*` stays bundled until/unless the REPL +
observer are themselves repackaged (gated on a real consumer, ADR-011). The
editor *app* already lives outside the binary (`brood-edit`). Tracked in
`roadmap.md`.

## ADR-086 — GUI keys are press/release transitions, not an OS-repeat flood

**Status:** accepted (2026-05-31). The keyboard analogue of ADR-077 (which added
mouse `:release`/`:drag`); same motive — give the app the *transitions* it needs to
track a held input itself, rather than a producer-paced stream it can't keep up with.

**Context.** `myedit` had a visible input bug: hold `C-n` and release, and the cursor
kept scrolling for a beat *after* the key was up. The cause was the GUI key path
(`gui.rs`): it relayed **every** `ElementState::Pressed` event — including the OS's
auto-repeat — straight into the subscriber's mailbox, and the `ui-run` loop drains
**one** message per render (`std/ui.blsp` `:poll` is a single `receive`). When the OS
repeat rate outruns the render rate (easy under a heavy fontify), the mailbox grows a
backlog of `:ctrl-n`s; the release was *discarded* (only `Pressed` was handled), so on
key-up nothing cancelled the backlog and it kept "playing." A producer-driven repeat
with an unbounded queue and no release signal.

**Decision.** Make the GUI key vocabulary press/release transitions, so repeat is the
*consumer's* job (paced by its loop, stoppable on release) instead of the OS's:

- **Drop OS auto-repeat.** `Pressed` with winit's `ke.repeat` set is *not* relayed.
  A held key now yields exactly one down event.
- **Deliver releases.** `Released` → `[:key-up <key>]`, where `<key>` is the same value
  a press yields (`:ctrl-n`, `"a"`, `:up`). The press stays the **bare** value, so
  every existing keymap/dispatch path is untouched — release is purely additive (the
  ADR-077 move, for keys).

**Missed key-up — the hard part.** A press/release model is only as good as the
guarantee that the release arrives. The case where it doesn't is **focus loss**: you
Alt-Tab away mid-hold and let go in another window. Two backstops, belt-and-suspenders:

- **Honor *synthetic* releases.** winit marks focus-driven key events `is_synthetic`.
  We drop synthetic *presses* (focus-gain replays of still-held keys — they'd be
  phantom keystrokes) but **deliver synthetic *releases*** — they're precisely "the key
  was let go while you weren't looking," which is what must stop the repeat.
- **`:blur` on focus-out.** `WindowEvent::Focused(false)` delivers a `:blur` keyword, so
  even when no release comes at all the app has an unambiguous "stop everything" signal.

In-focus releases are always real (non-synthetic) key-ups, so they're never missed; the
only gap is focus change, and both backstops close it. A consumer can layer a hard
repeat cap on top if it wants absolute insurance, but it isn't needed for correctness.

**Consequences.**
- *Additive to the press path.* A consumer that ignores `[:key-up …]`/`:blur` (the
  terminal observer) is unaffected — it just loses nothing it had. Dropping OS
  auto-repeat *is* a behaviour change for any GUI consumer that relied on it: holding a
  key now emits one down, and the consumer must drive its own repeat off the down/up
  pair (which is the point).
- *Terminal is unchanged.* `term-poll` (crossterm) has no release events and is not
  touched; a terminal app keeps the terminal's own key repeat. So a portable consumer
  must treat GUI-style repeat as opt-in — `myedit` gates it on having actually seen a
  `[:key-up]` (a `:gui-keys` flag), so a release-less terminal never engages it and so
  can't run away with no release to stop it.
- *The myedit half.* Track `:held-key`; re-issue it on the refresh `:tick` at a short
  repeat beat (idle 60 s → 300 ms initial delay → 35 ms rate), restored to idle on
  `[:key-up]`/`:blur`. Repeat is now paced by the render loop — it can't outrun the
  screen and stops the instant the key lifts. (Editor-side; lives in `myedit/src`.)

**References.** ADR-077 (mouse `:release`/`:drag` — the same "give the app transitions"
move), ADR-058 (GUI input as mailbox messages), ADR-056 (the minimal display/input
vocabulary, grown only when a consumer needs it), ADR-011 (ship the minimal form).

**Addendum (2026-05-31) — `ke.repeat` is unreliable on Wayland; dedup by transition,
and expose the held key for a poll.** The original decision filtered auto-repeat with
winit's `ke.repeat` flag. On GNOME/Wayland that flag is **not reliable** — a held key
arrives as a flood of *fresh* presses with `repeat == false`, so the filter let the
flood straight through (it only ever worked on X11). Observed in the wild: holding a
key that opens a window spawned one window per repeat. Two changes close it properly:

- **Suppress repeat by transition, not the flag.** The GUI event loop now tracks the
  physically-held key in `Win.held_key` (set on a fresh press, cleared on its release
  or on focus loss). A `Pressed` for the key *already* held is the auto-repeat → drop
  it; a genuine re-press (double-tap) only arrives after a release cleared the slot, so
  it still registers. This is platform-independent (it doesn't consult `ke.repeat` at
  all) and kills the flood **at the source**, so no app has to work around it.

- **`gui-held-key id` — poll the source of truth.** That same `held_key` is exposed as
  a primitive returning the held key value (or nil). A consumer-paced repeat confirms
  the key is still down *each tick* before repeating — the games-engine pattern (re-read
  device state per frame instead of trusting accumulated edges). This is what makes a
  missed key-up structurally unable to run away: the very next tick polls nil and stops,
  regardless of whether the `:key-up`/`:blur` event was delivered. The events remain as
  the instant-stop fast path; the poll is the guarantee. (`myedit` threads the window id
  onto its model and gates the poll on it, falling back to the events on the terminal /
  in tests, where there's no window to ask.)

The `:key-up`/`:blur` events from the original decision stay; the flag-based filter is
replaced by the transition rule above.

## ADR-087 — Expose O(1) kernel facts (`map-count`) as primitives rather than recompute them in Brood

**Context.** "Write the language in the language" (ADR-006) says a capability
goes in Brood unless it genuinely needs Rust. `count`/`empty?` on a map were
pure Brood over the one map enumerator, `map-pairs`: `(count (map-pairs m))` and
`(%eq (map-pairs m) nil)`. But `map-pairs` *materialises* the whole entries list
(an O(n) walk + n freshly-allocated `[k v]` vectors) — so asking a map only how
*many* entries it has, or *whether* it has any, paid O(n) time and allocation
for a fact the CHAMP trie (ADR-040) already stores: every node carries the
`size` of its own subtree, so the root's `size` is the count, in O(1).

**Decision.** Add a thin Rust primitive `map-count` that returns
`Heap::map_size(id)` (the root node's `size`), and route `count`/`empty?` on a
map through it (`(map-count m)` / `(%eq (map-count m) 0)`). No `map-pairs`
allocation for a length or an emptiness test.

**Why this clears the "prefer Brood" bar (ADR-006).** The rule is *mechanism in
Rust, policy in Brood* — a primitive is justified when it exposes something the
language can't bootstrap cheaply, not when it merely moves behaviour out of
Brood for speed. The entry *count* is structural metadata the kernel data type
already maintains and that no Brood code can read without walking the structure;
exposing it is mechanism, exactly like `vector-length` or `string-length` (the
sibling O(1) length kernels `count` already used). It is **not** an escape hatch
— the policy (what `count`/`empty?` mean, the dispatch over collection types)
stays in `std/prelude.blsp`; only the irreducible "ask the trie its size" step
is in Rust. Contrast a *wrong* primitive — e.g. moving `frequencies` to Rust —
which would relocate real policy and teach us nothing.

**Sibling decision (same session): `%quot`.** `quot` was Brood
`(/ (- a (rem a b)) b)` — three dispatched ops per call, paid by every tight
integer loop. It now passes through to a `%quot` primitive (truncating integer
division), and the VM inlines the `Rem`/`Div`/`Quot` `PrimOp`s on `(Int, Int)`;
non-integer and edge cases (`÷0`, the `i64::MIN / -1` overflow) defer to the
native so semantics and error messages are byte-identical. Same shape as this
ADR: expose/inline an irreducible arithmetic step the language can't make fast
on its own, keep the surface in Brood.

**Consequences.**
- `(count m)` / `(empty? m)` are O(1) and allocation-free on maps; `frequencies`,
  `group-by`, and any `(count some-map)` caller stop paying an O(n) `map-pairs`
  pass purely to measure size.
- One more small entry in the map kernel surface (`map-get`/`map-assoc`/
  `map-dissoc`/`map-pairs`/`map-count`). The bar for the next one stays: a fact
  the structure already holds, not behaviour that belongs in Brood.
- After adding a Rust primitive, the embedded `nest`/`brood-lsp` binaries must be
  rebuilt (`make install`) or `nest check` flags the new name as unbound until
  the on-PATH toolchain catches up.

**References.** ADR-006 (write the language in Brood; mechanism vs policy),
ADR-040 (CHAMP map — the per-node `size` this reads), ADR-076 (the VM that
inlines the `%quot` family), `docs/transients.md` (the other CHAMP-aware kernel
hook, `%map-into`).

## ADR-088 — Nodes form a transitive cluster mesh (connect to one, join all)

**Context.** Distribution (ADR-033/034/068/073) gave us authenticated
point-to-point links: `(connect addr)` dials exactly one peer. The roadmap
explicitly left the **cluster-join topology** open — when A connects to B, does
it join B's whole cluster (mesh) or only B (point-to-point)? A user hit the
gap directly: with A, B, C running and A↔B + C↔B established, **A could not see
C**. There was no peer discovery at all — the wire carried only node *names*, no
reachable address, so B could not have told A *how to dial* C even in principle.

**Decision.** Adopt Erlang's default: **a full mesh with transitive discovery.**
Connecting to any one cluster member auto-connects you to every node it knows.
Three coordinated pieces:

1. **Advertise a reachable address.** The handshake `Hello` (wire v3, magic
   `BRD\x03`) now carries the sender's dial address (`unix:PATH` / `tcp:HOST:PORT`
   — the first TCP listener if any, else the Unix socket). It's **folded into the
   auth HMAC**, so an on-path attacker can't rewrite where peers will later dial
   without the cookie. Each link stores its peer's address (`Conn.addr`).
2. **Gossip the peer table.** When a *genuinely new* peer joins, the node
   broadcasts a `Frame::Peers` list of `(name, addr)` for its other peers to
   everyone connected — newcomer learns incumbents, incumbents learn newcomer.
3. **Dial the unknowns.** On receiving gossip, a node dials any peer not already
   connected (short-lived thread per dial; a `PENDING_DIALS` set dedupes
   concurrent gossip for the same name). Each new link re-gossips, closing the
   mesh transitively, then goes quiet (a reconnect/duplicate doesn't re-broadcast,
   so there's no steady-state chatter). Simultaneous cross-dials collapse via the
   existing connector tie-break (ADR-034 §1).

Mesh is **on by default**; `BROOD_NO_MESH=1` reverts to point-to-point.

**Why mesh over point-to-point.** It's what a user means by "act as a cluster,"
and it matches Erlang, so the global-namespace intuition (`(nodes)` shows
everyone, any registered name is reachable cluster-wide) holds. The roadmap noted
mesh's costs — O(n²) connections and a larger trust surface — but: cluster sizes
here are small (dev/editor-daemon scale), and the trust surface is already bounded
by the **cookie** (you only ever link to nodes that share it; an authenticated
peer can already ship closures = RCE per ADR-081), so auto-meshing within a
cookie-sharing cluster crosses no new boundary. The opt-out covers the deliberate
point-to-point case.

**Why these mechanisms.** Gossip-on-join (not periodic) means zero idle traffic
and obvious convergence (the last establish to complete sees the full table and
sends the cross-gossip; dials only fire for genuinely-unknown peers, so it can't
loop). Authenticating the advertised address closes the one new injection vector
the feature introduces. Reusing the connector tie-break means the simultaneous
dials a mesh inevitably creates need no new race handling.

**Consequences.**
- `(connect "b")` now joins you to B's whole cluster; `(nodes)` reflects the
  full mesh. The reported A/B/C bug is fixed; covered by
  `cluster_mesh_connects_peers_transitively` (+ the `BROOD_NO_MESH` opt-out test)
  in `crates/cli/tests/distribution.rs`.
- Wire format bumped to v3 (greenfield — no back-compat; a v2 peer is rejected at
  the magic prefix).
- **Deferred (ADR-011):** auto-reconnect / re-heal after a transient link drop
  (use `ensure-link`); FQDN/host-routability resolution beyond what `name@host`
  already assumes; a global cap on concurrent mesh dials (bounded today by
  `MAX_GOSSIP_PEERS` per frame). Mesh over an *untrusted* TCP network is now safe:
  the channel is encrypted + integrity-protected (ADR-089), exactly as point-to-point.

**References.** ADR-033/034 (closure shipping, handshake v2 + connector
tie-break), ADR-068 (Unix transport + cookie), ADR-073 (`name@host` identity),
ADR-081 (channel TLS — still required before untrusted-network exposure),
`docs/distribution.md` §Cluster mesh.

## ADR-089 — Node-link channel encryption: a Noise-style X25519 + ChaCha20-Poly1305 session over the Stream seam

**Status:** accepted + implemented (2026-06-01). Closes ADR-081's gap #1 (no
channel confidentiality / per-frame integrity) — the headline network-security
item. Confined to `dist/`; **does not touch the language kernel**
(eval/heap/GC/value model unchanged).

**Context.** ADR-081's security review found that the cookie handshake
authenticates only the *handshake*: steady-state node-link frames travelled
**cleartext with no per-frame MAC**. Over TCP an on-path attacker who lets the
handshake complete could (a) read every inter-node message passively, and (b)
**inject a forged `Send` carrying a closure → RCE** afterward — *without* knowing
the cookie. The roadmap forbade exposing a TCP node on an untrusted network until
this closed. ADR-081 named the fix as "an authenticated-encrypted channel (TLS,
**or a Noise-style session over the existing `Stream` seam**)".

**Decision — the Noise-style session, not TLS.** A live link runs **two
independent threads sharing an `Arc<Stream>`**: a reader (`&Stream: Read`) and a
writer (`&Stream: Write`). A single `rustls`/TLS `Connection` can't be driven from
both threads — it holds shared mutable crypto state and interleaves control records
with data. A **per-direction AEAD** maps exactly onto the split instead: the writer
owns the send cipher, the reader the receive cipher, neither sharing state. Node
identity is also cookie/name-based, not PKI, so TLS would need self-signed certs
pinned via the cookie anyway. So: keep the carrier; encrypt above it.

The scheme (`dist/session.rs` + `dist/handshake.rs`, wire v4):
- **Ephemeral X25519 ECDH** per handshake (forward secrecy — recorded traffic
  stays secret even if the long-term cookie later leaks). Each side puts a fresh
  ephemeral pubkey in its `Hello`.
- **Authenticated by the existing cookie-HMAC:** *both* ephemeral pubkeys are
  folded into the `Auth` MAC (alongside the names + addr already there, ADR-088),
  so a man-in-the-middle can't substitute its own DH key — a swapped `Hello.eph_pub`
  fails the MAC check, no cookie ⇒ no forged MAC.
- **HKDF-SHA256** (built on the in-tree `hmac`/`sha2` — no separate `hkdf` crate)
  over the DH secret, salted by `initiator_nonce ‖ responder_nonce`, → two
  directional 32-byte keys.
- **ChaCha20-Poly1305 AEAD per frame**, nonce = a per-direction monotonic counter
  (`[0;4] ‖ counter_be`). The Poly1305 tag *is* the per-frame MAC; a forged,
  tampered, replayed, or reordered frame fails to open and the reader tears the
  link down — closing the post-handshake injection hole. Counters never wrap
  (error at 2⁶⁴) and the two directions use different keys, so every (key, nonce)
  pair is unique — no reuse.
- **Handshake metadata stays plaintext** (names, nonces, ephemeral pubkeys, MACs)
  — none are secret; only steady-state frames, *including shipped closures*, are
  sealed. Applied **uniformly** over both Tcp and Unix (one code path; the local
  cost of a DH + per-frame ChaCha is negligible).
- Wire **magic bumped v3 → v4** (`Hello` gained the pubkey + steady-state is now
  encrypted); a v3 peer is cleanly rejected at the magic prefix (greenfield — no
  back-compat).

**Consequences.**
- A TCP node now has an **authenticated, forward-secret, integrity-protected**
  link. ADR-081's "trusted-network/VPN only" caveat for TCP nodes is **lifted**.
- **Authentication now implies a secure channel** — the cookie proves possession
  at handshake time *and* the session protects every byte after.
- **Closure-shipping between *trusting* nodes is still RCE-by-design** — that is
  the Erlang model and the basis of hot code mobility, not a bug. A
  mutually-distrusting / multi-tenant threat model (no inbound code from untrusted
  peers, or a sandbox on inbound closures) remains a **separate future ADR** before
  any multi-client server mode ships (as ADR-081 already flagged).
- The reader/writer thread split is unchanged — the property that made the
  per-direction AEAD the right fit (and TLS the wrong one) here.

**Tested.** `dist/session.rs` unit: seal/open round-trip, tamper-reject,
replay/reorder-reject, wrong-direction-key-reject, counter-advances.
`dist/handshake.rs` unit: MAC covers both ephemeral pubkeys (tamper ⇒ different
MAC), directional keys agree under role-flip + differ per direction. All 26
real-TCP/Unix `crates/cli/tests/distribution.rs` cases (incl. closure shipping,
mesh, monitors, links, supervisor, wrong-cookie rejection) stay green over the
encrypted path; full `make test` green.

**References.** ADR-081 (the gap this closes — pre-auth DoS hardening was the other
half), ADR-033/034 (closure shipping + handshake v2 this builds on), ADR-068 (the
`Stream` seam the session rides), ADR-088 (the addr-in-MAC pattern the pubkey-in-MAC
mirrors). Lives in `crates/lisp/src/dist/session.rs` (the AEAD framing),
`dist/handshake.rs` (DH + HKDF + key agreement), `dist/wire.rs` (the `Hello` pubkey
+ `encode_payload` + magic v4), `dist.rs` (`establish` threads the session into the
reader/writer).

## ADR-090 — Serving a `ui-run` app to remote frontends: app-on-daemon, thin client over the display seam

**Status:** accepted + implemented (2026-06-01). The headline **M4 deliverable** —
"the same runtime listens on a socket and serves the M3 protocol to attached
frontends (the Emacs `--daemon`/`emacsclient` model)." All Brood policy
(`std/editor/serve.blsp`) over the existing mechanism; **no kernel change**.

**Context.** The substrate was all built: node-connect (encrypted, ADR-089),
dual-listen (ADR-074), registered names, location-transparent `send`, monitors, the
M3 display protocol (a frame is plain send-able data), and `ui-run` with its
pluggable `display` map. `nest observe --connect` proved *remote rendering* — but in
the **pull** direction: the loop + model run on the *client*, which requests
snapshots. That's right for a read-only viewer; it is **not** the emacsclient model,
where the app (model + editing logic) lives in the daemon and the frontend is thin.

**Decision — run the app on the daemon; make one `ui-run` display a *network*
frontend.** The daemon runs the app's *unmodified* `(ui-run model view update
display)`; the only new piece is the `display`:
- **`remote-display`** — a `display` map bound to an attached client's pid: `:draw`
  `send`s the frame `[:frame f]` over the link (it's plain data), `:poll` `receive`s
  the client's `[:key k]`, `:size` is the size reported at attach, `:leave` tells the
  client to restore its terminal (`[:bye]`). A `[:detach]` or a monitor `[:down …]`
  (client died / link split) returns `:close`, which `ui-run` already treats as quit.
  This realizes ADR-046 literally: one display protocol, now a frontend that lives on
  the wire — so an app written for a local terminal serves remotely with no change.
- **`serve` / session manager** — `(serve make-model view update)` registers a manager
  under the well-known node name `serve-name` (`:ui`). Each `[:attach client cols rows]`
  spawns an **independent session** process that `monitor`s the client, tells it its
  pid, and runs `ui-run` against a `remote-display` to it. `make-model` is a thunk → a
  *fresh* model per client.
- **`attach` / thin client** — `(attach spec &optional cookie)` (and `nest attach
  SPEC`): `node-start` (ephemeral) + `connect` (clean error *before* the terminal) +
  `monitor-node`, then `term-enter`, report `term-size`, attach, and loop — drain
  pushed `[:frame f]` → `term-draw`, poll the local terminal → ship each key to the
  session — until `[:bye]` / link drop, always restoring the terminal. Teardown is
  **symmetric**: the session `monitor`s the client *and* the client `monitor`s the
  session, so either side's death (even an abrupt one with no clean `[:bye]` — a
  throwing `make-model` runs before `ui-run` can install its `:leave`) ends the other
  via `[:down …]` rather than hanging it.

The daemon side is a normal `nest run --name N app.blsp` whose `main` calls `(serve …)`
then parks; the only new CLI command is `nest attach` (mirrors `nest observe --connect`).

**Scope (ADR-011 — ship the slice).** **In:** app-on-daemon + thin client; many
concurrent clients (independent sessions); graceful attach / detach / client-death
teardown. **Deferred:** a *shared* model across clients (collaborative editing — each
session is independent; sharing is done by talking to a common process); live terminal
**resize** after attach (`:size` is fixed at attach); per-client viewports onto shared
buffers; a dedicated `nest serve` auto-park command.

**Consequences.**
- Any Brood `ui-run` app (the coming editor included) is now servable to remote
  terminals with no change to its `view`/`update` — "the frontend is a protocol" made
  real, the local leg (`nest attach foo` ≈ `emacsclient -s foo`) and the remote leg
  (`name@host:port`) being the same code over the encrypted link.
- The observer's *pull* remote-attach and this *push* serve are complementary: pull =
  inspect a runtime's processes; push = drive an app whose state lives server-side.
- Multi-tenant / mutually-distrusting serving is **not** in scope — closure mobility
  between trusting nodes is still RCE-by-design (ADR-081/089); a sandbox boundary is a
  separate future ADR.

**Tested.** `tests/serve_test.blsp` (in-process client plays the protocol): attach →
initial frame → key-driven frames → quit → `[:bye]`; per-client model isolation (two
clients each see their own count); a session that dies without a clean `[:bye]` (a
throwing `make-model`) still notifies the client via the monitor; `remote-display`
`:draw`/`:size`/`:poll` units.
`crates/cli/tests/serve_attach.rs` (cross-process, real encrypted TCP, in the
`real-tcp` group): a daemon serves a counter app, a TTY-less client attaches and drives
it (n=0 → n=1) and quits. Full `make test` green.

**References.** ADR-046 (the display-protocol seam this rides — "one protocol, many
frontends"), ADR-053 (the observer's *pull* remote-attach this complements), ADR-068
(node-connect by name), ADR-089 (the encrypted channel it serves over), ADR-074
(dual-listen — local + remote front doors), ADR-011 (deferring shared model / resize).
Lives in `std/editor/serve.blsp`; `nest attach` in `crates/nest/src/main.rs`.

## ADR-091 — RUNTIME-region collection: single-process compaction now; multi-process via a cooperative rolling quiesce later

**Status:** accepted (2026-06-01). The single-process collector is **implemented +
tested**; the multi-process collector is **designed here, deferred** (ADR-011). This
ADR supersedes the exploratory `docs/runtime-collector-exploration.md` as the source
of truth. No language-surface change beyond the `(runtime-collect)` builtin + the
`:runtime-*` keys on `(gc-stats)`.

**Context — two kinds of memory.** Brood's heap has a per-process **LOCAL** region
(private; collected by the generational copying GC, ADR-055/061/072 — no coordination,
each process collects its own) and one shared **RUNTIME** code region per runtime
(`RuntimeCode`, behind `Arc`, `boxcar`-backed append-only slabs). `def` / hot-reload
`promote`s a closure-graph into RUNTIME and rebinds the global; **old versions are
never overwritten** (append-only), so an in-flight call keeps running the version it
entered on while new lookups get the new one — Erlang-style hot reload (ADR-013,
`shared-code.md`). The cost: RUNTIME grows with redefinition churn and was never
reclaimed.

**Why the shared region can't just be collected per-process (the crux).** LOCAL works
per-process *because each heap is private*. RUNTIME is the deliberate exception — it's
**shared**, which is the whole point (a `def` must be visible to every process; making
code per-process would break hot reload). Reclaiming it means **compacting**: copy live
code to fresh slabs and free the old. But code is addressed by bare integer **handles**
(slab indices), and those handles are held *everywhere at once* — in every process's
private LOCAL heap (a captured RUNTIME closure), on execution stacks (mid-call), in live
compiled-VM arms, and in the global table. Moving entry `#100 → #50` requires every
holder, in every process, to rewrite "100"→"50" with no reader observing a half-done
state. So reclaiming the shared region is fundamentally **more than per-process**: (a)
liveness is a *union* question — a version is dead only if *no* process references it;
(b) the swap must be atomic w.r.t. all readers.

**Decision — Step 1 (done): single-process compaction, gated by `Arc::get_mut`.**
`Heap::runtime_collect` evacuates the live RUNTIME graph into a fresh `CodeSlabs` and
rewrites every reference in one pass: globals, this process's roots/env-roots/dynamics,
both LOCAL generations, and the live compiled-VM arms; per-process caches (`vm_cache`,
`global_ic`) are cleared (rebuilt lazily); a forwarding table + `OnceLock`
reserve-then-fill handle DAGs/cycles; `verify_rt_slabs` asserts no dangling handle.
The eval safepoint calls it automatically (`maybe_runtime_collect`, adaptive
`rt_gc_threshold = max(BROOD_RT_GC_FLOOR(4096), 2·live)`); `(runtime-collect)` forces
it and returns `{:before :after :reclaimed :ran}`. **It runs only when this heap
uniquely owns the runtime `Arc`** — which is *exactly* the condition that makes it
sound without any stop-the-world: a uniquely-owned runtime has **no other readers**, so
the single owner safely rewrites its own handles + the globals and swaps. This bounds
the REPL and any single-process hot-reload loop (`nest run --watch` of a non-spawning
program). With live spawned processes the `Arc` is shared, the gate declines, and
`(runtime-collect)` reports `:ran false` (a safe no-op) — verified by
`tests/runtime_collect_test.blsp`; the reclaim/rewrite mechanics by
`crates/lisp/tests/runtime_collector.rs` (3000 redefs → live <50 → compacted; the
auto-safepoint bound; a LOCAL-held handle rewritten across a collect).

**Decision — Step 2 (designed, deferred): a cooperative rolling quiesce.** Because the
scheduler is *cooperative* (processes yield at the eval safepoint) and each process's
`Heap` lives on its own coroutine stack (unreachable from outside — so a coordinator
cannot rewrite another process's handles; each must rewrite its own), the multi-process
collector is a **rolling quiesce**, not a hard freeze:
1. A coordinator builds the new compacted region + a forwarding table from the *union*
   of all processes' roots (each process contributes its RUNTIME roots at its safepoint).
2. The **old region is kept alive** (a second live `CodeSlabs`); handles resolve against
   whichever region they belong to until migrated, so nothing dangles mid-migration.
3. Each process, at its next safepoint, applies the forwarding table to its own
   heap/roots/arms (self-rewrite) and acknowledges the new epoch.
4. The old region is freed only once **every** process has migrated.
Open wrinkles to resolve when built: a permanently-**parked** process pins the old
region (needs a wake-to-migrate or epoch-bounded escape hatch); handles may need a small
region/epoch tag to resolve against two live regions; and the read path may move to an
`ArcSwap<CodeSlabs>` (an atomic load per code read — a measured cost). This is the
largest, most race-prone remaining kernel piece, gated on a real long-lived
multi-process server demonstrating the need (the M4 daemon, ADR-090, is the candidate
consumer) — exercise it under `BROOD_GC_STRESS` across the worker pool before shipping.

**Consequences.** Hot-reload churn is bounded for single-process use today, with
`(gc-stats)` `:runtime-closures`/`:runtime-threshold` + `(runtime-collect)` for
visibility. A long-lived server with live processes still accretes superseded code until
Step 2 lands — acceptable for now (normal sessions are negligible; the dedup of
structurally-unchanged redefs, ADR-042, already curbs the common case).

**References.** ADR-072 (the generational LOCAL GC this reuses the trace/forward/verify
machinery from), ADR-013 + `shared-code.md` (why RUNTIME is shared — hot reload),
ADR-055/061 (the safepoint + operand-stack rooting), ADR-042 (unchanged-redef dedup),
ADR-011 (deferring Step 2). Lives in `crates/lisp/src/core/heap.rs`
(`runtime_collect*`, `maybe_runtime_collect`, `flush_rt_*`, `verify_rt_slabs`,
`rt_gc_*`), `builtins.rs` (`runtime-collect`, the `gc-stats` `:runtime-*` keys).

## ADR-092 — Editor syntax grammars are generated from the language's own introspection

**Status:** accepted + implemented (2026-06-01). Pure Brood policy
(`std/tool/grammar.blsp`) + a thin `nest grammar` shim; the only kernel change is
widening the canonical `SPECIAL_FORMS` list.

**Context.** Brood's editor integrations each hand-maintained the same "vocabulary."
The kernel already has the canonical special-form / core-macro list (`SPECIAL_FORMS`,
exposed as `(special-forms)` and used by the LSP semantic tokens + the REPL
highlighter), but `brood-mode` repeated it (`brood-special-forms`), the new
`brood-vscode` extension repeated it again (its TextMate alternation), and a future
`tree-sitter-brood` would make three. They drifted — e.g. `brood-mode` highlighted
`spawn`/`error` while the canonical list didn't.

**Decision.** **Generate the editor grammars from `(special-forms)` — one source of
truth.** A small Brood tool (`std/tool/grammar.blsp`, dogfooding — ADR-006) turns the
canonical list into a VS Code **TextMate** grammar (`(tmlanguage)` → JSON) and the
**Emacs** `brood-special-forms` defconst (`(emacs-special-forms)`), surfaced as
`nest grammar [tmlanguage|emacs]` (the `nest doc` model; prints to stdout, redirect to
the editor's grammar file). Only the keyword *alternation* is data-driven (escaped,
longest-first so `->>` beats `->`); the rest of the grammar (comments, strings, the
`def…`-head name-capture rule, `:keywords`, numbers) is fixed structure. Built on the
existing `(special-forms)` + `json-encode`.

**Reconciling the drift — promote, don't demote.** Where `brood-mode` highlighted more
than the canonical list (`spawn`, `spawn-link`, `remote-spawn`, `remote-spawn-sync`,
`error`, `with-out-str`, `bench`), we **added those to the kernel's `SPECIAL_FORMS`**
(new `kw::` consts; they're highlight-only, *not* evaluator special forms — the
evaluator keeps its own narrower `SPECIAL_SPELLINGS`). So every consumer now colours
them from one place: VS Code (via `nest grammar`), Emacs (regenerated defconst), the
REPL highlighter, and the LSP semantic tokens / completion. Adding a future special
form means editing `SPECIAL_FORMS` once, then regenerating — no per-editor edits.

**Consequences.**
- `brood-vscode/syntaxes/brood.tmLanguage.json` is now **generated** (`nest grammar >
  …`), not hand-maintained; `brood-mode`'s `brood-special-forms` is the generated
  canonical set (marked "regenerate with `nest grammar emacs`").
- VS Code/the REPL gained keyword colouring for the process/error macros; Emacs kept
  its richer highlighting — unification *upward*.
- `tree-sitter-brood` (the Neovim/Helix/Zed/GitHub *parser*) is one more emitter over the
  same `special-keywords`: `nest grammar tree-sitter` emits its `queries/highlights.scm`
  (a `#any-of?` over the canonical set — literal node-text, no regex escaping). The grammar
  itself (`grammar.js` + an external scanner mirroring `atom::classify`) is a faithful model
  of the reader, validated against the whole `std/` + `tests/` corpus.
- Macros not promoted (anything still outside `(special-forms)`) are coloured by the
  LSP's semantic tokens as functions, not by the static grammar — the intended split.

**References.** ADR-006 (policy in Brood), ADR-052 (`(special-forms)` shared with the
LSP/REPL highlighter), the central `kw::` spelling module (devlog 2026-05-30). Lives in
`std/tool/grammar.blsp`, `crates/nest/src/main.rs` (`nest grammar`),
`crates/lisp/src/builtins.rs` (`SPECIAL_FORMS`) + `core/keywords.rs` (the new consts);
consumed by `brood-vscode` and `brood-mode`.

## ADR-093 — Native char-class scanners + `scan-tokens`: lexing mechanism in Rust, faces in Brood

**Status:** accepted + implemented (2026-06-02). Three new builtins
(`string-span`, `string-span-until`, `scan-tokens`); the Brood fontifier
(`std/editor/highlight.blsp`) is rewired to walk `scan-tokens`. No semantic change to
`highlight-spans` (its tests are unchanged).

**Context.** Syntax fontification is on the editor's render hot path — re-lexed on every
edit and on scroll past the cached band (ADR: the editor's `:span-cache`). The lexer
(`hl--lex`) was pure Brood, scanning character-by-character via tail recursion:
`highlight-spans` cost ~0.5 ms/line interpreted, so a screenful was ~25 ms and a
margin-widened band ~150 ms — enough to make typing and scrolling feel sticky in a large
file, even with windowed fontification and the span cache. Profiling showed the cost was
two interpreted hot loops: the per-character advance (whitespace/atom/comment scanning)
and the per-token classification (`special-form?` was an O(n) `includes?` over the whole
special-form list; `hl--number?` ran `string->number` on *every* atom).

**Decision.** **Put the lexing *mechanism* in Rust and keep the colouring *policy* in
Brood.** Three builtins:

- `(string-span s start chars)` / `(string-span-until s start chars)` — forward
  char-class run scanners (skip a run *of* / *until* a char set), char-indexed like
  `substring`. The general primitive any tokenizer's inner loop wants; the markdown
  lexer's line scan and the highlight bracket/call matchers use them too.
- `(scan-tokens s)` — a lossless lexical token stream for Brood source: a vector of
  `[start end kind text]` (`:comment :string :number :keyword :symbol :open :close`),
  whitespace skipped, strings escape-aware. One native O(n) pass.

`highlight-spans` now walks `scan-tokens`, assigning faces over O(tokens) — the only
per-token work left in Brood. Crucially the **head-position** rule (a `:symbol` right
after `(` is a special form or a call) and the **face map** stay in Brood: `scan-tokens`
classifies lexical category (using data Rust already owns — `SPECIAL_FORMS` isn't needed
here; number-parsing matches `string->number`), and Brood decides what each category
*looks like*. Result: ~5× faster (`highlight-spans` 26 ms → 5 ms for a 50-line viewport,
148 ms → 31 ms for a 288-line band), so a per-keystroke band re-lex is ~11 ms.

Two adjacent pure-Brood wins shipped with it: `special-form?` is now an O(1) set lookup,
and `hl--number?` gates the `string->number` parse behind a first-char check.

**Consequences.**
- The mechanism/policy seam matches ADR-006: char scanning genuinely needs Rust (a
  per-char interpreted loop is the bottleneck); faces + head-position are Brood, editable
  live. `scan-tokens` is general tooling (a sibling of `parse-source`), reusable by
  structural tools and completion, not highlight-specific.
- `hl--lex` / `hl--atom-face` / `hl--constants` are removed (dead); `hl--number?` and the
  bracket/call matchers stay, now reading the native scanners.
- The markdown lexer got the cheap `string-span-until` swap for its line scan; its
  per-char *inline* scanner (emphasis/links) is a deferred follow-up — it has no
  `scan-tokens` analogue yet.

**Follow-up — the render-side tiler (2026-06-02).** Profiling the *render* (paid every
frame, not just on edit) showed `fontify-runs` — the per-visible-line span→`[substring
face]` tiler — was the next interpreted hot loop. Its no-overlay path (the common case:
no region/overlay crosses the line) is pure positional slicing with face coalescing, so
it became a fourth native builtin, `(span-runs text base spans)` — same mechanism/policy
split (faces stay opaque Values, re-emitted as-is). Warm `ed-view` ~29ms → ~24ms. A
follow-up extended it with an optional `ranges` arg `(span-runs text base spans ranges)`
that tiles by the union of span + range edges and merges overlay faces per segment
(`into` semantics, via the heap's `map_from_pairs_into`) — so a region/isearch overlay
during a **drag-select** renders as O(segments), not the old O(chars) per-char merge:
`ed-view` with a viewport-spanning region ~50ms → ~17ms. The whole Brood `fontify-runs`
is now a one-line call into it; the per-char `fontify-runs--*` helpers are deleted.
Separately, a flood of per-cell mouse `:drag` events (ADR-080) made a fast drag render
cell-by-cell, so `editor/ui`'s `gui-display` poll now coalesces queued drags to the
latest (`ui--coalesce-drag`) — render once per gesture step, not once per cell crossed.

**References.** ADR-006 (mechanism in the kernel, policy in Brood), ADR-052
(`highlight-spans` shape, `(special-forms)`), the editor's per-frame span cache. Lives in
`crates/lisp/src/builtins.rs` (`string_span`/`string_span_until`/`scan_tokens`/`span_runs`),
`std/editor/highlight.blsp`, `std/editor/markdown.blsp`.

## ADR-094 — `overlay-route`: the modal-overlay dispatch fallthrough lives in `editor/ui`

**Status:** accepted + implemented (2026-06-02). One small `std/editor/ui.blsp`
addition (`overlay-route` + `overlay-active`); the editor and the observer both adopt
it. No behaviour change to either app.

**Context.** A `ui-run` app's `update` typically has a few *transient* modes that sit
beside its keymap and capture input while open: the editor's minibuffer / completion
popup / incremental search / query-replace, the observer's eval minibuffer. Each app had
hand-rolled the same fallthrough rule — route a key to whichever overlay is open; the
overlay that *owns* the key handles it, any other key dismisses the overlay and is
re-dispatched normally. The editor expressed it as a `{:active? :owns? :handle :exit}`
handler list + `ed-route-transient`; the observer as an inline `cond`. Two copies of one
rule, and a third app would be a third.

**Decision.** Move the rule to `editor/ui` (the `ui-run` framework module both apps
already build on): `(overlay-route overlays model input fallback)` routes `input` to the
first active overlay or to `fallback`, with `:owns?` nil = capture-all and a non-owned
key running `:exit` then `fallback` (dismiss-and-process). The *overlay list* stays each
app's own data (its modes, its model shape); only the dispatch policy is shared.

- The editor's `ed-route-transient` is now a one-line call; `ed--transient-active` /
  `ed--transient-owns?` are deleted.
- The observer routes its eval-minibuffer (`:command` mode) + keymap tail through it.
  Its `:confirm` (kill confirmation) stays an explicit branch **above** the mouse case —
  that's a deliberate precedence (any input, even a click, resolves a pending kill rather
  than shifting the list under it), not the overlay-fallthrough shape, so it isn't forced
  into the router.

**Consequences.** The dispatch rule has one home and one test (`tests/ui_test.blsp`); a
new modal feature in either app is a list entry, not new control flow. The seam is the
same spirit as ADR-046 (one `ui-run` loop, many apps): shared *mechanism* in
`editor/ui`, per-app *policy* (which overlays, what they do) in the app.

**References.** ADR-046 (the `ui-run` framework). Lives in `std/editor/ui.blsp`
(`overlay-route`/`overlay-active`); consumed by `brood-edit`'s `input.blsp` and
`std/tool/observer.blsp`.

## ADR-095 — OS clipboard: `clipboard-get` / `clipboard-set!` builtins (the `clipboard` feature)

**Status:** accepted + implemented (2026-06-02). Two builtins behind a `clipboard`
feature (pulled in by `gui`), via the `arboard` crate. The editor's kill/copy/yank sync
through them.

**Context.** The editor's kill ring was internal-only — copy/cut/paste couldn't exchange
text with other apps. Brood had no clipboard access (it's an OS capability, not pure
data), so this is a genuine kernel-level gap (a `--with-gui` editor that can't paste from
the browser isn't a real editor).

**Decision.** Add `(clipboard-get)` → text-or-nil and `(clipboard-set! s)` → s, native via
`arboard` (text only; `default-features = false` drops the `image` dep, `wayland-data-control`
matches winit's dual X11/Wayland support). Gated behind a `clipboard` feature so the lean
runtime / headless tests link no clipboard stack — there the builtins are graceful no-ops
(`get` → nil, `set!` → its arg), so callers needn't branch.

- **Process-lifetime handle.** On X11/Wayland the selection *owner* must stay alive to
  serve paste requests, so a fresh `Clipboard` per call would lose the text the instant it
  dropped. The handle lives in a `OnceLock<Option<Mutex<Clipboard>>>` for the process; init
  failure (no display server) caches `None` → no-op, no retry.
- **Editor wiring (policy in Brood).** `commands/ed-push-kill` mirrors every new kill-ring
  head to the clipboard; `cmd-yank` first adopts the clipboard if it differs from the ring
  head (Emacs `interprogram-cut/paste-function`). Both gate on a model `:os-clipboard` flag
  the *live editor* sets (`main.blsp`) but pure-model tests omit — so tests never touch the
  process-global clipboard, which would make them order-dependent.

**Consequences.** Copy/cut/paste are system-wide. The `clipboard` feature is independently
toggleable; a non-clipboard build is unaffected. A right-click context menu (next) drives
the same commands by mouse.

**References.** ADR-046 (frontends), ADR-006 (mechanism in the kernel, policy in Brood).
Lives in `crates/lisp/src/builtins.rs` (`clipboard` mod + the two builtins), `Cargo.toml`
(`clipboard` feature / `arboard`), `brood-edit`'s `commands.blsp` + `main.blsp`.
