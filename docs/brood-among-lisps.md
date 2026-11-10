# Brood among the Lisps

**Status 2026-11-09, read against v0.50.0 + Unreleased.** This page places Brood among the
Lisps a newcomer is likely to know: Common Lisp, Scheme and Racket, Clojure, Janet, Fennel,
LFE and Emacs Lisp. Its job is to say what Brood *is* in that family, what it deliberately is
not, and what a Lisper will reach for and not find. It is not a parity list. A difference from
another Lisp is Brood's own design unless this page says otherwise, and a feature another Lisp
has is never, on that ground alone, something Brood owes. For which reflex to drop when coming
from Clojure, the row-by-row table is `language.md` § "Coming from Clojure".

The 1.0 plan this page feeds is [`road-to-1.0.md`](road-to-1.0.md).

## In one paragraph

Brood is a **Lisp-1 with eight special forms** (`quote if do def fn let letrec quasiquote`,
`crates/lisp/src/eval.rs`). Everything else, including `defmacro`, `try`, `match`, `cond`,
the REPL and the `nest` toolchain, is written in Brood. Its **data is immutable without
exception**: there is no `set!`, no atom, no transient, and the one mutable store is a `Table`.
**The runtime is the actor model**: share-nothing green processes on every core, with
preemption, links, monitors, supervisors and encrypted distribution, and messages that cross
between processes as copies. **Live redefinition is the design centre**: a `def` reaches every
running process on its next call. On top of that sit an advisory set-theoretic type checker,
and a three-tier engine (tree-walker, bytecode VM, Cranelift JIT) whose warm start is around
13 ms.

The shortest placement is this. It has **Emacs Lisp's purpose**: a program that edits itself
while it runs. It has **LFE's runtime model**: processes, links and supervision, without
needing a BEAM. It reads closest to **Clojure on the surface**: `[]`, `{}` and `#{}` literals,
keywords and persistent collections. Its macro system is its own middle path between Common
Lisp and Scheme.

## The axes

### Host and runtime

| | Runs on | Concurrency model |
|---|---|---|
| Common Lisp | native (SBCL, CCL, …) | OS threads, shared mutable heap |
| Scheme / Racket | native or VM (Chez under Racket CS) | Racket: threads, places, futures |
| Clojure | JVM (CLJS: JS; Babashka: GraalVM native) | STM refs, atoms, agents, core.async |
| Janet | its own small C VM, embeddable | fibers, threads with message passing |
| Fennel | Lua (compiles to Lua source, no runtime of its own) | Lua coroutines |
| LFE | BEAM | Erlang processes and OTP |
| Emacs Lisp | Emacs | one thread, timers, cooperative threads |
| **Brood** | **its own runtime in Rust; one static binary; wasm32 in a browser tab** | **preemptive M:N green processes, per-process heaps, share-nothing** |

Brood is its own host. It has no JVM, BEAM or Lua underneath, which means no host library to
borrow and no host to wait on. Programs reach the outside world through subprocesses
(ADR-104), TCP/TLS and HTTP (ADR-062), and sandboxed WASM components (ADR-145/453). It has no
C FFI, and that is deliberate (`interop.md`): a native call cannot be preempted, and a crash in
one would take every process with it. Among the Lisps above, only LFE gives the same failure
isolation, and it gets it from the BEAM.

### Data

The **immutable-by-default** Lisps are Clojure, LFE and Brood. Common Lisp, Scheme, Janet,
Fennel and Elisp are mutable at their core. Brood goes further than Clojure here, because it
has no escape hatch inside a value. ADR-026 makes the refusal absolute and ADR-170 freezes it.
This is not taste alone: the GC needs no write barriers for data, and processes can share
nothing because nothing changes. Mutable state lives in a process loop or in a `Table`.

Brood shares a few smaller choices with others and differs from them on others. It shares
Clojure's truthiness (only `nil` and `false` are falsy, ADR-004). Unlike Clojure it makes `()`
equal to `nil`, so the empty list is the one falsy collection, and a lint flags code that
leans on it (ADR-620). It has no character type, as in Janet and Fennel; a character is a
one-character string, and the cursor unit is the grapheme cluster (ADR-159). Strings are not
seqable (ADR-156). Records are a value kind of their own, never `=` to a map (ADR-542). The
numeric tower is closer to Scheme's than to Clojure's: integers widen to bignums, `/` is exact
and gives ratios, and decimals are a separate kind (ADR-196). `(= 1 1.0)` is false.

### Macros and syntax

| | Macro model |
|---|---|
| Common Lisp | `defmacro`, unhygienic; `gensym` by hand; reader macros |
| Scheme / Racket | hygienic `syntax-rules` / `syntax-case`; Racket's `#lang` grows whole languages |
| Clojure | `defmacro`; auto-gensym `x#`; namespace-qualified syntax quote |
| Janet, Fennel | `defmacro`, `gensym` by hand (Fennel checks some captures at compile time) |
| **Brood** | **`defmacro` with hygiene at every binding site of a template** (ADR-500): a binder written in a backtick template is renamed, a free reference resolves in the defining module, and `~'it` opts out for anaphora. Root call heads are written `/name` so a local can't capture them (ADR-423/638). Quasiquote nests by level (ADR-503). |

Brood takes the `defmacro` ergonomics of Common Lisp and Clojure and the capture safety Scheme
users expect, without syntax objects or a phase system. The gaps are written down
(`language.md` § Hygiene): binding macros a user defines and binders computed at expansion
time are not renamed. **No reader macros, no metadata, no `#(…)` or `#_`** (ADR-150). The `#`
dispatch space is reserved, which keeps every future literal additive (ADR-169).

### Functions, dispatch and polymorphism

- **One head grammar for every `fn`** (ADR-619): patterns in the parameter slots, `&optional`
  with defaults, `& rest` and a `:when` guard, with clauses tried in order. A shadowed clause
  is a compile error. Parameter lists are lists, never vectors (ADR-512). There is **no
  `&key`** (ADR-163): a trailing options map plus `{:keys …}` does the same job.
- **Abilities** dispatch on the identity of the first argument, open and replaceable live,
  which puts them in the same role as Clojure protocols or Racket generics.
  **`defmulti`/`defmethod`** dispatch on the exact tuple of argument identities (ADR-179). That
  is narrower than CLOS: no class precedence, no method combination, no `:around`. ADR-523
  gives the rule for choosing between them.
- **No nominal types.** A record identity is for dispatch. Subtyping in the checker is set
  inclusion.

### Control and errors

| | Errors | Non-local control |
|---|---|---|
| Common Lisp | the condition system: `handler-bind`, restarts | `block`/`return-from`, `catch`/`throw`, `unwind-protect` |
| Scheme / Racket | `raise`, `with-handlers`, exceptions as structs | first-class continuations (`call/cc`), `dynamic-wind`, prompts |
| Clojure | JVM exceptions, `ex-info` | none |
| LFE | Erlang exceptions, "let it crash" | none |
| **Brood** | **two channels** (ADR-310): an expected failure is a returned `failure` value with fields (ADR-617), and a bug is raised as one error shape `{:kind :message …}` (ADR-501). `catch` takes a pattern; `finally` unwinds (ADR-306). | **none exposed**; recovery is supervision |

Common Lisp users will miss **restarts**, and Scheme users **continuations**. Brood's answer to
both is the process. A failure that should be retried is retried by a supervisor that restarts
a clean process; that is LFE's and Erlang's answer, not Common Lisp's. Proper tail calls are
guaranteed in every tier (the tree-walker is a loop over an explicit frame stack, ADR-416), so
iteration is recursion: **no `loop`, `recur` or `while`** (ADR-154). Comprehensions are `for`,
`for :into` and `fold-for` (ADR-377).

Dynamic scope is opt-in and per process. `defdyn` declares a name and `binding` rebinds it,
restoring on throw. A spawned process starts from the defaults (ADR-151). `let` is always
lexical, unlike Elisp's historical default.

### Laziness

Clojure's sequences are lazy by default, and Scheme has streams and `delay`. Brood is **eager
by default**. Seq-views (`seq/lmap`, `seq/lfilter`) fuse pipelines, `range` is lazy, and
unbounded state belongs in a process. There is **no `lazy-seq`** (deferred.md #2, ADR-170). A
seq-view cannot be sent between processes.

### Live programming

Every Lisp in the list has a REPL. They differ on what survives a redefinition:

- **Common Lisp** redefines in place and saves the whole heap as an image
  (`save-lisp-and-die`).
- **Emacs Lisp** redefines in place. The running editor is the program.
- **LFE** gets the BEAM's two-version code loading with an upgrade callback.
- **Clojure** redefines a var, but code that captured the old function keeps it.
- **Brood**: a `def` reaches **every process** on its next lookup, because one runtime's
  processes share their code (`shared-code.md`). A long-lived process takes a `[:code-change]`
  hook, and an edit reloads the module and its dependents (ADR-419). Functions shipped in the
  binary are reserved, so a stray `(def inc …)` raises (ADR-166). A startup image restores
  bindings rather than a heap (ADR-218), so there is no Common Lisp-style `save-image` of a
  live session.

### Types

Typed Racket has gradual typing that is sound at its boundaries. Common Lisp has
`declare`/`declaim`, which SBCL uses for checks and speed. Clojure has core.typed and spec as
libraries. LFE has Dialyzer's success typing. **Brood ships a set-theoretic, gradual,
advisory checker** in the line of Castagna and Elixir (ADR-023/024). It has occurrence typing,
exhaustiveness over literal enums, and `sig`s that become runtime contracts with blame under
`nest run` and `nest test` (ADR-381). It never refuses a redefinition in the live image; only a
batch `nest check` gates.

### Tooling

Brood ships **one toolchain in one binary pair**: `brood` runs the language and `nest` runs
the project (new, run, test with property tests and coverage, check, format, doc, doctest,
rename, publish, release, repl, mcp, observe). There is a PubGrub solver with signed packages
(ADR-209/212) and a `brood-lsp`. The comparisons: Racket's `raco` is closest in scope; Clojure
splits the job across `deps.edn`, Leiningen and editor tooling; Common Lisp has
ASDF/Quicklisp and SLIME/Sly; Janet has `jpm`. `nest release` makes a single executable with
no toolchain on the target, as Janet's `jpm quickbin` does.

## What a Lisper will reach for and not find

Every row is a decision with an ADR, not a gap. ADR-170 is the ratified list.

| From | Reaches for | Brood instead |
|---|---|---|
| any | `set!`, `setf`, atoms, transients | a process loop, or a `Table` (ADR-026) |
| CL, Clojure | `loop`, `recur`, `while` | tail recursion, `for`, `fold-for`, `doseq` (ADR-154) |
| CL | the condition system, restarts | failure values plus supervision (ADR-310) |
| Scheme | `call/cc`, prompts | processes |
| CL, Clojure, Racket | reader macros, metadata, `#lang` | macros over the fixed reader (ADR-150/169) |
| CL | `&key` | a trailing options map (ADR-163) |
| CL | CLOS method combination, class hierarchies | abilities, `defmulti`, records (ADR-168/179/542) |
| Clojure | `lazy-seq`, infinite lazy sequences | seq-views, processes |
| Scheme, CL | a character type | one-character strings, graphemes (ADR-159) |
| CL, Janet, Clojure | a C FFI or host interop | WASM components, subprocesses (`interop.md`) |
| CL | `save-lisp-and-die` | `nest release` and startup images |

## What Brood has that the others mostly don't

- **Failure isolation and supervision without a BEAM**: one static binary that runs
  preemptive, share-nothing processes across all cores, and a crash in one takes down only
  what is linked to it.
- **A `def` that reaches every running process.** Code is shared live across processes, while
  data is never shared.
- **Immutability with no exceptions**, which is what lets the two points above be cheap and
  safe.
- **Hygienic `defmacro`** without syntax objects.
- **A set-theoretic gradual checker** shipped as part of the toolchain, advisory in the live
  image.
- **Eight special forms**, with the rest of the language (the REPL and the CLI included)
  redefinable Brood.

## What this means for 1.0

A Lisper judging a 1.0 looks for four things. The first is whether the surface will hold
still, and today the README says it will not. The second is whether the docs match the
implementation; `language.md` has a stale paragraph on arity dispatch (ADR-047 versus
ADR-619), and ADR-170's table predates ADR-542. The third is whether it runs on their machine:
release binaries cover Linux and macOS, not Windows or musl. The fourth is how to reach a C
library, where the answer is WASM, with build-on-fetch still open. Each of these is an item in
[`road-to-1.0.md`](road-to-1.0.md).
