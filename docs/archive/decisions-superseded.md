# Superseded / reverted ADRs (archive)

Full text of the design decisions that are **no longer in force**, moved out of
[../decisions.md](../decisions.md) to keep the active log focused on current
design. They stay here verbatim because each carries a useful retrospective —
what was tried, and why it was dropped. **Do not cite these as current design;**
the active index in [../decisions.md](../decisions.md) records what replaced them.

| ADR | Title | Outcome |
|----:|-------|---------|
| 002 | `Rc`/`RefCell` now, tracing GC later | fulfilled, then superseded (→ ADR-035/054/055/058/061) |
| 035 | Tracing GC: per-process mark-sweep at the outermost-eval safepoint | superseded / disabled |
| 039 | Supervised processes with mode-gated resume checkpoints | reverted (→ ADR-044) |
| 057 | Lexical addressing: O(1) variable lookup | rejected as scoped |

---

## ADR-002 — `Rc`/`RefCell` now, tracing GC later

**Status:** ✅ **fulfilled, then superseded** — by the hand-rolled per-process
copying collector (ADR-035 → ADR-054/055/058/061), *not* by the `gc-arena`
migration this ADR originally planned. Closed: nothing left to do.

**What actually happened.** The `Rc`/`RefCell` substrate was replaced wholesale,
not migrated to `gc-arena`. Heap values are now **`u64` handles into per-process
slabs** (`core/heap.rs`), environments are immutable frames (ADR-026 — no
`RefCell` mutation), and reclamation is a **moving semi-space copying collector**
that fires automatically at the eval safepoint, at any depth. The cycle-leak cost
this ADR accepted ("a closure capturing an environment that reaches it") is also
gone — the collector traces, and `promote` grew a forwarding table so even
*promoting* such a cycle into the shared region terminates (ADR-061 follow-up,
2026-05-30 devlog). `gc-arena`'s `'gc` lifetime brand was evaluated and rejected
for our native recursive evaluator (see ADR-035 "Why not … gc-arena").

**Why the containment still paid off.** All heap construction goes through
`value.rs` helpers, which is exactly what made replacing the substrate localised —
the migration target changed, but the discipline that made it cheap held.

**Original decision (historical).** Use `Rc<…>` for heap values and `RefCell` for
environment mutation in v0.1; plan a migration to `gc-arena` before sessions
become long-lived. Rationale: simplest correct thing to get moving, accepting that
reference cycles leak (irrelevant for a REPL / early milestones).

---

## ADR-035 — Tracing GC: per-process mark-sweep, fired only at the outermost-eval safepoint

**Status:** ⚠️ **superseded / disabled** (2026-05-29). The mark-sweep described
below was implemented, then **switched off** in favour of a **bump-only allocator +
arena flip on `(hibernate)`** (commits `f90f0de` Phase 1, `dee0814` Phase 2; see
ADR-041 and the status banner in [`docs/memory-model.md`](../memory-model.md)).
`Heap::collect` is now a **no-op**; the mark-sweep survives as `collect_old`
(`#[allow(dead_code)]`) for reference only. *Why it was disabled:* mark-sweep
reclaims by **reusing freed slots**, and slot reuse reintroduced the stale-handle
multi-threaded scheduler race that the never-reuse bump allocator eliminates. The
`GC_BLOCK == 1` safepoint and the poison tripwire are still wired but inert (and
`BROOD_GC_STRESS=1` therefore exercises nothing today). The one remaining niche the
mark-sweep was meant to fill — a *long-lived* process (e.g. the in-language test
runner) that never hibernates accumulating unreachable garbage — is to be solved by
**hibernating that process between batches**, not by re-enabling slot reuse. The
original rationale is kept below for context.

**Original status (historical):** accepted. Fulfils ADR-002's "tracing GC later"
and the deferred step in ADR-016 (arena-reset doesn't help a never-returning loop).
Implementation in `crates/lisp/src/core/heap.rs`; design walkthrough in
[`docs/memory-model.md`](../memory-model.md).

**Context.** Arena-reset (ADR-016) bounded long REPL/file sessions by truncating
the LOCAL heap at top-level boundaries, but a single never-returning loop (a
spawned server, a `(spin)` benchmark) has no such boundary and accumulates
linearly with iteration count. A general tracing GC was deferred because our
recursive tree-walker holds live `Value`s on the **Rust** call stack where a
collector can't find them as roots — and the docs anticipated the fix would
require an explicit-operand-stack VM rewrite (coupled with step 4b). Step 4b
shipped instead via stackful coroutines (ADR-018), so the VM-rewrite rationale
no longer applies; we need a GC that works with the recursive evaluator we
have.

**Decision.** A **precise, non-moving, per-process mark-sweep** that fires
**only at the outermost-`eval` `'tail:` safepoint**. The completeness argument
relies on one invariant — `GC_BLOCK == 1` — and on the trampoline structure of
the evaluator.

- **Roots at the safepoint** are: the eval's `expr`/`env` (passed in by the
  call), the heap's dynamic-binding stack, and an explicit `Heap::roots` stack
  (used only by `eval_str`/`eval_source`, the sole depth-0 callers that hold a
  `Vec<Value>` of unevaluated forms across an outermost eval).
- **`GC_BLOCK` is a thread-local depth counter** incremented by RAII guards at
  every `eval` and `macroexpand_all` entry. GC runs only when this is `1` ("we
  are the outermost contributor — no other eval/macroexpand frame holds an
  unrooted LOCAL transient"). Saved/restored around coroutine suspend, reset to
  0 at coroutine entry, so workers multiplexing processes don't leak depths.
- **Per-slab free lists** (`pairs`/`vectors`/`maps`/`strings`/`closures`/`envs`);
  `alloc_*` pop the free list before extending the slab. Handles stay stable
  across collection (non-moving), so a Rust local holding a rooted handle stays
  valid even though the slab around it was swept.
- **PRELUDE and RUNTIME are not swept.** The promotion invariant (every LOCAL→
  RUNTIME write deep-copies) guarantees those regions hold no LOCAL refs, so
  the trace never leaves the local heap.
- **Adaptive threshold:** after each collect, `gc_threshold = max(GC_FLOOR, 2 *
  live)`. Set `BROOD_GC_STRESS=1` to force GC at every safepoint (debugging /
  test stress — the suite is green under it).
- **Disabled during prelude build** (`Heap::new` sets `gc_enabled = false`),
  so freeze/re-tag sees a hole-free slab.

**Why this works (correctness sketch).** At `GC_BLOCK == 1` at the eval loop
top:

1. The current eval's loop-body locals (`head`, `rest`, `callee`, `argv`,
   `scope`) are declared inside the loop body and dead at `continue 'tail`;
   only `expr`/`env` persist — and both are passed to `collect` as roots.
2. No other eval or `macroexpand_all` frame is active (`GC_BLOCK == 1` means
   *this* is the only contributor), so no nested-eval transient is live.
3. A builtin mid-execution implies the eval that called it is blocked in
   `call_native`, not at its safepoint — GC and builtin transients are
   mutually exclusive on the stack.
4. The caller of the outermost eval is either `eval_str`/`eval_source`
   (forms vec rooted via `Heap::push_root`) or a coroutine body (holds `f` —
   already RUNTIME by `promote` — and a `scope` that *is* the current `env`).

Therefore every live LOCAL handle is reachable from the union {`expr`, `env`,
`heap.roots`, `heap.dynamics`}. ∎

**Why not stepping VM / handle scopes / gc-arena.**

- A stepping-VM rewrite would touch ~all of `eval` and re-shape every builtin's
  calling convention — the doc-anticipated cost. It's unnecessary here: the
  trampoline structure already lets us pick a safepoint where the operand
  stack *is* tiny and statically known.
- Handle-scope rooting (V8-style) across all of `eval` and every Rust-side
  builtin is ergonomically invasive and easy to get subtly wrong. The
  `GC_BLOCK==1` invariant collapses the rooting surface to two sites
  (`eval_str`, `eval_source`).
- `gc-arena` was the original ADR-002 path; the `'gc` lifetime brand reshapes
  every value-touching function and assumes a stepping evaluator. Both bad
  fits for our recursive eval + shared multi-thread RUNTIME region.

**Limits / what's deferred.**

- A computation that perpetually stays at `GC_BLOCK > 1` (e.g. a non-tail
  deeply-recursive function, or a server loop wrapped in `(try (loop) …)` where
  `%try` keeps the outer eval blocked) doesn't reach a safepoint and won't GC
  until it unwinds. Idiomatic Erlang-style loops return to the outermost
  between iterations, so this is rare in practice — and the fix is incremental
  (add explicit rooting for the few builtins that hold transients across eval,
  letting GC fire at deeper safepoints).
- Slabs don't shrink trailing dead runs — the free list reuses indices instead.
  Memory peaks at the high-water live count plus retained `Vec` capacity, then
  stays flat. (Trailing-truncate is a future optimization.)
- The interner and the RUNTIME code slabs are still append-only and grow with
  hot-reload (ADR-013) — orthogonal to per-process data GC.

**Verified.** The full suite passes under `BROOD_GC_STRESS=1` (GC at every
safepoint). Dedicated regression tests in `crates/lisp/tests/gc.rs` assert that
a 200k-iteration tail loop and a 20k-message server loop both stay bounded.

---

## Deferred / open questions

- **Macro hygiene:** currently unhygienic `defmacro` + `gensym`; hygienic macros
  (e.g. `syntax-rules`) are possible future work.
- **Nested quasiquote:** not level-tracked in v0.1 (see spec §spec note); fine
  for ordinary macros, revisit if needed.
- **`car`/`cdr` vs `first`/`rest`:** both provided; `first`/`rest` are the
  documented default.

## ADR-039 — Supervised processes with mode-gated resume checkpoints

**Status:** **reverted** (2026-05-29, commit `e3d3a0d`). Proposed 2026-05-28;
shipped as opt-in 2026-05-28 (`a4948cd` / mid-day, then `9907401` follow-on);
stripped 2026-05-29 because the kernel-side supervisor (RESUME_SLOT + safepoint
rooting + the supervise() retry loop) was contributing the bulk of the
multi-thread scheduler race surface (KI-1). The fan-out blocker outranked the
elegance gain. The userland substrate (`spawn` + `monitor`) remains and is
sufficient to write Erlang-style supervisors by hand. See
[`supervision.md`](../supervision.md) (now a short revert note + userland pattern)
and [`docs/devlog.md`](../devlog.md) for the strip rationale and metrics
(recurse.blsp failure rate ~95% → 0% across the strip and the Phase-1 bump
allocator follow-on). The design below is preserved as the **considered**
shape so a future revisit can pick up the trade-off honestly.

**Context.** Brood is the language a self-editing editor will be written in.
The editor is one long-running stateful process whose `(receive)` loop *must
not die* when a freshly-saved redefinition contains a bug. The current
process model is **Erlang let-it-crash**: an uncaught error inside a process
unwinds the coroutine and the process is gone. Erlang reaches that
elegantly through *gen_server + supervisor* — split the state-holder from
the worker so workers can be restarted with no state to lose. That
separation exists because mutable state is hard to roll back cleanly.

Brood is immutable. There is no mid-iteration partial mutation to undo;
every value the eval loop holds at a safepoint is byte-for-byte equivalent
to that same value before any iteration started. That property makes a
fundamentally different process model possible — one where **the runtime
itself is the supervisor**, every process is recovered automatically, and
the worker/state-holder split that defines Erlang/OTP can collapse.

The shape Brood's process model can take, that no mutable language can:

> **A process is its current call.** At every function call, the runtime
> updates a per-process `(callee, argv)` *resume slot*. On an uncaught
> error, the supervisor catches, logs, applies a small backoff, and
> re-invokes from the resume slot — **same function, same arguments**.
> Immutability means no partial state survived the throw; the resume is
> transactional. Late binding means a fresh redefinition (after the user
> fixes the bug and saves) is picked up on the next invocation.

This is the architectural decision. The trade-offs — performance, side
effects, mode-gating — are below.

**Decision.** Three coupled changes, all gated by a single runtime mode
flag, with sensible defaults per command:

1. **`spawn` is supervised, always.** `(spawn expr)` creates a process
   whose outermost eval frame is wrapped in the runtime's supervisor.
   Uncaught errors are caught at the process boundary, not propagated to
   the OS thread. The main process running a script is supervised the
   same way.
2. **Resume checkpointing.** While `dev-mode` is on, the eval loop updates
   a `Process::resume_slot: Option<(Value, SmallVec<[Value; 8]>)>` at
   every function call. On caught error, the supervisor re-invokes
   `apply(callee, argv)` from the slot. **State is preserved** — `argv`
   *is* the current iteration's accumulator. With dev-mode off, the slot
   isn't updated; an error restarts from the *spawn* entry expression (or
   exits, for one-shot processes). Recovery still works; state doesn't
   carry through.
3. **`spawn` accepts an optional name** — `(spawn :editor expr)` makes
   the spawn *idempotent on the name*. A live process registered under
   `:editor` makes the spawn a no-op. This is what makes hot-reload of a
   file containing `(spawn :editor (editor-loop init))` not spin up a
   second editor: the second load sees the name is alive, skips. The
   name table is the existing `NAMES` table that `register`/`whereis`
   already use — no new mechanism.

The mode gate (`BROOD_MODE=dev` / `BROOD_MODE=release`, with per-CLI
defaults):

| Command                       | Default mode | Why                                                                          |
|-------------------------------|--------------|------------------------------------------------------------------------------|
| `brood file.blsp` / REPL      | dev          | Interactive use is hot-reload-style; the user is editing while it runs.      |
| `brood --test`                | dev          | Tests catch transient errors; supervision keeps the suite running.           |
| `nest run`                    | dev          | Same as `brood`. Hot-reload is core to the workflow.                         |
| `nest test`                   | dev          | Same.                                                                        |
| `nest bundle` (when it lands) | release      | Bundled binaries ship to end users; no editing at runtime; pay no overhead.  |
| `nest run --release`          | release      | Opt-in for "I want production semantics on dev machine".                     |

Release mode means: no checkpoint slot updates, no resume — uncaught error
exits the process the Erlang way. Same eval loop, just a no-op for the
checkpoint branch. **The cost of hot-reload is paid only when the user is
hot-reloading.**

**Why.** Three forces, all pointing the same way:

1. **The editor is the destination, and the editor *must not die*.** Every
   keystroke handler is, effectively, an iteration of the editor's main
   loop. A bug in newly-saved code can't be allowed to terminate the
   editor. The supervised-resume model gives this for free, with no user
   ceremony.
2. **Immutability collapses the gen_server split.** Erlang separates
   state-holder from worker because the worker has to be safely restartable
   *despite* mutable state. Brood doesn't need that. State lives in the
   loop's call frame, and the resume slot puts it back exactly where it was.
   The whole supervisor-tree+gen_server pattern that occupies a chapter of
   every Erlang book becomes "spawn it".
3. **Hot-reload demands it.** The whole point of late binding + redefinable
   globals (ADR-013) is that *running code picks up new definitions*. If
   the running code dies the moment a newly-loaded redefinition throws,
   late binding is half a feature. Supervised resume completes it.

**Mode-gating is the price-vs-feature lever.** Resume checkpointing is two
writes per function call (a `Value` and an `SmallVec` of args). On the
hottest path that's a few ns; on a tight recursive numeric loop it might
be a measurable few percent. Hot-reload survivability isn't free, but it's
also not needed at runtime for a shipped editor binary. Dev mode pays;
release mode doesn't. Default chosen by command surface, overridable
explicitly.

**What this removes.**

- **`defonce`** (transitional shim in `std/prelude.blsp` today): subsumed
  by named-spawn for the process case (the dominant use) and by "state
  lives in a process" for the state case. **Kept in the prelude until
  ADR-039 lands** — removing it before named-spawn exists would leave
  users without a working "spawn-once on reload" pattern. The
  implementation commit removes `defonce` in the same change that adds
  named-spawn.
- **Hand-written supervisors.** No user code calls `monitor`-and-respawn
  loops. `monitor` remains for genuine "I want to know when this dies"
  patterns; it doesn't have to also be the restart mechanism.
- **The `live-loop` macro** I was about to propose: vanishes. Plain
  `(defn worker (state) … (worker new-state))` *is* a supervised loop.
- **Most `try`/`catch` at the top of a process.** Errors are caught by the
  runtime; user code only catches when it wants to *recover with context*
  (e.g., an HTTP server logging which request failed), not just "don't die".

**What this enables.** Some downstream simplifications worth flagging:

- **`nest test` doesn't need `:isolated` for crash-isolation** — a test
  that throws no longer dies its worker; it logs and continues. (`:isolated`
  still useful for the global-table sandbox use case.)
- **`std/reload.blsp`'s explicit `(try (load p) (catch e …))` becomes
  optional** — the watcher process is itself supervised. Keep the explicit
  catch for the *diagnostic context* (which file failed, which error), drop
  it as a *survival mechanism*.
- **The hot-reload demo simplifies.** No `defonce`, no manual park, no
  named pid: `(spawn :ticker (ticker 0))` at the top of the file. Reloading
  the file rebinds `ticker`, the spawn is a no-op, the existing process
  picks up the new code.

**Scope / deferred.**

- **The mode gate's wire** — exact env-var / CLI-flag spelling — is a
  small implementation detail recorded with the implementation. Likely
  `BROOD_MODE=release` + `--release` flag for both `brood` and `nest`.
- **Per-process supervision policy** (max restarts, backoff curve) lives
  on the spawn site: `(spawn :worker expr :max-restarts 10 :backoff
  :exponential)`. Default: 10 restarts over 5s, then give up; exponential
  backoff from 1 ms. Tuneable when real workloads ask for it.
- **The script case.** A top-level `.blsp` file that's a sequence of
  side-effecting forms (not a loop) gets supervised the same way, but the
  resume slot is empty after the last form; an error during step N
  re-invokes step N (only). For idempotent scripts (most are), retry is
  fine. For non-idempotent, the script can opt out with `--release` or by
  bare-spawning. Documented behaviour.
- **Side-effect duplication.** A `(println …)` followed by a crash means
  the line printed; resume re-prints. Same as a retried database
  transaction at the SQL layer — at-least-once. The mode gate lets users
  opt out when they need exactly-once.
- **`bound?`** — still useful for genuine "is this name in the global
  table" introspection; the defonce use case goes away, but it stays
  as a primitive.

**Open questions / answer-on-implementation.**

- Does the resume slot need to be GC-rooted? Yes — `argv` holds
  potentially-LOCAL values; the slot is a per-process root the GC must
  scan. Two extra roots per process. Negligible.
- Should the supervisor's *log channel* be process-local or runtime-global?
  Process-local seems right (each process gets its own diagnostic stream);
  the runtime aggregates into one stream by default. `nest test`'s
  per-test output already uses a similar pattern.
- Restart storm prevention. Document the algorithm:
  `backoff_ms = min(max_backoff, base * 2^restart_count)`; `restart_count`
  resets after `quiet_seconds` of no crashes. Tune base/max via spawn
  opts.

**Consequences.** This is the deepest behavioural change since ADR-018
(green processes); landing it touches `process.rs` (the worker's
coroutine entry — wrap the eval call in a catch + retry loop), `eval/mod.rs`
(update `Process::resume_slot` at every `Value::Fn(id)` / `Value::Native(id)`
dispatch when in dev mode), `value.rs` (the slot needs `Send` storage; a
SmallVec of Values does), and `Cargo.toml` / CLI flags for mode selection.
`std/prelude.blsp` loses `defonce`; `std/reload.blsp`'s `try`/`catch` gets
simpler. The proposed M2 editor work (`docs/roadmap.md`) is designed
against this model, not retrofit. ADR-038 (the bundler) gains a definite
release-mode story.


## ADR-057 — Lexical addressing: O(1) variable lookup (eval-dispatch Step 2)

**Status:** **rejected as scoped** (2026-05-29). Designed, then *not* implemented
— direct measurement showed the premise this rests on is false: variable lookup is
**~6%** of the eval loop, not the bottleneck. The design is kept on record (it's
correct, and lexical addressing may return as a *by-product* of a future
precompiled-body step), but on its own it's a poor trade: ~1–1.5 weeks of
high-churn work — including the campaign's only real data-race surface (the
global-cell seqlock) — for an under-10% gain. The evaluator-dispatch campaign's
Step 2 is **re-pointed at the call path + per-combination overhead** instead; see
[`handoff-eval-dispatch.md`](../handoff-eval-dispatch.md).

**Why rejected — the measurements that killed it.** Same machine, current build,
2 M-iter loops, isolating one cost at a time (`/tmp/{lookup,read,call}_cost.blsp`):
- A **local** variable read costs **~0 ns** over binding a constant — the env chain
  is shallow (1–3 frames of a few bindings each), so the "walk + scan" is free.
- A **global** read costs **~9 ns** over a constant (the `RwLock` read-acquire +
  `FxHash` probe — cheap and uncontended).
- One **closure call** costs **~52 ns** (`new_env` alloc + `bind_params` + body).
- So the bare ~400–480 ns/iter loop splits roughly: **lookup ~6%**, function calls
  ~a third, and the **majority is per-combination fixed overhead** (the
  `tick`/`gc_due`/`soft_limit` TLS guards run on *every* combination, spine
  `uncons`, argv `SmallVec`, native dispatch). Lexical addressing targets the
  smallest slice.

The original premise — that the ~400 ns is dominated by `env_get` chain-walking —
was an inference, not a measurement; a "what's the benefit?" review caught it
before the 1.5 weeks were spent (a vindication of the Step-0 baseline + the
"measure every step" guardrail). The high-leverage levers are the call machinery
(`new_env` per call) and folding the per-combination guards into one check —
recorded in the handoff doc as the re-scoped Step 2/3.

**Does code-safety rescue it? No.** A later review asked whether lexical
addressing, beyond speed, buys *user code safety* (catching unbound vars / typos /
shadowing before runtime). The general principle holds — a resolution pass that
classifies every reference against a compile-time scope *is* a safety tool — but in
Brood that value is **already delivered, at the right layer, decoupled from the
runtime**: `syntax/scope.rs::analyze` (the CST scope tree behind go-to-def /
find-refs / rename / shadowing) and the advisory **type checker** Step 4 (arity +
**unbound-symbol** diagnostics, scope-aware, surfaced by the LSP as warnings). The
runtime `LocalRef`/`GlobalRef` rewrite changes *how a lookup executes*, not *what is
checked* — it adds **no new diagnostic**, and its resolution pass would *duplicate*
analysis that already exists. And Brood's checking is **advisory, never rejecting**
(ADR-023/024) — because late binding + hot reload (ADR-013) make a currently-unbound
global legal — so even maximal static checking here is a warning, already emitted.
*If* more safety is wanted, the lever is the checker/LSP, not the evaluator: an
audit (devlog 2026-05-29) found one real gap — unbound symbols in **operand/value
position** aren't flagged (only call heads are; the checker is conservative there to
honour its no-false-positives rule around unexpanded macro args). Closing that is a
small, low-risk change in `types/check` — unrelated to, and not a justification for,
this ADR.

**Original context (retained for the design record).** Measuring `(sort < …)`
overturned its own premise (devlog
2026-05-29): the ~700× gap vs Rust is neither comparisons (~9%) nor allocation
(~140 ns/cons) nor GC (never fires below the 64K floor). The floor is **variable
lookup**. `env_get` (`core/heap.rs`) walks the lexical parent chain and scans each
`EnvFrame`'s assoc-list; *every reference to a global* — `cons`, `<`, `-`, `take`,
… (most refs in a hot loop) — walks the **entire** chain to `EnvId::GLOBAL`, then
probes a `Symbol→Value` HashMap. A bare tail loop costs ~400 ns/iter, dominated by
these walks. Step 1 confirmed special-form *classification* is not the cost
(enum dispatch moved the if-loop 404→406 ns, within noise). This ADR is the
structural fix: resolve each reference once, at compile time, to a direct address.

**Decision (three parts).**

**(1) Representation — internal resolved-reference `Value`s, carved out of the
public type universe.** Add two variants produced *only* by the resolver and
consumed *only* by `eval`'s `match expr`:
- `Value::LocalRef { up: u16, idx: u16 }` — bound `up` lexical frames out, slot
  `idx` within that frame.
- `Value::GlobalRef { slot: u32, sym: Symbol }` — global cell `slot`; `sym`
  retained for diagnostics, dynamic-var fallback, and cross-runtime re-resolution
  (see §messages).

Options weighed:
- *(A) full `Value` variants in the public lattice* — fast, but pollutes
  `type-of`/predicates/printer/equality/messages for things that are **code,
  never data**. A category error.
- *(B) internal special-form lists* `(%local up idx)` / `(%global slot)` —
  no new `Tag`, dispatched via the Step-1 enum, but allocates + re-walks a list
  per reference; likely no faster than the symbol lookup it replaces.
- **(C, chosen) dedicated `Value` variants excluded from the user type
  universe.** They get `Tag::LocalRef`/`Tag::GlobalRef` appended *after* `Rope`
  (existing bit order preserved), but are **omitted from `types::ALL_TAGS`** and
  the `type-of` surface, and the reader never produces them. The compatibility
  contract (`docs/types.md`) is met by a documented carve-out: the checker treats
  them as `dynamic()`; printer / structural-equality / `to_message` hit a
  `debug_assert!(unreachable)` (a resolved ref must never reach userland — quote
  and quasiquote keep raw symbols, so data is never resolved). `Copy` scalars, so
  the tracing GC ignores them (no handle to relocate).

`eval` gains two arms: `LocalRef{up,idx}` → climb `up` parents, index `vars[idx]`
directly (no symbol compare); `GlobalRef{slot,..}` → one indexed cell load.

**(2) Global cells.** Back the globals table with an append-only slot vector
(`boxcar::Vec<GlobalCell>` — stable refs, lock-free, already used for the shared
code region) alongside the existing `Symbol→slot` index map. `def` resolves a
symbol to its slot (reserving one if absent) and writes the cell; a resolved
`GlobalRef{slot}` read skips the *map* (no hash, no map lock). **Late binding /
hot reload preserved** (CLAUDE.md "shared code", ADR-013): the slot is stable, a
re-`def` updates the cell in place, so a *running* process holding
`GlobalRef{slot}` sees the new value on its next read — no inlined value, no
recompile. Forward refs reserve an empty (unbound) cell that the later `def`
fills. The slot vector lives in the shared `RuntimeCode` (per-runtime), so
separate runtimes stay independent.

*Cell synchronization (memory safety — load-bearing).* The cell is **not** a
bare `Value`. Globals are shared mutable state across a runtime's processes
(today: one `RwLock<SymbolMap<Value>>`, so every read is a rwlock read-acquire);
a `Value` is a **multi-word `Copy` enum** (discriminant + ≤64-bit payload), so an
unsynchronized read of a plain `cell: Value` concurrent with a `def` write is a
**torn read / data race — UB.** "Skip the map" must therefore mean *skip the hash
and the coarse map lock*, **not** *skip synchronization*. Each `GlobalCell` is a
**seqlock** (a generation counter: the reader loads `seq → value → seq` and
retries on mismatch) — the right shape for *frequent reads, rare `def` writes, a
small `Copy` payload*. A seqlock read is **two acquire loads + a compare —
strictly cheaper than today's rwlock read-acquire (an atomic RMW)** *and* it
drops the hash, so the perf win survives while the access stays sound.
(`arc_swap::ArcSwap<Value>` is the wait-free alternative if seqlock reader-retry
under a write storm ever bites; it costs an indirection + an `Arc` refcount bump
per read and an `Arc` alloc per `def`. Seqlock is preferred — `def` is rare.)
Only the `Symbol→slot` index map still needs a lock, and only on the rare
`def`/reserve path; resolved-slot reads never touch it.

*Publish ordering (why hot reload is visible and safe).* `def` already
`promote`s the value into the **append-only, immutable** shared RUNTIME region
*before* binding. The seqlock write is a release publish of the (promoted,
already-shared) handle; the reader's acquire load observes both the new handle
and the immutable data it points to. A process that reads the global mid-reload
sees either the old or the new binding — never a torn half — and whichever it
sees points at valid, immutable, shared data.

**(3) Resolution pass.** Thread a compile-time **lexical scope** (a stack of
frames, each the ordered names a `fn`/`let`/`letrec` binds) through the existing
`macroexpand_all` walk — which already (a) runs once per top-level/definition
boundary, (b) distinguishes *binders* (let targets, fn param lists) from
*references*, and (c) leaves quote/quasiquote opaque. **Resolution must run after
full macroexpansion** (names a macro introduces are resolved in the expanded
tree), so it hooks at the *tail* of `macroexpand_all` (or a sibling pass invoked
immediately after). For each `Value::Sym(s)` in operator/operand position:
bound at depth `d` slot `i` → `LocalRef{d,i}`; else → `GlobalRef{slot_for(s),s}`.
Idempotent (re-resolving a resolved node is a no-op) and applied wherever
macroexpand runs — including the `eval`/`load` builtins on dynamically-built code.

*Dynamic vars* (`defdyn`/`binding`) are never lexically bound; a `GlobalRef` read
consults the dynamic stack first (as `env_get` does at `GLOBAL` today), preserving
current semantics.

*`letrec` wrinkle.* Today its bind phase double-pushes (pre-define all names to
nil, then re-`env_define` the real values), so a name has two frame slots —
unindexable. Fix: pre-define N nil slots, then *update in place* during the value
phase (still within the bind phase, before the body runs — no observable
mutation), giving exactly N stable slots.

**Messages / closure-shipping.** A closure in a message carries resolved bodies.
`LocalRef` is self-contained (a depth/index, interpreted against the *receiving*
process's own runtime env — process-local, so it travels fine). `GlobalRef`
depends on *where the message lands*, and the two paths differ:
- **Same-node `send`** deep-copies the message across per-process heaps
  (`to_message`/`from_message`) but stays **inside the same runtime** — same
  shared `RuntimeCode`, same global table — so a `GlobalRef{slot}` is **still
  valid; no downgrade.** The same-node copy keeps the slot intact.
- **Cross-*node* (the `dist` wire)** lands in an **independent `RuntimeCode`**
  where a slot index is meaningless. So the **dist serialization** (not the
  same-node copy) **downgrades `GlobalRef → sym`**, and the receiver re-resolves
  against its own table on load (or lazily on first eval).

This is why `GlobalRef` retains `sym`: the dist path needs it, and it also serves
diagnostics + the dynamic-var fallback. (A `def`'d-but-unbound forward slot that
crosses the wire downgrades to its `sym` like any other.)

**Consequences / invariants.**
- **Tail calls** unaffected (resolution rewrites references, not control flow);
  `tail_calls_do_not_overflow` stays the gate.
- **GC** unaffected — resolved refs are `Copy` scalars; closure bodies are still
  `Value` trees traced as before.
- **Type checker** never rejects (refs are `dynamic()`); advisory contract intact.
- **Immutability** intact — global *cells* are binding mutation (already the only
  mutation Brood has, ADR-026/013), not data mutation.
- **Concurrency / memory safety.** 2a (`LocalRef`) adds no shared-mutable surface
  — env frames are process-local. 2b's global cells are the only new shared
  mutable state, and they are **seqlock-synchronized** (see §2): reads are
  wait-free-ish (retry only against a concurrent `def`), writes release-publish an
  already-promoted immutable handle. No torn reads, no data race; soundness does
  not rest on the coarse map lock it replaces.

**Risks.** Largest churn of the campaign — `eval/mod.rs`, `eval/macros.rs`,
`core/value.rs` + `types.rs` (the carve-out), `core/heap.rs` (global cells +
seqlock), `dist` (the cross-node `GlobalRef → sym` downgrade). Overlaps in-flight
GC-stats work in `heap.rs`. The global-cell change touches the hot-reload path →
the cross-process and hot-reload suites are the correctness gate, not just the
unit benches. **Get the seqlock right**: a `Value` is multi-word, so a bare slot
read is UB — this is the one place a subtle bug would be a data race rather than a
clean error.

**Rollout (each stage measured against the Step-0 baseline).**
- **2a — locals only.** Resolve `LocalRef`; leave globals as symbols (still map
  lookup). No `RuntimeCode` change, no message/hot-reload risk. Validates the
  resolver pass, scope threading, the `Value` carve-out, and idempotency on the
  *low-risk* path. (Moves deep-local programs; the global-heavy `sort`/`cons_build`
  benches move little here — that's expected.)
- **2b — global cells (seqlock) + `GlobalRef`.** The high-impact stage (this is
  what the `sort`/`cons_build` benches actually wait on). Gated on the hot-reload
  suite **plus a new concurrent-globals race test** — many spawned processes
  reading a global while another redefines it, under `BROOD_GC_STRESS` — which is
  what would catch a botched seqlock.
- **2c — dynamic-var fallback + the cross-node `GlobalRef → sym` dist-wire
  downgrade** (same-node `send` keeps the slot) and their explicit multi-node
  tests.

Locals-first deliberately front-loads the *shared machinery* (representation,
pass, idempotency) on the safe path before the high-impact, higher-risk global
change. An ADR number is reserved; promote **Status → accepted** once 2a lands
green with a recorded benchmark delta.

**References.** [`handoff-eval-dispatch.md`](../handoff-eval-dispatch.md) (the
campaign + Step 0/1 results), ADR-013 (hot reload / late binding — the constraint
that forces global *cells* not inlined values), ADR-026 (immutability — why this
is binding-, not data-mutation), ADR-023/024 + [`types.md`](../types.md) (the
compatibility contract the `Value` carve-out must satisfy), ADR-002 (the
`Rc`→`gc-arena` migration that `value.rs` helpers keep contained), the shared-code
model in [`shared-code.md`](../shared-code.md) (why `GlobalRef` can't cross runtimes).

