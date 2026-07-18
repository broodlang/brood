# Known issues

All historical interpreter defects are **resolved** (KI-9 below is a one-off arity
sighting judged a transient inconsistent-build artifact, not present in committed
code — kept as a record, not an open bug). This file is the condensed record — what
each was, how it was fixed, and the regression test that guards it — so a recurrence
is recognizable. For the narrative discovery writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md); deeper rationale is in the cited
ADRs / topic docs.

---

## KI-10 — `receive` compile cliff at the 13th arm · **open (worked around)**

Adding a 13th arm to a hot `receive` loop degrades it badly: on `buffer--serve`
(std/editor/buffer.blsp), a single TRIVIAL extra arm (`([:sync-probe] (recur …))`)
took the buffer suite from 4.9 s / 139 MB peak to 8.0 s / 248 MB — +65% wall,
+80% peak — and pushed the full parallel suite over the 1 GB test soft limit.
Bisected 2026-07-11 while adding a `[:sync …]` arm for the resync primitive;
12 arms are fine, 13 fall off the cliff, so the dispatch likely drops from an
indexed strategy to an allocating linear one at that width. Worked around by
merging `buffer--serve`'s two `[:edit …]` arms (buffer-edit always sends the
3-element form now) to stay at 12; the arm-budget constraint is noted in the
serve loop. Real fix: find the clause-count threshold in the receive/pattern
compiler and either raise it or make the wide path allocation-free.

## KI-9 — arity error from a closure shipped in a `spawn` body · **likely transient; not present in committed code**

Surfaced once while parallelising `nest check`: a driver spawned
`(spawn (send me (list :chunk (self) (chunk-fn chunk))))` where `chunk-fn` was a
*closure passed in as an argument* (captured in the spawned body's env); ~1 worker in
3 died with a bogus `arity error: fn: expected 0 arguments, got 1` at the
`(chunk-fn chunk)` call, silently skipping its files.

**Most likely a transient inconsistent-build artifact, NOT a standing kernel bug.**
The sighting happened while a *concurrent session* was mid-edit on the Rust tree
(uncommitted type-checker changes to `check.rs` et al., committed later as
`be0f8cc`); building from a half-applied edit can yield a binary with a real-but-
transient fault. The decisive evidence is the **frequency collapse**: it died ~66 %
of runs then, and **0 of 50+** now under identical repro conditions (a minimal
captured-closure spawn; the *exact* old shipped-closure `pfold` + `check-file`
chunk-fn at 24 and ~470 workers; the full multi-level `pfold-files→groups→spawn`
capture; all under `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` on a debug-assertions
build — all clean, no tripwire/verifier hit; deterministic arity-through-`spawn`+
`promote` passes). A stable race doesn't go 66 %→0 %; a changed binary does. A code
audit of the promote path (`heap.rs::promote_closure`/`promote_env`) found no defect
— arms/arity copied faithfully, reserve-then-fill with cycle-breaking, lock-free
concurrent append, and RUNTIME compaction can't fire mid-fan-out (uniquely-owned-Arc
gate). **No fix applied** — deliberately not blind-patching the moving-GC/shared-
RUNTIME path to chase a fault not present in the committed code. If it ever recurs on
a *clean committed build*, reopen with that repro.

**Best-practice (independent of this):** prefer shipping **data** to a spawned worker
and resolving the operation through a **global** (as the test runner and
`std/tool/project.blsp`'s `project--pfold-run` do) over capturing a closure value in a
`spawn` body — it avoids the heavier per-spawn `promote` deep-copy of a captured-env
closure regardless.

## KI-8 — RUNTIME form-position table (`positions`) stranded by compaction · **fixed 2026-07-03**

The bug-KI-3 class again, in a side table the KI-3 fix didn't cover. `RuntimeCode::positions`
(source positions of RUNTIME list forms) is keyed by the pair's RUNTIME **slab index**, with a doc
comment asserting "a RUNTIME pair never moves" — a pre-ADR-091 premise the compactor invalidated. A
compaction relocated the pairs but not the keys, so `(form-pos …)` / source-location returned a
stranger's position (or none) afterward. Diagnostics-only. **Fix:** `runtime_collect_with` remaps
the keys through the same `fwd.pairs` forwarding after the evacuation walks, dropping entries whose
pair didn't survive (mirroring the LOCAL `form_pos` remap). **Guarded by:** the `declared_sigs`
regression test exercises the same rewrite pass; the LOCAL analog `form_pos` is remapped in `collect`.

## KI-7 — declared `(sig …)` type-expressions corrupted by RUNTIME compaction · **fixed 2026-07-03**

The bug-KI-3 class in another off-graph holder. `RuntimeCode::declared_sigs` stores each `(sig name
type)` form's type-expression as a **promoted RUNTIME `Value`**, held in a `SymbolMap<Value>` beside
`globals` but NOT walked by `runtime_collect_with`. A compaction relocated the type-expr out from
under the stored handle, so the checker's `sig_of` later read a garbage form (confirmed: `(int ->
int)` read back as `(i 1)` after churn + compact). Silent wrong-data (no tripwire — the handle is a
valid RUNTIME index, just the wrong cell). **Fix:** `runtime_collect_with` now `flush_rt_value`s
`declared_sigs` alongside `globals`. **Guarded by:** `tests/runtime_collector.rs::declared_sigs_survive_a_runtime_compaction`.

## KI-6 — `%isolate` snapshot/restore not RUNTIME-compaction-safe · **fixed 2026-07-03**

A sibling of KI-2 (that fix handled orphan-process reaping; this one the compaction-relocation race).
`%isolate` snapshots the global table — an off-graph `SymbolMap<Value>` of raw RUNTIME handles — runs
a thunk, then restores it. A RUNTIME compaction *during* the thunk (its `def`s crossing
`BROOD_RT_GC_FLOOR`, trivially met in a large image) relocated those handles; the stale snapshot then
reinstalled handles aliasing *other* closures → an unrelated pre-isolate global silently misdispatched
(`foo` → a 1-arg `z-*` defined inside the rolled-back isolate). Latent for every `:isolated` test.
**Fix:** a re-entrant `Heap::rt_collect_block` counter (a `Cell<u32>`) suppresses RUNTIME compaction
while a globals snapshot is outstanding — `snapshot_globals` increments it and `restore_globals`
decrements it, so the invariant holds *structurally* (every caller of the protocol is covered, not
just `%isolate`). Checked at the `runtime_collect_with` choke point, so BOTH the auto safepoint path
(via `rt_gc_due`) and a manual `(runtime-collect)` are covered — an explicit collect inside an
`%isolate` is a no-op, not a corruption. The isolate's `def`s become garbage at restore and are
reclaimed by the next safepoint. **Guarded by:**
`tests/runtime_collector.rs::{isolate_is_safe_against_a_runtime_compaction_inside_the_thunk,
manual_runtime_collect_inside_isolate_is_a_noop}`.

## KI-5 — `nest test` OOMs: shared RUNTIME region accumulates every test file's code · **fixed 2026-07-03**

`run-project-tests` loaded every test file into one long-lived driver image before running any, so
each file's top-level `def`s promoted their compiled closures/chunks into the shared `RuntimeCode`
region + global table — globally rooted, live, unbounded, unreclaimable (only same-name redefinition
frees the old version). A 725-test suite crossed the 1 GB soft cap → `memory limit exceeded` on
whichever workers were allocating (brood-edit: 9 spurious failures, all passing in isolation).
**Fix:** `test/run-tests-scoped` (+ `-structured`) runs the suite file-by-file, each file inside its
own `%isolate` (reset → load one file → drain → rollback), so the file's `def`s roll back and the next
safepoint reclaims the promoted code — bounding memory to ~one file (relies on the KI-6 fix so the
mid-run rollbacks are compaction-safe). `BROOD_TEST_NO_SCOPE` reverts to the legacy load-all path.
brood-edit: OOM → 725/725 at 199 MB. **Guarded by:**
`tests/runtime_collector.rs::per_isolate_scoping_bounds_runtime_region_growth`.

## KI-4 — bitset stored as a non-UTF-8 `Value::Str` corrupts the GC on promote · **fixed 2026-06-15**

A bitset was a blob-backed `Value::Str` holding raw, non-UTF-8 bytes, but
`Value::Str`/`SharedBlob` carry a valid-UTF-8 invariant; promoting a closure that
captured one (`spawn`/`def`) read the bytes through the UTF-8 string accessor →
panic (armed) or UB/`flush_oob`/SIGSEGV (release). Surfaced ~1-in-3 in the
brood-life `--fair` demo. **Fix:** bitsets are a distinct `Value::Bitset` kind with
their own raw-byte slab (LOCAL `Vec` + RUNTIME `boxcar`), byte-clean accessor /
`promote_in` / equality / `Message::Bitset`, mirroring the `bigint` leaf slab — a
bitset can no longer reach a string accessor. **Guarded by:** the spawn-promote-a-
bitset path under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

> **Superseded 2026-06-28:** the whole `bitset` feature (the `Value::Bitset` kind +
> 13 `bitset-*` prims) was removed — it had no in-repo or external consumer left.
> The KI-4 fix is moot now that the type is gone; kept here as the historical record.

## KI-3 — RUNTIME compaction strands live VM / tree-walker constants · **fixed 2026-06-01**

Once the ADR-076 RUNTIME compactor made promoted code-region handles movable, two
sites held them as immovable: the tree-walker elided the operand-stack slot for a
RUNTIME root (so `runtime_collect` never rewrote it), and the VM held promoted
handles inline in `Node::Const`/`MakeClosure.fn_rest` (off the GC root graph). A
compaction at a nested safepoint left them dangling → `flush_oob` or a constant
read back as a different value. **Fix:** `needs_root_slot` (LOCAL **or** RUNTIME)
gives a RUNTIME handle an operand slot; the VM carries movable consts as
`ConstVal::Handle` and registers its live arm in `Heap::live_vm_arms`, which
`runtime_collect` rewrites in place. **Guarded by:** `compile::tests::{const_handle_round_trips,
rewrite_arm_handles_rewrites_every_embedded_handle}` and
`tests/runtime_collector.rs::auto_safepoint_collect_bounds_runtime_region`.

## KI-1 — multi-thread scheduler race: green processes can't resolve globals · **fixed 2026-05-29**

Spawning green processes that touched globals crashed workers with bogus `unbound
symbol` errors (a data race on shared global/scope state via the kernel
supervisor's RESUME_SLOT machinery, worsened by free-list slot reuse). **Fix
(in series):** strip the kernel supervisor (ADR-039, reverted → ADR-044); switch
to a bump-only allocator (slots never recycle, so a stale handle can't observe a
wrong-type value); per-worker pinned queues. **Durable invariant:** no recycled
slots / no stale handles across a safepoint. (The per-worker *pinning* stopgap was
later superseded by ADR-100's heap-captured continuations, which make cross-thread
migration safe and routine.) **Guarded by:**
`tests/concurrency_race.rs::fanout_with_concurrent_global_rebind_matches_serial`
(the `concurrency-v2.md` §6 bar) and the self-diagnosing `flush_oob`/`flush_bound!`
OOB check.

## KI-2 — `nest test` flaky / hangs when parallel tests share heavy global lookups · **fixed 2026-05-29**

Two bugs: (1) the KI-1 lookup race could kill a worker; (2) the runner didn't reap
a dead worker, so the run hung in `receive` forever. A 2026-06-07 recurrence under
maximal load was root-caused **not** to a core race but to test isolation:
`%isolate` (test-only) wholesale-restored the globals table, so a test that left an
orphan process running saw the orphan's next lookup die `unbound`. **Fix:** the
runner `monitor`s every worker and accounts for each exactly once (death → a failing
result, not a hang); `%isolate` reaps the processes its thunk spawned (via the
green-friendly `scheduler::yield_now`, never a thread sleep) **before** restoring
globals. Production never wholesale-restores globals, so the language itself was
never implicated. **Guarded by:** `tests/runner_failfast_test.blsp`.

## Platform gaps — GUI display seam · **all resolved 2026-05-31 (ADR-079)**

The GUI frontend had one font size for everything. Resolved: a `Face` carries an
integer `:scale` (per-op/region larger text in a scale×scale cell block — also
covers per-pane font); `gui-font!` takes an optional window id for per-window fonts;
`std/editor/pane.blsp` (ADR-077/078) provides pane layout + clip-rects. Per-pixel
`:height` sizing stays deferred (would break the uniform grid).

## Minor (all fixed)

- **Type-checker noise around `(require 'proc/hatch)`** — `check_file` pre-evaluates
  top-level `(require …)` so the required module's macros resolve.
- **`nest format` collapsed multi-line forms** — fixed (`5b19787`); respects author
  newlines. Still normalizes intra-line multi-space alignment (a standard trade-off).
- **Plain-release segfault on tail-recursive workers** — fixed by per-worker pinned
  queues, then made moot by ADR-100 (heap-captured continuations).
- **`cargo test --test suite` debug segfault** — coroutine stack overflow, not a
  memory bug; `WORKER_STACK_BYTES` raised (pages mmap'd lazily, ~0 cost until needed).
