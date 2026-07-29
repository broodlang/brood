# Design decisions (ADR log)

An **ADR** is an *Architecture Decision Record* — a short, dated note capturing
one design choice and *why* we made it, so we don't accidentally relitigate
settled questions. Newest at the bottom.

> **Dangling links are expected in older entries.** Entries are historical records
> and are not rewritten, so a few cite design/audit docs that have since been
> deleted — `concurrency-v2.md`, `supervision.md`, `memory-review.md`,
> `incremental-check.md`, `vm-perf-and-jit-runway.md`, `image-cache-plan.md`,
> `feedback-retro-game-of-life.md` (most trimmed in `fdce540` once the features
> they planned had shipped). Their content is superseded by the ADR that cites
> them, the topic doc, or the source itself; recover the text from git if needed.

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
**Still proposed, not built:** ADR-071 *(WASM extensions)*. ADR-119
*(incremental `nest check` cache)* has since shipped in full (Phase 1 + Phase
2) — stale entry, corrected here rather than left to relitigate. ADR-123
*(whole-program soundness under hot reload)* has shipped in full (ADR-124 +
ADR-125 + ADR-126) — its batch/CI hard-gate was believed unbuilt but turned
out to already exist (`nest check` has always exited 1 on any warning).

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
| 088 | Nodes form a transitive cluster mesh (connect to one, join all) |
| 089 | Node-link channel encryption: a Noise-style X25519 + ChaCha20-Poly1305 session over the Stream seam |
| 090 | Serving a `ui-run` app to remote frontends: app-on-daemon, thin client over the display seam |
| 091 | RUNTIME-region collection: single-process compaction now; multi-process via a cooperative rolling quiesce later |
| 092 | Editor syntax grammars are generated from the language's own introspection |
| 093 | Native char-class scanners + `scan-tokens`: lexing mechanism in Rust, faces in Brood |
| 094 | `overlay-route`: the modal-overlay dispatch fallthrough lives in `editor/ui` |
| 095 | OS clipboard: `clipboard-get` / `clipboard-set!` builtins (the `clipboard` feature) |
| 096 | VM perf as the JIT runway: one road, not two |
| 097 | Batteries-included default install; split + rename the process framework |
| 098 | Shrink the core: drop the `lambda`/`let*` aliases; demote `defmacro` to a macro |
| 099 | `proc/gen` is a real gen_server: `info`/`init`/`terminate` + a call timeout |
| 100 | Full process migration is a stepping-VM change, not a corosensei swap; fresh-only stealing is the migration-free partial |
| 101 | JIT compilation: three-layer assembly model, Cranelift backend, calling convention |
| 102 | Named timers for the `ui-run` loop |
| 103 | Foreign-language parsing: one `tree-sitter-parse` builtin into the existing node shape, not an opaque tree resource |
| 104 | Persistent child processes: a `Value::Subprocess` over the mailbox seam, not a richer `%os-cmd` |
| 105 | Keyword-literal (singleton) types: a literal-set refinement on `Ty` |
| 106 | Telemetry: handlers run in an isolated listener process (never the emitter) |
| 107 | `table`: an in-memory shared store (Brood's ETS) as a Rust-backed handle of deep clones |
| 108 | `lambda`/`let*` are exact synonyms for `fn`/`let` (canonicalised at macroexpand) |
| 109 | `string-split` is a native builtin (not pure Brood) |
| 110 | Gradual typing earns its place: `GradualTy`'s first consumers (assignment / return / value-position checks) |
| 111 | Lazy seq-views: fusing pipelines as an opt-in combinator, `map`/`filter` stay eager |
| 112 | Brood data is immutable, absolutely: remove user-facing transients; `Table` is the only mutable structure |
| 113 | mimalloc as the allocator backend (spend memory for speed; Brood targets long-running apps) |
| 114 | Keep the moving collector; the JIT already sidesteps stack maps, so harden the spill-to-roots discipline instead of switching to mark-sweep |
| 115 | Record/shape types: `(record :k T …)`, full `fields` refinement |
| 116 | Intersection of arrows: overloaded functions via `(and A B …)` |
| 117 | Int-literal types: `5` as a type, the first slice of ADR-105's deferral |
| 118 | Match exhaustiveness checking over literal-enum types |
| 119 | Incremental `nest check` cache: designed, not built (defer per ADR-011) |
| 120 | Bool and string literal types |
| 121 | Match exhaustiveness generalized to mixed-kind literal enums |
| 122 | Match redundancy / unreachable-clause detection |
| 123 | Whole-program soundness under hot reload — designed, not built |
| 124 | Cross-module visibility for declared value-type sigs (ADR-123 slice 1) |
| 125 | `nest run --watch` re-checks on reload — ADR-123's live-session trigger |
| 126 | `defmodule`-declared arrow sigs now seed the body-return-type check |
| 127 | `&optional` params in `(sig …)` arrow grammar |
| 128 | Tuple / positional product types |
| 129 | `build-id` keys off the running binary's own mtime, not just git-sha |
| 130 | `defrecord` is pure prelude sugar over closed maps, not a new `Value` kind |
| 131 | Dead-clause lint broadens to precise surface `let`-locals (not just sig-typed params) |
| 132 | `Control::Kill`: `(exit …)` reaches a process blocked in a native-nested `receive` |
| 133 | `|…|` bar-quoted symbols and keywords for round-trip printing |
| 134 | `editor/buffer-client`: the client half of the buffer-process protocol |
| 135 | The top-level program is a green process (everything is a process) |
| 136 | `require` is a concurrency contract: no observer sees a half-loaded module |
| 137 | Runtime events: a push system monitor (`system-monitor`), consumed by telemetry |
| 138 | The boot cache: expanded-prelude text, not a binary heap snapshot |
| 139 | Iolists: write boundaries take nested string/bytes trees, flattened once |
| 140 | Bit syntax: typed integer segments in the bytes pattern, pure Brood |
| 141 | Byte-faithful sockets: binary mode is inbound-only, carrier strings are gone |
| 142 | No growable read-buffer value; reads are chunk lists, scans are incremental |
| 143 | The socket reactor: one mio thread for every socket; queued writes; TLS everywhere |
| 144 | The dirty-native offload pool: blocking natives park a process, not a worker |
| 145 | WASM component interop, slice 1: the sandboxed native-extension host |
| 146 | Module privacy is enforced; `(:use-internals mod)` is the grant |
| 147 | Package manager v2: tarball deps + a git-backed registry |
| 148 | Test coverage is function-level, instrumented by hot reload |
| 149 | A binding container is a **list**; a vector there is an error |
| 150 | The pattern pin is `^expr`, not `~expr` |
| 151 | Ambient names are **declared** (`defdyn`), not spelled (`*earmuffs*`) |
| 152 | Reject the shape; never reinterpret it |
| 153 | `sig` adoption: annotate `std/`, and what that exposed |
| 154 | Ergonomics & conciseness pass: add the missing sugar, cut the redundant surface |
| 155 | `receive` clause bodies compile into the *calling* function, not into a per-message thunk |
| 156 | The collection protocol covers every collection; a misread shape is an error, not a reading |
| 157 | A literal `if` test picks its branch at compile time |
| 158 | Protocols move into `std/`: the polymorphism seam ships with the language *(value dispatch superseded by ADR-168; `defbehaviour` stands)* |
| 159 | Grapheme-*indexed* string accessors: make the correct spelling the fast one |
| 160 | Alternative (`or`) and conjunction (`and`) patterns; map keys are sub-patterns |
| 161 | Transducers become public surface |
| 162 | Retire the `lambda` alias: `fn` is the only spelling |
| 163 | The convention questions the syntax review raised, settled |
| 164 | `get`/`nth` diagnostics: an error must name the operation the caller wrote |
| 165 | A keyword is callable as an accessor; nothing else data-like is |
| 166 | Reserved names: the language's own functions cannot be redefined |
| 167 | Keyword accessors are typed, not just callable |
| 168 | `ability` is the one value-dispatch seam; `defprotocol`/`defimpl` retired |

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
them today). The endgame (M2–M4) is *an editor that is a running Brood image
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
  `[:nodedown …]` with a 200ms backoff until success. *(Superseded 2026-07-18:
  the reconnector is now `std/net/reconnect` — exponential backoff, idempotent
  named watchers, `[:nodeup]`/`[:nodedown]` subscriber events — and
  `ensure-link` was removed from the prelude; see the devlog.)*
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
[`ROADMAP.md`](../ROADMAP.md) M2, [`types.md`](types.md) compatibility contract.

## ADR-046 — The display/input seam: a frontend is a protocol of render-op data

**Status:** accepted (2026-05-29). The first M3 substrate — the seam between the
runtime and any frontend (local terminal today, a socket peer later).

**Context.** The editor must feel native locally *and* serve remote frontends,
from one codebase (architecture.md). The way to get that for free is to make the
display layer a **protocol, not a library**: the runtime emits a serialisable
stream of "render this" operations and consumes input events; the local frontend
implements that protocol in-process (the fast path), and a remote frontend
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
  and the M4 socket frontends will speak.

**References.** ADR-006 (mechanism/policy), ADR-045 (the rope, the other editor
substrate), ADR-005 relaxation (runtime-substrate crates), ADR-011 (ship the
simple form), ADR-043 (the root-vs-worker thread + stack model),
[`architecture.md`](architecture.md) (the seam), [`ROADMAP.md`](../ROADMAP.md) M3.

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
[`ROADMAP.md`](../ROADMAP.md) M1 ("Memory reclamation" — the cumulative-memory story
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
  frame; both share `apply_face`, so a future remote frontend interprets one more
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
- **Completion now *lists* when it can't extend** (2026-07-28). Tab shipped
  insert-or-common-prefix, which is silent on an ambiguous prefix: the user sees
  nothing happen and can't tell whether completion exists, is broken, or has simply
  run out of shared characters. The readline convention fixes the ambiguity for free —
  make progress if there is any, otherwise **show the alternatives** — so
  `lineedit--apply-completion` attaches the candidates as `:completions` exactly when
  the common prefix adds nothing, and the renderer paints them in dim columns below the
  input. Deliberately *not* a cycling menu or ghost text: both need a mode (what does
  the next key mean?), while a listing is stateless — `lineedit-handle` drops it before
  every dispatch, so it survives one keystroke and no command can leave it stale.
  Capped at `*lineedit-completion-max-rows*` (6) because the renderer's geometry is
  relative: a listing tall enough to scroll the terminal would desync the `[:up n]`
  cursor restore, the same constraint the one-line signature hint already lives under.
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
`ROADMAP.md` M3.

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
staged GC plan), [`ROADMAP.md`](../ROADMAP.md) M1.

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
[`ROADMAP.md`](../ROADMAP.md) M1. Stage C (generational nursery) deferred.

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
[`ROADMAP.md`](../ROADMAP.md) M3.

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
general pattern (terminal, sockets, an offload pool) is planned.

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
reader-thread → mailbox precedent),
[`ROADMAP.md`](../ROADMAP.md) M3/M4.

## ADR-060 — Sets: a library over maps, then promoted to a first-class `#{…}` kernel type

**Status:** accepted as a library (2026-05-30, `std/set.blsp`); **the deferral was
reversed and `#{…}` promoted to a first-class `Value::Set`/`Tag::Set` kernel type on
2026-07-24** — see the "Promoted to the kernel" follow-up below. The original
library-over-maps decision text is retained as historical record; the library now
survives as sugar over the kernel type, so its function names/meanings are unchanged.

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
- **Promoted to the kernel — shipped 2026-07-24 (supersedes the original
  deferral).** A `#{…}` reader literal, `#{…}` printing, and a distinct
  `Value::Set`/`Tag::Set` all landed: the full compatibility contract
  (`docs/types.md`) was paid — a new `Value` variant + `Tag` + type-lattice bit
  (`ALL_TAGS`), `value::tag`, GC trace/promote/copy-on-send (`Message::Set` + the
  dist wire codec), structural hash + `equal`, a `ConstVal::Handle` kind, and the
  reader/printer/evaluator/macroexpander arms. The set is still backed by the CHAMP
  map (`element → true`) so it reuses the map storage verbatim; it is its OWN kind
  only at the value/tag boundary (`set?` true, `map?` false, `type-of` `:set`, and
  a set is **never** `=` to a map). The `set` library became Brood sugar over the
  kernel ops (`%set`/`%set-add`/`%set-remove`/`%set-has?`/`%set-count`).

**Consequences.**
- A set and the equivalent `{… true}` map are now **distinguishable** (`set?` vs
  `map?`, distinct print, never `=`) — the original deferral's accepted cost is
  paid off. Existing `map?`-based set tests were updated to `set?`.
- The new `Value` variant added ~30 `Set` match arms across the kernel (GC,
  promote, region predicates, verifier, eval/compile/macroexpand); the compiler's
  exhaustiveness enforced most, and `GC_STRESS`+`GC_VERIFY` covered the
  wildcard-guarded GC paths a set shares with maps.

**References.** ADR-006 (write the language in the language), ADR-011 (defer power
features), ADR-040 (CHAMP map the set rides on), [`ROADMAP.md`](../ROADMAP.md)
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
other opaque handle).

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
  **deliver to the mailbox** — the same rule as TCP,
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
(resource backstops — fuel/epoch), ADR-059/062
(deliver-to-mailbox offload), ADR-054/055 (moving/generational GC — why the
boundary marshals), ADR-006 (write the language in the language — wrapper + policy
in Brood), ADR-011 (defer power features).

---

## ADR-072 — Stage C: a generational nursery + tenured old generation

**Status:** accepted (2026-05-30). The "make copying fast as well as stable"
refinement deferred by `docs/memory-review.md` §6 and ADR-055; the last remaining
GC item. Builds directly on the single-space copying collector
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
"copying gets fast" point), `docs/memory-model.md`,
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

**Move 2 (lift frameworks into external packages) — REVERSED by ADR-097**
(2026-06-07): `brood-net` and `brood-supervisor` were never finished (the
package dirs were deleted from the binary but never created), and the project's
direction changed to **batteries-included** — every framework module ships in the
default install. `net/*` and `proc/supervisor` are bundled in `CORE_MODULES`
again; there are no internal framework packages. The text below is retained as
the historical record of the externalization attempt. See ADR-097.

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
`ROADMAP.md`.

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

## ADR-091 — RUNTIME-region collection: single-process compaction now; multi-process via an Erlang-style 2-generation model

**Status:** accepted (2026-06-01; multi-process direction revised 2026-07-07). The
single-process collector is **implemented + tested**. The multi-process collector
pivoted from the original cooperative rolling quiesce (deferred, hard) to an
**Erlang-style 2-generation model** (Step 2 below); Stages 1a/1b/2/3a/3b/3c **and all of
Stage 4 — the free mechanism, live-globals migration, and the safepoint auto-arming state
machine — have landed.** The multi-process collector is now **unconditional** (2026-07-09):
a shared runtime always reclaims via the 2-generation machine, single-process compaction
still handles the uniquely-owned case, and the two perf blockers that had kept it opt-in
(the drain self-report walk cost + the per-deref `ArcSwap`) are resolved — full suite at
parity. Only a **purge policy** for a permanently-pinned (genuinely looping) generation
stays deferred. This ADR supersedes
the exploratory `docs/runtime-collector-exploration.md` as the source of truth. No
language-surface change beyond the `(runtime-collect)` builtin + the `:runtime-*` keys
on `(gc-stats)`.

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

**Decision — Step 2 (in progress): the Erlang-style 2-generation model.** The
cooperative rolling-quiesce sketch (below, superseded) tried to *compact + rewrite
handles across every process* — the largest, most race-prone kernel design in the
repo. We replaced it with what Erlang's code server actually does: **at most two
generations of code, no rewriting.** RUNTIME becomes two slabs (`gens: [CodeSlabs; 2]`
+ an atomic `current_gen`); a RUNTIME handle carries a 1-bit `code_gen` tag (GEN bit
32, kept distinct by the region-aware `canonical()`), so both generations resolve
simultaneously with **no handle migration** — the whole reason the rolling quiesce was
hard. `def`/`promote` mint into the *current* generation; **aging** (`age_runtime`)
flips `current_gen` to the other slot so new code lands there while the previous
generation keeps executing in-flight calls (exactly Erlang's *old* vs *current*). The
old generation is freed **whole** once no live process references it — reclamation
driven by process lifecycle + a cooperative liveness scan, never a per-cell trace of
the shared region. The 2-versions-max rule (aging refuses when the target slot is
non-empty) means a third redef must wait for the old generation to drain (Erlang
*purges* stragglers; we soft-wait for pins) — bounding the region to two versions,
not unbounded churn.

Why this is the right shape for Brood: **data is immutable and per-process**, so the
shared region holds *only code*, and code is only ever *appended* (hot reload never
mutates a live closure). That means a generation is a pure add-only epoch — we can drop
an entire old generation atomically instead of compacting live cells, sidestepping the
cross-process handle-rewrite entirely.

Progress: **Stage 1a** (handle `code_gen` tagging + region-aware `canonical()`),
**Stage 1b** (two-slab `RuntimeCode` + generation-aware accessors, behavior-preserving),
and **Stage 2** (generation-tagged `promote` + the `age_runtime` flip + the two-gen
read-path test) have landed. Normal runs never age (`current_gen` stays 0), so behavior
is unchanged until the reclamation stages arm it. **Stage 3a** (the per-process
liveness *probe*, `Heap::runtime_gen_referenced(gen)`) has also landed: a read-only walk
of the shared roots (globals + declared sigs) plus this process's private roots (operand/
env stack, dynamics, both LOCAL heap generations, and the live VM arms mid-execution),
returning whether generation `gen` is still referenced — the per-process half of the
Stage 3 union, exact for a single-process runtime. **Stage 3b** (the cross-process
union *mechanism*) has also landed: shared drain-coordination state on `RuntimeCode`
(`drain_active`/`drain_gen`/`drain_epoch` + a `pid → clean-epoch` ack map) plus
`Heap::{begin_gen_drain, report_gen_liveness, gen_drained, end_gen_drain, clear_gen_ack}`.
A drain arms a strictly-monotonic epoch and clears the acks; each process reports at its
safepoint (a clean process acks the epoch, a pinning one drops its ack); `gen_drained`
answers the union — every live pid acked the current epoch — with the caller supplying
the live-pid set (kept out of `core` for layering). It is **inert until a drain is armed**
(the always-case), so it's behavior-preserving with zero hot-path change. Soundness rests
on the post-aging invariant that new code only lands in the *current* generation, so a
clean ack stays clean (a global can never re-point at the drained generation). **Stage 3c**
(wiring the union into the live scheduler) has also landed: the cooperative report fires at
**both** engines' eval safepoints — the tree-walker loop and the VM trampoline
(`vm_run_bc`) — gated on a single `drain_active()` atomic load so it's inert (and free)
when no drain is armed; `process::{current_pid, live_pids, report_drain_liveness,
old_gen_drained}` supply the live set from the scheduler `REGISTRY` and answer the union.
Soundness across parked processes rests on the same no-new-refs invariant: a process that
acked clean can't reacquire a reference post-aging (so the probe needn't scan a parked
continuation), and a pinning process simply has no current-epoch ack (blocking the drain,
safely). Per-exit `clear_gen_ack` was left unwired — the ack map is cleared at every drain
boundary and dead pids are never queried (pids aren't reused), so it's pure hygiene with no
correctness role; the method stays for Stage 4's use. **Stage 4 (the free *mechanism*)**
has landed. Two pieces:

1. **Freeable storage — the generation slabs became `[ArcSwap<CodeSlabs>; 2]`.** A dead
   generation can't be reclaimed while the runtime `Arc` is shared: the append-only
   `boxcar` slabs can't be cleared through `&self`, and `Arc::get_mut` never succeeds with
   live processes (so the single-process compactor path can't run). `ArcSwap` lets
   `free_runtime_gen` **store a fresh empty slab** through `&self`; the old `Arc<CodeSlabs>`
   drops when the last reader guard releases it. The cost is that the reference-returning
   RUNTIME accessors (`closure`/`string`/`vector`/`map_node`/`env_frame`/`rope`/…) now hand
   back a guard-holding **`SlabRef<T>`** (derefs to `&T`) instead of a bare `&T`, so a read
   in flight during a free keeps the slab alive — safe by construction (chosen over an
   unsafe in-place swap that would rest on the drain invariant). `promote`/`def` append into
   the loaded slab's `boxcar` in place, so a store only ever happens on a free — never on the
   hot `def` path.
2. **The free — `Heap::free_runtime_gen(old_gen)`** (driven by `process::free_drained_gen`,
   gated on `old_gen_drained`): stores the empty slab, bumps `version` (self-invalidating
   the version-stamped `global_ic`/call-&-global ICs/shared JIT caches across every process)
   and a new `free_epoch` (each process lazily clears its handle-keyed `vm_cache` — which
   isn't version-stamped — on its next lookup, so a **reused slot's bit-identical
   `(gen,index)` handle can't hit a stale compiled body**). Verified by
   `reused_slot_runs_new_code_not_stale_cache` (define→call→free→age-into-freed-slot→define
   →call runs the new code, not the cached old body) and `free_reclaims_after_cross_process_drain`
   (two heaps; free refused while a peer pins, succeeds once released, slot then reusable).

**Stage 4 (the auto-arming — live-globals migration + the safepoint state machine)** has
now also landed, and is **unconditional** (no flag — a shared runtime always reclaims this
way). Four pieces:

3. **Live-globals migration — `Heap::migrate_live_globals(old_gen)`.** The design point
   [`age_runtime`](#) surfaced: aging only flips which slot new code lands in; it moves no
   existing binding. Because Brood's `def` is per-global (unlike an Erlang module reload,
   which re-exports *all* a module's functions as a unit), a global defined once and never
   redefined would stay in its birth generation forever and pin it. So aging is now paired
   with migration: it re-exports the live globals + `declared_sigs` into the current
   generation (reusing the compaction `flush_rt_*` machinery, generalised to be
   *gen-selective* — forward only `old_gen` nodes — and mint *dest-gen*-tagged handles),
   leaving the aged-out generation holding only superseded + in-flight code. The reframe: this
   is compaction where the "forwarding" is done by **retaining** the old generation (old
   handles stay resolvable) until its holders drain, instead of the un-coordinatable
   cross-process handle rewrite the abandoned rolling-quiesce needed. The reconcile installs a
   migrated handle only where the global still resides in `old_gen` (a concurrent
   redefinition — which lands in the current generation — wins), so it needs no value
   equality: after aging, `old_gen` is frozen. Single-flight via a `begin_aging`/`end_aging`
   CAS.
4. **The state machine — `Heap::advance_runtime_multigen`**, driven at the RUNTIME safepoint
   (both engines) when compaction can't run (shared runtime): *drain in flight* → free once
   the union is clean; *idle + other slot empty* → age + migrate + arm the drain; *idle +
   other slot occupied* → wait (the 2-versions-max back-pressure). Migration runs **before**
   arming the drain, so no process can newly acquire an `old_gen` reference once the drain is
   live — the invariant the "clean stays clean" report optimisation rests on (a process that
   reports clean for a drain epoch is not re-walked, bounding it to one liveness walk per
   drain).
5. **A promote⇄age soundness fix.** The concurrency test exposed a real bug: a generation
   flip on one process could interleave with an in-flight `promote` on another — `promote`
   reserves a slot in the current generation then fills it re-reading `cur_code()`, so a flip
   in between made the fill hit the *wrong* generation's slab (a panic / cross-generation
   split). Fixed with a `promote_lock` `RwLock`: promotion holds it **read** (concurrent
   appends to a lock-free `boxcar` are fine), aging holds it **write**, so no promote ever
   spans a flip. Uncontended on the default single-generation path.

Verified: five deterministic mechanism tests in `runtime_collector.rs` (migration re-exports
a stable global so its generation frees; migration preserves a post-aging redefinition; the
full age→migrate→drain→free cycle repeats and stays bounded; plus the Stage-4 free tests),
and an end-to-end `runtime_multigen.rs` (real workers churn a global; the collector ages +
migrates mid-flight and never miscompiles — every
`(f 0)` stays 0). JIT-native code executing an old generation is handled by the drain gate
(a process running it references the generation, so its probe blocks the free) plus the
`version`/`free_epoch` invalidation.

**Stage 5 (the soft purge — parked-process drain inspection)** has now landed. A generation
frees only once *every* live process reports clean, and reporting happens at an eval
safepoint — but a process **parked** in `receive` never reaches a safepoint, so a drain
armed after it parked could never collect its ack, and an idle server parked on
current-generation code would block every later drain forever (the parked-can't-ack
problem). Fixed the way Erlang's `check_process_code` does it — by *external inspection*: a
paused process's continuation is relocatable heap data (ADR-100 — its live values sit on its
own `Heap`'s `roots`/`env_roots`/`live_vm_arms`), and the scheduler holds that heap in the
mailbox's `waiter` slot, so the drain coordinator (`old_gen_drained` →
`report_parked_liveness`) walks each parked process's *own* quiescent heap and lets it ack
if it's clean of the draining generation. No wakeup, no kill — a parked-clean process stops
blocking the drain; a process genuinely paused *in* old code stays dirty (correct — it will
resume that code). This is strictly a soft purge: it never removes a live pin, only stops a
*false* pin (a clean-but-unreported parked process) from stalling reclamation. Verified in
`runtime_drain.rs` (a worker parks on gen-1 code while a drain of gen 0 is armed; the drain
completes only because the parked worker is inspected and acked — it deadlocks without the
inspection).

**Stage 5 soundness — re-home a `def`'d value out of the draining generation.** A later
review found the drain gate had a hole: the shared **globals table** (and `declared_sigs`)
is a drain root that no *process* liveness walk covers. `promote` is a no-op on an
already-RUNTIME value, so `(def k v)` with `v` resident in the draining generation stored
that stale handle into globals *after* migration moved the live globals off it — re-pinning
a generation a process already acked clean; once that process exits, the union goes
all-clean and the generation is freed with a live global still pointing into it (a
use-after-free), and the same shape let migration's reconcile clobber a concurrent
`(def k old-gen-value)`. Fix: `Heap::rehome_to_current` deep-copies a value in a non-current
RUNTIME generation into the current one (migration's `flush_rt_value`, under `promote_lock`),
wired into the global `env_define` and `set_declared_sig` paths — restoring the invariant
"no shared root points at the draining generation." No-op on the default single-generation
path. Regression:
`runtime_collector.rs::a_def_of_an_old_gen_value_is_rehomed_off_the_freed_generation`.

Still **deferred** — a purge policy for a *genuinely* pinned generation (a process actively
**looping** in old code, not merely parked). Whole-generation reclamation needs every live
process to become quiescent w.r.t. the draining generation; such a process pins it forever
(exactly Erlang's `code:purge` condition), and with a permanent pin the region grows
unbounded (churn can't reclaim past the 2-versions cap). Today's policy is the safe option
(c): don't age a third time — accept the 2× ceiling until the pin clears. The remaining
rungs — a `recur-latest` re-dispatch convention (the Brood analogue of Erlang's
local-vs-external call distinction: let a long-running loop voluntarily jump to the current
generation at its own safepoint) and a hard purge (kill + let a supervisor restart a
straggler, Erlang's `code:purge`) — are separate future decisions, the former worth its own
small ADR (it is new language surface). Off by default pending those + a perf A/B of the
aging migration copy.

<details><summary>Superseded sketch — the cooperative rolling quiesce (kept for context)</summary>

Because the scheduler is *cooperative* (processes yield at the eval safepoint) and each
process's `Heap` lives on its own coroutine stack (unreachable from outside — so a
coordinator cannot rewrite another process's handles; each must rewrite its own), the
earlier design was a **rolling quiesce**, not a hard freeze:
1. A coordinator builds the new compacted region + a forwarding table from the *union*
   of all processes' roots (each process contributes its RUNTIME roots at its safepoint).
2. The **old region is kept alive** (a second live `CodeSlabs`); handles resolve against
   whichever region they belong to until migrated, so nothing dangles mid-migration.
3. Each process, at its next safepoint, applies the forwarding table to its own
   heap/roots/arms (self-rewrite) and acknowledges the new epoch.
4. The old region is freed only once **every** process has migrated.
The killer wrinkle was step 3: every process rewriting handles in its own private heap,
race-free, with a parked process pinning the region indefinitely. The 2-generation model
avoids all of it by never rewriting a handle — it just tags which generation a handle
belongs to and drops a whole dead generation.
</details>

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

## ADR-096 — VM perf as the JIT runway: one road, not two

**Status:** accepted + round 1 implemented (2026-06-06): items 1–5 all landed —
fib −22%, sum_tail −26%, cons_build −42%, sort −13…−24%, spawn_fanout −25%
(~1.2–1.7× on top of the Stage-3 VM), no regressions, every item gated on both
suites + GC-stress. **Round 2 (2026-06-07): item 6 (defer-set shrink) done for
`letrec`** — direct self-recursion now VM-compiled (the `defseq` family +
hand-written local loops, which deferred wholesale before): `MakeClosure`
late-binds the closure to its own name in its captured env, and a **self-call
optimization** (`Node::SelfCall` → `Step::SelfTail`, in-place frame reset)
re-enters the arm with no resolve/dispatch/env-re-root. Load-robust result
(corrected 2026-06-07 with the `perf-stats` harness — see below): for
**RUNTIME-region closures**, i.e. the prelude `defseq` family, a real win —
`(count (map inc (range n)))` is **~58–60% faster** on the VM than the tree-walker
(`self_tail` fires per element; it deferred *wholesale* before round 2).
**Top-level `letrec`/lambda literals defer by design** — their `fn_rest` is
LOCAL-region (can't be baked into a cached `Node` tree without a use-after-GC), so
they run on the tree-walker (parity); the self-call benefits *promoted*/prelude
closures, not top-level one-shots. (An earlier "−30…−54%" headline was a noisy read
of a *top-level* `letrec` micro-bench that actually defers — `perf-stats` later
showed `self_tail`/`vm_apply` were zero there. The harness caught my own bad
measurement; lesson in `docs/benchmarking.md`.) Mutual recursion still defers.
Long-form analysis in `docs/vm-perf-and-jit-runway.md`.

**Round 3 (2026-06-07): `apply`-unfolding in `dispatch` + bench/test harness
hardening.** `dispatch` now wraps its passthrough-redirect inner loop in an outer
`'apply: loop` (mirrors the TW's `'dispatch` loop in `eval/mod.rs`): after each
passthrough exit, if the callee is the `apply` native with argc ≥ 2, the trailing
list is spliced and `continue 'apply` re-runs passthrough on the real callee — no
new Rust frame per iteration, O(1) stack. `apply_builtin` stays on `eval::apply`
for the TW fallback only. Result: **`apply`-driven tail recursion ~69% faster on
the VM** (`vm_apply` 2 → 10,001 per 10k-iteration run; ratio 1.09 → 0.31).
Harness additions: `try_body` bench (ratio ≈ 1.0 — LOCAL thunk breaks VM chain,
correct), `apply_driven` bench (now shows ~0.31 after this round), `reduce_range`
comment corrected. `perf-stats` pass confirmed: `try_body vm_apply = 0` (LOCAL
thunk → TW entirely); `apply_driven vm_apply = 10001` (every iteration on VM).
Differential corpus extended with `try`/`binding`/`isolate` thunk routing and
apply-unfolding cases. GC-stress + heap verifier clean.

**Context.** A JIT (emit machine code at runtime) is the natural next rung above the
closure-compiling VM (ADR-076): both engines compile a form once, but the VM *interprets*
the compiled `Node` tree (a Rust `match` per node — ~50–100 instructions for a hot
`(+ a b)`), where a JIT would run ~8. The question was whether to start one, run it in
parallel with VM tuning, or defer it. Analysis showed the architecture is unusually
JIT-friendly (immutability kills write barriers; the lexically-addressed IR, deopt seam,
and epoch-guard pattern already exist; frame-slots-on-`Heap::roots` lets a tier-1 JIT
sidestep stack maps under the moving GC) — but also that the highest-value VM-interpreter
work and the JIT prerequisites are *mostly the same list*.

**Decision.** No JIT work now — and no parallel track. Instead:

1. **Do the VM-interpreter perf round now**, ordered: call-site inline caches on
   `Node::Call`; a global-read IC on `Node::Global`; a wider inlined-prim family
   (`Prim1`, float fast paths); a compile-time GC-pure bit to skip operand rooting;
   an `exec_value`/`exec_tail` split; (stretch) shrink the defer set.
2. **Adopt JIT-alignment rules while pre-alpha** (cheap now, expensive later): one IC
   mechanism (the epoch-guarded slot, generalizing `Prim2`'s guard); never hard-bind a
   resolution without a guard; prefer indirection tables over in-place AST patching
   (machine code can't be atomically rewritten the way `rewrite_node` patches `ConstVal`s
   under an ADR-091 compaction); explicit safepoint discipline (values live in
   `Heap::roots` slots across any call/alloc); the packed-64-bit `Value` question is
   open and must be decided before 1.0.
3. **Gate any actual codegen** (Cranelift, executable pages) on: bytecode lowering done
   (ADR-076 §2.4's internal change), the editor existing, and a real profile showing
   interpretive dispatch — not allocation/GC/`env_get` — as the bottleneck.

**Benchmark protocol (binding for this round).** An archived `scripts/bench.sh` baseline
before any change; `scripts/quickbench.sh` between items (directional); the full suite +
GC-stress gate per landed item; an item that doesn't move its target benchmark is
investigated or reverted, not shipped. Final archived run closes the round.

**Consequences.** The VM gets measurably faster now; every landed item is also a paved
meter of JIT runway, so a future JIT becomes an increment (a back-end for an IR we
already trust) rather than a project. The cost: we deliberately leave the template-JIT
2–4× on the table until the gates above are met.

**References.** ADR-076 (the VM; its §2.4 names bytecode lowering as an internal change),
ADR-069 (dispatch perf — the passthrough + IC groundwork), ADR-091 (RUNTIME compaction),
ADR-026 (immutability), ADR-038 (bundle-size vs Cranelift). Lives in
`crates/lisp/src/eval/compile.rs`; plan + analysis in `docs/vm-perf-and-jit-runway.md`.

## ADR-097 — Batteries-included default install; split + rename the process framework

**Status:** accepted (2026-06-07). Amends **ADR-085 Move 2** (reverses the
externalization) and supersedes the short-lived stopgap that merged supervision
into `proc/hatch`.

**Context.** ADR-085 Move 2 lifted the net library (`net/tcp`/`net/http`/`net/sse`)
into a `brood-net` package and planned a `brood-supervisor` package for
`proc/supervisor`, on the thesis "keep the bundled stdlib small; push frameworks
out to packages." In practice the modules were *deleted* from the binary but the
packages were **never created**, so `brood-edit` (which `(:use)`s `net/sse` and
`proc/supervisor`) couldn't start — `nest run` died with `cannot find module
'net/sse'`. The project's direction also clarified: Brood should be
**batteries-included** — the editor, net, and concurrency frameworks are part of
what the language ships, not optional fetched deps. The package manager (ADR-037)
remains, but for *third-party* deps only; we do not externalize our own framework.

Separately, `proc/hatch` was the only metaphorically-named module in `std/` (every
other module is a plain noun — `buffer`, `keymap`, `log`, …), and it had come to
fuse two concerns over the same kernel primitives that share **no code and no
common consumer**: a gen_server-style server framework and OTP-style supervision.

**Decision.**
1. **Everything ships bundled in the default install.** `net/tcp`, `net/http`,
   `net/sse`, `proc/gen`, and `proc/supervisor` are all in `CORE_MODULES`
   (`crates/lisp/src/builtins.rs`). No internal framework `:path` packages;
   consumers carry no `:source-paths` to sibling dirs.
2. **Split `proc/hatch` into cohesive units.** `proc/gen` is the gen_server-style
   server loop (`defprocess` / `spawn-server` / `!` / `gen-call` / `stop`);
   `proc/supervisor` is supervision. They're independent — neither `:use`s the
   other; both sit directly on `spawn`/`send`/`receive`/`ref`/`link`/`trap-exit`/
   `exit`/`register`.
3. **Drop the cute name.** Module `proc/hatch` → `proc/gen`; the spawn fn `hatch`
   → `spawn-server`; the internal `hatch--clause` → `gen--clause`. `defprocess` /
   `!` / `gen-call` / `stop` keep their names. The `nest new` scaffold template
   `hatch` is renamed `gen`.

**Consequences.** The lean-binary goal of ADR-085 is explicitly traded for a
batteries-included default — every runtime carries the net + concurrency
frameworks. `std/` is still curated (the *language* core stays small); the
framework modules live under their `editor/`, `net/`, `proc/` namespaces and are
bundled, not externalized. `proc/gen` stays a require-able module (not prelude) —
`defprocess` expands to `receive`/`match`, which strands lambdas during the
prelude freeze (devlog). Consumers updated: `std/log` (`proc/gen` + `spawn-server`),
`examples/life`, `tests/{gen,buffer}_test`, the `project` scaffold; `net/*` and
`proc/supervisor` tests + the `webserver` example were restored to the brood repo.
`brood-edit` drops its `:source-paths` hacks and `(:use proc/supervisor)` /
`(:use net/*)` resolve from the binary.

## ADR-098 — Shrink the core: drop the `lambda`/`let*` aliases; demote `defmacro` to a macro

**Status:** accepted (2026-06-07).

**Context.** A pass over the public language surface to keep the core "as small as
possible" (the standing minimal-core principle). Two avoidable items showed up:
(1) the evaluator carried two spellings each for two forms — `lambda` for `fn`
and `let*` for `let` (Brood's `let` is already sequential, so `let*` was a pure
synonym) — yet **no `.blsp` source anywhere used either alias**; (2) `defmacro`
was a core special form, when it can be a macro over a one-line primitive — the
same move ADR already made for `try`/`catch` over `%try`. (`letrec` was also
reviewed and **kept**: it's irreducible — a macro can't introduce the
mutual-visibility scope without a Y-combinator, and merging it into `let` would
break shadow-rebinding `(let (x (+ x 1)) …)` and turn forward references into
silent `nil`s.)

**Decision.**
1. **Remove the `lambda` and `let*` alias spellings.** `fn` and `let` are the only
   spellings. Dropped from `SPECIAL_SPELLINGS`, `SPECIAL_FORMS`, the checker
   (walk/hygiene/recursion/guards), `syntax/scope.rs`, the macro resolver/expander,
   and the VM compiler; the `kw::LAMBDA`/`kw::LET_STAR` constants are deleted.
   `(lambda …)` / `(let* …)` now read as ordinary unbound symbols.
2. **`defmacro` is a prelude macro, not a special form.** A new kernel primitive
   `(%make-macro f)` tags a closure `f` as a macro (`Value::Fn` → `Value::Macro`);
   `defmacro` bootstraps in `std/prelude.blsp` with raw `def`/`fn` as
   `` (def name (%make-macro (fn …))) `` and every later `(defmacro …)` expands
   through it. The hot-reload "macro redefined" diagnostic moved from the old
   special-form arm into `def` (fires when an existing `Macro` global is rebound to
   a `Macro`); `name_value` now names `Macro` closures too, so macros keep their
   name.

**Consequences.** The evaluator core drops from 9 spellings to **8 true special
forms** (`quote if do def fn let letrec quasiquote`). `defmacro`'s *surface syntax*
is unchanged, so all tooling that pattern-matches `(defmacro …)` source (the
checker, `scope.rs`, the formatter, doc/forward-ref pre-scan) is untouched; it
stays in `SPECIAL_FORMS` for highlighting. The macroexpander already expands the
head before its structural dispatch (`macros.rs`), and the loader is form-by-form,
so the bootstrap is order-safe. One stale grammar-test assertion (which used
`let*` to demonstrate regex-escaping — `match*` still covers it) was removed. Full
suite green on both engines.

## ADR-099 — `proc/gen` is a real gen_server: `info`/`init`/`terminate` + a call timeout

**Status:** accepted (2026-06-07).

**Context.** The process *substrate* is already Erlang-family and the userland
**supervisor** (`std/proc/supervisor.blsp`) is ~95% of OTP, but the gen_server
layer (`std/proc/gen.blsp`) lagged. `defprocess` handled only its own
`[:$cast]`/`[:$call]`/`[:$stop]` envelopes, which left three gaps versus OTP:
(1) **no `handle_info`** — a server could not react to a monitor `[:down …]`, a
link `[:EXIT …]`, a timer tick, or any raw `send`, and because Brood uses
Erlang-style selective receive, those unmatched messages **accumulated in the
mailbox forever** (a slow leak, and the server was blind to the deaths it
monitored); (2) **no `init`/`terminate`** lifecycle hooks; (3) the client
`gen-call` **blocked forever** on a dead or wedged server, where OTP defaults to a
5 s timeout and monitors the callee.

**Decision.** Close all three in **pure Brood** — no kernel/Rust surface (ADR-006).

1. **`defprocess` gains `info`, `init`, `terminate` clauses.** `info` matches a
   non-envelope message (body → next state, like `cast`); `init` runs once at
   startup (body sees the state param, returns the initial state — the place to
   `trap-exit`/`monitor`/arm a timer/transform the seed); `terminate` runs on a
   clean `(stop)` (body for cleanup, `reason` bound). The macro now expands to a
   `letrec` loop fn called once after `init`, so `init` runs once rather than per
   message. Envelope clauses (`cast`/`call`/`query`) are always ordered **before**
   `info` clauses regardless of declaration order (so a broad `info` pattern can't
   swallow a `[:$call …]`), and a trailing **default catch-all drops** any
   otherwise-unmatched message and keeps state — the mailbox can no longer leak
   (OTP's default `handle_info`).
2. **`gen-call` is bounded and monitored.** `(gen-call pid payload)` now delegates
   to `(gen-call-timeout pid payload 5000)` (OTP's 5 s default); both `monitor` the
   server, so a dead server raises `gen-call: server … died: …` at once and a
   crossed deadline raises `gen-call: timed out …` (each catchable via `try`). The
   monitor is always dropped — and a late `[:down]` flushed — before returning.
3. **`spawn-server-link` / `spawn-server-named`** added (Erlang `start_link` and a
   registered name) alongside the existing `spawn-server`. Kept to three helpers
   rather than a link×name matrix (ADR-011); link+name is one line at the call site.

**Consequences.** A `defprocess` server now composes correctly under monitors and
`proc/supervisor`, never leaks unmatched messages, and `gen-call` fails fast
instead of deadlocking — the widest remaining OTP gap, closed entirely in
`std/proc/gen.blsp`. The single-state-param model is unchanged (multi-field state
via a map/tuple). Existing `defprocess` servers are source-compatible (the new
clauses and the catch-all are additive). Tests in `tests/gen_test.blsp` cover the
`info` path, the no-leak drop, `init`-once, `terminate`-on-stop, call timeout and
dead-server fast-fail, and named/linked spawn; full Brood suite (1416 tests)
green. The near-term follow-ups (`send-after`/`send-interval` timers, a
pid-returning synchronous `remote-spawn`, a `terminate`-style worker-cleanup
convention) and the larger deferred items (`gen_statem`, an Elixir-style
`Registry`/`pg`, an `Application` behaviour, rollback-on-failure supervisor
startup) are tracked in [ROADMAP.md](../ROADMAP.md).

## ADR-100 — Full process migration is a stepping-VM change, not a corosensei swap; fresh-only stealing is the migration-free partial

**Status:** accepted (2026-06-07). Fresh-only stealing **landed**; full migration
**deferred** (committed direction). See [concurrency-v2.md](concurrency-v2.md)
§3.2/§7, [memory-model.md](memory-model.md) (the "stepping VM" coupling),
[scheduler.md](scheduler.md). Builds on ADR-018/027 (M:N scheduler + preemption),
ADR-076 (the VM that reified the operand stack), ADR-055 (automatic copying GC).

**Context.** The scheduler pins each process to its spawn-worker for life: a
process never migrates between worker threads after it has started. KI-1b
(`concurrency-v2.md` §3.1a) proved *why* — resuming a `corosensei` coroutine that
suspended mid-computation on a **different** OS thread segfaults (smashed return
addresses). The reflex fix is "replace corosensei with a stackful library that
allows cross-thread resume." That diagnosis is **wrong**: the real blocker is that
a process's **call continuation** (the chain of pending non-tail calls) lives on
the **native Rust stack** — true of the tree-walker *and* of today's VM, whose
`exec_call → vm_apply` path still recurses natively (only the operand stack was
reified and only tail calls are trampolined). Any saved native stack is
thread-affine and fragile to move; swapping the stackful substrate inherits the
same hazard. So spawn-placement (ADR-018 era) balances *process count*, not
ongoing *CPU load*, and cannot self-correct when a process turns long-running
*after* placement.

**Decision.**

1. **Land fresh-only work-stealing now** (migration-free, INV-2-preserving). An
   idle worker steals a process that has **never been resumed** from a backed-up
   peer and runs it itself — safe because the first `resume` then happens on the
   thief with no saved native stack to migrate (§3.1a). Implementation in
   `scheduler.rs`: a `Process.fresh` flag, `try_steal` (rotating-start, `try_lock`,
   pulls the first fresh process from a victim's back and re-pins `worker_id`),
   `worker_loop` = own-queue → steal → park-with-backstop, a relaxed `STEALABLE`
   gate, and a `(steal-count)` builtin. This rebalances the spawn-burst backlog of
   unstarted processes; it does **not** move running ones.
2. **Full migration is the stepping-VM endgame, and is the committed long-term
   direction.** Reify the **call/frame stack** as relocatable heap data (a
   `Vec<Frame>` + a flat dispatch loop, the way the operand stack already is), so a
   paused process is plain `Send` data `(frames, operands, ip)`. Then suspension is
   "stop stepping," migration is "move the data," **corosensei is removed** (along
   with the 16 MiB per-process native stacks and the `unsafe impl Send`), and the
   same change independently delivers **fully precise mid-eval GC** (the original
   `memory-model.md` motivation for the stepping VM) and **anytime work-stealing**.
   The surrounding machinery is already migration-ready: `Send` per-process heaps,
   migration-surviving scheduler thread-locals, the one-owner (INV-2) handshake,
   and INV-1 (no slot reuse) are all engine-independent.
3. **Defer the build** (ADR-011) until a workload shows fresh-only stealing +
   spawn-placement leaves real long-task-occupancy skew. Staged inside the VM
   behind a flag (reify call stack → swap suspension for state capture → generalise
   stealing → optional periodic rebalancer), each step keeping the suite and the §6
   KI-1 plain-release bar green. One carve-out remains, exactly as in BEAM: a
   process blocked inside a long **native builtin** has no Brood-level safepoint to
   capture, so it can't be migrated mid-call (the dirty-scheduler analogue; handled
   by the M4 blocking-IO offload pool).

**Consequences.** Spawn distribution and fresh-backlog stealing are now BEAM-like,
and placement is arguably more proactive than BEAM's spawn-on-current-scheduler
default; live-process migration/rebalancing is the one BEAM feature Brood
structurally lacks, and ADR-100 records both *why* (continuation on the native
stack) and the *one* principled way to get it (the stepping-VM call-stack
reification), so the next person doesn't re-derive "just replace corosensei" and
hit the §3.1a wall. Verified for the landed half: `tests/work_stealing.rs` (20/20
release, 5/5 debug) and the KI-1 guard `tests/concurrency_race.rs` clean 13/13
plain-release incl. `BROOD_GC_STRESS`; full suite green. The full-migration design,
staging, and its added acceptance bar live in `concurrency-v2.md` §7.

## ADR-102 — Named timers for the `ui-run` loop

**Status:** accepted (2026-06-07). Landed in `std/editor/ui.blsp` (the
`:timers` model API + the clock-threaded `ui--loop`); tests in
`tests/ui_test.blsp`. Builds on ADR-046 (the one render-op protocol / many
frontends seam `ui-run` lives on) and the M4 let-it-crash render loop.

**Context.** `ui-run` had a single poll timeout — `:tick-ms` (default 1000 ms) —
and no notion of wall-clock time: a `nil` poll became a bare `:tick`, and that was
the *only* time-driven event an app could get. The editor (brood-edit) needed
several independent time-driven concerns at once — eldoc/diagnostics debounce
(250 ms), the which-key panel delay (500 ms), the undo-pause boundary (600 ms),
and consumer-paced key-repeat (300 ms then 35 ms) — and had to multiplex them all
onto that one scalar by *overwriting* it in a priority cascade
(`ed-post-step`: `(ed--which-key-arm (ed--arm-undo-pause (ed-arm-idle-beat m)))`),
with a held-key short-circuit and a single ambiguous `:tick` that re-derived its
intent from model flags. This made a **steady** animation timer impossible: a
cursor-blink beat (530 ms) would be stomped by the next keystroke's 250/600 ms
arming, and a held key zeroed it out entirely. Every new time-driven feature added
another branch to the cascade.

**Decision.** Declare time-driven work as *data* on the model: `:timers`, a map
`name -> spec`, where a spec is `{:every ms}` (repeating) or `{:after ms}`
(one-shot), optionally `:idle? true` (count from the last real input, not from
arm/last-fire). The loop parks the poll until the soonest timer is due and folds
`[:timer name]` when it fires — so a steady animation timer and several idle
one-shots coexist, each on its own clock and with its own fired-event identity. The
per-timer bookkeeping (when each appeared, when it last fired, and the last-input
time) lives in a `clock` map threaded through `ui--loop`, **not** in the model. The
firing math is factored into pure functions (`ui--deadline`, `ui--timer-live?`,
`ui--poll-ms`, `ui--fire`) so it tests deterministically off a hand-supplied
clock; a `:now-fn` model hook injects a scripted clock in tests (default `(now)`).
Named timers are **opt-in by the `:timers` key**: a model without it keeps the
exact legacy path (`nil` poll → bare `:tick` on the `:tick-ms` beat), so
`std/observer.blsp` and any pre-timers app are untouched.

Two subtleties the implementation pins down: (1) a `nil` poll only fires a timer
when `(now)` confirms its deadline has actually arrived — `display--poll-any`
slices the timeout across frontends and can wake early, so firing must be
clock-gated, not "the poll returned nil." (2) "fire once per arming/idle stretch"
falls out of the fire predicate `(and (>= now deadline) (> deadline last-fire))`:
a non-idle one-shot's deadline is fixed at arm-time so it never re-fires; an idle
one-shot re-arms automatically once new input pushes its deadline past the last
fire. Stale per-timer state is GC'd against the live timer set each iteration, so
a removed-then-re-added timer re-arms clean.

**Consequences.** The clock is wall-clock reality, so it does **not** roll back
when the let-it-crash loop rolls the model back on a `view`/`update` throw (a key
the user pressed stays pressed), and the `:focus` branch threads it through without
advancing `:last-input` (a focus message from another process is not user input).
brood-edit's eldoc/which-key/undo/key-repeat arming all migrate onto named timers,
deleting the `:tick-ms`-overwrite cascade, and a cursor-blink the old architecture
couldn't express becomes a one-line `{:every 530 :idle? true}` timer. The single
`:tick-ms` knob is kept only for the legacy back-compat path. (A companion change
in the same effort: GUI mouse presses now carry a click-chain count, enabling
double-click-word / triple-click-line — independent of timers but part of the same
"make the editor feel like GUI Emacs" push.)

## ADR-101 — JIT compilation: three-layer assembly model, Cranelift backend, calling convention

**Status:** accepted (architecture, 2026-06-07). Implementation gated on bytecode
lowering + editor workload profile (ADR-096 prerequisites). Full design in
[`vm-perf-and-jit-runway.md`](vm-perf-and-jit-runway.md) §6. Staged roadmap in
[`ROADMAP.md`](../ROADMAP.md) (JIT tier-1 entries under the VM section).

**Context.** ADR-096 deferred actual JIT codegen until three gates pass: (a) the
VM is bytecode-based, (b) a real editor workload profile names interpretive
dispatch as the bottleneck, and (c) the `Value` representation is decided. The
June 2026 profiling harness confirms the VM is already dispatch-bound at its
current tier (IC 99.99% hit, prim2 96% inlined), so the structural levers are
maxed and bytecode lowering is the live prerequisite. When that lands and the
profile gate opens, the concrete architecture for assembly integration and the
JIT calling convention is needed. This ADR records those decisions.

**Decision.**

**Backend: Cranelift (`--features jit`).** `cranelift-codegen` is the codegen
backend, compiled only when `--features jit` is requested. It is pure Rust (no
C++ dep), designed for dynamic-language JITs (used by Wasmtime and the rustc
Cranelift backend), and passes ADR-014's bar — ~7 kloc of Cranelift IR generation
replaces hand-assembling x86-64 and AArch64. Default builds and `nest release`
bundles (ADR-038) are unaffected; the large binary is an explicit opt-in.
LLVM would add stronger optimisations but those are tier-2 territory; a
hand-rolled assembler drops AArch64 and costs more to maintain.

**Three-layer assembly model** (full detail in `vm-perf-and-jit-runway.md` §6.1):

- *Layer 1 — Runtime code emission.* Cranelift writes machine bytes into
  `mmap`-allocated executable pages at runtime. The JIT itself has no `.s`
  files and no inline asm; opcodes are Cranelift IR.
- *Layer 2 — `std::arch::asm!`.* Reserved for hot interpreter stubs Cranelift
  doesn't improve: the computed-goto bytecode dispatch table (x86-64 only,
  `#[cfg]`-gated, pure-Rust fallback) and any rope/string SIMD paths. Optional,
  additive, and gated on profiling showing the overhead is still measurable after
  bytecode lowering.
- *Layer 3 — External `.s` files via the `cc` crate.* Platform trampolines
  (`trampoline_x86_64.s` / `trampoline_aarch64.s`) compiled in `build.rs` under
  `--features jit`. The trampoline saves callee-saved registers, pins the context
  pointer into its reserved register, calls the JIT'd function pointer, and
  restores on return. ~30 lines per architecture; the only hand-written machine
  code in the project.

**Calling convention** (full detail in `vm-perf-and-jit-runway.md` §6.2):

- Platform C ABI (`extern "C"`) with one reserved-register extension: r15
  (x86-64) / x28 (AArch64) is pinned to `*mut Heap` for the lifetime of a JIT'd
  call (same pattern as V8 and LuaJIT). The trampoline saves/restores it.
- All GC-visible values live in `Heap::roots` slots between safepoints — never
  in callee-saved registers across a call. Sidesteps stack-map generation
  entirely at tier 1.
- Runtime services (`brood_rt_alloc_pair`, `brood_rt_gc_safepoint`,
  `brood_rt_tick`, `brood_rt_global_epoch`, `brood_rt_call_slow`) are `#[no_mangle]
  extern "C"` functions. Their addresses live in a per-arm indirection table
  (ADR-096 §4.C) that RUNTIME compaction (ADR-091) can rewrite without
  invalidating machine code.
- Epoch guard: the call-site IC (ADR-096 §4.A) compiles to `cmp [EPOCH_SLOT],
  r_epoch; jne slow_path`. A `def` hot-reload bumps the global epoch and
  invalidates all JIT'd IC caches at their next call.
- Preemption: JIT'd loop back-edges call `brood_rt_tick()` (ADR-027).

**Prerequisites not in this ADR:**

1. Bytecode lowering (ADR-096, the JIT on-ramp) — Cranelift IR is generated
   from flat bytecode ops, not from the `Node` tree.
2. `Value` representation decision (ADR-096 §4.E — NaN-box vs 16-byte enum).
   The tier-1 design with values in `Heap::roots` works with either rep, but a
   single-word `Value` is what JIT'd register code wants, and pre-alpha is the
   cheapest window to decide this.

**Consequences.** Assembly enters the build in two minor, architecturally-localised
forms (optional Layer 2 inline stubs; Layer 3 trampoline files) plus the Cranelift
feature gate. No hand-written machine code per opcode. Default builds are
unaffected. The `Value`-repr decision is flagged as a prerequisite blocker.

## ADR-103 — Foreign-language parsing: one `tree-sitter-parse` builtin into the existing node shape, not an opaque tree resource

**Status:** accepted (2026-06-08). Landed in `crates/lisp/src/treesit.rs` (the
`tree-sitter-parse` builtin, feature `treesit`) + `std/editor/treesit.blsp` (the
generic fontify/navigation policy); tests in `tests/tree_sitter_test.blsp` and
`tests/treesit_module_test.blsp`. Drives ROADMAP §C (multi-language editor modes:
ruby/elixir). Builds on ADR-045 (the rope + `parse-source-positioned` positioned
CST) and the `std/tool/sexp` node abstraction.

**Context.** The editor (brood-edit) is policy over Brood: brood-mode gets
structural navigation from `std/tool/sexp` and syntax colouring from
`std/editor/highlight`, both written against the positioned-CST node shape
`parse-source-positioned` yields — `{:kind :start :end}` per node, `:text` on
leaves, `:kids` on containers, half-open **character** offsets. `sexp`'s own
docstring anticipated a second backend ("a different parser backend, e.g.
tree-sitter, can later produce the same shape and reuse these commands
unchanged"). To support Ruby/Elixir/… the editor needs a parser for languages
Brood's reader can't read. The roadmap sketched this as "an opaque tree/node
resource: parse, node-at, parent/children/siblings, type, range, incremental
reparse" — i.e. a new `Value` variant wrapping a live `tree_sitter::Tree`, with a
family of accessor builtins, mirroring `Value::Rope`/`Value::Socket`.

**Decision.** Don't add a resource. Add **one** builtin, `(tree-sitter-parse
source lang)`, that parses with tree-sitter and **eagerly projects the tree into
the same node maps `parse-source-positioned` already produces** — plain immutable
Brood data, no new `Value` variant, no GC/equality/printer/message surgery, no
accessor-builtin family. `:kind` is a keyword of the tree-sitter node type,
`:named` distinguishes grammar nodes from anonymous tokens (keywords/punctuation),
byte spans are projected to char offsets exactly as the Brood CST is. Everything
above it is Brood policy: `std/editor/treesit.blsp` walks that data for fontify
(colour a whole node when a per-language kind→face table assigns it one, else
descend — tree-sitter highlight semantics) and for structural motion (a container
is any node with `:kids`, a form is a `:named` node). The mechanism in Rust is
*parse + project*; which kinds get which face and which keys navigate are tables
and keymaps in the editor's modes. tree-sitter + the Ruby/Elixir grammars sit
behind a `treesit` feature (heavier native C build), but in `default` and the lean
`make install` (a modern editor needs it), unlike the opt-in windowing stack.

**Why not the opaque resource.** A `Value::TreeSitterTree` would touch every site
that matches on `Value` (heap slab + region routing, GC marking, equality,
hashing, printer, message-send rejection, the type lattice) for a feature whose
entire consumer set wants the *positioned node shape* anyway — the same shape we
already compute for Brood. The eager projection reuses `std/tool/sexp`'s decade of
node-shape design for free and keeps the kernel surface at zero new variants. The
cost is no **incremental reparse** and re-projecting the whole (windowed) slice per
fontify; tree-sitter parsing is C-fast and the editor already re-parses Brood per
frame, so this is well within budget. Incremental reparse / lazy node access
remain available as a later optimisation behind the *same* Brood-facing data
shape if a large-file profile ever demands it — the policy above wouldn't change.

**Consequences.** A new language is a grammar crate in `crates/lisp/Cargo.toml` +
one arm in `treesit.rs::language_for` + a face table and a layer in the editor —
no kernel change. Keyword colouring is cross-language (an anonymous alphabetic
token is a `:syntax/keyword`), so per-language tables name only the handful of
*named* nodes they colour.

**Follow-up (2026-06-28) — grammars are out of the default kernel.** The original
`treesit` feature bundled the Ruby/Elixir grammar crates, so a stock build linked
two *language-specific* parsers into the language core — the kernel knew named
languages. That contradicts "kernel = mechanism": the language itself shouldn't
enumerate Ruby/Elixir. Split it. `treesit` is now the **generic mechanism only**
(the tree-sitter runtime + the positioned-CST projection) and stays in `default`;
each grammar is an opt-in `treesit-<lang>` feature (`treesit-ruby`,
`treesit-elixir`, + a `treesit-grammars` bundle). `language_for`'s arms are each
`#[cfg(feature = "treesit-<lang>")]`, so a default build enumerates **no** language
and `tree-sitter-parse` reports any `:lang` as "not built into this runtime
(rebuild with --features treesit-<lang>)". `make test` / `make install` opt into
`treesit-grammars`; the two grammar test files self-skip (a runtime probe) so a
bare `cargo test` stays green. **End state:** dynamic runtime grammar loading
(`.so`/`.wasm` at runtime, no compile-time enum at all) — then the kernel ships
neither grammar nor language list; deferred until a real editor mode needs it
(ADR-011).

## ADR-104 — Persistent child processes: a `Value::Subprocess` over the mailbox seam, not a richer `%os-cmd`

**Status:** accepted (2026-06-13). Implemented: `crate::proc`, the `proc-spawn` /
`proc-send` / `proc-close` builtins, the `Value::Subprocess` handle, `tests/proc_test.blsp`.

**Context.** `%os-cmd` (`system/cmd`) runs a child to completion and returns its
captured `{:stdout :stderr :exit}`. That is exactly wrong for a *co-process you talk
to continuously* — an LSP server, a REPL, a formatter daemon — where you write a
request and read a reply over and over for the life of the child. The editor
(myedit) wants multi-source completion, and an LSP source needs a long-lived child
spoken to in framed JSON-RPC over stdio. Brood had no persistent-child-with-pipes
primitive — the language gap that blocked it. (Surfaced by the myedit prime
directive: a missing editor abstraction is a gap to fix in Brood, not to hack
around in the editor.)

**Decision.**

**Mirror the socket mechanism (ADR-062), not extend `%os-cmd`.** A persistent child
is the same *shape* as a TCP stream: a bidirectional byte channel plus a lifecycle,
where reads must not pin a scheduler worker. So it reuses the same seam sockets use
— the blocking-IO → mailbox handoff (ADR-059, `spawn_io_source`). Each child's
stdout and stderr are read on dedicated non-worker threads that **deliver to the
owning process's mailbox**; the Brood side just `receive`s. `%os-cmd` stays the
one-shot tool it is; this is a separate primitive, not a knob on it (ADR-011).

**Message vocabulary** (handle = a `Value::Subprocess`):
- `[:proc handle data]` — a stdout chunk;
- `[:proc-err handle data]` — a stderr chunk, **kept separate** from stdout
  (merging them would corrupt a framed protocol like JSON-RPC);
- `[:proc-closed handle code]` — emitted once on exit; `code` is the integer exit
  status, or `nil` if the child was terminated by a signal.

This is the exact pattern myedit's `ui-run` loop already uses to fold async pushes
in (the SSE-poll wrapper does the identical non-blocking `receive` for `[:sse …]`),
so the editor wiring needs no new mechanism.

**A dedicated `Value::Subprocess(u64)` handle**, not a reused int/ref. Consistent
with `Value::Socket`: a scalar handle (the GC never traces or moves it) that is a
global-registry id, type-safe (`expect_subprocess`), and round-trips through
messages — needed because the reader threads emit the handle in their `[:proc …]`
messages, and because the handle may cross a `send`/`spawn` (the registry is
runtime-global, so `proc-send` works from any process; output still lands in the
owner's mailbox). Not node-portable: the dist wire codec rejects it, exactly as it
rejects a socket — the id names an OS process on this host.

**Three operations, mirroring `tcp-connect`/`tcp-send`/`tcp-close`:** `proc-spawn`
(spawn with piped stdio, throws if the program can't start), `proc-send` (write a
string to stdin + flush, blocking), `proc-close` (kill if running + drop stdin,
idempotent; the final `[:proc-closed …]` still reaches the owner). Deferred as
power features until a concrete need (ADR-011): a graceful stdin-EOF-without-kill,
an explicit exit-code wait, a controlling-process handoff like sockets have.

**Implementation notes.** The registry holds the child's stdin behind its own
`Arc<Mutex<…>>` so a blocking `proc-send` to a child that never drains its stdin
serializes per-child *without* holding the global registry lock (a `ChildStdin`
can't be `try_clone`d the way a `TcpStream` can). Exactly one waiter reaps the
child: the stdout reader, after stdout EOF, polls `try_wait` with a brief lock + a
short nap (never holding the lock while blocked, so a concurrent `proc-close`
`kill` can always take it), then emits `[:proc-closed …]`. **Text-only, like the
socket mechanism**: inbound bytes are delivered via `from_utf8_lossy`, fine for
text protocols (JSON-RPC is UTF-8) and lossy for binary — Brood has no
arbitrary-bytes value kind, and adding one is a separate language-surface decision.

**Consequences.** A 19th runtime `Tag` (`subprocess`), threaded through `value.rs`,
`types.rs`, `message.rs` (+ dist-wire rejection), the printer, heap hashing/equality
ordering, and the MCP JSON bridge — the standard cost of a new scalar handle, paid
once. The editor's multi-source completion (the original motivation) now builds an
LSP source on top of this with no further kernel work. `%os-cmd` is untouched.

**Update (2026-06-14) — an optional options map: `:cwd` and `:env`.** `proc-spawn`
took only `prog` + `args`, so a child always inherited the editor's working
directory. myedit's project shell (`C-x p e`) must run commands *in the project
root*, not wherever the editor was launched — the same prime-directive signal that
motivated this ADR. Rather than have the editor wrap every command in
`sh -c "cd <root> && …"`, `proc-spawn` grew an optional third argument: an options
map `{:cwd "dir" :env {"K" "V" …}}`. `:cwd` sets `Command::current_dir`; `:env`
adds variables on top of the inherited environment. Both are the natural `Command`
knobs and are generally useful (LSP servers and the web mirror want a cwd too); the
arity becomes `range(2, 3)` and the absent/`nil` cases preserve the old behaviour.
Tests: `tests/proc_test.blsp` (`pwd` under `:cwd`, an env var under `:env`).

## ADR-105 — Keyword-literal (singleton) types: a literal-set refinement on `Ty`

**Status:** accepted (2026-06-14). Implemented: the `lit` refinement on `Ty`
(`crates/lisp/src/types/mod.rs`), `parse_type` accepting a bare keyword
(`types/check/annot.rs`), the runtime `type-matches?` keyword branch
(`std/prelude.blsp`), unit tests in `types/mod.rs` + `types/check.rs`, and
`tests/contract_test.blsp`.

**Context.** Many positions admit a *closed set of keyword values*, not "any
keyword": the editor's `init.blsp` `:fullscreen` is one of `:maximized` /
`:fullboth` / `:fullscreen` (or `nil`); a mode/state argument is `:on`/`:off`; a
direction is `:row`/`:col`. The type lattice could only say `keyword` — so a sig
couldn't enumerate the allowed values, and neither the advisory checker, the LSP,
nor a runtime contract could flag a wrong one. (Surfaced by the myedit prime
directive: the editor wanted to put its config's allowed values in the type system;
the fix belongs in Brood, not the editor.)

**Decision.**

**A literal-set refinement, not a new `Ty` kind.** `Ty` gains
`lit: Option<Arc<BTreeSet<Symbol>>>` alongside `arrow`/`elem`/`map_kv`. `Some(set)`
means the keyword member is constrained to exactly those keyword symbols, while every
*other* tag in `tags` stays open — so `(or :a :b nil)` is `{tags: keyword|nil,
lit: {a,b}}` and admits the two keywords *and* `nil`. Keyword-only for this slice;
bool/int/string literals are the same machinery (more `Lit` kinds) and a deferred
follow-on. `false` is therefore **not** a literal type — use `nil` for an "off" arm.

**Union is exact (the one departure from the other refinements).** `arrow`/`elem`/
`map_kv` *widen* to `None` when two sides differ, because the union of two distinct
arrows isn't one arrow. But the union of two literal *sets* is precisely their
set-union, so `(or :a :b)` keeps both. A side whose keyword member is *open* (the
keyword tag with no set) contributes every keyword, so it widens the result to open.
`intersect` narrows (empty intersection clears the keyword bit); `negate` widens to
"any keyword" (the omitted keywords are in the complement).

**`is_disjoint` gains a precise keyword case.** Its tags-only rule can't see that
`:a` and `:b` (both keyword-tagged) are distinct values. The call-check
(`walk.rs`, its only real caller) needs that to warn on `:c` against `(or :a :b)`.
So `is_disjoint` adds: when the *only* shared tag is `keyword` and both sides pin
disjoint literal sets, they're disjoint. This only ever reports *genuinely* disjoint
types (a literal set is an exact enumeration, not an approximation), so it never
raises a false warning — advisory-soundness holds.

**Bare-keyword surface syntax.** A sig writes `(or :maximized :fullboth nil)`, not
`'`-quoted. A keyword is self-evaluating and unambiguous in type position (base types
are bare *symbols*), and bare survives the runtime path: `sig!` quotes the whole
type-expr, so a bare `:maximized` reaches `type-matches?` as the keyword value
(matched by `=`); a quoted `':maximized` would arrive as `(quote :maximized)` and
silently match anything.

**Consequences.** `of_value` infers a keyword literal as its singleton, so
diagnostics now name the exact value (`got :k`) rather than the coarse `keyword`
tag — two existing checker tests were updated to the sharper output. Literal types
help code the checker *sees*; a data file read with `read-first` (an editor
`init.blsp`) is not type-checked, so in-file feedback there remains a separate
LSP-on-data concern.

## ADR-106 — Telemetry: handlers run in an isolated listener process (never the emitter)

**Status:** accepted (2026-06-14). Implemented: `std/telemetry.blsp`
(`require 'telemetry`), registered in `crates/lisp/src/builtins.rs`, tested by
`tests/telemetry_test.blsp` (19 cases incl. the crash-isolation guarantee + a
concurrent block). The kernel-event sources (ADR-137), the metric aggregators
(counter/sum/gauge/summary/`sample-every` + the bucketed `distribution`/histogram with
`metric-percentile`, `tests/telemetry_metrics_test.blsp`), and node up/down through the
`[:runtime kind]` stream (`watch-nodes`) all shipped 2026-07-24 — all pure Brood over
this seam. `defevent` schemas and the remote tier remain deferred follow-ons (ADR-011)
— see the roadmap entry.

This decision **superseded two earlier cuts** in the same session (greenfield, ADR-000
spirit): first an async single-registry process; then, after asking "why is Erlang
synchronous," an inline-in-the-caller model (Erlang's own shape). The inline model was
then reversed for the **hard requirement below**.

**Context.** A web framework (`../hatch`) and a long-lived daemon need an
instrumentation seam: `emit` a named event (request start/stop, cache hit, GC); let
operators `attach` handlers (logging, metrics) without editing the instrumented code.
Erlang/Elixir's `:telemetry` is the established shape (`execute` + `attach`) and
Phoenix is built on it. The decisive requirement here, stated by the user: **a
telemetry handler must NEVER be able to crash the process that emitted the event —
only a dedicated listener may crash.**

**Why inline can't meet that.** Erlang's `:telemetry` (and our inline cut) runs
handlers *in the caller*. A handler that **throws** can be caught (`try`), but an
**uncatchable** fault — a coroutine **stack overflow** (an uncatchable segfault in
Brood) or `(exit … :kill)` (untrappable) — runs in the caller and takes the caller
down with it. There is no way to wrap that inline. So the only way to guarantee "never
crash the emitter" is to run handlers in a **different process**.

**Decision.** Handlers run in a dedicated **listener process**; `emit` is a
fire-and-forget `send` to it. Pure Brood over `spawn`/`send`/`receive`, **zero new
kernel surface**:

- **Total emitter isolation.** `emit` only computes the payload and `send`s it (a
  `send` never throws), then returns nil. No handler code runs in the caller, so a
  handler that throws, hangs, loops, OOMs, or hard-exits cannot affect the emitting
  process. The single guarantee the requirement demands.
- **The listener absorbs handler faults.** It runs each handler under `try`/`catch`,
  so a *throwing* handler is caught and **detached** — the listener survives normal
  handler bugs. Only an *uncatchable* fault kills the listener.
- **Restartable, handlers survive.** The handler table is a `def`-rebound global
  (`*telemetry-handlers*`, visible across processes — ADR-013), **not** listener
  state. So the listener is a stateless executor: supervise it (an ordinary
  `:permanent` child) and a crash restarts it with every handler intact.
  `start-telemetry` spawns + registers it (`:telemetry`); `stop-telemetry` ends it.
- **`forward(id, event, pid)`** ships events to a process you own — to run heavy
  handler work off even the listener.
- **`telemetry-sync`** is a FIFO round-trip to flush pending emits (tests, shutdown);
  it times out rather than hanging and never runs handler code in the caller.
- **`span`** brackets a body with `:start`/`:stop`/`:exception` events (vector base);
  the body runs in the caller (timed), the events are emitted async.
- **Events are plain Brood values** compared by structural `=`.

**The trade-off, accepted deliberately.** One listener is a serialization point and
copies each event across heaps — the very thing the inline cut avoided. We accept it
because the requirement is **safety, not throughput**: a logging/metrics handler bug
must never take down a request. Handlers are expected to be cheap (log a line, bump a
`table` counter) or to `forward` heavy work. Sharding the listener is a future option
if throughput ever demands it.

**Alternatives rejected.** (1) **Inline-in-caller** (the prior cut / Erlang's model)
— more parallel and zero-copy, but *cannot* guarantee the emitter never crashes
(uncatchable handler faults). Rejected by the requirement. (2) **A process per handler
per emit** — perfect isolation but a spawn per event is far too costly; one listener
is the right grain ("only the listener"). (3) **A built-in self-restart guardian** —
considered, but left to the app's existing supervision (`proc/supervisor`): telemetry
stays a plain supervisable child, no bespoke restart logic, and tests stay simple.

**Consequences.** A handler runs in the listener, so it sees the listener's context,
not the emitter's — all context must travel in the event's metadata (it already does).
attach/detach update a global, so they're configuration-time (not safe to race from
many processes). If the listener isn't started, `emit`/`span` are no-ops (span still
runs and returns its body). Telemetry is the seam `nest observe` / `nest mcp` should
eventually consume, and where kernel events (GC, scheduler) get published once a Rust
emit seam is added.

**Follow-up (2026-07-24) — metric aggregators landed.** The deferred "metric
aggregators" are now shipped in `std/telemetry.blsp`, still zero-kernel: `counter`,
`sum`, `last-value` (gauge), `summary` (running count/mean/stddev/min/max), and
`sample-every` (1-in-N), the Elixir `Telemetry.Metrics` set as `attach` handlers.
Two properties make it clean: state is a shared `table` (ADR-107), and because every
handler runs *serially in the one listener* (the isolation guarantee above), a
read-modify-write on that table is race-free — so `summary` keeps float-safe RUNNING
aggregates and never retains samples (bounded state). Readers (`metric`,
`metrics-snapshot`) poll the table atomically from any process. Still deferred: a
distribution/histogram (percentiles need bucketing or sample retention), `defevent`
schemas, and the remote tier.

## ADR-107 — `table`: an in-memory shared store (Brood's ETS) as a Rust-backed handle of deep clones

**Status:** accepted (2026-06-14). Implemented: `Value::Table(u64)` + `Tag::Table`
(`core/value.rs`), the store `crate::table` (`crates/lisp/src/table.rs`), the
`table`/`table-put`/`table-get`/`table-has?`/`table-delete`/`table-incr`/
`table-count`/`table-snapshot`/`table-drop` builtins + the `table?` prelude predicate,
the `Message::Table` codec (cross-process, runtime-local), and
`tests/table_test.blsp` (17 cases incl. cross-process sharing + concurrent-incr
atomicity). Deferred: owner-death GC / `heir`, ordered/bag tables, select/match,
a distributed (Mnesia-like) tier — all gated on a real consumer (ADR-011).

**Context.** Shared, concurrently-read/written state is the one thing the
process-and-immutable-data model can't express cheaply: a process holding a map
makes every read a `send`/`receive` round-trip + a cross-heap copy, and a
`def`-rebound global (what telemetry's handler table uses, ADR-106) isn't atomic and
pours data into the shared *code* region. Erlang's answer is ETS — an in-memory term
store any process reads/writes directly. The web framework (`../hatch`) and the
coming daemon want the same. The user's framing: build it carefully, **fewer features
but more robust**, and "call it `table`."

**Decision.** A `table` is genuine mutable state, so per the immutability contract
(ADR-026) it is a **Rust-backed opaque resource behind primitives**, never a mutable
`Value` — the blessed path (like the rope). `Value::Table(u64)` is a scalar handle
into a global registry of stores. Unlike `Socket`/`Subprocess` (process-local) it is
**sendable** (`Message::Table`): every copy of the handle indexes the *same* store,
the way a `Pid` names one shared process.

**The store holds deep clones in `Message` form — this is the whole robustness
story.** On `table-put`, key and value are `to_message`'d (the same serialization a
cross-process send uses) into owned, heap-independent trees; on `table-get` the value
is `from_message`'d into a **fresh** copy in the *caller's* heap. Consequences:

- **The moving GC never sees the store.** Nothing in it is a live heap handle, so the
  collector can't trace, move, or dangle into it — the entire use-after-GC class is
  structurally excluded. `Table(u64)` itself is a GC leaf (like `Socket`).
- **No cross-process aliasing.** get *clones out*, so two processes never share a
  mutable object — exactly ETS copy-in/copy-out semantics.
- **Key equality is borrowed, not reinvented.** The store buckets by Brood's
  `hash_value`; a (rare) collision is resolved by reconstructing the stored key into
  the caller's heap and calling `Heap::equal`. So table keys behave *identically* to
  immutable-map keys, with zero parallel equality code to drift.
- **Flat locking.** Registry `Mutex` → clone the `Arc<Store>` out → drop it → then the
  store's own `Mutex`. Never nested, so no deadlock; per-table ops only contend on the
  same table.

**Surface (fewer features, robust).** `table`, `table-put`, `table-get` (+default),
`table-has?`, `table-delete`, `table-count`, `table-snapshot` (→ an immutable map,
the read-all / enumeration primitive; consistent point-in-time, the MVCC win over
ETS's dirty reads), `table-drop`, `table?`. Plus **`table-incr`** — the one atomic
mutator: a read-modify-write done entirely under the store lock, so concurrent
counters never lose an update. **No closure-based `update`** — running arbitrary
Brood code under the store lock would risk deadlock/reentrancy and can't be made
atomic safely; `table-incr` covers the real concurrent case (counters/metrics).

**Alternatives rejected.** (1) **Store live `Value` handles in a shared region** —
rejected: it would fight the moving GC (the cross-heap/dangling hazard), the single
biggest robustness risk; cloning to `Message` form removes it entirely. (2) **A
`def`-rebound global** (the telemetry approach) — fine for a tiny startup-configured
table, wrong here: not atomic, and it churns the code/GC region with data. (3) **A
process holding a map** — the round-trip + copy per read is the cost `table` exists to
remove. (4) **A fancier name** (`roost`/`coop`) — the user chose plain `table`.

**Consequences / limits.** A table lives until `table-drop` or runtime exit — no
owner-death reclamation in v1 (an app-lifetime store created at startup is the model;
`heir`/owner semantics are deferred), so a forgotten table leaks until exit (safe, not
UB). Not node-portable: the dist wire codec rejects a `Table` (send its
`table-snapshot` — a plain map — across nodes instead). Values that can't be messaged
(another table is fine; a socket/transient is not) can't be stored — a clean error,
by construction. **Keys must be reliably looked-up-able** — `check_key` rejects values
that can't match themselves after a clone: identity values (`Fn`/`Macro`/`Native`,
which round-trip to a new identity) and `NaN` (which never equals itself); plain data
and id-stable handles (`Pid`/`Ref`/`Socket`/`Subprocess`/`Table`) are fine. (A bad
value *nested* in a compound key has the same hazard as in a map key — documented, not
walked.) `table-incr` works in the i64 range (a bignum value is a precise error, not a
silent miss). Three independent adversarial reviews (concurrency/GC-safety,
correctness/round-trip, and Value/Tag-integration completeness) found no crash,
corruption, deadlock, poisoning, GC-safety, or leak defect; the suite
(`tests/table_test.blsp`, 35 cases) passes under `BROOD_GC_STRESS` + the heap
verifier. Adding the `Value`/`Tag` extended the type lattice by one (the
compatibility-contract sites in `value.rs`/`heap.rs`/`types.rs`/`printer.rs`/
`message.rs`/`wire.rs`); `table?`/`(type-of x) :table` complete it.

## ADR-108 — `lambda`/`let*` are exact synonyms for `fn`/`let` (canonicalised at macroexpand)

**Status:** accepted (2026-06-14). Implemented: `kw::LAMBDA`/`kw::LET_STAR`
(`core/keywords.rs`); both in the evaluator's `SPECIAL_SPELLINGS` (`eval/mod.rs`,
`lambda`→`SpecialForm::Fn`, `let*`→`SpecialForm::Let`); head canonicalisation in
`macroexpand_all_depth` (`eval/macros.rs`) after the quote guard; both added to the
LSP-facing `SPECIAL_FORMS` (`builtins.rs`) and the checker's `SPECIAL_HEAD` /
`is_syntactic_keyword`; `tests/lambda_let_star_test.blsp`.

**Context.** `eval/mod.rs::foreign_construct_hint` listed `lambda` and `let*` among the
names that "Just Work" — so it deliberately *withheld* the "use `fn`/`let`" hint it gives
foreign constructs (`defun`, `setq`, …). But neither was ever implemented: no special-form
dispatch, no macro rewrite, so `((lambda (x) x) 5)` raised `unbound symbol: lambda` at
runtime. The worst outcome — a Scheme/CL user's muscle-memory spelling failed with a bare
unbound error and *no* guidance, because the hint that would have redirected them was
suppressed by a comment that believed the forms existed.

**Decision.** Honour the documented intent: make them **exact synonyms**, not foreign
constructs. `lambda` ≡ `fn`; `let*` ≡ `let` (Brood's `let` is already sequential, so `let*`
adds nothing but the familiar spelling). This keeps with "meet the user where their habits
are" without growing the language's semantics — a synonym adds zero new evaluator behaviour.
(The alternative — declaring them foreign and adding a `foreign_construct_hint` entry — was
rejected: the codebase, including `nest grammar`/scope tooling, already treats `fn`/`lambda`
as one thing, and a redirect hint is a worse experience than the form simply working.)

**Mechanism.** Two layers. (1) The evaluator's special-form table dispatches `lambda`/`let*`
directly, so a *raw, un-expanded* form reaching `eval` — a quasiquote-built closure, an
`(eval '(lambda …))` — is handled. (2) `macroexpand_all` rewrites the head `lambda`→`fn` /
`let*`→`let` immediately **after the quote/quasiquote guard** (so quoted *data* keeps its
spelling — `(first '(lambda (x) x))` is still `lambda`) and **before** lowering. So the
whole downstream pipeline — pattern lowering, the VM compile pass, and the tree-walker's
own lowering re-entry — only ever sees `fn`/`let`, and no scattered `kw::FN`/`kw::LET` site
needs to learn the synonym. Full parity follows for free: destructuring params, variadic,
multi-arity dispatch, recursion, and closures that round-trip across processes.

**Consequence.** A stored/printed expanded form shows `fn`/`let`, not the user's `lambda`/
`let*` (expansion is lossy by design, as with every macro). The advisory checker, which
macroexpands in whole-file mode, sees the canonical form; for the un-expanded fragment path
(`(check 'form)`) the checker's `is_fn_head` / `SPECIAL_HEAD` entries recognise `lambda`/
`let*` directly. Surfacing this also fixed three pre-existing checker false-positives (the
two synonyms looking unbound, multi-arity-clause params, and self-recursive `let`-bound
closures) — see the 2026-06-14 devlog entry.

## ADR-109 — `string-split` is a native builtin (not pure Brood)

**Status:** accepted (2026-06-14). Implemented: `string_split` + its registration and
arity/doc entry in `crates/lisp/src/builtins.rs` (beside `%str-index-of`); the former
`string-split`/`string-split--acc` defns removed from `std/prelude.blsp`;
`tests/strings_test.blsp` already covers the semantics.

**Context.** Per ADR-006/008 the string library (`split`/`join`/`replace`/`trim`/…) is
written in Brood over the `substring`/`%str-index-of`/`str` primitives, and `string-split`
was a pure-Brood tail-recursion: find the separator, emit the head, recurse on the *tail*
substring. But Brood strings are char-indexed and `substring` is O(index) (UTF-8 has no
O(1) char access — the same reason `%str-index-of` is native), so re-slicing the shrinking
tail each step makes the whole split **O(n²)**. This surfaced in the editor (brood-edit):
parsing a 174 KB `git ls-files` output took **~840 ms**, dominating project-file listing on
a large repo. `string-split` is also the substrate ~10 std modules build on (`file`
read-lines, `path`, `text` words, `diff`, `datetime`, `url`, `net/http`, `net/sse`), so the
quadratic cost was latent everywhere, not just the editor.

**Decision.** Make `string-split` a native builtin — one O(n) pass via Rust's `str::split`
(empty separator → chars, matching the old semantics and `string->list`). A correct O(n)
version is unexpressible in pure Brood given char-indexed `substring`, so this is genuinely
kernel-worthy (ADR-008's bar), and it *shrinks* the surface: one focused builtin replaces a
recursive Brood pair and joins the existing native string-scanner family
(`%str-index-of`/`string-span`/`string-span-until`). The 174 KB parse dropped ~840 ms →
~10 ms; every `string-split` caller benefits.

**Alternatives rejected.** (a) A native O(n) char iterator + a pure-Brood fold — still one
builtin, but allocates a heap string per *character* (~30× more), slower and GC-heavy.
(b) Byte-offset `index-of` + byte-slice `substring` — exposes raw UTF-8 byte offsets
(boundary footguns) and adds two builtins. (c) Route through the native regex engine —
heavier (compile per call), wrong semantics (literal vs pattern), removes nothing.

## ADR-110 — Gradual typing earns its place: `GradualTy`'s first consumers (assignment / return / value-position checks)

**Status:** accepted (2026-06-15). Implemented: `annot::parse_value_sig_decl` (non-arrow
`(sig x T)`), `Ctx.declared_value_ty`, `walk::gradual_of` (expression → `GradualTy`), the
gradual-assignment check in `check_def`, the return-type check in `check_fn_seeded`, and
`expr_ty`'s declared-global fallback (`crates/lisp/src/types/check/{walk,guards,ctx,annot}.rs`).
Tests in `check.rs`; supersedes the "foundation-only, unconsumed" status note in `types.md`
(refines ADR-024).

**Context.** `GradualTy`/`consistent_with` had been built and unit-tested but had **zero
production callers** — referenced only by their own tests, with a standing note to "wire it
in only when a real gradual-assignment consumer arrives." The question was delete it
(greenfield: drop dead weight) or give it the consumer. The compatibility answer settled it:
`GradualTy` is the *set-theoretic* way to do gradual typing (consistent subtyping derived
from set inclusion, not a Siek–Taha bolt-on — ADR-024), so it's the right direction; what it
lacked was a job.

**The key insight (why this isn't just disjointness rebranded).** The existing advisory
checker is a **disjointness** pass over `Option<Ty>` — it warns only when an argument's type
is *provably disjoint* from what's wanted. For that rule `GradualTy` adds **nothing**: an
"unknown" is already silent, which is `dynamic()`'s behaviour for free. `GradualTy` earns its
place **only in a check with assignment / subtyping semantics** — one that *errors when
something is not a subtype*, where consistency gives the gradual benefit-of-the-doubt. So the
consumers are assignment sites, not the disjointness walk:

- **`(def x <expr>)` vs `(sig x T)`** — the value must be *consistent* with `T`.
- **Return type** — a `(sig f (P… -> R))` body's last form must be consistent with `R`.
- **Declared globals in value position** — `(sig g int)` flows `g`'s type into the
  disjointness check, so `(string-length g)` is caught.

**The capability `Option<Ty>` structurally can't provide:** a reference to a redefinable
global with a declared type is `dynamic_within(t)` — a **bounded dynamic**. `Option<Ty>` has
only known/unknown; it can't say "unknown but definitely numeric." So `(def count label)`
with `label : string`, `count : int` is flagged (`string ∩ int = ⊥`), where the disjointness
pass — treating every global as an untracked `None` — sees nothing. This is the genuine
value-add, and it's exactly the hot-reload-safety motivation of ADR-024 (a global is
`dynamic(t)`, never assumed static): it warns on a provable mismatch with the declared
*contract*, and defers when the bound merely overlaps.

**The false-positive discipline (the load-bearing design rule).** The checker's inferred
types are sound *over-approximations* (`(+ int int)` is typed `number`). So:
- An **over-approximated** value (a call result, a `let` local) is `dynamic_within(t)` →
  consistency uses `∩ ≠ ⊥`, which can only fire on a *provable disjointness* and **never
  over-warns on a widened guess** (`(def n (+ 1 2))` vs `int` *defers*).
- A **precise** value — a literal, or a `(sig …)`-typed **parameter** (its exact contract
  type) — is `stat(t)` → consistency uses `⊆`, which can additionally catch a value *merely
  wider* than the target (`(defn f (x) x)` returning a `number` param where `int` is
  declared). This is the first diagnostic disjointness structurally cannot produce.

The result held the zero-false-positive bar through every slice: project-wide `nest check`
over `std/` + `tests/` stayed at 3 warnings (all the intentional non-tail recursion lint).

**Deferred (ADR-011).** Catching a wider *call-result* body (body typed `number`, declared
`int`) needs **precise** result types — overloaded/dependent arithmetic sigs, or full
occurrence-typing body inference (the historical false-positive source). Both wait for a
concrete consumer that justifies trading the perfect no-FP record. The bounded option
(overloaded `(+ int int) : int` sigs) is the recommended next step if one arrives.

## ADR-111 — Lazy seq-views: fusing pipelines as an opt-in combinator, `map`/`filter` stay eager

**Status:** accepted (2026-06-15). Implemented: `Value::SeqView(VecId)` + the `%seqview` /
`%seqview-parts` / `seqview?` builtins (`core/value.rs`, `core/heap.rs`, `builtins.rs`);
prelude `lmap`/`lfilter`/`lkeep`/`lremove`/`eduction` + `fold`/`seq`/`count`/`empty?`/`join`
view-handling (`std/prelude.blsp`); realise-at-boundary in `first`/`rest`/`prim_eq`/`apply`
+ the stringifiers, with safe fallbacks in `equal`/`hash`/printer/`to_message`. Tests:
`tests/sequence_test.blsp` "lazy seq-views". Implements compute-frontier lever 3c.

**Context.** Idiomatic `(reduce + 0 (map f (filter p (range n))))` materialises a cons list
per stage — the `pipeline`/`strings` benchmarks' cost and a GC/memory outlier (~180 MB).
The lever is to **fuse** the pipeline (fold straight through, no intermediate lists), modelled
on the existing reducible `Value::Range`.

**Decision.** Add a kernel `Value::SeqView(VecId)` mirroring `Value::Range` — a distinct tag
over the vector slab, backing `[source xform]` (a transducer), `tag = Pair` so it *is* the
list it stands for. `fold` fuses over it (`(fold (xform rf) init source)`); `seq` realises it.
**Fusion is opt-in:** `map`/`filter`/`keep`/`remove` stay **eager**, and the fusing views are
the new `lmap`/`lfilter`/`lkeep`/`lremove` + `eduction`.

**Why not lazy `map`/`filter` by default** (the originally-scoped design). Tried it; it broke
`nest test`. Brood code pervasively iterates `map` **for side effects** — the module loader
(`(map require-one …)`), the test runner (`(map run-test …)`), `require` over a list — and a
lazy view silently drops those effects unless realised. The design's "footgun is benign
because Brood is immutable" was wrong: immutability constrains *data*, not *I/O*. Making the
default lazy would force auditing ~180 `map`/`filter` sites and adopting a permanent "never
map for effect" discipline — a poor trade against ADR-011 (ship the simple form; defer the
powerful, sharp-edged one). Opt-in fusion keeps the idiom intact and the win available.

**Mechanics.** Realising a view runs its transducer (a Brood closure), which the pure-heap
paths can't do, so: `first`/`rest`/`prim_eq`/`apply` realise via a kernel→prelude bridge
(`%seqview-realize`, GC-rooted like `%range-reduce`); the stringifiers realise their args;
`equal`/`hash`/printer/`to_message` get safe non-panic fallbacks (identity / sentinel /
`#<seq-view>` / a "realise it first" error) because the prelude realises before those in
normal use. GC promote/flush/verify treat a view like a vector (its backing holds heap
values), unlike `Range` (ints only).

**Results & limits.** `pipeline` (n = 1e6): ~2.0 s / 173 MB → ~0.63 s / 13 MB (~3.3× faster,
~13× less memory). `strings` is **not** yet fused — `join` realises the view because the
native `%string-join` walks via `seq_items`, which can't run a transducer; full fusion needs
a string-builder reducer (a transient buffer the transducer appends into), deferred as a
follow-up.

## ADR-112 — Brood data is immutable, absolutely: remove user-facing transients; `Table` is the only mutable structure

**Status:** accepted (2026-06-15). Implemented: removed `Value::Transient`,
`Tag::Transient`, the `transient`/`assoc!`/`dissoc!`/`persistent!`/`transient?`/
`transient-get`/`transient-count`/`transient-contains?` builtins, the `transients`
slab + its GC write barrier (`remembered_transients`) and epoch re-anchor
(`transient_reanchor`), and the prelude callers (reverted to `into`/`%map-into` or
immutable `assoc` folds). Kept the kernel-internal watermarked CHAMP build
(`%map-into`/`map_from_pairs`). See devlog 2026-06-15 and `docs/transients.md`.

**Context.** A user-facing transient (`Value::Transient` + `assoc!`) had been shipped
(the "Phase 2 overruled" note in `docs/transients.md`) for fast map building. But a
Lisp-callable, identity-mutable data structure directly contradicts ADR-026's first,
absolute bullet — *"Lisp data is immutable. No primitive mutates a `Value`; none may be
added."* `docs/transients.md`'s own Phase-1 section had argued exactly this and rejected
the user-facing surface; it was overruled, then the cost showed up: the GC grew a
transient-specific write barrier + epoch re-anchor purely to keep a mutable cell safe
across collections.

**Decision.** Immutability of Brood data is **absolute and non-negotiable**. The **only**
mutable structure in the language is `Value::Table` (ETS — a shared store behind an opaque
handle that deep-clones keys/values in and out, so no data value is ever aliased-and-
mutated). No other mutable structure — transient, builder cell, mutable buffer, anything —
may be added, regardless of build-performance appeal. Fast bulk construction is done as a
**GC-quiet in-place build inside a single Rust builtin** that returns a fresh immutable
`Value` (the kept `%map-into` watermark trick) — an implementation detail of *constructing*
the value, never observable as mutation.

**Consequence (the upside).** Removing the mutable transient let the GC delete the write
barrier and epoch re-anchor that existed only for it. The minor flip's founding invariant —
*old never points to young, no write barriers for data* — holds for every value again; the
sole remembered-set is the `def`/env-frame *binding* barrier (ADR-013, binding mutation, not
data). ~840 net lines removed. CLAUDE.md now states the rule absolutely so it isn't
re-litigated. Reaffirms and hardens ADR-026; reverts the transient half of the work in
`docs/transients.md`.

## ADR-113 — mimalloc as the allocator backend (spend memory for speed; Brood targets long-running apps)

**Status:** accepted (2026-06-15). `core/alloc.rs`'s `Counting` global allocator delegates to
`mimalloc::MiMalloc` (`BACKEND`) instead of `System`; byte-counting/limits (ADR-043) unchanged.
`mimalloc` added to the `brood` lib deps.

**Context.** Brood is built for **long-running applications — editors and web servers — not
short scripts**, so steady-state throughput matters more than peak RSS, and spending memory to
go faster is the right trade (boot time is the one guardrail — must stay fast). Its immutable
data path-copies on every update (a CHAMP `assoc` clones each node on the root→leaf path; a
fresh `Value` per builtin), so allocation throughput is load-bearing. Profiling showed ~10% of
the `wordcount`/`assoc` hot path in `malloc`/`free`/`Drop`.

**Decision.** Route the counting allocator's backend through **mimalloc** (per-thread heaps +
size-segregated free lists → ~bump-speed alloc/free). Drop-in `GlobalAlloc`, MIT, no correctness
surface (byte counter tallies requested `Layout` sizes around it; `BROOD_MEM_LIMIT` still works;
startup unchanged at ~38 ms). Relaxes ADR-005's dependency-free rule for genuine runtime
infrastructure — the bar `boxcar` already cleared.

**Trade.** Faster (~15% `wordcount`, ~28% `bintree`, single-thread geomean ~12× → ~9.9× off the
fastest), at higher RSS (mimalloc holds freed pages for reuse: `wordcount` 54→90 MB). Accepted:
for a long-running runtime that's the correct direction, and it can't be tuned to both
(`MIMALLOC_PURGE_DELAY=0` restores low RSS but tanks throughput below system malloc). Not
vendored (it's ~33k LOC of C); taken as a normal cargo dep.

**Note.** This subsumes the once-considered in-tree CHAMP node-array arena (its purpose was the
same malloc churn). Remaining map-perf levers (single-pass `map-update`, shrinking the path-copy
memcpy) are orthogonal and tracked separately (`champ-map-perf`).

## ADR-114 — Keep the moving collector; the JIT already sidesteps stack maps, so harden the spill-to-roots discipline instead of switching to mark-sweep

**Status:** accepted (2026-06-28). Records why we evaluated replacing the per-process
moving/copying collector with a **non-moving mark-sweep** heap (rejected on the benchmarks below)
and — after reading the JIT↔GC code — *corrects an earlier draft of this ADR* that recommended
adding JIT **GC stack maps**: investigation showed the JIT **already sidesteps stack maps by
design**, so the real work is hardening the existing spill-to-roots discipline, not building stack
maps. One debug-assertion landed (below); no collector change.

**Correction (the stack-map premise was wrong).** An earlier draft recommended giving the JIT
Cranelift stack maps so the collector could relocate JIT-staged roots in registers. Reading the
code (`jit/mod.rs` ABI doc §6.2; `jit_lower.rs` call staging; `compile/mod.rs::dispatch`) showed
that premise is false: Brood keeps `Value` as a 16-byte enum (NaN-boxing was measured and declined,
`docs/value-repr.md`), so **a `Value` never rides in a register**. JIT'd code keeps *all* live
`Value`s in `Heap::roots` (the operand stack the collector already scans) and holds only *unboxed*
`i64`/`f64` in registers within a safepoint-free segment; before any call it **spills** every live
`Op::Handle` deeper on the stack into a GC-visible frame slot (`jit_lower.rs` ~L1981). The ABI doc
calls the no-stack-map problem *"the single hardest part of JIT-ing under a moving collector,
sidestepped."* So stack maps would be **pure redundancy** — building them would add machinery for
zero benefit, against the keep-it-minimal principle. The earlier draft is corrected here rather
than left to mislead.

**What the bug class actually is.** The bug #2 family (commits `dbf134a`, `e000652`, the 2026-06-24
`dispatch` fix) is **not** a register-liveness problem. It is the **Rust dispatch/IC/deopt glue**
caching a LOCAL `Value`/`EnvId` in a plain Rust local (`cur_argv: SmallVec<[Value;4]>`, a `fast` IC
link) *before* a JIT sub-call's safepoint and reading it **stale** afterward instead of re-reading
from `roots`/`env_roots` (which the collector relocates in place). The invariant: *any glue holding
a LOCAL handle across a call that can GC must re-read it from `roots` after — never trust the
pre-safepoint Rust-local copy.*

**Audit (2026-06-28).** Only `compile/mod.rs::dispatch` holds a Rust-local `cur_argv` across
`jit_tier`; its post-call arms already re-read from `roots[base..]` (the prior fixes). The VM
trampoline caller (`vm_run_bc`, ~L4311) is roots-only — structurally immune. The one residual
unverified spot was the `rest_slot.is_some()` → `cur_argv` fallback (documented dead-code for the
JIT int-subset); it is now a `debug_assert!(heap.dbg_value_stale(v).is_none())` so a future
regression of the invariant trips loudly in debug instead of corrupting. Detection at large already
exists: the per-deref epoch tripwire, `BROOD_GC_VERIFY`, and `BROOD_JIT_VERIFY`.

**Context.** Brood's data is immutable (ADR-026/112) and processes are isolated (per-process LOCAL
heap, messages deep-copied). The reasonable intuition — *"with immutability + isolation, GC should
be easy; are we over-complicating it?"* — is half right, and worth pinning down precisely:

- **What those properties already buy, and we cash in.** Because data never mutates, *old can never
  point to young*, so the generational minor collection needs **no data write barrier / remembered
  set** (the sole remembered set is the narrow `def`/env-frame *binding* rebind for hot reload,
  ADR-013). Per-process heaps collect independently (no stop-the-world); a dead process frees its
  whole heap wholesale. That is the payoff, and it is real.
- **What they do *not* buy.** The complexity that remains is not from handling mutation (there is
  none). It is from one **performance** choice: a **moving/copying** nursery (for bump allocation +
  compaction). A moving collector relocates LOCAL objects, so any handle held across a collection
  without being re-read goes stale — the *stale-handle* bug class. (Staged JIT call args are **not**
  the problem: they live in `Heap::roots`, which the collector scans and relocates; see the
  correction above. The class is Rust-glue locals held across a safepoint.) The epoch stamps,
  poison bits, per-deref tripwire, and the `BROOD_GC_VERIFY` heap verifier all exist **only** to
  catch that class. The recent run of JIT+GC crashes (bug #2 family; see `jit-gc-frame-corruption`)
  is squarely this.

Immutable data is *nearly* acyclic, so a **non-moving** collector (mark-sweep, or refcounting)
would be sound and would **erase the stale-handle class by construction** — handles never move, so
none can go stale. (Not *purely* acyclic: closures capture environments and `def` rebinding can
introduce cycles/old→young edges — exactly the corner the one remembered set covers — so RC would
need a cycle collector; plain mark-sweep would not.) The question is what throughput that costs.

**Measurements** (this machine, clean `--release --features jit`, min of 6). Method: A/B each workload **GC-on** vs **GC suppressed**
(`BROOD_GC_FLOOR=500M` → 0 collections), so the delta is the *current copying collector's* cost.
Survivor rate = `copied / (copied + reclaimed)` from `(gc-stats)`.

| workload | survivor | collections | GC-on | GC-off | copying GC's effect |
|----------|----------|-------------|-------|--------|---------------------|
| `fib 32` (compute-bound) | — | 0 | 0.08s | 0.08s | none (no allocation pressure) |
| `listsum` (build+fold 20k lists ×1500) | 14% | 474 | **0.93s** | 1.57s | **net −40%: copying is *faster*** |
| `bintree` (depth-16 ×300) | 60% | 451 | 8.64s | **5.42s** | **+37%: copying is the cost** |

Two findings decide it:

1. **Copying is sometimes a net *win*, via compaction.** On `listsum`, GC-on beats GC-off by 40% —
   compaction keeps the working set cache-hot, while the un-collected heap grows and thrashes. A
   non-moving collector does not compact, so it forfeits this win (it stays bounded by reusing
   freed slots, so it won't thrash like GC-off — but it won't get copying's perfect locality
   either), *and* pays free-list allocation instead of bump-pointer (~1.5–3× per alloc, on **every**
   workload — a flat mutator tax a copying nursery avoids).
2. **Copying's cost is real only on high-survivor, allocation-pathological workloads** (`bintree`,
   60% survivor, the GC-stress benchmark by design). There mark-sweep would likely *win* (it marks
   the same live set but never copies it). But that is the minority case.

Net: **throughput does not favor mark-sweep** — it's roughly a wash (a win on `bintree`, a loss on
locality-bound/alloc-heavy code, nothing on compute-bound), plus a universal allocation tax. So the
choice is **not** a throughput decision; it is *bug-class elimination + kernel simplicity* vs.
*rewrite cost + losing bump-allocation/compaction*.

**Decision.** **Keep the moving collector. Do not add JIT stack maps** — they would be redundant
(the JIT keeps no `Value` in a register across a safepoint; see the correction above). Instead,
**harden the spill-to-roots discipline** that already prevents the bug class: keep all live handles
in `roots` in JIT'd code (done — `jit_lower.rs` spills before every call), and in the Rust glue
*always re-read* LOCAL handles from `roots`/`env_roots` after any call that can GC. The one residual
unverified site (the `dispatch` rest-arm `cur_argv` fallback) now carries a `debug_assert!` that the
fallback handles are current-epoch, so a regression of the invariant fails loudly in debug. The
epoch/tripwire/verifier stay as the always-available safety net.

**Rejected alternative — switch to non-moving mark-sweep.** Tempting for kernel simplicity (no
forwarding, no epochs, no "old never points to young" reasoning, tripwire/verifier deletable) and
genuinely aligned with the "immutability should make this simple" instinct. Rejected because the
benchmarks show it trades away a measured locality/allocation win (and pays a flat free-list alloc
tax) to fix a bug class the existing spill-to-roots discipline already prevents — a worse trade
while the JIT is the performance story. It stays a live fallback: if the moving collector's
glue-discipline cost ever outweighs its throughput win, a non-moving LOCAL heap is the
simplicity-first escape, and the immutability invariant guarantees it would be correct.

**Trade.** The cost of keeping the moving collector is the spill-to-roots discipline: JIT'd code
must spill handles before calls (structural, done) and Rust glue must re-read from `roots` after
safepoints (a reviewable invariant, now debug-checked at the last unguarded site). That is cheaper
and lower-risk than either a stack-map implementation (redundant) or a collector rewrite
(throughput regression). Supersedes nothing; refines ADR-035/055 (the copying collector stays) and
ADR-101 (documents that the JIT's GC-root contract is spill-to-roots, *not* stack maps).

## ADR-115 — Record/shape types: `(record :k T …)`, full `fields` refinement

**Status:** accepted; **shipped 2026-07-03** ([`types.md`](types.md) Step 5+,
[`type-records.md`](type-records.md)). Heterogeneous, keyword-keyed map shapes with
required-by-default and `(optional T)` fields are a new `(sig …)` type-expression;
`type-matches?` enforces them at the `sig!`/`BROOD_CONTRACTS=1` runtime-contract
boundary; `Ty` carries a full `fields` refinement with width/depth subtyping;
`(get r :k)` resolves to the exact field type on both a declared *and* an inferred
record; a `{…}` map literal infers its own record shape with zero annotation.
Advisory throughout — contract #5 holds. Refines the Step 5+ staircase alongside
ADR-078's arrow/element/map_kv refinements.

**Context.** `(map K V)` ([`type-map-kv.md`](type-map-kv.md)) gives a map *uniform*
key/value types, but most config/options/record-shaped maps aren't uniform —
different keys carry different types, and some are optional. Before this, the best
`(sig …)` could say about such a value was bare `map`: zero field-level checking, so
`(sig! make-window ((record …)))`-shaped intent had no representation at all.

**Decision.** Add `(record :k1 T1 :k2 T2 …)` to the type grammar, list-headed like
every other compound type form (`(map K V)`, `(vector E)`, `(and A B)`, `(or A B)`)
rather than reusing the `{…}` map-literal reader syntax — it slots into
`parse_type`'s existing head-symbol dispatch instead of adding a new
`Value::Map`-shaped branch to the parser. Fields are **required by default**; an
`(optional T)` wrapper marks a field as allowed to be absent/`nil`. Records are
**open**: extra keys beyond the declared fields are allowed and ignored (the
permissive direction of structural width subtyping) — the ADR-011-simple choice,
since a closed-record variant is a pure addition later if a concrete need for it
shows up.

**Shipped in one pass, not staged — the maintainer chose to build past the initial
grammar-plus-runtime slice in the same session.** `Ty` gained a `fields:
Option<Arc<BTreeMap<Symbol, (Ty, bool)>>>` refinement (name → declared type,
required?), tagged `MAP_BIT` like `map_kv` — no new `Tag` variant, the same trick
`keyword_lit` uses layering onto the `Keyword` tag. Three genuinely new pieces, none
copy-paste from the `map_kv` precedent:

1. **Width/depth record subtyping** (`is_subtype`) — for every field `other`
   declares, `self` must also declare it (required if `other` requires it) with a
   covariant field type; an undeclared field imposes no constraint (width). Written
   **conservative on purpose**: if `self` doesn't declare a field `other` marks even
   merely *optional*, subtyping returns `false` rather than reasoning about absence
   — sound (never claims a false subtype relation), just incomplete. See
   `record_fields_is_subtype` in `types/mod.rs`.
2. **`get`-by-literal-key sink** (`check/guards.rs`) — `(get r :name)` with a
   *literal* keyword key resolves to the exact declared/inferred field type; every
   prior `map_kv` sink rule only ever inspected the key's *type*, never its value,
   so reading the literal off the call form is new code shape.
3. **Record-literal type inference** (`expr_ty`) — `{…}` map literals previously had
   **no** typing arm at all (vector literals already got `vector_of(element_union
   (…))`; maps got nothing). Every resolvable `:key value` pair in a literal becomes
   a required field (it's data, evaluated once — the key is definitely present); an
   unresolvable value or non-keyword key is silently omitted from the shape rather
   than guessed. This is what makes `(get {:a 1} :a)` precise with **zero**
   annotation.

**Union/intersect deliberately reuse the existing generic merge helpers**
(`merge_union`/`merge_intersect`) rather than a fancier field-wise algorithm (union
each shared field, demote a required-on-one-side field to optional). The blunt
"widen to `None` unless the two refinements are identical" rule is already the
established sound pattern for *every* refinement in this lattice (`arrow`, `elem`,
`map_kv`) — records get it for free. Less precise on a union of two distinct record
shapes, but sound, and ships without inventing new merge logic.

**Required-field check needs no separate presence test.** `(get v k)` on a missing
key already returns `nil`, and `type-matches?` on the bare field type then fails on
its own unless that type happens to accept `nil` — reusing the exact mechanism
`(map K V)`'s own branch relies on for its key/value checks, rather than adding new
presence-testing logic.

**Soundness, verified two ways.** (1) Every new piece has a targeted unit test
(`record_subtyping_is_width_and_depth_but_conservative`,
`record_union_widens_on_field_mismatch_but_keeps_a_match`,
`record_is_disjoint_only_on_tags_like_every_other_refinement` in `types/mod.rs`;
`record_field_refinement_flows_through_checker` in `types/check.rs`). (2) The
record-literal inference is the highest-blast-radius piece — it changes the
inferred type of *every* `{…}` literal project-wide — so it was diffed directly:
`nest check` run across the whole `std/` + `tests/` corpus with the new `expr_ty`
arm disabled vs. enabled produced a **byte-identical warning list**, i.e. zero new
warnings anywhere in the existing codebase. `is_disjoint` still never inspects
`fields` (tags-only, unchanged), so a wrong or absent record refinement can only
ever *miss* a warning, never manufacture a false one (contract #5).

**Deferred** (`type-records.md`): closed records (reject unknown keys);
`assoc`/`keys`/`vals` field-precise sinks (only `get` was built, the highest-value
case); a less conservative subtyping algorithm. Each additive, gated on a real
consumer per ADR-011.

**Trade.** No new `Value`/`Tag` variant. Supersedes nothing; extends the Step 5+
staircase ADR-078 started and sits alongside `(map K V)` (ADR-078's third slice) as
a second, heterogeneous map refinement.

## ADR-116 — Intersection of arrows: overloaded functions via `(and A B …)`

**Status:** accepted; **shipped 2026-07-05** ([`types.md`](types.md) Step 5+,
[`type-arrow-intersection.md`](type-arrow-intersection.md)). A function's return
type can now depend on which arm's domain a call's argument provably matches —
`(and (int -> int) (bool -> bool))` — instead of the old behavior where two
distinct known arrows silently widened to "any function", discarding both. No new
grammar: reuses the already-shipped `(and …)` conjunctive-type syntax. Advisory
throughout — contract #5 holds. Refines the Step 5+ staircase alongside ADR-078
(arrow/element/map_kv) and ADR-115 (records); closes the item `ROADMAP.md`
flagged as **"the single biggest expressiveness gap"**.

**Context.** ADR-078 explicitly deferred this when it chose `Ty` as a
single-refinement struct over a replacing enum: "intersections for overloaded
fns... the bulk of the set-theoretic-algebra complexity ADR-011 says to defer."
`(and (int -> int) (bool -> bool))` already *parsed* (the existing `(and …)` grammar
folds pairwise through `Ty::intersect`), but `intersect`'s generic `merge_intersect`
helper treated two distinct known `Sig`s as an unresolvable conflict and widened to
`arrow: None` — so a declared overload sig was silently useless.

**Decision — no new syntax; function intersection types already mean overloading.**
`f : (A→B) ∧ (C→D)` is the standard type-theory encoding of overloading (call `f`
with an `A`, get a `B`; call it with a `C`, get a `D`) — precisely
`docs/type-intersections.md`'s already-shipped `(and …)` feature (a value
satisfying every constituent type at once), just applied to two *distinct* arrows
instead of one arrow plus a flat tag (that doc's own `(and fn (int -> int))`
example). So the fix lives entirely in `Ty::intersect`'s arrow handling plus a new
checker consumer — not a new keyword, and it doesn't touch `(map K V)`/`(vector
E)`/`elem`/`fields` intersect logic at all.

**Representation.** `Ty` gained `overload: Option<Arc<Vec<Sig>>>` (tagged
`FN_BITS` like `arrow`), holding only **2+ distinct** signatures — a single one
always collapses back to `arrow`, so every existing single-arrow consumer (the
callback-arity check, `Sig::is_subtype`) is untouched for the common case. The new
`intersect_arrows` extracts each side's candidate list (`overload`'s list, or
`[arrow]`, or `[]` for "any function"); a zero-candidate side leaves the other's
candidates untouched (reproducing today's `(and fn (int -> int))` and
identical-arrow behavior exactly); two sides with candidates dedup-union into a
combined list, collapsing to `arrow` at length 1 or `overload` at 2+.
**`Ty::union` needed zero new logic** — the existing generic `merge_union` helper
already treats `overload` as just another equality-comparable `Option<Arc<T>>`,
exactly how `map_kv`/`fields` were threaded through in ADR-115. **`is_subtype`**
generalizes (not parallels) the old single-arrow check: `other`'s candidate list is
`[the one arrow]` when unrefined-to-an-overload, so the same code path reproduces
the old behavior unchanged; for a genuine overload, `self` must satisfy every
signature `other` requires (at least one of `self`'s own candidates `Sig::is_subtype`
of each) — **sound but not complete** (contract #5), the same conservative shape as
ADR-115's `record_fields_is_subtype`. **`is_disjoint`** stays untouched, tags-only.

**Call-site resolution** (`check/guards.rs`, `resolve_overload_ret` in `ctx.rs`)
mirrors the existing `SigWithVars` parallel-declaration-path pattern: a new
`Ctx::declared_overloads` table (populated by `parse_sig_decl_overload` alongside
`parse_sig_decl`/`parse_sig_decl_with_vars`) is checked in `expr_ty` at the same
priority as a user's other declared-sig forms. For each candidate whose arity fits
(`Sig::param`, already folding a variadic `rest` in), every *known* argument type
must be a subtype of that position's declared type; a fully-compatible candidate's
`ret` is unioned into the result — one match gives the exact per-clause return
type, several gives a sound superset, zero widens to `Ty::ANY` rather than ever
fabricating a return type for a call fitting no declared arm.

**Deferred:** flagging an argument that fails *every* overload arm — needs a
second hook in `check/walk.rs`'s separate arity/argument-checking loop (today reads
only a single `ctx.declared_sig`), materially bigger surface than the return-type
resolution this shipped. The requested payoff ("input-dependent return types") is
fully isolated to `expr_ty` and didn't need it.

**Follow-up, same day: cross-module resolution was missing.** The `Ctx`-based path
above only covers a call in the *same file* as the declaration (`check_file`
allocates a fresh `Ctx` per file). A plain single-arrow `(sig …)` already works
cross-module via a separate mechanism — `%register-sig` writes the raw type-
expression form into a shared heap-level store (`RuntimeCode::declared_sigs`) at
load time, and `declared_heap_sig` reads it back via `.as_arrow()` — so before this
follow-up, an *overloaded* sig was **invisible outside its declaring file**, strictly
worse than a plain single-arrow sig. The fix needed no storage change (the heap
store already held the opaque raw form regardless of what it represented): a new
`declared_heap_overload` (mirroring `declared_heap_sig`, extracting
`.overload_sigs()` instead of `.as_arrow()`) wired into the same fallback positions
`sig_of`/`declared_heap_sig` already occupy in `expr_ty`'s call-form handling and
`callback_ret` (HOF callbacks) — not `check/walk.rs`'s argument-checking loop,
which stays out of scope per the deferred item above. Verified by a Rust test that
*evaluates* a declaration (so `%register-sig` genuinely fires) before typing a call
against a fresh, empty `Ctx` — simulating a second module with zero local knowledge
of the first — and confirmed the test fails without the fix and passes with it.
Verified end-to-end too: a real two-file `nest new` project (an overloaded `clamp`
declared in one module, called via `(:use)` from another) correctly flagged a
genuine mismatch and stayed silent on the correct call.

**Soundness, verified two ways** (the ADR-115 playbook): (1) 9 targeted unit/checker
tests covering two-distinct-arrows, identical-arrow collapse, any-function
one-sidedness, three-way accumulation, rendering, conservative subtyping,
disjointness, the full exact/alternate/mismatch/widen call-resolution matrix, and
cross-module resolution via the heap store. (2) `nest check` across all of `std/` +
`tests/` with the new `intersect_arrows` logic and the new `expr_ty` branch disabled
vs. enabled — byte-identical warning output, zero new warnings (unsurprising, since
nothing in the corpus declares an overload yet — but the diff proves the change is
inert everywhere it isn't used).

**Trade.** No new `Value`/`Tag` variant — an overloaded function is still a plain
runtime closure/native, same as any other `Fn`/`Native`-tagged value; `overload` is
purely a static refinement. Supersedes nothing; closes the Step 5+ "Gaps to parity"
item ADR-078 deferred.

## ADR-117 — Int-literal types: `5` as a type, the first slice of ADR-105's deferral

**Status:** accepted; **shipped 2026-07-05** ([`types.md`](types.md) Step 5+,
[`type-int-literals.md`](type-int-literals.md)). A bare int like `5` in a
`(sig …)` type position is a literal singleton type, exactly like the already-
shipped keyword-literal type (ADR-105). Advisory throughout — contract #5 holds.
Closes the first half of ADR-105's one-line deferral ("bool/int/string literals
are the same machinery... a deferred follow-on").

**Context.** Taken at face value, ADR-105's deferral note reads like a small
mechanical extension. It isn't, for two reasons found by actually scoping it:
(1) `Value` has no `Ord`/`Eq`/`Hash` at all (`Value::Float(f64)` structurally
can't get them, NaN), so a generic `BTreeSet<Value>` literal set across kinds is
impossible — each kind needs its own concretely-typed storage; (2) the existing
`lit: Option<Arc<BTreeSet<Symbol>>>` is hardwired to one tag (`KEYWORD_BIT`) at
every one of its ~6 call sites, so supporting `(or :ok 5)` — two different
literal-bearing tags on one `Ty` — isn't free with a single field.

**Decision.** Point 2 resolves via a pattern this repo already established
twice: `arrow`/`overload` are two independent fields both tagged `FN_BITS`
(ADR-116), and `map_kv`/`fields` are two independent fields both tagged
`MAP_BIT` (ADR-115). A third independent field, `lit_int: Option<Arc<BTreeSet
<i64>>>` tagged a new `INT_BIT`, follows the same precedent: since it's tied to
a *different* bit than `lit`'s `KEYWORD_BIT`, a `Ty` carries both simultaneously
with zero special-casing — `(or :ok 5)` just ends up with `lit: Some({:ok})`
*and* `lit_int: Some({5})`. Every one of the ~6 `lit`/`KEYWORD_BIT` call sites
(`union`'s `merge_union_lit`, `intersect`, `negate`, `is_subtype`,
`is_disjoint`, `Display`) got a mechanically parallel `lit_int`/`INT_BIT`
block, same shape, new tag — no new algorithm, pure duplication of an
already-proven-sound pattern. Grammar (`annot.rs::parse_type`): one new match
arm, `Value::Int(n) => Some(Ty::int_lit(n))`, no ambiguity risk (unlike
keywords, an int literal can't collide with a symbol-spelled base type).
Runtime (`type-matches?`): one new branch, `(int? t) (= t v)`, next to the
existing keyword one — before this, a bare int in type position silently fell
to the `else true` catch-all, accepted but never enforced.

**Scoped to int only, and to declared-sig literal sets only — both
deliberately.** Bool/string literals are the same pattern again, deferred (bool
carries an open design question ADR-105 didn't resolve: whether its "`false`
isn't a literal type" carve-out for keywords still applies once bool literals
are a real kind). More significantly: **call-site argument literal precision
was tried and reverted.** Keywords get more than declared-sig precision —
`Ty::of_value` (the value→type bridge) turns a *literal keyword in code* into
its singleton too, so `(c-mode :bogus)` is a provable disjointness the static
checker itself catches, not just the runtime contract. Extending `of_value` to
do the same for `Value::Int` looked like the obvious symmetric completion, but
`of_value` feeds *every* literal int expression's inferred type throughout the
whole checker, not just call arguments — making every int literal a singleton
changed the *rendered text* of unrelated misuse-warning messages project-wide
(`"got int"` → `"got 5"`), breaking 7 pre-existing, unrelated tests
(`eq_against_a_literal_is_a_guard`, `let_binding_propagates_its_rhs_type`,
`match_literal_pattern_narrows_the_scrutinee`, and four others). Reverted
cleanly rather than pushed through outside the session's agreed scope — a real
design pass (deciding whether the wording churn is acceptable, and auditing
every other `of_value` consumer) is a separate follow-on, not a same-slice
mechanical addition like the rest of this ADR.

**Soundness, verified two ways** (the ADR-115/116 playbook): (1) unit tests
mirroring every keyword-literal test exactly (render, union-exact-but-widens,
subtyping, disjointness-precision, intersection) plus a mixed-kind coexistence
test (`(or :ok 5)`); a checker-level test proving a declared int-literal-set
return type flows through `sig_of`/`expr_ty` to callers; a `tests/contract_test.blsp`
runtime-contract block. (2) `nest check` across all of `std/` + `tests/` with the
new `Value::Int(n)` parse arm disabled vs. enabled — byte-identical, zero new
warnings.

**Trade.** No new `Value`/`Tag` variant — an int-literal type is still a plain
runtime `Int` value; `lit_int` is purely a static refinement, same trick every
other literal-bearing tag uses. Supersedes nothing; closes half of ADR-105's
deferred item, leaves the other half (bool/string, and the reverted
call-site-precision extension) explicitly open.

## ADR-118 — Match exhaustiveness checking over literal-enum types

**Status:** accepted; **shipped 2026-07-05** ([`types.md`](types.md) Step 5+,
[`type-match-exhaustiveness.md`](type-match-exhaustiveness.md)). A `match` whose
scrutinee's declared type is a *pure* keyword- or int-literal enum
(ADR-105/117) is flagged when its clauses don't cover every member, unless a
catch-all clause makes it trivially exhaustive. `case` doesn't exist in Brood
(confirmed dead/vestigial — `crates/lisp/src/eval/mod.rs` tells users to use
`match`/`cond`), so this is `match`-only. Advisory throughout — contract #5
holds.

**Context.** Keyword-literal (ADR-105) and int-literal (ADR-117) types give
`Ty` a precise enumerable set, but nothing consumed it for the reason literal
types are usually introduced: catching a `match` that forgot an arm. Initial
scoping assumed a new `match`-clause parser was needed — the checker has no
correct view of `match`'s real clause shape today (`gradual_of_compound` in
`check/walk.rs` assumes a wrong flat layout, effectively dead for genuine
`match` forms, a pre-existing bug left as-is) — which would have made this a
2-3 slice effort.

**Decision — recognize the compiled failure shape instead of parsing clauses.**
`match` always compiles `(match expr clause…)` to a `let`+`if`+`%eq` chain whose
innermost failure is `(throw [:match-error 'context target 'patterns])`
(`match-no-match`, `std/prelude.blsp`). Two properties of this shape make it a
ready-made signal with **zero new parsing**: (1) the throw is *syntactically
absent* whenever a catch-all clause exists (an irrefutable clause compiles to
its body directly, no further `if`) — so finding the shape at all already means
"no catch-all"; (2) the full list of tried patterns is quoted literal data
sitting in the throw's 4th vector slot — no clause-boundary reconstruction
needed, just read it off. And critically: the else-branch of a `(%eq m__N
lit)` test is `then_only` (`guard_assertion` — being false proves nothing about
the tag in general), so `check_if` never narrows `m__N` down the chain — its
ctx type at the final throw is exactly its original declared type, unchanged.
So the whole check is: recognize the throw shape in the **existing,
already-macroexpanded** checker walk; read the scrutinee's declared type via
the existing `expr_ty`; if it's a *pure* literal-enum
(`is_subtype(Ty::of(Tag::Keyword))`/`…Int`, nothing else mixed in), diff it
against the tried patterns; report what's missing. New code: one helper
(`match_exhaustiveness_gap`, `check/guards.rs`) and one check in `check_into`'s
existing generic call-handling (`check/walk.rs`, the same spot the
function-as-value lint and callback-arity check already live) — no
`SPECIAL_HEAD` entry, no new pass, no `Ty` change.

**Conservative by construction.** A non-literal pattern among those tried (a
destructuring pattern, a guarded bind) bails to `None` rather than
half-reasoning about coverage; a scrutinee whose type isn't a *pure* one-kind
literal-enum (mixed keyword+int, or one with a trailing `nil`) bails too —
sound (never a false positive), incomplete (may miss a real gap), per contract
#5's usual bar.

**Why this doesn't reopen the ADR-117 `of_value` question.** The scrutinee's
type comes from its *declared* `(sig …)` type via the same `ctx.declared_sig` →
`sig_params` → `expr_ty` pipeline every other check uses — nothing about
literal-in-*code* inference (`of_value`) is touched, so the warning-message
wording-churn that got that extension reverted doesn't recur: this check only
ever fires on the one specific compiled `throw` shape, never on an arbitrary
literal expression elsewhere.

**Deferred:** mixed-kind enums (`(or :ok 5)`) and enums with a trailing
non-literal tag (`(or :ok :error nil)`) — both declined by the purity check;
clause redundancy/unreachable-clause detection (a different, simpler problem —
compares clause patterns to each other, no scrutinee-type knowledge needed).

**Soundness, verified two ways** (the session's established playbook): (1) six
targeted tests in `crates/lisp/src/types/check.rs` (missing keyword arm,
full coverage silent, catch-all silent, missing int arm, a destructuring
clause mixed in stays silent, a non-literal-enum scrutinee stays silent) plus a
real end-to-end demonstration through the `brood` CLI producing exactly the
expected warnings. (2) `nest check` across all of `std/` + `tests/` with the
hook disabled vs. enabled — byte-identical, zero new warnings.

**Trade.** No new `Value`/`Tag`/`Ty` field at all — this is purely a new
checker consumer of refinements that already existed (ADR-105/117). Supersedes
nothing; activates the exhaustiveness use case those two ADRs were introduced
for but hadn't yet wired up.

## ADR-119 — Incremental `nest check` cache: designed, not built (defer per ADR-011)

**Status:** proposed (design only), 2026-07-05. Full design in
[incremental-check.md](incremental-check.md). No runtime code. Recorded so the hard
parts are on paper before we commit, and so the build is gated on a concrete
large-real-project need rather than the synthetic 100K–1M-file stress projects.

**Context.** `nest check` (and the check pre-flight in `nest test`/`nest run`) re-reads,
re-parses, and re-checks **every** source + test file on every invocation — O(all files),
even when one file changed. This session parallelised both check passes across the worker
pool (`std/tool/project.blsp`): 100K 41s→12.5s, 300K 161s→37s, 1M ~9min→2:44. But
parallelism is a **constant-factor** win (core count) over an operation that is
fundamentally O(all files, every time). The complexity-class fix for the common
edit→recheck cycle is to cache per-file results and re-do only the delta — ≈O(changed +
dependents), ≈0 on a no-change re-run — which would dwarf any parallelism factor. The
question raised: isn't there now an argument for a proper compilation/caching step?

**Decision — design it now, build it later, in two phases.** Yes, incremental checking is
the architecturally-correct answer for scale, but (a) today's real Brood projects are tiny
— the only driver is a synthetic benchmark — and (b) cache invalidation under Brood's
late-binding model is the genuinely hard part. So ADR-011 applies: **write the design,
defer the build** until a real large codebase justifies it. The design splits by
invalidation shape:

- **Phase 1 — the whole-project CST passes** (`unused-private`, `duplicate-defs`) are
  **pure functions of file text**. Cache each file's extract keyed by `hash(content)`;
  on recheck reuse the extract for unchanged files (skip the parse — the dominant cost)
  and re-aggregate the global counts every run (cheap, no parse). **Sound and complete
  with content hashing alone — no dependency graph** — because the always-recomputed
  aggregate *is* the cross-file coupling. Captures the parse-bound ~40 % of check time.
- **Phase 2 — the per-file type check** (`check-file`) resolves cross-module names through
  the loaded image, so content hashing is insufficient. Instrument `check-file` to record
  a **dependency fingerprint** (the external globals it resolved + their observed
  arity/sig/existence); key the cache on `hash(content) + hash(deps)`; maintain a
  reverse-dependency map to invalidate dependents when a signature changes. One-shot (no
  fixpoint: checking is read-only over the image). This is where the complexity lives —
  deferred behind Phase 1.

**Late-binding interaction.** The cache is a static on-disk artifact for the `nest`
toolchain over source *files*; runtime `def` hot-reload never touches on-disk files, so
it's out of scope. Source-level late binding is captured by Phase 2's fingerprint. And the
**advisory contract** (types.md #5 — the checker never rejects a runnable program) is a
correctness safety margin unavailable to a real compiler: a stale-cache *miss* can only
drop an advisory warning until the next edit, never break a build — so Phase 2 may
over-invalidate freely.

**Cache staleness stamp** (mirrors [image-cache-plan.md](image-cache-plan.md) /
`release.rs::runtime_cache_path`): the whole cache is invalidated on a mismatch of
`BROOD_GIT_SHA` (the checker's logic changes between builds) + a format-version int + a
hash of the checker-relevant prelude/std; stored under `$XDG_CACHE_HOME/brood/check/…`,
never in the project tree.

**Relation to the sibling image cache.** [image-cache-plan.md](image-cache-plan.md) caches
the *evaluated global image* to skip **eval** on warm startup — a different axis (program
startup) from this (advisory checking). Complementary; neither substitutes for the other.
Brood closures are AST-as-data, so neither needs a bytecode format invented.

**Alternatives rejected:** more parallelism / a faster checker (constant-factor only,
never stops re-doing unchanged work); a whole-project cache keyed on an all-files hash
(any single edit busts it); a per-file `check-files` cache without dependency tracking
(unsound under cross-module resolution — the reason Phase 1 is restricted to the pure
passes).

## ADR-120 — Bool and string literal types

**Status:** accepted; **shipped 2026-07-05** ([`types.md`](types.md) Step 5+,
[`type-bool-string-literals.md`](type-bool-string-literals.md)). `true`/`false`/
`"GET"` in a `(sig …)` type position are literal singleton types, exactly like the
already-shipped keyword (ADR-105) and int (ADR-117) literals — closing ADR-105's
deferred item in full.

**Decision.** Mechanical repetition of ADR-117's `lit_int` pattern, twice more:
`lit_bool: Option<Arc<BTreeSet<bool>>>` (bool is natively `Ord`/`Eq`/`Hash`/`Copy`,
a straight copy across all ~6 call sites) and `lit_str: Option<Arc<BTreeSet<String>>>`.
String has one real wrinkle: `Value::Str` is a heap handle (`StrId`), not inline
data, so two textually identical string literals can have different underlying ids
— storing `StrId` would break equality. `lit_str` stores the actual `String`
content (`heap.string(id)`); `Ty::str_lit(s: &str)` takes the string slice, not a
`Value`/`Heap` pair, so `Ty` stays heap-independent like every other constructor.
Independent tags/fields (as established twice already by ADR-115/116/117), so any
combination composes on one `Ty` with zero special-casing.

**`false` is now a legitimate literal type.** ADR-105's "`false` is not a literal
type — use `nil`" guidance was scoped to avoiding `false`/`nil` confusion in an
*enumerated keyword* set specifically, never a technical restriction (booleans and
keywords are different `Value` variants — there was no ambiguity to resolve). Now
that bool-literal types are their own real kind, both values are legitimate
singletons.

**No revisit of the `of_value` call-site-argument question** — the same boundary
ADR-117 settled (extending it for int cascaded into unrelated warning-message
wording across 7 pre-existing tests, reverted). Bool/string literals stay
declared-sig-only.

**Soundness:** unit tests mirroring every `int_literal_*` test exactly, plus a
`tests/contract_test.blsp` runtime block; `nest check` corpus diff (new parse arms
disabled vs. enabled) — byte-identical, zero new warnings.

## ADR-121 — Match exhaustiveness generalized to mixed-kind literal enums

**Status:** accepted; **shipped 2026-07-05** ([`type-match-exhaustiveness.md`](type-match-exhaustiveness.md)).
`match_exhaustiveness_gap` (ADR-118) required the scrutinee's type to be *entirely*
one bit (pure `Keyword` or pure `Int`). Generalized to any combination of the now-5
enumerable tags (keyword/int/bool/string literals, plus `nil` — a natural
one-inhabitant singleton).

**Decision.** The purity check becomes one tag-subset test: build `coverable =
Ty::of(Keyword).union(Int).union(Bool).union(Str).union(Nil)` once;
`target_ty.is_subtype(&coverable)` — since `coverable` carries no refinements,
`is_subtype`'s per-bit refinement checks never fire, so this is exactly "is every
tag in `target_ty` one of these five." The declared/tested-set construction moves
to **string labels** (`BTreeSet<String>`) rather than two separately-typed sets —
sidesteps needing a combined Rust sum-type across 4 different literal payload
types, and composes cleanly with `nil` (no payload at all). A new
`render_literal_pattern` renders any of the five kinds to the same canonical label
used for both the declared type's members and the tried patterns.

**Soundness:** five new tests (`(or :ok 5)` mixed keyword+int, `(or :ok :error
nil)` trailing-nil, a bool-literal match, a string-literal match, a fully-covered
mixed-kind match staying silent); `nest check` corpus diff — still zero new
warnings (`not exhaustive` count unchanged).

## ADR-122 — Match redundancy / unreachable-clause detection

**Status:** accepted; **shipped 2026-07-05** ([`type-match-redundancy.md`](type-match-redundancy.md)).
A `match` clause (or a hand-written `if`/`%eq` chain) whose literal test
duplicates one already tried earlier in the same chain is flagged as unreachable.

**Decision.** A different, independent problem from exhaustiveness — needs no
scrutinee `Ty` at all, purely structural on the compiled `if`/`%eq` chain. Reuses
the exact point `check_if` (`check/walk.rs`) already recognizes a literal `%eq`
guard (`guard_assertion`/`literal_eq_guard`), but extracts the **raw literal
`Value`** instead (a new `literal_eq_test_raw`, since redundancy needs exact value
equality, not a tag `guard_assertion` already collapsed away) and scans *forward*
into the `else`-chain (`find_redundant_clause`) for another test of the same
symbol against the same literal. Stops silently the moment the chain isn't itself
another same-symbol `%eq`-if (a catch-all body, a `match-no-match` throw, or a
divergent hand-written `if`).

**Genuinely general, not `match`-specific** — fires on a hand-written same-symbol
`%eq`-if chain too, the same way ADR-118's exhaustiveness check is really about
the `(throw [:match-error …])` shape rather than `match` itself. **No
double-reporting**: each level's scan only looks *downstream* of itself, so a
chain testing `p1, p2, p1, p2` produces exactly two warnings, not zero or four.

**A real corpus finding, not a bug.** Verifying against the whole `std/` +
`tests/` corpus surfaced exactly one new warning:
`tests/pattern_matching_test.blsp`'s test **"first matching clause wins"**
deliberately writes `(match 1 (1 :first) (1 :second) (_ :z))` to verify runtime
clause-priority semantics — a **true positive**, correctly identifying the
`:second` clause as genuinely unreachable. Left as-is (advisory, never gating;
rewriting a working pre-existing test to dodge a *correct* warning isn't
warranted).

**Soundness:** four targeted tests, including one confirming the hand-written
(non-`match`) case. Given this touches `check_if` — a very hot, heavily-shared
function every `if` in every program passes through — verified with extra rigor:
full baseline stayed green after wiring the hook in, and the corpus diff
surfaced exactly the one true-positive finding above and nothing else.

## ADR-123 — Whole-program soundness under hot reload: designed, not built

**Status:** proposed (design only), 2026-07-05. Full design in
[type-soundness-reload.md](type-soundness-reload.md). No runtime code — recorded
so the reload-conflict resolution is on paper before implementation starts, per
ADR-011 (gated on this being picked up as the next slice of type-system work).

**Context.** `ROADMAP.md`'s Elixir-parity gap list previously marked
*pervasive static soundness/gating* as something Brood deliberately won't
pursue (✋), reasoning that gating on global `def`/`defn` types must conflict
with Erlang-style hot reload (ADR-013: a `def` rebinds a global unconditionally
at runtime, visible to every process sharing that runtime's code region on its
next lookup). That framing is revised: whole-program soundness, including
globals, is now the target, not a deliberately-skipped item.

**The fact that makes this tractable.** Traced the compiler/JIT to confirm:
runtime type safety in Brood is **already fully independent of the static
checker**. Every `Value` carries a runtime `Tag`; every operation — arithmetic,
calls, even the JIT's unboxed fast paths — does a real runtime tag check
regardless of what the checker proved (`crates/lisp/src/lib.rs` labels `types`
"the advisory type lattice + checker — nothing gates on it"; `types/check.rs`
and `eval/compile.rs` are fully separate pipelines over the same AST, with no
data flow from proved `Ty`s into codegen). So a reload that breaks a prior
static proof cannot crash or corrupt anything — worst case is a clean,
catchable runtime type error at the point of actual misuse, the same class of
error Brood already has for any dynamic-typing mismatch. Soundness here is a
claim about the checker's guarantee staying valid, not a memory-safety
property — which means we're free to choose *when*/*how hard* to enforce it.

**Decision.** Treat soundness as continuously re-asserted against the image's
*current* state, not proven once forever: give globals a real, trackable
current type (seeded from curated/inferred/`(sig …)` sources, replacing the
permanent `dynamic()`); have the checker record a dependency edge whenever a
call site is gated on a global's type; on every `def` that rebinds a global,
run a targeted re-check of the new body plus every recorded dependent, and
surface a fresh advisory warning for any assumption that no longer holds. The
reload always still happens — this never blocks `def`, preserving ADR-013 and
the live-image premise. A genuine hard gate only exists for **batch/CI
tooling** (`nest check`/`nest test`, a future `--strict` switch treating any
warning — including these — as a failing exit code), never for the
interactive/live image. `(sig! …)` remains the one opt-in hard runtime gate
inside a running image.

**Why not a hard reject-on-reload.** Blocking `def` when it breaks a caller's
prior proof fights the project's reason to exist — routine, deliberately
inconsistent intermediate states while live-editing. There's also no clean
implementation point: the shared-code-region model (ADR-013) would need to
know every live caller across every process sharing the runtime and decide the
fate of in-flight calls, which the append-only, no-rollback region isn't built
for.

**What's left before this can ship** (deferred, on paper only):
per-global current-type store; the reverse-dependency index (shares its
dependency-fingerprint mechanism with ADR-119's Phase 2 cache — build one,
get both); the `def`-time reload hook (must stay cheap on a hot loop that
`def`s internally, likely gated to attached-checker/LSP sessions only);
precise `is_subtype`-based invalidation that respects refinements, not just
base tags; and a decision on where the fresh warnings surface (LSP push,
`nest run --watch`, REPL message on `def` — probably all three off one event).

**Alternatives rejected:** hard-reject the reload (fights the live-image
premise, no clean implementation point); restrict reloads to widen-only
changes to keep prior proofs permanently valid (constrains real bugfixes more
than the cost it avoids, since nothing crashes either way per the load-bearing
fact above); leave globals `dynamic()` forever (the status quo being revised).

**Update (same day, before any Step-2/3 code was written).** The planned
reverse-dependency index is **superseded, not built**: ADR-119 Phase 2 (the
incremental `nest check` cache) landed the same day and, for an unrelated
reason (skipping re-check of unchanged files in the batch CLI), already ships
the equivalent capability — `check-file-deps`/`check-deps-fp` — via a
**pull-based** re-fingerprint check rather than a maintained push-based
`global → dependents` map. Re-observing a file's already-recorded facts
against the current image is cheap and needs no index at all; a mismatch
means "something this file depended on changed," full stop. So Step 2 as
originally written (build a reverse index) is dropped entirely — there is
nothing independent left to design there. The real remaining gap narrows to
just **Step 3's trigger**: Phase 2's cache is consulted only by the batch CLI
today, with no live-session analogue (nothing re-runs it on a `def` in a
running REPL/eval session, or pushes a result anywhere). Full revision in
[type-soundness-reload.md](type-soundness-reload.md).

## ADR-124 — Cross-module visibility for declared value-type sigs

**Status:** accepted; **shipped 2026-07-05.** The first concrete slice of
ADR-123's "per-global current-type store": before a dependency index or a
reload hook can mean anything, a global's declared type has to actually be
*visible* wherever it's referenced, not just within the file that declared it.

**Context.** Arrow signatures (`(sig f (int -> int))`) already had this:
`%register-sig` stores every `(sig …)` under the module-qualified symbol
(the same key `def` produces), and `sigs::declared_heap_sig` reads it back —
so a call to `f` resolves its declared arrow whether `f` is called
intra-module (post-qualification) or cross-module. **Value-type sigs**
(`(sig x T)`, the non-arrow case the gradual-assignment check consumes) had no
such counterpart: `walk::gradual_of`'s global-reference branch and
`walk::check_def`'s assignment check both only consulted `Ctx::declared_value_ty`
— populated purely by scanning the *current file's own* un-expanded forms
(`check.rs`'s Pass 2.5). A reference to a value-sig'd global from another
module, or even a same-module reference that got qualified to `mod/name`
during expansion, fell through to pure `dynamic()` and lost the bound.

**Decision.** Added `sigs::declared_heap_value_ty` — reads the same
`heap.declared_sig_value` store `declared_heap_sig` does, but keeps the
non-arrow branch of the parsed type instead of `.as_arrow()` (mirrors how
`annot::parse_value_sig_decl` is `parse_sig_decl`'s non-arrow counterpart).
Wired it as a fallback in both places that previously only checked the
file-local ctx: `gradual_of`'s bare-global-reference branch, and
`check_def`'s "does this def's value match its declared type" gate (the
second one hadn't been in scope for ADR-123's original write-up — found while
building the test, since a cross-module test needs *both* the reference side
and the definition side visible to produce a real assignment warning end to
end).

**Soundness:** a new cross-module test mirroring
`overload_resolves_cross_module_via_the_heap_store`'s technique (a real
`Interp`, `eval_str` the declarations so `%register-sig` actually populates
the heap, then check a bare `(def …)` form against an empty `Ctx` — simulating
a second module with zero file-local knowledge): both the mismatch case
(`count: value of type string ... not assignable ... int`) and the consistent
case (assigning a string-declared name from another string global stays
silent) pass. Full `nest check` corpus diff against `std/` + `tests/` — byte-
identical, zero new warnings (91 before, 91 after). 216/216 types tests green.

**Relation to ADR-123.** This makes global value-types real and
cross-module-visible, which the dependency index (recording "this call site
relied on global G : T") needs as a precondition — but it's not the
dependency index itself, and it doesn't touch `def`'s reload path at all. The
remaining ADR-123 work (the reverse-dependency index, the `def`-time re-check
hook) is unaffected and still fully undesigned-in-code.

**Post-merge addendum (same day).** This ADR landed alongside a separately
developed, independent branch — **ADR-119 Phase 2** (the incremental
`nest check` cache) — which merged cleanly at the Rust level but introduced a
new invariant `declared_heap_value_ty` (this ADR) predates: every read of
global state in `types/check/` must route through a `deps::obs_*` wrapper, or
Phase 2's dependency-fingerprint cache can't see what a file's check actually
depended on and may keep serving stale warnings after an edit. `sigs::
declared_heap_sig`/`declared_heap_overload` were updated by that branch to
comply; this ADR's new `declared_heap_value_ty` (added independently, on the
other side of the fork) still read `heap.declared_sig_value` directly. Fixed
to route through `deps::obs_declared_sig_value`, with a targeted regression
test (`cross_module_value_sig_dependency_is_captured_for_incremental_cache`)
that isolates `check_def`'s def-target gate specifically — a pure def target
with no local `(sig …)` and no other reference anywhere in the file, so
nothing else would incidentally record it. Verified the test actually catches
the regression by reverting the fix and confirming it fails (fingerprint
unchanged after the sig edit), then restoring it. 359/359 unit tests, corpus
`nest check` unchanged (91 warnings).

## ADR-125 — `nest run --watch` re-checks on reload (ADR-123's live-session trigger)

**Status:** accepted; **shipped 2026-07-05.** Closes ADR-123's one remaining
open question after the reverse-dependency index turned out to be
unnecessary (see the ADR-123 update above): where does the live-session
trigger for re-asserting soundness on `def` actually live.

**Context.** ADR-119 Phase 2 (`check-file-deps`/`check-deps-fp`) is only ever
consulted by the batch `nest check` CLI. `nest run --watch` already has a
file-change trigger (`std/tool/reload.blsp`'s poll-based `reload-on-change`,
which calls `reload-defs` on every detected edit) — but nothing re-ran the
checker in response.

**Decision.** Gave `reload-on-change` (and its internal `reload--loop`/
`reload--dir-loop`) an optional `on-reload` callback: a 1-ary fn invoked with
the reloaded path after every *successful* reload, its own errors caught
separately so a broken callback can never take the watcher down (same
contract as a broken save). `reload.blsp` stays project-agnostic — it has no
idea what the callback does. `nest run --watch`'s generated glue
(`crates/nest/src/main.rs`) supplies the actual policy: inside a project,
`(fn (_p) (project/check-project-sources))`; outside one (a bare-file watch),
`nil` — unchanged behavior. Safe to call from every watched file's own reload
process concurrently, since ADR-119 Phase 2's dependency recorder is
per-`Heap` (`Heap::check_dep_rec`, landed the same day — see below), not a
shared thread-local; a directory watch spawning one reload process per file
can invoke the checker from all of them at once without corrupting anything.

**A serialization design that turned out unnecessary.** The original plan
was to route every `on-reload` callback through one dedicated serializing
process, because `check-file-deps`' dependency recorder was thread-local at
the time — concurrent green processes migrating across OS threads could
clobber it (documented in `deps.rs`). While this was being designed, an
independent, concurrent refactor moved the recorder onto `Heap` itself
(per-process, not per-OS-thread), making concurrent dep-capture genuinely
safe and the serializing workaround moot. Paused and waited for that refactor
to land and compile before finishing this feature, rather than build a
workaround for a hazard about to be fixed at the source.

**Verified end-to-end**, not just unit-tested: scaffolded a real project via
`nest new`, started `nest run --watch src` in the background, edited a
function body to introduce a real call-site type mismatch while the process
was running, and confirmed the warning appeared live without a restart —
then fixed it and confirmed the warning cleared on the next reload. Also
added `tests/reload_watch_test.blsp` (2 tests) covering the `on-reload`
contract directly (fires with the right path; a throwing callback is caught
and the watcher keeps reloading on later edits) — independent of `nest`/the
checker, since `reload.blsp` doesn't depend on either.

**A test-writing pitfall worth recording:** the first draft of these tests
timed out — not a bug in the feature, but in the test. `file-mtime` is
millisecond-resolution, and two `spit` writes with no gap between them can
land in the same millisecond, which `reload--loop` (correctly) reads as "no
change." A small `(sleep 100)` between the initial write/load and the
edit that's supposed to trigger a reload fixed it. Separately, an early draft
also tripped the module-qualification pre-scan: a variable named to look
like a private, module-owned name (`reload-watch-test--val`, echoing the
enclosing module's own name) gets auto-qualified if *any* literal `def` for
it appears in the same `defmodule`-wrapped file — even one added later, deep
inside a `spawn`/`fn` for debugging. Renamed to a plain, non-`--` name and
read the dynamically-`load`ed global via `(eval 'sym)` rather than a bare
reference, so the checker's static unbound-symbol pass — which can't see a
name a runtime-loaded temp file will define — stays silent without
introducing a real qualification mismatch. Corpus `nest check` stayed at 91
warnings throughout (would have gone to 97 without the `eval` fix).

**What's still open — nothing.** The genuine hard reject this design flagged
as unbuilt (`nest check --strict` / `BROOD_CHECK_STRICT=1` treating any
warning as a failing exit code for CI) turned out to already exist:
`cmd_check` in `crates/nest/src/main.rs` has exited 1 on any nonzero warning
count all along, unconditionally — no flag was ever missing. Checked, not
assumed: confirmed directly (a clean file exits 0, one with a warning exits
1, no flags involved). Also unrelated but discovered along the way: a `(sig
name (A -> B))` declared inside a `defmodule` block doesn't seed the
body-vs-declared-return-type check (`check_def`'s seeding path reads the
file-local `Ctx` under the *bare* name Pass 2.5 recorded, but the expanded
`defn` target is the *qualified* name) — a real false-negative. **Fixed
same-day, see ADR-126.**

## ADR-126 — `defmodule`-declared arrow sigs now seed the body-return-type check

**Status:** accepted; **shipped 2026-07-05.** Fixes the gap ADR-125 surfaced
(logged in `docs/type-annotations.md`'s "Known gap" section, now updated to
"Fixed gap").

**Decision.** Same shape as ADR-124's fix, applied to the arrow-sig seeding
path instead of the value-sig one: `check_def`'s lookup of the declared sig
for the closure it's about to check-seed now falls back from the file-local
`ctx.declared_sig(name)` to the heap-wide `declared_heap_sig(heap, name)` —
`sigs::declared_heap_sig`, already used by call-site checking (`sig_of`),
reads the qualified-key store `%register-sig` populates. `ctx.declared_sig`
is keyed by the *bare* name Pass 2.5 recorded from un-expanded source text;
`name` at the `check_def` call site is `defn`'s *expanded* def head, which is
module-qualified inside a `defmodule` block — the two only coincided at the
root namespace, which is why the gap went unnoticed.

**Verified two ways**, matching this session's established playbook: (1) a
new unit test, `defmodule_declared_arrow_sig_seeds_return_type_check` —
verified its bite by reverting the fix and confirming the test fails, then
restoring it. (2) A full `nest check` corpus diff across `std/` + `tests/` —
byte-identical, 91 warnings before and after. The pattern this fixes (a
genuinely mismatched `defmodule`-qualified `sig` + `defn` pair) doesn't occur
anywhere in the current committed source, so the fix closes a real gap
without surfacing any pre-existing bugs to triage. 360/360 unit tests green
(up from 359).

## ADR-127 — `&optional` params in `(sig …)` arrow grammar

**Status:** accepted; **shipped 2026-07-05**
([`type-annotations.md`](type-annotations.md)). Part of the Elixir-parity
gap list's "richer `(sig …)` type-exprs" item — investigated the other two
parts first (rest params, nested generics) and found both **already
shipped**: `&` rest was already parsed (`parse_arrow`); nested type
variables in compound positions (`(list ?A)`) already unify via the
`SigWithVars`/`SigTerm` route (type-variables.md slices 1–2). `&optional`
was the one genuinely missing piece, and it failed in the worst possible
way: `parse_arrow` had no case for the `&optional` marker symbol, so
`parse_type` on it returned `None`, which propagated out through the
`?`-chained parser and dropped the **entire** declaration silently — not
"optional param unchecked," but "the whole sig vanishes with zero warning."

**Decision.** Extended `Sig` with an `optional: Vec<Ty>` field (empty for
every pre-existing constructor — verified zero behavior change when unused).
`Sig::param(i)` now falls through params → optional → rest, the single
choke point already used by every call-site/subtyping consumer, so adding
the field there was sufficient for argument-type checking with no other
call site needing to know about `optional` specifically. `parse_arrow`
parses `params... [&optional opt...] [& rest] -> ret`, mirroring a
closure's own `(req &optional opt & rest)` shape; `&optional` before `&` is
required (the reverse order is dropped, not misparsed). `Sig::is_subtype`'s
arity-compatibility gate was generalized from an exact `params.len()`
equality check to a **range** comparison (`self`'s achievable arity range
must contain `other`'s) — verified algebraically equivalent to the original
check when `optional` is empty on both sides, so no existing arrow-subtype
comparison changes. `check_def`'s param-seeding filter/loop (the same
`check_fn_seeded` path ADR-126 touched) needed two more fixes to actually
*use* the new field: the filter that gates seeding on `params.len() ==
<closure's real param count>` now accepts the whole arity range instead of
exact equality, and per-position seeding switched from raw `s.params.get(i)`
to `s.param(i)` so optional (and rest) positions get seeded at all.

**A deliberate soundness choice, not just plumbing.** A required param is
seeded with its exact declared type (`bind_sig_param`, `stat`, checked with
`⊆`). An optional param is seeded with `T | nil` instead — via a plain
`bind` (not `bind_sig_param`, so it isn't treated as an exact/authoritative
contract for the dead-clause lint) — because it may genuinely be absent at
the call site. Seeding it as exact `T` would make a defensive
`(if (nil? b) …)` check in the body look like dead code to a lint that
assumes a sig-typed param's declared type is precise; verified directly with
a test pair (a defensive nil-check stays silent; using the param
unconditionally as if it can't be `nil` still warns).

**Verified two ways**, matching this session's playbook: (1)
`optional_sig_params_parse_and_check` — call-site type + arity checking, the
nil-widening behavior in both directions, `&optional` combined with a
trailing `& rest`, and the malformed-marker-order case all covered in one
test, all passing on first write (a good sign the design was right, not just
debugged into working). (2) Full `nest check` corpus diff across `std/` +
`tests/` — byte-identical, 91 warnings before and after. 360/360 unit tests
green (one pre-existing, unrelated failure aside — see note below).

**Unrelated test collision noted, not fixed.** While verifying, found
`cross_module_value_sig_dependency_is_captured_for_incremental_cache`
(ADR-124's regression test) fails against a concurrently in-progress,
uncommitted `deps.rs` refactor (a new `dep.own` exclusion filter dropping a
file's own def-names from its recorded dependency set — a legitimate,
unrelated optimization). Confirmed via `git stash`/`git stash pop` (careful:
this also stashed the other session's concurrent uncommitted work
momentarily, restored immediately) that the failure is caused by that
refactor, not by anything in this ADR. Left untouched — not this ADR's to
fix mid-flight on someone else's in-progress work.

## ADR-128 — Tuple / positional product types

**Status:** accepted; **shipped 2026-07-05**
([`type-tuples.md`](type-tuples.md)). Closes the last concrete item on the
Elixir-parity gap list picked up this session — Brood previously had no way
to express a fixed-arity, per-position-typed vector shape at all.

**Decision.** A fifth structural refinement on `Ty` — `tuple:
Option<Arc<Vec<Ty>>>`, tagged to `Vector` alone (not `pair`, per ADR-003's
vector/list split) — following the exact layering pattern `fields` (ADR-115,
records-on-`Map`) already established: a tuple is still a plain runtime
`[ ]` vector, no new `Value` kind, just a refinement the checker reasons
about. `(tuple T1 T2 …)` in `(sig …)` grammar; `Ty::tuple_of`/`tuple_elems`
constructor/accessor; wired into `union`/`intersect`/`negate` via the same
generic `merge_union`/`merge_intersect` helpers every other refinement uses
(no bespoke merge logic needed — `Vec<Ty>: PartialEq` was already
sufficient).

**Subtyping needed real thought, not just plumbing.** `Ty::elem_ty()` — the
single choke point every `first`/`nth`/`is_subtype` consumer already reads —
now derives a union-of-positions fallback when a type has `tuple` but no
plain `elem`, which is what makes `tuple<int,string> <: vector<int|string>`
fall out for free everywhere `elem_ty()` is consulted, no separate
tuple-awareness needed at most call sites. `is_subtype`'s tuple-vs-tuple case
needed its own function (`tuple_is_subtype`, mirroring `record_fields_is_subtype`'s
shape): exact arity match — unlike a record's open width subtyping, a
tuple's arity *is* its shape — then covariant per position.
`is_disjoint` (the predicate the "argument N expects X, got Y" warnings
actually use, not `is_subtype`) gained a genuinely sound tuple-vs-tuple case
too, the same shape as the existing literal-set special cases: disjoint on
arity mismatch or any disjoint position — advisory-soundness holds, only
ever adds a provable verdict.

**The literal-inference change was the real risk, and it came back clean.**
A vector literal `[a b c]` now infers `tuple_of([...])` (exact per-position
types) instead of widening to `vector_of(union)` — a behavior change to
*existing* inference, not just new grammar. Argued it's strictly safe before
touching it (a tuple is already a subtype of the corresponding uniform
vector via the `elem_ty()` fallback, so nothing that worked before could
stop working), then verified: full `nest check` corpus diff across `std/` +
`tests/` came back byte-identical, 91 warnings before and after.

**Positional sinks** (`first`/`second`/`third`/`last`/`nth` with a literal
index) resolve to the exact position when statically known, not the coarse
`elem_ty()` union every other element access still gets — in-range is the
exact type with no `nil` (a well-typed tuple's arity is fixed and known, so
an in-range access is never absent); a provably out-of-range literal index
is exactly `nil`, matching the runtime.

**Runtime contract:** `type-matches?` (`std/prelude.blsp`) gained a `tuple`
case alongside `record` — vector, exact arity, then per-position check —
for `sig!`/`BROOD_CONTRACTS=1`.

**A workflow gotcha surfaced and worth recording separately:** the
incremental `nest check` cache (ADR-119) is stamped with a git-SHA build-id,
which doesn't change across uncommitted local rebuilds — so iterating on
checker logic without committing can silently serve stale cached warnings
through the `nest check` CLI even after a correct rebuild. Cost real time
mid-session (several "why isn't this working" cycles that were actually
"why is this cache stale") before being traced and worked around with
`BROOD_NO_CHECK_CACHE=1` for the rest of verification. Rust-level
`cargo test` was never affected (`file_warnings()` calls the checker
in-process, no CLI cache in the loop) — only CLI-level `nest check`
invocations during dev iteration are at risk.

**Verified two ways**, matching this session's playbook: (1)
`tuple_sig_params_parse_and_check` covers parsing, call-site argument
mismatch + arity mismatch, all four positional sinks, declared-return-type
mismatch, and the tuple-satisfies-uniform-vector subtype case, all in one
test, all passing on first write. Plus 5 new `sig!` runtime-contract tests
in `tests/contract_test.blsp`. (2) Full `nest check` corpus diff (cache
genuinely disabled this time) — unchanged at 91. 362/362 unit tests,
2605/2605 whole-project test suite.

**Deferred:** nested type variables inside a tuple position (`(tuple ?A
?B)`) — the `SigWithVars`/`SigTerm` route doesn't have a tuple case yet,
only the non-variable `parse_type` path does; gated on a real consumer
(ADR-011).

**Correction (same day, discovered while building ADR-129).** This entry's
"91 warnings before and after" corpus-diff claim was itself measured through
the check-cache staleness bug ADR-129 fixes, and turned out to be
comparing against a **stale cached count**, not a genuinely fresh one. The
true, cache-independent baseline (verified via a clean worktree at this
commit's parent, and again with `~/.cache/brood/check` deleted entirely) is
**93**, not 91 — but the 2-warning difference is **not** a tuple regression:
it's `tests/bytes_test.blsp`'s pre-existing "expects bytes, got `vector<int>`"
mismatch (`byte-length`/`byte-at` given a `[a b rest]`-shaped `match` result),
confirmed present *before* this ADR with that exact wording. This ADR's
literal-inference change only *reworded* it to `(tuple int, int, int)` —
same warning, more precise text, not a new one. No action needed on the
bytes finding itself; recorded here so the "91" figure in this entry and in
`docs/devlog.md`'s matching entry isn't taken as ground truth going forward.

## ADR-129 — `build-id` keys off the running binary's own mtime, not just git-sha

**Status:** accepted; **shipped 2026-07-05**. Fixes the real workflow
gotcha flagged when ADR-128 shipped: the incremental `nest check` cache
(ADR-119) could silently serve stale results during active, uncommitted
checker-logic iteration.

**Root cause.** `(build-id)` — the cache's staleness stamp — was
`"<version>+<git-sha>"`, with `BROOD_GIT_SHA` baked in at compile time via
`crates/lisp/build.rs` running `git rev-parse --short HEAD`. Two compounding
problems: (1) that command is insensitive to a dirty working tree — the same
commit, rebuilt with different uncommitted source changes, produces the
identical SHA; (2) `build.rs`'s own `cargo:rerun-if-changed` directives only
watch `.git/HEAD` and `.git/refs/heads`, so a plain source edit + `cargo
build` doesn't even re-run the script to recompute the (unchanged) SHA. Net
effect: an uncommitted local rebuild of the checker never produces a new
`build-id`, so `nest check`'s cache-stamp comparison
(`project--cache-stamp`/`project--read-cache`) never detects that the
binary's actual behavior changed — it keeps serving warnings computed by the
*previous* binary.

**Decision.** Added a second stamp component, `binary_stamp()`
(`crates/lisp/src/builtins/system.rs`): this executable's own last-modified
time, read at **runtime** via `std::env::current_exe()` +
`std::fs::metadata(..).modified()`, cached once per process (`OnceLock`,
since it can't change mid-run). `(build-id)` is now
`"<version>+<git-sha>+<binary-mtime-hex>"`. This is correct by construction
rather than by tracking which source files matter to which cache: the
binary's own file mtime changes on literally any rebuild, for any reason,
committed or not — no `build.rs` changes needed, no risk of missing a
relevant source path. `build-id` has exactly one consumer in the codebase
(`project--cache-stamp`), so the change is low-risk.

**Trade-off, accepted deliberately.** A rebuild for a totally unrelated
reason (e.g. touching `crates/lisp/src/gui.rs`) also bumps the binary's
mtime and invalidates the *whole* check-cache, even though checker behavior
didn't change. This is strictly the safe direction to be wrong in — the
cache's own design already establishes that over-invalidating (a spurious
cache miss, paying the cost of a superfluous fresh check) is free of
correctness risk, per the "advisory contract" (`docs/types.md` #5): the
checker never rejects a runnable program, so a stale-cache *miss* can only
ever drop a warning that gets caught on the next real check, never fabricate
a false one. Only local `brood`/`nest` developers (rebuilding constantly)
pay this cost; an installed release binary changes rarely, so normal users
get full incremental benefit as before.

**Verified end-to-end**, not just by inspection: confirmed `(build-id)`
changes on a bare `touch` of a source file with zero content change (proving
it's genuinely tied to the rebuilt binary, not file content); then did a
real round-trip against `nest check`'s cache — populated the cache with a
genuine warning, temporarily disabled the check that produces it (`false &&
…`, a real behavior change, not the no-op `if false { } else if …` first
attempted, which is equivalent to no change at all and briefly gave a false
"still broken" reading), rebuilt without committing, and confirmed `nest
check` (no `BROOD_NO_CHECK_CACHE` needed) correctly showed the warning gone;
reverted, rebuilt again, confirmed the warning correctly came back. Both
directions verified before trusting the fix.

**A concrete case where "verify, don't assume" caught a real mistake mid-
session:** the first attempt to prove the bug used `if false { .. } else if
let Some(s) = sig { .. }`, believing it disabled the branch — it doesn't
(`if false {A} else if COND {B}` reduces to exactly `if COND {B}`). The
warning still appearing was correctly read as "the test methodology is
broken," not "the fix doesn't work," precisely because the *fix* had
already been independently confirmed via the plain `touch` test — isolating
which of the two moving parts (the fix vs. the verification harness) was at
fault before concluding either way.

362/362 unit tests unaffected (this bug and its fix are entirely CLI/cache-
layer; `cargo test`'s in-process `file_warnings()` never touched this cache
and was never at risk).

## ADR-130 — `defrecord` is pure prelude sugar over closed maps, not a new `Value` kind

**Status:** accepted and **implemented** 2026-07-10 (`std/prelude.blsp`
`defrecord` macro + helpers; the `eval/mod.rs` `defrecord` stub removed, leaving
`deftype`/`definterface`/`reify` pointing at it; `defrecord` added to the
`SPECIAL_FORMS` highlight list; tests in `tests/record_test.blsp`). This ADR
settled the map-first-vs-records question the roadmap deferred "pending an ADR"
and scoped the work; the build stayed exactly this small — zero new core.
Update (2026-07-10): the *static* checker **now flags** a literal wrong-type
argument at a `sig` call site, records included — `(point "a" 4)` against a typed
constructor is a static warning. The arg check itself pre-existed (ADR-110 gating
"B1") but was dead inside a `defmodule` (pass 2.5 keyed user sigs bare while call
heads resolve qualified); the fix qualifies user sig names and recovers
macro-emitted (`defrecord`) sigs from the expanded forms. So per-field type
enforcement is now *static* too, on top of the return-type flow and
`BROOD_CONTRACTS=1` runtime contracts. See the 2026-07-10 devlog entry. Revisits, but does **not** reverse, the standing "model data with
plain maps" stance (the `eval/mod.rs` helpful-error stub for
`defrecord`/`deftype`).

**Context.** The brood-life dogfooding review's top cross-axis request is a
`defrecord` macro with an optional per-field type. Its three motivations are
real and independently confirmed by other Brood code: (1) the `(get m :key)`
**access tax** — every field read is a verbose primitive call that names the
key but not the thing; (2) program state is **unnamed** — a bare `{:x … :y …}`
map carries no clue what record it is at the def site or in an error; (3)
**map-key typos are silent** — `(get m :witdh)` returns `nil` and the bug
surfaces far away, with nothing to catch it. Against that pull stands a
documented decision (the stub: *"Brood has no records/types — model data with
plain maps; for polymorphism use `defprotocol`/`defimpl`"*), the absolute
immutability rule (ADR-026/112), and the keep-the-core-small rule (ADR-011).
Two things have since shifted the ground under the old stance: the checker now
has real record/shape types — `(record :k T …)`, open, width/depth subtyping,
per-field `get` result types (ADR-115) — and a **closed record is already a
subtype of `map<keyword, any>`** (commit `132bb2a`, `types/mod.rs`). So the
*type* machinery a typed record needs already exists; the only open question is
the *surface* — and whether it demands any new core.

**Options considered.**

1. **Reject — status quo (plain maps + `defprotocol`).** Cheapest in surface,
   but it leaves all three costs standing. The typo cost is the sharpest: it is
   a whole class of silent bug that the language gives the programmer *no* tool
   against, in a codebase whose entire premise is catching mistakes statically
   where it cheaply can. `defprotocol` answers polymorphism, not naming or
   typo-safety. Rejected: the payoff is real and recurring, not a one-off.

2. **`defrecord` as a pure prelude macro over closed maps** — a `defmacro` in
   `std/`, expanding to plain map construction/`get`, with an optional per-field
   `sig` that lowers to the *existing* `(record …)` type. No new `Value`, no new
   `Tag`, no new special form, no kernel Rust at all; records **are** maps at
   runtime, so every map operation (`assoc`, `merge`, `keys`, pattern match,
   `send` across processes) keeps working and immutability is untouched.
   **Chosen.**

3. **A real new `Value`/`Tag` nominal record kind.** Gives nominal identity
   (two records with identical fields are distinguishable; protocol dispatch can
   key on the type) and closed-by-construction typo-safety. Rejected: it is a
   new core `Value` kind (compatibility-contract cost per `docs/types.md`), it
   fractures the "records are maps" property (every map builtin would need a
   record arm, or records become second-class), and it buys nominal identity we
   have no concrete consumer for. Exactly the power-feature ADR-011 says to
   defer until a need forces it, and a violation of "Rust provides mechanism,
   Brood provides policy" (ADR-006) — this is policy.

**Decision — option 2, worked out concretely.** `defrecord` is a Brood prelude
macro (bootstrapped like the other `def*` macros; the `eval/mod.rs` stub is
deleted and its LSP/grammar/`treesit` keyword entries updated). `(defrecord
point (x y))` expands to plain `defn`s over existing primitives:

```lisp
(defn point   (x y) {:x x :y y})   ; positional constructor, named after the record
(defn point-x (p)   (get p :x))    ; one accessor per field
(defn point-y (p)   (get p :y))
```

- **Construction** is a plain closed-map literal — a fresh immutable `Value`,
  no tag, nothing mutable. **Access** goes through the generated accessors,
  which is what kills the `(get m :key)` tax *and* buys typo-safety for free:
  `(point-witdh p)` is a call to an **undefined function**, caught today by the
  checker's unbound-reference lint and at runtime — whereas `(get p :witdh)` is
  forever silent. That is the crux of why the sugar earns its place: the same
  bytes, but a typo becomes a name error instead of a `nil`.
- **Functional update** reuses plain `assoc`/`merge` (records are maps):
  `(assoc p :x 9)` returns a fresh record. We ship *no* new updater primitive;
  an `assoc`-style `(update-record …)` helper is deferred sugar (see open
  questions), not core.
- **Per-field `sig`** is opt-in and lowers to the shipped type grammar. A typed
  `(defrecord point ((x int) (y int)))` additionally emits
  `(sig point (int int -> (record :x int :y int)))` for the constructor and
  `(sig point-x ((record :x int) -> int))` per accessor. Field-presence and
  field-type checking then fall out of the *existing* ADR-115 machinery with
  zero new checker code: the constructor's declared return flows a precise
  record type to every call site, and `(get r :k)`-by-literal already resolves
  to the exact field type. This composes with the gradual checker exactly like
  every other `sig` — advisory, contract #5 holds, never rejects a runnable
  program.

**Net kernel cost: zero.** No `Value`, no `Tag`, no special form, no builtin —
100% a prelude macro plus type machinery that already shipped. This is the
option maximally aligned with all three governing rules at once: minimal core
(ADR-011 — a macro over primitives, never a special form), write-the-language-
in-the-language (ADR-006 — it lives in `std/`), and absolute immutability
(ADR-026 — a record is a plain immutable map, no sneaky mutable anything). It
complements rather than reverses the map-first stance: records *are* maps, so
the stub's advice was never wrong, only incomplete — `defrecord` is the named,
typo-checked *front door* to the same immutable map it always recommended.

**Open sub-questions, deferred to the build slice (each an ADR-011 additive, not
a blocker):**

- **Nominal vs. structural identity.** As specified a record has **no** nominal
  identity — two `defrecord`s with the same fields produce indistinguishable
  maps, and there is no `point?` predicate that can reliably tell a `point` from
  any other `{:x :y}` map. Recommendation: stay **structural** (no hidden
  `:__type__` tag) until a concrete consumer — most likely protocol dispatch on
  record type — forces nominal identity; a tag field is a pure, immutability-
  preserving addition later. Do **not** add it speculatively.
- **Can `sig` typo-catching be a *checker lint* without false positives?** The
  accessor route already catches typos with **zero** false-positive risk (an
  unbound function name is unambiguous). Pushing further — flagging
  `(get r :typo)` on a *closed* record as an unknown key — needs the deferred
  **closed-record** variant (ADR-115's deferral list) and risks false positives
  on legitimate open extension (a map that carries extra keys by design). Keep
  the guaranteed-clean accessor lint now; treat a closed-record key lint as its
  own later decision, gated on measuring the false-positive rate against `std/`.
- **Constructor / updater ergonomics.** Positional `(point 1 2)` is the minimal
  constructor; a keyword-arg constructor and a functional `(update-point p :x
  inc)` updater are ergonomic sugar to defer. Also open: whether accessors
  should validate shape (they don't — a plain `get`, keeping them a
  transparent alias) and the surface bikeshed (record-name casing; the exact
  per-field `sig` spelling). None affect the core decision.

**Trade.** Adds prelude surface (one macro family) in exchange for naming,
a killed access tax, and free typo-safety — at zero core cost. Supersedes the
`defrecord`/`deftype` helpful-error stub in `eval/mod.rs` (records now exist, as
sugar); extends ADR-115's record types with a value-level front end; leaves
ADR-026 immutability and the ADR-006/011 core-size rules fully intact.

## ADR-131 — Dead-clause lint broadens to precise surface `let`-locals (not just sig-typed params)

**Status:** accepted, shipped 2026-07-10.

**Context.** The dead-clause lint (a guard that narrows a variable's type to the
empty set, so the branch can never run — `docs/type-annotations.md`) fired only
for **sig-typed parameters** (`Ctx::sig_params`). That gate kept it
false-positive-free but left the common surface case uncovered:

```lisp
(let (port 8080)
  (cond (string? port) …   ; can never run — port is int
        :else …))
```

The roadmap tracked this as ⬜ "broaden the dead-clause lint beyond sig-typed
params (needs the surface-vs-generated scoping)."

**Decision.** Add a second dead-clause-eligible set, `Ctx::dead_clause_locals`,
holding **surface `let`-locals with a precise type**, and make the lint scan both
sets (`newly_dead_binding`). A `let`-local qualifies iff **all** hold:

1. **Precise (non-redefinable) RHS** — `gradual_of(rhs).dynamic == false`: a
   literal or integer-closed expression, never a call-result or global reference.
   This is what makes the conclusion **reload-safe**: a `dynamic` binding's type
   could change under hot reload, so a "dead" verdict on it could go stale — those
   are excluded, exactly as ADR-124/Gap A excludes redefinable globals.
2. **Surface name** — not a gensym temporary (`<prefix>__<digits>`, factored into
   `is_gensym_sym`). A macro tests its own gensym temps, never the user's named
   local, so the gensym filter *is* the surface-vs-generated scoping the roadmap
   called for — no position inspection at the guard site needed (the sig-param
   lint never needed one either).
3. **Known, non-`never` type** with a source position on the binding.

**Soundness.** A `let`-binding is immutable within its scope, so even an
*over-approximated but precise* type narrowed to `never` proves the branch dead:
if the tracked type `T ⊇` the real type and `T ∩ guard = ⊥`, then
`real ∩ guard ⊆ T ∩ guard = ⊥` too. Restricting to precise (non-`dynamic`)
bindings is not needed for *this-execution* soundness (immutability already gives
it) but keeps every verdict stable across reloads — the same bar the rest of the
reload-aware checker holds (ADR-123/124/125). Shadowing a name drops its
eligibility (mirrors `sig_params`).

**Verification.** Two new checker tests (`dead_clause_flagged_for_a_precise_let_local`,
`dead_clause_let_local_respects_precision_gensym_and_compatibility`) cover the
catch plus the three exclusions (compatible narrowing, call-result/`dynamic` RHS,
gensym); the existing sig-param tests are unchanged. `nest check` stays at **zero
warnings** across `std/` + `tests/` — no false positive in the corpus — and a
real project catches `(let (port 8080) (cond (string? port) …))` end-to-end.

**Trade.** One small `Ctx` set + one gensym helper (also de-duplicated out of the
unused-let lint) for a materially more useful lint, with the surface-vs-generated
risk handled entirely at the *binding* (eligibility), never at the guard. Leaves
the sig-param path and every soundness invariant intact.

## ADR-132 — `Control::Kill`: `(exit …)` reaches a process blocked in a native-nested `receive`

**Status:** accepted, shipped 2026-07-10.

**Context.** A `receive` reached through a **native frame** — inside a `try`/`%isolate`
or a HOF callback (`map`, `fold`, …) — is *native-nested*: its continuation can't be
captured across the native boundary, so instead of the state-capture park it **blocks
the worker thread** on the mailbox condvar (`wait_for_message`, the §7.4 dirty-scheduler
carve-out). `(exit pid reason)` only ever roused a **green waiter** (a parked, captured
continuation) via `wake_parked`; a cv-blocked receiver has no green waiter, so `exit`
did nothing to it and the block path had **no `kill_pending` check**. Result: an exit —
even an untrappable `:kill` — was deferred **indefinitely**; the target only died if some
unrelated message later happened to wake the cv. Because `(try (receive …) (catch …))`
is an idiomatic supervision shape, this was a real liveness hole (found + repro'd during
the 2026-07-10 house-cleaning sweep). The `exit`/`scan_mailbox` comments even claimed a
"`receive_match` loop-top `kill_pending` check" that did not exist.

**Decision.** Add a second `Control` variant, **`Control::Kill`** (the enum was
`Suspend`-only), that rides the error channel like `Suspend` — so `try`/`%try`/`%isolate`
**re-raise** it (`is_control`), never catch it (an exit signal is not a catchable throw)
— and unwinds the native stack untrappably. The reason is **not** carried in the signal:
it stays in the mailbox (`state.kill`), read at death by `handle_capture_outcome`, exactly
as the loop-top hard-kill path already did. Three cooperating pieces:

1. **`exit` wakes a cv-blocked receiver.** When `wake_parked` returns `None` (no green
   waiter), `exit` now `cv.notify_one()`s — mirroring `deliver`'s message-wake path.
2. **The block path checks the flag and unwinds.** `wait_for_message` bails without
   blocking if `kill_pending` is already set (closing the lost-wakeup race: `request_kill`
   publishes `kill_pending` *before* it takes the state lock, so a kill that completed
   before we waited is visible under the lock), and `receive_match` returns
   `Err(Control::Kill)` on wake.
3. **Only the top-level driver converts it.** `vm_run_bc`'s error handler turns a
   `Control::Kill` into `VmOutcome::Killed` **only when `capture`** (the top-level body
   driver of a scheduler-run green process); a **nested** `vm_apply` run (a `map`/`try`
   callback) re-raises so the kill keeps unwinding to the top-level driver — otherwise it
   hit the "nested vm_apply does no kill capture" `unreachable!`. Symmetrically, the block
   check itself is gated on `in_capture_run()`, so it fires only where a driver exists to
   convert it: on the **root / file-runner thread** (e.g. the `nest test` collector's
   native-nested receive under `%isolate`) a `Control::Kill` would just leak as an
   empty-message error, and that thread isn't a killable process, so it keeps the old
   block-and-ignore behaviour.

Both a hard `:kill` and a soft `(exit pid reason)` now die at a native-nested receive; the
reason in `state.kill` distinguishes them at death, matching the capturing-receive path
(`park_on_receive`). A native-nested receive with a *matching* message still completes
normally (the message wins this round; a pending soft exit is honoured at the next
receive, as for a capturing receive) and an `(after …)` timeout still fires.

**Consequences.** The idiomatic `try`-around-`receive` supervision pattern is now
killable, closing the liveness hole. The `Control` channel gains one variant but no new
special form or user-facing surface. The root/file-runner stays un-killable-via-exit (an
acknowledged, pre-existing edge — it owns no `run_one` to retire it). Verified: hard+soft
× try-nested + HOF-nested + normal-message + timeout; 8/8 `exit_test`, race-checked,
GC-stress clean, full suite + `nest test` (2695) green.

## ADR-133 — `|…|` bar-quoted symbols and keywords for round-trip printing

**Status:** accepted, shipped 2026-07-10.

**Context.** The printer upholds a round-trip invariant — `(read (pr-str x)) == x` — for
strings (it re-escapes control chars) and floats (`inf`/`nan` reserved words). But a
**symbol or keyword** whose name isn't a clean atom token broke it. `(symbol "a b")`,
`(symbol "")`, `(symbol "123")`, `(keyword "")` — all reachable via the `symbol`/`keyword`
builtins, which intern any string — printed as `a b` / (empty) / `123` / `:`, which re-read
as *multiple tokens* / EOF / the **number** `123` / the symbol `:`. Keywords built from
arbitrary strings (JSON keys, data-derived names) are the common real case.

**Decision.** Add Common-Lisp-style **`|…|` bar-quoting**: a `|…|` token is a symbol whose
name is the (un-escaped) body; `:|…|` is the keyword form. Inside the bars, `\|` and `\\`
escape a literal bar/backslash; any other `\X` is literal `X`. The printer emits bars
**only when needed** — a name that is empty, holds whitespace/a delimiter/`|`/`\`, or (for
a symbol) would classify as a number / reserved word / keyword / the lone `.` dotted-pair
separator; a clean name still prints bare (`hello`, `:my-key`, `1+`). Non-readable output
(`str`/`print`) always emits the raw name — bars are a *reader* device, not for display.
A single `Scanner::scan_bar_body` backs all three tokenizers — the **reader**, the tooling
**CST** (`cst.rs`), and **`scan-tokens`** (the highlighter/formatter stream) — so they can't
disagree on where a bar-quoted token runs (the ADR-025 "one source of truth" rule). Safe
to add because no existing Brood source uses `|` as a symbol character (all `|` occurrences
are in strings/comments).

**Consequences.** Every symbol and keyword now round-trips through `pr-str`/`read`,
including the pathological spellings — important for a self-editing editor that serializes
values. The language gains one lexical form (no special form, no evaluator change). This
is deliberately more than the strictly-minimal design (ADR-011 would defer a power
feature), taken because the round-trip *invariant* the printer already claims was
genuinely broken for keyword-from-string, a real use. See also the sibling reader change
the same day: number-shaped tokens now need genuine numeric intent (a digit + valid sign
positions), so `++`/`--`/`...`/`1+`/`2+3` read as symbols and the reader agrees with the
`scan_atom_kind` tooling classifier.

## ADR-134 — `editor/buffer-client`: the client half of the buffer-process protocol

**Status:** accepted, shipped 2026-07-11.

**Context.** `editor/buffer`'s actor shell (`spawn-buffer` / `buffer--serve`) defines the
SERVER half of a two-party protocol: the process owns the authoritative text, serializes
edits, keeps the `recent` transform ring, and pushes `[:buffer-updated pid version view
origin]` to subscribers. The other half — what a subscriber must keep per hosted document
and how it folds each push into its local copy — lived only in myedit (`src/collab.blsp`),
though it is the subtle, load-bearing part: echo suppression (your own edit's round-trip
must not clobber newer local keystrokes), transforming a foreign splice over your
in-flight ones (the client-side mirror of the server's ring — how two parties typing in
different places inside one round-trip merge exactly, no CRDT), pending-splice remap, and
the ambiguous-collision resync fallback. myedit's actor endgame (its ROADMAP §E.2) hosts
EVERY buffer as a process, and its collab registry needs the same fold to keep a
crash-recovery text mirror — three consumers of one protocol client.

**Decision.** Extract the client half into **`std/editor/buffer-client`**: a *link* record
(`{:proc :me :version :pending}`, `link-init`), `link-propagate` (local text change →
based `buffer-splice` tagged `:me`, counted in `:pending`), and `link-fold` — a PURE
`(link version view origin) → [link' action]` state machine with the action vocabulary
`:stale | :noop | [:splice [lo hi repl]] | :resync | [:text s]`. The caller applies the
action to whatever holds its local copy (an editor's buffer pool, a registry's mirror, a
test's string); the fold itself never touches a process, so the whole merge matrix is
testable with hand-fed pushes. `text-splice` (the minimal positional diff) and
`view-parts` (the three wire shapes) move here from myedit with it. NOT extracted: a
generic versioned-projection/pubsub module — the server's subscription list is ~30 lines
with exactly one implementation; the protocol split, not a pubsub framework, is the
reusable seam.

**Consequences.** "A document lives in a process; any number of holders track it by
deltas" is now a std capability usable outside the editor (a log tailer, a config
watcher, a test harness). myedit's collab layer rebases onto it unchanged in behavior,
and its every-buffer-hosted flip and registry crash-recovery build on the same fold —
policy stays in the app, protocol lives beside its server half. Content must cross as
splices (`link-propagate`), never closure edits, on any transform-collaborated buffer —
the ring-invisibility caveat `buffer--serve` documents (closure edits remain for
markers/metadata).

## ADR-135 — The top-level program is a green process (everything is a process)

**Status:** accepted, in progress 2026-07-12.

**Context.** Brood's file runner (`brood file.blsp`) evaluated the program's top-level
forms **directly on the main thread**, as a privileged *root process*: it owns no worker
run queue and, when it `receive`s, it **blocks its OS thread** on the mailbox condvar
(`wait_for_message`) rather than parking-and-capturing its continuation the way a
scheduler-run green process does (ADR-100 §8.4). Every other process is equal and
userspace-scheduled; the root is the one exception. That asymmetry is a real
message-latency tax for the overwhelmingly common idiom of a top-level driver talking to a
spawned worker: each leg crosses the main↔worker **thread boundary**, so it pays a
cross-thread `futex` wake + wait *per message* — the direct-handoff fast path
(`wake_enqueue`, ADR-100) and its wake-elision (skip `notify_one` when handing to the
current worker) apply only *between* green processes on a worker, never to the root. Measured
on ping-pong (1M round-trips): root-driver **~6.5 µs/RT**; the *same* program with its
driver moved into a spawned green process runs at **~3.8 µs/RT** — the root penalty is ~2×.
BEAM has no such penalty because in Erlang `main` **is** a process; there is no privileged
thread.

Routing the program through the existing `(load …)`/`eval-string` builtins does **not**
fix it: those run the tree-walker (`eval::eval`), whose `receive` blocks (no reified frame
stack to capture), and even a VM run nested under a builtin frame is *native-nested* — its
continuation can't be captured across the Rust frame, so it blocks too (§7.4). Park-and-
capture requires the program to run as the **direct body** of a scheduler-driven process,
with **no persistent Rust driver frame** between the process entry and each form's VM run.
A single compiled `(do form…)` body is also wrong: top-level `def`/`defmacro` must take
effect *before* later forms are compiled (a macro defined in form 3 used in form 5), which
only per-form **interleaved** compile+eval preserves — the very thing `eval_source` does
today.

**Decision.** Run the whole program as **one** ordinary green process (so `(self)` is a
single stable pid across every top-level form — a per-form process would hand each form a
different `self`, breaking `(def me (self))` … `(send me …)`), driven **form-by-form by the
scheduler**:

1. **A `Program` process body.** `Process.body` gains a program variant carrying the
   read+positioned form list, a current-form cursor, the bracketed namespace/forward-ref
   state (`compile_ns` / `ns_known_names` / `imports`), and the last form's value. The forms
   are GC-rooted in the *process* heap and re-fetched by root index after any collection
   (exactly as `load`/`eval_source` root the unevaluated tail).
2. **`run_program_body` drives one form per entry.** On a fresh entry it compiles the
   cursor's form (`macros::compile`, so an earlier form's `defmacro` is already in effect)
   and VM-runs it; on `Done` it records the value and advances the cursor; on `Suspended`
   it returns the continuation up **unchanged** — the loop's Rust frame unwinds, the cursor
   lives in the `Process`, and a resume re-enters mid-form via the stored `Suspended`. So a
   `receive` anywhere inside any top-level form (however deep in a call chain) park-captures
   like any green process, and the program migrates between workers between quanta.
3. **The main thread spawns it, monitors, and blocks once.** `run_files` mints the program
   process, `spawn_link`/monitors it, and blocks on the single terminal DOWN — one thread
   block for the whole run, not one per message. The DOWN reason carries normal-vs-error;
   an error becomes exit code 1 (terminal restored first, as before). The advisory checker
   still runs on the main thread ahead of the spawn.

**Consequences.** The top-level driver now uses the userspace direct-handoff + wake-elision
path, closing the root penalty: ping-pong drops ~6.5 → ~3.8 µs/RT (~13× → ~7.6× vs BEAM;
the residual gap is intrinsic — immutable-data per-message allocation, heap-captured
migratable continuations, and per-process heap-isolated message copies, none of which we
trade away). `(self)` at the top level is now a normal process pid, not the root pid (more
BEAM-consistent; nothing depended on the old value). Top-level macro semantics are
unchanged (per-form interleave preserved). The root/main thread keeps owning stdout,
terminal teardown, and exit-code translation; a program that never messages is unaffected
in behavior. `nest run FILE` routes through the same path via a `%run-program-file`
primitive (mechanism the language can't express from within a closure — per-form capture
driving), so a project run gets the fast path too; project setup runs first on the main
thread and its `*load-path*` `def` is a shared global the program process sees. The
`--watch`/`--for` wrap (which already spawns the program under a monitor) and the REPL keep
their existing paths for now — moving them onto the mechanism is a follow-on.

## ADR-136 — `require` is a concurrency contract: no observer sees a half-loaded module

**Status:** accepted, shipped 2026-07-11 (fix `fdc35d3`; error-unwind + tests same day).

**Context.** `defmodule` provided its feature at the TOP of its file, so `require`'s
load-once check ("already in `*features*`?") answered yes while the rest of the file was
still evaluating. Single-threaded that's invisible; across processes it's a TOCTOU — a
`(require 'proctree)` racing another process's load of the same module returned
immediately and the caller's next `proctree/fn` call hit *unbound symbol*. Surfaced as a
once-in-several-runs myedit suite flake, but the window is live in production wherever a
spawned loader races an interactive require (myedit's deferred feature loading is exactly
that shape).

**Decision.** `require` returning now MEANS the module's defs exist. Loads in flight are
tracked in `*features-loading*` (feature → loader pid), set before the load begins:
(a) a CONCURRENT requirer waits — 5 ms `sleep` ticks (never a bare `receive`, which would
eat a queued mailbox message) — for the loader's end-of-file `provide`, taking the load
over after ~5 s if the loader died (re-evaluating a module is idempotent: same source,
same defs); (b) a CIRCULAR require inside one file's own load returns immediately (the
loader pid matches — the old early-provide contract, kept); (c) `defmodule`'s
top-of-file provide is suppressed while a require-driven load is in flight
(`defmodule--provide`), and a direct `(load "x.blsp")` keeps the immediate provide it
always had; (d) the marker CLEARS on a failed load (try/unwind + rethrow), else the same
process's retry would fake success via the circular arm and every other process would
stall out the await window per attempt.

**Consequences.** The require contract is now safe under the actor grain the rest of the
system runs on — any process may require anything at any time. Cost: one map lookup per
require; polling waiters (a waiters-list wakeup on `provide` is the known deeper
mechanism if the 5 ms ticks ever matter). The takeover-after-timeout keeps a >5 s load
theoretically re-enterable — acceptable while module loads are sub-second; revisit with
the module-cache work. Load-once state now lives in two places (`*features*`,
`*features-loading*`); a single load-state map would be cleaner if a third state ever
appears.

## ADR-137 — Runtime events: a push system monitor (`system-monitor`), consumed by telemetry

**Status:** accepted (2026-07-19). Implemented: `crates/lisp/src/process/sysmon.rs`,
the `system-monitor` builtin (`builtins/system.rs`), emit sites in
`scheduler.rs` (spawn/exit), `core/heap.rs` (post-collect), and the VM driver's
deopt branch (`eval/compile/mod.rs`); Brood policy `telemetry/watch-runtime`
(`std/telemetry.blsp`); tested by `tests/sysmon_test.blsp`. Deferred follow-ons:
node up/down through this stream, `defevent` schemas, aggregators, the
`nest observe`/`nest mcp` consumers, the remote tier (ADR-011 — see roadmap).

**Context.** The observability timing tier (2026-07-18) added *counts and
snapshots* — `gc-stats` pauses, `sched-stats`, the sampling profiler — but no
way to *consume events as they happen*: an operator watching for long GC
pauses or a supervisor-dashboard tracking process churn had to poll. BEAM
answers this with `erlang:system_monitor/2` (+ trace); .NET with
EventPipe/EventSource. The ADR-106 telemetry seam already gives Brood
apps an attach/handler stream; what was missing was the **kernel emitting into
it**.

**Decision.** A **push** monitor with BEAM's shape, not an EventPipe-style
ring buffer:

- The kernel delivers each selected event the moment it happens as an
  ordinary mailbox message — `[:system kind subject-pid detail]` — to **one**
  subscriber pid, via the same `process::deliver` seam monitor/link/dist
  delivery already uses. No ring buffer, no polling primitive, no new wait
  machinery; the subscriber is a plain process using `receive`, and fan-out /
  aggregation is Brood policy on top.
- One uniform 4-element shape for every kind (`:gc`/`:spawn`/`:exit`/`:deopt`)
  so a single `receive` arm routes all of them; details carry existing
  structured values (the exit reason is exactly what monitors see).
- Config is explicit selection: `(system-monitor pid)` = everything;
  an opts map = exactly its truthy keys; `:gc-min-pause-us` is BEAM's
  `long_gc` threshold. Arming returns the previous config (save/restore).
- Two load-bearing guards: events **about the subscriber are never emitted**
  (its own event-triggered GC would otherwise feed itself forever), and the
  subscriber's **death disarms** the monitor in `deregister` (a dead
  subscriber must not keep charging every spawn/exit/GC in the runtime).
- Cost when off: one relaxed `AtomicBool` load per emit site — the same
  budget class as the profiler's armed check.
- Policy lives in `std/telemetry.blsp`: `watch-runtime` re-emits each kernel
  event as a `[:runtime kind]` telemetry event, unifying runtime and app
  observability behind the ADR-106 listener (its emitter-isolation guarantee
  carries over: a bad handler can't hurt the runtime, only the listener).

**Alternatives rejected.** (1) **A ring buffer + drain builtin** (EventPipe
shape) — cheaper per event under flood, but adds a polling loop, a second
consumption model beside `receive`, and buffer-sizing/overflow policy; the
mailbox already IS a bounded-cost event queue and BEAM demonstrates the push
model at scale. (2) **Emitting telemetry directly from Rust** — would wire the
kernel to a Brood-defined global handler table and run policy inside the
runtime; the pid seam keeps mechanism/policy split and works with no telemetry
loaded. (3) **Multiple subscribers in the kernel** — fan-out is one `send` in
Brood; last-caller-wins matches BEAM and keeps the hot path to one config read.

**Consequences.** A subscriber that selects `:spawn`/`:exit` on a
spawn-heavy workload opts into that message volume (as with BEAM trace) —
use the threshold/selection knobs. Events are fire-and-forget: a slow
subscriber's mailbox grows (mailbox bounds remain a separate roadmap item).
The `:deopt` kind fires from the VM driver's deopt-observation branch only —
JIT-internal fast-link deopts that re-enter without passing it are uncounted
(same undercount the perf-stats counter accepts).

## ADR-138 — The boot cache: expanded-prelude text, not a binary heap snapshot

**Status:** accepted (2026-07-19). Implemented in `crates/lisp/src/lib.rs`
(`boot_from_cache`/`boot_from_source` around the `SHARED` bundle), with
`build_id_string` shared from `builtins/system.rs` and
`gensym_counter`/`gensym_floor` in `core/value.rs`. Opt-out:
`BROOD_NO_BOOT_CACHE=1`.

**Context.** Every OS-process boot (each CLI invocation, `nest` subcommand,
and nextest test shard) rebuilt the shared prelude from source: ~31 ms, of
which ~27 ms was macro expansion — 744 expander invocations of genuine Brood
list-work, already running on the VM (ADR-119), so no cheap dispatch fix
exists (see the 2026-07-19 devlog measurements). Parse, eval, and freeze
together are only ~4 ms.

**Decision.** Cache the *post-compile text*, not the heap. The source boot
prints each prelude form after `eval::macros::compile` (expanded +
namespace-resolved + static-quasiquote) to
`~/.cache/brood/prelude-expanded-<hash(build-id)>.blsp`; a warm boot reads
those forms back and evaluates them directly, skipping the compile pass —
**~38 ms → ~6.5 ms** measured. Load-bearing choices:

- **`build-id` is the staleness key** (ADR-129's insight reused): the prelude
  is `include_str!`'d, so "the binary changed" covers every input the cache
  depends on. The key is embedded in the header line *and* hashed into the
  filename — per-binary files, because `build-id` embeds each executable's own
  mtime (`brood`, `nest`, and every test binary differ; one shared file would
  thrash). Stale siblings are age-pruned (~7 days) at write time.
- **Text, not a binary heap format.** Freeze is 0.7 ms — serializing
  `SharedCode` would buy nothing and cost a versioned format. The reader is
  the deserializer; the printer is the serializer; both already exist and are
  fuzzed.
- **Provably round-trippable or not written.** Each form must satisfy the
  print→read→print fixpoint before it's added; one failure poisons the whole
  cache write and the boot simply stays on the source path. A cache that
  *reads* but fails any later step is deleted and the boot falls back.
- **Gensym safety.** The header records the caching boot's final gensym
  counter; a cache boot floors its counter there (`gensym_floor`) so a runtime
  `gensym` can never re-mint a name embedded in the cached expansions
  (ADR-133 bar-quoting makes the `name__N` symbols print/read cleanly).
- **LSP parity.** The raw prelude is still read positioned on the cache path
  purely for `note_definition` — stdlib `M-.` is identical on both paths; only
  the compile pass is skipped. The raw/cached form streams are zipped 1:1
  (compile never splits a top-level form); any length drift rejects the cache.
- **Concurrent boots** (nextest: many processes, one binary) write via
  pid-suffixed temp file + rename, so a reader never sees a torn file.

**Alternatives rejected.** (1) **Full `SharedCode`/heap serialization**
(ReadyToRun proper) — 0.7 ms upside, a binary format + relocation story
downside. (2) **Making expansion itself fast** — the right long-term lever
(it speeds every `require`/reload) but it's the same VM-on-allocation-heavy
frontier as `pipeline`/`nqueens`, not a startup-sized job; the cache is
orthogonal and doesn't remove that incentive. (3) **Shipping the expanded
prelude inside the binary at build time** — build.rs would need a bootstrapped
interpreter (chicken-and-egg) and every `cargo build` would pay the expansion.

**Consequences.** First boot after a rebuild pays ~38 ms (source boot + cache
write, the write itself trivial); every later boot is ~6.5 ms. The cache
directory is a plain-text mirror of the expanded prelude — useful for
debugging expansion itself. If the printer/reader ever disagree on a form the
cache silently degrades to source boots (correct, just slower), so printer
regressions can't corrupt semantics.

## ADR-139 — Iolists: write boundaries take nested string/bytes trees, flattened once

**Decision.** Every byte-producing write boundary — `tcp-send`, `proc-send`,
`spit`, `spit-append`, `spit-bytes`, `append-bytes` — and the in-memory
materialiser `bytes-concat` accept any **iolist**: a string, a `bytes` value, a
byte int 0–255, or an arbitrarily nested proper list/vector of iolists (`nil`
empty; an improper tail is a final leaf, as in Erlang). One shared iterative
flattener (`builtins::io::flatten_iolist`) lowers the tree to bytes exactly
once, at the write. String leaves are UTF-8 at text boundaries; binary-mode
sockets/children keep their 0–255-codepoint byte-string rule for string leaves.

**Why.** The O(n²) `(str acc chunk)` accumulation class — the response builder,
the log line, the chunked drain — exists only because the write boundaries
demanded one contiguous value. Letting the boundary flatten makes the correct
thing (collect parts, hand over the tree) the default, with zero intermediate
copies. Erlang's model, and a natural fit for a process/`receive` language
whose data is immutable — an immutable tree cannot be cyclic, so the walker
needs no visited set, and flattening is structurally guaranteed to terminate.

**Deliberately NOT included** (ADR-011): `str`/`join` stay display-rendering —
making them flatten would change what `(str [1 2])` prints and conflate
"render for humans" with "serialise for devices"; a future decision may add an
explicit in-memory `iolist->string` if `utf8-bytes->string`+`bytes-concat`
proves too clunky. The checker signature is the shallow surface
(`string|bytes|int|pair|vector|nil` — the lattice can't express recursion);
the flattener enforces the leaves at runtime. The read-side twin (a growable
read buffer that freezes to `bytes`) stays a separate roadmap item.

## ADR-140 — Bit syntax: typed integer segments in the bytes pattern, pure Brood

**Decision.** The `(bytes seg…)` match pattern (previously byte-granular:
literals, one-byte binders, sized `(x n)` sub-bytes, `& rest`) gains **typed
integer segments**: `(x :u16)`, `(x :i32-le)`, … — widths 1/2/4/8 bytes,
unsigned `:uN` and signed two's-complement `:iN`, **big-endian by default**
(network order) with explicit `-be`/`-le` spellings; `(_ :u32)` skips a width;
repeated binders stay equality constraints. The whole feature is **Brood, not
Rust**: the matcher (`match-bytes-typed-seg` in `std/prelude.blsp`) lowers a
typed segment onto new prelude functions — `bytes-uint`/`bytes-uint-le`/
`bytes-int`/`bytes-int-le` (offset-based reads over `byte-at`) and the
encoders `int->bytes`/`int->bytes-le` (truncating to the width, the
bit-syntax convention) — all public, usable outside patterns.

**Why.** The flagship remaining BEAM capability gap (the 2026-07-22 parity
program, ROADMAP): binary protocol parsers destructure frames declaratively
instead of doing index arithmetic, which is what makes porting the HTTP/WS
parsers off the carrier-string bridge tractable. Pure Brood is the
dogfood-correct shape — the mechanism (`byte-at`, overflow-checked
arithmetic) already existed, so no kernel surface grew; a full-range `:u64`
read past `i64` *just works* because ints auto-widen to big integers, giving
exact Erlang semantics for free.

**Deliberately NOT included** (ADR-011): **bit-granular** (sub-byte) widths —
byte-aligned segments cover TLV/length-prefixed protocols; a flag byte's bit
fields are one `bit-and`/`bit-shift-right` away, and bit offsets would double
the lowering's complexity for one consumer (WebSocket headers). **Float
segments** (`:f32`/`:f64`) — no bits↔float primitive exists yet; add when a
consumer appears. **UTF-8 string segments** — `(utf8-bytes->string sub)` after
a sized segment is one call. Each is additive within the same spec-keyword
namespace if a real need lands.

## ADR-141 — Byte-faithful sockets: binary mode is inbound-only, carrier strings are gone

**Decision.** Two coupled changes retire the Latin-1 "carrier string" era:

1. **Kernel: a socket's/child's binary flag governs ONLY the inbound decode.**
   Text mode delivers UTF-8 strings (split multibyte carried across reads);
   binary mode delivers first-class `bytes` values. Outbound
   `tcp-send`/`proc-send` take any iolist in either mode and a **string leaf is
   always its UTF-8 bytes** — the Latin-1 send rule (each codepoint 0–255
   written as one raw byte, an error above U+00FF) is deleted
   (`flatten_iolist` loses `latin1_strings`; supersedes the ADR-139 clause
   that kept it). Raw bytes go out as `bytes` values — that is what they are
   for.
2. **`std/net` is bytes-native end to end.** The http **server** socket is
   binary for the connection's whole life — no read-then-flip-back-to-text,
   so the flip-window race class (the original U+FFFD live-nav bug's shape)
   is structurally gone, and since sends are mode-independent the response
   path (plain, streaming, SSE frames) needs nothing. The http **client**
   connects in binary mode: a response `:body` is a byte-faithful `bytes`
   value (`body-text` decodes a text body; a binary download round-trips
   exactly — impossible over the old text-mode client), and a request `:body`
   may be a string, `bytes`, or iolist. `tcp-drain`/`tcp-drain-timeout`
   return `bytes` (chunks joined once via `bytes-concat`; text-mode string
   chunks contribute their UTF-8 bytes). **SSE deliberately stays on
   text-mode reads**: `text/event-stream` is a UTF-8 text protocol, and the
   kernel's text decode is exactly the right framing for it — binary mode is
   for binary protocols, not a purity rule.

**Why.** The hatch audit's "one bad abstraction": the Latin-1 byte-string +
per-socket mode flag caused U+FFFD corruption, made every binary protocol flip
modes at exactly the right moment (race-prone), and split "bytes" into two
parallel notions. `bytes` values (inbound, ADR-137-era), iolists (outbound,
ADR-139), and bit syntax (parsing, ADR-140) removed every reason it existed;
this ADR deletes the last of it. One send rule everywhere: strings are UTF-8,
bytes are bytes.

**Remaining seam.** `tls-request` is string-typed in both directions (the
request arg is a `String`; the response decode is hardcoded text), so an
https binary body is still not byte-faithful — the client documents it, and
the fix rides the server-mode TLS/reactor work where that surface is
rebuilt anyway.

## ADR-142 — No growable read-buffer value; reads are chunk lists, scans are incremental

**Decision.** The roadmap's "growable read buffer (or `bytes` transient)" item
is resolved by **not building it**. A mutable append-buffer *value* — however
disguised — is a transient, and ADR-026 forbids transients absolutely (one was
shipped and removed once already). And the need it targeted no longer exists:
the read-side idiom is a **list of inbound chunks joined once** at the parse
boundary (`bytes-concat` of the reversed chunk list — the list is itself an
iolist), which is O(n) in copies, allocation-light, and already what every
`std/net` read path does. What *was* still quadratic was CPU, not copying: the
request-head reader re-scanned the whole accumulator for `\r\n\r\n` on every
chunk (a drip-fed head made it O(head²) — the slow-loris amplifier). Fixed in
Brood: `http--read-until` threads a `from` offset (`bytes-index-of` already
takes one), backing up `marker-length − 1` bytes so a terminator straddling a
chunk boundary is still found — each byte is scanned once. A companion
`*http-max-head-bytes*` cap (64 KiB) bounds the memory a terminator-less head
can pin.

**Why record a non-build.** So the item doesn't resurface: the honest reading
of the hatch audit's ask ("an append buffer that freezes to `bytes`") is that
it predated iolists, `bytes`-native sockets, and bit syntax — with those three
shipped, every concrete consumer it listed (head reader, chunked drain, frame
gather) is already O(n) on the chunk-list idiom, and the only kernel-shaped
alternative left standing violates the immutability contract for zero
measured win.

## ADR-143 — The socket reactor: one mio thread for every socket; queued writes; TLS everywhere

**Decision.** `crate::net` is rebuilt around **one reactor thread** (mio /
epoll) that multiplexes every socket the runtime owns — plaintext streams, TLS
streams (server *and* the `tls-request` client), and listeners — replacing
four families of dedicated threads (a blocking reader thread per stream, an
accept thread per listener with a 2 ms nap loop, an actor thread per TLS
connection with a 10 ms poll, a one-shot thread per `tls-request`). The
mailbox contract is unchanged: the same `[:tcp …]`/`[:tcp-closed …]`/
`[:tcp-accept …]`/`[:tcp-error …]` shapes, the same passive-accept
`tcp-controlling-process` handoff (retargeting is a lock-free subscriber-cell
store, as before), the same unclaimed-accept reaper, the same per-chunk
binary-flag read. Structure: a **control plane** (the id registry the builtins
validate against) and a **data plane** (the reactor's per-socket state
machines), joined by a command channel + waker.

**Semantic changes, deliberate:**
- **`tcp-send` is asynchronous.** It lowers the iolist, queues to the reactor,
  and returns; the reactor flushes as the peer accepts bytes. `tcp-close`
  **drains the queue first** (bounded by a 5 s linger), so send-then-close can
  never truncate a response — the old blocking-write model's documented
  footgun (hatch audit #1), now structurally gone. A slow/stuck reader is
  bounded by a 16 MiB per-socket queue cap (past it the connection drops);
  write failures surface as `[:tcp-closed …]`. Callers that could be told
  synchronously (unknown socket, a listener, an unclaimed TLS stream) still
  error at the call.
- **Peer half-close leaves the write side usable** (Erlang semantics): read
  EOF emits `[:tcp-closed …]` but the request-then-FIN client still gets its
  response; the entry lives until close/error/owner-death.
- **TLS is a first-class stream everywhere.** The rustls connections are
  driven sans-io on the reactor, so the old "read+write share state, can't
  split across threads" constraint dissolved: TLS streams honor
  `tcp-set-binary` (including `tls-request` responses), `tls-request` takes an
  **iolist** request, and gains an optional **`ca-pem` trust anchor** argument
  (private CAs; `tls-self-signed` dev servers — also what made in-tree
  end-to-end TLS tests possible at all: `tests/tls_test.blsp` is the first).
  `http-request`/`http-get`/`http-post` accept `:ca` and run binary-mode over
  https, closing ADR-141's "remaining seam" — a binary body is byte-faithful
  over both transports. And `serve-loop` needs *nothing* for TLS: handed a
  `tls-listen` socket it serves https unchanged (pinned by test).

**Why.** Thread-per-socket caps a server at thread-spawn rates and
thread-stack memory (and the TLS actor added a 10 ms latency floor); one
epoll thread is the standard shape for socket scale — the runtime's green
processes were always the right consumer model on top (mailbox delivery,
`receive` backpressure), so only the transport needed replacing. `mio` is
runtime substrate under the boxcar bar: the readiness layer, no
Lisp-callable behaviour.

**Hardened 2026-07-23** (the validation pass): TLS outbound gained the same
`OUT_CAP` accounting the plaintext path had (a stuck HTTPS reader could grow
rustls's writer buffer unboundedly), and a plaintext `OUT_CAP` breach now
notifies the current subscriber even on an unclaimed socket. **Documented
by-design:** a peer half-close leaves a plaintext fd until an explicit
`tcp-close` (`std/net/tcp.blsp`; the serve-loop's per-connection process
reclaims it on exit). **Deferred (LOW):** TLS half-close symmetry with
plaintext (a TLS server can't reply to a client that close_notify'd before
reading), and a lossy `close_notify` if the socket is write-backed-up at
teardown.

## ADR-144 — The dirty-native offload pool: blocking natives park a process, not a worker

**Decision.** BEAM dirty-scheduler parity via the ADR-059 seam, not scheduler
surgery. A new kernel mechanism **`%offload`** runs an **allow-listed**
blocking native on a small OS pool (≈`nproc/4`, min 2, lazily started): the
caller's args are deep-copied out as `Message`s, the pool thread rebuilds them
in a private scratch `Heap`, calls the native's fn pointer, and delivers
`[:offload token result]` (or `[:offload-error token err]`, the ADR-135-style
structured error) back to the caller's mailbox. Policy is the prelude
**`offload`** wrapper: fire `%offload`, park in a **selective receive** on the
job's token (other mail stays queued), rethrow errors as ordinary throws. The
allow list is natives that are long/blocking and data-in/data-out — no
globals, no env lookups, no process identity — today `%git-clone`,
`%git-resolve-ref`, `%pbkdf2-sha256-bytes`, `%digest`, `%hmac`, `slurp`,
`slurp-bytes`, `spit`, `spit-bytes`, `spit-append`, `append-bytes`,
`tls-self-signed`. Offloading anything else is refused at the call (a
heap-sharing or env-reading native off-process would race the caller's
world). The package manager's `%git-clone`/`%git-resolve-ref` call sites go
through `offload` — a `nest fetch` no longer pins a scheduler worker for the
duration of a clone.

**Why this shape.** The runtime already has exactly one blessed pattern for
"blocking work must not hold a worker": run it off-thread, deliver a message,
`receive` (ADR-059 — sockets, subprocess pipes). Reusing it means zero new
park/suspend machinery in the VM, errors ride the existing structured-error
seam, and the policy layer is ~ten lines of Brood. The alternative — true
BEAM-style process *migration* onto dirty schedulers — buys generality (any
native, no copy) at the price of deep scheduler surgery; revisit only if a
consumer needs to offload heap-sharing work. Reduction preemption already
bounds *short* native hogging (~one quantum); the pool is for the genuinely
long tail the `BROOD_STALL_MS` tracer was built to find.

**Also unlocked.** The ADR-071 WASM interop story listed the offload pool as
its gate (a sandboxed component call is a long native); that gate is now
open.

## ADR-145 — WASM component interop, slice 1: the sandboxed native-extension host

**Decision.** ADR-071 (proposed 2026-05-30, docs/interop.md) moves to
**accepted and partially implemented**. The kernel embeds `wasmtime`
(Component Model + Cranelift, fuel metering on) behind a **default-on `wasm`
cargo feature** (the `jit` precedent; `--no-default-features` strips the
engine). Slice 1 is the runtime capability:

- **Primitives** (mechanism): `%wasm-load` (bytes of a compiled component or
  WAT text → instance token), `%wasm-call` (export by name, args marshalled
  by the export's own WIT parameter types, **fuel-capped** — a runaway guest
  traps to a catchable error), `%wasm-exports`, `%wasm-close`. Marshalling:
  ints (range-checked per WIT width), floats, bools, chars, strings, lists,
  tuples, options lower; results additionally lift records → keyword maps,
  variants → tagged vectors, enums → keywords, and a WIT `result` error
  raises. No WASI is wired — **pure compute, deny-everything** in this slice.
- **An instance is mutable state** and follows the language's rule for it: an
  opaque token behind primitives, never a sendable `Value`. Calls serialize
  per instance (the store is single-threaded, a mutex enforces it);
  instances are independent. `%wasm-call` is on the ADR-144 offload
  allowlist — a long guest call parks the process, not a worker.
- **Policy in Brood** (`std/wasm.blsp`): `wasm-load` (file) /
  `wasm-instantiate` (in-memory), `wasm-call`, `wasm-call-blocking` (offload),
  `wasm-exports`, `wasm-close`, and **`use-native`** — the `use Rustler`
  moment: every component export `def`d as an ordinary (hot-reloadable) Brood
  function, driven by the component's own interface, no hand-written stubs.
- **Checker**: a new `(check-allow :unbound …)` category — `use-native`'s
  bindings are runtime `def`s the source checker cannot see; the category
  exists for exactly that class (ground truth = the live image).

**Tests without a toolchain.** `wasmtime` parses WAT text directly, so the
suite's guests are hand-written WAT components (`tests/wasm_test.blsp`) —
including a memory+realloc component that proves strings and `list<s32>`
cross the canonical ABI byte-faithfully, and a spin loop the fuel meter
kills. No wasm toolchain is needed to build or test Brood.

**Deferred to later slices** (the rest of docs/interop.md): the
package-manager `:native` manifest/lock/build-on-fetch integration
(`%wasm-build`), WASI capability grants (deny-by-default stays until then),
guest `resource` handles (opaque stateful guest objects), epoch-based
preemption of in-flight calls, and `Value::Bytes` zero-copy into linear
memory. **Hardened 2026-07-23** (the validation pass): a per-store
`ResourceLimiter` (256 MiB) + a 64 MiB load-input cap close the fuel-doesn't-
bound-memory OOM hole; the registry/instance locks are poison-tolerant; a
`option<option<T>>` marshal is rejected (nil can't distinguish the two levels).
**Still deferred:** an instance **finalizer** — today a `%wasm-load`ed
instance is a manual resource freed only by `%wasm-close` (no process/GC reap;
auto-reap-on-owner-death was rejected as a footgun given no ownership-transfer
op), so a load-without-close leaks.

## ADR-146 — Module privacy is enforced; `(:use-internals mod)` is the grant

**Decision.** The `--` module-private convention becomes **real semantics**
("private should be private"). From inside a module, a hand-written qualified
reference to another module's `--` name — plain or through an `(:alias …)` —
is a **compile error at load**; `(:use mod :only […])` refuses to import one.
The enforcement walk runs in `eval::macros::compile` over the
**pre-expansion source** and skips `quote`/`quasiquote`, which yields the
three deliberate doors:

1. **`(:use-internals mod)`** — the explicit grant (Swift's `@testable
   import` shape): a test or tightly-coupled tool module declares its
   privileged access in its header, loudly and greppably. Implies `(:use
   mod)` for publics; the grant rides the per-file import table under an
   impossible key (`/internals/<mod>`, the `%alias` trick), so every
   save/restore site works unchanged.
2. **Top-level / REPL code (no namespace) is unrestricted** — the
   live-hacking hatch hot reload depends on: redefining or advising a
   private from the REPL keeps working.
3. **A module's macros may expand to its own privates anywhere** — privacy
   governs what an author can *type*, and macro templates live behind
   `quasiquote` (the test framework's `describe`/`test` → `test/test--run`
   pattern made this non-negotiable).

Reflection (`eval`/`global-names`) still sees the flat global table —
enforcement is a source-level contract, not value-level sealing (Java
reflection, not a capability system). The checker surfaces violations too:
`check_file` now reports **compile errors as diagnostics** (previously
swallowed), and its header scan understands `:use-internals`.

**What enforcement flushed out.** Every cross-module private reference
in-tree was triaged: 14 functions that siblings genuinely needed were
**promoted to public API** (net/http's `parse-url`/`request-headers`/
`render-headers`; lineedit's embedding quartet `lineedit-init`/`-handle`/
`-overrides`/`-remember`; project's model six `project-find-root`/
`-abs-paths`/`-collect-sources`/`-apply`/`-parse-dep`/`-parse-deps`;
format's `format-cst-root`), and eleven test files declare
`(:use-internals …)` for the genuinely-internal helpers they pin.

**Supersedes** the "privacy is soft" clause of ADR-019/065 and the
"link-checked `--private`" hatch-findings item (this is the stronger form).

## ADR-147 — Package manager v2: tarball deps + a git-backed registry

**Status:** accepted / implemented (2026-07-24). Extends ADR-037. Design in
[`packages.md`](packages.md) (*The registry (v2)* + the manifest/subcommand tables);
tests in `tests/package_test.blsp` (tarball + registry blocks).

**Context.** ADR-037 shipped a git-/path-deps package manager and *deferred* three
things to v2 "until a concrete pain shows up" (ADR-011): tarball/HTTP source kinds, a
registry, and discovery. The concrete pull arrived (a request to finish both). Two
things also changed since ADR-037: a byte-faithful in-tree HTTP client now exists
(`std/net`, ADR-141/143) — so the planned Rust `%http-get` is unnecessary — and
first-class bytes + `%digest` make download+verify trivial.

**Decision.** Add two source kinds and a registry, reusing the existing machinery and
keeping every ADR-037 invariant.

- **`:tarball` deps** — `[name :tarball URL :sha256 HEX]`. Downloaded via `std/net`'s
  `http-get` (dogfooding the language over a new Rust HTTP client), or read from a
  `file://` path (offline/local artifacts + the offline test path); http(s) follows a
  bounded number of redirects (release assets 302 to a CDN). The **`:sha256` is
  mandatory** — the integrity pin standing in for git's reviewed commit; the bytes
  are verified before extraction, and a mismatch is a loud error. This preserves
  ADR-037's "no unverified code" property (the npm supply-chain surface stays closed).
  Extraction is the **one new Rust primitive, `%untar-gz`** — a thin shell to system
  `tar` (the same dependency tradeoff as `%git-clone`'s `git`), on the ADR-144 offload
  allow-list, stripping the single wrapper directory so the package root lands in
  `_deps/<name>/`. Everything else (cache stamp, lock, load-path, conflict) is the
  git path, generalized.

- **A git-backed registry** — the index is **just a git repo** of metadata
  (`packages/<name>.blsp` = a vector of `{:version :git :ref :description}` entries),
  **not a hosted service**. This is the crux: it keeps ADR-037's decisive property —
  *no central infrastructure to host or pay for* — while adding discovery and named
  resolution (the crates.io-index / Go-proxy model). A URL index is cloned into
  `_deps/.registry-<hash>/`; a local-path index is used in place. `nest publish`
  appends the project's entry (from `:name`/`:version`/`:description`/`:repository`)
  to a **local** index checkout and **does not auto-commit** — the user owns the index
  repo (its commit policy, signing, review): we write, they `git push`. `nest search`
  greps it. A **`[name :version "X.Y.Z"]`** dep resolves the **exact** version to its
  git source and pins it, reusing the `:git` clone/cache/lock path.

- **Invariants kept from ADR-037.** No semver / constraint solver — registry deps are
  *exact* version pins, and two versions of one name is still a loud conflict. No
  install scripts — a tarball/registry package is the same pure Brood source a git dep
  is; nothing runs at fetch beyond `require`'s normal top-level evaluation. Two new
  optional manifest fields, `:description` and `:repository`, feed `publish`.

**Why.** The registry-shape choice ADR-037 called "baked in once and hard to walk
back" is answered without reversing it: a git-backed index *is* decentralized. The
one new Rust primitive (`%untar-gz`) is mechanism the language genuinely can't
bootstrap (a gzip+tar decoder); download, verify, index format, resolution, publish,
and search are all Brood policy (ADR-006). Deferred still (ADR-011): semver ranges,
tarball sources inside registry entries (entries point to git today), signed
packages, and auto-refresh/TTL for the cloned index.

## ADR-148 — Test coverage is function-level, instrumented by hot reload

**Context.** `nest test` reached `mix test` parity on selection (ADR-none; devlog
2026-07-24) except for `--cover`. No coverage mechanism existed. The obvious
implementation — line coverage — needs a seam in the VM: compiled IR nodes carry
`pos: Option<Pos>` and `CompiledArm` records its file, so the position data is
there, but recording it per executed instruction means either a runtime branch on
the hot path or a compile-time instrumentation mode, plus disabling the JIT (native
code bypasses any hook), plus aggregating hits across the many green processes a
suite runs in.

**Decision.** Ship **function-level** coverage first, implemented as **pure Brood
policy with zero kernel support**, and record line coverage as a separate future
tier rather than a half-built version of the same thing.

A function counts as covered when it is **entered once**. The implementation
(`std/tool/coverage.blsp`) composes three things the language already has:

- `global-names` + `source-location` enumerate the denominator — every global that
  is a function defined under the project's `:source-paths`. Macros, natives, and
  data are excluded; std and the prelude can't inflate the count.
- `def` rebinding + late binding (ADR-013) *are* the instrumentation: each target
  is rebound to a shim that records a hit and forwards. Late binding means every
  already-loaded caller, in any process, picks up the shim with no reload.
- `Value::Table` (ADR-107) collects hits. Tests run across processes with separate
  heaps; a table is shared by identity and `table-incr` is atomic, so parallel
  tests can't lose an update. The sanctioned mutable structure, used for exactly
  what it exists for.

**Why this split.** It is the ADR-006 principle applied honestly: Rust provides
mechanism, Brood provides policy — and here Brood needed *no* new mechanism, so
adding a VM coverage mode would have been building machinery to avoid using the
language. It also satisfies the ADR-011 bar: the cheap tier answers the question
that changes behaviour ("what does my suite never touch?"), and the expensive tier
stays deferred until something concrete needs it.

**Consequences, accepted deliberately:**

- **The shim is variadic, not arity-preserving.** It has to be — `arglist` reports
  only ONE arm of a multi-arm function, so a shim built from it would silently
  break the arities it never saw. Variadic forwarding is correct for fixed,
  `&optional`, `& rest`, and multi-arm alike. Costs: an arity error surfaces from
  inside the shim rather than at the call, and every rebind changes the arity.
- **A new off-switch, `BROOD_NO_RELOAD_DIAG=1`**, silences the hot-reload
  arity/macro diagnostics, which coverage would otherwise trip once per function.
  Off-switch only; the default stays on so accidental mismatches still surface.
  `nest test --cover` sets it for its own process.
- **Hit counts are a lower bound, not a profile.** A self-recursive tail call is
  counted once: the VM's `SelfCall` deliberately bypasses global lookup, and so the
  shim. "Was it entered" stays correct; frequency does not.
- **A `--cover` run is not a timing run** — instrumentation adds a frame and
  defeats JIT inlining of the wrapped call.
- Coverage is reported even when the suite fails (that's when it's most useful),
  and `--cover-min` is gated after the suite result so a red suite reports itself
  first.

**Line coverage — built, as a second opt-in tier** (2026-07-25; `docs/coverage.md`
has the detail). It followed the shape sketched here: a **compile-time** seam rather
than a per-instruction runtime check, so an ordinary run's bytecode is byte-for-byte
unchanged; the JIT off; `std/tool/coverage.blsp` extended rather than replaced.

- The seam is `Inst::RecordLine(u32)`, emitted by `emit_node` only when
  `BROOD_COVERAGE` is armed, and executed by `exec_chunk` — which already holds the
  arm's `src_file`, so no new state threads through the hot executor. Hooking the
  tree-walking evaluator instead (where `form_pos` carries file AND line) was tried
  first and does not work: a compiled body never goes through `eval`, so it records
  top-level forms and nothing inside a function.
- The flag is read once and cached, so it must be set **before any `Interp` exists**
  (the prelude compiles during construction). Set too late, it produces no
  instrumentation and no error.
- **The denominator is the compiler's, not the source text's.** This is the part that
  took three attempts and the reason it deserves recording. Counting "lines that hold
  a form" compares different populations and reported 14% for a fully-exercised
  fixture; counting instrumented lines without forcing compilation reported 100% for a
  fixture containing a dead function, because arms compile on first *call*. The
  resolution is `%coverage-precompile`, forcing every project function to compile
  before the suite so a never-called function is in the denominator and nowhere else.
  A wrong percentage is worse than no percentage — it is exactly the number people put
  in a CI gate.
- **A found bug, fixed on the way:** a baked-in std module's forms were attributed to
  whichever file was mid-load when the `require` ran (`%load-string` set no file), so a
  21-line `src/main.blsp` was credited with `std/log`'s lines 127-131/150-152/175. The
  same field feeds `:trace` frames, so this was never coverage-only. `%load-string` now
  takes an optional name, and the embedded-module table carries each module's
  **repo-relative path**, derived from the same literal as its `include_str!` so the two
  cannot drift; `require--force` passes it, so `std/log`'s forms are recorded as
  `std/log.blsp` — a path a tool can actually open. Pinned by
  `crates/cli/tests/std_attribution.rs`: every recorded line must exist inside the file
  it is attributed to. (`source-location` was unaffected — definition sites are recorded
  separately.)
- With both tiers on, `--cover-min` gates on the LINE percentage, being the stricter
  number. A shortfall now prints `FAILED: coverage N% is below …` and raises a bare
  signal `nest` recognises, instead of surfacing as an error with a trace and a version
  banner after a report the user had already read.

## ADR-149 — A binding container is a **list**; a vector there is an error

**Context.** Param lists and `let` bindings were documented as lists ("code is
lists, data is vectors", ADR-010) but vectors were *also* accepted, as a
convenience for Clojure muscle memory. That made every Clojure binding shape
**reinterpret** instead of fail, because a vector in a binding *position* is
already meaningful — it destructures:

| written (Clojure habit) | what Brood made of it |
|---|---|
| `(defn g ([x] :one) ([x y] :two))` | one **2-parameter** fn, patterns `[x]` and `:one`, **empty body** — surfacing later as a misleading `expected 2 arguments, got 1` at the call site |
| `(defn f ([x] x))` | 2 params, empty body → returns `nil` |
| `(let [[a 1] [b 2]] …)` | destructures `[a 1]` against the *value* `[b 2]` → `unbound symbol: b`, **no hint** — while the Scheme-shaped `(let ((a 1) (b 2)) …)` mistake *did* get a flatten hint |

**Decision.** The container is a list, full stop. A **vector** where a binding
container belongs — `fn`/`defn` param list, `let`/`letrec`/`binding` bindings,
`for`/`doseq` bindings — is a clean error with a hint. A vector *inside* one still
destructures (`(let ([x y] p) …)`), which is now its only meaning there. A
`fn`/`defn` whose clause heads are vectors (`([x] …) ([x y] …)`) is rejected as
the same mistake.

**Why not keep both spellings.** The cost was not verbosity, it was that the
*wrong* reading was a valid program. Rejecting the container turns three silent
misreads into three hints, and it cost nothing to adopt: `grep` found **zero**
vector param lists and zero vector binding containers across 46k lines of `std/`
+ `tests/` — only Rust test fixtures and one `dynamic_test` case that existed to
assert the alias.

**Consequences.** `(defn f [x y] …)` and `(let [a 1] …)` stop working (they never
appeared in first-party Brood). The formatter, checker, and LSP are unaffected —
they read the same lists they always did.

## ADR-150 — The pattern pin is `^expr`, not `~expr`

**Context.** A pin — "match the current value of `expr`" — was spelled `~expr`,
reusing the reader's `(unquote …)` for the same "drop to evaluation" intuition it
has in quasiquote (ADR-none; `docs/pattern-matching.md`). But a pin *was*
literally `(unquote expr)`, and quasiquote consumes `(unquote …)` first. So inside
a macro template a pin could not be expressed at all:

```clojure
(defmacro await-tag (tag) `(receive ([:reply ~tag v] v)))   ; ~tag = template unquote
```

The workaround was `` ~(list 'unquote tag) ``. This bit precisely where it hurts:
the request/reply idiom (`(receive ([:reply ^ref v] …))`) is the thing most worth
wrapping in a macro, and `std/proc/gen.blsp` only escaped it by hand-writing
`gen-call` rather than generating it.

**Decision.** `^` becomes its own reader macro: `^form` reads as `(%pin form)`
(Elixir's spelling; Brood has no metadata, so `^` was free). `~` belongs to
quasiquote alone. `~expr` in pattern position is an error naming the fix. The
marker is `%`-prefixed so a user's own `pin` function cannot collide with it.

**Consequences.** 167 pins across `std/` + `tests/` were migrated mechanically
(a scanner that tracks quasiquote context, so only real pins were rewritten). A
`^:kw` / `^{…}` Clojure metadata annotation now reads as a pin of a keyword/map —
called out in the language reference's Clojure crib table.

## ADR-151 — Ambient names are **declared** (`defdyn`), not spelled (`*earmuffs*`)

**Context.** Under ADR-065 any `*earmuffed*` name was *ambient*: never
namespaced, so a `def` of it in any module rebound one root binding. That is
right for `*load-path*` and `defdyn` knobs, and wrong for everything else — it
made an ordinary module-local constant silently global:

```clojure
;; module a                      ;; module b
(def *width* 10)                 (def *width* 999)
(defn a-width () *width*)   ;=> 999   — b clobbered a's binding, no diagnostic
```

Three overlapping meanings had accumulated on one spelling: "dynamic variable",
"config constant" (`std/net/http.blsp`'s `*http-max-head-bytes*` is a plain
`def`), and "exempt from namespacing".

**Decision.** Ambient status comes from a **declaration**: a name declared with
`defdyn` is never namespaced; every other name is, earmuffs included. So a plain
`(def *width* 10)` in module `a` defines `a/*width*` — isolated, like any other
definition — and a knob other modules must reach is declared `(defdyn *knob* v)`.
Earmuffs remain the *convention* for a knob, and the checker still reads them as
a typing signal (`docs/types.md`); they no longer decide scoping.

`defdyn` declares at **expansion** time as well as run time, because the compile
pass resolves namespaces *after* macroexpansion — the mark has to exist before
the `def` head it emits is qualified. The namespace pre-scan (`scan_def_names`)
drops any name the file declares `defdyn`, so declaration order inside a file
doesn't matter.

**Consequences.**

- 29 cross-module knobs in `std/` became `defdyn` (a one-word change each);
  52 module-local ones gained proper isolation with no edit at all.
- The prelude's own registries (`*load-path*`, `*features*`, `*module-docs*`, …)
  stay **plain root globals** and got root **setters** — `set-load-path!` /
  `add-load-path!` / `record-module-doc!` — because only root code can rebind a
  root name now. Declaring them dynamic instead was tried and rejected: it
  changed how `:isolated` tests snapshot them and made a load-path race in the
  suite reappear.
- A name the *kernel* reads bare (`*reload-diagnostics*`, `*features*`) must be
  declared; `*reload-diagnostics*` is now `defdyn`'d in the prelude.

## ADR-152 — Reject the shape; never reinterpret it

**Context.** ADRs 149–151 each removed a case where a plausible-but-wrong
spelling produced a *different working program* instead of an error. A review of
the surface (2026-07-25) found the same failure mode in four more places, all
cheap to close:

**Decision.** Each of these is now a diagnostic, not a reinterpretation:

1. **`(catch Type e body…)`** — Clojure's typed catch. Brood's `catch` takes one
   bare binder, so the class name became the binding and the intended binder was
   evaluated as a statement. Because the prelude defines `e`,
   `(catch Exception e (println "caught" e))` printed **2.718…** — a wrong
   answer with no diagnostic. Detected by shape (a symbol binder whose handler
   starts with a bare symbol and has more forms after it: always dead code, so
   zero false positives).
2. **`&optional`/`&` in a pattern-dispatched `fn`** — matched as a literal symbol,
   so the clause silently stopped being variadic and `(f 1 2)` failed with a
   `[:match-error …]` listing `(x &optional (y 5))` as a *pattern*. Now an error
   naming the two mechanisms.
3. **An unrecognised `defmodule` header clause** — silently ignored, so a
   misspelled `(:use-internal m)` or a Clojure `(:require m)` looked like it
   imported names / granted privacy access and did nothing. Now an error. This
   immediately found four std modules (`encoding`, `datetime`, `stats`, `stream`)
   whose `(:doc "…")` header had been dropping their module docstring on the
   floor since it was written.
4. **A nested quasiquote** — levels are not tracked, so an inner `~x` was expanded
   at the outer level (`` `(a `(b ~(+ 1 2))) `` → `(a (quasiquote (b 3)))`, where
   the standard reading leaves `(+ 1 2)` alone). Now an error with a hint; a
   `` ` `` inside an `~unquote` is ordinary code and stays legal. Level tracking
   can land later and can only widen what is accepted (ADR-011).

Two related fixes came out of the same pass:

- **Arity precedence** now matches its documentation: among arms accepting a call,
  an exact fixed arity beats a variadic one, then the most required params, then
  the **fewest `&optional` slots**. Without that last tie-break, `((x) :one)` vs
  `((x &optional y) …)` resolved by *clause order* — `(f 1)` picked the
  `&optional` arm. Both engines' selectors (`Closure::select_arm` and the VM's
  `CompiledClosure::arm_for`) carry the identical key.
- **Calling data** gets a hint per kind: `(:a m)` → "write `(get m :a)`",
  and likewise for a map, set, or vector in head position. Brood deliberately has
  no callable data; that is a design choice worth *naming* at the point the
  reflex fires, as the reader's `#(…)` / `#"…"` hints already do.

**Why this is a principle and not four bug fixes.** A language surface can accept
a wrong spelling, reject it, or reinterpret it. The first two are fine — the third
is the only one that costs a debugging session, and it is the one an LLM (and a
newcomer carrying Clojure habits) hits most. When a spelling is ambiguous, the
default is to reject it and name the fix (`docs/llm-native.md`).

## ADR-153 — `sig` adoption: annotate `std/`, and what that exposed

**Context.** ADR-082 shipped `(sig …)`/`(sig! …)` as the type-annotation surface.
A surface review (2026-07-25) found **zero `(sig …)` declarations across 46k lines
of `std/`** — the feature was designed, documented, and never used, so nothing was
learned from it. The options were: adopt it, replace it with an inline annotation
in the parameter list, or delete it. **Decision: adopt it**, on the reasoning that
an out-of-band declaration is not disqualifying (Erlang's `@spec` is out-of-band
and widely used), so the honest test is to write some and see what breaks.

**Decision.** Annotate the public API of `std/path` (14), `std/set` (7), and
`std/json` (2) as a pilot, and fix whatever the attempt reveals rather than
working around it. What it revealed, in order:

1. **`bytes` and `decimal` had no spelling in the type grammar.** Both are real
   runtime tags — `type-of` returns them, `bytes?`/`decimal?` narrow to them — but
   `base_ty` had no name for either, so *no signature could describe a bytes value*.
   That rules out `std/encoding`, `std/hash`, and `std/net/tcp`, which are all
   bytes. Added. The compatibility contract in `docs/types.md` says a `Value` kind
   needs a `Tag` and a bit; it also needs a **name**, or it can't be annotated.
2. **`sig!` could not expand early in the prelude.** Its expansion called
   `index-of`, which the prelude defines at ~2770 — so a signature on anything
   defined earlier failed with `unbound symbol: index-of` *while building the
   prelude*. Replaced with a bootstrap-safe `sig--pos`, so a signature now expands
   anywhere in the file.
3. **`BROOD_CONTRACTS=1` turns a declaration into a rebinding.** With that flag
   every `sig` behaves like `sig!`, which rebinds the name — so a `sig` written
   *above* its `defn` (the natural place: a signature is documentation) died with a
   bare `unbound symbol: mod/name` and took the whole module load with it. Two
   fixes: the guard now names the correct placement, and it asks about the
   **namespace-qualified** name (`bound?` on a quoted symbol is not resolved, so it
   was asking about bare `basename` while the definition was `path/basename` —
   reporting "not defined yet" for a correctly-placed sig).
4. **The prelude cannot carry signatures at all.** A runtime contract wraps the
   function in a closure that captures a local frame, and the prelude freeze
   requires a shared closure to capture only the global environment — so
   `BROOD_CONTRACTS=1` + a prelude `sig` is a hard panic. The 14 prelude
   annotations were withdrawn; `std/` modules (loaded after the freeze) are fine.

**Consequences.**

- 23 signatures in `std/`, enforced across module boundaries in both directions
  (argument *and* result type), with `nest check` still at zero warnings.
- The placement rule (`sig` below its definition) is now documented and checked
  structurally by `tests/sig_adoption_test.blsp`.
- **Open design question, deliberately not decided here:** `BROOD_CONTRACTS=1`
  making a *declaration* into an *action* is the same reinterpretation antipattern
  ADR-152 removed elsewhere, and it is why three of the four problems above exist.
  The candidates are (a) `sig` becomes a pure declaration always, with enforcement
  only via explicit `sig!`, or (b) a kernel hook applies registered signatures as
  contracts at `def` time, which is placement-independent and would also reach the
  prelude. That is a call about a shipped feature, so it stays for its own session.

**Also in this pass (the alias trims).** `concat`→`append`, `intersperse`→
`interpose`, `reductions`→`scan`, `all-globals`→`global-names` were pure aliases
(two of them trivial pass-through wrappers) and are gone — one spelling each, ~26
call sites migrated. `cond` no longer special-cases `:else`: bare `else` needs the
case (a symbol would otherwise be an unbound reference), a keyword never did, and
`(cond … :else x)` still catches for the same reason `(cond … 42 x)` does. Kept
deliberately, on the author's call: **`car`/`cdr`** (Lisp lineage a reader expects)
and **`lambda`** (a second spelling of the `fn` special form).

## ADR-154 — Ergonomics & conciseness pass: add the missing sugar, cut the redundant surface

**Context.** A review (2026-07-26) of the whole language for conciseness and
ergonomics found the *core* already terse and coherent — the friction was all one
layer up, in the library/macro surface, where fixes are cheap (pure macros and
renames, no evaluator change). Two gaps dominated, quantified across `std/`:
- **String-building ceremony** — 492 `(str …)` and 151 `(error "…" x)` sites, no
  interpolation. The house error convention *is* quote-chopped concatenation.
- **The manual-loop tax** — 83 top-level `--acc`/`--loop`/`--at`/`--walk` helpers
  (~4% of all `std/` functions) that exist only to be a hand-written tail loop,
  each threading state through a separate top-level name far from its call site.

The review also found redundant surface (two names for one thing) and a
Clojure-surprising `some?`. Since Brood has **no external users** (and won't for
months), the renames are free — the deciding factor for the trims below.

**Decision — add the sugar (all pure prelude macros; zero core/runtime cost).**
- **`fmt`** — string interpolation: `(fmt "x={x} sum={(+ a b)}")`. The template is
  parsed at macro-expansion time and lowered to a plain `(str chunk expr …)`, so it
  is *exactly* the hand-written `str` call minus the quote-chopping — no runtime
  machinery, no new reader syntax, nothing to migrate. `{{`/`}}` are literal braces;
  braces nest inside a hole. Chosen over a reader sigil because `#"…"` is already the
  (rejected) Clojure-regex form and `#b"…"`/`#{…}` are taken — and because "policy in
  Brood, mechanism in Rust" (ADR-006) prefers a macro to a permanent surface commitment.
- **Local loops — `letrec`, not a `loop`/`recur` macro** (prototyped, then dropped).
  The 83 top-level `--acc`/`--loop` helpers were the motivation, and a `loop`/`recur`
  macro was built first (a `letrec` expansion with a macro-time code-walk rewriting
  `recur`), then reshaped to a Scheme named-let, then **removed entirely**. The
  reasoning, in order: (1) since Brood is a Lisp-1, a `loop` macro is a reserved word,
  and the Lisp-1/Lisp-2 tradeoff (Lisp-2 frees the name but taxes every higher-order
  call with `funcall`/`#'` — rejected); (2) `recur` earns its keep in *Clojure* mainly
  because the JVM has **no tail-call optimization** — `recur` is the workaround. **Brood
  has proper tail calls**, so `(defn f (x) (f (dec x)))` is already O(1); the core reason
  for `recur` evaporates. (3) With `recur` gone, `loop` is just terse sugar over
  `letrec`, which Brood already has — not worth a reserved word. **Decision: no
  `loop`/`recur`.** A self-contained local loop is a `letrec`-bound closure called by
  name — `(letrec (go (fn (i acc) … (go …))) (go 0 0))` — which closes over the
  enclosing scope (thread only the changing state) and is O(1) via tail calls; a
  top-level `defn` covers loops needing no enclosing locals. The `loop`/`recur`
  unbound-symbol hint points to `letrec`. The general reserved-word cost of *other*
  macros (`when`/`for`/`cond`/…), and the "make operators lexically shadowable" idea
  (Option C) that would remove it, are recorded in [deferred.md #7](deferred.md).
- **`if-let` / `when-let`** — bind, test the *source* value (a fresh temp, so a
  destructuring target behaves), branch. The AI-facing docs already *claimed* these
  existed; now they do.
- **`some->` / `some->>` / `cond->` / `cond->>` / `doto`** — the conditional and
  short-circuit threading macros, built on the existing `thread-first/last-step`
  placement helpers so "where the value goes" is defined once.
- **`run!`** — the function-valued counterpart of `doseq` (`(run! println xs)`).

**Decision — cut the redundant surface (one spelling each).** Merges/removals,
call sites migrated: `string-contains?`→`includes?` (306 sites; `includes?` is the
polymorphic superset, so this is a semantics-preserving mechanical merge),
`string-index-of`→`index-of` (which gained an `&optional from`),
`string-last-index-of`→`last-index-of`, `string-capitalize`→`capitalize`,
`string-upcase`→`upper`, `string-downcase`→`lower`, `flat-map`→`mapcat`,
`length`→`count`, `entries`→`map-pairs`, `read-file`/`write-file`/`append-file`→
`slurp`/`spit`/`spit-append`, `path-exists?`→`file-exists?`, `working-dir`→`cwd`,
`host`→`hostname`. **`some?`→`any?`** (it means "any element matches a predicate",
which every Lisper misreads as Clojure's non-nil test — freeing the name removes a
silent-wrong-program landmine; `some?` is now unbound, so misuse errors loudly).
The deprecated **`:refer`** import marker is gone — `:only` is the sole filter, and
any other marker is a clean error (ADR-152).

- **String naming is now consistent by policy:** a `string-` prefix survives *only*
  where the bare name would collide with another meaning (`string-repeat` vs the
  list `repeat`) or is a kernel primitive (`string-length`, `string-split`);
  everything else is bare.
- **Reverses part of ADR-153**, which kept `car`/`cdr` "on the author's call" as
  Lisp lineage. This session's explicit directive — simplify aggressively, there are
  no users — supersedes that: `car`/`cdr` are gone (use `first`/`rest`).
- **Deliberately *not* changed:** `multimap`'s `multimap-` prefix and `set/conj`
  are **kept**. Dropping the multimap prefix would break the module *internally* —
  inside namespace `multimap` a bare `get`/`assoc` resolves to `multimap/get`
  (current-namespace-wins), not the prelude, so the prefix is load-bearing, not
  stutter for its own sake. `set/conj` shadows the prelude `conj` only under
  `(:use set)`, exactly like Clojure's `clojure.set` — a namespaced collection lib
  intentionally provides names meant to shadow when used; that is the `:use` contract,
  not a defect.

**Consequences.**
- Purely additive at the core: no new special form, no `Value` kind, no evaluator
  change; the 8 special forms are untouched and immutability is unaffected.
- The checker's curated signatures (`types/check/sigs.rs`) were re-keyed to the new
  names; the `loop`/`recur` unbound-symbol hint points to `letrec` + tail recursion.
- `nest check` stays at zero warnings across `std/` + `tests/`; the in-language
  suite and the Rust checker tests are green. New coverage in
  `tests/ergonomics_test.blsp` (fmt, if-let/when-let, some->/cond->, doto, run!,
  incl. a cross-process send of a closure that uses fmt + a `letrec` loop).

## ADR-155 — `receive` clause bodies compile into the *calling* function, not into a per-message thunk

**Context.** `receive` expanded to `((%receive matcher ms on-timeout))`. The matcher
was a `(fn (msg) …)` built by the `match` compiler whose every clause body had been
wrapped in a `(fn () body…)` **thunk**; `%receive` scanned the mailbox, and returned
the thunk of whichever clause matched for the macro to apply in tail position. The
thunk existed for two good reasons: a body must run only *after* the primitive
commits to (and removes) the message, never during the scan; and returning it for
the caller to apply in tail position is what keeps a receive loop O(1) stack.

Measured on 2026-07-26, it was also the dominant cost of message passing. Isolating
a self-send + `receive` (no cross-process scheduling at all) put a receive at
**820 ns**, against 310 ns for the matching `send` — i.e. `pingpong`'s cost was
essentially all receive machinery, the scheduler handoff having already been
flattened by wake elision and ADR-135. Two compounding defects:

1. Building and calling the thunk cost **~235 ns per received message** (a closure
   plus its captured-env frame, then a second closure activation), against ~50 ns
   for an equivalent small-vector protocol.
2. `Inst::MakeClosure` is not in the JIT subset, so *building* that thunk made the
   **whole matcher arm ineligible for the JIT**. The hot message path ran with no
   native code at all — confirmed by `BROOD_NO_JIT=1` and `BROOD_NO_HOF_JIT=1` both
   changing the number by zero.

**Decision.** Split selection from execution. `%receive` (now arity 2) only answers
**which clause matched and what its pattern bound**, as a `[idx var…]` vector — or
`nil` for no match, and `nil` on timeout (unambiguous: a match always answers with a
vector). Every clause **body is emitted at the call site** by the macro, which
rebinds the pattern variables out of that vector and dispatches on `idx`:

```clojure
(let (r (%receive (fn (msg) …[idx var…]… ) ms))
  (if (= (nth r 0) 0) (let (from (nth r 1)) body0)
                      body1))
```

This is Erlang's "receive clauses compile into the enclosing function", reached from
the Brood side rather than by teaching the kernel to run bytecode mid-scan. Bodies
land in the owning arm's own chunk, so a receive loop's self-call is an ordinary tail
call in that arm; every body sits in tail position, so O(1) stack still holds. The
matcher allocates one small vector (`Inst::MakeVector` — *in* the JIT subset) instead
of a closure, so matcher arms now lower and tier.

**Consequences.** Semantics are unchanged — clause order, selective-receive message
order, `:when` guards (still run exactly once, during the scan), `after`, and
`(receive)` with no clauses all behave as before. Measured: **`ring` 1376 → 720 ms
(−48%)**, **`pingpong` 249 → 194 ms (−22%)**, isolated receive 820 → 615 ns; `loop`,
`bintree`, `nqueens`, `spawn`, `fib`, `sieve` unchanged. Gates: 3417 in-language
tests, 864 Rust tests, the process/message files under
`GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY`, and `nest check` at zero warnings.

The residual gap to BEAM on these rows is now the mailbox/copy/scan machinery
(`send` 310 ns, receive 615 ns), not the clause protocol. Note the deeper lever —
compiling the pattern *test* into the calling arm's bytecode so the scan makes no
closure call at all — is still open; `BROOD_NO_HOF=1` (197 → 509 ms on `pingpong`)
shows how much that per-candidate `vm_apply` still costs.
## ADR-156 — The collection protocol covers every collection; a misread shape is an error, not a reading

**Context.** A 2026-07-26 review of the surface (the sibling of ADR-154's
conciseness pass — this one asked whether the primitives are *orthogonal*) probed
the ops × collection-kinds matrix against the running binary instead of the docs.
The core came out clean: 8 special forms, one pattern grammar at every binding
site, `count`/`empty?` universal. The failures clustered in two places.

**One: a set was not a member of the collection protocol.** `(conj #{1} 2)` raised
"conj: not a collection", `(into #{} [1 1 2])` returned the *list* `(1 1 2)` (kind
and dedup both lost), and `(get #{10 20} 10)` was `nil` while `(get #{10 20} 0)`
was `20` — `get` fell through to `nth` and indexed positionally, so a membership
read returned an answer that is wrong under every reading. `disj` didn't exist at
all in the prelude, and `first`/`rest` erred on a **map** even though `seq`, `last`,
`map`, `filter`, `fold`, `reduce` and `into` all read a map as its `[k v]` pairs.
Separately, `(cons 9 (range 2))` *printed* as the dotted `(9 . (0 1))` while `=`
and `count` both treated it as the proper list it is — a printed form that doesn't
read back as its own value.

ADR-154 had considered `set/conj` and **kept** it, on the stated premise that this
mirrors `clojure.set`. That premise is false: Clojure's `conj` and `disj` are both
`clojure.core` and both polymorphic; `clojure.set` defines neither. The cost of the
mistake was worse than stutter — a module-local `conj` *shadows* the polymorphic one
under `(:use set)`, so `(conj [1 2] 3)` raised "%set-add: expected set, got vector"
in any file that imported the module.

**Two: two deferred pattern features failed by silent reinterpretation** — the
exact thing ADR-152 ("reject the shape; never reinterpret it") exists to prevent.
`(match 2 ((or 1 2) :hit) (_ :miss))` answered `:miss`: `(or 1 2)` is a 3-element
list pattern whose head *binds a variable named `or`*, so a plausible-looking
program took the wrong branch with no diagnostic anywhere. And a map pattern's
unknown keys were *ignored*, so `{:a v}` degenerated to "is the target a map?" — it
matched anything, bound nothing, and the body then died on an unbound `v`, a
diagnostic pointing at the body rather than the pattern.

**Decision — complete the protocol.**
- `conj` / `disj` / `get` / `into` dispatch on a **set** in `std/prelude.blsp`;
  `set/conj` and `set/disj` are removed (`std/set` keeps only what is genuinely
  set-specific: the `set` constructor and `union`/`intersection`/`difference`/
  `subset?`). `conj`/`disj` are variadic there like everywhere else, and `(get s x)`
  is **membership**, yielding the element or the default — so `get` and `contains?`
  finally agree on a set.
- `first` / `rest` accept a **map**, yielding its `[k v]` pairs, matching every other
  seq op. Both are kernel builtins, so the arm went in `builtins/sequences.rs` and a
  dedicated `seqable` domain (`seq` + `Set` + `Map`) went in the primitive signature
  table — kept separate from the shared `seq` const so widening the head/tail pair
  didn't silently widen seven other primitives' domains.
- A lazy **range in a cons tail splices** in the printer, so
  `(cons 9 (range 2))` prints `(9 0 1)` and re-reads as itself. A genuinely improper
  tail still prints dotted.
- **Not changed:** `contains?` still *errors* on a vector or list. Clojure accepts a
  vector there and answers by **index**, which makes `(contains? [1 2] 1)` true for
  the wrong reason; a loud error beats inheriting that trap. `first`/`map`/`into` on
  a **string** also still error — strings bridge through `string->list`/
  `string->graphemes`, and codepoint-vs-grapheme is a decision the caller must make.
  Both are recorded in ROADMAP rather than settled here.

**Decision — reject the two misread shapes.**
- `(or …)` / `(and …)` / `(not …)` in **pattern** position is a clean error naming
  the two spellings that work (one clause per alternative, or a `:when` guard).
  Brood has no alternative/boolean patterns and this ADR does not add them; it makes
  their absence *audible*.
- A **map pattern** accepts only `:keys` and `:or`; any other key (general
  `{:key subpattern}` nesting, `:as` — both still deferred per ADR-011) is a clean
  error. `{}` stays legal as the honest "any map" pattern. The check runs in
  `match-map-vars`/`match-compile-map`, so it covers `match`, refutable `let`,
  `fn`/`defn` clauses and `receive` at once — one grammar, one rejection.

**Decision — `case` exists, and earns its name by what it refuses.** The review
found `case` documented-as-if-existing ("`case` is just `match` with literal
patterns") but unbound, with a foreign-construct hint saying Brood *has* no `case`;
meanwhile the checker already modelled `(case key v1 r1 … default)` in two places.
`case` is now a prelude macro over `match*` taking flat `test result` pairs with a
lone trailing default. Under "one spelling each" a pure alias would not qualify —
what qualifies it is the **restriction**: a `case` test must be a literal, and a
**bare symbol is rejected**, because in `match` a bare symbol silently *binds*.
So `case` is the form that cannot make the one mistake `match` invites, and
anything richer than a constant is rejected with a hint naming `match`. The
exhaustiveness lint now names the surface form (`case: not exhaustive`) instead of
always saying `match`.

**Decision — the combinators and predicates the review found missing.** `partial`,
`complement`, `constantly` (`comp` had shipped alone, leaving a hand-written `fn`
as the only way to partially apply); `vec` (documented in two places, unbound);
`disj`; `nan?` / `infinite?` (`nan`/`inf` are *reader literals*, so the language
could produce a NaN long before it could test for one); and `comment`, the
form-level "don't run this" — Brood has no `#_` discard, and the checker skips a
`comment` body so names inside need not resolve.

**Consequences.**
- No new special form, no `Value` kind, no evaluator semantics change; the 8
  special forms are untouched and immutability is unaffected.
- **Downstream break:** a *qualified* `set/conj` / `set/disj` call is now unbound
  (`hatch/src/web/pubsub.blsp` has four). A bare `conj`/`disj` under `(:use set)`
  keeps working and now means the polymorphic prelude one — the semantics for a set
  argument are identical.
- Two previously-silent misreads now fail at macroexpansion. Any code relying on
  the old readings was already wrong (a variable named `or`; a map pattern that
  matched every map).
- `nest check` stays at zero warnings across `std/` + `tests/`. New coverage:
  `tests/set_test.blsp` (the protocol block, plus proof that `(:use set)` no longer
  shadows `conj` for other kinds), `tests/pattern_matching_test.blsp` (both
  rejections, in every binding position), `tests/ergonomics_test.blsp`
  (combinators, `case`, `comment`, `vec`/`nan?`/`infinite?`),
  `tests/sequence_test.blsp` (the range-in-tail print round-trip).

## ADR-157 — A literal `if` test picks its branch at compile time

**Context.** ADR-154 removed `cond`'s special case for `:else`, on the reasoning that
`(cond … :else x)` still catches "for the same reason `(cond … 42 x)` does". That is
true of the *semantics* and false of the cost. `cond` expands to nested `if`s, so the
catch-all became `(if :else x nil)` — a real keyword `Const` plus a branch. A keyword
constant is not an integer, so the arm fell out of the unboxed-i64 register worker's
subset (`jit_lower_i64_arm`) and dropped to the general JIT path.

Measured 2026-07-26, same checksums, benchmark ports untouched:

| row | `:else` | `else` |
|---|---|---|
| `ackermann` | 4285 ms | **360 ms** (11.9×) |
| `collatz` | 162 ms | 97 ms (1.7×) |
| `primes` | 96 ms | 58 ms (1.7×) |
| `nbody` | 330 ms | 329 ms (float path — unaffected) |

Silent, because `:else` still selected the right branch throughout. Blast radius at the
time: `brood-edit` 94 uses, `pong` 40, and four benchmark ports.

**Decision.** Fold the branch at compile time whenever the test is a literal.
`compile_node`'s `if` arm compiles both branches (so every compile-time effect — slot
allocation, `note_definition` for LSP nav — still happens) and then, if the condition
node is a `Const`, returns the taken branch and discards the other. Truthiness is the
language's own: only `nil` and `false` are falsy, and both are `ConstVal::Atom`s, so
every `ConstVal::Handle` (string, bignum, pair, vector, map, …) is truthy.

**Why this rather than the alternatives.** Rejecting `:else` with a hint (the ADR-152
"reject the shape" reflex) would contradict ADR-154's explicit decision that it still
catches, and would break 134 downstream call sites for a performance reason. A checker
warning would leave the cliff in place and make every author memorise a rule. Folding
fixes it in the compiler, where the problem is: **no spelling is privileged** — `else`,
`:else`, `true`, `42` and a string now cost exactly the same nothing — and no caller
has to know the rule exists. It is also a plain win on its own terms, since a constant
test is dead weight in any program.

**Consequences.** No semantic change: the losing branch was already unreachable, and it
is compiled before being discarded, so nothing about diagnostics or LSP nav moves.
Pinned by `tests/const_test_fold_test.blsp` (which branch runs, the losing branch is not
evaluated, and both spellings agree past the tier threshold, `:serial` so folded arms
really do tier). Gates: 3360 in-language + 68 corpus tests, 864 Rust tests, the fold
tests under `GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY` and on `BROOD_VM=0` / `BROOD_NO_JIT=1`,
`nest check` zero warnings.

**One thing does move: line-coverage denominators** (`nest test --cover-lines`, ADR-148
tier 2). Coverage emits a `RecordLine` per positioned node in `emit.rs`, so a folded-away
branch never gets one and leaves the denominator. Measured on a fixture with
`(if false (+ y 999) (- y 1))`: **33% of 3 executable lines → 50% of 2**. This is
deliberate and, we think, the better metric — that branch is *unreachable*, not merely
unexecuted, so no test could ever cover it and counting it puts 100% permanently out of
reach for no actionable reason. It does NOT weaken the ADR-148 guarantee it might look
like it contradicts: that guarantee is about a never-*called* function reporting 0%
rather than vanishing (which is why `precompile` forces compilation), and a function's
own lines are unaffected here. Flagging it because a project's reported percentage can
move without any test changing.

The general lesson is the one ADR-155 also produced: on this runtime a *constant in the
wrong position* can cost an order of magnitude by silently disqualifying an arm from a
JIT subset. Neither the checker nor the benchmark suite catches that class — only an
A/B against the previous shape does.

## ADR-158 — Protocols move into `std/`: the polymorphism seam ships with the language

**Context.** A review asked what Brood lacks as a language. The largest answer was
open polymorphism: nothing in the tree let a *later* module add a case to an
existing operation. Dispatch was a `cond` on `type-of` (closed — extending means
editing the dispatcher) or multi-clause `defn` (closed the same way, per definition
site). Worse, the runtime hint for `deftype`/`reify` told readers to *"use
`defprotocol`/`defimpl` (the `protocol` module)"*, and **there was no such module**.

The reason that hint existed is the interesting part: the kernel has carried
`types/check/protocol.rs` for months — a full conformance pass that checks each
`defimpl` against its `defprotocol`, reports a missing op, an arity disagreement, or
an op the protocol never declared — plus LSP goto/hover over the same forms. The
*tooling* for protocols was in-tree and dormant. The *macros* lived downstream in
the `hatch` package (`hatch/src/protocol.blsp`), whose own module docstring called
itself **"prototype for std/protocol"**. So the design had already been built,
proven in a real application (hatch's JSON `Encode` protocol replaced a closed
`cond` on `type-of`), and validated by the checker — it had simply never been
promoted.

**Decision — promote it verbatim into `std/protocol.blsp`** (embedded, opt-in via
`(require 'protocol)` / `(:use protocol)`; never in the prelude). It is ~110 lines
of Brood over two registry globals: `*protocols*` (name → declared op specs, the
data the checker/LSP read) and `*impls*` (`[protocol op type-key]` → fn, the
dispatch table). `defprotocol` defines one generic `defn` per op that calls
`dispatch`; `defimpl` registers per-key implementations; `defbehaviour` records a
module-level contract with no value dispatch. `hatch/src/protocol.blsp` is deleted
and hatch consumes std's copy — its 750 tests pass unchanged, which is the migration
proof.

Why promote rather than design fresh: the mechanism is already the answer this repo
would have arrived at (policy in Brood, mechanism nowhere — there is no kernel
support at all), it is the one the in-tree checker already validates, and it has a
downstream user. Designing a second facility would have orphaned both.

**Decision — dispatch stays single, on `type-of` of the first argument.** No
multiple dispatch, and **no second axis keyed on a `:type` field**, even though that
is what would make `defrecord` values dispatchable per shape (records are structural
maps, ADR-130, so they all land on `:map`). A `:type` axis would silently change
what *any* map carrying a `:type` key dispatches to — a reinterpretation of existing
data, which is the failure mode ADR-152 exists to prevent. Per ADR-011 the simple
shape ships; the documented workaround is to branch on a field inside the `:map`
implementation, and the record-shape axis is recorded in ROADMAP as a question
rather than a plan.

**Consequences.**
- No kernel change, no new `Value`, no evaluator change: `defprotocol` lowers to
  `defn`s + registry calls. The prelude is untouched, so a program that never
  `require`s it pays nothing.
- The `deftype`/`reify` hint is now *true*, and names `(require 'protocol)`.
  `docs/types.md`'s protocol-conformance bullet no longer describes a dormant pass.
- The collection protocol (ADR-156) is **still Rust-side** — `count`/`first`/`conj`
  dispatch in the prelude and the kernel, so a user type cannot join it. Re-hosting
  the seq protocol on this facility is the obvious follow-on and is deliberately not
  attempted here: it is a performance-sensitive rewrite of the hottest paths in the
  language, not a promotion.
- New coverage in `tests/protocol_test.blsp` (15 cases): dispatch per kind,
  `:default`, multi-argument ops, single-dispatch pinned explicitly, the loud
  missing-impl error, openness (adding an impl for a built-in after the fact),
  re-registration/hot-reload, introspection, records-dispatch-as-`:map` pinned as a
  limitation, `defbehaviour` recording ops without defining functions, and
  cross-process dispatch through the shared registries.
- **Registration is configuration-time, and the tests proved it.** Both registries
  are updated with `def` — a read-modify-write of an immutable map — so two processes
  calling `defimpl` *concurrently* can lose one update. Top-level `defimpl` (the
  normal case) runs single-threaded as the module loads and is safe; the rule is the
  one telemetry's `attach`/`detach` already follow. This surfaced as a test that
  passed standalone and failed under the full parallel suite, which is exactly the
  property worth pinning — the registering tests are now `:serial` and the contract
  is in the module docstring.
- Writing the tests surfaced a second trap worth knowing: naming a protocol op
  `describe` shadows the test framework's `describe` macro under `(:use test)`, and
  the failure surfaces as "group name must be a string" from a *later* form. Generic
  op names collide with macros exactly as any other global would.

## ADR-159 — Grapheme-*indexed* string accessors: make the correct spelling the fast one

**Context.** `docs/language.md` has stated the rule for months: *"A code point is not
a character — a grapheme cluster is… This is the unit to step a cursor by; stepping
by code point splits a cluster and corrupts the text."* But every **indexed** string
operation — `string-length`, `char-at`, `substring`, `index-of` — is code-point
indexed. So the only correct way to read the cluster at a cursor position was
`(nth (string->graphemes s) i)`, which segments the *whole* string and allocates a
vector of every cluster in it. On the editor's hottest path — a cursor motion per
keystroke — that is O(line length) per character moved, and the incentive it creates
is to quietly use the wrong unit because the right one is expensive.

**Decision.** Add the three cluster-indexed accessors as kernel primitives, mirroring
the code-point trio they shadow:

| Cluster-indexed | Code-point-indexed counterpart |
|---|---|
| `(grapheme-count s)` | `string-length` |
| `(grapheme-at s i [default])` | `char-at` |
| `(substring-graphemes s start [end])` | `substring` |

`grapheme-at` walks to `i` and stops, allocating one string; `grapheme-count`
allocates nothing. Out-of-range reads return the default rather than raising, and
slices clamp at both ends — the `nth`/`take`/`drop` convention, so cursor arithmetic
at a line's edge needs no bounds guards.

Rust, not Brood, because the boundary rules are UAX #29 tables (the same
`unicode-segmentation` dependency `string->graphemes`/`display-width` already use) —
not something the language can bootstrap. This is the "mechanism in Rust" case:
`string->graphemes` remains the vector-producing form for when you genuinely want all
the clusters.

**Deliberately not done.** No grapheme-indexed `index-of`, no rope-level grapheme
cursor. A rope cursor is the real answer for a large buffer (it can cache the
segmentation), but it belongs with the rope, sized against a real editor workload —
these three unblock the correct spelling everywhere first. Recorded in ROADMAP.

**Consequences.** Purely additive; no existing behaviour changes and the code-point
accessors keep their meaning (a byte/codepoint index is still the right unit for a
parser, which is what `string->codepoints` serves). Coverage in
`tests/strings_test.blsp`, every case built on a **decomposed** `e` + U+0301 so
cluster and code-point indices genuinely differ — a precomposed U+00E9 would make the
tests pass while proving nothing, which is the trap in testing this area.

## ADR-160 — Alternative (`or`) and conjunction (`and`) patterns; map keys are sub-patterns

**Context.** ADR-156 made three pattern shapes *loud errors* because they had been
silent misreads: `(or 1 2)` read as a 3-element list pattern binding a variable named
`or`, and a map pattern's unknown keys were ignored so `{:a v}` degenerated to "is it
a map?". Turning them into errors was the right first move — but it also advertises
the absence, and the three are the most-asked-for gaps in the grammar. General
`{:key subpattern}` and `:as` had been deferred since the pattern compiler shipped.

**Decision — implement `or` and `and`.**
- **`(or p q …)`** tries each alternative in order; the first match wins. Every
  alternative must bind the **same names**, checked at compile time with a clean
  error. Without that rule a body could reference a name only some alternatives bind
  and *which* it is depends on the input; Rust rejects it for the same reason.
- **`(and p q …)`** matches every pattern against the same value, left to right, with
  later patterns seeing the names earlier ones bound (so a repeated name is an
  equality constraint, as everywhere else). This *is* the capture-while-destructuring
  idiom — `(and whole {:keys [a]})` is Clojure's `:as`, Rust's `x @ pat` — which is
  why no separate `:as` is needed.
- **`(not …)` stays rejected.** A negative match binds nothing, which makes it a
  *guard*, and `:when` is the guard slot. The hint says so.

An `or`'s `success` branch is **duplicated per alternative** rather than hoisted into
a shared thunk. That is deliberate: a thunk would put each body behind a call and cost
the guarantee that a `match` in tail position stays O(1) stack. Alternatives are
nearly always literals, so the duplication is small — and the tail-position property
is pinned by a 30,000-iteration test.

**Decision — a map pattern's explicit keys are sub-patterns, and the two halves keep
different semantics on purpose:**

| Spelling | Semantics | Absent key |
|---|---|---|
| `{:keys [a b]}` | Clojure destructuring | binds `nil` / the `:or` default; never fails |
| `{:k pat}` | Erlang/Elixir map pattern | the clause **fails** |

Requiring presence is what makes `{:status 200}` a useful *test* rather than a
lenient bind, and it is what both ecosystems do for their respective syntaxes. Keys
are emitted **quoted**: in pattern position a key is a literal (`{[1 2] p}` looks up
the vector), never an expression. `{}` still matches any map.

`:as` in a map pattern stays a hard error *because* explicit keys now work — it would
otherwise read as "this map must have an `:as` key", the exact silent-misread class
ADR-152 forbids. The error names the `and` spelling.

**Consequences.** All of it is in the Brood pattern compiler (`std/prelude.blsp`) —
no new special form, no kernel change — so it lands at every binding site at once:
`match`, refutable `let`, `fn`/`defn` clauses, `receive`. 22 new cases in
`tests/pattern_matching_test.blsp` cover alternatives (literal, binding, nested,
mismatched-binding rejection, tail-position), conjunctions (capture, sequenced
bindings, failure), and map sub-patterns (presence semantics, nesting, non-keyword
keys, mixing with `:keys`, and all four binding sites).

## ADR-161 — Transducers become public surface

**Context.** The fusing stage constructors (`%xmap`/`%xfilter`/`%xremove`/`%xkeep`)
have existed since ADR-111 as *private* plumbing behind `lmap`/`lfilter`/`lkeep`/
`lremove`. That covered the built-in stages and nothing else: a user who wanted a
stage of their own — `take-while` as a stage, a stateful de-duplicator, a windower —
had no way to write one and no way to run one. Policy that Brood could express was
locked inside the prelude's private namespace.

**Decision.** Publish the contract and the entry point: `transduce` runs a stage stack
over any collection (`(transduce xform rf init coll)`), and `xmap`/`xfilter`/
`xremove`/`xkeep` are the built-in stages under public names. The contract is two
sentences — a **reducing function** is `(acc x) -> acc`; a **transducer** is
`(rf) -> rf'` — so a custom stage is a plain `fn` with no protocol to implement and no
registration.

Composition order is documented explicitly because it is the one confusing part:
stages compose **left to right in data-flow order** under `comp`, the reverse of
ordinary function composition, because each stage wraps the *next* one's reducer.

**Not done:** no `eduction`, no early termination (`reduced`), no stateful-stage
lifecycle (init/completion arities). Clojure's full protocol has three arities per
stage; Brood's has one, which is all `fold` needs. Early termination is the first
thing a real use will want (`take`), and it wants a `reduced` sentinel threaded
through `fold` — a change to the hottest function in the library, deliberately not
bundled here (ADR-011).

**Consequences.** `lmap`/`lfilter`/… are unchanged and remain the ergonomic form —
they now simply share a documented vocabulary with user stages. Purely additive: five
new prelude functions, no kernel change.

## ADR-162 — Retire the `lambda` alias: `fn` is the only spelling

**Context.** ADR-098 decided to drop the `lambda`/`let*` aliases. `let*` went;
`lambda` stayed, and ADR-108 then recorded the opposite — that both are exact
synonyms canonicalised at macroexpand. Meanwhile `docs/language.md` claimed for
months that *"the `lambda` synonym was removed — one spelling each"*, and the
2026-07-26 downstream doc sweep removed mentions of it **on that basis**. So the
language accepted a spelling its own reference said didn't exist, two ADRs
contradicted each other, and the tooling highlighted it as a special form.

**Decision.** Remove it. `fn` is the only spelling. The evidence: zero uses in
`std/`, zero in all 12 sibling projects, and zero in `tests/` outside the one file
that existed to test the alias itself. ADR-154 established the governing principle
("one spelling each") and removed `car`/`cdr` on exactly this basis; ADR-108's
"harmless alias" rationale doesn't survive it.

Removed from: the evaluator's `SPECIAL_SPELLINGS`, the macroexpand canonicalisation,
the checker's `is_fn_head` / `is_syntactic_keyword` / sig-head list, and the tooling
`SPECIAL_FORMS` (so editors stop colouring it). `tests/lambda_test.blsp` is deleted.
The `kw::LAMBDA` constant survives for one purpose: the unbound-symbol hint, which
now says *"Brood spells `lambda` as `fn`"* — so the mistake is a one-line fix rather
than a bare "unbound symbol".

One diagnostic changed with it: an inline callback of the wrong arity was described
as **"the lambda"**, which named a form the language no longer has. It now reads
"the fn".

**Consequences.** A breaking change with no known caller. Quoted data is unaffected
(`'(lambda (x) x)` was always left alone — it is data, not a form). The evaluator's
special-form table is one entry smaller, and the macroexpand hot path loses a
per-combination symbol comparison.

## ADR-163 — The convention questions the syntax review raised, settled

The 2026-07-26 syntax review filed nine surface items. Three became code (ADR-160/161/
162). The rest are **convention questions**, where the valuable output is a decision
plus a written rule, not a change — and where changing would cost a wide breaking
sweep for taste. Recorded together so they stop being re-litigated.

**Named arguments: no `&key`; a trailing options map is the convention.** Spec §7.4
designed `&key (width 80)` and never built it. It stays unbuilt. `{:keys [a b] :or
{…}}` destructuring already gives the call-site readability (`(make-window {:width
100 :title "x"})`), composes with `merge` for defaults — which `&key` does not — and
costs no new parameter-list grammar, no new arity rules, and no interaction with
`&optional`/`&`/patterns (a matrix that already has three documented "these don't
combine" rules). The sweep the review anticipated turned out to be empty: no function
in `std/` takes more than two `&optional` parameters, so there is nothing to migrate.
The rule is now in `brood-for-claude.md`: three or more optional parameters → take an
options map.

**`fold` and `reduce` both stay, with the relationship documented.** `reduce` is the
2-or-3-arg surface; `fold` is the strict 3-arg form it wraps, and is what all of
`std/` folds with. Renaming `fold` to `%fold` would be a ~200-site mechanical rename
of an *ambiguous* name — and the ADR-154 downstream sweep documented exactly how that
goes wrong (a `fold` parameter and a `fold` call are the same shape in a Lisp, so
call-position heuristics rename binders). Not worth it: `docs/language.md` now states
that `reduce` dispatches to `fold` and that `fold` is the strict-arity primitive.

**`cond`'s bare `else` stays** (ADR-004). `:else`, `true`, and `42` all catch by plain
truthiness, so blessing the symbol `else` does add a reserved word for no capability —
but it is used throughout `std/` and every sibling project, reads better than `:else`
at the end of a long `cond`, and removing it would break every downstream `cond` in
the workspace to delete one prelude line. Decision: keep, and document *why* it is
special-cased rather than leaving readers to wonder.

**`!` keeps its three meanings, documented.** `sig!` (enforced), `set-load-path!` /
`clipboard-set!` (root/OS-state setters), `(! pid msg)` (the `proc/gen` cast). In a
language with no data mutation the Scheme/Clojure "warns of mutation" reading is
vacuous, so `!` is free to be per-context — but that must be *stated*, because an LLM
or a newcomer will otherwise infer a rule that isn't there. `brood-for-claude.md` and
the writing-brood skill now say: don't add a trailing `!` to a name of your own, and
here is what the existing ones mean. Renaming `(! …)` to `cast` was considered and
rejected: it is Erlang's spelling, it is the API hatch/`proc/gen` users already know,
and the sweep would touch every gen-server client in the workspace.

**Mixed naming lineage: write the rule down, don't rename.** `partition` (Clojure)
sits beside `chunk-every`/`chunk-by` (Elixir), `enumerate` (Python), `scan`
(Haskell), `&optional` (CL), `!`/`spawn` (Erlang), `letrec` (Scheme). Aligning on
`partition`/`partition-all`/`partition-by` would be the tidiest single change, and it
is still a breaking rename of three functions across the workspace to buy
guessability that the docs can supply instead. Decision: the house rule is **"the
best name for the job, from whichever language named it best"**, now stated in
`brood-for-claude.md` — with the corollary that `apropos`/`doc-search` exist precisely
because a name can't always be guessed.

**Failure convention: `throw` for bugs, tagged values for expected alternatives.**
`std/` mixes both (19 `[:ok …]`/`[:error …]` sites alongside `throw`), which left
every caller to check which a given function does. The rule, now documented: **throw**
when the caller almost certainly cannot continue (a type error, a missing file, a
protocol violation — anything a bug would cause); return a **tagged vector** when
failure is an ordinary outcome the caller is expected to branch on (a parse that may
fail on user input, a lookup that may miss, a timeout). `error-message` remains the
shape-agnostic accessor for the `catch` side.

**Reader gaps: documented, not changed.** `inf`/`nan`/`-inf` are reader float
literals, so those three tokens can never be symbols — now stated in the data-types
table, which previously implied numeric intent requires a digit. `1/2` reads as a
*symbol* (and `/` is the namespace separator, so it looks like namespace `1`, name
`2`) — left alone: rejecting it would mean special-casing a shape no one writes on
purpose, and it is already covered by the "no ratios" note. `#|` still produces
"unterminated `|…|` bar-quoted symbol"; a dedicated "Brood has no block comments"
message is a nicety, filed in ROADMAP rather than done.

**`sig` placement stays below the definition; inline signatures need their own ADR.**
The review's fairest criticism — a function's name, parameters, and types live in two
forms with an ordering constraint that only bites under `BROOD_CONTRACTS=1` — is real,
and the fix (`(defn f ((x int) -> int) …)`) is a change to `defn`, the checker's
`sig_of`, `defrecord`'s emitted sigs, `sig!`'s wrapping, and every `sig` in `std/`.
That is an ADR-082 revision, not a tweak. Deferred with the reasoning recorded, not
dismissed.

## ADR-164 — `get`/`nth` diagnostics: an error must name the operation the caller wrote

**Context.** `docs/language.md` promises that type errors are self-identifying —
*"they name the operation, the type it wanted, and the tag + printed form of what
actually arrived"*. `get` is the most-called accessor in the language (~4,800 sites
across the workspace) and broke that promise in **four** of its five failure modes,
because its non-map path fell through to `nth`, whose integer arithmetic raised first:

| Expression | Before |
|---|---|
| `(get deps :name)` — list | `-: expected number, got keyword (:name)` |
| `(get [1 2] :name)` — vector | `<=: expected number, got keyword (:name)` |
| `(get 5 :name)` — non-collection | `empty?: expected collection, got int (5)` |
| `(get "str" :name)` — string | **`nil`**, silently |
| `(get nil :name)` | `nil` (correct) |

Three of those name `-`, `<=`, `empty?` — internals of `nth`/`nth-list` that the
caller never wrote, and that give no clue what the actual mistake was. The fourth is
worse than any error: `(:name s)` on a string that should have been a map produced a
plausible-looking absent value, so the mistake surfaced arbitrarily far away.

This surfaced while designing callable keywords (`(:k m)`), which would inherit the
behaviour wholesale — the most common misuse of that feature, `(:name deps)` where
`deps` is a *list* of maps, is exactly the first row of the table. So it is a
prerequisite for that feature, and a defect on its own.

**Decision.** `get` and `nth` check the shapes they cannot answer and raise errors
that name themselves:

- **A non-integer key on an integer-indexed collection** (vector, list, string,
  bytes) → `get: expected an integer index, got keyword (:name) — a vector, list,
  string or bytes is indexed by position, not by key. Did you mean one element of it,
  or `get-in` for a nested key?` The hint matters: the two real intentions behind
  that mistake are "one element" and "a nested key".
- **A non-collection** → `get: expected a collection (map, set, vector, list, string
  or bytes), got int (5)`.
- **`nth` with a non-integer index** → names `nth` instead of leaking `>=`/`-`.
- **`nth` on a non-collection** → names `nth` instead of leaking `empty?`.

**The `default` argument does not suppress these.** A default means "the key is
absent", not "the key is the wrong type" — so `(get [1 2] :name :dflt)` raises. This
is the distinction that makes a default safe to use: it can't quietly absorb a bug.

**One semantic change, deliberate:** `(get "str" :k)` was `nil` and is now an error.
A test asserted the old behaviour, on the reasoning that "strings are traversable but
not key-value maps" — true, and precisely why a keyword key is a *mistake* there
rather than a miss. The test now pins the error, with the old expectation and the
reason for the flip recorded in place.

**Measured cost, and a rejected implementation.** Diagnostics on the hottest accessor
in the language deserve a number, so (2M calls, release + JIT):

| | Before | After |
|---|---|---|
| `(get m :k)` — map, keyword key | 735 ms | 739 ms (noise) |
| `(get v i)` — vector, integer key | 1130 ms | 1432 ms (~27%) |
| `nth` standalone | 11 ms | 11 ms (unchanged) |

A first implementation factored the kind test into a `get--indexed?` helper — four
type checks behind a call — and measured **1130 → 2050 ms (~1.8×)**. It was rewritten
to keep the original branch order: map, set, then the cheap `nil?`/`string?` prim
checks, then **one** fallthrough to `nth` for every integer-indexed kind. The comment
in `std/prelude.blsp` records this so the ordering isn't "tidied" back.

The residual 27% was accepted on evidence, not vibes: the keyword-key form is **4,796
call sites** in the workspace and is unaffected; the integer-key form is **124**, and
positional access in hot code goes through `nth`/`vector-ref` (720 sites), whose wall
time is unchanged. About 115 ms of the delta is `nth`'s own entry check appearing
through this call path — an inlining-boundary effect, since `nth` measured standalone
is identical either way.

**One wart, kept knowingly.** `(get 5 0)` — a non-collection with an *integer* key —
reports as `nth: expected a collection to index, got int (5)`. Catching it in `get`
would require the collection test on the fast path, which is the 1.8× regression
above. The message is accurate (positional indexing does delegate to `nth`), names a
real op, and states the actual problem; it just names the callee rather than the
caller. Recorded rather than hidden.

## ADR-165 — A keyword is callable as an accessor; nothing else data-like is

**Context.** `(map :name people)` — an accessor passed to a higher-order op — had no
spelling. Every such call had to invent a throwaway binder purely to discard it:
`(map (fn (p) (get p :name)) people)`. Counted across the workspace: **67 sites**
(59 `map`, 4 `filter`, 3 `sort-by`, 1 `keep`).

That number is the whole case, and it is worth stating what the case is *not*. An
earlier draft justified this with the 4,796 `(get x :keyword)` sites in the workspace;
checking them showed almost none would be converted — `(get m :name)` puts the
subject first and reads better than `(:name m)`. The 81 nested `(get (get x :a) :b)`
sites want `get-in` (already used 166×), not keywords. And the threading-chain
argument — `(-> m (get :a) (get :b))` — had **zero** sites. So: 67, of one shape.

**Why now rather than after 1.0.** It is additive in the strict sense (today `(:k m)`
raises `cannot call non-function`, so no valid program changes meaning) but it is
*idiom-shaping*: ship 1.0 without it and `std/`, every sibling project and every doc
example is written the older way, so adoption becomes a churn wave at the moment
stability was promised. That is the test `docs/roadmap-for-v1.md` applies, and this is
the only item that failed it for that reason.

**Decision.** A keyword applied to 1 or 2 arguments is an accessor:
`(:name p)` ≡ `(get p :name)`, `(:name p "unknown")` ≡ the 3-argument `get`.
Receivers mirror `get` exactly — a **map** by key, a **set** by membership (yielding
the element, the ADR-156 rule), `nil` as empty — and anything else is a type error
**naming the keyword**, which is strictly more specific than `get`'s message since the
key is already in it. An integer-indexed collection is deliberately in that last
group: `(:name deps)` where `deps` is a *list* of maps is the single most likely
misuse, and ADR-164 had just made the same shape loud for `get`.

Implemented in **`eval::apply`**, the one function both engines funnel non-closure
callees through — *not* as a compile-time rewrite of `(:k m)`. That is what makes a
keyword a first-class value the higher-order ops can take (`(map :name xs)`,
`(apply :name (list p))`, `(let (f :name) (f p))`); a syntactic rewrite would have
covered only the head position and missed the entire point.

**Scope: keywords only.** Maps, vectors and sets stay non-callable, and keep their
existing hints. A callable map is a second spelling of `get` (against ADR-154's "one
spelling each"); a callable vector or set answers by index-or-membership, the
ambiguity ADR-156 refused for `contains?`. One blessed exception to "the head of a
form is a function" is a rule you can state in a sentence; four is a different
language.

**The performance finding is NOT part of the justification.** `(:name p)` measures
130 ms/1M against `get`'s 393 ms, but that is not a property of the syntax — it is the
Brood/Rust boundary, and the breakdown says so:

| | with JIT | no JIT |
|---|---|---|
| `map-get` kernel op, called directly | 107 ms | 209 ms |
| + a single-arity Brood wrapper | 231 ms | 274 ms |
| + the four-branch `cond` type dispatch | 369 ms | 366 ms |
| the real multi-arity `get` | 393 ms | 374 ms |
| `(:name p)` | 130 ms | 197 ms |

The accessor is fast because it reaches `map_get` through one Rust arm, skipping a
Brood closure call (+124 ms) and a `cond` chain (+138 ms). Implemented *in Brood* it
would measure like `get`. Claiming the speed as a benefit would be the exact move
`CLAUDE.md` warns against — using Rust to make a slow Brood function fast **hides**
the gap instead of fixing it. The feature stands on the 67 sites.

What the measurement *did* surface belongs in ROADMAP: `get` pays ~2.7× the kernel
op's cost for one closure call plus a four-branch `cond`, and **the JIT closes none of
it** (393 with, 374 without). That is the same shape as the variadic-`+` finding that
motivated multi-arity dispatch — a language-level call/type-dispatch gap, not a `get`
defect.

**Consequences.**
- The checker's `relax_param_for_arg` now admits a keyword wherever a callable is
  expected (an arrow parameter, or `apply`'s `fn | native`). The lattice cannot say
  "keyword, which behaves as `(map -> any)`" — the tags are disjoint bits — so
  without this the most-motivating call would draw a false warning.
- The keyword arm of `not_a_function_error` is gone (it used to advise `(get m :k)`);
  map/set/vector keep theirs. `tests/syntax_finalization_test.blsp` flipped from
  asserting the hint to asserting the accessor, with the reason recorded in place.
- `(:k)` and `(:k a b c)` get a keyword-specific arity message naming both valid
  forms — they are the two ways this gets typo'd.
- No change to `get`, `get-in`, or any existing call: purely additive at the call
  protocol, and the only value kind whose callability changes is `Keyword`.

## ADR-166 — Reserved names: the language's own functions cannot be redefined

**Context.** Every global was rebindable, including `get`, `+`, `first` and `when`.
`docs/language.md` advertised it — *"Because it's ordinary Brood, any of it can be
redefined at runtime"* — and the prelude's own docs treated it as a feature. But the
feature it was justified by, ADR-013 Erlang-style hot reload, is about **your** code:
you redefine your editor's commands while the editor runs. Nothing in that story
requires patching `get`.

Meanwhile the cost was real: a monkey-patchable standard library is the Ruby/JS
footgun (a library can't rely on `map` meaning `map`), and — larger — it blocks
optimisation. Every prelude call has to be late-bound because any global *might* be
rebound, which is precisely the overhead measured in ADR-165: against `map-get`'s
107 ms/1M, a single-arity Brood wrapper costs +124 ms and `get`'s type-dispatch a
further +138 ms, with the JIT closing none of it.

**Decision — the seal boundary is the binary boundary.** If it shipped inside the
`brood` binary it is **reserved**: the prelude's functions and macros, every Rust
builtin, and every function an embedded std module defines. If you or a package
author wrote it, it is **yours** — fully redefinable, so hot reload is untouched.
That line is one sentence, and it is discoverable: reserved ⇔ it came with Brood.

A `def` of a reserved name raises, naming the symbol and the three ways forward — a
different name, a local `let` shadow (still legal: that binds a local, it is not a
redefinition), or a `(defmodule …)`, where `(defn get …)` defines `your/mod/get` and
is yours.

**Two exemptions, and both are part of the rule rather than caveats on it.**

1. **Only functions, not names.** The prelude's *data* globals — `*features*`,
   `*load-path*`, `*module-docs*` — stay rebindable, because prelude functions rebind
   them with `def` at runtime; that is Brood's one mutation, and reserving them breaks
   `require`/`provide`/`defmodule` outright (found immediately: the first
   implementation sealed them and `(require 'set)` died on `*features-loading*`). The
   rule is "a shipped **function** can't be redefined", which is exactly what was
   asked for.
2. **A dynamic variable is never reserved, whatever it holds.** `defdyn` (and
   `%declare-dynamic`) *declares a name rebindable* — that is the entire meaning of
   the declaration — so reserving one would contradict it. This is not a technicality:
   an output **port is a function** (`(fn (s) …)`), so `*out*`/`*err*` fell under the
   function-valued test, which would have left only the scoped `binding` form and made
   a *permanent* output redirect impossible. The check sits next to the reserved-set
   probe rather than in the seed filter, so it also covers a `defdyn` inside an
   embedded module and a name declared dynamic after seed time.

**Why Erlang's `unstick` isn't copied.** The BEAM marks OTP's modules *sticky* and
keeps `code:unstick_mod/1`, and the precedent was examined rather than assumed. Its
three reasons don't transfer: sticky is anti-*accident* (it stops a stray `lists.erl`
clobbering OTP) rather than a rule; the hatch exists for operational hot-patching of a
system that can't be restarted, whereas Brood's std ships *inside the binary* so
changing it means rebuilding anyway; and instrumentation on the BEAM is a **VM
facility** (`erlang:trace/3`, `dbg`), not code replacement — Brood likewise has
`profile-start`, `system-monitor`, telemetry and the `BROOD_*` trace flags. The
"wrap `get` to trace map access" argument was reaching for the wrong tool.

**Why now, before the freeze.** The decision is asymmetric: **relaxing** a restriction
later is backward-compatible — every program that worked still works — while
**adding** one breaks whoever monkey-patched. Of the two possible mistakes, sealing is
the recoverable one, so it is the right way to enter a language freeze. It also lands
*after* ADR-158 gave protocols, which is the sanctioned extension point that replaces
patching (Elixir's order too).

**Implementation.** The reserved set lives in `RuntimeCode` beside the globals table,
so every inner process shares one set, and it is probed only when a global `def` runs.
Seeded at runtime-seed time with every shipped binding whose value is a function,
macro or builtin. Embedded std modules extend it *as they load*: `require` routes
baked-in source through a new `%load-module-source` primitive that holds a
per-process exemption — so the module's own `def`s are allowed and become reserved —
and releases it **even when the load throws**, since a leaked exemption would silently
un-reserve the language. A project file off `*load-path*` goes through the ordinary
`load`, so a package's names are never reserved. One gate, in the evaluator's `def`
arm, because the VM defers `def` forms to the evaluator — it therefore covers both
engines, `load`, `eval` and the REPL. It sits *above* the arity-change diagnostic so a
refused `def` doesn't first print a `[reload]` warning about a rebinding that never
happens.

**Consequences.**
- **Blast radius in user code: two lines.** Across brood + 12 sibling projects the
  only genuine root-level clobbers were bench fixtures — `(def comp (table))` in a
  sieve (the prelude's function composition) and `(def dec …)` for decoded bytes (the
  prelude's decrement). Both were *accidental collisions the rule now catches*, which
  is the argument for it in miniature. Everything else that looked like a
  redefinition — `path/join`, `http/response/capitalize`, `proc/agent/get`,
  `mitch/update` — is module-scoped and never touched the root binding. The namespace
  system (ADR-065) had already done the work.
- **Blast radius in the kernel's own test suite: six tests**, and they were the
  interesting part. Three were fixture collisions (`inc`, `keep`, `dec`). Three
  existed *to prove redefinition of a shipped function works* across the VM/JIT —
  `prim1_guard_sees_redefinition`, `redefining_an_operator_after_tiering_is_honored`,
  `type_of_prim_redef_falls_back`. Those properties are now unreachable, so each was
  retargeted: two assert the refusal, and a **new** test
  (`redefining_a_user_fn_after_tiering_is_honored`) pins the property that still
  matters — a JIT'd caller honouring the redefinition of *your* function. Without it
  the epoch guard would have been left with no coverage at all, since every name the
  old tests used is now reserved. That is the live-editing case, and it deserved a
  test of its own regardless.
- **The optimisation this unlocks** is the point, and is deliberately *not* bundled
  here: with a reserved binding immutable, the compiler can bind it **early** and the
  JIT can inline it without a staleness guard. The `PrimOp1` epoch guard is already
  unreachable for its original purpose (every prim it covers is reserved). That is
  Erlang's local-vs-remote-call optimisation arriving by the same insight, and it is
  the highest-leverage perf item in the library (`get` alone is ~4,800 call sites).
  Filed in ROADMAP.
- The checker can likewise stop treating reserved globals as `dynamic()` and give them
  precise types — a sharpening of every warning that flows through a prelude call.
  Also filed, not done.
- Coverage in `tests/reserved_names_test.blsp` (15 cases): prelude functions,
  builtins, macros and embedded-module functions all refused; the error naming the
  symbol *and* all three escapes; user globals and `defn`s still redefinable including
  an arity change; the prelude's data registries still rebindable (with `require`
  exercised to prove it); `let` shadowing and module-scoped definition both still
  legal; the reserved set shared across processes (a spawned process refuses too, yet
  can still redefine its own names); and an `:isolated` block for the dynamic-var
  exemption — `*out*` permanently redirected, `print` following it, and the scoped
  `with-out-str` form unaffected (isolated because rebinding a global port would
  otherwise swallow every concurrent test's output).

## ADR-167 — Keyword accessors are typed, not just callable

**Context.** ADR-165 made a keyword callable. It taught the checker one thing — that a
keyword is acceptable where a *callable* is expected, so `(map :name people)` doesn't
warn — and stopped there. That left a hole, found by asking the obvious follow-up
question rather than assuming: **what does the checker know about `(:name x)` itself?**
Nothing. A keyword head is not a `Value::Sym`, so the form bypassed every sig, arity
and result-type path in `check_into_inner`:

| Form | `get` spelling | keyword spelling (before) |
|---|---|---|
| receiver can't be keyed | *(also unchecked)* | **no warning** |
| wrong arity | arity error | **no warning** |
| result type from a typed record field | `nil \| int` — flagged | **no type at all** |

The middle column matters: `(string-length (get (pt 1 2) :x))` was already caught,
while the identical `(string-length (:x (pt 1 2)))` was not. Two spellings of one
operation typed differently, which is the worst outcome for a feature whose whole
justification was that it reads better.

**Decision — check the form, and infer through it.**

1. **Receiver kind.** A keyword accessor's argument must be keyable — a map (by key),
   a set (by membership) or `nil` (empty), exactly the receivers `apply_keyword`
   accepts. Warn only when the argument's type is *provably* none of those
   (`is_disjoint` under the gradual reading), so an inferred or redefinable value never
   misfires. This is the check that catches ADR-165's own stated worst case: `(:name
   deps)` where `deps` is a *list* of maps.
2. **Arity.** `(:k)` and `(:k a b c)` are flagged with the same wording the runtime
   uses, so write-time and run-time agree.
3. **Result type.** A keyword head now runs the *same* record-field / `map<K,V>` rule
   that `(get m :k)` has had since ADR-115: a declared field's type wins, `V | nil`
   for a known map, and an unknown key on a record falls through — records are open,
   so an undeclared key's type is genuinely unknown rather than an error. The two
   spellings are now pinned to produce the same warning count on the same program.

**Consequences.**
- `(:x p)` participates in the advisory checker exactly as `get` does, including
  flowing a record field's declared type into a downstream misuse.
- No new false-positive surface: both new warnings fire only on a *provable* mismatch,
  and the arity check is structural.
- Three tests in `types/check/tests.rs` pin the receiver check (including the four
  silent cases — map, set, nil, and an unknown parameter), the arity check, and the
  result-type equivalence with `get`.
- **`get`'s own receiver check, done in the same ADR** after the "wider change"
  excuse turned out to be wrong. The claim was that one shared signature covers every
  collection kind so tightening it is entangled. In fact `get` had **no signature at
  all**: it is multi-arity and `infer_sig` bails on multi-arm closures, so its domain
  was simply unconstrained — which is why `(count 5)` and `(first 5)` were caught and
  `(get 5 :k)` was not. Two changes, both small:
  1. A curated sig with the same `countable` domain `count` uses (every kind `get` can
     key or index), result `any`, variadic `default` tail.
  2. The **relationship** a flat signature genuinely can't express: a *literal
     keyword* key can only address a keyed receiver, so `(get deps :name)` on a list
     of maps is a provable mistake — the write-time half of ADR-164's runtime error,
     and now symmetric with the keyword spelling.

  Verified false-positive-free the only way that counts: `nest check` stays at zero
  warnings across `std/` + `tests/`, and across all 11 sibling projects — roughly
  5,000 `get` call sites. A computed key and an unknown receiver both stay silent.

## ADR-168 — `ability` is the one value-dispatch seam; `defprotocol`/`defimpl` retired

**Status:** accepted. Supersedes ADR-158's *value dispatch*; ADR-158's
`defbehaviour` (the module-as-implementor contract) is untouched and stays in
`std/protocol.blsp`.

**Context.** ADR-158 shipped `defprotocol`/`defimpl` — open generic functions
dispatching on `(type-of first-arg)`. The design note
[`protocol-dispatch-design.md`](protocol-dispatch-design.md) then measured why
nobody used them: in the whole of `std`, exactly **one** module did. `type-of`
distinguishes only ~13 built-in kinds, and every *application* type is a
structural map (ADR-130: `defrecord` is sugar over a plain map, so every record
reports `:map`). So a protocol could tell an `:int` from a `:string` but **could
not tell one record from another** — which is the single most common reason to
reach for a protocol at all. Speed was never the blocker; **dispatch identity for
user types** was.

That left two seams for one idea — `protocol` (dispatch on a value) and
`behaviour` (a module satisfies a contract) — where the interesting half couldn't
express the interesting case.

**Decision.** One concept, **`ability`** (`std/ability.blsp`): open generic
functions dispatching on the first argument's **identity**.

1. **Identity, not kind.** `identity-of` returns a `defrecord` value's **nominal
   id** — a `:module/name` keyword baked in at macro-expansion via `(current-ns)`
   — else the value's `type-of` kind. So two record shapes defined in one module
   dispatch apart, and built-in kinds keep working with `:default` as the
   fallback.
2. **ADR-130 survives.** A record is still a structural map: `type-of` is `:map`,
   and `get`/`assoc`/`=` are unchanged. The identity is a *dispatch-only* notion
   layered on top, held in a reserved `:__id__` field.
3. **The `:type`-field axis is permanently rejected.** Sniffing a `:type` key
   would silently reroute *any* map that happens to carry one — the exact
   implicitness ADR-011 rejects. A `defrecord` identity is explicit and
   construction-time, which is the same power made safe. This closes the
   pre-1.0 breaking question in
   [`roadmap-for-v1.md`](roadmap-for-v1.md) §3.
4. **Registry, not structural satisfaction.** The design note leaned Go-style
   (satisfaction *derived* from op resolution, coherent because nothing is
   registered). We took the **registry** route instead: structural satisfaction
   loses retroactive extension of a *foreign* record, and "make my type work with
   their operation" is the flagship use case. Coherence is bought back
   *explicitly* — every impl is tagged with its registering `current-ns`, so a
   **cross-module** re-registration is a loud warning (last wins) while a
   same-module one is ordinary hot reload and stays silent.
5. **Drivers are values.** Because dispatch is on the first argument, "swap the
   backend" is passing a different record — no config indirection, no module-atom
   dispatch. Ambient selection becomes a one-line wrapper, not a second
   mechanism.
6. **Sealed abilities.** `:sealed [id …]` records a closed member set; the checker
   then demands a *direct* impl of every op for every member (a `:default` doesn't
   count). Runtime dispatch is unaffected — sealing is a contract.
7. **`defprotocol`/`defimpl` are removed**, not deprecated (greenfield: no
   compatibility shims). The `deftype`/`reify` runtime hint now points at
   `defability`/`impl`, and reaching for `defprotocol` raises an unbound-symbol
   error carrying that hint.
8. **`defbehaviour` stays.** When the implementor is a *namespace* rather than a
   value — a live view a router calls by name — there is no dispatch value, and
   `(:implements …)` + a checker pass is the right shape. That is genuinely a
   different problem, so it keeps its own (much smaller) module.

**Checker support.** `types/check/protocol.rs` validates each `impl` against its
ability's declared ops (missing op, arity disagreement, undeclared op), enforces
sealed exhaustiveness, and warns at a **call site** when an op is applied to an
argument of statically-known identity for which no impl and no `:default` exists.
That last check is inference-driven, so a record-typed *variable* is caught too —
enabled by `defrecord` emitting a **map-literal** body plus a `sig`, so the
record shape (carrying the `:__id__` keyword literal) flows through a `let`. Kept
sound rather than aggressive: an op fn is recognised only by its exact def symbol,
an identity is taken only when certain, and the impl set unions the file's own
`register-impl` forms with the live `ability/*impls*` registry.

**The identity leak, resolved pragmatically.** The obvious objection to a visible
`:__id__` field is that it leaks into structural views. Verifying the constraints
changed the plan rather than confirming it: the `Value` layout is JIT-pinned and
map ops match `Value::Map` with catch-alls, so a `Record` variant is a pervasive,
risky change — and, the key realization, a record being **`≠` a bare map is
*correct*** (Elixir-struct semantics), and a record *printing* with its id is
informative. So we do **not** want to hide the id from `=`. The one genuinely
harmful leak — an internal key reaching external JSON — is fixed in std
(`json-encode` omits `:__id__`), and `record?`/`record-id`/`fields` mean nothing
outside `ability` touches the field. The residue is cosmetic (`keys`/`count`
include the id; use `fields`), deferred behind a possible future hidden slot.

**Consequences.**
- One seam to learn and document instead of two-and-a-half. The polymorphism
  section of [`language.md`](language.md) is now a single story.
- ADR-011 is honoured on stricter terms than the rejected axis would have allowed:
  the identity is opt-in per *definition*, not inferred per *value*.
- Coherence is explicit rather than guaranteed. A cross-module clash is possible;
  it is just never silent.
- Retroactive extension of a foreign record works (register from any module), the
  property structural satisfaction would have cost us.
- **Still open:** return-type dispatch (needs bidirectional inference) and
  monomorphization (static resolution where the identity is known — the runtime
  win). Both additive, both post-1.0.
- **Known warts:** `impl` requires the dispatch id written as `identity-of`
  produces it — qualified (`geometry/circle`) — while `defability`'s `:sealed`
  accepts a bare name and qualifies it; a bare `impl` id misregisters silently
  (KI-15). The LSP still matches the retired `defprotocol`/`defimpl` and has not
  been migrated (KI-16).

**References.** ADR-158 (the protocol facility this supersedes), ADR-130 (records
are structural — preserved), ADR-011 (defer power features / no implicit capture),
ADR-006 (all of `ability` is Brood; zero new Rust primitives),
[`protocol-dispatch-design.md`](protocol-dispatch-design.md) (the full design
record: the measurements, the language survey, and the space explored),
[`roadmap-for-v1.md`](roadmap-for-v1.md) §3 (the pre-freeze question this closes).

## ADR-169 — The reader reserves `#…` dispatch forms and digit-led non-number tokens

**Context.** The reader is the one surface where staying silent is a *permanent*
commitment. Two token spaces were being spent by accident:

- **`#…`.** `#` was an ordinary atom character that fell through to `read_atom`, so
  any unrecognised `#…` interned as a **symbol** — `#foo` was a legal name, and
  `#|a comment|#` (a Scheme/CL block comment) read as the bar-quoted symbol
  `|#\|a comment\|#|`. Only `#{…}` (set) and `#b"…"` (bytes) were real forms.
- **Digit-led tokens.** The atom classifier only rejected tokens made *entirely* of
  number characters (`1e`, `1.2.3`), so anything with a stray letter or punctuation
  leaked through to a symbol — `0x1F`, `1/2`, `1_000`, `1N`, `1+`, `12-34` all
  became identifiers, surfacing far away as "unbound symbol" instead of at the typo.

Both are the same latent hazard: any `#` literal or numeric syntax Brood might add
after 1.0 would be **taking a token that had been a valid name** — a breaking change.
[`roadmap-for-v1.md`](roadmap-for-v1.md) §2 named the one concrete instance (`1/2`,
which has to be rejected now if a ratio type is ever wanted) and asked for the whole
space to be settled before the freeze. This ADR is that settlement.

**Decision.** State the rule on the token's *first character* instead of all of them,
and reject the whole space rather than the enumerated cases:

1. **`#` is a dispatch character, not an atom character.** `#{…}` and `#b"…"` are the
   only two `#` forms. Every other `#…` — including a bare trailing `#` on its own and
   `#|…|#` — is a **reader error**, never a symbol. (A trailing `#` *inside* a token,
   `x#`, is untouched — that is quasiquote auto-gensym, and it is load-bearing.)
2. **A token that leads with a digit — or a sign/dot immediately followed by a digit —
   must be a number.** If it is not one Brood has, it is a reader error, never a
   symbol. Names with a sign or dot but *no digit behind it* (`+`, `-`, `...`,
   `.foo`, `foo.`, `--5`) are not digit-led and stay symbols.

Every rejection carries a teaching hint (the LLM-native error style,
[`llm-native.md`](llm-native.md)): the Scheme/CL/Clojure form it *looks* like, named
alongside the Brood idiom — `#|…|#` → `;`/`(comment …)`, `1/2` → `(/ 1 2)` or a
`0.5M` decimal, `0x1F` → `(string->number "1F" 16)`, `1_000` → `1000`, `1N` → plain
`1` (integers already widen to bignum). The hint for a reserved-numeric token lives in
`syntax/atom.rs` (`reserved_numeric_hint`) so the reader and the tooling CST explain it
identically (the ADR-025 one-definition rule), and `AtomKind::ReservedNumeric` maps to
`NodeKind::Error` so the LSP flags it like a malformed literal rather than offering to
rename it.

**Why this is a freeze item, and why rejecting the whole space costs nothing.** The
asymmetry is the same one ADR-166 turned on: *relaxing* a reservation later is
backward-compatible; *adding* one is not. So a language freeze has to decide the
reservations first — "later" is exactly what a freeze gives up. And the price is nil,
because none of these tokens is a real name today: `inc`/`dec` are the Brood spelling
of `1+`/`1-`, no in-tree or sibling program names anything `#foo` or `0x1F`, and
Clojure rejects the same tokens. What is bought is that every future numeric syntax
(ratios, radix literals, digit separators, a bigint suffix to pair with `1M`) and
every future `#` literal stays **purely additive** after 1.0, and diagnostics land at
the mistake instead of a distant unbound-symbol.

**On ratios specifically (the §2 open question).** Deciding to *reserve* `1/2` is not
deciding to *add* ratios — it keeps the option open at zero cost. The freeze list
records ratios as a documented **"not in 1.0"**: `(/ 1 2)` yields a float and `0.5M`
an exact decimal, and `/` is also the namespace separator (the two never collide — a
digit-led token is a number, `mod/name` is not). Reserving the token means a post-1.0
ratio type would be additive rather than breaking.

**Consequences.**
- The printer needed no change. `printer::symbol_needs_bars` already asks
  `atom::classify`, so the moment `1+`/`0x1F` stopped classifying as `Symbol` the
  printer began bar-quoting them — `(symbol "1+")` still round-trips, now as `|1+|`.
  Another dividend of the one-definition rule.
- Deliberately unaffected: `1M`/`1.0M` decimals, `.5`, `5.`, `1e10`, and the three
  reader-literal floats `inf`/`nan`/`-inf` (already irreversible — ADR-062-era; those
  bare tokens can never be names).
- Closes the last open pre-freeze **language-surface** decision. The remaining v1
  paperwork is ratifying the freeze list itself as its own ADR.

**References.** [`roadmap-for-v1.md`](roadmap-for-v1.md) §2 (the pre-freeze question
this closes), ADR-166 (reserved names — the same relax-is-safe/add-is-breaking
asymmetry, applied to bindings instead of reader syntax), ADR-025 (the CST/reader
one-definition rule the shared hint honours), ADR-011 (defer power features — ratios
stay reservable, not shipped), [`llm-native.md`](llm-native.md) (the teaching-hint
convention). Tests: `tests/reader_hints_test.blsp`, `tests/reader_malformed_test.blsp`,
`tests/malformed_test.blsp`.

## ADR-170 — The 1.0 freeze list: what Brood permanently is not

**Context.** A 1.0 is a promise that the language *surface* stops moving. That promise
is only credible if it is paired with an explicit statement of what the language
**refuses** — the features a newcomer from Clojure/Scheme/CL will reach for and not
find. Every one of those refusals had already been decided, in its own ADR, over the
course of the language's design; but they were scattered, so the "why not X?" question
kept being re-litigated. This ADR gathers them into one ratified list, the companion to
[`roadmap-for-v1.md`](roadmap-for-v1.md)'s pre-freeze work: that file settles the few
*irreversible* surface decisions that must happen before the freeze (ADR-165/166/168/169);
this one records the permanent *absences*, so the freeze is a document a user can trust
rather than a version bump.

**The principle.** Two rules generate the whole list. **ADR-011** — favour the simplest
surface, defer power features until a concrete need justifies them; every knob is a tax
on every user, forever. And the **freeze asymmetry** (ADR-166): *relaxing* a restriction
later is backward-compatible, so a refusal can always be reversed if a real need appears
— *adding* a restriction later breaks whoever relied on its absence, which is why the
irreversible reservations had to be made *before* 1.0. So the cost of refusing is
recoverable and the cost of the feature is permanent; when in doubt, refuse.

**Decision — the freeze list.** Brood 1.0 permanently does not have:

| Refused | Why | Where decided |
|---|---|---|
| Mutation of data — no `set!`, atoms, cells, transients | The whole design rests on it: no write barriers, share-nothing processes, safe freezing | ADR-026, ADR-112 |
| `while` / `loop` / `recur` | Proper tail calls make recursion O(1); `letrec` covers local loops | ADR-154 |
| Named arguments (`&key`) | A trailing options map + `{:keys …}` reads the same and composes with `merge` | ADR-163 |
| Metadata (`^{}`), reader macros, `#(…)`, `#_` | Permanent surface for what a macro already does; `^` is the pattern pin | ADR-150 |
| A character type | A character is a 1-char string; the cursor unit is a grapheme cluster | ADR-159 |
| Ratios | `(/ 1 2)` is a float, `0.5M` an exact decimal, and `/` is the namespace separator; the `1/2` token is *reserved* (rejected by the reader) so a post-1.0 ratio type stays additive | ADR-169 |
| Digit-led tokens as names (`0x1F`, `1_000`, `1N`, `1+`) | A digit-led token must be a number; reserving the shapes keeps radix literals / digit separators / a bigint suffix additive after 1.0 | ADR-169 |
| `#…` beyond `#{…}` / `#b"…"` (incl. `#\|…\|#` block comments) | `#` is a dispatch character; reserving the space keeps every future `#` literal additive | ADR-169, ADR-150 |
| `contains?` answering by index on a vector | Clojure's trap: `(contains? [1 2] 1)` true for the wrong reason | ADR-156 |
| Strings as seqable | Codepoint vs grapheme is the caller's decision; bridge explicitly | ADR-156, ADR-159 |
| Unbounded laziness / `lazy-seq` | Seq-views fuse pipelines; processes cover unbounded state | deferred.md #2 |
| Alternative *negation* patterns (`(not …)`) | Binds nothing, so it is a guard — `:when` is the slot | ADR-160 |
| `:as` in a map pattern | `(and whole {…})` says it exactly | ADR-160 |
| Multiple dispatch | Single dispatch on the first argument's identity; `match` covers the rest | ADR-158, ADR-168 |
| Dispatch inferred from a `:type` **field** | Would silently reroute any map carrying `:type`; a `defrecord` identity is explicit and construction-time instead | ADR-168 |
| Nominal *types* | `defrecord` is structural sugar over a map; `defrecord` adds a dispatch-only identity, not a type — `type-of` is still `:map` and `=` stays structural | ADR-130, ADR-168 |
| More than one spelling per thing | `lambda`, `let*`, `car`/`cdr`, `concat`, `some?`, `length` all removed | ADR-098, ADR-154, ADR-162 |
| Monkey-patching the language | shipped functions are reserved; extend with an ability, shadow with `let`, or namespace it | ADR-166 |

**What this list is not.** It is not a list of things that can never be reconsidered —
by the asymmetry above, any *relaxation* stays open (a future Brood could grow ratios,
`&key`, or lazy sequences without breaking a single 1.0 program, because each is
additive). It is the set of things 1.0 **ships without**, stated so the absence reads as
a decision rather than an oversight. The genuinely irreversible entries — the reader
reservations (ADR-169) and the record-dispatch axis (ADR-168) — are the ones that had to
be settled *before* the freeze; the rest are simply deferred power features (ADR-011)
that a later version may add.

**Consequences.**
- The freeze is a document, not just a tag: a user can read what Brood refuses and why,
  with a one-click path to the full reasoning in each cited ADR.
- The "why not X?" questions have a single canonical answer, so they stop being
  re-litigated after 1.0.
- `roadmap-for-v1.md`'s freeze-list section now points here as the ratified source; that
  file remains the pre-freeze *work* tracker (the irreversible decisions + the deferred
  list), and this ADR is the permanent *refusals* record.

**References.** ADR-011 (defer power features — the generating principle), ADR-166 (the
relax-is-safe / add-is-breaking asymmetry), [`roadmap-for-v1.md`](roadmap-for-v1.md) (the
pre-freeze gate this ratifies the freeze-list half of), and every ADR cited in the table
above.

## ADR-171 — The display protocol: records customize printing via `Display`/`to-str`

**Status:** accepted, implemented 2026-07-28.

**Context.** `str`/`pr-str`/`%render` are kernel builtins that format every `Value` in
Rust (`syntax/printer.rs`). A `defrecord` value is structurally a `Value::Map`, so it
prints as its raw map — `{:__id__ :money/usd, :cents 1050}`. There was no seam for a
record to define *how it prints*: the equivalent of Elixir's `String.Chars`
(`to_string/1`, used by `IO.puts` and interpolation) or Haskell's `Show`. With the
`ability` system (ADR-168) in place, this is now expressible as an open generic, and
doing so is the headline case in the "write the language in the language" audit — the
one place value rendering genuinely wants third-party extension.

**Decision.** Add an opt-in std module `std/show.blsp` (`require 'show`) built on
`ability`:

- **`(defability Display (to-str [self] :-> string))`** — one op, a value → its display
  string.
- **`(impl Display :default (to-str [x] (str x)))`** — the fallback is the native `str`,
  so *nothing prints differently* until a record supplies its own impl. Pure extension.
- **A late-bound prelude hook, `*show*`** (a dynamic var, nil by default). The screen
  printers `print`/`println`/`eprint`/`eprintln` route each argument through it —
  `(if *show* (map *show* xs) xs)` — so the default (nil) path is one branch and zero
  cost. Loading `show` installs a hook that routes only **records** through `to-str`
  (built-ins pass through untouched, staying on the fast native renderer); `(binding
  (*show* nil) …)` disables it for a scope.

A companion **`Inspect`** ability (op `(inspect x)`, `:default` → `pr-str`, plus
`(inspectln x)` — the `IO.inspect` move, returns `x`) provides the DEBUG form, Elixir's
`Inspect` alongside `String.Chars`. It is deliberately **not** wired into `pr-str`:
`pr-str`'s output must round-trip (re-read to the same value), a kernel guarantee no
protocol may override; `inspect` carries no round-trip contract, so a record shapes it
freely (`#money<$10.50>`). `inspect` is called explicitly (or via `inspectln`), not
through the print hook.

**Shipping an impl with a library (the Elixir `defimpl` model).** A library ships its
display by putting the `impl` at the **top level of the module that defines the
record** — loading that module (which a consumer must do to reach the record) runs the
`impl`, registering it into `*impls*` as a load-time side effect. So a consumer that
`(:use bank)` to use `bank`'s `money` struct gets its `Display` impl **automatically**,
without naming `show` — verified: `(println (money …))` shows `$10.50` from `(:use
bank)` alone. Two activation levers for the library author:
- putting `(:use show)` in the library's header **activates** protocol printing on load
  (so implicit `(println record)` honors the impl app-wide — benign, since built-ins are
  unchanged); and
- a consumer needs `(:use show)` of its own only to call the protocol functions
  (`to-str`/`inspect`) *explicitly* — implicit display through the print hook needs no
  import. The open question deferred here (ADR-011): whether to **split** the ability
  (side-effect-free `Display`/`Inspect` a library depends on to declare impls) from the
  **activation** (`(def *show* …)`, an app/opt-in step), so a library can register an
  impl *without* flipping global print behavior. Left for a concrete need.

**Scope: the screen printers only.** `str`/`pr-str`/`fmt` stay on the native renderer —
they are the hottest paths (every error message, every string build), and routing them
through a Brood ability would regress them broadly for a niche benefit. Reach for
`(to-str x)` explicitly to honor the protocol inside a `str`/`fmt` (the Elixir
`to_string` move). This matches the request ("printing *to screen*") and keeps the
change additive and reversible.

**Why not change the default record printing (drop `:__id__`)?** Deliberately not:
ADR-130/168 record that a record printing *with* its id — and being `≠` a bare map — is
*intended* (Elixir-struct semantics), not a leak. The protocol is the seam to override
that per-record, not a reason to change the default.

**Why opt-in, not always-on?** The `ability` macros live in `std/ability.blsp`, loaded
on demand, not in the bootstrapped core; the prelude cannot depend on them. So the hook
is a prelude primitive (nil default) that the `show` module *installs* on load —
Erlang-style late binding, and faithful to Elixir's "the protocol is there once its
module is available." An always-on version would require moving the ability system into
the core image; deferred (ADR-011) until a concrete need.

**Consequences.**
- A record can define its screen display: `(impl Display money/usd (to-str [m] …))`,
  then `(println (usd 1050))` shows `$10.50`. Built-ins are byte-for-byte unchanged and
  incur no dispatch cost; with `show` unloaded there is no behavior change at all.
- First cross-module use of a `defability` op from another module (the test `(:use
  show)` calling `to-str`). Surfaced a checker gap: a `:use`d ability op from a *loose
  disk* module (not embedded, not in a project) is flagged unbound though it runs —
  embedded modules and same-module use resolve fine. Filed as a follow-up; does not
  affect `show`.
- The second audit candidate — a `JsonEncode` ability letting user records serialize
  instead of `json--emit` hitting its `else (error …)` tail — is the same shape and is
  left as a documented follow-up.

**References.** ADR-168 (abilities — the mechanism), ADR-130 (records as maps carrying
`:__id__`), ADR-006 (policy in Brood, not Rust), ADR-011 (defer the always-on variant).

## ADR-172 — Abilities v2: app-sovereign coherence, `impl`/`bridge`, compile-enforced, live-replaceable

**Status:** accepted (design), **amended 2026-07-28** — the orphan rule (§1) and the
`bridge` form (§2, §4) are **dropped before implementation**; see the amendment
immediately below. Shipped: the precedence ladder (§3) via package identity, and **§8 —
`Display` is core and always on**: the whole ability system + `Display`/`Inspect` were
folded into the prelude (`std/ability.blsp` + `std/show.blsp` deleted), so a record
customizes printing with just `(impl Display …)` — no `(require 'show)`, no `display-on`
(the interim activation is gone), and **§7's inline cache** — ability dispatch through a
per-op, epoch-validated inline cache (the `%dispatch` kernel primitive), so a hot
monomorphic call skips the two `*impls*` CHAMP lookups; the shared `global_epoch`
(bumped by `register-impl`'s `def *impls*` and by compaction) makes it reload-safe,
GC-safe, and cross-process-correct with no new invalidation machinery, and it is
invisible to the language (a pure memo of `impl-for`). Dispatch overhead vs a direct
call roughly halved. What remains of §7 is compile-time *static* resolution where the
receiver type is known, and the `:sealed` closed-switch — both pure optimizations over
the working IC.

**Amendment (2026-07-28) — abilities stay OPEN; no orphan rule, no `bridge` syntax.**
A design review pulled §1/§2 apart and found the restriction unnecessary and the form
substanceless:

- **`bridge` has zero runtime substance.** It expands to the *identical* `register-impl`
  call as `impl`, tagged with the same `(current-ns)`, landing at the same app tier — no
  behavior `impl` lacks. Its only purpose was to be the sanctioned, greppable, app-only
  channel for *orphan* impls, so that `impl` could be restricted to owned slots (§1).
  A second form that does exactly what an existing form does, to support a restriction we
  are not adopting, is precisely what "keep the language as small as possible" and ADR-011
  forbid — the same reasoning that dropped `:bridges` (§4).
- **The orphan rule (§1) is premature.** "A library must not `impl` a type/ability it
  doesn't own" guards against a *multi-third-party-library* collision that greenfield
  Brood (one app + `std`) does not have. Adopting it now would also break two capabilities
  we want and already have: impl'ing an ability for a **primitive** id (`:int`, owned by
  nobody) from anywhere, and impl'ing a **library** ability (`Display`) for your own
  records — both orphans-or-restricted under §1.
- **App sovereignty does not need either.** It is already delivered by the precedence
  ladder (§3, shipped): **app > type-owner > ability-owner > other**, with same-tier
  cross-module collisions warned. The app always wins; a library can't quietly outrank it.
- **The app/library line is computable, so no keyword is needed for it.** Package identity
  (`*ns-package*` vs `*project-name*`) already tells the checker who is the app. *If* a
  real multi-library orphan conflict ever appears, orphan-authorization becomes a **lint on
  plain `impl`** — "an orphan impl outside the app is flagged; inside the app it's allowed
  and listed" — with no new form, and it stays **advisory in the live image / hard-reject in
  CI** (§6). Deferred until such a conflict exists (ADR-011).

Net model: **abilities remain the open ADR-168 registry, made deterministic by the §3
precedence ladder.** `impl` is legal for any ability and any id — primitive, owned, or
someone else's — exactly as it runs today. `bridge` is not built.


**Context.** ADR-168 made abilities *open and late* — `impl` for any id, from any
module, at any time — dispatched through a runtime `*impls*` registry, coherence merely
warned. That is essentially Elixir's protocol model: maximally flexible, but a
dependency can silently change how a type behaves, conflicts are last-wins roulette, and
there is no notion of the application outranking a library. ADR-171 shipped `Display`
as an *opt-in* protocol whose activation was a load-time side effect of `(:use show)` —
which meant a library could flip global print behavior just by being used. A design
review (2026-07-28) reframed the problem: the axis that matters is not *coherence* but
**authority** — the application author must outrank every library — and the goal is
**compile-time guarantees without giving up hot reload** (the language's north star,
ADR-013). Full Rust-style static dispatch is therefore off the table; the target is
*static guarantees over live-replaceable semantics*.

**Decision.** One authority model for every ability, enforced by the compiler, dispatched
through the existing inline-cache/JIT path with deopt-on-reload, with the runtime
registry retained as the source of truth and reload backstop.

**1 — One coherence rule (`impl`).** `(impl A id …)` is legal **iff you own `A`**
(you `defability`'d it) **or you own `id`** (you `defrecord`'d that record type) — the
Rust orphan rule. A built-in id (`:int`, `:string`, `:default`) is owned by nobody, so
only the *ability's* owner may impl for it. This one rule kills three hazards at once: a
library can't touch your types, can't hijack a built-in, and two owners can't silently
collide (only the type-owner and the ability-owner can even produce an impl for a given
`(A, id)`).

**2 — Deliberate linking (`bridge`).** Orphan impls are *not* banned — they are moved
behind a distinct, intent-revealing form. `(bridge A (id (op …)) …)` is the **only**
sanctioned orphan site, and it is strictly **app-only** — a library writing `bridge` is
a checker error. It reads as *"I am the app, deliberately connecting two libraries I
use,"* and every cross-library hookup in a codebase is greppable (`grep bridge`). Two
shapes:

    (bridge JsonEncode
      (ecto/decimal (encode [d] (decimal->json-number d)))
      (ecto/uuid    (encode [u] (json-str (uuid->string u)))))

    (bridge JsonEncode :via json-str            ; uniform strategy for a family
      ecto/date ecto/time ecto/naive-datetime)  ; each: (json-str (to-str x))

So the whole rule is one clean line: **`impl` what you own; `bridge` what you link.**

**3 — App sovereignty.** The app (the top-level program) is the single exemption: it may
`impl`/`bridge` anything and always wins. Precedence on a resolved dispatch is
deterministic:

    app  >  type-owner  >  ability-owner  >  :default  >  native

(type-owner beats ability-owner: a type knows itself better than the ability author's
default *for* that type.)

**4 — Reusable glue, without a `:bridges` mechanism.** Elixir's "a package that impls
types for another package" tempted an earlier design (a manifest `:bridges` list
authorizing a glue package's orphan impls) — **dropped.** It contradicted §2 (`bridge`
is app-only, yet a package isn't the app) and its only real value — the turnkey "add
package X and it just works" — *is* the silent, transitive, ambient behaviour this whole
model rejects. Reusable glue does not need orphan impls in a package; split it the
ordinary way:

- the package exports plain **conversion functions** (owned, coherent — no orphans):
  `(defmodule json-ecto …) (defn decimal->json (d) …) (defn uuid->json (u) …)`;
- the **app** writes the `bridge`, calling them:
  `(bridge JsonEncode (ecto/decimal (encode [d] (json-ecto/decimal->json d))) …)`.

The reusable logic lives in a package; the **app always declares the link**. You can
never end up with a bridge you did not write — maximal auditability, and it keeps
`bridge` unambiguously app-only (which shrinks the ADR-070 dependency: "who is the app"
only has to answer "the root/entry program," never "an authorized bridge package").
Colliding `bridge` forms for the same `(A, id)` remain a **compile-time error the app
resolves**, not a silent last-wins.

**5 — The unifying principle.** *Owned + explicit* is automatic; anything **borrowed** (a
`bridge`) or **ambient** (an implicit path — §7) requires the **app** to act. A `bridge`
is the app *writing* the link; `display-on` is the app *enabling* the implicit path.
**Libraries propose; the app disposes** — and nothing a library ships takes effect across
the app without an explicit line the app wrote.

**6 — Compile-time enforcement, live-safe.** Coherence (§1), the app-only rule for
`bridge` (§2), `bridge` conflicts (§4), and `:sealed` exhaustiveness are
checked at **`nest check` / CI as a hard reject**, re-run on every reload — but stay
**advisory in the live image** (ADR-123–126): a running REPL may momentarily hold a
transient incoherent impl while you edit, and only *shipping* incoherence is blocked. The
guarantee is "you cannot ship incoherence," not Rust's "it cannot exist" — the price of
keeping live editing.

**7 — Dispatch: specialized, deoptimizable; sealed goes fully static.** An ability op is
a call whose target depends on the first argument's identity — exactly what Brood's
inline caches + JIT already specialize for ordinary calls, with deopt on type change.
Lower ability calls through that path: **resolve at compile time where the receiver type
is statically known** (a literal, a `defrecord` result, a typed variable), cache
(monomorphic/polymorphic IC) otherwise. Redefining an impl **deopts** the specialized
call sites and they re-resolve on the next call — so late binding survives. `:sealed`
abilities (a closed member set) compile to a **closed, exhaustive switch** — no runtime
table, like a Rust enum. The runtime `*impls*` registry remains the source of truth and
the reload backstop; the compiler is a *checking* layer and a *specialization* layer
**over** it, never a freeze (this is where Brood must be more dynamic underneath than
Elixir's frozen consolidation, precisely to keep §6's liveness).

**8 — Display is the one implicit-path ability.** `Display`/`to-str` is wired into the
core `str`/`print`/`fmt` path, **records only** (built-ins always format natively —
nobody owns them, §1), **app-gated** (the implicit path is off until the app enables it;
a library can never enable it), and **guarded** (a throwing impl → native fallback).
`pr-str` and kernel error rendering **never** dispatch — the round-trip form and the
never-fail path stay native. `Inspect`/`inspect` is the explicit debug form. This is the
only Display-specific machinery; the *authority* rules above are uniform across every
ability (Display is not privileged in who-may-impl, only in being on an implicit path).

**9 — Optional + dev dependencies.** Today the package manager (ADR-037) has one
**required** `:dependencies` list plus per-dep `:features` (Cargo-style build flags); no
optional/dev distinction. This ADR adds both — noting that with `:bridges` dropped (§4),
`:optional`'s *strongest* motivation (gating a glue package on both libraries being
present) is gone, so it falls back to the generic Cargo/Elixir optional dependency:

- **`:optional` per-dep** — declared but not force-installed; present only when the app
  *also* depends on it. Its remaining use is compile-if-present *library features* (a lib
  optionally depends on `ecto` and exposes ecto helpers only when `ecto` is present) — a
  real but less common pattern; already shipped and cheap, worth revisiting later for its
  keep. A `bridge` in the *app* is likewise **compile-if-present**: `(bridge JsonEncode
  (ecto/decimal …))` where `ecto` isn't a dependency is *inert*, not an error — the type
  doesn't exist, so the form contributes nothing.
- **`:dev-dependencies`** — a second list resolved for `nest test`/dev and on the dev
  load path, but **excluded from a release bundle** (ADR-038). Unaffected by the `:bridges`
  removal; the clearly-useful half.

**10 — Implementation plan (staged).** Slices, in dependency order:

1. **Package manifest** — `:optional` per-dep flag + `:dev-dependencies` list: parse,
   normalize (`std/tool/project.blsp`), resolver honors them (`std/tool/package.blsp`),
   release excludes dev-deps (`bundle.rs`). *Independent of everything below — buildable
   now.*
2. **`bridge`** — the macro (owner-check-exempt, strictly app-only site), compile-if-present
   inertness, same-`(A,id)` conflict = error. (No `:bridges` / glue-package authorization —
   dropped, §4.)
3. **Coherence checking** — owner-only `impl` (own ability or type), orphan → hard reject,
   `bridge` conflicts, at `nest check` (`types/check/protocol.rs`, which already tracks
   record identity). *Wants ADR-070 for the clean app/library line; interim uses the
   root-namespace convention.*
4. **Precedence resolution** — `app > type-owner > ability-owner > :default > native`,
   deterministic and static; extend `*impls*`'s keying + `impl-for` to carry provenance.
5. **Dispatch specialization** — lower ability op calls through the inline-cache/JIT path
   with deopt-on-reload; `:sealed` → a closed switch. *The performance slice.*
6. **Display always-on core** — records-only dispatch on the `str`/`%render` path,
   app-gated `display-on`, guarded; supersede the opt-in `std/show.blsp` (ADR-171).

ADR-070 (package-rooted namespaces) gates the clean app/library distinction in slices 2–3
only; everything else is independent. Slice 1 has no blockers and is the starting point.

**How it compares.**

| | Brood (this) | Elixir | Rust | .NET | Ruby |
|---|---|---|---|---|---|
| Coherence enforced | `nest check`/CI, re-run on reload | release consolidation only | always (absolute) | by construction | never |
| Orphan impl | `bridge`, app-only, explicit | allowed, silent, transitive | rejected | impossible | allowed (monkey-patch) |
| App is final authority | **yes**, deterministic | no | no | no | load-order |
| Conflict | compile error, app resolves | last-wins + warning | can't happen | can't happen | silent |
| Add impl at runtime | **yes** — re-checked + deopt | dev only | no | no | yes |
| Closed set exhaustiveness | `:sealed` → static | no | enums | no | no |
| Safety · Speed · Liveness | ✓* · ✓* · ✓ | ~ · ✓ · ✗ (release) | ✓ · ✓ · ✗ | ✓ · ✓ · ✗ | ✗ · ~ · ✓ |

The one novel cell is the last row: Brood aims for **all three** — Rust's guarantees with
Ruby's liveness — because safety is a *checking* layer and speed a *specialization* layer
over a registry that stays replaceable. The asterisks are honest: safety is *build-time*
(§6), speed is *best-effort* (a statically-unknown receiver still uses a cached lookup).

**Consequences.**

*Advantages.* Compile-time safety without freezing (impls stay addable/replaceable);
an authority layer no other language has (the app outranks every library,
deterministically — the direct answer to "a dependency must never override me or act
without my say-so"); one uniform `impl`/`bridge` rule across all abilities; graduated
(`:sealed` static, open specialized-but-reopenable); and it reuses machinery that exists
(`protocol.rs` identity tracking; the IC/JIT deopt path).

*Limitations.* No unrestricted orphan impls — you `bridge` (deliberate, app-scoped) or
newtype-wrap, more ceremony than Elixir's "just write it." Safety is build-time, not
every-instant (the live image can transiently hold incoherence, by design). Speed is
best-effort (dynamic-eval / heterogeneous receivers don't monomorphize). More machinery
underneath (speculative specialization + deopt is heavier than a frozen table — the cost
of liveness). And the clean *app-vs-library* line (who may `bridge`, whose `impl` wins)
wants **package-rooted namespaces** ([ADR-070](decisions.md), not yet done); the interim
convention is that the program's root namespace / entry module is the app. This ADR gives
ADR-070 a concrete motivation.

*Migration from what ships today.* `std/show.blsp` (ADR-171) is the interim runtime
`Display`/`Inspect`; its activate-on-`:use` model becomes app-gated (`display-on`), and
its open registration gains the §1 coherence rule. ADR-168's `*impls*` registry is
retained but reframed as the runtime backstop under a compile-time checking +
specialization layer, and `impl` gains the owner-or-ability restriction with `bridge` as
the sanctioned orphan escape. No `Value` kind, special form, or immutability contract
changes.

**References.** ADR-168 (the open runtime ability mechanism this tightens), ADR-171 (the
interim `Display` protocol), ADR-013 (hot reload — the constraint that rules out frozen
static dispatch), ADR-123–126 (checker never gates the live image; CI hard-rejects),
ADR-130 (records as maps carrying `:__id__`), ADR-070 (package-rooted namespaces — the
clean app/library line), ADR-011 (defer power features — why the always-on-in-core and
bridge machinery are scoped, not maximal).

## ADR-173 — `spy`: a homoiconic tree-tracing debug macro (borrow Elixir's `dbg`, do it more Lisp)

**Status:** accepted, implemented (2026-07-28). `spy` ships in `std/prelude.blsp`;
tests in `tests/spy_test.blsp`.

**Context.** Elixir's `dbg` is one of its most-loved conveniences: wrap any
expression (or drop `|> dbg()` into a pipeline) and it prints the source and value —
of the whole expression and, for a pipe, each stage — then returns the value
unchanged, so inserting/removing it never changes behaviour. Doing the same in Brood
was flagged as the highest value-to-effort item on the "Elixir-loved ergonomics"
backlog (after `with`, ADR — none; devlog 2026-07-28). The design question was whether
to transliterate `dbg` (special-case a fixed set of constructs, reconstruct source
from the AST) or exploit that Brood code *is* data.

**Decision.** Ship it as **`spy`** (a Lisp-tradition name over `dbg`), a **prelude
macro** — not a Rust builtin (ADR-006: mechanism in Rust, policy in Brood) — with
three deliberate choices:

**1 — Full homoiconic tree-trace, not a fixed special-case set.** `spy` fully
macroexpands the form and instruments *every evaluated position* in place, so it
traces the entire call tree, not just pipelines. Because `macroexpand` only resolves
the *outer* head (`(+ 1 (when …))` leaves the inner `when`), the walker re-expands at
every node. Instrumenting **in place** is what preserves evaluation semantics:
laziness (an untaken `if` branch, a short-circuited `and` tail) and single-evaluation
fall out for free, and referential transparency holds — the value is always returned
unchanged. A pipeline needs **no special case**: `(-> x f g)` expands to `(g (f x))`
and the ordinary call rule traces each stage. `fn` bodies, `quote`, and `quasiquote`
are left opaque (a closure body runs later/elsewhere; quoted data never evaluates).
This is strictly more than `dbg`, and simpler, because homoiconicity removes the
AST-to-source reconstruction Elixir needs.

**2 — A swappable sink (`*spy-sink*`), so a trace is DATA, not text.** Elixir's `dbg`
hardwires printing. `spy` emits structured entries — `{:spy :enter :form f}`,
`{:spy :node :form f :value v :depth d}`, `{:spy :exit :value v}` — through a `defdyn`
sink. The default pretty-prints an indented tree to **stderr** (never corrupting
stdout data); a host — the editor, the `nest observe` viewer, a test — rebinds the
sink to capture the trace as data. This is the "even better than `dbg`" bet and the
seam that lets the self-editing editor later render `spy` values as inline overlays
(M2/M3). It also subsumes the "no-op in production" need without a separate gate
(ADR-011): rebind the sink to a no-op — no `*debug*` knob added.

**Consequences.** One new public macro (`spy`) + one dynamic (`*spy-sink*`); no core
/ evaluator change, no new special form (ADR keeps the core small). Scope drawn at
descend-into `if`/`do`/`let`/`letrec` + calls; other special forms trace their top
value only (sound, conservative) — a fuller per-special-form rule table is deferred
until wanted. No source position in the trace yet (no position primitive is exposed
for a macro's argument form); the source *form* echo carries the information. Related:
[[with]] (ADR — the prior ergonomics borrow), ADR-006 (write it in Brood), ADR-011
(defer power — why the special-form rules and a `:label` arg are scoped, not maximal),
ADR-013 (hot reload — the sink seam mirrors the late-binding philosophy).

## ADR-174 — A process-native tracing debugger (`std/tool/debug`)

**Status:** accepted, implemented (2026-07-28, `worktree-spy-debugger`). Prototype toward
the ROADMAP `--breakpoints` gap. Spawn-level *and* send-level causality both shipped
(§2 and §4).

**Context.** Elixir's `dbg`/`IEx.pry` is the reference for interactive debugging, and it
has two limits everyone hits: a **pry timeout** (the process can't wait forever for the
one IEx session) and **no multi-process story** (a second process hitting the same
breakpoint queues behind the first and times out). Both stem from one root cause — the
debugger is a *terminal*, not a process. Brood is an actor runtime (share-nothing green
processes, immutable data, `spy`/ADR-173 already a swappable-sink tracer), so it can
dissolve both limits structurally instead of porting `dbg`'s design.

**Decision.** Make the debugger **a process**, and build the tool as **policy in Brood**
(`std/tool/debug.blsp`, a `dev-tools` DEV_MODULE — compiled out of a lean release) over
the thinnest kernel mechanism.

**1 — `break` parks without a timeout.** A breakpoint `send`s the process's snapshot to a
debugger process and blocks on `receive` for `[:resume]`. A parked process costs nothing
(off the scheduler), so it waits **indefinitely** — no timeout to need. Many processes
hitting the same `break` each park independently and fan into the debugger's mailbox as a
**queue of paused processes**, each inspectable and resumable. `break-when` is a
data-driven (predicate) breakpoint. This is the direct answer to `pry`'s two limits.

**2 — Causal spans, transparently propagated across `spawn`.** The debugger endpoint +
current span live in one dynamic, `*trace-context*`. The **kernel copies it into a child
at `spawn`** (`scheduler/lifecycle.rs`, reusing the existing `promote` + `push_dynamic`
machinery, so it's GC-safe — verified under `BROOD_GC_STRESS`). So a plain
`(spawn (fn () (break …)))` inside a `with-debugger` scope inherits the debugger and
parks with **no re-wiring**, and the debugger reconstructs a **cross-process causal tree**
— something `dbg` cannot do. Opt-in and **`#[cfg(dev-tools)]`-gated**: a lean release
compiles neither the hook nor the module (zero code, not merely zero cost); when the
debugger is inactive it's one empty-dynamics-stack check per spawn.

**3 — Traces are data, so debug the population.** `spy` entries flow to the debugger as
structured events; `value-distribution` / `modal-value` / `outliers` fold them, so 10k
processes hitting a trace point yield a *distribution* + the anomalies, not 10k text
dumps (Elixir's failure mode). `causal-tree` / `debug-report` / the live `debug-watch` /
interactive `debug-attach` render it.

**4 — Send-level causality (implemented).** Causality now follows a value A→B *through
a message*, so a long-lived server (never wired to the debugger) handles each request in
the *sender's* context — the thing `dbg` cannot do at all. The mechanism, all
`#[cfg(dev-tools)]` so a lean release is byte-identical:
- The durable context lives in a settable per-process **`trace_context` slot on the
  `Heap`** (replacing the earlier dynamic), GC-traced exactly where `dynamics` is
  (5 collector sites) — verified under `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1`.
- The mailbox message becomes an **`Envelope { msg, #[cfg] trace }`** — access is uniform
  `.msg`, so the receive matcher is untouched, and in release it's a zero-cost newtype
  over `Message`. `send` attaches the sender's context for a **local** pid (context is
  per-runtime, never crossing nodes); `receive` adopts it on pop.
- A context is tagged **own** (set by `with-debugger`/`span`, propagated by `spawn`) vs.
  **adopted** (from a message, used to handle it but NOT propagated onward) — so an
  adopted context can't leak transitively through unrelated spawns. (This distinction was
  found by a test: without it, the framework's own result messages leaked context into
  later test processes.)

**5 — Eval in a paused process's captured scope (path A shipped; B deferred).** At a
breakpoint you can evaluate expressions in the worker's scope — `eval-at` over the
`%eval-in` primitive, which builds a fresh env from a `{name → value}` map and evaluates a
form against it (GC-safe: a single-use frame per form, held forms + values rooted). Two
paths, and the split is forced by the engine:
- **A (shipped):** the map is the values *explicitly named* at `break` (`(break "here"
  :n n :total total)`), so `(eval-at d 1 "(* n total)")` resolves them. Works under the VM.
- **B (deferred):** *automatic* capture of every in-scope local by name. The VM keeps
  locals in **positional slots, not by name** (`%locals` — which walks named env frames —
  works only under the tree-walker), so no runtime primitive can recover them. B is a
  **compiler intrinsic**: teach the compiler to emit a `{name → slot-value}` map from its
  lexical-scope table at a `%scope` marker. Per ADR-011 it's a focused, VM-careful pass of
  its own, not bundled — the roadmap tracks it.

Related: ADR-173 ([[spy]] — the sink this builds on), ADR-006 (write it in Brood),
ADR-013 (hot reload — the late-binding kinship), ADR-046/051 (`nest observe` — the render
target for a future debugger pane). Still open: wiring the causal tree into `nest observe`
as an interactive pane, and cross-*node* causality (deliberately excluded — the debugger
is per-runtime).

## ADR-175 — Compiled code belongs to the runtime, not the process (the BEAM module-area model)

**Context.** A green process costs ~15 KB, against the BEAM's ~2.7 KB for a process
holding equivalent state. Measured 2026-07-28 (see devlog): the cost is not the mailbox,
the payload, or GC retention — it is that **every process compiles its own copy of every
function it calls**. `(fold + 0 nil)` — a fold over an *empty* list — costs ~18 KB in a
freshly spawned process, and live bytes scale linearly with the number of *distinct*
prelude functions a process touches (0 fns 13.8 KB/proc; 1 fn 31.6; 2 fns 50.0; 3 fns
59.9; 4 fns 66.5). At 300k live processes that is the difference between 4.58 GB and
Elixir's 942 MB on the same workload.

The duplication is already recognised in the tree. From `JitWorkItem`'s doc: *"thousands
of short-lived processes each queue their OWN `CompiledArm` copy of the same shared
closure; without the dedupe a spawn storm compiled `fib` ~68×."* ADR-101's `share_key`
fixed that for **native code** — a per-runtime `jit_code_cache` keyed by
`(closure_id, argc)`, epoch-guarded, idempotent publish. It did not fix the bytecode,
the `Node` tree, or the inline caches, which is what this ADR is about.

**What Erlang does.** A BEAM process holds *no code*. The PCB is stack + heap + mailbox;
code lives once per node in a module area, reached through the export table. Literals
live in a shared, refcounted per-module literal area rather than being copied into each
process. Hot reload is the current/old two-version rule, with old code purged once no
process references it. That separation — **code is node-global, state is process-local**
— is why a BEAM process is ~2.7 KB, and it is the property Brood lost by caching compiled
artefacts on the `Heap`.

Brood already has three of the four pieces:

| BEAM | Brood today |
|---|---|
| module area (code) | shared PRELUDE / RUNTIME AST regions |
| literal area | `ConstVal::Handle` into those regions |
| export table + hot reload | global table + `global_epoch` / ADR-091 generations |
| **compiled code, node-global** | **absent — per-process `vm_cache` + IC tables** |

**Decision.** Move the compiled artefacts to the runtime, mirroring `jit_code_cache`:
a per-`RuntimeCode` cache keyed by the existing `share_key` `(closure_id, argc)`, holding
`Arc<CompiledArm>`, validated by `epoch == global_epoch()` before install. Processes
install a shared arm instead of compiling their own. This is the module area, applied to
the artefact that is currently the odd one out.

**Explicitly rejected: dropping the retained `Node` tree.** The tree is 58% of
compiled-arm memory and 75% of it looked droppable on a static eligibility test (31/38
arms), so this was investigated first and rejected on five independent grounds:

1. `pub body: Node` has no interior mutability, and arms live behind `Arc<CompiledArm>`.
2. Unique ownership is never available — `CallIcEntry.arm`, `live_vm_arms` and
   `CompiledClosure::compiled` all hold the same `Arc`.
3. **The background JIT compiler thread holds an `Arc<CompiledArm>` off a queue and reads
   `arm.body` to lower it.** A drop races a live cross-thread read: use-after-free.
4. `Inst::TryCatch` holds `NodePtr`, a non-owning raw pointer *into* the arm's own tree,
   so `body` and `chunk` can only ever be replaced atomically together.
5. `vm_site_alloc` only pushes; call-site ids are never individually reclaimed. Every
   drop→recompile cycle would leak IC slots monotonically — an optimisation that leaks
   memory when repeated.

Sharing avoids all five: nothing is dropped, nothing is recompiled, no lifetime changes.

**The one real obstacle: call-site ids.** `vm_site_alloc` returns a *dense per-process*
index (`t.len()-1`) that is baked into each compiled `Node::Call` and used to index the
per-process IC tables. Share an arm and its baked ids come from whichever process
compiled it, so every process's IC vectors must be dense up to the global maximum.
Measured: a unit process uses 21 sites (4.3 KB of IC tables); the root uses 251 (33.4 KB).
Naive sharing would push every process toward the root's figure to save ~14.5 KB of body
— **a net loss of ~14 KB/proc**. So sharing is only correct together with a site-id fix,
and is staged behind one.

**The freeze changes the staging — Brood can do something the BEAM cannot.**
ADR-166 seals every shipped function *permanently* (the BEAM's `sticky` has
`code:unstick_mod/1`; we deliberately rejected that hatch). So for prelude code two
things hold that have no Erlang equivalent:

1. **Prelude compiled code is immortal.** No current/old duality, no purge, no
   `check_process_code`, no epoch guard — the invalidation machinery Erlang needs
   exists only because any module can be replaced, and ours cannot.
2. **A call to a frozen callee can be direct-linked at compile time.** The callee can
   never be rebound, so it is a constant. This is BEAM's *intra*-module direct call,
   except the entire frozen prelude behaves as one module — where the BEAM would still
   route a cross-module call through the export table, we can bind it outright.

Measured 2026-07-28, counting only sites compiled **after** seal seeding (i.e. the
compiles a spawned process actually pays for; sites compiled during the prelude build
predate sealing and read as unfrozen, which is a measurement artefact, not a result):

| program | post-seal call sites | frozen callee | late-bound |
|---|---|---|---|
| top-level `fold`/`map`/`filter`/`count`/`str` | 81 | 80 (98.8%) | 1 (`*out*`) |
| the same work inside a `spawn` | 537 | 535 (99.6%) | 2 (`unit`, `*out*`) |
| **40 user fns calling each other** | 722 | **483 (66.9%)** | **239 (33.1%)** |

**The fraction is workload-dependent, and the first two rows are not representative.**
They are prelude-dominated microbenchmarks with one user function between them; a real
application is full of user→user calls, every one of which is late-bound because user
code *must* stay redefinable for ADR-013 hot reload. Adding 40 mutually-calling user
functions drops the frozen share to 66.9%, and the late-bound share grows with the size
of the user's own code. Quoting "~99%" as the design's operating point would be
measuring the standard library and calling it an application.

Direct-linking still removes the IC slot for the frozen majority (67-99% of sites
depending on workload), which is worth having on its own. But it does **not** dissolve
the site-id obstacle — it scales it down by the frozen fraction. For user-heavy code a
third of sites still need IC slots, and those are exactly the sites living in the user
arms that Stage 2 would share.

It also removes a cost we pay today for nothing: a user `def` bumps `global_epoch` and
invalidates *every* IC, including prelude→prelude entries that no `def` could ever
affect. Direct links need no epoch guard, so hot reload stops disturbing frozen code.

**Staging (revised).** Each stage is independently landable and verifiable:

- **Stage 1 — direct-link frozen callees.** A `Node::Call` whose callee is a sealed
  global binds to it at compile time: no site id, no IC probe, no epoch check.
  **Caveat found while reading the VM (2026-07-28), and it constrains the design:** the
  call IC does not merely cache the *binding*, it caches the resolved
  `(Arc<CompiledArm>, EnvId)` payload — `vm_call_ic_probe` returns both, and a hit skips
  arm resolution entirely. So "bind the callee at compile time and drop the site" would
  save the slot at the cost of re-resolving the arm on **every prelude call**, i.e. a
  slowdown on the hottest paths in the system to buy memory. Stage 1 must therefore keep
  an arm fast path. The natural form: for a *frozen* callee the resolution is permanently
  valid and process-independent, so the cached arm belongs with the **shared code** (one
  entry per site for the whole runtime) rather than in a per-process table — which is
  Stage 3's mechanism, sound here precisely because the binding can never change. That
  entangles Stage 1 with Stage 2 more than this ADR first assumed: sharing the *cache*
  entry requires the cached `Arc<CompiledArm>` to be shared too. Sequence accordingly, and
  do not land a Stage 1 that regresses `make ab` on the prelude-heavy rows. Wins
  memory (no IC slot for ~99% of sites) *and* speed (ADR-166's own stated motivation —
  "every prelude call has to be late-bound because any global might be rebound"), and is
  independently useful whether or not sharing ever lands. **Ordering caveat:** sealing is
  seeded after the prelude is built, so a site compiled during boot must not be
  direct-linked on the strength of a seal that is not yet populated — the compile path
  has to distinguish "not sealed" from "not sealed *yet*", or it will silently bind
  nothing at boot and everything later.
- **Stage 1b — arm-relative site ids.** Sites become relative to their arm; each process
  allocates one contiguous IC block per arm instance it runs, and the frame resolves
  `base + rel` (`BcFrame` already carries `arm_slot`, so the base can hang off the
  existing per-process arm registration rather than a new lookup). Originally the gating
  stage; the freeze demotes it to **optional and much smaller**, since after Stage 1 it
  covers only the ~1% late-bound residue. Deferred until Stage 2 measurement shows
  whether it matters at all — ADR-011.
- **Stage 2 — shared `CompiledArm` cache.** The `jit_code_cache` analogue over
  `Arc<CompiledArm>`, same key, same epoch guard, same idempotent publish. With Stage 1
  in place, per-process IC cost stays proportional to arms actually used.
- **Phase C (done, 2026-07-29) — share RUNTIME-keyed user arms.** The eligibility gate
  now admits every shared-region closure, not just PRELUDE. The "double-rewrite" blocker
  recorded above is impossible (the compactor holds `Arc::get_mut` on the runtime, so it
  runs single-process); the actual hazards are a *cached* shared arm being missed by
  compaction's stack-only rewrite (fixed by clearing the shared cache alongside
  `vm_cache`) and generation-free handle recycling (fixed by stamping entries with the
  publisher's pre-compile `free_epoch`). Took a 40-arm user body from 37.1 to 4.55
  KB/proc and `spawn-live` to 3.00 s / 2.01 GB.
- **Stage 3 (optional) — shared inline caches.** BEAM's export table is node-global, and
  our IC entries cache *global* resolutions that are already epoch-guarded and hold
  promoted/immovable callees, so they arguably belong with the code too. Blocked on
  `CallIcEntry.fast` being a `Cell` (not `Sync`); it would need the atomic treatment
  `FastLink` already uses. Deferred until Stages 1–2 are measured — ADR-011.

**User code and hot reload are untouched — that is the point of the ADR-166 line.**
Direct-linking applies *only* to sealed names. A user function is never sealed, so its
call sites keep the inline cache and the epoch guard, and ADR-013 hot reload behaves
exactly as today. Sharing a *user* arm is likewise already-solved ground rather than new
risk: `share_key` covers "a RUNTIME/PRELUDE arm" and the installer checks
`epoch == global_epoch()` before use, so a `def` (which bumps `version` at
`heap.rs:4072` — "Invalidate every process's global inline cache") invalidates the shared
entry and the arm is recompiled. The asymmetry is the whole design: **frozen code is
shared and bound once; user code is shared but epoch-guarded, and rebinding still wins.**

A side benefit follows from the same asymmetry. Today one user `def` bumps the global
version and invalidates *every* IC entry in *every* process, including prelude→prelude
entries that no `def` could affect. Direct links carry no epoch guard, so after Stage 1 a
hot reload stops disturbing frozen code.

**Verification (required, not optional).** This touches the VM inner loop, the JIT, and
GC-visible structures, so reading is not evidence:

- the differential fuzzer (VM vs tree-walker) and the reader-robustness fuzzer;
- `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` over the suite — shared arms change what the
  RUNTIME compactor's `rewrite_node` walks;
- `BROOD_JIT_VERIFY=1`, plus a `BROOD_NO_JIT=1` A/B to separate a JIT miscompile from a
  sharing bug;
- **a TSAN build (`--features system-alloc`) is mandatory** — the hazard that killed the
  drop design was a threading one, and Stage 2 publishes `Arc`s read by the background
  compiler thread;
- `make ab` across the full row set (sharing changes IC hit rates), and the 300k
  `spawn-live` row for the memory claim itself;
- an off-switch from the first commit (`BROOD_NO_SHARED_ARMS=1`), matching
  `BROOD_NO_INLINE` / `BROOD_NO_HANDOFF`.

**Consequences.** Expected: per-process cost drops toward the parked-process floor
(~6.3 KB measured), since a process would hold state and IC blocks but no code. Risk
concentrates in Stage 1, which rewrites how every call site addresses its cache — the
extra indirection is on the IC hot path, so `make ab` gates it, not intuition. If Stage 1
costs more than a few percent, Stage 2's memory win has to justify it explicitly rather
than be assumed.

## ADR-176 — Hard `:kill` honoured on the non-capturing eval paths; REPL Ctrl-C interrupts the eval, not the image

**Status:** accepted (2026-07-28). Kernel: every eval path now honours a pending hard
kill at its reduction rollover. REPL: Ctrl-C kills the running evaluation and returns
to the prompt; the image survives.

**Context.** `(exit pid :kill)` promises death "at the next reduction tick", and the
top-level VM body driver delivers it (`tick_capture` + `capture_hard_kill_pending` →
`VmOutcome::Killed`). But that check lived only on the *capturing* path. The other
three execution routes — the tree-walker's `'tail:` loop, its `'dispatch` passthrough
redirect, and a nested VM run behind a native frame (`eval`, `try`, an HOF) — ticked
through plain `tick()`, which is pure accounting: on rollover, refresh the budget and
keep going. Nothing on those paths ever read the kill flag, so **a process evaluating
code via `eval`/`eval-string` was unkillable**: a spinning child died from a direct
call and survived the identical loop under `eval-string`, forever. Nobody had noticed
because nothing killed eval-ing processes until the REPL tried to (Ctrl-C evaluates
everything through `eval-string`) — but the hole reached every supervisor and `gen`
server facing a stuck code-evaluating child.

**Decision.** One shared safepoint primitive, `tick_reporting_hard_kill()`
(`process/scheduler.rs`): exactly `tick()`, except on the rollover — after `preempt()`
refreshes the quantum — it also reports a pending hard kill. Five call sites adopt it:
the tree-walker loop top, `passthrough_redirect_ok`, both `exec_chunk` self-tail
safepoints' non-capture branches, and `vm_run_bc`'s non-capture frame boundary. A
non-capturing path can't *return* an outcome across its native frames, but a kill only
needs to **unwind**: on `true` the site raises `LispError::kill_signal()` — the
pre-existing untrappable control signal from the native-nested-`receive` kill path —
which `%try`/cleanup natives re-raise and the body driver converts to death with the
mailbox's pending reason (`handle_capture_outcome` gains the conversion for the
tree-walked-body shape, so no route leaks it as a crash).

Two details are the actual lesson:

- **The passthrough redirect is a load-bearing safepoint.** In a tree-walked loop whose
  operators are thin wrappers (`>`/`-`/`+` are Brood defns over `%`-prims, ADR-069),
  the hot path ticks in `passthrough_redirect_ok` — the reduction budget drains there,
  so the loop-top check alone can be starved of rollovers. The eval *deadline* had
  already escaped through this exact gap once (its check is in that function for that
  reason); the kill check now sits beside it. Any future budget-consuming safepoint
  must go in both places or it will repeat this bug.
- **Checking only at the rollover keeps the hot path untouched** — one thread-local
  decrement, as before; the flag load happens once per ~2000-reduction quantum.
  Measured kill latency ~4ms on a JIT'd loop. On the root thread `CURRENT` is unset,
  so the check is constant-false — the REPL's own top-level eval can't kill itself.

**REPL consequence** (`std/tool/repl.blsp`, the feature this unblocked). Interactive
sessions install a SIGINT handler (`%install-interrupt-handler` / `%interrupt-taken?`,
the minimal kernel seam — a relaxed atomic store in the handler, a read-and-clear
probe; ADR-006, signals are mechanism). Each submitted form evaluates in a spawned
green process; the loop parks in `receive` polling the flag (~40ms), and Ctrl-C
`(exit child :kill)`s it — `; interrupted`, prompt back, image intact; `def`s made by
the child persist (shared code region). A second Ctrl-C while the first hasn't landed
(an eval wedged inside one native builtin never reaches a reduction tick) halts with
exit 130 — escalation, not a dead prompt. Piped sessions install nothing and keep the
default disposition (`echo … | brood` still dies on Ctrl-C like any Unix program). The
off-switch is `(def *repl-interruptible* false)` in `.broodrc.blsp` — a redefinable
global resolved to the armed state after the rc loads, not an env knob.

**Verification.** `tests/exit_test.blsp` pins the three routes (eval-string, eval of a
read form, kill-through-`try`) plus soft-exit-still-deferred-inside-eval; repro'd and
re-verified across default / `BROOD_NO_JIT=1` / `BROOD_VM=0`, under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`, and end-to-end through a pty (Ctrl-C during a
runaway form; session, `*1`, and child-made `def`s all intact after).

**References.** ADR-100 §8 (state-capture preemption and the `capture` gate this check
was wrongly folded into), ADR-063 (exit signals), ADR-069 (thin-wrapper passthrough —
why the redirect is the hot tick), ADR-052/ADR-048 (the REPL this serves), the
2026-07-28 devlog entries (including the wrong first diagnosis and what it missed).

## ADR-177 — `std/` adopts abilities: which seams became protocols, and which stayed `cond`

**Status.** Accepted, implemented 2026-07-29.

**Context.** Abilities became core in ADR-168/172 (`defability`/`impl`/`defrecord`,
nominal dispatch through `%dispatch`'s inline cache, precedence by tier), and ADR-171
shipped the first one (`Display`). The 2026-07-28 audit that accompanied ADR-171 looked
for more candidates in `std/` and found only two, concluding "the rest of `std/` is
correctly-closed `cond`/state-machines per ADR-011". A second, deliberately more
aggressive pass over the same tree found that conclusion too narrow: it had asked only
"where does third-party extension want in?", which is one of *four* distinct reasons a
site wants an ability.

**Decision.** Adopt abilities at every `std/` seam matching one of these four shapes, and
nowhere else. The shape is the criterion — not "could this be polymorphic?".

1. **A closed `cond` with an `else (error …)` tail where the error is the whole problem.**
   The set of cases is closed *by us* but shouldn't be. → `JsonEncode` in `std/json.blsp`
   (`to-json`): a record picks its wire shape, and a pid / fn / datetime becomes encodable
   by impl'ing it instead of hitting `json: cannot encode`. Registers **no `:default`** on
   purpose — a `:default` would make every value "encodable" and turn the loud error into
   an infinite recursion.

2. **One `:kind` tag re-`cond`ed in several places.** The bug isn't dispatch, it's
   *scatter*: adding a kind means finding every chain, and missing one fails late and
   quietly. → the `:sealed` `Dependency` ability in `std/tool/package.blsp` over the four
   dep records now defined in `std/tool/project.blsp`, replacing five separate
   `(get dep :kind)` chains (resolve, compatibility, lock row, manifest entry, tree
   label). Sealing is the point: `nest check` now reports a member missing an op, which no
   `cond` chain could. A resolved *entry* is its dep record with the resolution fields
   `assoc`'d on, so one ability covers both dep and entry.

3. **A documented seam already waiting for a richer value.** → `Port` in `std/io.blsp`
   (`io-write`), whose docstring already said it existed to let a port "grow into a richer
   value (named, introspectable) without touching callers"; and `LogBackend` in
   `std/log.blsp` (`backend-emit`), which lifts a backend from "a map whose `:format` fn
   you may replace" to "a value that owns its whole write policy". Also `Response` in
   `std/net/http.blsp` (`send-response`), which replaces the server's
   `(contains? resp :stream)` branch — the two stock kinds differ in *who closes the
   socket*, so that belongs with the response type.

4. **A value type that is a plain map identified by structural sniffing.** Not an ability
   at all — a `defrecord`, plus `Display`/`Inspect`. → `buffer` (was
   `(and (map? x) (rope? (get x :rope)))`), `queue`/`pq`, `multimap`, and
   `datetime`/`date`/`time-of-day`. The wins are a predicate that can't be fooled, a print
   form that isn't the internal representation, and — for `pq` — an *empty* queue that is
   truthy, since the old bare-list representation made `(if pq …)` silently false when
   empty (`()` ≡ `nil`).

`datetime` also earns a genuine ability: `Temporal`/`to-iso`, `:sealed` over the three
types, collapsing `date->iso8601` / `time->iso8601` / `dt->iso8601` — three functions the
caller had to choose between by knowing which shape it held — into one op that dispatches.

**Sealed or open, deliberately per ability.** `Dependency` and `Temporal` are `:sealed`
(the sets are genuinely closed and we want exhaustiveness). `JsonEncode`, `Port`,
`LogBackend` and `Response` are **open** — each exists precisely so a type we don't know
about can join.

**Explicitly rejected**, so the next pass doesn't re-litigate them:

- **The prelude's collection protocol** (`conj`/`get`/`nth`/`count`/`into`) — hot,
  kernel-backed, closed by design; an ability taxes every collection op.
- **`str`/`pr-str`/`fmt`** — settled by ADR-171 (`pr-str`'s round-trip guarantee; the
  hottest path). Tier 3 below routes *policy-layer* string conversion through `to-str`,
  which is not the same thing.
- **Closed internal AST/CST node kinds** (`std/regex`, `std/format`, `std/tool/sexp`,
  `std/editor/treesit`) — single-module, hot, and genuinely closed: correct `cond`/table
  dispatch per ADR-011.
- **`std/telemetry`'s metric kinds** — closed, one site, state in a `Table`. Sealing would
  buy checker coverage only; not worth the churn.
- **Error-value rendering** (`std/tool/test`, `std/tool/repl`, the prelude) — all three
  render kernel error *maps*, so every id is `:map` and one impl would capture every map
  in the image. Needs kernel errors to be records first; recorded as a follow-up.
- **`std/proc/gen`, `std/proc/supervisor`** — the implementor is a module or a thunk, not a
  value. That is `defbehaviour`'s job (ADR-168), already covered.
- **`std/editor/layers`' buffer types** — a deliberate mode *registry* (Emacs major
  modes), not type dispatch.
- **`std/stream`** — a stream is a pid, so every stream presents identity `:pid` and
  dispatch cannot tell kinds apart; the message protocol is already open.

**Tier 3: `(str v)` → `(to-str v)` at policy-layer call sites.** `std/template`
(substituted values), `std/csv` (cells), `std/url` (query values) now render values
through `Display`, so a record substitutes/emits as the string its type defines. This is
consistent with ADR-171 keeping `str` itself native: these are *library* call sites that
were already stringifying a user value, not the kernel renderer. The csv change also fixes
a latent bug — a non-string cell previously reached `includes?` and raised a type error.

**Consequences.** `nest check` gained real exhaustiveness coverage over the dep kinds; six
structural sniff-tests became identity checks; `std/` is now the worked example of the
ability system rather than a tree that merely *could* use it. The costs are honest ones: a
record is never `=` to a look-alike map (so tests comparing against map literals had to be
updated — `project_test`, `package_test`, `datetime_test`), and each of the four
`defrecord`-only conversions adds one wrapper allocation (`pq`, `multimap`) or one
`:__id__` field (`buffer`, the temporal types, the dep records).

**References.** ADR-168 (abilities subsume `defprotocol`), ADR-171 (`Display`, and the
narrower audit this widens), ADR-172 (the authority model, `:sealed`, `%dispatch`),
ADR-011 (prefer the simplest design — the reason the rejection list is as long as the
adoption list), ADR-130 (`defrecord`), the 2026-07-29 devlog entry.
