# Design decisions (ADR log)

An **ADR** is an *Architecture Decision Record* — a short, dated note capturing
one design choice and *why* we made it, so we don't accidentally relitigate
settled questions. Newest at the bottom.

> **The editor is `bedit`.** Entries up to ADR-198 call it `myedit`, and some call its repo
> `brood-edit` — both were renamed to `bedit` (the command it always was) on 2026-08-02.
> Entries are dated records and are not rewritten, so read either name as `bedit`; only the
> *paths* were corrected, since a wrong path is misinformation rather than history.

> **Dangling links are expected in older entries.** Entries are historical records
> and are not rewritten, so a few cite design/audit docs that have since been
> deleted — `concurrency-v2.md`, `supervision.md`, `memory-review.md`,
> `incremental-check.md`, `vm-perf-and-jit-runway.md`, `image-cache-plan.md`,
> `feedback-retro-game-of-life.md`, `value-repr.md`, `jit-tier2.md`,
> `jit-stage1.md`, `frame-representation.md`, `runtime-collector-exploration.md`
> (most trimmed in `fdce540` once the features they planned had shipped). Their content is superseded by the ADR that cites
> them, the topic doc, or the source itself; recover the text from git if needed.

## Index

The **in-force** ADRs live in seven files by number, each in number order:

| ADRs | File |
|------|------|
| 001–099 | [decisions/adr-001-099.md](decisions/adr-001-099.md) |
| 100–199 | [decisions/adr-100-199.md](decisions/adr-100-199.md) |
| 200–299 | [decisions/adr-200-299.md](decisions/adr-200-299.md) |
| 300–399 | [decisions/adr-300-399.md](decisions/adr-300-399.md) |
| 400–499 | [decisions/adr-400-499.md](decisions/adr-400-499.md) |
| 500–599 | [decisions/adr-500-599.md](decisions/adr-500-599.md) |
| 600–699 | [decisions/adr-600-699.md](decisions/adr-600-699.md) — a new ADR goes at the bottom |

To jump to one, open its file and search for its `## ADR-NNN` header. The per-entry
**Status** line is the source of truth for current state (the table below is a navigation
aid, not a status report, and stops at ADR-168).

**Four superseded/reverted/rejected ADRs have been moved out** to keep this log
focused on current design — their full text (with retrospectives) lives in
[archive/decisions-superseded.md](archive/decisions-superseded.md): ADR-002
*(superseded by the tracing/copying GC)*, ADR-035 *(superseded/disabled)*,
ADR-039 *(reverted → ADR-044)*, ADR-057 *(rejected as scoped)*. They're still
listed (italicised) in the index below so the numbering stays complete.
An ADR replaced by a later one, in whole or in part, but left in place says so in a
`> **Superseded** …` or `> **Partly superseded** …` line directly under its heading, so a search hit
on it shows that line first.
ADR-071 *(WASM extensions)* is accepted and partly built (ADR-145). ADR-119
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
| 163 | The convention questions the syntax review raised, settled *(failure half superseded by ADR-310)* |
| 164 | `get`/`nth` diagnostics: an error must name the operation the caller wrote |
| 165 | A keyword is callable as an accessor; nothing else data-like is |
| 166 | Reserved names: the language's own functions cannot be redefined |
| 167 | Keyword accessors are typed, not just callable |
| 168 | `ability` is the one value-dispatch seam; `defprotocol`/`defimpl` retired |

---

