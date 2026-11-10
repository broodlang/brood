# Lexical-addressing gotchas — a checklist for the VM compiler

> Handoff (2026-05-30) from a parallel `worktree-lexical-addressing` experiment.
> That branch addressed locals as rewritten *coordinate forms in the closure body*;
> the merged VM (ADR-076) instead keeps coordinates in a **per-process side cache**
> (`Heap::vm_cache`, `Arc<CompiledArm>`) and reads frame slots directly off
> `Heap::roots`. The branch's *code* is superseded, but the **edge cases it hit are
> universal** — any lexical addressing must handle them. This is the checklist to
> verify against as the VM grows. Compiled code is now also shared across a runtime's
> processes (ADR-175, `RuntimeCode::shared_closures`); it holds slot numbers, never a
> sender's frame layout.

## Status against the VM

| # | Gotcha | Status |
|---|---|---|
| 1 | Shipped-closure stale coordinates | **Handled.** Stage 2c landed: a closure's captures are read from a flat env frame by compile-time capture lists, and a free variable resolves by name through the closure's own env, so nothing addresses by frame offset across a process boundary (`bytecode-vm.md` §Stage 2c). |
| 2 | Static analysis on addressed bodies | **Not a bug** — the closure's stored `body` is untouched source; the checker never sees coordinates. |
| 3 | Frame-model edge cases | **Handled** by the flat frame (Stage 2a): one slot per binder, sequential `let`. A `(fn …)` capturing a not-yet-finalized `letrec` binder defers to the tree-walker. |
| 4 | Verify-and-fallback safety | Not adopted (flat frame carries no names); rely on construction + tests. |
| 5 | Frame-slot-direct beats coordinate-forms | **Confirmed** — validates the merged VM's design. |

## The checklist

### 1. Shipped-closure stale coordinates (the subtle one — highest value)

A frame coordinate baked into a closure is **invalid after `spawn`/`send`**: the
receiver rebuilds the closure's captured env *flattened* — only its free vars, and
**reordered** (`process/message.rs` `ClosureMsg::captured`). So a coordinate that
indexed the sender's frame layout points at the wrong slot on the receiver.

- The branch's fix: strip coordinates → names during `to_message`.
- **The VM's defence:** never ship coordinates. They live in `Heap::vm_cache`
  (per-process), re-derived from the closure's *source* body on the receiving side.
- **The constraint Stage 2c met:** with local-capturing closures VM-eligible, the
  compiler must derive captured-var slots from the closure's **actual captured-env
  layout in *this* process** (post-flatten/reorder), never from a remembered
  sender layout, and never bake them into anything that crosses a process boundary.

### 2. Static analysis breaks on addressed bodies

The type checker reads stored closure bodies; coordinate forms in them make
param-inference and unbound-checks misfire.

- The branch's fix: a separate `expand_and_resolve` (checker path stops *before*
  addressing) + `deref_local` in `infer_sig`.
- **The VM's defence:** addressing is not a body rewrite — the stored body stays
  source. Keep it that way: if a future stage ever rewrites bodies, the checker
  needs its own pre-addressing path.

### 3. Frame-model edge cases (silent miscompiles if wrong)

- **`letrec` nil-prebind + append → duplicate slots.** letrec pre-binds names to
  `nil` then `env_define`s them, which *appends* (eval scans from the end). A naive
  slot assignment can hand one name two slots. Assign each letrec name **one
  distinct slot**, pre-init nil, then fill in order.
- **Sequential `let`.** Brood `let` *is* sequential (`SpecialForm::Let`; there is no
  separate `let*`): each binding sees the earlier ones. Assign slot indices
  **incrementally** as bindings are compiled, with the rhs compiled against the
  scope *so far*.
- **`%match` opacity.** The branch found the pattern engine's output opaque to
  addressing. In the VM a `match` expands to ordinary forms that compile, so this
  applies only to a shape the compiler declines, which defers.
- **Don't address the call-head position.** Leaving a call head as a name keeps the
  fast symbol-call path; addressing it buys nothing and risks the head-vs-operand
  distinction. (The merged VM compiles the head uniformly to `Global`/`Local` and
  resolves globals via the inline cache + passthrough — equivalent and fine.)

### 4. Verify-and-fallback safety (cheap insurance)

Carry the **name** alongside the coordinate; at read time assert `slot.name ==
name`, and **fall back to a name-scan on mismatch** — so a bad coordinate is never
a *wrong value*, only a (debug-panicking, release-correct) slow path. The branch
did this in `heap.rs::env_get_indexed`.

- **VM note:** the flat `Heap::roots` frame carries no names, so this isn't free
  here. Consider a debug-only parallel name table for the frame to assert
  `compile-time slot == runtime name` under `debug_assertions`. Until then,
  correctness rests on coordinates being derived from the same structure that
  builds the frame, plus the suite + `BROOD_GC_STRESS` gate.

### 4b. One pair, one position (KI-288)

The body cache is keyed on the **identity of a closure's first body form**
(`VmCacheKey::Body`), and the capture slots are derived from the first instance's
enclosing names (`env_chain_names`). Both assume a pair sits in exactly one code
position. Anything that puts one pair in two — a source rewrite keeping a fallback beside
its fast copy, a macro splicing one argument twice — makes two closures (or one closure in
two scopes) one cache entry, and the second runs the first's compiled body.

- **The defence:** every pass that places a sub-form a second time copies it with
  `copy_form` (`eval/macros.rs`), and the expansion walk returns fresh cells for every list,
  `quote` forms included. Keep it that way: a new rewrite that reuses a sub-form copies it,
  and a walk that returns an unchanged list as it is must run where its input already has
  one pair per position (not the expansion walk — its rebuilding is what separates a
  spliced-twice macro argument).

### 5. Benchmark finding (confirmation, not work)

A *list-form* coordinate (a rewritten `Value`) regressed **shallow** locals — the
decode cost exceeded the name-scan it replaced. The VM's **frame-slot-direct**
read (`root_at(base + slot)`, no coordinate `Value`) is exactly why it avoids this.

## See also

ADR-076 + [`bytecode-vm.md`](bytecode-vm.md) (the VM design + as-built),
`process/message.rs` (`ClosureMsg::captured` — the flatten/reorder that makes #1
real), ADR-073 (package-rooting design, from the same handoff — unrelated to the
VM; for whoever takes package/namespace isolation).
