# Live editing — hardening hot-reload toward Emacs-grade

> Status: **partially shipped.** The hot-reload *mechanism* is built and
> documented in [`shared-code.md`](shared-code.md) (shared RUNTIME region,
> late-bound globals, append-only code). This doc is the *next* layer: the
> handful of things still missing before you can edit the running editor all
> day the way you edit a running Emacs. **Stages 1, 2, the dedup half of 5, and
> 7 shipped 2026-05-29 under [ADR-042](decisions.md); Stages 3, 4, 6, and the
> collector half of 5 remain.** Each stage lands behind `cargo test` + the Brood
> suite and gets an ADR in [`decisions.md`](decisions.md) when it ships.

## The bar

The project's whole point is "edit the editor while it runs"
([`shared-code.md`](shared-code.md) §Why). The bar is **Emacs**: redefine a
function and the change is live on the next call, with no restart, no lost
state, all day. We don't need every Emacs feature — we need the *core loop* to
feel the same.

## What already works (verified 2026-05-29 against the live image)

The foundation is real, not aspirational. Confirmed by evaluating against a
running `nest mcp` image:

- **Redefine-and-see-it** — `(def inc (fn (x) (+ x 100)))` then `(inc 1)` → `101`.
  Late binding through the per-runtime globals table (`heap.rs` `RuntimeCode`).
- **Overrides reach the prelude.** The prelude is seeded into the *mutable*
  globals table, so a `def` shadows a prelude name — and because prelude
  closures resolve their free references through that same live table,
  **prelude internals pick up your override too.** Verified: overriding `first`
  made the prelude's `(defn second (coll) (first (rest coll)))` return the
  overridden value. This is bigger than I first assumed — "open up the prelude"
  is *mostly already done* (see Stage 8).
- **Cross-process** — a `def` reaches every spawned process on its next lookup
  (the shared `Arc<RuntimeCode>`).
- **In-flight calls are safe** — append-only code means a call already running
  the old closure finishes on it; the next call gets the new one.
- **Process-threaded state already survives reload.** This Lisp is strictly
  immutable (ADR-026 — `def` is the only mutation; no `atom`/`set!`). The
  idiomatic place for editor state is therefore a long-running green process
  that threads its state through its own loop argument. That state is
  *per-process data*, not a global binding, so reload doesn't touch it — the
  loop keeps its state and picks up new code via late binding. The state problem
  below is real but *narrower* than "Emacs loses nothing": it's specifically
  about state and resources bound at **global** scope.
- **A reload trigger exists** — `std/reload.blsp` polls file mtimes and calls
  `reload-defs`, which re-evals only `def…` top-level forms (skipping
  side-effecting calls like `(main-loop 0)`).

## The gap (why it isn't Emacs yet)

Five things, roughly in order of how soon they bite:

1. **Reload clobbers global state and re-creates singletons.** `def` always
   re-evaluates its RHS, and `reload-defs` re-runs every `def…` form. Two sharp
   consequences at *global* scope (process-threaded state is fine — see above):
   - **Global cells reset:** `(def *registry* {})` is set back to empty on every
     save.
   - **Singletons/resources duplicate:** `(def *server* (spawn (serve)))` or
     `(def *db* (open-conn))` at top level **re-runs on every save**, spawning a
     second server / opening a second connection while the first leaks. This is
     the nastier of the two.
   Emacs solves the analogous problem with `defvar` (init only if unbound); we
   have no equivalent yet. **This blocks editing anything that owns a global
   resource.**
2. **Memory grows without bound.** The RUNTIME region is append-only and never
   purged — every redefinition leaks the previous version forever. Erlang keeps
   exactly two versions and purges precisely *because* of the edit-all-day
   workload.
3. **Polling, not editor-driven.** The primary path is a 250 ms mtime poll
   (one green process per watched file). Emacs is `C-M-x` — eval *this form,
   now*, zero latency. We have the better primitive already (MCP `eval`/`load`);
   it just isn't wired to the editor as the headline path.
4. **No state migration across a shape change.** A running loop picks up new
   *code* but keeps its old-shaped *state*. Change the shape of the map a loop
   threads and the running loop feeds old data to new code. Erlang has
   `code_change/3`; we have nothing.
5. **Stale macro expansions.** Redefining a macro doesn't re-expand callers
   already compiled with the old expansion — and nothing warns you.

## Guiding principle

**Simplicity over performance — to a degree.** Where a simple design is merely
*slower*, take it and note the cost. Where a simple design is *wrong* (loses
state, corrupts memory, lies about success), don't. Stage 5 (bounded memory) is
the explicit "to a degree" line: we accept a slow, quantified leak now in
exchange for keeping the lock-free read path simple, and design the real
collector as a separate, later, well-scoped piece of work.

---

## Stage 1 — `defonce`: state survives reload  ·  *the blocker*

**Problem.** Reload clobbers global cells and re-creates global singletons
(gap #1).

**Design.** Add `defonce` — evaluate the init form *only if the symbol is not
already bound*; otherwise leave the existing binding untouched. This is Emacs's
`defvar` / Clojure's `defonce`. Global state and resources use `defonce`
(`(defonce *server* (spawn (serve)))`, `(defonce *registry* {})`); plain
behaviour uses `defn`/`def`.

It's a **pure prelude macro — no kernel change at all**, because the needed
predicate `(bound? 'sym)` already exists (`builtins.rs:622`) and `unless` is in
the prelude:

```clojure
(defmacro defonce (name val)
  "Bind `name` to `val` only if it isn't already bound — so the value
survives hot-reload (cf. Emacs `defvar`). Re-evaluating this form is a
no-op once bound; restart or re-`def` to force re-init."
  `(unless (bound? '~name) (def ~name ~val)))
```

**Why it composes with reload for free.** `reload-defs` re-evals forms whose
head starts with `def` — `defonce` qualifies, gets re-evaluated on every save,
and no-ops itself when already bound. Nothing in the watcher changes.

**Prototyped & verified (2026-05-29).** Defining this macro in the live image
and running `(defonce *demo-state* 41)` → `(def *demo-state* 99)` →
`(defonce *demo-state* 41)` left `*demo-state*` at **99** — the second `defonce`
no-ops, state preserved. `(bound? 'unbound-sym)` returns `false` (not an error),
so the macro is safe on first definition. The whole stage is essentially these
two lines plus tests + docs + adding it to `std/prelude.blsp`.

**One caveat to document.** `bound?` checks *any* binding in scope, not just the
global. At top level that's the global, so `defonce` is correct there; just note
it's intended for top-level use (which is the only place reload re-evaluates
anyway).

**Simplicity / perf.** As small as it gets: one prelude macro, zero new kernel
surface. No perf concern.

**Done when.** A watched file with `(defonce *server* (spawn (serve)))` plus the
server's handler `defn`s keeps the *same* server process across saves (no
duplicate spawn) while handler edits go live immediately. `cargo test` + suite
green. → **ADR-042 (shipped 2026-05-29).**

---

## Stage 2 — `reload-defs` hardening: atomic, honest detection

**Problem.** Two sub-issues with `reload-defs` (`builtins.rs:1552`):
- **Atomicity.** It evals forms one at a time and `break`s on the first error,
  which *can* leave a file half-reloaded. (Note: a **syntax error is already
  atomic** — `read_all_positioned` parses the whole file before any eval, so a
  half-saved/unparseable file applies *zero* defs. The residual window is a
  *runtime* error while evaluating form N, after forms 1..N-1 already landed.)
- **Detection.** `head.starts_with("def")` (`builtins.rs:1573`) over-matches a
  top-level call to a user fn named `default-…` and under-matches a definition
  produced by a user macro whose name doesn't start with `def`.

**Design.**
- **Atomicity (cheap 90%):** macroexpand *all* def forms before evaluating
  *any*. This closes the macroexpansion-error window too (broken macro call in a
  half-saved file → zero defs applied, same as a syntax error). The residual
  runtime-error-mid-eval case stays; a true snapshot-and-rollback of the
  affected global bindings is possible but deferred — it's rare and the leak it
  prevents is "some defs newer than others," not corruption.
- **Detection:** accept the heuristic but tighten it. A *macro-defined* definer
  (`defcomponent`, `defcommand`) legitimately starts with `def`, so the head
  check is right for those. The only real false positive is a top-level
  *function call* whose name starts with `def`, which doesn't belong in a
  reloadable file anyway. Document the contract: **a reloadable file contains
  definitions plus, optionally, a single entry call that the runner — not
  reload — invokes.** Optionally match the head against the known kernel
  def-forms *plus* the `def`-prefix rule, to drop the `(default-config)` false
  positive. The under-match (a macro *not* named `def…` that expands to a
  definition) stays a known limitation with a trivial workaround: **name
  definer macros with a `def` prefix** — which is the Lisp convention anyway. No
  dependency graph, no registry.

**Simplicity / perf.** Reorder-then-eval is simpler to reason about and no
slower in practice (expansion is cheap; we expand anyway). Skip the rollback
machinery.

**Done when.** A file whose last def throws at eval time reports the error and
the earlier defs are visibly applied (documented behaviour), while a
syntactically broken save changes nothing. Suite green. → **ADR-042 (shipped
2026-05-29).**

---

## Stage 3 — Editor-driven eval: the Emacs `C-M-x` path  ·  *makes it feel right*

**Problem.** Polling is laggy and indirect (gap #3). The instant path exists
(`nest mcp` `eval`/`load`, `mcp.rs`) but isn't the headline.

**Design.** Expose eval-at-point as custom LSP commands
(`workspace/executeCommand`, since "eval this form" isn't a standard LSP
request) in `crates/lsp`, reusing the same image the MCP talks to:
- `eval-defun` — eval the top-level form under the cursor.
- `eval-region` — eval the selection.
- `eval-buffer` — `reload-defs` the current buffer (def-only, so an entry call
  isn't re-run).

These are thin: locate the enclosing top-level form (the LSP already parses
CST), send its text to the image's `eval`, surface the result/diagnostic inline.
Polling (`std/reload.blsp`) stays as the fallback for edits made *outside* the
editor.

**Simplicity / perf.** Reuses existing eval/load + CST; near-zero latency by
construction. The main cost is LSP plumbing, not new runtime mechanism.

**Done when.** Editing a `defn` and hitting the eval-defun keybinding makes it
live with no file write and no poll wait. → ADR for the LSP command surface.

---

## Stage 4 — Watcher simplification (and an optional notify upgrade)

**Problem.** `reload-on-change` spawns *one green process per watched file* plus
a dir scanner; it never reaps deleted files and can double-watch a path covered
by both a dir watch and an explicit watch.

**Design (simple win).** One watcher process holding a `path → mtime` map,
stat-ing the set each tick, instead of a process per file. Reap vanished paths.
Dedup the watch set. This is *less* code than the current per-file fan-out and
removes the "hundreds of polling processes for a big project" footgun.

**Design (perf, optional, deferred).** Replace mtime polling with the `notify`
crate (inotify/fanotify) — O(1) instead of O(files), sub-poll latency. Cost: a
Rust-side watcher feeding events to Brood, i.e. more Rust/Brood coupling. Given
Stage 3 makes in-editor edits event-driven already, polling is only the
*external-edit* fallback, so `notify` is a nice-to-have, not load-bearing. Take
it only if the single-watcher-process version proves too laggy.

**Simplicity / perf.** The single-process version is both simpler and cheaper —
take it now. Defer `notify`.

**Done when.** Watching a directory with N files uses one watcher process;
deleting a file stops its reload attempts. Suite green.

---

## Stage 5 — Bounded RUNTIME memory  ·  *the explicit "to a degree" line*

**Problem.** Append-only RUNTIME never frees superseded code (gap #2). For an
all-day session this leaks every intermediate version of every redefined
function.

**The tension (state it honestly).** The append-only `boxcar::Vec`
(`heap.rs:311`) is *why* reads are lock-free and stable: existing elements never
move, so process threads dereference closure bodies without locking while a
`def` appends. Reclaiming individual slots fights that directly:
- A **free-list slab** could free slots but gives up the lock-free stable-ref
  property (the whole point of `boxcar`).
- A **compacting copy at a global safepoint** (move live RUNTIME code to a fresh
  region, rewrite handles) preserves lock-free reads *between* collections but
  is a moving GC for the shared region — it must pause every one of the
  runtime's processes at once (extending the existing per-process GC safepoint,
  ADR-035, to coordinate runtime-wide) and rewrite every reference.

**Decision — defer, with a quantified, documented leak.** A promoted function
body is small (hundreds of bytes to a couple of KB, depending on size and nested
closures); a few thousand redefinitions a day is single-digit to low-tens of MB.
For the common session this is a non-issue; for a multi-day session it's real but
not catastrophic. So:
- **Now:** document the leak (it's already implied by `heap.rs:18`); optionally
  **dedup** — skip the append if the promoted code is structurally identical to
  the current binding (helps save-without-change and formatter churn).
- **Later (its own stage + ADR):** the compacting collector above, reusing the
  GC safepoint machinery. Only build it when a real session actually hurts.

This is the principle in action: accept a *slow, bounded, non-corrupting* leak
to keep the hot read path simple; don't contort the design for memory we can
reclaim later.

**Simplicity / perf.** Maximally simple now (document + optional dedup). The
real collector is deferred precisely because it's the one place simplicity and
correctness don't conflict — the leak is slow enough to wait.

**Done when (this stage).** The leak is documented and dedup-on-identical lands.
The collector is a tracked follow-up, not part of this stage. → **dedup half
shipped 2026-05-29 under ADR-042; the collector remains deferred.**

---

## Stage 6 — Upgrade hook for long-lived processes

**Problem.** A running loop keeps old-shaped state across a code change (gap #4).

**Design — keep it in userland (matches the supervision call, ADR-039).** Don't
add a kernel `code_change`. Instead document the pattern and provide the one
small primitive that makes it ergonomic:
- A loop that wants upgrade-safety threads a **versioned** state (`{:v 1 …}`)
  and, in its `receive`, matches a `[:code-change]` message by migrating its
  state map before continuing.
- The reloader (Stage 3/4) optionally `send`s `[:code-change]` to processes that
  registered interest after a successful reload.
- Expose the current code generation as a plain global counter (e.g. a `def`'d
  `*code-version*`, bumped on each successful reload) so a loop can detect "I'm
  running under new code with old-shaped state" without a message. (A plain
  global, not a `defdyn` dynamic var — `defdyn` is a *per-process* binding stack,
  ADR-032, which is the wrong shape for a single runtime-wide generation count.)

**Simplicity / perf.** No kernel surface beyond a counter; the policy lives in
Brood where supervision already lives.

**Done when.** A documented example loop survives a state-shape change across
reload by migrating on `[:code-change]`. Suite green.

---

## Stage 7 — Macro-redefinition staleness warning

**Problem.** Redefining a macro silently leaves already-expanded callers on the
old expansion (gap #5).

**Design — warn, don't track (for now).** Mirror the existing arity-change
diagnostic (`eval/mod.rs:195`): when `defmacro` *rebinds* an existing macro,
print `[reload] macro X redefined; callers compiled before now keep the old
expansion — re-eval them`. A true reverse dependency index (who expanded X) is
deferred; the warning is 90% of the value at 5% of the cost.

**Simplicity / perf.** One `eprintln!` on the rare macro-rebind path.

**Done when.** Redefining a macro prints the staleness note. Suite green. →
**ADR-042 (shipped 2026-05-29).**

---

## Stage 8 — The native floor (mostly a non-issue — recorded for completeness)

The 2026-05-29 check showed **overriding the prelude already works**, internals
included. So "open up the prelude" — which I'd initially flagged as a major
gap — is largely solved by the existing late-binding-through-the-globals-table
design. What genuinely *can't* be hot-patched:

- **Rust builtins / kernel** (`+`, `fold`'s primitive, the reader, the
  scheduler). These are native; you can shadow the *symbol* with a Brood
  redefinition but not edit the native body.
- **Editing the prelude *source file*** and having new bodies go live — actually
  works via `reload-defs` on the prelude path (each `defn` re-defs into RUNTIME,
  shadowing the frozen original); the frozen PRELUDE copy just becomes dead
  weight.

**Action:** none required for the editor goal beyond a one-line note in
[`shared-code.md`](shared-code.md) that overrides reach prelude internals. Revisit
only if a concrete need to hot-patch a *native* builtin appears (it shouldn't —
the editor is Brood, not Rust).

---

## Non-goals / explicitly deferred

- **A true RUNTIME collector** (Stage 5 later half) — deferred until a real
  session hurts.
- **`notify`-based watching** (Stage 4) — deferred; polling + editor-driven eval
  covers it.
- **Macro dependency graph** (Stage 7) — warn instead.
- **Kernel-level upgrade/`code_change`** (Stage 6) — userland pattern instead,
  consistent with let-it-crash supervision (ADR-039).
- **Snapshot/rollback in `reload-defs`** (Stage 2) — read+expand-first covers the
  common breakage; full transactional rebind deferred.
- **Schema/record-redefinition migration** — *not applicable today and a
  data-model win worth naming.* The classic hard reload case (Erlang records,
  CL `update-instance-for-redefined-class`) doesn't exist here because data is
  structurally-typed immutable maps — there is no nominal type whose field set
  can drift out of sync with live instances. A map simply has the keys it has.
  Revisit only if nominal records/`deftype` are ever added (roadmap M-types
  step 5+); until then, the only shape-drift concern is a *process loop's* state,
  handled by Stage 6.

## Suggested order

Stage 1 (state survival) and Stage 3 (editor-driven eval) are the two that
*make or break* the Emacs feel — do them first. Stage 2 makes reload trustworthy.
Stage 4 is a cheap cleanup. Stages 5–7 are hardening you can schedule against
real usage. Stage 8 is a doc note.

## Open questions

1. Should `eval-buffer` (Stage 3) use `reload-defs` (skip the entry call) or full
   `load` (run everything)? Likely `reload-defs`, matching the watcher — confirm
   against how the editor's entry point is structured.
2. `*code-version*` (Stage 6): a single per-runtime counter, or per-file
   generations? Single counter is simpler and probably enough.
3. Dedup-on-identical (Stage 5): compare promoted structure, or hash the source
   form? Source-hash is simpler and catches the common save-without-change case.
4. `defonce` for singletons (Stage 1) interacts with Stage 6: if a `defonce`'d
   *process* holds state and you reload its loop's code, the process survives
   and late-binds the new code — good. But if you change the *shape* of that
   state, you still need the Stage 6 migration hook. Worth a worked example
   showing `defonce` + `[:code-change]` together.
