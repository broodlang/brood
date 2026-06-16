# Call dispatch & hot reload — the per-call late-binding tax (lever #1 scope)

> **Status: SCOPE / decision (2026-06-16).** Records the hot-reload decision and scopes the
> three ways to remove the per-call validation tax, informed by how BEAM does it. The
> per-call IC + epoch validation is the bulk of `jit_dispatch_call` (~30–43% of fib/spawn).

## Decision: keep hot reload (Emacs-style); make it cheap by paying at `def`-time

Brood **keeps runtime redefinition of globals** (ADR-013) — it's the editor's reason to exist
(eval a `defun`, it takes effect for the next call; introspection/advice). What changes is the
*cost model*: today every global call validates a per-site inline cache + a global epoch **per
call**; we move that cost to **`def`-time** (repoint a dispatch cell once) so steady-state calls
are validation-free.

**Accepted compromises (Emacs-style is fine with these):**
- In-flight / recursive calls may **finish on the old version** of a redefined function (BEAM
  semantics). The *next* fresh call hits the new code. Acceptable for an editor.
- The **prelude is not redefinable** (it's an immutable shared region) — already true.

## How BEAM does it (researched against `/home/whk/src/erlang/otp/erts/emulator/beam/`)

The only mutable indirection is on the **module boundary**; cost is paid at load, not per call:
- **Local (intra-module) calls** compile to **bare direct native `call`s — no validation, no IC**
  (`jit/x86/instr_call.cpp:80`, `erlang_call(resolve_beam_label(...))`). Code replacement never
  patches these; old code keeps its direct targets and is reclaimed by purging.
- **External (`mod:fun`) calls** go through **one** indirection: a stable per-function `Export`
  whose `dispatch.addresses[active_code_ix]` holds the current native address (`instr_call.cpp:120`,
  `export.h:40`). One indexed load + indirect call — no branchy validation.
- **Loading** writes new addresses into a *staging* code-index slot, then flips
  `the_active_code_index` with **one atomic store** (`code_ix.c:122`). No call sites are walked.
- Two live versions per module (current/old) + a 3-slot code index give lock-free reads + safe
  concurrent old/new execution. The **reduction/yield check is in the callee prologue**
  (`a.dec(FCALLS); jle`), not at call sites.

Takeaway: **split the call ABI on the redefinability boundary, and put a repointable dispatch cell
on each redefinable function.** Don't validate per call — make the cell the source of truth.

## The three options (to try + measure — "try all, see what's best")

All keep hot reload; they differ in how much per-call work they remove and how much machinery.

**A. Prelude direct-linking.** The prelude is immutable, so a call whose callee resolves to a
prelude function can be a **direct native call** — no IC, no epoch (BEAM's "local call"). Covers
`+`/`<`/`map`/`fold`/… (a large fraction of hot calls). Lowest machinery; always safe. Doesn't
help calls to *user* globals (the editor's own funcs).

**B. Dispatch-cell (BEAM's model, the general fix).** Give every redefinable global function a
stable **dispatch cell** holding its current native code pointer (+ arity guard). A call loads
`cell.code` and calls it directly — **one load, no epoch compare, no IC probe**. `def` repoints
the cell (atomic store); in-flight callers holding the old pointer finish on old code. Removes the
per-call validation entirely while keeping full hot reload. Most machinery (a cell per function,
the `def` path repoints it, the JIT emits the load+indirect-call), but it's the real answer and
subsumes A (prelude functions get a cell too, never repointed).

**C. Sealed release build.** A `--sealed` / `--release` build flag that **disables `def`-rebinding**
→ *every* call links directly (no cell, no IC). For shipped apps that don't self-edit. Trivial once
B exists (just pin the cells / skip the indirection); a strict upper bound on the win, and a good
way to **measure the ceiling** before committing to B's machinery.

## Recommended approach

1. **Measure the ceiling first (C-style):** a build/flag that direct-links all calls (no per-call
   validation) — tells us how much #1 is worth before building B's machinery. (This session's
   lesson: several "obvious" wins measured ~0; measure before building.)
2. If the ceiling justifies it: **B (dispatch-cell)** is the durable answer — keeps Emacs-style
   reload, removes the per-call tax, subsumes A. Ship **C** as a build mode on top (free once B
   exists). The reduction/yield safepoint can move to the callee prologue (BEAM-style) to keep call
   sites to a single load + indirect call.

## Risk + validation

The call ABI is load-bearing + GC/JIT-entangled — HIGH risk. Per-increment gates (per
`docs/jit-tier2.md §7`): the JIT≡tree-walker + VM≡tree-walker differential corpora,
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`, the full in-language suite through the JIT, `make test`, and
a **hot-reload-across-call test** (a `def` between warmed batches must take effect for the next
call while an in-flight recursive call finishes on old code — extend `tests/jit_shared_spawn_test.blsp`).

## Key files & symbols

- `crates/lisp/src/eval/compile.rs` — `jit_dispatch_call` (the per-call probe to replace),
  `vm_call_ic_fast_link` / `vm_call_ic_probe` (the IC), `jit_tier`, the `Inst::Call` lowering.
- `crates/lisp/src/core/heap.rs` — `RuntimeCode` (globals + `version`/`global_epoch`; the natural
  home for dispatch cells), `CallIcEntry`, `jit_shared_lookup`/`jit_shared_publish`.
- `crates/lisp/src/jit/mod.rs` — `brood_rt_call`/`call_slow` (the call FFI), `Jit::new`.
- Background: `docs/jit-optimizing-tier.md` (inlining — complementary), ADR-013 (hot reload).
