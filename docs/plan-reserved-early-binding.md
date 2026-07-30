# Plan — Phase 1: reserved-name early-binding (ADR-166 realization)

Status: **not started, deliberately held** (2026-07-30) until the concurrent JIT
perf thread settles, so the two don't collide in `jit_lower`/`dispatch` or confound
each other's `make ab` numbers. This is the concrete, ready-to-execute plan for
ROADMAP "Language core & types" item — *Early-bind reserved names (ADR-166)* and its
sibling *`get`'s call + type-dispatch overhead*.

The bigger effort is three phases (see ROADMAP); this doc is **Phase 1 only**.

## Goal

Since a **reserved** global (anything shipped in the `brood` binary — prelude fns,
builtins, embedded std modules; ADR-166) **cannot be rebound**, a call to one is
statically resolvable. Phase 1 makes the compiler resolve a reserved-*defn* call head
at compile time and drops the now-pointless staleness guard on that call, and makes
the checker type reserved globals precisely instead of `dynamic()`. It does **not**
inline the callee body — that is Phase 3, which this unblocks.

Target callees: reserved **prelude defns** (`get`, `map`, `filter`, `reduce`,
`assoc`, `count`, `str`, …). Reserved builtins that already map to `PrimOp`/`PrimOp1`
(`+`, `-`, `<`, `first`, `rest`, `map-get`, …) are **already** compile-time inlined
(`resolve_prim`/`resolve_prim1`), so they are out of scope.

## What already exists (do NOT rebuild)

- `Heap::is_reserved_global(sym)` — the reserved test; already consulted for the KI-19
  staging exemption (`compile/mod.rs` ~1055, `let staged = … && !is_reserved_global`).
  Note its module-load carve-out: it reports **false** while a module defines its own
  reserved surface, so any resolution keyed on it is automatically conservative there.
- `resolve_prim` (`compile/mod.rs:646`) / `resolve_prim1` (`:742`) — the model for
  compile-time head resolution to inline ops. Phase 1 adds the *defn* analogue.
- The BROOD_MONO devirtualization (`compile/mod.rs` ~1021, ADR-182) — already turns a
  call head into a **`Const` callee** and the exec/dispatch path already runs one. This
  is the closest existing precedent for baking a resolved callee; reuse its shape, but
  see the caveat under Step B (a plain `Const` callee falls to the *computed-head*
  path, which skips the fast-link — Phase 1 must keep the fast-link).
- Checker inference is already substantial: Pass 2.7 (once-defined global value types)
  and Pass 2.8 (function return-from-body) in `types/check.rs` ~848. So Step A's gain is
  **narrow** — it is precise *arrow/return* types for reserved fns at call sites, not a
  from-scratch typing.

## The change, in dependency order

### Step A — Checker: precise types for reserved globals (no `jit_lower`, zero collision)

`expr_ty`/`gradual_of` of a `Node::Global(sym)` returns `dynamic()` for a redefinable
global (`types/check/ctx.rs:170`, `check.rs` ~854-869). For a **reserved** `sym`, the
value is immutable, so return its actual type:
- a reserved **fn** → its inferred arrow (Pass 2.8 already computes the return; wire the
  reserved case to consult it instead of falling to `dynamic()`);
- a reserved **data** global (`*features*` etc.) stays `dynamic()` — ADR-166 exemption 1
  keeps prelude *data* rebindable, so it is correctly not reserved-as-a-function.

Files: `types/check.rs`, `types/check/ctx.rs`, `types/check/walk.rs` (the `expr_ty`
Global arm, ~1577/1595). Guard on `is_reserved_global` + `!value::is_dynamic` +
`!is_earmuffed` (mirror the Pass 2.7 skip set). **Ship this first, on its own** — it is
self-contained, improves warnings, and touches none of the perf-critical code.

### Step B — Compiler: resolve reserved-defn call heads at compile time

In `compile/mod.rs` `compile_call`, where a free-symbol head becomes `Node::Global(h)`
(~1013-1016): when `h` resolves to a reserved global whose value is a `Value::Fn(id)` in
the PRELUDE region with a compiled arm for this argc, emit a **new resolved-call node**
(working name `Node::ResolvedCall { id, arm_hint, args, site, pos }`) instead of
`Node::Global(h)` + generic call.

Key constraints:
- Must feed the **fast-link** path, not the computed-head slow path. The mono `Const`
  callee falls to computed-head (no fast-link) — acceptable for a literal-arg op, **not**
  for `get` at 4.8k sites. So `ResolvedCall` carries the resolved `Fn(id)` as an *elided*
  head (like a free-global head today) so it still gets a call site + fast-link, but with
  the callee identity known at compile time.
- Multi-arity: pick nothing here (that is Phase 2); just resolve the *identity*. The
  existing per-argc arm selection at the call site still applies.
- Conservative: decline the rewrite if `is_reserved_global(h)` is false, if the head is
  shadowed by a local (`scope.lookup(h).is_some()`), or if resolution is unavailable at
  compile time (prelude self-build ordering — a forward ref during prelude construction;
  fall back to `Node::Global`).

Files: `compile/mod.rs` (the head arm + a new `Node` variant in `ir.rs`), `emit.rs`
(lower the new node — mostly a thin wrapper over the existing elided-call emit).

### Step C — JIT/dispatch: drop the staleness guard for a reserved-resolved call

The IR fast-link path guards every call on `epoch == global_epoch()`
(`jit_lower/call.rs` `chk_epoch`, and the `FastLink.epoch` field). For a `ResolvedCall`
to a reserved callee the epoch can **never** change the resolution (reserved ⇒ no `def`
can rebind it; RUNTIME compaction still bumps the epoch, so keep the guard bit that
protects the *code pointer* validity, but the *identity* re-check `sym`/`argc` becomes
statically true). Net: the reserved path skips the global-read IC entirely and the
identity re-validation, keeping only the code-pointer/epoch check that GC correctness
needs. The ROADMAP note that "the `PrimOp1` epoch guard is already unreachable for its
original purpose" (`ir.rs` PrimOp1) is the same observation — reserved ⇒ no staleness.

Files: `jit_lower/call.rs` (the `chk_ident`/`chk_epoch` blocks — make them conditional on
whether the callee is reserved-resolved), `eval/compile/dispatch.rs` (the interpreter
mirror). **This is the step that collides with the parallel perf thread** — do it last,
behind a clean commit, and re-verify with the KI-20 discipline below.

## Correctness plan (same discipline as the KI-20 fix)

1. `cargo test --release -p brood --test jit --test differential` after **each** step —
   JIT output must stay bit-identical to `BROOD_VM=0` and the VM.
2. Debug run under `BROOD_GC_STRESS=1 BROOD_JIT_VERIFY=1` (the reserved-resolved callee
   still stages args through `roots`; the GC/handle checks must stay clean).
3. Full `make test`.
4. A reload test: `def`-ing a **user** function of the same *bare* name as a reserved one
   inside a `(defmodule …)` must still resolve to the user's, not the reserved arm
   (guards the shadowing/carve-out logic).
5. A checker test (Step A): a `sig` mismatch against a reserved fn's real return type now
   warns where it previously deferred to `dynamic()`; `nest check` stays zero-warning on
   `std/` + `tests/`.

## Measurement plan

`make ab BASE=<pre-phase1-sha>` — needs the `brood-benchmarks` repo present at
`../brood-benchmarks` (not on this machine as of 2026-07-30; clone first). Rows to watch:
- **Wins expected:** prelude-heavy rows that call reserved defns hot — `regex`,
  `wordcount`, `sieve`, `json` (the same rows the KI-19 staging *regressed*, which is the
  signal this path is hot). A `get`-heavy microbench (`(get m :k)` × 2M) is the direct
  probe — but note Phase 1 alone only removes the call/IC overhead, **not** the wrapper +
  `cond` body (that is Phase 3), so expect a *modest* per-call win here, not the full
  393 ms measured in ADR-165.
- **No-regression gate:** `fib`, `bintree`, `nqueens`, `pipeline` (self-recursion +
  message rows) — pinned best-of-15 with a base-vs-base control (some rows drift between
  whole `make ab` invocations; see CLAUDE.md). Re-run any tiering-affected row **unpinned**.
- **The editor rows (the motivating workload):** `brood-edit/bench/keystrokes.blsp`
  (`nest run bench/keystrokes.blsp` in that repo) times the live per-keystroke path —
  self-insert, Enter/indent, backspace, C-n/C-p/C-f, the frame render, and the felt
  type+render cycle — over a large brood-mode buffer, with text-mode and small-buffer
  controls. Baseline 2026-07-30: type+render ~3.8ms felt (of which ~1.25ms is the
  brood-mode fontify/span refresh — interpreted span assignment over native
  `scan-tokens`, exactly the reserved-call-heavy code Phase 1 targets). Run it
  before/after each step alongside `make ab`.

## Risks / gotchas

- **Prelude self-build ordering.** During prelude construction a reserved head may not be
  resolvable yet (forward ref). `is_reserved_global` already reports false mid-module-load,
  but double-check the prelude-build path and fall back to `Node::Global` when resolution
  is `None`.
- **Shadowing.** `(let (get …) …)` and `(defmodule m (defn get …))` both make `get` mean
  something else locally; the rewrite must only fire for the *global* reserved binding
  with no local shadow (`scope.lookup(h).is_none()`, already the condition for the
  `Node::Global` head arm).
- **GC / RUNTIME compaction still bumps the epoch.** Do not drop the epoch check that
  protects the *code pointer* (a compaction can move the arm's native code); only the
  *identity* (`sym`/`argc`) re-check is statically dischargeable for a reserved callee.
- **JIT miscompiles pass tests.** Steps B/C are `jit_lower`-critical; treat like KI-20.

## Explicitly NOT in Phase 1 (later phases)

- **Phase 2 — multi-arity devirtualization:** pick the arm for a known argc at compile
  time (the ADR-165 `+24 ms` multi-arity-dispatch cost).
- **Phase 3 — see-through inlining of a non-leaf reserved body:** inline `get`'s `cond`
  (which calls builtins), so `(get m :k)` lowers toward `(map-get m :k nil)` at the 4,796
  keyword call sites. The big win, the highest risk; Phase 1 (compile-time identity +
  no staleness guard) is its prerequisite.
