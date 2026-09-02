# ROADMAP

Brood is a general-purpose **language and runtime** — born as the substrate
for a modern, Emacs-like, self-editing editor, grown past that intent into a
language in its own right. This repo is all of it: the language core, the
runtime, the standard library, and the `std/editor/*` framework for
interactive applications. The foundation came in milestones (M1–M4), each
shippable and useful on its own.

Guiding constraints (see `CLAUDE.md`): keep the **language core small** — prefer
adding a primitive function or a prelude macro over a new special form — and write
as much as possible *in Brood itself*. Tags: **[kernel]** = needs new Rust;
**[Brood]** = can be written in the prelude.

Legend: ✅ done · 🟡 in progress · ⬜ not started · ❌ tried and reverted

> This is the single canonical roadmap. Deep design lives in the per-topic docs
> under [`docs/`](docs/) (ADRs in [`docs/decisions.md`](docs/decisions.md), a
> dated history in [`docs/devlog.md`](docs/devlog.md)).
>
> **For the 1.0 language freeze specifically, read
> [`docs/roadmap-for-v1.md`](docs/roadmap-for-v1.md)** — the short list of what must
> change on the language *surface* before 1.0 (three items), what is deliberately
> deferred because it is purely additive, and the draft freeze list of what Brood
> permanently is not. This file remains the superset; that one is the release gate.

---

## Contents

- [**Active work**](#active-work--dated-findings--backlogs) — dated findings & backlogs (the
  large, time-ordered section; skim by the `###` sub-headers below)
  - [Lisp survey + OTP-gap backlog](#lisp-survey--otp-gap-backlog-2026-08-30) ·
  - [Backend seams](#backend-seams--swappable-jit--engine--perf-legibility-2026-08-11) ·
    [Runtime-feature parity](#runtime-feature-parity-program--beam--net--node-2026-07-22) ·
    [Robustness gaps vs BEAM/.NET](#robustness-gaps-vs-beam--net-2026-07-18-runtime-survey) ·
    [Elixir-parity perf gaps](#elixir-parity-performance-gaps-2026-07-12-refreshed-2026-07-18) ·
    [Stability backlog](#stability-backlog-2026-07-10) ·
    [External conformance corpora](#external-conformance-corpora-2026-07-25)
- [**Done — the foundation**](#done--the-foundation) — shipped milestones (M1–M4)
- [**What's next — by area**](#whats-next--by-area) — the forward-looking backlog:
  [Language core & types](#language-core--types) · [VM & JIT](#vm--jit) ·
  [Tooling & errors](#tooling--errors) · [Editor & display](#editor-m2--display-m3) ·
  [Server / daemon](#server--daemon-m4) · [Packaging & ecosystem](#packaging--ecosystem)
- [**Design notes**](#design-notes-context-for-the-above) ·
  [**Cross-cutting open questions**](#cross-cutting-open-questions-revisit-dont-build-yet) ·
  [**Language features (held at arm's length)**](#language-features--candidates-held-at-arms-length) ·
  [**Killed directions**](#killed-directions-dont-retry) ·
  [**Out of scope**](#out-of-scope-deferred-additive-later) ·
  [**Guiding principles**](#guiding-principles)

**Status at a glance (2026-07-30):** M1–M4 shipped. No open bugs — every issue in
[`docs/known-issues.md`](docs/known-issues.md) is resolved. The abilities/types thread is
complete (dispatch, provided methods, `:derives`, record patterns, exhaustiveness, ability
bounds, super-abilities, occurrence typing). Active fronts: **perf/JIT tuning**, the **seq
protocol on abilities** (read half done), **std-library breadth** (no markdown/WHATWG-URL yet),
and **observability**. See "What's next — by area".

## Active work — dated findings & backlogs

### Argument order + error conventions — ✅ COMPLETE (2026-08-30)

Three linked changes, all landed:

- **ADR-302 — `!` means "raises", not "has effects".** Brood was spending `!` on both
  meanings at once: four raising bangs against 37 effectful ones. `!` now marks raising
  only; the Scheme reading was vacuous here anyway (it marks *mutation*, which Brood has
  none of — ADR-026). 38 names renamed; `run!` became **`each`**, since six modules define
  their own `run`.
- **ADR-303 — the `$` placeholder** in `->` / `->>` / `some->` / `cond->`: a step naming
  `$`, at any depth and inside vector and map literals, receives the threaded value there.
  Bound once to a gensym, so `(-> (expensive) (+ $ $))` evaluates once; `'$` stays a symbol.
- **ADR-308 — data-first argument order.** The collection is argument one
  (`Enum.map(enum, fun)`), `reduce` keeps both arities, and `->>`/`some->>`/`cond->>` are
  **deleted** — one convention needs one pipe. ~1780 call sites over 30 functions. The
  reducer's own `(fn (acc x))` params deliberately did *not* move (that swap fails silently;
  the outer one fails loudly), and `into` stays `(to from)`.

**The surface audit (`std/tool/audit.blsp`, `(audit/report)`)** landed with it: four
mechanical checks over every public callable — a docstring, a `form → result` example,
data-first argument order, and a declared signature whose parameter count matches the
function. Three of the four are at **zero** and gated by `tests/audit_test.blsp`. It found
`seq/transduce` and `sort-by` still data-last, and two kernel signatures
(`string/grapheme-at`, `string/substring-graphemes`) that declared `Arity::range(2, 3)` but
left the third argument undeclarable.

Open, with a number that is honest rather than flattering: **1130 of 1593 public callables
carry no example.** The figure jumped from 238 when the audit learned to `require` every
baked-in module first — `reflect/global-names` lists only what is LOADED, so two thirds of
the surface had been silently unaudited. Docstrings are at 100%. Closing the examples is
authorship, not a sweep: ~40 names are side-effecting (`file/rm`, `exit`, `system/halt`)
where an indented example would really run under `doctest`, and the rest need a
human-meaningful case per function. Do it module-by-module with `doctest` gating each batch.

Ahead, deliberately not started: the `[:ok v]` / `[:error reason]` result convention the `!`
rename was groundwork for — a structured error map shared by `throw` and the tuple, plus
`result`/`unwrap`/`unwrap-or`, and **tuple-union exhaustiveness in the checker** (ADR-128
tuples are tag-disjoint, but `check/exhaustive.rs` covers only literal-enum and sealed
scrutinees, so a forgotten `[:error …]` branch would not warn). Without that last piece the
convention is a naming rule rather than an enforced one.


### Lisp survey + OTP-gap backlog (2026-08-30)

A feature-by-feature comparison of Brood against Clojure, Common Lisp and Racket, and of
its process model against OTP, produced two backlogs. Every item below was checked against
the ADR-170 freeze list — nothing here reopens `loop/recur`, transients, `&key`, metadata,
reader macros, continuations or a syntax-object expander. Ordered by value per hour within
each list; the OTP list is first because it is what makes Brood's strongest axis credible.

**Where the process model stands vs OTP.** At BEAM level already: preemptive M:N scheduling,
per-process heaps + GC, links/monitors/`trap-exit`/`exit :kill`, selective receive with
receive-marks, one-shot reply aliases, `gen` with `terminate`, supervisors with all three
strategies + intensity window + shutdown policy + nested teardown, hot reload, encrypted
full-mesh dist with remote spawn/monitor. ADR-039's kernel supervisor was reverted for being
the scheduler-race surface and that call stands — **none of the items below touch the
scheduler.** What is behind OTP is *around* supervision, and nearly all of it is Brood.

- ✅ **A. Crash reports by default** (ADR-305, shipped 2026-08-30). The premise was
  overstated — the kernel did print a `process N died: …` one-liner and dump it — but the
  line had no trace, repeated per crash-loop iteration and had no seam. Shipped:
  `proc/system-monitor` became one subscription per pid with an `:exit-abnormal` selector,
  `std/proc/crash-report.blsp` prints each crash site once, `brood file`/`nest run`/bundle/
  REPL arm it, the one-liner yields. Original note: The kernel already emits `:exit` with the
  structured reason through `proc/system-monitor`; nothing listens unless a supervisor is
  linked. OTP's answer is the `logger` process, not
  kernel supervision. Build `std/proc/crash-report.blsp`: a listener spawned at boot (armed by
  `nest run` and the REPL; off under `nest test`, which has its own reporting), subscribed to
  non-`:normal` exits, printing one SASL-shaped report — pid, registered name, reason, the
  error's `:trace`, the last message — deduplicated per site the way ADR-232's drop warning
  is. **Default-on** for ADR-232's reason: a flag you must arm before the bug is absent when it
  matters (KI-36). Pure Brood.
- ⬜ **B. `defapp` — an Application behaviour.** `nest run --main` calls a function; there is
  no "this project is a tree with a root supervisor, its dependencies start first, shutdown
  is the reverse". `(defapp name {:start (fn () sup-pid) :stop … :requires [other-app]})` in
  `std/proc/app.blsp`; `nest run`/`nest release` resolve `:main` to the app when one is
  declared, start `:requires` in dependency order, link the root supervisor to the top-level
  process, stop apps in reverse on `halt`/SIGTERM. Extend `--check-boot` to "start every app,
  stop every app, run nothing". Supersedes the `Application` entry under "OTP deferred" below.
- ✅ **C. `try … finally`** (ADR-306, shipped 2026-08-30 — a prelude macro clause over two
  `%try`s; `throw` now rebuilds an error from its caught map so a rethrow renders as the
  original). Original note: All three Lisps have unwind cleanup (`finally` /
  `unwind-protect` / `dynamic-wind`); Brood has none — a resource held inside a `gen` handler
  can only be released by process death. A `(finally …)` clause lowered by the existing `try`
  macro onto a `%try` variant; no new special form. Pair with a `with-open`-style macro over
  `Port`, and teach the `:discarded-catch` lint about it. *Reassessment of the "`terminate/2`
  on hard kill" item under Dist refinements:* OTP's `terminate/2` does not run on `kill`
  either — it runs on a trapped `shutdown` exit, which is what the supervisor's
  `:shutdown <ms>` path already sends before the hard kill. That item is by-design; document
  the semantics and close it.
- ⬜ **D. `defstatem` over `gen`.** "OTP deferred, gated on a consumer" — a network protocol
  handler or an editor mode is the consumer. A macro: states as clause heads
  (`(:connected (:msg …) → [:next :closing data])`), state timeouts via `timer/send-after`,
  postponed events kept in the state map, all lowered onto `defserver`. No kernel work.
- ⬜ **E. Registry + process groups.** A local Elixir-style `Registry` is a `table` plus a
  monitor per entry — a day's work — and gives via-tuples for `gen/call`. Cluster-wide `pg`
  is a `node/monitor`-driven replicated set, eventually consistent like Erlang's own. `:global`
  (locking, consensus) stays deferred; it is the part of OTP people replace.
- ✅ **F. Mailbox bounds** (ADR-308, shipped 2026-08-30): `(proc/flag :max-mailbox n)`,
  the `:max-heap` protocol on the queue axis — senders check at enqueue but are never
  blocked and drop nothing; the flooded process raises catchable `E0046` at its next
  safepoint or `receive` entry; clear-inside-catch rescues. Original note: kills → refined
  to a catchable raise (what `:max-heap` actually does); backpressure proper stays a
  library (`gen/call` with a timeout). Also the "Per-process resource limits" item under
  Robustness gaps.
- ⬜ **G. Soak, not features.** The remaining gap with OTP is evidence, and no feature closes
  it. `breakage/` and the stress ladders exist; what they lack is duration. A nightly
  multi-hour soak — a supervised ring across three nodes, a node killed every N minutes,
  weekly under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` — reported by `make green`.

**What the other Lisps have that fits Brood.** Value ranked; "shape" marks an import that
needs a Brood-specific form to respect an ADR.

- ✅ **1. `finally`** — item C above (ADR-306).
- ⬜ **2. A complete regex engine.** Every other Lisp has ranges, captures and `{m,n}`;
  `std/regex.blsp`'s pure-Brood NFA has none, which blocks the Fowler/rust-regex corpora
  (External conformance corpora). Ranges and `{m,n}` are parser work; captures need a
  tagged NFA (Pike VM) — still linear, still Brood. The dogfood rule's ideal case: if it is
  slow, the VM is missing something and that is the finding.
- ⬜ **3. `iterate` / generators without unbounded laziness** *(shape)*. Not a lazy cons: a
  seqview stage `(seq/iterate f x)` consumed only by the fused `seq/l*` pipeline or a `take`,
  plus `std/stream` as the process-backed generator for anything that crosses a `send`.
  Keeps ADR-111's rule that laziness is opt-in and heap-local. Replaces the "unbounded stream
  generation" entry under Language core & types.
- ⬜ **4. Sorted map and set.** `compare` is already a total order over every value
  (ADR-285). A persistent balanced tree in `std/sorted.blsp` implementing `Seqable`/`Conjable`
  so `conj`/`into`/`get` dispatch through the collection protocol (ADR-156); the natural home
  for a range query, which `pq`/`multimap` do not cover.
- ⬜ **5. `reduced` early termination for transducers.** Without it `take`/`take-while`/
  `first`/`some` cannot be transducers. A wrapped-value sentinel checked by `transduce`'s
  loop, Clojure's exact contract; unblocks `xtake`/`xtake-while`.
- ⬜ **6. Declarative macro argument grammars** *(shape)*. Racket's `syntax-parse` is the
  best macro-error story in any Lisp; Brood has the philosophy (ADR-152) and the match
  compiler but each prelude macro hand-rolls its checks (`%for-check-binds`). Multi-pattern
  clauses on `defmacro` heads (they exist for `defn`) plus a `:hint` per clause, so a
  no-match raises with the macro's own message at the offending sub-form (ADR-297 positions
  already propagate). No expander rewrite.
- ⬜ **7. `next-impl` in abilities** *(shape)*. The precedence ladder (app > type-owner >
  ability-owner > default) has the ordering but no way for the winner to call the impl it
  shadowed, so an app override copies the type-owner's body. A `(next-impl)` form valid only
  inside an `impl` body, resolving one rung down. No `:before/:after` combination (ADR-011).
- ⬜ **8. Accumulating comprehensions.** A `:into coll` clause on `for` (reuses `conj`'s
  kind preservation) and a `for/fold`-shaped `(fold-for (acc init x xs) body)`. Prelude macros.
- ⬜ **9. Small library gaps** — each a few prelude lines: `juxt`, `fnil`, `memoize` (table-
  or process-backed; there is no atom), `cycle`, `partition-by`, `condp`, `when-some`,
  `postwalk`/`prewalk`, `pmap` (spawn + fan-in), and `format` width/justification
  (`%5d`, `%-10s`).
- ⬜ **10. Contract blame.** `BROOD_CONTRACTS=1` reports the mismatch but not the party. The
  checking shim in `std/prelude/core.blsp` knows the signature's module and the call site;
  attach `:blame :caller | :callee` to the error map.

- ⬜ **11. Contracts on by default in dev mode** — asked 2026-08-30. Blocked on three
  recorded things, in order: (a) the open ADR-153 design question — `BROOD_CONTRACTS=1`
  turns a *declaration* into a *rebinding* (placement-sensitive, wraps identity/`arglist`);
  the better shape is a kernel `def`-time hook applying registered signatures, which is
  placement-independent and reaches the prelude; (b) the prelude cannot carry contracts at
  all today (a shim captures a local frame; the freeze forbids it); (c) unmeasured JIT cost —
  a shim in front of every `sig`'d std function defeats leaf splicing and the IC on the
  hottest calls. Then: default-on under `nest run`/`nest test` only, never in a bundle
  (Racket's model). KI-81 made the flag reliable only on 2026-08-29.

**Doc drift found on the way** (fix with the first item that lands nearby): the ADR-170
freeze table still reads "Multiple dispatch — refused" against ADR-179's `defmulti`;
`docs/language.md:2341` says `/` returns a float (ADR-196), `:923` still lists `lambda`
(ADR-162), `:3199` documents `std/enum.blsp` (now `std/seq.blsp`, ADR-234);
`docs/primitives.md:329` and this file's special-form count disagree on `catch`.

### Toolchain gaps a downstream migration exposed (2026-08-27)

Migrating hive and its whole dependency closure (hatch, store-postgres, store, s3) across the
namespacing waves took the registry down **twice**. Neither outage was a language bug — both
were gaps in what the toolchain can tell you before you ship. Recorded as KI-66 and KI-67;
the roadmap items are what would remove the class.

**Status: all five are done (2026-08-27; items 1, 3, 4 and 5 are [ADR-257](docs/decisions.md);
item 2 shipped with KI-67).** Three of them were a question the toolchain simply had no
command for — *does it boot?*, *what is this binary?*, *what moved, and where to?*

- [x] **1. A boot check** (KI-66). `nest check` resolves names, `nest test` runs the suite,
      and neither loads `main` — which is where the first outage died (`unbound symbol:
      int->char`, raised *during* `require`). **`nest run --check-boot`** loads every source
      module the way a bundle does, resolves `:main`, and runs nothing; **`nest release`** then does it to the binary just written — the artifact, not the tree that
      produced it, which is the half a source check structurally cannot see. That second half
      ran only under an opt-in `--smoke` until 2026-08-29, which is the wrong default for the
      one question a release has to answer; it is now on unless `--no-smoke`, and a binary
      that fails it is deleted rather than left for a later upload to find.
      `project/check-boot` / `project/check-bundle-boot`, sharing one entry resolver with
      `run`/`run-bundle` so the check cannot drift from the boot it checks.
      Sabotage-verified in both directions (`crates/nest/tests/boot_check_and_renames.rs`).
      **It does not catch the second outage** — `os/getenv` was reached from inside a
      function `main` calls, and resolving an entry without invoking it never executes that
      line. That is the deliberate boundary: `nest check` covers an unbound name in a body,
      and `nest run --for` (already wired into hive's `bin/ci` and `package-ci.yml`'s opt-in
      `boot-check`) covers a name reached only when `main` runs. `--check-boot` is the member
      of that set that is always safe to run — no window, no port, no side effects, valid for
      a library with no runnable `:main`. See ADR-257 for the table.
- [x] **2. `nest check` inside a `try`** (KI-67). The checker deliberately skipped unbound
      symbols in a `try` body — which is where I/O lives, which is where the renamed
      primitives live; hatch shipped a dead spool write for exactly this reason. Unbound
      symbols now survive that filter while every other lint stays suppressed.
- [x] **3. A bundle says what it is.** `myapp --brood-build-info` prints the brood version,
      build-id, features, and the app + module count. Answering that meant grepping the
      binary over SSH — and the first attempt used `strings`, absent from
      `debian:bookworm-slim`, which reported 0 and read exactly like "no JIT". The
      **`--brood-` argv prefix is reserved by the runtime** (two names, first position only)
      so the bundle's "argv belongs to the app" contract costs the app nothing; it reads the
      manifest and module directory only, never loading a module, so it answers on a bundle
      that is broken — which is when it is asked.
- [x] **4. A migration aid for a rename wave.** `nest check --fix-renames` (with
      `--dry-run`) applies the unambiguous half: for each bare name reported unbound, the
      single public `mod/name` that now defines it, rewritten into this project's
      *references* via the CST — so a docstring or comment naming the same identifier comes
      back byte-identical. It declines, with the reason printed, a name defined in several
      namespaces, a name that moved behind `%` (`map-pairs` → `%map-pairs` has no hint
      *because* it was withdrawn — naming the target ends the guesswork without rewriting
      onto it), and — the hazard that cost a revert — **a name the project itself defines**:
      `nest rename` is not scope-aware, and renaming `register` in hive also renamed hive's
      OWN sign-up handler, producing the reserved `(defn proc/register …)` so the module
      stopped defining at all.
- [x] **5. `docsite/render-css` emits CSS variables.** A `:wrap? false` host embedded a
      fragment whose stylesheet hard-coded a light palette; `component-dark-css` existed but
      was emitted only by the wrapped path and gated on `prefers-color-scheme` — the
      reader's OS, not the host's theme — so hive overrode ~30 selectors by hand. The sheet
      now declares its palette as custom properties on `.docsite` and **no rule names a
      colour**, which is what makes a host's redefinition impossible to out-vote;
      `render-css-dark` hands over the dark set with no media query attached. Second
      incomplete hand-off in that API (`render-js` was the first, fixed 2026-08-26); the
      guide headings' undarkened `#1f2933` fell out as a fix.


### Type system — the audit, and the five items it ranked (2026-08-27)

**Status: done (ADR-259..263).** A probe corpus through `brood --check` separated three layers
in very different states — the lattice, the inference in front of it, and the annotation
surface feeding it — and each turned out to need a different kind of fix. Details in
[docs/type-system-status.md](docs/type-system-status.md); what is *left* is that document's
"What's left" table, which is now shorter and more specific than the backlog it replaced.

- [x] **1. `sig` fails closed, and the definition owns the arity** (ADR-259). Four shapes
      exited 0 with no diagnostic — a misspelled type, a misspelled constructor, a sig whose
      arity contradicts its `defn`, and a sig for a name never defined. The third *suppressed*
      a correct check: inside a file the declared sig was the only arity source, so a wrong one
      made a wrong call type-check clean and die at run time. A same-file call now has an arity
      check at all, read from the def site's own parameter list.
- [x] **2. The walk's totality is gated** (ADR-260). `REACH_CASES` plants an unresolvable name
      in every code position of every special form and container literal, in both walks; a
      companion test makes a *new* special form declare its own case. It found the next
      instance of the KI-67/KI-70 class immediately: a `quasiquote` template was skipped whole,
      though its `~`/`~@` escapes are code.
- [x] **3. A parameter's type is its domain** (ADR-261). A guarded use is credited *within its
      guard* and the alternatives union, so branch shapes, `match` patterns, head
      destructuring, multi-arm functions and `:when` clause guards all constrain callers with
      no annotation. Supersedes ADR-190's "a guarded use never constrains a param".
- [x] **4. A union keeps its terms** (ADR-262). `(or (tuple int) (tuple string))` used to widen
      to bare `vector`, which made the tagged-union idiom invisible to every check. Single-term
      types are byte-identical; unions that cannot merge keep up to four terms, and the five
      set operations quantify over them.
- [x] **5. `(not T)`, and complements that read as complements** (ADR-263). The lattice has
      computed complements since ADR-023 and the grammar could not say one; `expects string,
      got nil | bool | number | …` (twenty-two tags) now reads `(not string)`.


### Standard-library surface audit — the bare namespace (2026-08-26)

**Status: the reduction landed; examples and three structural items remain.** Every public
function in `std/` + the prelude (1,374 across 106 files) read against seven criteria: module
placement, duplication, documentation-with-an-example, naming consistency, fit with the
language, parity with the process model, and whether one bare ability op could replace a
family of per-type functions. ADR-250 through ADR-253 carry the decisions.

**Bare names 510 → 268** (names a top-level `def` in a script cannot use, 470 → 239):

- [x] 203 prelude-private helpers moved behind `%` (ADR-250) — they held ordinary words
      (`merge-sort`, `flip-cons`, `for-fold`, `take-acc`) that no user could call anyway
- [x] `bit/` (10) and `decimal/` (4) namespaces for kernel-primitive families (ADR-251);
      `system/*` and `string/*` for the metadata and char/width functions
- [x] `proc/register` / `proc/whereis` moved beside `proc/info`, plus the two missing halves:
      `proc/unregister` (`register` had no inverse) and `proc/alive?`
- [x] `apropos`, `doc-search` and LSP completion no longer offer `%` plumbing
- [x] `os` vs `system` boundary redrawn (2026-08-27) — they were not two modules for one
      concern but one boundary in the wrong place. **`os` is the operating system**
      (`os/env`, `os/cmd`, `os/type`, `os/spawn`); **`system` is the Brood runtime**
      (`system/argv`, `system/halt`, `system/brood-version`). `system/env` was literally
      `(os/getenv name)` — one function, two names — and the CHILD-PROCESS half lived in
      `proc`, so `proc/spawn` started an OS process while the bare `spawn` special form
      started a green one. `spawn` cannot be namespaced (it is a special form), so the OS
      half moved to `os/spawn` / `os/write` / `os/close` / `os/set-binary`
- [x] **The kernel's own bare names, read for the first time (2026-08-28, ADR-290).** Every
      pass above read `std/`; nobody had read the RUST side. Of 391 registered primitives,
      285 are `%`-hidden and 63 already namespaced, leaving **43 bare** against which no
      criterion had ever been applied — two of them `eval` and `load`. Twelve moved:
      `reflect/eval` `reflect/eval-string` `reflect/load` `reflect/global-names`
      `reflect/special-forms` `reflect/doc-forms` `reflect/builtin-modules`
      `reflect/current-ns` `reflect/dynamic?` `reflect/private?`, plus `proc/trap-exit` and
      `proc/system-monitor`. **43 bare primitives → 31.** What did NOT move was decided by
      the target modules' own headers, which both already reserved a set: `reflect` keeps the
      REPL-typed group (`doc`, `arglist`, `bound?`, `apropos`, `doc-search`, `macroexpand`
      — and `macroexpand-1` stays with it), `proc` keeps the mainstream actor model, so
      `unlink`/`demonitor` stay bare beside `link`/`monitor` rather than splitting an inverse
      pair. Two latent defects fell out: `EFFECTFUL_IN_GUARD` had six names that no longer
      named anything, and the KI-72 regression test was **vacuous under the CI gate** (it
      passed `os/exe-path`, which there is the libtest binary — `running 0 tests`, exit 0)
- [x] **The "~15 more bare names" item, resolved — it was closer to zero (2026-08-28,
      ADR-291).** Six moved (`reflect/set-load-path` `reflect/add-load-path`, and the lazy
      seq-views `seq/lmap` `seq/lfilter` `seq/lkeep` `seq/lremove`); bare prelude names
      180 → 174. The rest of the estimate dissolved on inspection: `builtin-modules` went
      with ADR-290, **`reload-defs` and `module-doc` were already `system/`-qualified** (this
      entry was stale), and the loader itself (`require-one`, `provide`, `*load-path*`,
      `*autoloading*`, `*require-parent*`) is mechanism rather than tooling — `require-one` is
      resolved by name from Rust in `eval/derive.rs`. ADR-291 records **five rules for what
      stays bare**, each from a near-miss here: a module header that already reserves the
      name; half of an inverse pair; an earmuffed global (`is_earmuffed` is a spelling rule,
      so qualifying one silently disables its own typing); a name a test depends on being
      bare (`reserved-package-name?` is the KI-72 `autoload_race` probe); and anything in
      `(special-forms)`, since a namespaced name that renders as a control keyword is a
      contradiction — which is why `for`/`doseq`/`dolist`/`dotimes`/`with-out-str`/
      `with-err-str` stay bare

**Process framework** (ADR-252):

- [x] `gen` can defer a reply — `defer` clause + `gen/reply`, so a server can hand work off
      without blocking its loop. The one gap that limited what could be *built*
- [x] `task`/`await` compose; `pq`/`multimap` get `Conjable`

**Still open:**

> Numbers below **re-measured 2026-08-29** with `scripts/stdlib-audit.blsp` rather than
> carried forward. Three of this list's claims had gone stale in the reader's favour — an
> item recorded as open can be *fixed* and still read as work, which is its own cost.

- [ ] **Examples.** `docstring, no example` is **1077**; `documented + example` is **447**;
      258 more have no docstring slot at all (`def`/`defdyn`/ability ops), so coverage is
      **447/1524 = 29.3%** — not the 16% recorded here, which predates the doctest work and
      the 38 core examples. `tests/doc_examples_test.blsp` *executes* every one, so each
      written is a test gained. A campaign, not a pass
- [x] **Ability seams: the seqable half is CLOSED (2026-08-29, ADR-295).** `count` and
      `empty?` now accept a rope and a table — a rope in CHARACTERS, so it agrees with
      `string/length` for the text it stands for (pinned by a test, multi-byte included); a
      table by entries. Sized directly rather than through `Seqable`, whose `->seq` is a list
      view a rope would have to materialise. **`(seq r)` was left alone deliberately: it was
      never the bug** — a string returns itself from `seq` and raises on `first` too, so a
      rope doing the same is consistent with the thing it models. The checker's `countable`
      gained `Rope`/`Table` in step. Superseded text follows:
- [ ] ~~**Ability seams do not reach built-in kinds** (ADR-253) — **half closed.** `Display`
      DOES reach a built-in now: `(impl Display :rope (->string [x] …))` is consulted, almost
      certainly because `->string` became an ability op in v0.15.0. What remains is the
      *seqable* family: `(count r)`, `(first r)` and `(empty? r)` raise on a rope and on a
      table, and **`(seq r)` returns the rope unchanged** — a wrong VALUE rather than an
      error, which is the one worth fixing first. The `record?` test is right on the fast
      path and wrong on the failure path, where falling through is free~~
- [x] **Naming seams — CLOSED 2026-08-29: four of the five were not defects.** Each was
      re-measured rather than scheduled, and only one survived.
      - ✅ `bytes/append` is gone (`file/spit-bytes-append`; `std/bytes.blsp:39` records it)
        and `feature?` is `system/feature?`, not the string prelude. Both already fixed.
      - ❌ **The stutters are not renameable.** All 24 come from `defrecord`, not from a
        hand-written name: `(defrecord queue (front back size))` inside `(defmodule queue)`
        emits `queue/queue-front`. Nine are the constructor case (`queue/queue`, `set/set`),
        which is the normal single-type-module pattern. The other fifteen cannot be
        shortened — `datetime/datetime-day` → `datetime/day` **collides with an existing
        polymorphic function** (verified: both answer 29 for the same value) — and cannot be
        removed or made private either: `std/` references only 5 of its 56 generated
        accessors (all in tests), but **bedit references 41 of its 46**. A real consumer
        depends on them. The only lever is a `defrecord` option for accessor naming, which
        is a feature, not a cleanup.
      - ❌ **`seq/remove-nth (i coll)` is correct and deliberate.** Index-first is the
        module-wide convention (`take`, `drop`, `chunk-every`), `std/seq.blsp:133` records
        it, and `sig`s exist specifically to catch a reversal. This entry had it backwards:
        `remove-nth`'s move *to* index-first was KI-71, which surfaced as seven unrelated
        buffer-lifecycle failures. Acting on the entry would have recreated that bug.
      - 🟡 **The one real residue is narrower than "five verbs".** `spawn`/`spawn-link` are
        core; `gen/`, `agent/`, `supervisor/` and `node/` each qualify their own, so a call
        site is never ambiguous. What genuinely reads wrong is that `gen/start`,
        `gen/start-link` and `gen/start-named` are three one-line wrappers that **do not
        compose** — there is no `start-link-named`, and `start-named`'s own docstring tells
        you to register the result of `start-link` yourself. That is a verbs-vs-options
        question (`(gen/start f state :link true :name :foo)`), not a rename, and it is a
        breaking change to the gen surface. Deferred deliberately, with the reason recorded
        so it is not re-derived as "five verbs" again.
- [ ] **The bare namespace is a FLOW, not a stock — nothing gates a new bare name.** It went
      268 → **264** across a day in which ADR-290/291 removed 18, because roughly fourteen
      arrived: of ten sampled, eight (`defbehaviour`, `conj-onto`, `lookup-get`,
      `lookup-keys`, `->seq`, `assoc-in`, `dissoc-in`, `inspect`) did not exist that morning.
      Reduction work is cancelled by ordinary feature work at about the rate it is done, so
      the next audit will re-derive this same list unless adding a bare name has to record a
      reason. This is the structural item behind all four ADRs

### Backend seams — swappable JIT / engine + perf legibility (2026-08-11)

**Status: all five items landed (2026-08-11; items 1–2 are ADR-221).** Full plan and the
plan-vs-reality corrections:
[`docs/backend-seams.md`](docs/backend-seams.md). Structural session, deliberately
chosen over a compute lever because the cheap end of the compute frontier is mined out (see
[VM & JIT](#vm--jit)'s at-rest status) while every remaining lever is a multi-session redesign —
and because the biggest of them (the X-register call convention) is a rewrite of exactly the
call protocol this work makes explicit.

Starting position, all in-code facts: Cranelift is confined to **7 backend files / 128
references** (~6.4 kLOC under `eval/compile/jit_lower*` plus the module owner in `jit/mod.rs`);
`ir.rs` is Cranelift-free, so **the IR is already the seam**; the production invocation surface
is **two places** in `jit_runtime.rs` (three calls — the background one picks between the plain
and inlined lowering). What is missing is not decoupling but a
*compile-checked contract* — the six obligations a backend must satisfy exist only as prose
across `jit-tier2.md` / `jit-optimizing-tier.md`.

- ✅ **1. `trait JitBackend` + `CraneliftBackend`** — **done 2026-08-11** (ADR-221). `jit/mod.rs`
  split into `mod`/`backend`/`rt`/`cranelift`: the `brood_rt_*` table (backend-independent ABI)
  apart from the Cranelift module owner, with six obligations documented on the trait.
  `ActiveBackend` is a `#[cfg]`-selected type alias, so dispatch stays static. Perf-neutral by
  construction, not by measurement: the backend's whole output is a `*const u8` and `lower_arm`
  runs once per arm on the background compiler thread behind a `Mutex`, so no execution path
  touches it. Dropped as speculative (ADR-011): a `name()` with no caller.
  - ✅ **Amended after review:** the trait was not the whole surface — `jit_runtime.rs` reached
    around it **four times** into the Cranelift backend's unboxed-scalar submodule. Now three
    **tiering advisories** (`may_adopt_shared_code`, `declines_inline_upgrade`,
    `note_depth_bail`), deliberately *associated functions* so consulting them per activation
    costs no `GLOBAL_JIT` lock (the price: the trait is no longer object-safe, recorded on it).
    Also fixed a real error in obligation 3, which omitted outcome **5** (the depth bail) — the
    outcome one of the advisories exists to service. Guarded by a sabotage-verified test, because
    the two Cranelift predicates it delegates to are easy to swap and either swap passes every
    other test in the tree. The lowering stays
  under `eval/compile/jit_lower*` — it reads `compile`'s private IR — which is now documented
  rather than incidental.
- ✅ **2. Hoist the decisions into `eval/compile/jit_plan.rs`** — **done 2026-08-11** (ADR-221);
  the valuable half. The cut was contiguous (`jit_lower.rs` 98–662), itself evidence the code was
  already logically layered. **Two tiers**: frame layout (`jit_spill_reserve`, `jit_ckpt_depth`,
  `non_tail_call_count`, `chunk_in_jit_subset`) ungated, because the VM sizes frames with or
  without a backend; a `jit_plan::codegen` module gated once for everything a code generator
  consults. **Four definitions became two** — the two frame-layout helpers were each defined
  twice, and `jit_lower` also carried `#[cfg(not(feature = "jit"))]` copies that could never
  compile. The profitability gate is now `plan_general_lowering() -> Result<(), BailReason>` with
  `BROOD_JIT_BAIL_TRACE=1` naming refusals. **No `LoweringPlan` struct**: the plan predicted one
  and the code disproved it — the entry point makes two choices and the order between them is
  load-bearing, so bundling them would invite the bug below.
  - ⚠️ **The trap this surfaced, and the blind spot behind it.** The first version consulted the
    gate *before* the unboxed-scalar path, whose predicate describes `fib`/`pfib` exactly. They
    would have silently stopped lowering — still correct on the VM, so the differential stayed
    40/40 and `make test` 974/974; **only a benchmark would have moved**. Nor was it visible by
    inspection: `jit_lower/i64.rs` emitted **no `[jit-ir]` line at all**, so the scalar path was
    invisible to the tool CLAUDE.md points at for "did this arm ever lower?", where absence is the
    documented signal. Both fixed (the path reports `scalar-register: i64|f64`), and
    **`scripts/jit-lower-witness.sh`** is the new gate: the sorted *set* of arm fingerprints, not
    the count (installation is async — ±2 on a 78-lowering sweep — while the set is deterministic).
    Item 1 diffed empty; item 2 was 0 removed / 2 added, the additions being `fib` and `pfib`
    becoming visible for the first time.
- ✅ **3. `enum Engine`** — **done 2026-08-11.** `bool` → `TreeWalker`/`Bytecode`;
  `vm_enabled()` → `active_engine()`. The generalization that matters is **`Engine::ALL`** +
  **`Engine::short()`**: `benches/eval.rs` had its own local `Eng { Vm, Tw }` and
  `tests/{differential,gabriel_engines}.rs` each hardcoded the pair — all three now iterate
  `Engine::ALL`, so a new engine gets bench rows and inherits the differential + the Gabriel
  corpus untouched, and `bench_ratio.py` (a two-engine regex by construction) reports every
  engine as its own column. Seven `vm_enabled()` sites became one `run_on_active_engine` (three
  were byte-identical) plus exhaustive `match`es where they genuinely differ — so a third engine
  cannot silently collapse to "not the VM" and tree-walk. Honest limit documented on the enum:
  `ir.rs` is shared by both engines, the JIT *and* the deopt/journal protocol, so "swap the VM"
  means replacing `exec_chunk.rs` + `vm_run_bc.rs` (~2 kLOC) while keeping that IR.
  - 📌 Corrected a wrong claim in `docs/benchmarking.md` en route: it said `(Vm, N)`/`(Tw, N)`
    appear as *neighbouring* rows. Divan sorts rows by label, so they never did; the property
    that makes the ratio load-robust is *one process*, and `bench_ratio.py` pairs by
    `(bench, size)` regardless of print order.
- ✅ **4. One-command perf triage** — **done 2026-08-11.** `make perf-brood` (counter-armed
  build, `release-brood`'s flags + `perf-stats`, so they cannot drift from what it is compared
  against); **`std/tool/perf.blsp`** — `(perf/report)` / `(perf/summary)` carrying
  `docs/benchmarking.md` §2's dispatch-/env-/alloc-bound rules, where a ratio is **nil, not
  zero**, without samples and a non-perf-stats binary yields `:no-data` with a hint rather than
  a claim; and **`brood --debug-flags`** (`crates/lisp/src/debug_flags.rs`), the `BROOD_*`
  catalogue grouped for triage, with a test asserting every catalogued name still exists in the
  source so a rename cannot leave a line telling you to set something the runtime ignores.
- ✅ **5. Perf verdict against a measured noise floor** — **done 2026-08-11**, with one part
  deliberately not built. `scripts/ab-bench.sh --json <path>` emits machine-readable rows;
  `--floor` measures each row's own base-vs-base spread and the `verdict` column calls a
  regression only when the delta clears `max(5%, 2 × floor)` — CLAUDE.md's own prescription,
  which exists because a +5.3% "confirmed" regression was a baseline that had wandered ~10%
  across a day (the same change read +0.9% against a +0.5% floor).
  - ⛔ **The committed baseline keyed by release tag was NOT built, on purpose.** Absolute ms
    drift 10–20% between runs here and do not compare across machines at all, so a stored
    number would be a false reference inviting exactly the comparison this repo already knows
    is invalid. Left as an open question rather than shipped misleading. Still not a blocking
    CI gate: per-row drift would make a hard threshold a flake generator.

Gate for 1–2 is the **full JIT gate every increment** despite both being behaviour-preserving
(moving-GC codegen: "it only moved code" is the claim that needs checking, not asserting), plus
an identical `BROOD_JIT_DUMP_IR` compile count before/after — the sharpest witness that the
decisions moved without changing. Non-goals: no second backend, no third engine, no change to
generated code or to any of the hoisted decisions.

### Syntax review — ✅ COMPLETE (2026-07-26)

A review of the *surface* for orthogonality, convention, and "the Lisp features people
expect", probed against the running binary rather than the docs. The verdict on the
core held up: 8 special forms, one pattern grammar at every binding site,
`count`/`empty?` universal. **Every filed item is now either shipped or decided:**

Shipped as ADRs:
- ✅ **[ADR-156](docs/decisions.md)** — the set/map collection-protocol holes, the
  range-in-cons-tail printer bug, the two silently-misread pattern shapes, and
  `partial`/`complement`/`constantly`/`case`/`comment`/`vec`/`disj`/`nan?`.
- ✅ **ADR-158** — protocols promoted from the `hatch` package into `std/protocol.blsp`
  (`defprotocol`/`defimpl`/`defbehaviour`), the open-dispatch answer. The kernel had
  carried the checker/LSP conformance pass for months while the macros lived
  downstream. **Superseded by ADR-168:** `defprotocol`/`defimpl` dispatched only on
  `type-of`, so no two records could dispatch apart; they were retired in favour of
  the ability system (`defability`/`impl`/`defrecord`, nominal dispatch), now **core
  in the prelude**. `defbehaviour` stays in `std/protocol.blsp`.
  - ✅ **The display protocol** (ADR-171, 2026-07-28) — the first ability for
    open-extension rendering: `Display`/`->string` (Elixir's `String.Chars`), with a
    zero-cost prelude `*show*` hook so the screen printers let a record define how it
    prints. **Now core and always on** (slice 6 below): a record customizes printing
    with just `(impl Display …)` — no `(require 'show)`, no activation step.
  - ✅ **`std/` adopts abilities** ([ADR-177](docs/decisions.md), 2026-07-29) — a second,
    wider audit than ADR-171's (which found only two candidates by asking solely about
    third-party extension). Shipped six abilities — `JsonEncode` (the ADR-171 follow-up),
    `Dependency` (`:sealed`, replacing five scattered `:kind` cond chains in the package
    manager — `nest check` now reports a missing op), `Port` (`std/io`'s documented
    "richer port value" seam), `LogBackend`, `Response` (`std/net/http`), `Temporal`
    (`:sealed`) — plus `defrecord` identities for five value types that were previously
    identified by structural sniffing (`buffer`, `queue`, `pq`, `multimap`, the temporal
    types). ADR-177 also records the **rejection list** (the prelude's collection
    protocol, `str`/`pr-str`, closed AST/CST node kinds, telemetry metric kinds,
    `std/stream`, the `proc/*` module contracts) so the next pass doesn't re-litigate it.
  - ✅ **Provided ops (default method bodies)** ([ADR-185](docs/decisions.md), 2026-07-29)
    — an op spec may carry a body (`(op [args] :-> ret? body…)`), which `defability`
    registers as the op's `:default` impl. Implement the required ops, inherit the derived
    ones (Rust/Haskell provided methods, Elixir's derived defaults); an id-keyed impl
    overrides a default. A prelude-macro change over the existing `:default` mechanism — no
    new special form — plus a checker adjustment (provided ops excluded from per-impl
    completeness and `:sealed` exhaustiveness; required ops still demanded). **[prelude/checker]**
  - ✅ **Deriving — `:derives`** ([ADR-185](docs/decisions.md) part 2, 2026-07-29) — a
    `defability` declares a `:derive-record` recipe (each ability decides how it derives
    itself, Elixir `@derive` / Rust `#[derive]`), and a `defrecord` opts in with
    `:derives [A …]`. Derivation runs at **load** (`derive-into`), not expansion — the
    checker macro-expands without evaluating, so an expand-time recipe call would break it;
    a checker pass reads the `derive-into` forms so a derived record still satisfies
    call-site and `:sealed` checks. Composes with provided ops (derive the required op,
    inherit the rest). **[prelude/checker]**
  - ✅ **Record patterns + sealed-match exhaustiveness** ([ADR-187](docs/decisions.md), 2026-07-29)
    — the biggest structural gap from the most-loved-languages review (Rust/Gleam/Elm-style
    match-on-a-constructor). **Part 1: `(record name {map-pattern})`** — matches a `defrecord`
    value by nominal id (derived syntactically, checker-safe) then a map pattern against its
    fields (`{:k p}`/`:keys`/`:or` compose); keyword-field not positional. **Part 2:
    exhaustiveness** — a `match` on a sealed-ability-typed scrutinee (declared or inferred)
    warns for any uncovered member with no catch-all. Sound by construction. **[prelude/checker]**
  - ✅ **Occurrence typing — inferred params check callers** ([ADR-190](docs/decisions.md),
    2026-07-30) — an unannotated function's inferred parameter types flag wrong callers, incl.
    the sealed-op-derived case, cross-file. Sound (under-constrained → under-warn). **[checker]**
  - ✅ **Ability bounds** ([ADR-192](docs/decisions.md)) — a sealed ability name in a `sig` is
    `where T: Ability`; `(and A B)` is `T: A + B`. Documented (already worked via ADR-181/186).
  - ✅ **Super-abilities — `:requires`** ([ADR-193](docs/decisions.md), 2026-07-30) — an
    implementor of an ability must also implement its declared prerequisites (`Ord :requires
    [Eq]`, Rust's `trait Ord: Eq`), enforced by the checker. **[prelude/checker]**
  - 📥 **Deferred ability/type abstractions** (from the loved-languages review, ADR-011 —
    ship when a concrete need names them; all *sound* to skip, none a correctness gap):
    - ⬜ *low* — **Parametric / associated abilities** (`(Collection E)`, Haskell/Rust
      associated types). Highest cost, lowest value for a *dynamic gradual* language (containers
      duck-type; `?A` sig vars + `(list T)` cover most). Defer hardest.
    - ⬜ *low-med* — **Custom match extractors / view patterns** (F# active patterns, Scala
      `unapply`). Match on computed meaning, not just shape. `:when` guards + record patterns
      cover the common case; wait for a parsing-heavy need.
    - ✅ **Exhaustiveness from *inferred* scrutinees** (2026-08-28) — a `match` whose scrutinee
      is typed by inference rather than by a `sig` gets exhaustiveness: `(defn pick (b) (if b
      :ok :err))` then `(match (pick b) (:ok 1))` reports the missing `:err`, the same as when
      a `(sig …)` declares the union.
    - ✅ **Per-arm parameter checking of a multi-arity callee** (2026-08-28) — closed by the
      clause-guard overload inference (ADR-226/261 path, `infer_overload_from_clauses`). A
      multi-arity `defn` now selects the arm by argument count and checks against *that* arm:
      `(defn greet ((name) …) ((name n) …))` flags both `(greet 42)` and `(greet "x" "y")`,
      naming the arm's own types, while the correct calls stay silent.
    - 🚫 *won't* — **Open-ability bounds** — declined, not deferred: an open ability accepts
      late impls, so no argument is soundly rejectable on the type (ADR-123/124). Safety lives
      at the op call site instead.
  - ✅ **Checker gap (resolved, note was stale):** a `:use`d ability op from a *loose
    disk* module used to be flagged `unbound symbol` though it ran. No longer reproduces
    on any current checker entry point — `brood --check <file>`, `nest check <file>`, or
    an in-project `nest check` all resolve the imported op cleanly. Closed by the
    combination of `check_file` evaluating the `(defmodule … (:use …))` header (so the
    checker `require`s the provider and sees `mod/op` as a real global, exactly the
    runtime's view), the checker reading the live ability registries (ADR-186), and the
    KI-24 resolver hardening. Guarded by `crates/cli/tests/checker_cross_module_ability.rs`
    (a faithful reconstruction of the `show` cross-module shape: loose-disk provider +
    `:use` consumer + op call). **[kernel/checker]**
  - 🟡 **Abilities v2** ([ADR-172](docs/decisions.md), design decided 2026-07-28,
    **amended 2026-07-28**) — makes ADR-168's open runtime model **deterministic and
    app-sovereign** without closing it. **Amendment: the orphan rule and the `bridge`
    form are dropped before implementation** — abilities stay OPEN (`impl` legal for any
    ability and any id, incl. primitives and unowned types), because `bridge` had zero
    runtime substance (it expands to the same `register-impl` as `impl`) and the orphan
    restriction guards a multi-third-party-library collision greenfield Brood doesn't
    have. App sovereignty is delivered by the **precedence ladder alone**: `app >
    type-owner > ability-owner > other`, same-tier cross-module collisions warned. If a
    real orphan conflict ever appears, orphan-authorization becomes a **lint on plain
    `impl`** (app/library line is already computable via package identity), advisory-live
    / hard-CI — no new form. What remains: dispatch specialized through the IC/JIT with
    deopt-on-reload and `:sealed` fully static/exhaustive (slice 5). **The ability system
    itself is now core** — folded into the prelude, so `defability`/`impl`/`defrecord`
    are always available, no `(:use ability)`. **[kernel/checker/eval]**
    - ✅ **Slice 1 — optional + dev dependencies** (2026-07-28): the manifest takes
      `:optional true` per dep and a `:dev-dependencies` list (tagged `:dev true`, own
      slot). **Slice 1b** wired dev-deps end to end: `project--ensure-deps-on-path`
      load-paths them for dev/`nest test`, `bundle-collect` excludes them from a release
      bundle (verified with a scratch project).
    - ❌ **Slices 2 + 3 — `bridge` + coherence checking: dropped** (2026-07-28
      amendment). No orphan rule, so no orphan escape hatch to build; `impl` stays open.
    - ✅ **Slice 4 — deterministic precedence, all four tiers** (2026-07-28):
      `std/ability.blsp` resolves competing impls by tier (**app > type-owner >
      ability-owner > other**), not load order — `defability` records its owner ns,
      `register-impl` keeps the highest-tier impl per slot (a guard at registration;
      dispatch stays a plain map-get). The top **app** tier shipped via **package
      identity** rather than waiting for full ADR-070 name-prefixing: a `defdyn
      *ns-package*` maps each namespace to its owning package's name (static scan at
      project setup), and a ns whose package is `*project-name*` — or has no owner
      (root/REPL) — is the app. `ns-package`/`trace-with-packages` also tag stack frames
      with their owning package.
    - 🟡 **Slice 5 — dispatch specialization** (2026-07-28): the **inline cache** shipped —
      the `%dispatch` kernel primitive backs ability dispatch with a per-op, epoch-validated
      cache (`ic[op-key] = (epoch, id, fn)`), so a hot monomorphic call skips `impl-for`'s
      two CHAMP lookups. The shared `global_epoch` (bumped by `register-impl`'s `def *impls*`
      and by RUNTIME compaction) makes it reload-safe, GC-safe (verified: debug tripwire +
      heap-verifier clean under `GC_STRESS`), and cross-process-correct — no new invalidation
      machinery — and it's invisible to the language (a pure memo of `impl-for`). Dispatch
      overhead vs a direct call roughly halved. Still ⬜: compile-time *static* resolution
      where the receiver type is known, and `:sealed` → a closed exhaustive switch.
    - ✅ **Slice 6 — `Display` core, always on** (2026-07-28): the ability system +
      `Display`/`Inspect` folded into the prelude; the prelude wires `*show*` on by
      default. A record customizes printing with just `(impl Display …)` — no
      `(require 'show)`, no `display-on`. `std/ability.blsp` + `std/show.blsp` deleted
      (their content is now prelude); the `Interp` needs no per-runtime load. **Records
      unified**: `defrecord`/`defrecord*` collapsed into one identity-carrying `defrecord`
      (constructor + accessors + nominal id + dispatch); records are now nominal (not `=`
      to a bare map). The star is gone.
- ✅ **ADR-159** — grapheme-*indexed* accessors (`grapheme-count`, `grapheme-at`,
  `substring-graphemes`), so the documented-correct cursor step stops costing a vector
  of every cluster in the string per keystroke.
- ✅ **ADR-160** — `(or …)` / `(and …)` patterns and general `{key subpattern}` map
  patterns; `and` doubles as the `:as` capture. `(not …)` and map `:as` stay rejected.
- ✅ **ADR-161** — transducers as public surface (`seq/transduce` + `seq/xmap`/`xfilter`/
  `xremove`/`xkeep`), so a user can write their own fusing stage.
- ✅ **ADR-162** — the `lambda` alias retired; `fn` is the only spelling.
- ✅ **ADR-163** — the convention questions settled *as decisions*: no `&key` (a
  trailing options map is the rule), `fold`+`reduce` both stay documented,
  `cond`'s bare `else` stays, `!`'s three meanings documented, naming lineage is
  "best name for the job" + `apropos`, the failure convention is throw-for-bugs /
  tagged-value-for-expected, and the reader gaps are documented rather than changed.
  (**ADR-310 settled the second half**: an expected failure is a `failure` *value* —
  its own falsy kind carrying a message — not a tagged tuple, which does not thread
  through `->`. `string/->number`, the `encoding` decoders, `datetime/parse-*` and
  `url/percent-decode` return one; raising stays the bug/unexpected channel.)
- ✅ Also landed: `dissoc-in` (completing `get-in`/`assoc-in`/`update-in`), `for`
  taking multiple body forms like every other iteration form, and a hint-table audit
  (five hints named features that didn't exist).

What that review consciously left OPEN, with the reasoning:

- ✅ **Callable keywords — `(:key m)`** (ADR-165, 2026-07-26). Keyword-only, in the
  shared `eval::apply`, so a keyword is a first-class value the HOFs can take
  (`(map :name people)`). Map/vector/set stay non-callable by decision.
- ⬜ **Early-bind reserved names now that they can't be rebound (ADR-166).** The
  reserved set makes a shipped binding *immutable*, so the compiler can resolve
  `get`/`+`/`first` at compile time and the JIT can inline them **without a staleness
  guard** — the `PrimOp1` epoch guard is already unreachable for its original purpose
  (every prim it covers is reserved). This is Erlang's local-vs-remote-call
  optimisation arriving by the same insight, and it is the mechanism behind the item
  below rather than a separate idea. The checker can likewise stop typing reserved
  globals as `dynamic()` and give them precise types. **[kernel/JIT]**
  - 📋 **Phase 1 is planned** (2026-07-30, held pending the concurrent JIT perf thread):
    compile-time resolution of reserved-defn call heads + drop the staleness guard +
    checker precise-typing. Concrete, step-ordered, ready to execute — see
    [`docs/plan-reserved-early-binding.md`](docs/plan-reserved-early-binding.md). Phases 2
    (multi-arity devirt) and 3 (see-through inlining of `get`'s `cond`) build on it.
- ⬜ **`get`'s call + type-dispatch overhead, which the JIT cannot see through.**
  Found while measuring ADR-165: against the `map-get` kernel op at 107 ms/1M, a
  single-arity Brood wrapper costs **+124 ms**, its four-branch `cond` a further
  **+138 ms**, and multi-arity dispatch **+24 ms** — total 393 ms, with the JIT
  closing *none* of it (374 ms with the JIT off). This is the variadic-`+` finding
  again (the one that motivated multi-arity dispatch, see CLAUDE.md): the fix is a
  language capability — cheaper closure calls and/or type-dispatch the JIT can lower
  — not moving more accessors into Rust, which only hides it. `get` is ~4,800 call
  sites, so this is the single highest-leverage perf item in the library.
  **[kernel/JIT]**
- ⬜ **`sig` inline signatures.** A function's name, params and types live in two
  forms with an ordering constraint that only bites under `BROOD_CONTRACTS=1`. The
  fix — `(defn f ((x int) -> int) …)` — is an ADR-082 revision touching `defn`, the
  checker's `sig_of`, `defrecord`'s emitted sigs, `sig!`'s wrapping, and every `sig`
  in `std/`. Deferred with the reasoning in ADR-163, not dismissed. **[Brood]**
- 🟡 **Re-host the seq protocol on abilities** (2026-07-29). The **read/iteration half
  shipped**: a `Seqable` ability (op `->seq`, default = a record's fields id-free) that
  `seq` — and so `map`/`filter`/`fold`/`for`/`into`, plus `count`/`keys`/`vals` — consults
  for a RECORD, so a custom-collection record defines its own iteration and joins the
  protocol. Hybrid, à la `Display`: built-ins keep their native fast path (the `Seqable`
  branch fires only for a record, detected by one `:__id__` check, once per collection op,
  not per element), so zero cost to lists/vectors/maps. This also **fixed the `:__id__`
  leak** unified `defrecord` introduced — `(count r)`/`(keys r)`/`(seq r)` are now the
  field view, not the raw map (which included `:__id__`). The **build half** also shipped:
  a `Conjable` ability so `conj`/`into` (both prelude defns) dispatch for a record —
  default is the map behaviour, a custom collection defines its own insertion. **Dogfooded
  onto std**: `std/queue` and `std/multimap` now impl the protocol, so a queue/multimap is
  a first-class collection (`count`/`seq`/`map`/`fold`/`for`/`conj`/`into` all work) and
  their bespoke `queue-to-list`/`queue-from-list`/`multimap-size` collapsed into one-liners
  over it. Still ⬜: the Prim1 accessors (`first`/`rest`/`empty?`/`nth`) don't route
  through `Seqable` — they're JIT-inlined ops the hot `fold--loop` uses, so routing them
  needs kernel work (or raw `%first`/`%rest`); use `(first (seq c))` meanwhile — plus an
  optional `Counted` for O(1) `count`. **[Brood]**
- ✅ **Numeric protocol — arithmetic for records (`Num`)** (2026-07-29). A money value,
  complex number, 2-D vector, or bignum uses `+`/`-`/`*`/`/` via a `Num` ability
  (`num-add`/`num-sub`/`num-mul`/`num-div`). A Brood-side attempt (a `(record? a)` branch in
  `+`'s binary arm) was ❌ first — it defeats the JIT's arithmetic specialization, a **~195×
  fib regression** (60 ms → 11.7 s) — so it's done in the **kernel**: the `%add`/`%sub`/
  `%mul`/`%div` builtins dispatch `Num` only from their COLD non-numeric fallback (a record
  operand → apply the matching `num-*` op via `apply_value`). The inlined int/float path
  never reaches it — **fib stays 61 ms, zero regression**. Checker widened: `+`/`-`/`*`/`/`
  accept `number | map` (a record is a map), so `(+ money money)` and `(get (+ a b) :field)`
  type-check while `(+ "a" 1)` is still caught, and pure-numeric results stay precisely typed
  (`numeric_call_ty` handles those; the widened sig only applies once an operand is a
  record). No `:default` — a record with no `Num` impl raises `ability Num/num-add: no impl
  for :ns/rec`. **[kernel/checker]**
- ✅ **Record-shape dispatch** — resolved by **ADR-168**. Records stay structural maps
  (ADR-130 intact: `type-of` is still `:map`, `get`/`assoc`/`=` still structural), and
  a `defrecord` value carries a *dispatch-only* `:module/name` nominal identity baked
  in at definition, so two record shapes dispatch apart. The rejected `:type`-field
  axis stays rejected: it would silently reroute any map carrying a `:type` key, where
  a `defrecord` identity is explicit and construction-time.
- ⬜ **Transducer early termination** (`reduced`) and stateful-stage lifecycle.
  ADR-161 ships the one-arity contract `fold` needs; `take`-as-a-stage wants a
  `reduced` sentinel threaded through `fold`, the library's hottest function.
  **[Brood]**
- ⬜ **A rope-level grapheme cursor.** ADR-159's three accessors unblock correctness
  everywhere; a large buffer still wants a cursor that caches the segmentation.
  Size it against a real editor workload. **[kernel]**
- ⬜ **`contains?` on a vector/list, `first`/`map` on a string** — the two remaining
  ✗ in the collection matrix, both deliberately left erroring (ADR-156): `contains?`
  on a vector would have to answer by *index* (Clojure's trap), and a string would
  have to pick codepoint vs grapheme for the caller. **[Brood]**
- ✅ **`#|…|#` now says "Brood has no block comments"** (ADR-169, 2026-07-27) rather
  than reading as the bar-quoted symbol `|#\|…\|#|`. Folded into the broader reader
  reservation: `#` is a dispatch character (`#{…}` / `#b"…"` its only forms) and a
  digit-led non-number token (`1/2`, `0x1F`, `1_000`, `1N`, `1+`) is a reader error,
  not a symbol — closing the freeze gate's §2 (reader's permanent reservations). **[kernel]**
- ⬜ **Unbounded laziness** (`iterate`, `lazy-seq`) stays rejected — seq-views plus
  processes cover it, and `Value::Lazy` adds a GC story, force semantics, and
  head-holding pitfalls. Recorded in [deferred.md](docs/deferred.md) #2.

### `jit_lower_arm_inner` emit-loop decomposition — ✅ COMPLETE (2026-07-25)

The dependency-ordered continuation of the Tier-1 item-1 split (see "Structural /
code-organization cleanup" below for what's landed: `i64.rs`/`prepass.rs`/`emit.rs`,
`Op` at module scope, the `Frame` context, and the arith/scalar/slot-kind helpers —
`jit_lower.rs` 5437 → 4308). The pattern is proven; each step is a behaviour-identical
closure→free-fn relocation into `jit_lower/emit.rs` (or a new sibling), verified with
**differential 2/2 + jit 34/34 (JIT_VERIFY) + full suite** per step. **All steps below
are now done — `jit_lower.rs` is 5437 → 2271, with the per-`Inst` arm bodies in
`call.rs`/`prim.rs`/`control.rs`.** Ordered by dependency:

- ✅ **Batch 5 — operand-materialization family** (done 2026-07-25): `read_words`,
  `store_words`, `as_int`, `as_block_arg`, `as_f64`, and `store_op` moved to `emit.rs`
  as free fns taking the `Frame` context (extended with `slot_f64_cache`). Kept thin
  one-line delegating closures at the original site so the ~35 call sites in the emit
  loop stay byte-identical — zero call-site churn, zero codegen change. `exit_done`
  stays a 2-line local closure (it needs the arm-local `done_block`) and now delegates
  to `emit::store_op`.
- ✅ **`Funcs` struct** (done 2026-07-25) — a `Copy` runtime-call context bundling the
  heap ptr, out-slot, target pointer type, the arm's `error` block, and the
  vector-slab `FuncRef`s (`vnbase`/`vobase`/`vref`), threaded alongside `Frame`. Grows
  with more `FuncRef`s as the arm-body extraction proceeds.
- ✅ **Big helpers** (done 2026-07-25): `store_op`, `call_handle`, `vector_ref`
  (~177 lines), `table_prim`, `eq_dispatch` (~239) all moved to `emit.rs`, taking
  `(&mut FunctionBuilder, …, Frame, Funcs)`; delegating wrappers keep the call sites
  unchanged. `jit_lower.rs` 4308 → 3785; `emit.rs` 273 → 923. Verified: differential
  2/2, jit 34/34 (incl. `GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY`), full `make test`
  811/811, and JIT vs `BROOD_VM=0` output bit-identical across arith/float/vector-ref/
  keyword-eq.
- ✅ **Per-`Inst` arm bodies** (done 2026-07-25): Call + SelfCall →
  `jit_lower/call.rs`; Prim1/MakeVector/Prim3 + fused Prim2/Prim2SlotSlot/Prim2SlotInt
  → `jit_lower/prim.rs`; Jump/JumpIfFalse + `record_block_flags` →
  `jit_lower/control.rs`. `Funcs` grew to carry every runtime-call `FuncRef` (+ shared
  `TICK_BATCH`); `inline_vec_ref` moved to `emit.rs`; the operand `stack`, `spill_next`,
  and `bool_param` thread through as explicit params. Each `Inst` arm became
  `emit_<inst>(&mut b, &mut stack, …, frame, funcs)` (`Call` returns a `Flow` so the
  caller keeps the tail `break`; the rest return `Option<()>`). The trivial leader arms
  (`Const`/`Local`/`Global`/`Pop`/`SetLocal`) stay inline as scoped. Every move was
  behaviour-identical (closure calls → direct `emit::` calls). `jit_lower.rs` 3785 →
  2271. Verified per family (differential 2/2 + jit 34/34), then the whole split under
  `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 BROOD_JIT_VERIFY=1` (36/36), full `make test`
  846/846 + doctest, and JIT vs `BROOD_VM=0` vs `BROOD_NO_JIT=1` output bit-identical on
  an arm-exercising program (`BROOD_JIT_DUMP_IR` confirmed `sum-to`/`fold--loop`/… tier
  through the new code). **The `jit_lower_arm_inner` emit-loop decomposition is
  complete.** **[kernel/JIT]**

### Structural / code-organization cleanup (2026-07-24)

Findings from a full-repo structural review (9 parallel reviewers over all of
`crates/` + `std/`). **Verdict: the codebase is well-structured** — coherent
module boundaries, disciplined naming (zero `do_`/`normalize_`), no dead-code
piles / commented-out blocks / TODO rot, clean feature-gating, the two-parser and
tiered-evaluator hazards both *managed*. These are "sharpen a good thing" items,
none urgent. Ranked by payoff. All **[kernel]** unless marked.

**Tier 1 — maintainability hazards (real payoff):**

1. 🟡 **Split `eval/compile/jit_lower.rs` (5437 → 4576)** — partial, done
   2026-07-24. Extracted the self-contained **unboxed i64/f64 scalar worker**
   (`Scalar`/`I64Ctx`, the `i64_*`/`lower_i64_*` family, and `jit_lower_i64_arm`,
   ~860 lines) — a cluster used only by the `jit_lower_arm` dispatcher, never by
   `jit_lower_arm_inner` — into `eval/compile/jit_lower/i64.rs`, re-exported so the
   tiering glue's `jit_lower::…` paths are unchanged. Behaviour-identical (a
   relocation of independent fns). Then (2026-07-24) started the
   `jit_lower_arm_inner` decomposition itself: extracted the pure, Cranelift-free
   **pre-lowering analysis** (block-leader + operand-stack-depth abstract interp)
   into `eval/compile/jit_lower/prepass.rs::block_analysis` — data-in/data-out, no
   CLIF emitted, verified behaviour-identical. Then did the key **enabling refactor**
   for the emit loop: moved the fn-local `Op` operand-model enum to module scope
   (so extracted helpers can name it) and pulled the arithmetic emitters
   (`emit_arith`/`emit_float_arith`) into `eval/compile/jit_lower/emit.rs` as free
   fns taking `(&mut FunctionBuilder, …, deopt)` — the proven pattern for the rest.
   Then extracted the scalar slot-access helpers (`box_scalar`/`load_slot_int`/
   `store_int`/`copy_value`) into `emit.rs` too, threading a `Copy` `Frame`
   context (`rb_var`/`base`/`nslots`/`deopt`/`carry_vars`).
   `jit_lower.rs` now 5437 → 4325 with `i64.rs`/`prepass.rs`/`emit.rs` split out.
   🟡 The remaining **emit-loop decomposition** — the ~1,600-line
   `for ip in 0..len` CLIF loop (Call ~300, Prim1/2/3 + fused ~700, SelfCall ~200,
   control ~130) over the virtualized `Op` stack — is the high-risk remainder.
   Every arm shares `b` (the `FunctionBuilder`, which can't move into a struct — it
   borrows `ctx.func`), the `stack`, ~30 `FuncRef`s, and the hoist maps, so a family
   extraction needs a pervasive `LowerCtx { stack, funcs, hoisted, … }` refactor
   (helpers take `(&mut FunctionBuilder, &mut LowerCtx, …)`) applied across the
   whole loop before the first helper can move — an all-or-nothing change to
   JIT-critical code where a subtle miscompile passes the tests. Deferred to a
   focused pass with per-family JIT-differential + `BROOD_JIT_DUMP_IR` + benchmark
   verification.
2. ✅ **Split `process/scheduler.rs` (2080 → 1088)** — done 2026-07-24. The key
   insight that unlocked it: **keep every shared static in the root** and relocate
   only *functions* — then each child reaches the state via `use super::*`, so no
   accessor layer is needed and the move is behaviour-identical (statics don't
   move; the reduction budget, run queue, pid tables all stay put). Three child
   modules under `process/scheduler/`: **`guards.rs`** (GC-block/macro-block depth
   + RAII guards + the stack-overflow byte guard), **`lifecycle.rs`** (spawn /
   exit / deregister — process birth & death), **`pool.rs`** (run queue +
   stealing + the `worker_loop`/`run_one`/`finish_quantum`/`handle_capture_outcome`
   execution loop). The public/`process`-facing surface is re-exported from
   `scheduler.rs`, so every `crate::process::…` call site is unchanged. The root
   keeps the reduction/preemption core + capture-driver glue + `Process`/`Ctx` +
   the shared statics (the coherent "scheduling state" nucleus). Verified: compiles
   default + no-default-features; full suite 3071/3071.
3. ✅ **Split `core/heap/gc.rs` (4520 → 2689)** — done 2026-07-24. Extracted the
   RUNTIME shared-region collector (ADR-091 — two-generation aging, single-process
   compaction, node-liveness drain, live-globals migration + the `RuntimeForward`/
   `flush_rt_*` helpers, ~1830 lines) to `heap/gc_runtime.rs`. The two regions were
   self-contained (no cross-calls into the LOCAL collector), so behaviour-identical.
   Verified: 21 heap tests under `GC_STRESS`+`GC_VERIFY`; suite green.
4. ✅ **`register()`/`PRIMITIVE_DOCS` drift guard** (`builtins/mod.rs`) — done
   2026-07-24. Added a unit test that registers every primitive into a fresh env,
   enumerates the natives, and asserts every **user-facing** (non-`%`) primitive
   has a `PRIMITIVE_DOCS` entry and no doc entry is an orphan. Surfaced 12
   genuinely-undocumented user-facing primitives (`bytes`/`byte-at`/`byte-length`/
   `bytes->list`/`bytes-concat`/`bytes-index-of`/`subbytes`, `max`/`min`,
   `current-ns`/`seqview?`/`demonitor-node`) — docs added. `%`-prefixed internal
   ops stay exempt (wrapped by prelude fns/macros). **[kernel]**

**Tier 2 — real duplication to dedupe: 5/6/7/9 done 2026-07-24 (suite 2985/2985,
`nest check` zero-warning, 238 type-lattice Rust tests green). 8 deferred with
rationale (below).**

5. ✅ **`types/mod.rs` 4-way literal-refinement copy-paste** — the per-kind blocks
   in `union`/`intersect`/`is_subtype`/`is_disjoint`/`negate` now go through four
   generic helpers (`merge_union_lit_set`/`intersect_lit_set`/`lit_is_subtype`/
   `lit_disjoint`, `T: Ord + Clone`), and all ten `Ty` constructors use
   struct-update over `Ty::flat(tags)`. ~250 lines removed; behaviour identical.
6. ✅ **`lib.rs` `eval_str`/`eval_source` near-dups** — factored the private
   `eval_forms(Vec<(Value, Option<Pos>)>)` core carrying the load-bearing
   GC-rooting/namespace/reset logic once; the two public fns are now 3-line
   adapters. The restore now runs exactly once on every path (was mirrored).
7. ✅ **`std/` path + url duplication** **[Brood]** — **url:** `net/http`'s
   `parse-url` now wraps `url/parse-url` (the one RFC-3986 parser) + HTTP defaults,
   instead of a lossy reimpl. **path:** resolved as *not* true duplication — a
   deliberate keep-both. `path.blsp` is the full public path API; the prelude
   `path-*` subset is the necessary bootstrap layer (modules aren't loadable at
   boot, and the two have different contracts — `path/basename` strips a trailing
   slash, `path-basename` doesn't; `path/join` is variadic with absolute-reset,
   `path-join` is 2-arg). Documented as such in both files.
8. ❌ **`gui.rs` ↔ `gui_gpu.rs`** — **descoped (not a cleanup).** `gui_gpu.rs` is a
   declared *prototype* that doesn't even draw text yet ("Text is not yet drawn");
   the missing `Cursor`/`ScrollRegion`/underline are *unimplemented GPU features*, not
   diverged geometry. Adding them is display-gated **feature work** blocked on GPU text
   rendering — it belongs to the GPU-window frontend milestone (Editor M3), not this
   structural-cleanup list. Removed as a cleanup item.
9. ✅ **`eval/compile/inline.rs` `node_*` predicate family** — collapsed
   `node_has_selfcall`/`node_has_self_call`/`node_has_make_closure` into one
   generic `node_any(node, &pred)` combinator.

**Tier 3 — quick wins (verified, safe/mechanical): ✅ all done 2026-07-24.**
(Suite 2979/2979 green; built with gui+treesit-grammars+jit.)

- ✅ Deleted dead `parse_jobs_args` (`cli_support.rs`) and `Scanner::set_pos`
  (`syntax/scanner.rs`) — both had zero callers.
- ✅ Fixed doc-comment misattachments: the crash-dump doc now sits on
  `install_crash_dump` (was on `fmt_utc_ms`); `syntax/cst.rs` string paragraph
  moved onto `fn string`; the orphaned `mailbox-size` doc removed from
  `terminal.rs` (the primitive is documented in `PRIMITIVE_DOCS` + `mailbox.rs`).
- ✅ Fixed stale/misleading headers: `builtins/io.rs` "terminal frontend" banner
  → "process introspection"; corrected path comments in `std/editor/ansi.blsp`
  and `std/net/http.blsp`. **[Brood]**
- ✅ `std/` consistency **[Brood]**: added `scaffold`'s `defmodule` docstring;
  renamed the `treesit` module to `editor/treesit` (+ its callers); moved
  `agent.blsp` → `std/proc/agent.blsp` and renamed the module to `proc/agent`
  (+ registration, benches, tests).

Also en route: fixed a broken build — `heap::stall_guard` was referenced by
`gui.rs` but not re-exported after the `heap/gc.rs` split (only `stall_guard_pid`
was); added it to the `pub(crate) use self::gc::{…}` list.

**Tier 4 — policy-in-Rust notes (judgment calls, "Rust=mechanism, Brood=policy"):**

- ✅ `gui.rs` colors — **resolved as *not* duplication** (2026-07-25, like the `path`
  case in Tier 2 #7). The premise was stale: there is **no `theme.blsp`**, and none of
  the three constants (`1e1e2e`/`cdd6f4`/`f5f5f5`) appear in `std/`. Colours are already
  *policy in Brood* — every render op carries its own `[r g b]`/hex resolved through
  `std/editor/face.blsp`; the three `gui.rs` constants are the rendering *mechanism's*
  fallback defaults (used only when Brood supplies none: `Op::Clear` with no `gui-bg!`,
  a face with no `:bg`/`:fg`). A "move to Brood" would add render-time coupling for zero
  dedup benefit. Fixed the stale `matches theme.blsp` comment to say so. (The
  kinetic-scroll physics model remains a legitimate future policy-in-Brood candidate, but
  is behaviour needing a live display to tune — genuinely deferred.)
- ✅ `builtins/io.rs` split (done 2026-07-25): **crypto+hashing** (HashAlgo/`%digest`/
  `%hmac`/`%random-bytes`/`%chacha20-*`/`%pbkdf2-sha256-bytes`) → `builtins/crypto.rs`;
  the **package-manager git/tar mechanism** (`run_git`/`git_or_err`/`%git-resolve-ref`/
  `%git-changed-files`/`%git-clone`/`%untar-gz`/`%rm-rf`) → `builtins/pkg.rs`; and the
  misfiled **transcendental math** (`sin`/`cos`/…/`%f64-sqrt`/`atan2` + their `math1_*`
  macros) `sequences.rs` → `numeric.rs`. The byte helpers `collect_bytes`/`bytes_to_value`
  stay in `io.rs` (general, used broadly). `io.rs` 1932 → 1436; new `crypto.rs` (258),
  `pkg.rs` (263). Glob re-export means `register()` is untouched; drift-guard +
  crypto/hash/package/format suites green.
- ❌ `nest`'s `cmd_run` — **descoped (correctly-placed CLI glue).** On a read of all
  227 lines: it's the `nest` binary's arg-orchestration *mechanism* — it interprets
  flags and assembles a Brood program, already delegating *every* behavior to Brood
  modules (`project/run-project`, `reload/reload-on-change`, `%run-program-file`,
  `%spawn`/`monitor`/`receive`, `check-file`). The "policy in Rust" is thinner than it
  looked. A rewrite would relocate ~8 subtle behaviors (doc-vs-script routing, watch
  promotion, the multi-file warning, `--main` override, recheck-on-reload, supervised
  wrapping, `--for` timing, `BROOD_NO_CHECK`) whose highest-value path (`--watch`
  hot-reload) is only verifiable interactively — modest value, real regression risk.
  Removed as a cleanup item; revisit only if `nest` grows a broader Brood-driven CLI.

### Runtime-feature parity program — BEAM / .NET / Node (2026-07-22)

The distilled, ranked program for closing the remaining runtime *feature* gaps
against the peer runtimes, from the 2026-07-11 capability audit plus the
2026-07-18 robustness survey below (most of that survey's ranked items are now
closed). The architecture is at/above parity already — scheduling, isolation,
per-process GC, distribution, hot reload; live continuation migration,
encrypted-by-default dist, and OSR exceed BEAM — and cached-boot startup
(~6.5 ms, ADR-138) beats Node (~17 ms). What remains, by leverage:

**Tier 1 — scheduled, in order:**

1. ✅ **Binary pattern matching / bit syntax + the parser port** — BEAM's
   flagship remaining capability, both halves shipped 2026-07-22.
   **The pattern (ADR-140), pure Brood:** the byte-granular `(bytes seg…)`
   pattern gained typed integer segments — `(x :u16)`/`(x :i32-le)`/`(_ :u32)`,
   u/i × 8/16/32/64 × be/le, big-endian default — lowered onto new prelude
   reads `bytes-uint`/`-le`/`bytes-int`/`-le` + encoders `int->bytes`/`-le`;
   `:u64` past `i64` auto-widens to a big integer (exact Erlang semantics).
   Sub-byte widths / float / UTF-8 segments deferred (ADR-140).
   **The parser port (ADR-141):** binary mode is now inbound-only — send
   string leaves are ALWAYS UTF-8, the Latin-1 carrier send rule is deleted —
   and `std/net` is bytes-native end to end (server sockets binary-for-life,
   no flip-back race; client responses byte-faithful `bytes` + `body-text`;
   `tcp-drain` returns `bytes`; SSE deliberately stays text-mode). Remaining
   seam: `tls-request` is string-typed both ways — rides item 3.
   **[Brood + a kernel rule deletion]**
2. ✅ **Growable read buffer — resolved by NOT building it (ADR-142,
   2026-07-22).** A mutable buffer value is a transient (forbidden, ADR-026),
   and the chunk-list + `bytes-concat`-once idiom is already O(n) in copies;
   what was still quadratic was the head reader's per-chunk rescan — fixed
   with an incremental `bytes-index-of :from` scan + a 64 KiB head cap
   (slow-loris guards, `std/net/http.blsp`). **[Brood]**
3. ✅ **`mio` reactor + TLS everywhere — shipped 2026-07-22 (ADR-143).** One
   reactor thread multiplexes every socket (plaintext, TLS client+server,
   listeners), replacing thread-per-socket; same mailbox contract. `tcp-send`
   is queued with drain-before-close (the truncation footgun is gone; 16 MiB
   cap bounds slow readers); TLS streams honor `tcp-set-binary`; `tls-request`
   takes iolists + an optional `ca-pem` trust anchor (private CAs — and the
   first in-tree e2e TLS tests, `tests/tls_test.blsp`); `http-get`/`post`
   accept `:ca` and are byte-faithful over https; `serve-loop` serves https
   unchanged when handed a `tls-listen` socket. **[kernel]**
4. ✅ **Dirty-CPU offload pool — shipped 2026-07-22 (ADR-144).** `%offload`
   runs an allow-listed blocking native (git/kdf/digest/file-IO/keygen) on a
   small OS pool via the ADR-059 copy-out → message-back seam; the prelude
   `offload` wrapper parks the caller in a selective receive — a process
   waits, never a worker. The package manager's clones/ls-remotes ride it.
   Opens the ADR-071 WASM-interop gate. Deferred: BEAM-style process
   *migration* to dirty schedulers (for heap-sharing natives) until a real
   consumer needs it. **[kernel mechanism, Brood policy]**

**Tier 1 is complete** (2026-07-22): bit syntax + the parser port, the
read-buffer non-build, the socket reactor with TLS everywhere, and the
offload pool all landed the same day.

**Tier 2 — real gaps, each gated on a first consumer (ADR-011):** the cluster
**registry** (`Registry`/via-tuples, `:global`, `pg` — "OTP deferred" below);
**mailbox bounds / backpressure** (survey item below); the **observability
remainder** (aggregators + node up/down done 2026-07-24; still `defevent` schemas,
the remote tier, `nest observe`/`nest mcp` consuming the stream — "Telemetry" below);
**`gen_statem`** and an **`Application` behaviour**.

**Tier 3 — cheap ergonomic parity:** a **grapheme-correct string API**
(codepoint-vs-grapheme indexing is a real divergence vs Elixir's `String`;
`unicode-segmentation` is already a dep, wired only to display-width);
~~**protocols/multimethods**~~ (✅ shipped as **abilities**, ADR-168 — open generic
functions with nominal dispatch replace hand-written `type-of` cascades);
**`&key` args** (designed — ADR-011); ⬜ **lexically-shadowable operators**
("Option C" — resolve operator position against local scope first, so a macro
name like `for`/`when` stops being a reserved word; decision **deferred**, kept as
reserved words for now, full spec + gotchas + hygiene notes in
[deferred.md #7](docs/deferred.md)); the dist
**`terminate/2` hook** + **FQDN long names** (dist refinements below). (The
parked-`receive` mailbox-slot leak that used to sit here was fixed
2026-07-23 — survey housekeeping above.)

**Explicitly no work:** the JSON/base64 native-codec rows (by-design pure
Brood vs C codecs); and the residual message-latency gap vs BEAM (~3–6×) is
*performance*, not features — its deep lever (inline receive compilation) is
tracked under the Elixir-parity gaps below.

### Robustness gaps vs BEAM / .NET (2026-07-18 runtime survey)

A structured survey of the runtime against Erlang/BEAM and the .NET CLR
(scheduler, fault isolation, GC/JIT, diagnostics — code-verified with file
refs). The shape that emerged: Brood is **architecturally BEAM-class already**
(per-process generational GC, reduction preemption with JIT back-edge batching,
links/monitors/`trap-exit`, selective receive, real OSR — which BeamAsm doesn't
have). What remains are targeted gaps, ranked here by leverage. Each keeps the
mechanism/policy split: kernel primitive, Brood policy.

- ✅ **Stack traces in error values — the biggest debuggability gap.** Shipped
  2026-07-18. Every `LispError` now accumulates a `trace` as the raise unwinds:
  the VM walks its live `BcFrame`s (a caller's `code[ip-1]` is the `Inst::Call`
  with the call-site pos; arms carry `fn_name`/`src_file`), the tree-walker
  attaches one entry per eval frame that entered a closure (tail entries rename
  the frame, first entry keeps the call site — matching the VM's frame reuse
  exactly; `apply_closure` seeds the tracker for native-boundary callbacks).
  Caught kernel errors surface it as `:trace` — innermost-first
  `{:fn [:file :line :col]}` maps, capped at 32 — and uncaught errors print
  `at fn (file:line:col)` lines (CLI + REPL). En route the ADR-135 program-exit
  seam was upgraded from a flattened string to the structured error, so file
  runs now render the caret/hint/trace they previously lost. Engines
  agree (error_format_parity extended by the suppression of information-free
  synthetic frames); JIT'd arms trace via their deopt re-raise. The follow-up
  also shipped same day: **process death reasons carry the structured error**
  — an uncaught error retires the process with `[:error {:kind :message …
  :trace}]` (`message::error_reason`, heap-independent so it deep-copies to
  monitors/links and crosses the dist wire), BEAM's `{Reason, Stacktrace}`
  parity for supervisors.
- 🟡 **Per-process resource limits (BEAM `max_heap_size` / mailbox bounds).**
  Lever (1) shipped 2026-07-18: **`(process-flag :max-heap n)`** (Erlang
  `process_flag/2` shape; positive int sets, nil clears, absent reads, returns
  previous). Mechanism: `Heap::proc_mem_limit` checked at the end of both
  collection paths against the *live* (post-GC nursery+old) footprint — a
  sticky flag the eval/VM safepoints raise as a catchable `E0045` **in that
  process only** (uncaught → kills just the offender; the ADR-043 hard cap
  still aborts the whole OS process). Policy is Brood: set the flag first
  thing in the spawned fn. Tests `tests/process_limit_test.blsp` (7 cases,
  green on VM/TW/no-JIT/GC_STRESS). ⬜ Remaining lever (2): optional mailbox
  bounds — a `send` to a full mailbox drops/errors by policy (accounting
  exists: `process-info` `:mailbox`); deferred per ADR-011 until a concrete
  consumer picks the policy (drop vs error vs park has real design surface,
  incl. remote delivery which can't error the sender). **[kernel mechanism,
  Brood policy]**
- ✅ **Startup image snapshot (ReadyToRun / `.beam` analogue).** Shipped
  2026-07-19 as the **expanded-prelude boot cache** (ADR-138). Cold start
  re-parsed + re-expanded the prelude every run (~31 ms, of which
  macro-expansion was ~27 ms — 744 expander invocations of genuine Brood work,
  already VM-run; see the 2026-07-19 devlog measurements). The fix: the source
  boot prints each post-`compile` (expanded + resolved) prelude form to
  `~/.cache/brood/prelude-expanded-<hash>.blsp`, keyed by `build-id` (the
  ADR-129 staleness key — the prelude is `include_str!`'d, so any binary
  change invalidates), and the next boot reads those forms and skips
  `eval::macros::compile` entirely. **Measured: ~38 ms source boot → ~6.5 ms
  cache hit** — single-digit-ms target met with no binary heap format (freeze
  is only 0.7 ms, so full `SharedCode` serialization stays unnecessary). The
  design-care items both handled: the raw prelude is still read positioned so
  `note_definition`/LSP `M-.` are identical on both paths, and the caching
  boot's final gensym counter is stored in the header + floored at cache boot
  (`gensym_floor`) so runtime gensyms can't collide with cached expansions.
  Per-form print→read→print fixpoint check gates writing (an unprintable form
  poisons the cache and the source boot just runs); any read/eval failure
  deletes the file and falls back. `BROOD_NO_BOOT_CACHE=1` opts out.

  **Completed 2026-09-02 (ADR-314): the binary snapshot this entry called
  "unnecessary" is now the default**, and the line above about freeze being only
  0.7 ms is exactly why it went unrevisited. The residual ADR-138 left — parse +
  eval + freeze, "only ~4 ms" then — grew into **9.36 ms of a 12.4 ms empty
  run**, and the stdlib image (ADR-256/281) meanwhile built the value codec and
  the differential a binary format would have needed. So the cold boot now also
  writes the prelude's *bindings*, and a warm boot materialises them rather than
  reading and evaluating 544 forms: **boot 9.36 → 5.32 ms, a whole empty run
  13.5 → 8.3 ms**. ADR-138's text cache stays as the fallback beneath it;
  `BROOD_NO_PRELUDE_IMAGE=1` opts out. Guarded by
  `prelude_image_matches_source.rs`, a two-process differential.
  **[kernel]**
- 🟡 **Observability: timing tier + trace pipeline + profiler.** Slice 1
  shipped 2026-07-18 — the survey's two named holes are closed: **GC pause
  durations** (`gc-stats` `:pause-total-us`/`:pause-max-us`/`:pause-last-us`,
  timed around `collect`), **scheduler counters** (`(sched-stats)` —
  spawned/exited/preempts/steals/migrations/workers/peak), and the **sampling
  CPU profiler** (`profile-start`/`profile-stop`): an epoch ticker + a
  frame-boundary probe in `vm_run_bc` that records each process's reified
  named-frame stack into a histogram — no signals, one relaxed load per frame
  boundary when off (exactly what the state-capture rewrite made possible).
  JIT-resident loops attribute at their quantum preempt; the tree-walker isn't
  sampled. `tests/observability_test.blsp`. Slice 2 shipped 2026-07-19 — the
  **kernel event stream** (ADR-137): `(system-monitor pid opts)` pushes
  `:gc`/`:spawn`/`:exit`/`:deopt` events to one subscriber as
  `[:system kind subject-pid detail]` messages (BEAM `system_monitor` shape,
  `:gc-min-pause-us` = `long_gc`); `telemetry/watch-runtime` re-emits them as
  `[:runtime kind]` telemetry events, so runtime + app events share the
  ADR-106 attach seam. `tests/sysmon_test.blsp`. Slice 3 (2026-07-24): the
  **aggregators** landed — counter/sum/gauge/summary/`sample-every`, then the
  **distribution/histogram** aggregator + `metric-percentile`, and **node up/down**
  folded into the `[:runtime kind]` stream via `watch-nodes` (poll-and-diff `(nodes)`;
  see "Telemetry" under M3). ⬜ Remaining: `defevent` schemas, the
  `nest observe`/`nest mcp` consumers (unify the snapshot builtins behind the stream),
  and the remote tier. **[kernel sources, Brood aggregation]**
- ✅ **Distribution self-healing: auto-reconnect + backoff.** Shipped
  2026-07-18. Brood policy: **`std/net/reconnect`** — a named, idempotent
  watcher process per node spec that connects, arms `monitor-node`, and on
  `[:nodedown]` retries `(connect spec)` with exponential backoff
  (`:min-ms`/`:max-ms`), re-arming + notifying subscribers `[:nodeup name]`.
  Kernel seam: `route` reports link-missing and `send` raises a catchable
  **E0060 noconnection** when the sender opted in via
  `(process-flag :send-errors true)` (queue-and-retry instead of silent drop;
  process liveness stays Erlang-silent). End-to-end test
  `reconnect_watcher_heals_a_fallen_link` (down → raise → heal → message
  flows). The cluster **global registry / `pg`** stays in "OTP deferred"
  below (gated on a real consumer). **[Brood + kernel seam]**
- ✅ **Dirty-CPU accounting for long native builtins.** Shipped 2026-07-18 as
  a **`BROOD_STALL_MS`-armed diagnostic** (revised same day by measurement):
  when the stall tracer is armed, `call_native` times each builtin in a green
  process, `scheduler::charge_native` charges the elapsed time against the
  reduction budget (~2 reductions/µs, the BEAM NIF model; ≥~1 ms drains the
  quantum), and a trip **names the builtin** (`[stall] native %range-reduce
  took 766ms`). Always-on per-call charging was tried first and **rolled
  back**: the A/B measured **8–22% on the message-heavy rows**
  (pingpong/ring/json — two `Instant::now` per native call) while buying
  almost nothing, because reduction preemption already bounds post-native
  hogging to ~one quantum (~1 ms); the un-preemptible time *inside* a long
  native is only fixed by the offload pool — ✅ **shipped 2026-07-22 (ADR-144,
  the parity program's item 4)**. This item is complete. **[kernel]**
- ✅ **Housekeeping found by the survey — all closed:** permanently-parked
  `receive` waiters no longer leak in an embedded host (fixed 2026-07-23:
  `Interp::drop` runs `shutdown_runtime_parked`, which routes each of the
  dropped runtime's parked waiters through the normal death path — pinned by
  `crates/lisp/tests/interp_teardown.rs`, including the
  other-runtimes-untouched case). **Deep-structure hardening of the recursive
  heap walkers** fixed 2026-07-20 (`stacker::maybe_grow` segmented growth in
  promote/GC-flush/equal/hash; `tests/deep_values_test.blsp` pins 20k–60k
  depth), and extended to the CODE walkers (expander/resolver/checker) on
  2026-07-23. **[kernel]** ✅ Exit-signal propagation fixed (2026-07-18):
  kill **hardness** is now a request property separate from the reason
  (`MailboxState.kill_hard`), so link propagation stays hard (dies at the next
  reduction tick) but carries the **originating reason** — a cascading death
  reports why the tree fell (`[:error {… :trace}]` end to end), BEAM
  semantics; the sticky-latch guarantee keys on hardness. **[kernel]**

### Findings from hatch (2026-08-13)

One item from hatch's attempt to actually adopt the framed-read combinators, plus one
already-fixed-but-unreleased startup-image bug the same session's test runs re-surfaced.

- ✅ **`tcp-read-until` / `tcp-read-n` needed a THIRD bound: `:deadline-ms`.**
  `:timeout-ms`/`:max-bytes` (added 2026-08-07, on hatch's last report) still were not
  enough to adopt them. `:timeout-ms` is an *idle* wait, reset per chunk — deliberately,
  and right for a body. But a peer that drip-feeds one byte per (idle − 1)ms re-arms it
  forever, and `:max-bytes` bounds only the SIZE that drip reaches, never the TIME: a
  worker can be held for `max-bytes × idle` — hours — inside both bounds. hatch's HTTP
  head reader had hand-rolled exactly this defense (`(min idle (- deadline (now)))`
  recomputed per receive, in all four of its read loops), so adopting the combinators
  would have *regressed* its slow-loris hardening. It is not expressible from the
  outside: the idle timer resets *inside* the call, and splitting the read across
  several calls would miss a delimiter straddling the boundary between them (each call
  scans only its own accumulator). **Fixed:** both combinators take `:deadline-ms`, a
  total wall-clock budget resolved to an absolute epoch-ms at the call and never reset;
  the per-chunk wait is now `(min idle remaining)`. It shares the `[:timeout acc]` return
  with the idle timeout — both mean "did not arrive in time", one 408. Off by default.
  Tests: `tests/tcp_test.blsp` › *framed-read limits — :deadline-ms*, including a real
  dripper that defeats a 250ms idle timeout and is cut off at 301ms by a 300ms deadline.
- ✅ **A `defdyn` global loses its dynamic-variable registration when restored from the
  ADR-218 startup image** — already **fixed in `83151776`** (2026-08-11, image format v5:
  the dynamic-var names are recorded in the image and re-marked on open). Re-derived
  independently from the hatch side on 2026-08-13 and noted here only because the fix is
  **unreleased** — the last tag is v0.3.9 (2026-08-08), which predates it, so anyone on an
  installed 0.3.9 toolchain still hits it. Symptom, for searchability: `nest test` on a
  pristine hatch checkout is green twice, then fails 38 `web/bml` tests on every run after,
  with `binding: *bml-source* is not a dynamic variable (declare it with defdyn)` (E0099).
  Workaround until the next release reaches a machine: `rm -f .brood/image.bin`. **Cutting
  a release is the whole remaining action** — nothing to fix.

### Findings from hatch (2026-07-11)

Three runtime/language items surfaced while eliminating a whole *class* of O(n²)
bugs in [`hatch`](../hatch) (the Brood web framework). Every one was the same
shape — `(str acc x)` / `(bytes-concat acc x)` accumulated in a per-read loop,
quadratic in the read count — and every fix was the same manual idiom (cons onto a
list, `reverse` + `join` once), written five times across the HTTP/WebSocket stack
(body drain, head reader, chunked de-chunk, WS reassembly, live-view render). These
would retire the bug class at the language level. See hatch's
[`docs/tcp-http-audit.md`](../hatch/docs/tcp-http-audit.md) §16–§17.

- ✅ **Iolists — the highest-leverage one. Shipped 2026-07-19 (ADR-139).**
  `tcp-send`/`proc-send`/`spit`/`spit-append`/`spit-bytes`/`append-bytes`/
  `bytes-concat` accept arbitrarily nested string/bytes/byte-int trees
  (`[status-line headers "\r\n\r\n" body]`), flattened exactly once at the
  write by one shared iterative walker — the Erlang model; immutability means
  no cycles, so termination is structural. Additive (all previously rejected
  lists). The ADR-139 Latin-1-per-string-leaf clause for binary-mode sockets
  was superseded 2026-07-22 (ADR-141: string leaves are always UTF-8).
  `str`/`join` deliberately stay display-rendering (see the ADR) — an
  explicit in-memory materialiser beyond `bytes-concat` is a future call.
  The `std/net` response builders were ported onto iolist sends 2026-07-19.
- ✅ **`bytes`-native HTTP parsing (the carrier-string bridge is dead).**
  Shipped 2026-07-22 (ADR-141) — see the parity program above: `std/net` reads
  and parses `bytes` end to end, the Latin-1 carrier send rule is deleted from
  the kernel, and the text/binary mode-flip races are structurally gone. The
  WebSocket half lives downstream in hatch (no WS in this repo) — its port
  onto `bytes` + bit syntax is hatch work, now unblocked. **[done]**
- ✅ **Framed reads — the input-side twin of iolists** (shipped 2026-07-25). The
  original framing ("a transient/builder value + freeze") was **rejected** — a
  user-facing mutable/transient buffer violates immutability (ADR-026), and the O(n²)
  it was meant to cure is already gone (iolists + the cons-accumulate `tcp-drain`
  idiom). What the sites actually repeated was the *receive → accumulate → split*
  loop, so the fix is **combinators**, in Brood (`std/net/tcp.blsp`): `tcp-read-until`
  (read to a delimiter — the HTTP request head `\r\n\r\n`, a line, a protocol record)
  and `tcp-read-n` (read a length-prefixed body/frame — Content-Length, a chunk, a WS
  payload). Both return `[frame rest]` — the surplus already read past the frame, so
  the caller keeps it for the next one — or `[:closed acc]` on early EOF. Pure
  `receive` loops over an immutable reversed-chunk accumulator, one `bytes-concat` at
  the end (no per-chunk rebuild); `tcp-read-n` tracks a running byte count so it never
  rescans. Retires the length-drain gymnastics for the socket cases. Tests:
  `tests/tcp_test.blsp` (6 loopback cases — delimiter, cross-chunk delimiter, early
  EOF, exact-n, multi-chunk-n, short-EOF). A caller-managed exposed accumulator value
  (for the interleaved WS-gather case) stays deferred (ADR-011 — no in-repo consumer
  yet). **[Brood]**
- ✅ Smaller ergonomic wins — all closed: **`mapv`/`filterv`** shipped
  2026-07-18 (prelude one-liners over `into`; `tests/sequence_test.blsp`);
  **`let` vector-destructure of a list value** verified 2026-07-18 to raise a
  clean `[:match-error :let …]`; and the **`foo--private` convention** went
  past "link-checked" to **enforced module privacy** (2026-07-23, ADR-146):
  a cross-module qualified private reference is a compile error at load,
  `(:use-internals mod)` is the explicit test/tooling grant, top-level/REPL
  stays unrestricted, and a module's macros may expand to its own privates.
  14 genuinely-shared helpers were promoted to public API en route.

### Elixir-parity performance gaps (2026-07-12, refreshed 2026-07-18)

Benchmarked brood ÷ **Elixir** per row (`../brood-benchmarks`). Elixir is *also*
immutable + GC'd + boxed-float + actor-based, so **every gap here is an
implementation deficiency, not an "immutability tax"** — the bar is "match an
immutable peer," and BEAM proves each is reachable. Ranked by ratio; `[kernel]`
unless noted.

**The 2026-07-13 priority set — the four rows where Brood was *last of 7
languages* (`nbody`, `regex`, `sieve`, `persistent-map`) — is now cleared
(2026-07-17):** nbody left 7/7 with the `fsqrt` inline (2026-07-15), sieve is
3/7 after the dense-Table work (2026-07-16), persistent-map is 6/7 (2026-07-15),
and regex left 7/7 at ~92 ms compute, past Clojure (2026-07-17). The remaining
open rows were `nqueens`, `ring`/`pingpong`, `bintree`, and `loop`; the
**2026-07-26 round took `ring` −48% and `pingpong` −22%** (ADR-155, the inline-receive
lever) and re-measured the other three, which are **at their floors for now** —
`loop` at ~4 cycles/iteration, `nqueens` and `bintree` allocation-bound with the
plausible-looking JIT lever measured and refuted (see each row).
`json`/`base64` stay excluded as gaps (Elixir/Node win them with native C codecs
against our by-design pure-Brood code — a separate, lower-priority pure-Brood-codec
track); `base64` is the residual coin-flip last place.

- 🔶 **`nbody` — was 7/7 (~40× Elixir, 5.9s); now ~0.82s (~8× total), ~5× Elixir
  (2026-07-14).** The gap was **not** float-across-calls (the `docs/jit-float.md` premise) —
  it was two things, both fixed:
  1. **Data structure (benchmark).** Bodies were a `(list …)`, so `(f b i k)` =
     `(nth (nth b i) k)` did an **O(i) list walk**, re-walked per field, where every other
     port indexes an O(1) array/tuple (Node `x[i]`, Elixir `elem(b,i)`). Two
     faithful-transcription fixes in `brood-benchmarks/bench/brood/nbody.blsp`: bodies
     `(list …)` → **vector** (~3.3× on the VM) and **bind `bi`/`bj` once** (drop the
     re-walking `f` helper, matching Elixir; +~23%). → 6.65 → 1.25s.
  2. **JIT deopts (kernel — committed, branch `perf/jit-nbody-float`).** At 1.25s the JIT was
     *net-neutral* (jit ≈ no-jit): `BROOD_DEOPT_TRACE` showed `newvel`/`advance-body`
     deopting on ~every call (~498k). Two root causes: **(a)** `inline_vec_ref` deopted on
     any vector past `INLINE_VEC_CAP` (2) — nbody's **7-element** body vectors are
     heap-backed, so every constant-index `(nth v k)` fell to the VM; fixed by falling back
     to the `brood_rt_vector_ref` helper on the non-inline branch (bintree's 2-elem inline
     path unchanged). **(b)** `(nth v k)` yields an `Op::Handle` (type-erased) and
     `op_is_float(Handle)` is `false`, so `(- (nth bi 0) (nth bj 0))` took the integer path →
     `as_int` → deopt on the `Float` tag; fixed by `as_f64(Handle)` tag-checking `Float` +
     extracting, and routing `Handle`-operand arithmetic to the float path in float-context
     arms (`has_float_slot`) — deopt-safe (a wrong guess deopts, never miscompiles), and a
     right guess yields `Op::Float` that cascades unboxed via `store_op`'s `slot_float` mark.
     Also added float `/` to `emit_float_arith` (zero-divisor guard → deopt, matching the
     VM's `(/ x 0.0)` error). `newvel` now runs fully native (deopts 498k → 249k). → 1.25 →
     ~0.82s. Verified: suite **2730/2730**, jit 28/28, differential 2/2, all 13 numeric
     benches bit-identical to `BROOD_VM=0`, `GC_STRESS`+`VERIFY`+`JIT_VERIFY` clean, bintree
     unregressed.

  **OFF 7/7 (2026-07-15):** lever (2) shipped — `sqrt` inlines as Cranelift `fsqrt`
  (0.74 → 0.54 s, "kills the last coin-flip 7/7"), and the closure-arm
  call-profitability gate + deopt feedback took another −28% (2026-07-16). Still
  ~a few × Elixir; the remaining levers: **(1)** the residual `advance-body`
  deopts — no float *param*, so the `has_float_slot` gate misses it; catching it
  needs a float-context signal that survives `(nth …)`/call-return type erasure
  (cross-arm return typing, or a compile-time float-global check for `dt`)
  *without* regressing int-vector arms; **(3)** cut the `global_ic_miss` on
  `dt`/`sm` reads in call-mediated arms. **Layer B (typed cross-arm float ABI)
  is deprioritised** — the hot calls have no float *args*. The next big general
  win is **full float type-specialization** (profile-drive an arm's float
  slots/stack so vector-read floats stay unboxed everywhere, covering
  advance-body too). **[kernel/JIT + benchmark]**
- ✅ **`regex` — was ~62× vs Elixir (981ms), 7/7 (interpreted CPS backtracker);
  OFF 7/7 (2026-07-17): ~92 ms compute, past Clojure (103 ms) — and it stayed
  pure-Brood.** Lever (a) shipped first: the AST compiles to a **lazy DFA**
  (closure-free state table + flat step loop; catastrophic patterns now linear;
  1.03 → 0.69 s, 2026-07-14), then `re:compile` discipline + the JIT learning
  keyword `=` (2026-07-15), dropping a dead `(:use editor/buffer)` (578 → ~301 ms
  wall, RSS 182 → 65 MB), and the 2026-07-17 round: memo-cache split (hot object
  out of the deep-cloning Table read), a 6-slot vector hot object, and fixing the
  **self-tail arg-position-`if` deopt storm** (a lazy `Op::Slot` materialised as
  an int-guarded payload at the block boundary — the regex loops now bind the
  branch in a `let`). Engine follow-up recorded: per-leader stack-shape analysis
  would make the natural nested-`if` style equally fast; until then any self-tail
  loop threading an opaque value through an arg-position branch hits this cliff.
  A native regex primitive stayed out — the dogfood-correct fix won. **[std/Brood]**
- ✅ **`errors-deep` — was 26× (mis-filed as "throw/unwind cost"), FIXED (`3cefcad`,
  branch `perf/errors`, 2026-07-15): 0.28 → 0.07 s (~4×, 5/7 → ~2/7 by compute — past
  Ruby/Node/Python, behind only Elixir).** The diagnosis inverted the premise: throw +
  catch with zero frames between is ~free and the unwind was always cheap — the linear
  ~96 ns/frame cost was the `throw` call **knocking `descend` out of the unboxed-i64
  register worker's subset**, so all 2.5 M frames were *built* on the interpreted VM
  call protocol. Fix: the register worker lowers `(throw <scalar>)` via a
  `brood_rt_i64_throw` callback (park error → sentinel 3 → native unwind → outcome 3),
  with a per-throw runtime check that global `throw` still binds the builtin (a redef
  deopts → the VM runs the redefinition — late binding exact). Verified: 3 engines
  bit-identical; payload identity (int + float workers); non-final-`do` throws; 40 k
  depth-bail; suite 777/777. **[kernel]**
- ✅ **`sieve` — was ~19× vs Elixir (1.0s), 7/7 (Table op overhead); now 3/7
  (2026-07-16, ~0.06 s — at Clojure's heels).** The levers landed as a
  Table-general series (every Table user benefits, `Table` stays the one
  sanctioned mutable): **dense int-keyed Table storage + fused table-op prims**
  (0.88 → 0.15 s, 2026-07-15), a **lock-free registry + fast scalar hash**
  (2026-07-15), the lock-free dense store + resume-tier fix (sieve −33%, loop
  −75%, 2026-07-16), and the **JIT inlining dense table ops** (0.10 → 0.06 s,
  4/7 → 3/7, 2026-07-16). No bitset primitive needed. **[kernel]**
- ⬜ **`nqueens` — 95 ms; allocation-bound, NOT dispatch-bound (re-confirmed
  2026-07-26).** Was 15×; −31% from routing closure arms through the
  call-profitability gate + deopt feedback (2026-07-16). Residual: list/closure
  allocation per branch; overlaps the HOF-fold and allocation paths (see
  [`docs/compute-frontier.md`](docs/compute-frontier.md)). **Negative result worth
  not re-chasing:** `BROOD_JIT_DUMP_IR` shows `safe?` and the `reduce` step lambda
  tiering but **`solve` never lowering** — its `(fn (acc c) …)` is an
  `Inst::MakeClosure`, the exact JIT bail that was crippling `receive` (ADR-155).
  It looks like the same bug and is not: rewriting the port with the `reduce`
  replaced by an explicit tail loop — no closure anywhere, every arm free to tier —
  measured **95 ms, identical to the original** (checksum 724 both ways). So
  admitting `MakeClosure` to the JIT subset must be justified on some other
  workload; it buys nothing here. **[kernel]**
- ✅ **`ackermann` — was 14× (non-tail double recursion), FIXED (`f90910c`, 2026-07-13):
  4.02 → 0.36s, 7/7 → 3/7.** The i64 unboxed worker's subset checker only matched *non-tail*
  self-calls (fib's arg-position recursion); `ack`'s recursion is in *tail* position
  (`SelfCall`), and its native-recursion depth cap was a stale 1400 (< `ack`'s ~4093 depth).
  Taught the subset about tail self-calls + raised the cap to 32768. Now 3rd, past
  Node/Clojure/Ruby/Python. **[kernel/JIT]**
- 🟡 **`ring` / `pingpong` — the inline-receive lever SHIPPED 2026-07-26 (ADR-155):
  `ring` 1376 → 720 ms (−48%), `pingpong` 249 → 194 ms (−22%).** Already cut from
  ~13× before that (ADR-135 top-level-as-green-process, 6.5 → 3.3 µs/RT + wake
  elision), then closure arms shared behind an `Arc` (ring 2.02 → 1.50 s, pingpong
  ~18%, 2026-07-13) and closure-template caching (2026-07-11), then the mailbox
  mutex trimmed to ONE acquisition per matched message (2026-07-19); `type-of`
  became a compiled prim (2026-07-19 — profiling REFUTED the copy hypothesis: the
  `to_message`/`from_message` copy is ~2% of a pingpong RT). **That refutation is
  specific to `pingpong`'s message shape, not general** — it sends `[:ping me]` and a
  bare `:pong`, so there is almost nothing to copy. ADR-178 (2026-07-29) removed one of
  the two copies for a parked local receiver and measured the win scaling with payload:
  −3% at an empty payload, −24% at 64 elements, −35% at 1024. Both readings are correct;
  quote the message shape with the number.

  The 2026-07-26 round found the remainder was **not** scheduling: isolating a
  self-send + `receive` (zero cross-process handoff) priced a receive at **820 ns**
  vs 310 ns for `send`, i.e. `pingpong` was paying for receive machinery almost
  exclusively. Two compounding defects, both from the body **thunk**: building +
  calling it cost ~235 ns/message (vs ~50 ns for a small-vector protocol), and
  because `Inst::MakeClosure` is outside the JIT subset it made the whole matcher
  arm unlowerable — `BROOD_NO_JIT=1` and `BROOD_NO_HOF_JIT=1` both changed the
  number by **zero**. Fix: `%receive` (arity 3 → 2) now only *selects* a clause
  (`[idx var…]`, nil = no match, nil = timeout) and the macro emits every body at
  the **call site**, so bodies compile into the owning arm and matcher arms tier
  (`tail_call` on pingpong: 400,309 → 473).

  ⬜ Still open: the per-candidate `vm_apply` in the scan — `BROOD_NO_HOF=1` is
  197 → 509 ms, so that protocol is still doing real work. The deeper lever is
  compiling the pattern *test* into the calling arm's bytecode so the scan makes no
  closure call at all. Beyond that the residual is `send` (310 ns) + the remaining
  receive (615 ns): mailbox lock, `from_message`, and the `hof_resolve` redone per
  receive. **[kernel]**
- ⬜ **`bintree` — GC / allocation pressure; 118 ms, unchanged (re-measured
  2026-07-26).** Build+walk trees; per-node alloc + minor-GC throughput vs BEAM.
  Inline small-vector storage (2026-07-01) and the checkpoint purity exemption +
  nursery capacity seeding (2026-07-18) trimmed it; the 2026-07-18 profile says
  what's LEFT is the deferred big-ticket JIT items (~17% `jit_run_fast_link` +
  ~11% frame staging — the "true call inlining" lever — and ~10% allocation FFI),
  not regressions. Its `[left right]` cells are returned from `make` and walked by
  `check`, so they **escape** and scalar replacement is inapplicable; the only
  lever left is a narrower cell representation, which spends a core invariant
  (2026-07-24 spike). The one open watch-item. **[kernel]**
- ⬜ **`loop` — raw iteration overhead; AT THE FLOOR (re-confirmed 2026-07-26).**
  Was 6×; the resume-tier fix took −75% (2026-07-16). 50 ms wall ≈ 40 ms compute
  for 30M iterations = **~1.33 ns/iter**, about four cycles for an
  overflow-checked add, a compare, a branch and the safepoint tick. Measured and
  rejected: threading the bound as a parameter instead of reading the global
  (which is how the Elixir/Node ports are written, so *more* faithful) is
  **70 ms vs 50 ms — slower**; the global read is already hoisted and the third
  argument costs more than it saves. Incremental JIT-tuning grind (BEAM has a
  25-yr lead) — expect small wins. **[kernel/JIT]**
- ✅ **`persistent-map` — was 5.2× vs Elixir (612ms v 118ms), 7/7; FIXED by lever (1)
  (2026-07-15, benchmark transcription in `brood-benchmarks`, no kernel change):
  0.71 → 0.16 s locally (~4.4×), harness-scaled ≈ 138 ms → past Clojure's 285 ms
  (7/7 → 6/7), within ~1.2× of Elixir.** The port's hand-written `get`+`assoc` (two
  descents) became `map-int-add` — the same fused single-descent RMW idiom `wordcount`
  already uses, and the faithful counterpart of Elixir's one-call `Map.update/4`.
  Diagnosis notes (measured, 2026-07-15): with `map-int-add` the loop is *already
  optimal* on the kernel side — the LINMAP rewrite turns the accumulator into a private
  Table (`map-int-add` → `table-incr`) and the loop **already runs JIT-native** (the
  letrec-style rewrite emits `SelfCall`, so the gate + back-edge tiering cover it);
  two hypothesized JIT levers (gate relaxation for defn-style tail loops with calls +
  a Call-tail back-edge escape) were implemented, measured **zero win**, and reverted —
  the residual floor is the per-iteration native-call/Table cost, the same shared floor
  as `sieve`/`regex`. Deferred levers (2)/(3) (assoc-path node alloc, general fused
  `update`) remain valid for *non*-linmap map workloads. **[benchmark + measured]**

### Findings from brood-life profiling (2026-06-13)

The four-axis language review from optimising `brood-life` (a GUI Game of Life) was
triaged proposal-by-proposal and the accepted items shipped 2026-07-09 (`clamp`,
`as->`, `{:keys …}`/`:or` map destructuring, lazy seq-view fusion, `read-string`
trailing-form drop, and more), alongside two allocation/GC bug fixes (transient
corruption, allocation serialisation). One item stays deferred:

- ⬜ **JIT float specialisation** — ordinary perf tuning (partial scaffolding in
  `compile/mod.rs`, "type-specialize float arms"); gated on a concrete hot float
  workload, not a completeness gap.

### Stability backlog (2026-07-10)

- ✅ **Continuous fuzzing (`cargo-fuzz`)** — all five libFuzzer targets ship
  (`crates/lisp/fuzz/fuzz_targets/`): **reader**, **evaluator**, and (added
  2026-07-23) **JSON** (through a persistent `Interp`), the **dist wire
  framing** (the unauthenticated surface, via `dist::fuzz_decode_frame`), and
  the **bundle footer/archive** (`bundle::fuzz_parse`) — alongside the July
  stress kit (`make stress`). `make fuzz T=<target>` runs one; the fuzz dep is
  lean (`default-features = false` + `system-alloc` — ASAN must own
  allocation) and `ASAN_OPTIONS=symbolize=0` avoids a 90 s system-symbolizer
  stall per exit. First smoke: ~62 M total execs across the three new
  targets, zero findings. En route, a repo-wide build bug fell out: the build
  script's relative `rerun-if-changed=.git/HEAD` never existed, so **every
  build of every profile recompiled `brood`** — fixed (absolute + existing
  paths only).
- ✅ **Host-panic hardening (audit residue)** — closed 2026-07-23. The checker
  now runs its whole analysis under `catch_unwind` with the compile-ns /
  known-names / imports / GC-roots state restored on both paths (a panic
  degrades to one "checker internal error" diagnostic — brood-lsp and `nest
  check` survive); `expr_ty` already had its depth cap, and the remaining
  recursive walkers (`check_into`, `collect_def_names`, `check_recursion`,
  `check_macro_hygiene`, `collect_syms_into`, plus the expander's
  `macroexpand_all_depth`/`resolve_walk` — deep CODE, the sibling of the
  2026-07-20 deep-VALUE fix) grow the native stack in heap-backed segments
  (`stacker`). Pinned by a 30k-deep-form test that previously aborted the
  host.
- ✅ **Prelude freeze vs boot-expanded `receive`** (found + fixed 2026-07-22):
  the freeze's dangling-env assert swept the whole closure slab, including
  boot *garbage* (the builder heap never collects), so a dead
  captured-frame closure from a boot-time receive-matcher expansion killed
  boot. Fixed with a **reachability mark pass** at freeze: reachable closures
  keep the hard assert (a live captured frame really would dangle);
  unreachable ones get their env scrubbed (unobservable). The prelude
  `offload` now deliberately sits *after* the `receive` macro, so every boot
  regression-tests the fix; `BROOD_BOOT_TRACE=1` reports the scrub count.

### External conformance corpora (2026-07-25)

Every test in this repo is currently **hand-written** — the one exception is
`tests/numeric_conformance_test.blsp`, whose cases were *adapted by hand* from the
chibi r7rs-tests and Gabriel suites. That means our correctness bar is "cases we
thought of". The industry answer is to vendor the corpora other implementers have
already paid for in production bugs: the historically-fatal float-parse strings,
the decimal arithmetic suite, the Unicode break tables, the regex semantics files.
None of them are Brood-specific; all of them are machine-readable.

**Conventions.** Vendored data lives under `tests/corpus/<suite>/`, each with a
`README.md` recording the upstream URL, the pinned commit/version, and the licence
(never vendor GPL data — `ansi-test` is mined for *ideas* only). Runners are ordinary
Brood tests named `tests/conformance_<suite>_test.blsp`, tagged `:tags [:conformance]`
plus `:slow` when they run more than a second, and they locate their data relative to
`(current-file)`. `scripts/fetch-corpus.sh` (re)fetches each upstream and subsamples
the huge ones — the committed subset stays small enough to read, and the *full*
corpus is a script run away for an exhaustive local pass.

| # | Suite | Pins down | Status |
|---|-------|-----------|--------|
| 1 | **parse-number-fxx-test-data** (Apache-2.0) | decimal→f64 parsing; 5.2M cases incl. every historically-fatal input (`2.2250738585072011e-308`, half-way ties, 800-digit mantissas) | ✅ 2026-07-25 |
| 2 | **dectest** (Cowlishaw/IBM, ICU licence) | IEEE 754 decimal arithmetic — the definitive suite; Python vendors it as `Lib/test/decimaltestdata`. **Found 2 real scale bugs** (below) | ✅ 2026-07-25 |
| 3 | **UCD test files** (Unicode licence) | `GraphemeBreakTest` + `NormalizationTest` — cursor motion in `std/editor/*` lives or dies here. Needed two new primitives (`string->graphemes`, `string-normalize`); `WordBreakTest`/`LineBreakTest`/`CaseFolding` still open, each needs its own surface | ✅ 2026-07-25 |
| 4 | **Fowler testregex** + **rust-lang/regex `testdata/*.toml`** | POSIX regex semantics, leftmost-first vs leftmost-longest, capture groups | ⬜ **blocked** — `std/regex` is a deliberate subset (no ranges, captures, `{m,n}`, backrefs), so the corpora would be ~95% skips. Wire when the engine grows those |
| 5 | **JSONTestSuite** (MIT) | the `y_`/`n_`/`i_` minefield cases against `std/json`. **Found an RFC violation + KI-11** (below) | ✅ 2026-07-25 |
| 6 | **CommonMark `spec.json`** (BSD-2) | ~650 examples against the markdown renderer | ⬜ **blocked** — there is no `std/markdown`; nothing to test yet |
| 7 | **WPT `urltestdata.json`** (BSD-3) | WHATWG URL parsing against `std/url` | ⬜ **blocked** — `std/url` is RFC 3986 with no base-URL resolution, IDNA or per-component encode sets; WHATWG is a different spec, so this would be ~90% skips |
| 8 | **NIST CAVP** (public domain) | SHA-1/256/384/512 byte vectors + ~1,250 HMAC cases — the one corpus whose failures would be *security* bugs. **Wycheproof deliberately not wired**: its value is ECDSA/AES-GCM/RSA, none of which Brood implements | ✅ 2026-07-26 |
| 9 | **Kuhn `UTF-8-test.txt`** (CC BY 4.0) | malformed-UTF-8 decoding: overlongs, surrogates, truncation, boundary code points | ✅ 2026-07-26 |
| 10 | **SMHasher3**-style statistics | avalanche / bit-bias / collision quality of the CHAMP hash | ⬜ |
| 11 | **Paranoia** (Kahan, public domain) | FP arithmetic sanity as a *runnable program* — doubles as an end-to-end VM/JIT float exerciser. **Found a `pow` underflow bug** (below) | ✅ 2026-07-26 |
| 12 | **chibi `r7rs-tests.scm`** + SRFI-1/13/133/125 reference tests | portable s-expression suites, beyond what `numeric_conformance_test` already adapts | ⬜ |
| 13 | **Gabriel / Larceny R7RS benchmarks** | real Lisp programs with checkable outputs; VM/JIT shakedown *with* an oracle. 8 ported (`nboyer`, `chudnovsky`, `mazefun`, `deriv`, `takl`, `cpstak`, `nqueens`, `primes`); `gcbench`/`destruc` **descoped** (no oracle / mutation IS the program), `peval`/`earley`/`conform`/`nucleic` deferred with reasons. **Found KI-13** (a checker hang) **and a gap in the engine-differential gate** (below) | ✅ 2026-07-26 |
| 14 | **csv-spectrum** (BSD-2) | tricky-CSV corpus for `std/csv` — **found a CRLF-in-quotes bug** (below). **toml-test dropped**: nothing in the tree parses TOML (manifests are `.blsp` data), so there is no target | ✅ 2026-07-25 |
| 15 | **MPFR-generated ULP tables** | ELEFUNT-style accuracy bounds for `sin`/`cos`/`exp`/`log`/`pow`, references from `mpmath`/`rug` | ⬜ |
| 16 | **Chez Scheme `s/mats`** (Apache-2.0) | the largest Lisp *compiler* test corpus in existence — closures, arity, tail calls; translate the applicable portions | ⬜ |

**Findings so far.** The point of the exercise is bugs, so they get recorded here.
*parse-number*: none — expected, since the reader delegates to Rust's `f64::from_str`;
the 33,552 cases are a regression gate. *dectest*: **two real scale bugs**, both
`bigdecimal` identity short-circuits that Brood inherited — its `Sub` returns the
other operand untouched when one side is zero (`1 - 0.0` → `1`, not `1.0`) and its
`Mul` when one side is one-valued (`1.00 * -1` → `-1`, not `-1.00`), each discarding
the short-circuited side's scale, and `Add` doing neither so `+` and `-` disagreed.
`num_bin` now pins every exact decimal result to the standard's ideal exponent
(finer-of-two scales for `+`/`-`, sum for `*`). Significance surviving arithmetic is
the whole reason to reach for a decimal over a float. *JSONTestSuite*: **an RFC 8259
violation** — `std/json` accepted unescaped control characters inside strings (a raw
tab or newline parsed as content, where §7 requires U+0000–U+001F to be escaped);
fixed in `json--string--acc`. And, more seriously, **KI-11**: two deeply-nested
documents *abort the OS process*, because deep non-tail recursion on the **JIT** path
overflows the native stack while the bytecode VM and the tree-walker both handle the
identical input correctly. That is a JIT call-path bug, not a JSON one — any Brood
service parsing untrusted nested input is killable with a few kilobytes, and
`try`/`catch` cannot see it. **Fixed 2026-07-26** (the JIT tail-chain native-depth cap);
see `docs/known-issues.md`.

*UCD*: NormalizationTest's ~19,000 cases pass the full conformance closure (every one
of the five columns normalising into every form, not just `NFC(source)` — idempotence
is where normalisers break). GraphemeBreakTest is 602 cases with **one failure, and it
is upstream**: `unicode-segmentation` 1.13.3 omits U+2701 from its
Extended_Pictographic table, so a `2701 ZWJ 2701` sequence splits where UAX #29 rule
GB11 joins it. The rule is right for every other pictographic (U+270A, U+2764,
U+1F468, U+1F3F3), so it is a table gap around the U+2700 dingbats — worth an upstream
report. Excluded and pinned by a test asserting the *current* behaviour, so the
exclusion fails loudly the day the crate is fixed.

The Gabriel row is also the first corpus to find a defect *outside* the code under test —
its two findings are in the checker and in the test harness, not in a library.

*csv-spectrum*: **a CRLF-in-quotes bug**. RFC 4180 §2.6 makes a CRLF inside a quoted
field *content*, but `std/csv` swallowed the `\r` in its `:quoted` state along with
the ones that really are line endings — so any multi-line quoted cell (anything
exported from Excel on Windows) silently lost its carriage returns and failed to
round-trip. Fixed; line-ending normalisation now happens only in the `:unquoted` and
`:quote-seen` states. This was the first corpus aimed at a **pure-Brood** subject
rather than a Rust crate behind a thin wrapper, and it found something on the first
run — which is the argument for prioritising the remaining pure-Brood targets.

*UTF-8 stress*: none — Brood delegates to Rust's `String::from_utf8` and `slurp`
correctly raises on a malformed file rather than substituting U+FFFD. Value is the gate
plus the explicit accept-vs-reject record (overlongs rejected, noncharacters accepted).
*NIST CAVP*: none — the digests come from CAVP-validated crates, so the exposure was
never the compression function but the wiring (algorithm keyword, hex casing,
bytes/UTF-8 boundary, `Tlen` MAC truncation), all correct. *Paranoia*: **a `pow`
underflow bug**. A negative exponent computed `1 / base^|exp|`, so the positive power
overflowed to `inf` and the reciprocal flushed the **whole subnormal range** to zero —
`(pow 2.0 -1074)` returned `0.0` where 2⁻¹⁰⁷⁴ is representable (`5e-324`), and every
exponent past −1023 was wrong the same way; an int base failed for the sibling reason
(bignum power, underflowing reciprocal). Fixed in the prelude by splitting the exponent
in half so no intermediate leaves range. Paranoia also pinned Brood's one deliberate
IEEE 754 departure: **division by zero raises** rather than yielding infinity (overflow
still produces infinity, so infinities exist — they just aren't reachable by dividing).

*Gabriel benchmarks*: no wrong answers — all 8 ports match upstream on the tree-walker
*and* the VM+JIT, including `nboyer`'s three rewrite counts (95024/591777/1813975, exact)
and `chudnovsky`'s ten 50-to-500-digit integers. Two findings, one a real defect:

✅ **KI-13 (FIXED 2026-07-27) — `nest check` hung on the `deriv` port.** Cross-module return-type
inference for an undeclared recursive callee blew up **exponentially in branch count**:
2/3/4/5 recursive `cond` branches building nested list structure cost 105 ms / 105 ms /
**8.7 s** / did-not-finish-in-900 s. The same call *inside* the defining module is
instant — it is the cross-module `sig_of` → `infer_sig` → `expr_ty` path, where nothing
bounds the *size* of the inferred `Ty` (`InferGuard` correctly breaks recursion *cycles*;
that is a different thing). This is a CI gate and the LSP's own code path, so a hang is
worse than a wrong warning, and the trigger is ordinary code. Workaround in the port: a
**declared** sig is consulted before body inference, so `(sig deriv (any -> any))` takes it
back to 105 ms. Likely fix: cap inferred-type size and widen past the cap (widening an
over-approximation is sound by construction — it can only lose precision). Repro + table
in [`docs/known-issues.md`](docs/known-issues.md).

⬜ **`BROOD_VM=0` does not give the in-language suite tree-walker coverage.**
A test body run by `nest test` (or `brood --test`) under `BROOD_VM=0` shows no slowdown at
all, and `BROOD_JIT_DUMP_IR=1` lists its arms reaching the JIT — the env var gates how a
*top-level form* is run, while the framework invokes each test as an already-compiled
closure (the same function at top level via `brood file.blsp` interprets correctly, 0 JIT
arms, ~10× slower). So `make test-both`'s tree-walker leg does **not** exercise the ~3400
in-language cases the way its comment implies; real per-expression engine agreement comes
from `differential.rs`, which pins the engine with `set_forced_engine` rather than the env
var. Hence this corpus ships a Rust runner too (`crates/lisp/tests/gabriel_engines.rs`) on
that same mechanism. Worth deciding whether the framework should honour a forced engine for
test bodies — it would widen the gate considerably, at a real wall-clock cost (measured
debug tree-walker: `nboyer` n=0 38 s vs 0.25 s on the VM). Also measured: the debug
tree-walker spends ~12.6 kB of native stack per frame, so `primes<=1000` (999-deep non-tail)
trips the 12 MB budget there with a clean `recursion too deep` — correct, and release
handles it fine. See `tests/corpus/gabriel/README.md`.

### FIXED 2026-07-27 — the >100× slowdown was the RUNTIME collector, not contention

Both observations below turned out to be one bug, filed as KI-14 and fixed: the ADR-091
RUNTIME collector's drain report re-walked a deep process's entire root stack at every
reporting safepoint. `roots` is the VM operand/env stack, so the "cheap" Phase-1 probe is
really O(recursion depth); a 100k-deep JSON parse made it enormous, and a Phase-2-dirty
process paid that walk on every safepoint only to discard the result. Measured in one
drain epoch: **78,409 Phase-1 walks over a 1,727,686-entry root stack.** Full write-up and
the three-part fix in [known-issues.md](docs/known-issues.md#ki-14).

That explains the shape that made this look like contention: the drain only arms once the
shared RUNTIME region crosses `BROOD_RT_GC_FLOOR`, so the differential was never
concurrency or test size — it was **how much code the process had loaded**. Few files → no
drain → fast; whole suite → drain armed → quadratic.

1. The two 100k-deep JSONTestSuite documents: `nest test --only 'test:every n_ document'`
   went from a >10-minute hang to **9.8 s**. Full `nest test` including conformance:
   **3592 tests, ~90 s**.
2. The UCD normalisation sweep: **2.5 s for Part1 inside the debug wrapper**, down from
   >120 s. Its sampling knob (`BROOD_UCD_PART1_OF`) is kept — the wrapper is still a debug
   build — but it is no longer masking anything.

`cargo nextest run` is green: **877/877, wrapper at ~435 s** under full concurrency.

Worth recording for the next investigation of this kind, since the guesses below were
confidently wrong: the suspects named here were the per-process GC under many concurrent
green processes and the `:isolated`/snapshot machinery. Neither was involved. What found
it was sampling the hung process under gdb (which cannot *attach* on this machine — yama
`ptrace_scope` — but can *launch*: `gdb --batch -ex run` plus a background `pkill -INT`),
then a temporary counter to turn the stack samples into numbers. Two theories — image size,
then drain-epoch count — died on those measurements before the real cause showed up. The
instinct the section did get right: **do not fix this by shrinking tests.** That was tried
across four cycles and the number barely moved, because test size was never the variable.

Six of the nine wired suites finding real defects — including the checker hang KI-13
(since fixed) — is the argument for the rest. Note the shift as the subjects got harder: the
suites aimed at Rust crates behind thin wrappers found nothing, the ones aimed at
pure-Brood libraries found a bug each, and the one that runs whole *programs* through the
real toolchain (Gabriel) found its bugs in the **checker and the test harness** rather
than in any library.

**Not a corpus — the technique that actually finds JIT bugs.** ⬜ **EMI /
equivalence-modulo-inputs** (Le & Su, PLDI'14 — Orion/Athena): mutate provably-dead
paths in a program; the output must not change. We are unusually well set up for it
— three engines that must agree (`BROOD_VM=0` tree-walker, the bytecode VM, the JIT)
give a free three-way oracle, and `fuzz/differential.rs` + `fuzz/jit.rs` are already
the seed. Nothing downloadable finds an optimizer miscompile; this does. Related
prior art worth reading before extending the fuzzers: **Fuzzilli**'s IL + mutator
design (JS-specific, but it is *the* reference for coverage-guided JIT fuzzing), and
Cranelift's own `cranelift-fuzzgen` / `bugpoint` / `enable_verifier`, which we get
upstream for free.

---

## Done — the foundation

Compressed; per-item history is in [`docs/devlog.md`](docs/devlog.md) and
[`docs/archive/`](docs/archive/), decisions in
[`docs/decisions.md`](docs/decisions.md).

- **Stage 1 — a full functional Lisp.** Reader (lists/vectors/atoms/keywords,
  quasiquote); tree-walking evaluator with proper tail calls, lexical scope,
  closures, Lisp-1; macros (`defmacro`, quasiquote, `macroexpand`, `gensym`);
  `defn`/`&optional`/`& rest`; i64+f64 with overflow-checked arithmetic; immutable
  maps + `{ }` literals (ADR-030); the string, math, and sequence libraries;
  dynamic variables (`defdyn`/`binding`, per-process); pattern matching across
  `match`/`let`/`fn` (ADR-021/022) incl. `{:keys …}`/`:or`; `case`,
  `dotimes`/`dolist`, `letrec`; error handling (`throw`/`try`/`catch`/`error`) with
  source locations; modules (`provide`/`require`, `foo--private`, ADR-019); the
  project model + parallel test runner (ADR-020); reducible lazy `range` and
  transducers.
- **Concurrency — green processes on all cores** (`docs/concurrency.md`).
  `spawn`/`send`/`receive`/`self` with per-process `Send` heaps and copy-on-send;
  green M:N scheduling on a worker pool (originally corosensei coroutines,
  ADR-018; replaced 2026-06-08 by state-capture continuations with general
  work-stealing + live migration, ADR-100); shared code region
  for cross-process hot reload (ADR-013/014); closures sent between processes and
  across nodes (ADR-033); reduction-counted preemption (ADR-027); selective
  `receive` + `(after ms …)` timeouts.
- **Types — set-theoretic gradual typing** (ADR-078 and follow-ons). Function
  arrows, element/parametric types, structural combinators, narrowing, singleton/
  literal types, map K/V, records/shapes, tuples; the sound half of local inference;
  `(sig …)` contracts + `BROOD_CONTRACTS=1`; the full-soundness-vs-hot-reload
  mechanism (re-check per reload, ADR-123/124/125). LSP tiers 0–2 + a
  dev-ergonomics pass.
- **Execution — closure-compiling VM + tier-1 JIT.** The VM is the default engine
  (ADR-076); a Cranelift template JIT (ADR-101) is a default cargo feature (integer
  arithmetic, fused Prim2, hot-reload epoch guard, in-native inline caches).
- **M2 — editor data model.** Rope substrate (ADR-045); buffer model;
  buffers-as-values; evaluate-the-Lisp-I'm-editing; per-process memory reclamation.
  Collaboration seam (2026-07): buffer *processes* with subscriptions + versioned
  delta pushes, edit-surviving markers (presence cursors ride them, pid-keyed
  cleanup on subscriber death), structured `buffer-splice`/`buffer-marker-move`,
  and concurrent-splice **transforms** (`splice-transform` — exact merges for
  disjoint edits, no CRDT) — what a downstream editor's multiplayer editing runs on.
- **M3 — display protocol + native frontend.** Serialisable render-op protocol
  (ADR-046); input events; in-process terminal frontend; per-op/per-window fonts
  (ADR-079); `nest observe` (inline + remote, ADR-053); telemetry core
  (`std/telemetry.blsp`, ADR-106); resilient `ui-run`.
- **M4 — server / daemon mode.** TCP sockets (ADR-062); TLS *client*/HTTPS;
  distributed nodes (`name@host`, cookies, encryption ADR-089, dual-listen, mesh
  join); userland supervision + a real `gen_server`; an ETS-style in-memory table
  store; `std/task`. `std/editor/serve` (ADR-090): the daemon/emacsclient seam —
  per-client and shared sessions, attach identity, async event pass-through,
  `serve-stop` — plus the exit-signal hardening it forced (ADR-132; pid identity
  across `node-start`).

Runtime housekeeping (both items landed):

- ✅ **Tracing GC for mid-eval / never-returning loops.** The per-process
  generational semi-space copying collector (ADR-055/061/072) fires at the eval
  safepoint at **any** depth — roots are the explicit operand/env stacks the VM
  reified. Superseded the ADR-016 arena-reset.
- ✅ **Work-stealing scheduler.** Landed 2026-06-08 via the state-capture
  rewrite (ADR-100): corosensei deleted, a paused process is relocatable heap
  data, stealing is general (any queued process) and live cross-worker migration
  works. History + invariants: ADR-100 in [`docs/decisions.md`](docs/decisions.md).

---

## What's next — by area

### Language core & types

- 🟡 **Elixir-loved ergonomics — borrow the beloved features that fit a small,
  immutable Lisp** (2026-07-28). Survey of what Brood still lacks vs. Elixir's
  most-liked features (pipe / `with` / pattern-matching / OTP / specs / `mix` /
  observer / streams / `get-in` are already covered; `#(…)` capture was
  consciously declined). Ordered by value-to-effort:
  - ✅ **`with`** — Elixir-style sequential match-binding with `:else`, as a
    prelude macro over `match` (2026-07-28; see devlog). No new special form.
  - ✅ **`spy`** (chose this name over `dbg`) — a *homoiconic tree-tracing* debug
    macro (2026-07-28, ADR-173). Went beyond `dbg`'s fixed special-cases: fully
    macroexpands and instruments every evaluated position in place, so it traces
    the whole call tree (pipelines fall out for free — `(-> x f g)` → `(g (f x))`),
    laziness is preserved, and it's referentially transparent. Output flows through
    a swappable `*spy-sink*` (`defdyn`) so a host can capture the trace as data (the
    editor-inline-values seam); default is an indented stderr tree. Prelude macro,
    no core change. ⬜ Deferred: per-special-form descend rules beyond
    `if`/`do`/`let`/`letrec`, source-position `file:line` (no primitive exposed
    yet), a `:label` arg.
  - ✅ **Doctests** (2026-08-28) — runnable examples in docstrings, executed by `nest test`.
    Smaller than estimated: the parser and the discovery pass already existed as six private
    functions inside `tests/doc_examples_test.blsp`, hard-wired to `builtin-modules` so they
    could gate `std/` and nothing else. Lifted to `std/tool/doctest.blsp`, made public, and
    pointed at a prefix, so a project's own docstrings gate the same way; that test file now
    *uses* the module rather than duplicating it (which is also what proves the extraction
    faithful — it still catches a sabotaged `std/` example with the identical message).
    Uses the repo's established `    (form)   → result` convention rather than `>>>`.
    Scoped to the project's package, skipped for a nameless project, and run only once the
    suite is green.
  - ✅ **`reduce-while`** (≈ `Enum.reduce_while`) — early-terminating fold via
    `[:cont acc]` / `[:halt acc]` (2026-08-13). Pure prelude fn over `seq`/`match`
    (`std/prelude.blsp`, `reduce-while-loop` accumulator); tests incl. cross-process
    in `tests/prelude_enum_test.blsp`.
  - ✅ **Function-head guards** — `:when` guards on `defn`/`fn` *clause heads*
    (2026-08-13, ADR-226). Met "keep the core small" by **reuse, not new
    machinery**: a `:when`-bearing clause simply routes the whole fn through the
    existing `match*` engine (which already evaluates guards for pattern clauses),
    so guard eval / fall-through / hygiene / TCO all come for free. Only a fn that
    uses a guard pays match-dispatch; `:when` + `&optional`/`&` in one clause is a
    loud error (one mechanism per fn). `eval/macros.rs` `clause_has_when_guard`.
  - ✅ **`tap` / `then`** (Kernel) — single-fn pipe helpers (2026-08-13). Plain
    prelude functions beside the `->`/`doto` family: `(tap x f)` runs `(f x)` for
    effect and returns `x`; `(then x f)` returns `(f x)`. Elixir-parity spelling
    for the "apply a function value in a pipeline" case (`doto`/`->` splice forms).
- ✅ **Stdlib namespacing (ADR-227)** — split the flat prelude so *core* stays bare
  and *derived helpers* move into namespace modules (`enum/`, `map/`-extras, `math/`,
  …), keeping current names. Staged, one green commit per family; the prelude *is* the
  boot image, so only boot-independent helpers move (the pinned core — `map`/`filter`/
  `reduce`/`distinct`/`take-while`/`partition`/`zip` — stays bare, which is right anyway).
  - ✅ **`enum` (stage 1, 2026-08-13)** — 14 sequence helpers (`dedupe`, `frequencies`,
    `group-by`, `chunk-by`, `chunk-every`, `interpose`, `interleave`, `scan`, `zip-with`,
    `min-by`, `max-by`, `reduce-while`, `enumerate`, `index-where`) → `std/enum.blsp`,
    plus the new `enum/distinct-by`. `(:use enum)` or qualify. Suite green (4643/4643).
  - ✅ **`map`-extras (stage 2, 2026-08-14)** — `merge-with`, `update-vals`, `update-keys`,
    `select-keys` → `std/map.blsp`. Core map protocol stays bare; the bare `map` *function*
    is unaffected by the module name. `enum/group-by` now `(:use map)`. Suite green.
  - ✅ **`math` (stage 3, 2026-08-14)** — `sqrt`/`pow`/`ceil`/`round`/`round-to`/`clamp`/
    `abs`/`sum`/`product`/`even?`/`odd?`/`positive?`/`negative?` + consts `pi`/`e` →
    `std/math.blsp`. Core arithmetic stays bare (`quot`/`mod`/`rem`/`floor`/`min`/`max`);
    `mod`/`binding` had their `abs`/`odd?` inlined. Suite green (4643/4643).
  - ✅ **`json` (stage 4, 2026-08-14, `a57cc573`)** — with auto-require in place, `std/json.blsp`
    drops its redundant `json-` export prefix (`json-parse`→`parse`, `json-encode`→`encode`);
    consumers and the JSON fuzz target move to the qualified `json/parse`. Suite green.
  - ✅ **Auto-derived imports (2026-08-14, `a57cc573`)** — shipped as *qualified-reference
    auto-require*, **not** the bare-name derivation first planned: a qualified `mod/name` infers
    `(require 'mod)` for any module (macro heads eager, value refs deferred, root region scanned),
    so no explicit `require` line is needed. Bare names still need `(:use …)` or qualification —
    no bare-name magic. The KI-17 unrequired-module lint is now obsolete. See
    `docs/auto-derived-imports.md`; `nest check` zero warnings, suite green.
- ✅ **Syntax finalisation pass (2026-07-25, ADR-149/150/151/152)** — closed the
  cases where the surface accepted a plausible-but-wrong spelling and
  **reinterpreted** it instead of rejecting it. Binding containers are lists (a
  vector there is an error, so Clojure's `(defn g ([x] …) ([x y] …))` and
  `(let [[a 1] [b 2]] …)` fail loudly instead of becoming different programs); the
  pattern **pin is `^x`**, freeing `~` for quasiquote alone (a pin *was*
  `(unquote x)`, so a macro could never emit one — 167 pins migrated); **ambient
  names are declared with `defdyn`**, not spelled with earmuffs (two modules
  writing `(def *width* …)` no longer share and clobber one root binding); and
  Clojure's typed `catch`, `&optional` inside a pattern-dispatched `defn`, an
  unrecognised `defmodule` header clause, and a nested quasiquote are all errors
  with hints. Also: arity precedence no longer depends on clause order, and calling
  data (`(:a m)`) gets a hint. ✅ **`sig` adoption + alias trims (2026-07-26, ADR-153)**: 23
  signatures now live in `std/path`/`set`/`json`, enforced cross-module in both
  directions; the attempt exposed and fixed four defects (`bytes`/`decimal` were
  unspellable types, `sig!` couldn't expand early in the prelude, and
  `BROOD_CONTRACTS=1` turned a declaration into a rebinding — twice). Redundant
  aliases (`concat`, `intersperse`, `reductions`, `all-globals`) and `cond`'s
  `:else` special case are gone; `lambda` kept, `car`/`cdr` removed (ADR-154). ⬜ Still open: whether
  `BROOD_CONTRACTS=1` should stop rewriting `sig` into `sig!` (it is why three of
  those four defects existed, and it blocks annotating the prelude at all), and
  `defrecord`'s 5-uses-all-in-the-prelude adoption question.
- ✅ **KI-12 fixed (2026-07-26)** — the prelude freeze re-tagged a RUNTIME handle as
  PRELUDE, so the default `*load-path*` held an unrelated object in every build and
  filesystem module lookup from it never worked. `localize_for_freeze` copies
  non-LOCAL reachable state into the builder's slabs first; `to_prelude` re-tags
  LOCAL only. Also: a unit tagged `:slow`/`:conformance` now raises its batch
  timeout, so the external corpora stop being hard-killed as if hung.
- 🟡 **Merely-wider inference case.** The description here was wrong about its own example:
  `(/ x 2)` on ints is not `number` (int ∪ float) — Brood's division is **exact**, so it is
  `int | ratio`, and never a float at any arity. `/` is contagious but not int-closed, so the
  arithmetic rule fell straight through and the checker made **no claim at all** about the
  most ordinary arithmetic expression there is.
  - ✅ **The sound half shipped** (2026-08-28): `(/ int int)` now types as `int | ratio`, which
    catches a declared `float` return, and a result fed somewhere non-numeric, at no cost in
    false positives. `int.union(ratio)` in `numeric_call_ty`; tests in `check::tests` +
    `tests/sig_adoption_test.blsp`.
  - ⬜ **The residue stays deferred — now measured, not argued** (2026-08-28). A body of
    `int | ratio` **declared `int`** is correct whenever the numerator is even. The
    deferral is *architectural*, not an omission: the body grades as **dynamic**, so
    `consistent_with` uses `∩ ≠ ⊥` and `int|ratio ∩ int ≠ ⊥` passes. Flagging it means
    switching that arm to `⊆`.
    - **What that would cost, measured** by temporarily enabling it: **zero** new warnings
      across all of `std/` + `tests/` — and **4 of 5** on a probe of ordinary correct code.
      `(/ x 1)`, `(/ 6 3)`, `(/ x x)` and `(/ (* 2 x) 2)` are each *provably* int and would
      each be flagged; only `(/ x 2)` for an unknown `x` is genuinely undecidable. The
      in-repo cost is zero only because nobody has yet written the code that breaks — which
      is exactly the trap: the gate looks free until a user divides by 1.
    - **The order for a future attempt** is therefore *narrow first, flag second*: make the
      decidable cases type as exactly `int` (literal folding, a literal ±1 divisor, then
      parity for `(/ (* 2 x) 2)`), and only once the residual really is undecidable is
      flagging it a strictness judgement rather than a false positive.
    - Note the gap is *narrower* than this entry originally claimed — the remaining
      ambiguity is int-vs-ratio, not int-vs-float.
- ✅ **Parameter-type inference from body usage → callers checked** (occurrence typing,
  [ADR-190](docs/decisions.md), completed 2026-07-30). The **sound (unconditional-demand)
  slice shipped 2026-07-25**: `infer_sig` infers a parameter's type from every position
  *guaranteed to execute* on a call — a call argument (incl. nested), a `do` form, a
  `let`-binding RHS/body, an `if`/`when`/`cond`/`match` *test*, an `and`/`or` *first* operand —
  intersecting demands (`collect_param_demands`, `sigs.rs`); branch/guard-gated positions are
  skipped and a shadowing binder excludes the param, so the guarded-use false positive can't
  arise. **ADR-190 then wired the consumer**: Pass 2.8 stores each same-file function's inferred
  *params* (not just its return), the caller arg-check consumes them, and it works **cross-file**
  (`sig_of` surfaces params even when the return defers) — so an unannotated function now flags
  a wrong caller. Plus **ability-op occurrence typing**: `(area s)` on a sealed, `:default`-free
  op derives `s : Shape`. Sound throughout (under-constrained → under-warn); whole repo
  warning-clean. ⬜ Remaining (deferred, ADR-011): demands from *conditional* positions, per-arm
  checking of a multi-arity callee, and the merely-wider return case above.
- ✅ **Earmuffed-global typing** (2026-07-25, companion to the above): the checker now
  types a `*earmuffed*` global as unknown (like a `defdyn`), not by its load-time
  default value — a redefinable/dynamic-by-convention global (`*project-root*`, a plain
  `def` reassigned at runtime) is `dynamic()` per the type philosophy. Clears the
  pre-existing `(canonicalize *project-root*)` false positive (`global_value_ty`).
- ✅ **First-class set kernel** — shipped 2026-07-24. A distinct `Value::Set`/
  `Tag::Set` backed by the CHAMP trie (`element → true`): the `#{…}` reader literal
  (evaluates + dedups its elements), `#{…}` printing, `set?`, `type-of` → `:set`,
  order-independent equality with a set **never** `=` to a map, seqable
  (`count`/`first`/`rest`/`map`/`fold`/`into`), cross-process round-trip
  (`Message::Set` + wire codec), and full GC/promote/compaction survival (the
  compatibility contract — GC trace, copy-on-send, hash, `ConstVal` handle, type
  lattice). Kernel ops `%set`/`%set-add`/`%set-remove`/`%set-has?`/`%set-count`;
  the `set` library (`std/set.blsp`) is now Brood sugar over the kernel type
  (constructor + `conj`/`disj` + `union`/`intersection`/`difference`/`subset?`).
  Tests: `tests/set_test.blsp` (18, incl. cross-process), `reader_hints_test`;
  green under `GC_STRESS`+`GC_VERIFY`, differential, `nest check` zero-warnings,
  suite 2921/2921 (ADR-060).
- ⬜ **Unbounded stream generation** (`iterate` / infinite producers) — lazy
  seq-view fusion already shipped (ADR-111); picks up when an editor feature needs it.
- 🟡 **`std/` curation + frameworks sequencing** (ADR-085/097) — `std/` curated and
  hierarchical module names shipped; the model is batteries-included (frameworks ship
  in the default install, not fetched). ⬜ Next: a future GUI framework ships bundled
  too; gated on the first real consumer.
- 🟡 **Native interop — WASM components** (ADR-071/145,
  [`docs/interop.md`](docs/interop.md)). ✅ **Slice 1 shipped 2026-07-22
  (ADR-145): the sandboxed host.** Embedded `wasmtime` (default-on `wasm`
  feature), `%wasm-load`/`%wasm-call`/`%wasm-exports`/`%wasm-close` with
  WIT-typed marshalling + fuel metering, `std/wasm.blsp` (`wasm-load`,
  `wasm-call`, `wasm-call-blocking` on the ADR-144 pool, `use-native` binding
  every export as a Brood fn), the `:unbound` checker category, and
  toolchain-free WAT-component tests. ✅ **Slice 2 (bytes marshalling) shipped
  2026-07-24:** a `list<u8>` parameter accepts a Brood `bytes` value directly
  (one-pass octet lower — the byte-oriented calls: hashing, compression, codecs,
  binary parsing), and a non-empty `list<u8>` result lifts back to `bytes` (an
  int vector still lowers; an empty `list<u8>` result stays an empty vector — the
  documented ambiguity edge). Copy-based (zero-copy read-mapping is still
  deferred); `crates/lisp/src/wasm.rs` `lower`/`lift` + `tests/wasm_test.blsp`
  (`blob-echo`/`byte-sum` over a `list<u8>` WAT component). ⬜ Remaining slices:
  the package-manager `:native` manifest/lock/build-on-fetch integration
  (`%wasm-build`) — the delivery vehicle, recommended next; WASI capability grants
  (gated on that manifest); guest `resource` handles; epoch preemption (low value
  — fuel already bounds runaways); and the blob **zero-copy** read-mapping
  optimization (over today's copy).

### VM & JIT

> **Status (2026-07-24): this track is at rest — its frontier is effectively mined
> out.** A full sweep found the remaining items are either already shipped or
> not worth doing: native-HOF-callback routing (already done via `apply_engine`);
> the allocation frontier `bintree`/`nqueens` (representation-capped by the boxed
> 24-byte `Value` — no sound quick win, `compute-frontier.md` 2026-07-24 block);
> and JIT Stage 4 (no-go — gated off in production, below). The VM+JIT is in good
> shape; further compute-perf work is high-effort/capped-payoff. Reopen only for a
> concrete need (e.g. closure-arm inlining if a real workload demands it).
>
> **The structural work this at-rest state argues for is IN PROGRESS (2026-08-11)** — see
> [Backend seams](#backend-seams--swappable-jit--engine--perf-legibility-2026-08-11) and
> [`docs/backend-seams.md`](docs/backend-seams.md). It changes no generated code: it makes the
> backend contract compile-checked and hoists the bail/profitability decisions above it, so the
> one remaining redesign-class lever (the X-register call convention) has an interface to be
> checked against.

- ✅ **The `let`-self-ref `send` divergence no longer reproduces** (verified
  2026-07-19): a `let`-bound self-recursive closure sent to a pid is rejected with
  the same "cannot send a self-referential local closure" error by BOTH engines in
  every shape tried — top level, created inside a VM-compiled `defn`, `send`
  executed from inside a VM arm, and via `spawn` (identical die-uncaught behavior).
  Presumably fixed en route by the capture/closure-template unification work; if a
  diverging shape resurfaces, it belongs in the differential fuzzer corpus.
- ✅ **Native higher-order callbacks route through the VM** (`try`/`binding`/
  `apply`/`isolate`) — done. Each dispatches its Brood callback through the shared
  `apply_engine` selector (`crates/lisp/src/eval/compile/mod.rs`), which runs the
  callback compiled on the VM when it's the active engine and on the tree-walker
  only under `BROOD_VM=0` — the once-per-call analog of the `use_vm`/`apply_value`
  branch `%range-reduce` uses per element. Call sites: `%try`/handler
  (`builtins/system.rs` `try_catch`), `%binding` thunk (`binding`), `%isolate`
  thunk (`isolate`), and the `apply` builtin's target (`apply_builtin`). The
  cached-arm hot path (`hof_resolve`/`hof_apply_step`) `%range-reduce` also uses is
  deliberately absent here: it amortizes cost across a tight loop and buys nothing
  for a thunk invoked once. The divergence above (since verified fixed) confirms
  the VM-routed path is behaviour-identical to the tree-walker.
- 🛑 **JIT Stage 4 — RUNTIME compaction survival** (ADR-091) — **investigated
  2026-07-24; NO-GO, near-zero real payoff.** The idea was a constant-pool
  indirection table (ADR-096 §4.C) so `runtime_collect` could rewrite handles
  without invalidating native code. Findings (verified in-code + measured):
  (1) RUNTIME **data-handle** relocation ALREADY survives in native code — the JIT
  bakes a pointer to the `ConstVal` and reads live bits via `brood_rt_const_load`;
  compaction rewrites those in place (`rewrite_arm_handles` over `live_vm_arms`),
  and runtime-service *function* addresses already sit in a per-arm indirection
  table. (2) The only native-invalidating step is the **epoch bump** in
  `runtime_collect_with` (`heap.rs` bumps `runtime.version` = `global_epoch()`,
  which nulls every arm's `jit_code` on next tier). (3) BUT that single-process
  compaction path is gated on `Arc::get_mut` (unique runtime ownership), which
  **never holds during real execution** — `spawn_root_program` keeps the runtime
  `Arc` shared for the whole run, so `(runtime-collect)` returns `:ran false` and
  the auto-safepoint compaction is skipped too. Production reclamation goes through
  the 2-generation path, which frees whole generations **without** rewriting handles
  or bumping the epoch → it does not invalidate native code at all. So the
  invalidating path is off precisely when it would matter; building the
  epoch-decouple/IC-remap machinery buys ~nothing. Left behind: the missing
  end-to-end coverage was added — `crates/lisp/tests/jit_runtime_compaction.rs`
  (a JIT'd RUNTIME-handle arm stays correct across a real relocation; green under
  `GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY`).
- ✅ **Leaf-callee inlining** (the real call-heavy lever) — **implemented 2026-07-19;
  DEFAULT ON since the same evening (`BROOD_NO_LEAF_INLINE=1` opts out)** after the
  gating measurements came back flat everywhere they had to (boot / 100×-hello
  batch, `require`-heavy loads incl. editor/buffer+sexp+json+regex, `nest check`
  over std/, the in-language suite wall, every benchmark row) and the wins held:
  ~30% on the scalar-helper loop shape, a further ~8% on type-predicate dispatch
  compounding with `PrimOp1::TypeOf` (predicates are call-free now, so they
  splice into hot matchers). A hot fixed-arity `defn`
  whose non-tail static-head calls all resolve to small, calls-free, non-capturing
  callees gets a stored derivation (args → `LetBind` into shifted callee slots,
  callee body spliced above the caller's frame) that rides the existing two-stage
  deferred-upgrade channel. Soundness: derivation happens once at arm-compile time
  (heap access for callee resolution, reentrancy-guarded), is epoch-stamped, and
  the lowerer refuses any other epoch — hot reload wins by construction (tested:
  a post-warm `def` of a spliced callee takes effect). The inlined engine has no
  deopt checkpoint, so derivation requires ZERO residual non-tail calls (from-ip-0
  re-run stays effect-free); `jit_ckpt_read` now also refuses the inlined engine
  (the small layout's ckpt slot lies inside the spliced range — a real Int there
  faked a journal). `inline_nslots` is floored at the small frame (spill+ckpt
  reserves made it possible for the "grow" to be an underflowing shrink).
  **The zero-residual-call restriction was lifted 2026-08-03 — see
  ✅ Partial leaf splicing below (ADR-210).**
  **Measured: ~30% on the scalar-helper loop shape** (`(+ acc (sq (add1 i)))`
  1.65 → 1.2 s); benchmark-suite rows flat (they're recursive/HOF/alloc-bound —
  the remaining shapes need closure-arm support (defn-gate today) + Phase 3/4).
  Gates green with the flag on: JIT≡VM differential under GC_STRESS+VERIFY,
  VM≡TW differential, 3 dedicated tests incl. hot-reload + residual-call gate.
  ⬜ Next: closure arms (needs a fast-link invalidation story without a defn
  name) — the expansion/`require` measurement + default-flip are done.
- ✅ **Partial leaf splicing** — **implemented 2026-08-03, DEFAULT ON**
  (`BROOD_NO_PARTIAL_LEAF=1` opts out); ADR-210. Lifts the zero-residual-call
  restriction above, so **one un-spliceable callee no longer blocks inlining of every
  small callee beside it** — `mandelbrot`'s `row-sum` splices `->float` with the
  recursive `esc` still a real call. The blocker was not the checkpoint *layout* (the
  previous write-up's framing) but the inlined body's own **bytecode ip space**: a
  journalled resume ip meant nothing to `vm_resume_deopt`, which drove the small chunk.
  Fixed with a **resume arm** (`ir::LeafInline::resume`) — a full `CompiledArm` over the
  spliced body, so a deopt resumes in the chunk that wrote the journal, and journalling
  reuses `jit_ckpt_depth` unchanged. A journalled derivation splices above the caller's
  *full* small frame so the two layouts' journals cannot alias; if `jit_ckpt_depth`
  declines, the derivation is refused rather than run unjournalled. Also removed the
  leaf path's `JIT_INLINE_CHUNK_KEEPALIVE` entry + per-lowering recompile, and fixed a
  pre-existing hazard where a probe's callee resolution *cached and published* an arm
  compiled under the reentrancy guard, permanently denying that callee its own
  derivation (`probe_arm_for` now caches nothing). Gates: 5 fuzz generators × 4 engine
  configs, 40/40 `tests/jit.rs` under GC_STRESS+VERIFY, 4350/4350 in-language suite with
  the mechanism on and off, and an effect-duplication guard **verified by sabotage**
  (`tests/jit_effect_once_test.blsp` cases 5–6).
  **Measured 2.4×** on the shape it targets (a lowering self-tail caller, one spliceable
  leaf beside one residual call: 562 → 237 ms / 2M; the arm lowers twice with the flag on,
  once with it off). **Every published benchmark row is flat** — attributed with the on/off
  switch on a single binary, which is what disposed of an apparent `nqueens` −5% (the switch
  is worth 0.3% there, and that row drifts ~3% between invocations). ⚠️ **`mandelbrot` — the
  row that motivated this — gets nothing: `row-sum` never lowers to native at all**, flag on
  or off, and leaf inlining is JIT-only (the VM runs the small body). Check
  `BROOD_JIT_DUMP_IR=1 … | grep '^\[jit-ir\] ====='` for an arm before assuming any inlining
  change can move it. ✅ **And forcing `row-sum` native is NOT the lever** — measured
  2026-08-04: it is refused by the call-mediated profitability gate (the one added because
  tiering that shape regressed `nbody` 15–20%), and exempting it makes `mandelbrot` **+0.7%**
  and `matmul` **+5.1%** against 0.3% floors. The arm does lower with the exemption, so the
  mechanism works — it simply is not faster. ⬜ The real `mandelbrot` lever is removing the
  **boxing** (unboxed floats across call boundaries), which is a much larger piece of work.
  Benchmark for the mechanism itself: `scripts/fuzz/stress/leaf_splice.blsp`.
- ⬜ **Layer-2 computed-goto dispatch** (`std::arch::asm!`, x86-64, `#[cfg]`-gated,
  pure-Rust fallback) — only if profiling still shows dispatch overhead. Additive.
- ⬜ **Allocation / GC frontier — `bintree` + `nqueens`** (the two worst compute
  rows). **Profiled 2026-07-24 — no sound quick win; representation-capped.** Both
  are heavily JIT'd, NOT interpreted (JIT ~3.5×/~2.9× over the plain VM: bintree
  ~422→~119 ms, nqueens ~286→~100 ms local); bintree runs 100% native, nqueens' hot
  paths (the `reduce` step lambda + `safe?`/`solve`) run native too. The ~9.5× gap
  (bintree 102 vs Elixir 10.4 ms N=200; nqueens 83 vs Node/Elixir 8.7/9.0 ms N=10)
  is the **boxed 24-byte `Value` allocation floor + GC churn**, not dispatch:
  bintree churns ~819K escaping 48-byte tree-node vectors/run. Measured dead ends:
  `BROOD_GC_FLOOR` sweep *regresses* (not GC-frequency-bound); the cells **escape**
  so JIT escape analysis is inapplicable; nqueens' lambda already runs native
  (`BROOD_NO_HOF=1` A/B confirms — no stuck slow path). The only remaining lever is
  a **narrower cell representation**, which spends a core invariant (new `Value`
  kind / brushes the NaN-boxing line `types.md`/compute-frontier §2 reject) for a
  capped payoff. **Defer** unless that invariant spend is explicitly wanted — better
  ROI in JIT Stage-4 / closure-arm inlining. Full analysis:
  [`docs/compute-frontier.md`](docs/compute-frontier.md) (2026-07-24 RESUME block).

### WebAssembly — a cooperative single-threaded scheduler (playground concurrency)

> **Goal.** Make green processes — `spawn` / `send` / `receive`, gen-servers, the whole
> concurrency layer — run in the `wasm32` build (the in-browser playground and the
> runnable docs on brood.fly.dev), single-threaded. Today they trap: the playground's
> counter example dies with `RuntimeError: unreachable executed`.

**Why it traps (diagnosis).** *Not* the processes — the **worker pool**.
`scheduler::pool::ensure_workers` (`pool.rs:311`) starts the executor pool with
`std::thread::spawn(worker_loop)`; `wasm32-unknown-unknown` has no threads, so the spawn
traps. Even if it compiled, `mailbox::wait_for_message` (`mailbox.rs:1254`) parks a
receiving process by **blocking its worker thread on a condvar** — on WASM's single
thread that would deadlock.

**Why it's feasible now.** corosensei (stackful coroutines) was removed in ADR-100 §8
(2026-06-08): a green process is now suspended by a **heap-captured continuation (state
capture)**, not a native stack switch. A process can be paused and resumed with no
threads and no stack-switching primitive — exactly what WASM lacks. The hard part of
in-browser concurrency is already solved; what remains is a driver.

**Design — a `#[cfg(target_arch = "wasm32")]` cooperative scheduler.** Keep the native
pool untouched; add a single-threaded path behind cfg:

1. **No worker threads.** `ensure_workers` is a no-op on wasm32 (one logical executor —
   the calling thread).
2. **A cooperative pump** (`pump_ready`): pop the run queue, run each ready process one
   quantum via the existing `run_one` / `finish_quantum`, loop until the queue drains.
   The reduction/preemption model is unchanged — a process that burns its quantum is
   re-enqueued; the pump just round-robins on one thread.
3. **Non-blocking park (the crux).** On wasm32, `park_on_receive` (`pool.rs:543`) must
   **return to the pump** instead of `wait_for_message`-blocking: the receiver is
   state-captured and left parked (off the run queue). A `send`'s `wake_enqueue`
   (`pool.rs:64`) re-queues it; the pump picks it up on its next turn. No condvar, no
   thread block. This is the one genuinely new mechanism.
4. **Root integration.** `Interp::run_program` / `eval_source` (`lib.rs:402`) drive the
   pump to completion rather than blocking the root thread on the root's `receive`. The
   root process becomes a scheduled process; `run()` returns once it finishes (result +
   captured stdout).
5. **Would-block termination.** If the run queue drains while a process is still parked
   on a `receive` that can never be satisfied (all idle, no pending timers), the pump
   reports a catchable *deadlock / would-block* error instead of hanging — the
   single-thread analog of "every scheduler is asleep."

**Open questions / caveats (single CPU).**
- **Receive timeouts & timers.** `(receive … (after ms …))` and any timer needs a
  cooperative deadline check against a monotonic clock (wasm `Instant`), fired when the
  pump next idles — coarser than the native timer thread. A first cut may support only
  immediate/zero timeouts and document the limit.
- **Blocking primitives.** `sleep`, blocking I/O, `%offload` — cooperative or
  unsupported on wasm; a blocking `sleep` must yield to the pump. The playground has no
  I/O anyway.
- **No parallelism.** CPU fan-out examples run *cooperatively*, not faster — expected
  and fine for a playground/teaching context.

**Milestones.**
1. cfg(wasm32) `ensure_workers` no-op + `pump_ready` driving the run queue (a
   compute-only spawned process runs, no trap).
2. Non-blocking park/wake → the counter example (`spawn` + `send` + `receive`) runs to
   completion under the pump.
3. Root-process integration in `run_program`/`eval_source`; would-block termination.
4. Cooperative receive-timeouts (or a documented limitation).
5. Playground + docs: re-enable the runnable **Processes** example on the site; add a
   wasm concurrency test (`tests/wasm_test.blsp`).

**Touch points.** `process/scheduler/pool.rs` (ensure_workers, the pump),
`process/scheduler/lifecycle.rs`, `process/mailbox.rs` (the park path), `eval`/`lib.rs`
(root drive), `crates/playground/src/lib.rs` (pump after eval). No new dependencies —
everything behind `cfg(target_arch = "wasm32")`, so the native scheduler stays
byte-for-byte unchanged.

### Documentation generator — an ExDoc-equivalent for Brood

> **Goal.** Pull documentation *from the language* — docstrings, arglists, type
> signatures, source locations — and generate a browsable HTML doc site (API reference
> + narrative guides + search + runnable examples), the way ExDoc does for Elixir. Host
> it on hive per-package (the hexdocs.pm equivalent) and for the language itself (std +
> prelude — the canonical reference).

**What already exists (reuse, don't rebuild):**
- `nest doc [MODULE] | --all` emits **Markdown** from docstrings by loading the module
  and introspecting the live image (arglist + docstring + type-arrow + source location —
  the same data `lookup` returns; `--all` covers every builtin + prelude global).
- hive's `docbuild` builds **per-package** docs on publish by **parsing source forms
  without evaluating them** (the security story for untrusted uploads — a public registry
  must never run what it's handed), storing rendered module JSON in the `docs` table; the
  web UI renders `/packages/:name/:version/docs`.
- hive's `/docs` reference page already embeds the playground WASM so examples *run in the
  browser* — a Brood upgrade over ExDoc the generator should inherit.

**Design.**
1. **A structured doc model** (`nest doc --json`), not just Markdown. Per module: the
   moduledoc + a list of definitions `{name, arity, arglist, doc, type, source, privacy,
   examples}`. Two extraction backends behind one schema — **loaded-image introspection**
   (rich: macros expanded, types resolved; for trusted local/std docs) and
   **parse-don't-eval** (hive's `docbuild` path; for untrusted registry uploads). The
   HTML generator consumes the schema, indifferent to which backend produced it.
2. **Guides (ExDoc "extras").** A `project.blsp` `:docs` key declaring narrative markdown
   pages + ordering/groups, e.g.
   `:docs {:guides ["guides/intro.md" "guides/processes.md"] :main "guides/intro.md"}`,
   rendered as first-class pages alongside the API reference.
3. **The HTML generator** (`nest doc --html [-o out]`) — a static, self-contained site: a
   left sidebar (guides + modules), a page per module (moduledoc + each definition with
   its signature/docstring/source link), client-side full-text **search**, **autolinking**
   of `mod/fn` references in docstrings to their pages, and syntax-highlighted code. Reuse
   the WASM interpreter so **code examples are runnable inline** (the `/docs` mechanism,
   generalized).
4. **Doctests.** Examples in docstrings written as `expr ;=> result` are extracted and run
   by `nest test`, so docs can't drift from behaviour (Elixir doctest parity). One shared
   example parser feeds both the runnable HTML blocks and the test runner.
5. **Hosting on hive.** Extend `docbuild` from "module JSON → web UI" to rendering the full
   ExDoc-like site per package/version, and publish the **language** reference (std +
   prelude via `nest doc --all --json`) as the canonical site — the hexdocs.pm +
   language-docs equivalent. The hand-written `/docs` guide becomes the first "extras"
   guide of that site.

**Milestones.**
1. `nest doc --json` — the structured model from the loaded-image backend; unit-tested
   against a fixture module.
2. `nest doc --html` — the static site (sidebar, module pages, search) from the JSON.
3. Guides/extras (`:docs` in project.blsp) + autolinking + runnable examples.
4. Doctests: `;=>` extraction, run under `nest test`.
5. hive: `docbuild` emits the full site; host the language reference at a stable URL.

**Touch points.** `std/tool/` (the `nest doc` command + JSON/HTML emit), the runtime's
doc-table / `lookup` introspection, hive's `docbuild` + `web/views/packages/docs`, and a
shared HTML template (which can share the `/docs` page's styling + runnable-example JS). No
new language features required — this is tooling over data the runtime already exposes.

**Open questions.** Parse-don't-eval limits for untrusted packages (no macro-generated
docs, no computed docstrings) vs. the rich loaded-image mode for std — the schema tolerates
both, but the untrusted path is necessarily thinner; document the gap. Versioned URLs +
"latest" redirects (the hexdocs shape) on hive.

### Tooling & errors

- **Stability metadata per name: `:since`, `:deprecated`, `:beta`.** Three facts a
  library must state and Brood currently has no way to say — *when did this appear*,
  *is it going away*, and *is it settled enough to build on*. All three are the same
  mechanism (a fact recorded against a name, read by the tooling), so they should ship
  together rather than as three features.

  ```lisp
  (defn parse (text) …)
  (meta parse :since "0.9.0")
  (meta old-parse :deprecated "0.14.0" :use 'parse)   ; what to use instead
  (meta try-this :beta "the shape of the options map may change")
  ```

  **Why the machinery already exists.** A per-name fact recorded at definition time and
  read back by tooling is exactly what `%mark-private` (ADR-146) and `%register-sig`
  (ADR-259 / the `(sig …)` macro) already are — a Brood macro over a primitive that
  writes into a side table the checker, `nest doc` and the LSP consult. `meta` is the
  same shape with a payload of data instead of a type, and it should be one form with
  keyword clauses rather than three macros, so a name can carry all three at once.

  **Where each one is spent — this is the part that decides the design:**

  - **`:since`** is *documentation only*. `nest doc` shows it, hover shows it, and the
    doc catalogue can render a "new in 0.14" index. Nothing warns. It is also the one
    fact that could be **derived rather than declared** — the version a name first
    appeared in is recoverable from git history, and a `nest doc --stamp-since` that
    writes them once beats asking every author to remember. Declaring it by hand is the
    fallback for names whose history predates the convention.
  - **`:deprecated`** is a **checker** warning, not a runtime one: `nest check` reports
    each use, with `:use` naming the replacement so the message is actionable and
    `nest check --fix-renames` could later rewrite it mechanically (it already does the
    unambiguous half of a rename wave — ADR-257). Warning at run time instead would fire
    in a hot loop, on a machine that cannot act on it, long after the edit that caused
    it. It should also be **suppressible per call site**, via the existing
    `(check-allow :deprecated …)` directive, since a library must sometimes call its own
    deprecated name from the shim that replaces it.
  - **`:beta`** is the one with a genuine runtime component, and it needs the discipline
    the kernel already worked out for the ADR-232 drop warning: **deduplicated per name,
    printed once**, never per call. A beta warning that floods is a beta warning everyone
    silences. `nest check` should report it too — the static path is where it is
    actionable — with the runtime notice as the backstop for a name reached through
    `eval` or a dynamic dispatch the checker cannot see.

  **Open questions to settle before building:**
  - Does `:deprecated` gate CI? `nest check` exits nonzero on any warning today
    (ADR-023's batch-only hard reject), which would make a single deprecation break every
    downstream build the day it lands. It probably needs its own severity, which is the
    unresolved half of the warning-suppression question already noted below.
  - What does a version *mean* for a std name — the Brood release, or the package
    version from `project.blsp`? For `std/` they are the same; for a published package
    they are not, and the answer decides whether the value is a literal string or is
    resolved against the enclosing project.
  - Does metadata compose with hot reload? A `def` rebinding a name mid-run must not
    silently keep the old name's `:deprecated` fact (the same late-binding rule ADR-013
    established for the code itself).

- ✅ **LSP: type-directed record-field completion** — shipped 2026-07-30. Completing a
  `:keyword` at a map-**key** position (`get`/`assoc`/`update`/`dissoc`/`contains?`,
  or the keyword-accessor head `(:… m)`) whose map argument the checker types as a
  record offers *that record's* field names, each with its declared field type. The
  missing wiring became `check::arg_ty_at` — a position-keyed type query that runs the
  full `check_file` walk with a capture hook armed (thread-local in `check/walk.rs`),
  so `let`-bound RHS types, sig-typed params, Gap A globals, and a same-file
  `defrecord`'s ctor sig are all in force at the capture point; keyed by the *call
  form's* position because a bare-symbol argument carries none of its own. Mid-edit
  buffers are repaired before the strict read (the partial key is blanked in place,
  unclosed delimiters closed), `:` is now a completion trigger char, and every miss
  degrades to no extra candidates. The noisy every-record heuristic stayed rejected.
  ⬜ Still out (no concrete need): field completion inside a bare **map literal** —
  the literal under construction has no identity to infer from (an `:__id__`-carrying
  literal is rare hand-written form), and typing it from an *expected* parameter
  type needs bidirectional checking the checker doesn't do.
  (The sibling `impl` op-body **snippet** completion shipped earlier — a fillable
  `(op [self] …)` skeleton, snippet-gated on the client's `snippetSupport`.)
- ✅ **`nest test` selection — `mix test` parity** — shipped 2026-07-24. The suite
  had **no way to run a subset by name**: 2965 tests, and the only narrowing was a
  file path. Now `--only`/`--exclude`/`--include SELECTOR` (a tag, `test:substr`,
  or `describe:substr`), `FILE:LINE` (the covering test), `--failed` (last run's
  failures, kept as a set difference in the project cache dir), `--seed` (shuffle,
  seed echoed for replay), `--partitions N --shard K` (stable-hash CI shards),
  `--max-failures`, `--repeat-until-failure`, `--timeout`, `--slowest N`,
  `--no-trace`. Tags come from `:tags [kw …]` on `describe`/`test`, merged
  group→test. Selector parsing + all selection logic live in Brood
  (`std/tool/test.blsp`, `test--make-filter`); `nest` only forwards argv.
  `tests/test_selection_test.blsp` (54 cases, incl. cross-process spec round-trip).
  ✅ **`--stale` + `--formatter` shipped 2026-08-01.** `--stale` re-runs only test files
  whose transitive dependencies changed since they last ran — the dependency graph is the
  KI-17 require-closure already computed for the checker (`project--require-closures`), the
  change signal is the newest mtime across a test file + its dependency source files vs a
  per-file record in the project cache dir (like `--failed`); whole-project runs only.
  `--formatter NAME` emits machine output via a formatter registry (`register-test-formatter`
  as the extension seam) — `tap` (TAP v13) and `json` built in — suppressing the progress
  dots + human summary. Both in `std/tool/test.blsp`/`project.blsp`, wired through
  `crates/nest/src/main.rs`; `crates/nest/tests/stale.rs` + `tests/test_selection_test.blsp`.
  🚫 **`--breakpoints` — deferred, not declined, with a concrete reason** (assessed 2026-08-01).
  ExUnit's pause-on-failure fights Brood's concurrent runner: a failing assertion throws and
  unwinds inside a green **worker** process before the runner catches it, and an interactive
  pry needs the stdin-owning **driver** — so a naive hook deadlocks (the runner blocks on
  workers that are parked awaiting a pry that can't happen until the runner returns). The
  debugger already delivers the capability for a *targeted* test — `(with-debugger d (fn () …
  (break …)))` then `(pry d)`, and `eval-at` now sees every local (path B) — so a broken
  suite-wide `--breakpoints` would be strictly worse. Revisit when the runner grows an
  inline-pry driver (its own focused pass). (`--cover` shipped — see "Test coverage" below.) A
  **process-native tracing debugger** now exists (`std/tool/debug`, ADR-174): `break`
  parks without a timeout + a multi-process paused queue, `break-when`, `spy`-fed
  aggregate queries, a causal tree propagated transparently across **`spawn` AND `send`**
  (a server handles a request in the sender's context), `step` single-step evaluation,
  `pry` (drop into the real styled REPL at a debug point), and `eval-at` (evaluate an
  expression in a breakpoint's captured scope — the *explicitly named* `break` locals).
  ✅ **Automatic locals capture (`eval-at` path B) — shipped 2026-08-01.** `eval-at` now
  sees EVERY in-scope local at a breakpoint, not just the values named at `break`. The
  mechanism is the **`(%scope)` / `(%locals)` compiler intrinsic**: the VM compiles either
  0-arg call straight into a fresh `{:name → value}` map read from the compile-time
  lexical-scope table (`compile_scope_map` in `eval/compile/mod.rs`, keyed by the name as a
  keyword so a named `:val` cleanly overrides a captured local on `merge`), with the
  env-frame builtin as the tree-walker fallback so both engines agree. `break`/`break-when`
  became **macros** so the capture expands in YOUR lexical scope (the snapshot carries
  `:scope` = auto-captured locals + `:vals` = explicitly-named, the latter winning); the
  named-vals path is unchanged. Verified across VM/tree-walker/no-JIT/GC-stress
  (`tests/debug_test.blsp` path-B block). ⬜ Still deferred: an interactive `nest observe`
  debugger pane.
  ✅ **`,resume` / `,step` REPL meta-commands + a REPL command registry — shipped 2026-08-01.**
  `std/tool/repl.blsp`'s `,cmd`s moved from a hardcoded name-list + dispatch `cond` into a
  registry (`*repl-commands*`) with `register-repl-command` as the extension seam; `pry`
  registers `,resume [N]` (resume all / the Nth parked) and `,step` (advance one) against the
  `*debug-session*` it binds, so you drive a debug session without typing full forms.
- ✅ **Test coverage — both tiers** — function-level `nest test --cover` (2026-07-24)
  (ADR-148, [`docs/coverage.md`](docs/coverage.md)): which project functions the
  suite never entered, plus `--cover-min PCT` to fail the run under a floor.
  Implemented as **pure Brood policy with zero kernel support** — `global-names` +
  `source-location` for the denominator, `def` rebinding + late binding (ADR-013)
  as the instrumentation seam, and `Value::Table` (ADR-107) for cross-process hit
  aggregation. The shim is variadic *because* `arglist` reports only one arm of a
  multi-arm function; one new off-switch (`BROOD_NO_RELOAD_DIAG=1`) silences the
  arity diagnostic the rebinding legitimately trips.
  ✅ **line-level `nest test --cover-lines` shipped 2026-07-25** as the stricter
  second tier, in the shape this entry predicted: the seam is `Inst::RecordLine`,
  emitted by `emit_node` only when `BROOD_COVERAGE` is armed and executed by
  `exec_chunk` (which already holds the arm's `src_file`), so an ordinary run's
  bytecode is byte-for-byte unchanged; the JIT is off for the run; reporting extends
  `std/tool/coverage.blsp`. Hooking the tree-walker instead was tried and cannot work —
  a compiled body never goes through `eval`.
  The hard part was the **denominator**, and it took three attempts: counting lines
  that hold a form reported 14% for a fully-exercised fixture (different populations
  on the two sides of the ratio), and counting instrumented lines without forcing
  compilation reported 100% for a fixture with a dead function in it (arms compile on
  first *call*). `%coverage-precompile` forces every project function to compile before
  the suite, so a never-called function lands in the denominator and nowhere else.
  Fixed on the way: a baked-in std module's forms were attributed to whichever file was
  mid-`require`, so `std/log`'s lines were credited to the app's `src/main.blsp` —
  `%load-string` now takes a name, and the embedded-module table carries each module's
  repo-relative path (same literal as its `include_str!`, so they can't drift), so
  `std/log`'s forms record as `std/log.blsp` — openable, not a marker.
  ✅ **branch** coverage (`nest test --cover-branches`) — shipped 2026-08-01. Whether BOTH
  edges of each `if`/`cond`/`match` decision were taken (the strictest measure: a line reads
  as covered the moment it runs once, even if its else-branch never fires). New
  `Inst::RecordBranch(line, col, taken)` emitted at each `if`'s then/else edge when coverage
  is armed — keyed by the TEST's position, so sibling `cond`/`match` arms on shared lines stay
  distinct (a constant test is folded away, so only real decisions instrument). Shares the
  `--cover-lines` seam (bytecode instrumentation, JIT off, precompile denominator pass);
  reports fully-covered vs half-covered branches and gates `--cover-min` on the branch number.
  Primitives `%coverage-branches`/`%coverage-branch-instrumented`; reporting in
  `std/tool/coverage.blsp`; `tests/coverage_test.blsp`.
- ✅ **`nest` correctness + UX hardening pass** — 2026-07-24/25, driven by an
  adversarial harness (~50 hostile values × every flag and positional, malformed
  manifests, malformed sources, bare directories, concurrent invocations) plus
  end-to-end scaffold runs. Zero panics and zero injections survive
  (verified with a *computing* oracle, so an echoed payload can't be mistaken for an
  evaluated one). Fixed, worst first: **`nest format` silently restructured code that
  didn't parse** (the tolerant CST's recovery moved a top-level `defn` inside another
  and appended the missing paren — being mid-edit with an unclosed paren is routine,
  and format-on-save makes it automatic; `format-file` now gates on the strict reader
  and leaves the file byte-identical); **`nest add` could brick a project** two ways
  (a name that isn't a plain symbol was written verbatim into the manifest, and a
  failed resolve left an unresolvable dep behind — now round-trip-validated and
  atomic); **a misspelled manifest head was silently ignored**, dropping every
  setting; a `--partitions 0` **panic**; and a whole class of *leaked internals* —
  twelve project-scoped commands, a missing FILE, the TUI commands without a tty, and
  `nest search`'s raw error map now all report one actionable line instead of a Brood
  stack trace. `nest new` also produced projects that failed their own `nest format
  --check`; the scaffolder now formats its own output, so no future template can
  drift. Test count 810 → 842 nextest cases / 3161 in-language, with the suite's
  warning channel now empty. Concurrent `nest add`/`remove` also lost an
  update (measured: 1–3 of 3 landing); fixed with a locked compare-and-swap over one
  new primitive, `%file-swap` (`docs/packages.md`).
- ✅ **Shell completion (`nest completions`)** — shipped 2026-07-24
  ([`docs/tooling.md`](docs/tooling.md) §Shell completion). TAB completion for
  bash/zsh/fish covering subcommands, flags, **and project-aware values**: test
  files, `:tags` for `--only`/`--exclude`/`--include`, declared dependency names,
  module names, and `ValueEnum` choices. Split by which side owns the truth —
  subcommands + flags are derived from **clap's own argument model** (so a new flag
  is completable immediately and a renamed one can't leave a stale completion), while
  project-dependent values come from `std/tool/complete.blsp` and only when the
  cursor is at a value position, so the common case never pays interpreter startup.
  Two tested invariants: completion **never fails** (exit 0, empty stderr, whatever
  it's handed — no project, unparseable manifest, hostile text) and **silence means
  fall back** to the shell's filename completion. One new primitive,
  `(builtin-modules)`, exposes the Rust-side baked-in module table.
  `crates/nest/tests/complete.rs` (18) + `tests/complete_test.blsp` (38).
- ✅ **`nest format --changed`** — shipped 2026-07-23. A git-aware narrower
  scope: formats only the `.blsp` files git reports not-committed-clean
  (modified/staged/untracked), via a new `%git-changed-files` primitive
  (returns the paths, or `:not-a-repo`); falls back to the whole project
  outside a git repo, and `--check` still scans everything (CI's clean-tree
  gate). `crates/nest/tests/format_changed.rs`.
- 🟡 **LSP** — tiers 0–2 ship. ✅ **finer finding spans** shipped 2026-07-23:
  a type-mismatch / callback-arity finding now anchors at the offending
  **argument** when it is a positioned sub-form (a nested call —
  `(string-length (+ 1 2))` points at `(+ 1 2)`), falling back to the call
  head only for a bare literal/symbol (which the pair-keyed position table
  doesn't record). No `Pos`-threading rewrite needed — the argument value
  already carries its position. ✅ The **create-missing-`defn`** code action
  already ships (verified 2026-07-23 — `create_defn_action` in
  `crates/lsp/src/code_actions.rs`, a stub `(defn foo (a b …) nil)` with arity
  matched to the call site, tested; the roadmap line was stale). ✅
  **incremental document sync** shipped 2026-07-24: the server advertises
  `TextDocumentSyncKind::INCREMENTAL` and splices each `didChange` range into the
  stored buffer via the UTF-16 `LineIndex` (`apply_content_change`, per-edit index
  rebuild so a batch compounds correctly); the parse stays whole-document (cheap).
  2 new tests (ranged-splice offset precision; multi-edit batch compounding); 116
  LSP tests green. ✅ **Range semantic tokens shipped 2026-08-01**
  (`textDocument/semanticTokens/range`): the capability is advertised and the handler
  computes the whole-document token stream (cheap off the cached CST) and filters it to the
  requested range, so a large file's editor can classify just the visible viewport
  (`semantic_tokens::semantic_tokens_range`; test `range_returns_only_the_tokens_it_covers`).
  ⬜ **Delta** semantic tokens stay deferred — they need result-id caching + per-edit diffing
  for a payoff the roadmap already judged marginal (the walk is cheap), so not worth the
  stateful machinery until profiling shows the full/range responses hurt.
- 🟡 **Errors that teach (LLM-native)** ([`docs/llm-native.md`](docs/llm-native.md))
  — first instances landed. ✅ **reader-level hints** for the Clojure/Scheme
  syntax the reader mis-parses shipped 2026-07-23: `#{…}` (set literal),
  `#(…)` (anonymous-fn reader macro), and `#'` (var-quote) now raise a clean
  parse error with a `:hint` naming the Brood idiom (was "odd map" / "unbound
  #"); Scheme/Clojure **nested `let`/`letrec` bindings** `((a 1) (b 2))` raise
  a hint to flatten (`tests/reader_hints_test.blsp`). ✅ **The
  `explain-error` / `find-pattern` MCP tools + the intent→idiom cookbook**
  shipped 2026-07-23 (`std/tool/explain.blsp`, a new baked-in `explain`
  module): `explain-error` maps a stable E-code (or a caught error's `:code`,
  or a kind keyword) to `{:summary :causes :fix :example}` — the
  Brood-idiomatic fix, not just the message; `find-pattern` keyword-searches a
  curated intent→idiom cookbook ("loop / mutate / build a string / spawn /
  parse binary …"). Both are pure Brood data + wrappers, wired as `nest mcp`
  tools (now 20 in a project context). `tests/explain_test.blsp` + `mcp_test`. ✅ **cookbook expanded
  2026-07-24** — five confirmed Clojure/Scheme reflexes folded in: keyword-as-fn
  `(:k m)` → `(get m :k)`, char literal `\c` → 1-char string / `int->char`,
  discard `#_` → `;`, regex `#"…"` → `(require 'regex)` + `regex/match?`, and the
  `#{…}` set entry updated to the now-first-class kernel literal. ✅ **reader hints
  for the last three silently-mis-parsed forms shipped 2026-07-24**: `#_` (discard →
  `;` comment), `#"…"` (regex literal → `(require 'regex)` + `regex/match?`), and `\c`
  (character literal → 1-char string / `int->char`), each a clean parse error with a
  `:hint` naming the Brood idiom (`read_hash` + a new `'\\'` arm in `read_form`;
  `tests/reader_hints_test.blsp`, language.md table). ⬜ Still: folding each new repeat
  mistake into the rule-of-three (ongoing curation, no named gap).
- 🟡 **MCP tooling** — ✅ the write sandbox is **symlink-escape-proof** (shipped
  2026-07-23): a new `canonicalize` primitive (real-path resolution — symlinks
  + `.`/`..`, works for not-yet-existing targets) backs a second sandbox gate
  in `mcp--project-path`, so a project-relative `..`-free path that resolves out
  of the tree through a symlinked directory is now rejected, not just the
  lexical cases (`crates/lisp/tests/mcp_sandbox.rs`). ✅ **the streaming /
  progress-notification tier** shipped 2026-07-23: a `tools/call` carrying an
  MCP `_meta.progressToken` arms a sink so a handler's `(mcp-progress progress
  total message)` streams `notifications/progress` to the client *during* the
  synchronous call (via the reentrant stdout lock); the core `check` tool
  reports per-file, and `%mcp-progress` is a no-op elsewhere so any handler
  can call it safely (tests in `mcp.rs`). ✅ **GC/process *traces* exposed**
  (shipped 2026-07-24): a new `watch-runtime` MCP tool arms the kernel
  `system-monitor` on the handler process for a bounded window (`:ms`, capped
  5 s, with an optional `:filter` kind selector), then returns the collected
  `[:runtime kind pid detail]` stream — GC pauses, spawn/exit churn, JIT deopts —
  a *trace*, not a snapshot, complementing `processes`/`node`. Pure Brood over the
  telemetry/`system-monitor` seam (`std/tool/mcp.blsp`); tests in
  `tests/mcp_test.blsp`. ⬜ Still: none named — the snapshot-vs-stream gap is now
  closed.

### Editor (M2) & display (M3)

- ⬜ **Major/minor modes** — how a buffer selects which keymaps are active.
- ⬜ **Mouse / resize input events** — deferred until a feature needs them.
- ⬜ **GPU-window frontend** — a later additive path speaking the same display
  protocol; arbitrary per-px buffer sizing rides with it.
- 🟡 **Telemetry** (ADR-106) — core landed; the kernel event *sources* shipped
  2026-07-19 (ADR-137: `system-monitor` → `telemetry/watch-runtime`, GC/spawn/exit/
  deopt as `[:runtime kind]` events). ✅ **Metric aggregators + sampling** shipped
  2026-07-24: `counter`/`sum`/`last-value` (gauge)/`summary` (running
  count/mean/stddev/min/max) + `sample-every` (1-in-N), the Elixir
  `Telemetry.Metrics` set folded entirely in Brood over the `attach` seam — zero
  new kernel surface. State is a shared `table` (ADR-107); folds run serially in the
  one listener so a read-modify-write is race-free and stays bounded (no sample
  retention). `metric`/`metrics-snapshot` readers poll it atomically.
  `tests/telemetry_metrics_test.blsp`. ✅ **Distribution/histogram aggregator +
  node up/down shipped 2026-07-24** (both pure Brood, zero new kernel surface):
  `distribution` buckets a measurement into explicit ascending upper bounds
  (Prometheus / Elixir-`distribution` shape) with bounded per-bucket counts (no
  samples retained, like `summary`), and `metric-percentile` estimates a quantile by
  in-bucket linear interpolation (`histogram_quantile` — bounded memory for approximate
  percentiles); `watch-nodes` polls `(nodes)`, diffs the peer set, and re-emits
  `[:runtime :nodeup]`/`[:runtime :nodedown]` through the same `[:runtime kind]` seam as
  `watch-runtime` (polling because the kernel has no `[:nodeup]` event — it catches both
  inbound peers and outbound `connect`s). `tests/telemetry_metrics_test.blsp` now 15.
  ✅ **`defevent` + checker-validated schemas — shipped 2026-08-01.** A telemetry event now
  carries a declared shape: `(defevent name event :measurements ((f type)…) :metadata ((f
  type)…))` registers the schema (`event-schema`/`telemetry-events`) and defines a **typed
  emitter function** whose emitted `(sig …)` makes `(name 42 "/" 200)` a checker-validated
  call — reusing the `sig` seam exactly as `defrecord` does for field types, so zero new
  checker machinery (verified: a wrong arg type and a wrong arity both warn). Raw `emit`
  still works; `(telemetry-validate! true)` enables an advisory listener-side presence-check
  of declared events (`telemetry--check-event`, pure + tested). `tests/telemetry_metrics_test.blsp`.
  ✅ **Snapshots unified behind the stream — shipped 2026-08-01.** `runtime-snapshot` folds
  `gc-stats`/`sched-stats`/`vm-stats`/process-count/`metrics-snapshot` into one map (dev-tools
  stats read only when bound, lean-safe), and `watch-vitals` samples it onto the stream as
  `[:runtime :vitals]` events, foldable into gauges via `last-value` — one subscription
  instead of scraping N builtins. `nest mcp` gains a `vitals` tool over it (the point-in-time
  companion to `watch-runtime`). ✅ **Remote tier — shipped 2026-08-01.** `telemetry-serve`
  registers a `:telemetry-remote` agent (opt-in, after `node-start`, like `observe-serve`);
  a peer `(subscribe-remote node event)` streams that event's emits over the node link — the
  target attaches a forwarder that `send`s each matching emit to the subscriber (transparent
  cross-node `send`), which re-emits it into ITS OWN telemetry stream tagged `{:remote-event
  …}`, so local handlers/aggregators consume a peer's events through the same attach seam.
  No new wire format (ADR-053's observer pattern). Protocol tested single-runtime in
  `tests/telemetry_metrics_test.blsp`; cross-node routing is transparent dist `send` (covered
  by the dist suite).

### Server / daemon (M4)

- ✅ **Inbound (server-side) TLS + the `mio` reactor** — shipped 2026-07-22
  (ADR-143, the parity program's item 3 above): one reactor thread for every
  socket, full-duplex TLS driven sans-io (the read/write-split constraint
  dissolved), `serve-loop` serves https unchanged. M4 is complete.
  - ✅ **Reactor reap hardening** (2026-07-24): a **TLS handshake-completion
    timeout** (default-on, 30 s — a peer that stalls mid-handshake holds an fd the
    app can't reclaim, since it never sees the socket until the handshake
    finishes), and an **opt-in per-socket idle timeout** (`tcp-set-idle-timeout`,
    default off — slow-loris protection a server arms on untrusted connections;
    off by default so a legitimately long-idle stream — SSE, long-poll, the editor
    daemon — is never reaped). The defence-in-depth complement to the client-side
    `tcp-close` fixes (2026-07-24 validation round 3).
- ✅ **OTP near-term** — all three closed as of 2026-07-18:
  **`send-after`/`send-interval`/`cancel-timer`** shipped (pure Brood in the
  prelude — a timer is a green process on the scheduler's timer wheel; the
  interval variant monitors its target and self-cleans; `tests/timer_test.blsp`).
  The other two turned out to be **stale roadmap entries** — a synchronous
  **`remote-spawn-sync` returning the child pid** and the **`[:$stop]`
  graceful-teardown convention** (supervisor `:shutdown` policies + `defprocess`
  `terminate`) had both already shipped.
- ⬜ **OTP deferred** (ADR-011, gated on a real consumer; **`gen_statem`, `Registry`/`pg` and
  `Application` now have concrete shapes as items D, E and B of the 2026-08-30 backlog above**): **`gen_statem`** state
  machines; an Elixir-style **`Registry`**/via-tuples + **process groups (`pg`)**; an
  **`Application`** behaviour; **synchronous, ordered, rollback-on-failure** supervisor
  startup + per-child intensity counting + child `type`/`significant`/`auto_shutdown`
  metadata.
- ✅ **Hot-upgrade state migration — a userland `code_change`** (ADR-013/039;
  [`live-editing.md`](docs/live-editing.md) **Stage 6**, shipped 2026-08-06). A long-lived
  `gen`/loop kept threading its **old-shaped state** after a reload (gap #4). Fixed in Brood
  (no kernel `code_change`, matching the ADR-039 supervision call): `defprocess` gained a
  `(code-change old-state body…)` clause that migrates the loop's state on a `[:$code-change]`
  envelope, `(gen/code-change pid)` is the push trigger (a supervisor / `on-reload` hook calls
  it per affected child; no clause → safe no-op), and `reload/*code-version*` (bumped on each
  successful reload, read via `(reload/code-version)`) is the pull signal for loops that poll.
  Tested inline + cross-process in `tests/gen_test.blsp`. This is also the hand-off a running
  server needs for a **dispatch-logic** upgrade: since 2026-07-30 a top-level `defn` self-loop
  picks up its own redefinition (commit `4bbef7d9`), but an inner `letrec` loop — the shape
  `defprocess` expands to — still runs old code until it re-enters through a global. ⬜ Full
  OTP `release_handler`/appup orchestration (coordinated suspend/upgrade/resume/rollback across
  a supervision tree) stays deferred: Brood's immutable-map data removes OTP's hardest upgrade
  case (nominal schema migration — "a map has the keys it has"), leaving only loop-state drift,
  which this covers.
- 🟡 **Dist refinements** (ADR-011): ✅ exact propagated exit reason for a
  *non-trapping* linked peer (fixed 2026-07-18 — hardness split from the reason,
  see the survey housekeeping item above; the shared `deliver_exit_to` covers
  remote links too). Still ⬜: a `terminate/2` hook on hard kill (**reassessed 2026-08-30 as by-design — OTP's
  does not run on `kill` either; see item C of the Lisp-survey backlog**); **long-name
  FQDN resolution** (a long name is passed explicitly today, no resolver);
  Windows Unix-socket transport.

### Packaging & ecosystem

- ✅ **Package manager** (ADR-037/147, [`docs/packages.md`](docs/packages.md)) —
  `:path` deps end-to-end, **`:git` deps** (slice 2), and **the verbs + auto-fetch**
  (slice 3: `nest fetch`/`update`/`tree`/`add`/`remove`) all shipped 2026-05-30
  (`%git-clone`/`%git-resolve-ref` in `builtins/io.rs`, policy in
  `std/tool/package.blsp`). ✅ **v2 shipped 2026-07-24 (ADR-147):** **`:tarball`
  deps** (`[name :tarball URL :sha256 HEX]` — download via `std/net` `http-get` or
  `file://`, mandatory sha256 verify, strip-extract via the new `%untar-gz` shell to
  `tar` on the offload pool) and a **registry**. NOTE (ADR-211): the registry shipped
  as a **hosted HTTP/tarball service** — the sibling **hive** app (Brood/Hatch/Postgres) —
  **not** the git-backed index ADR-147 first described; a release carries an immutable,
  sha256-pinned **tarball** + dependency metadata, `nest publish` POSTs a token-authed
  upload, `nest search`/resolve hit a JSON API (`/api/v1/packages/:name/releases` answers
  the resolver's per-package query in one request), and `[name :version "X"]` resolves a
  range from live metadata. ✅ **semver-range
  resolution shipped (ADR-209):** the version grammar (`^`/`~>`/conjunctions in
  `std/version.blsp`), a **PubGrub (CDCL), newest-compatible solver**
  (`std/resolver.blsp`, driven by an injected provider so it is exhaustively testable
  offline), the registry wiring (a `:version` manifest entry is a range, resolved from
  live published metadata into the lock, with a network-free fast path when the lock
  fully covers), and a `:brood "<constraint>"` runtime gate (checked at setup against
  the `(brood-version)` primitive), and a lock-**preference** seed so adding one dep keeps
  the rest pinned instead of re-solving the closure to newest. The solver is validated by
  a generative oracle fuzz against brute-force enumeration (3400+ random universes, 100%
  agreement). Conflicts render a **minimized, pub-style structured proof** — a "Because … and …,
  <consequence>" chain over the incompatibilities PubGrub actually resolved, naming only the
  requirers that truly clash — and **pre-releases** are ordered and matched per the npm/Cargo
  rule (a plain range excludes them; one that names a pre-release admits it).
  ✅ **external tarball URLs shipped (ADR-211, 2026-08-04):** a release may point at a
  GitHub/S3/CDN asset (`:source_url`) instead of hive holding the bytes — still checksum-pinned
  (the publisher hashes the URL, every downloader re-verifies), created with `nest publish
  --source-url URL`. ✅ **package signing shipped (ADR-212, 2026-08-04):** ed25519 signatures
  (`%ed25519-*` primitive), **TOFU + advisory** — `nest key gen` makes a keypair, `nest publish`
  signs the checksum, and the client verifies + pins the signer's key in the lock on first
  install, warning on a change; the registry only relays the signature (not a keyserver). The
  one open supply-chain gap (sha256 proves integrity, not authorship) is now closed.
  ✅ **semver ranges over `:git` tags shipped (ADR-209 seventh refinement, 2026-08-06):** a
  `:git` dep may track a `:version` range (`[foo :git URL :version "^1.2.0"]`) instead of an exact
  `:ref`; the newest matching tag is picked (via `%git-list-tags`), the resolved version is locked
  (network-free re-runs), and `nest update` advances it. Greedy per-dep (not a cross-package
  PubGrub member) — the deferred unified-git solve is recorded with its ADR-011 reason. This was
  the resolver's **last** deferred algorithm item. *(Tarball-backed registry entries and
  cloned-index auto-refresh from the old list are done / moot under the hosted design; a
  client-side response TTL cache was investigated and declined — see ADR-211. Still deferred:
  semver ranges over `:git`-**tarball** sources.)*
- ⬜ **Single-binary bundling** (ADR-038) — `nest bundle` appends a zip of
  project + `_deps/` to a pre-built `brood`; deferred until the editor needs end-user
  distribution.
- ⬜ **`nest release`** — a self-extracting filesystem for runtime data files, a
  static-musl default, and `.deb`/`cargo install` packaging of the *runtime* (open
  until a real consumer needs it).
- 🟡 **tree-sitter grammar + GitHub recognition** — editor grammars (`nest grammar`,
  ADR-092), the `tree-sitter-brood` parser, and `brood-vscode` all ship; ⬜ **publish
  it** (editor bindings/CI) and file the ⬜ **`github/linguist` PR** (gated on `.blsp`
  adoption across many repos). Today a `.gitattributes` Clojure stopgap.

---

## Design notes (context for the above)

### Types — goal & the hot-reload constraint

The target is Elixir's sound, gating, whole-program checker for the *interior* of
code, kept compatible with Erlang-style hot reload for *globals and module
boundaries*. Globals stay `dynamic()` because hot reload rebinds them via `def`, so a
type proven at check time can be falsified by a later reload; what *can* be gated is
everything local — `let`/`fn`-param bindings, call arity and argument types, `match`
coverage, `sig!` contracts — while global `def`/`defn` types, inter-module flow, and
global-fn return types stay advisory. Slogan: *Elixir's checker for the interior,
Erlang's late binding for globals and module boundaries.* The full-soundness-vs-reload
mechanism has shipped (re-check per reload rather than prove once — ADR-123/124/125),
and the old "checking never rejects a runnable program" invariant has been revised
throughout (`CLAUDE.md`, [`docs/types.md`](docs/types.md) contract #5): the checker
never gates the live image; the one hard reject is batch/CI (`nest check` exits
nonzero on any warning).

### Telemetry — what we improve over Erlang's `:telemetry`

Async-by-default delivery (handlers run off the emitting process); events as data
with a declared schema (`defevent`, checker-validated) rather than bare atoms; an
immutable, process-backed handler registry; location-transparency over the dist link;
and built-in metric aggregation (counter/gauge/summary/histogram) + sampling — folding
today's ad-hoc `gc-stats`/`vm-stats`/`process-info` instrumentation behind one stream.

---

## Cross-cutting open questions (revisit, don't build yet)

- **Shipping a runtime binary** — a self-extracting filesystem for data files,
  static-musl default, `.deb`/`cargo install` (see `nest release` above); open until a
  real consumer needs it.
- **Publishing the grammar** — the `github/linguist` PR isn't filable day-one; it's
  gated on `.blsp` adoption across hundreds of repos.

---

## Language features — candidates, held at arm's length

**The default answer here is no.** This section exists so a recurring idea gets recorded
once with its counter-argument, instead of being re-litigated every few months and
occasionally winning on enthusiasm. Nothing below is scheduled.

The bar a candidate has to clear, in order:

1. **It buys a capability, not a spelling.** An alias for something we already have is
   not a feature — ADR-227 already forbids bare re-export aliases, and greenfield means
   we delete the old spelling rather than keep both.
2. **It pays for the bare vocabulary it spends.** ADR-250 established the root namespace
   as the scarce resource and clawed back 203 words; anything taking more owes an
   argument. A module namespace (ADR-251) is the cheap way to add without spending.
3. **It does not grow the evaluator.** Measured 2026-08-28: Brood has **nine** true
   evaluator forms — `catch def do fn if let letrec quasiquote quote` — against Elixir's
   ~25 `Kernel.SpecialForms`. The other 31 entries `(special-forms)` reports are prelude
   macros. That ratio is an asset and the thing to protect: prefer a primitive plus a
   macro over a special form, every time.

### Held candidates

- **`car`/`cdr`/`cadr`/`caddr`…** — asked for on grounds of Lisp familiarity. Fails bar 1
  outright: it is an alias set over `first`/`rest`/`second`/`third`/`nth`, and an
  open-ended one. `c[ad]+r` also encodes a path inside a name, which stops being readable
  at two levels (`(cadddr x)` against `(nth x 3)`); Clojure dropped them for that reason.
  *If the pull is real*, the shape that clears the bars is an opt-in **`lisp` compatibility
  module** — `(:use lisp)` makes them bare inside one file and costs root nothing.
- **A declared shadow in `defmodule`** (`(:exclude-core [get])`, the Clojure
  `:refer-clojure :exclude` / Common Lisp `(:shadow …)` shape). Brood currently sits at
  the permissive end for module code: a `(defn get …)` silently shadows, with no signal to
  a reader. Deferred, not rejected — KI-73 removed the *breakage* that made it urgent, so
  what remains is a legibility argument, and ADR-011 says wait for a concrete need.
- **Full free-reference macro hygiene.** Would subsume the `/name` root-escape discipline
  that `tests/prelude_capture_test.blsp` now enforces by hand. ADR-066 rejected
  Scheme-style per-symbol lexical context on ship-by-name/homoiconicity/GC grounds, and
  that reasoning still holds; the gate is the cheap 90%.

### Union dispatch positions — and why the type system is the easy half

> **Designed 2026-08-28: [`docs/union-dispatch-design.md`](docs/union-dispatch-design.md)** —
> the specificity rule, the four questions checked empirically, and the build order.
>
> **A prior decision now makes this step 3 of three.** Cross-type numeric arithmetic and
> comparison will **raise unless a method is declared** for the pair — implemented by widening
> `num_multi_dispatch`'s trigger from "an operand is a record" to "the operands are different
> numeric types" and *deleting* the float-contagion branch, with the tower shipped as an
> opt-in `std/num/tower.blsp`. Measured blast radius: **27 call sites, 0 at boot**. That
> decision is what turns union positions from a convenience into a capability.
>
> **Attempted strict-first on 2026-08-28 and reverted** — the order is backwards. The
> mechanism works (15 lines, correct loud error), but `->float` is itself implemented with
> mixed arithmetic, so the declarations cannot bootstrap; and by hand the tower needs **36**
> methods versus **4** with unions. Revised sequence: typed `defmulti` → union positions →
> a promotion primitive → strict default.

The question that prompted this: can a dispatch position accept a *union*
(`[usd (or :int :float)]`), and how does the checker derive the result?

**Where things stand.** `defmulti`/`defmethod` already do the multi-argument half:

```lisp
(defmethod convert [usd eur] (a b) …)     ; per-argument types
(defmethod scale   [usd :int] (a n) …)    ; record x built-in kind
(defmulti mix :commutative)               ; author [usd eur]; [eur usd] derived
```

A union in a position is a clean compile error ("each position must be a record name or a
built-in id keyword"). `defability` is **single-dispatch** — the generated op dispatches on
`(%identity-of first-arg)` (`std/prelude/tools.blsp`), first argument only.

**The type system is not the obstacle — it is already solved, and for a structural reason.**
An ability op declares its own return (`(compare-to [self other] :-> int)`), and
`check_impl_returns` verifies *every impl body* against it. So `(area s)` types as the op's
declared `:-> RET` **whichever impl runs**; inference never joins return types across impls
(`types/check/infer.rs`: "the declared return is the only static handle… a contract, not a
guess"). Adding a union to a dispatch position therefore changes **nothing** about the result
type. Unions are free on the inference side, precisely because an ability separates the
contract (the op) from the implementations.

`defmulti` is the opposite: it declares no return at all, so a multimethod call is already
opaque to the checker. A union there costs nothing and gains nothing.

**So the ordering is the reverse of what it looks like.** The union is the easy part; the
missing piece is **multi-argument ability dispatch** — abilities are where return types live,
and they only dispatch on argument one. Sequence: multi-arg abilities first, then union
positions nearly fall out.

**Two things to get right when it is built.**

- **Keep dispatch nominal.** A union position must reduce to a *set of ids*
  (`(or :int :float)` → `{:int, :float}`), not an arbitrary type. Dispatch is identity-based
  (ADR-177/179) and must stay that way; borrowing the type language's *spelling* is fine,
  borrowing its structural types is not — `(or (record …) int)` has no id to dispatch on.
- **It is an inference GAIN, not a cost.** Ability ops declare a return but not argument
  types. A union position is the first construct where dispatch and typing coincide, so the
  dispatch table becomes a free source of argument types: `[usd (or :int :float)]` tells the
  checker argument 2 is `int|float` at every call site. Today it knows nothing about it.

Against the bar: it fails test 1 as a pure convenience (two methods already express one
union), and passes it for the numeric tower, where a binary op over `int`/`float`/`decimal`/
`ratio` needs 16 methods that `:commutative` only halves. Build it when a `Comparable`-style
ability over the tower is actually wanted — that is the concrete need that makes it a
capability rather than a spelling.

### Warning suppression — configurable, and currently only at one granularity

Today there is exactly one lever: **`(check-allow :category form…)`**, a source pragma
wrapping the forms it covers (`docs/type-annotations.md` §"Suppressing an advisory lint on
purpose"). It is the right *innermost* granularity — the suppression sits on the code it
excuses, and a reader sees it — but it is the only one, so there is no way to say:

- **per function** — "this whole `defn` is a deliberate non-tail loop", without wrapping
  the body and re-indenting it;
- **per file** — "this file is generated / is a test fixture / deliberately shadows core";
- **globally, per project** — a `project.blsp` key, e.g. an advisory the team has decided
  it does not want, or one a dependency's generated code trips.

The last is the one with teeth, because `nest check` exits nonzero on **any** warning in
batch/CI (ADR-023/024) — so a single lint the project disagrees with currently blocks CI
with no supported answer but editing every site.

Design notes for whoever picks it up:

- **Keep `check-allow` the innermost and most specific**; broader scopes should widen, never
  replace it. A file-level or project-level blanket is exactly how a real warning gets lost,
  so the broader the scope, the louder it should be in the output — `nest check` should
  *report* what a project globally suppresses rather than silently honouring it.
- **The category vocabulary already exists** (`:non-tail-recursion`, `:unreachable-clause`,
  `:unbound`, `:type-mismatch`, `:unrequired`, …) — see `types/check/ctx.rs`. Whatever the
  new scopes are, they should take the same keywords, not a second vocabulary.
- **Suppression is not the same as opting out of the gate.** ADR-124/125 are firm that the
  checker never gates the live image; this is only about the batch/CI exit code.

### Reduce before adding

The more valuable direction, and it needs no evaluator change — both families are
prelude macros:

- **Four iteration forms** — `for` / `doseq` / `dolist` / `dotimes`.
- **Three matching forms** — `match` / `match*` / `case`.
- `unless` is `when` + `not`; `with-out-str` / `with-err-str` are one idea twice.

---

## Killed directions (don't retry)

- ❌ **Kernel-supervised processes** (ADR-039) — reverted 2026-05-29; it was the bulk
  of the multi-thread scheduler race surface. Userland supervision replaces it;
  named-spawn is intentionally not delivered in the kernel.
- ❌ **JIT Stage 3, Increment 2** (in-IR frame setup + `call_indirect`) — NO-GO,
  confirmed twice. The call-heavy win is leaf inlining, not cheapening the call itself.

---

## Out of scope (deferred, additive later)

- `&key` named arguments (designed — ADR-011) and supplied-p flags
- Hygienic macros / `macroexpand-all`
- Rationals (ints already auto-widen to big integers on overflow; f64 +
  decimals cover the rest)
- True per-file **namespaces** — flat Emacs-style `provide`/`require` is in scope
  (ADR-019); real namespace isolation stays a later, additive Brood macro layer
- Characters as a distinct type (chars are 1-char strings)

---

## Guiding principles

1. **Policy in Brood, mechanism in Rust.** Prefer a primitive + a prelude macro over
   a new special form; write as much as possible in Brood itself.
2. **The frontend is a protocol.** The display seam is serialisable render-ops, so a
   terminal, a GPU window, or a remote client are all just consumers.
3. **Every milestone is usable on its own** — the language stands without the editor,
   the editor without the server.
