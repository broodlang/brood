# Handoff: native-callback VM routing + the `let`-self-ref send divergence

Two linked VM tasks, paused to hand off because the `let`/closure/`send` area is
under active edit. **#1 is the blocker; #2 is the payoff.** Pick up #1 first.

> ⚠️ **Re-establish ground truth before doing anything** — this area changed
> several times in one session. Run the probe in §1 and confirm the table still
> holds; if it doesn't, re-derive from the current behaviour, don't trust this doc's
> exact verdicts.

Companion docs: `docs/benchmarking.md` (the `perf-stats` / `bench-ratio.sh` tools
to measure with), `docs/decisions.md` ADR-096 (the VM perf round), `docs/bytecode-vm.md`
(the closure-compiling VM). Relevant commits: `b704c13` (round-2 self-call /
`letrec` self-name `env_define`), `4af9d2a` (the `%range-reduce`-on-VM precedent
that #2 generalizes).

---

## Task #1 — make `let`-self-ref closures consistent across engines (correctness)

### The divergence (ground truth at HEAD `9931e1d`)

A self-referential **local** closure — `(let (f (fn (x) (f x))) f)` — built in a
RUNTIME context (inside a `defn`, so the VM actually compiles it):

| case | VM | tree-walker |
|---|---|---|
| `let`-self-ref **call** (`(f 3)`) | `:done` | `:done` |
| `letrec`-self-ref **call** | `:done` | `:done` |
| **`let`-self-ref `send`** | **ACCEPTED** | **REJECTED** |
| `letrec`-self-ref `send` | REJECTED | REJECTED |

Reproduce:

```lisp
(defn mk-let    () (let    (f (fn (x) (f x))) f))
(defn mk-letrec () (letrec (f (fn (x) (f x))) f))
(defn snd (g) (let (me (self)) (try (do (send me g) :ACCEPT) (catch e :REJECT))))
(println (list :let (snd (mk-let)) :letrec (snd (mk-letrec))))
;; run twice: default (VM) and BROOD_VM=0 (tree-walker), compare.
```

### Root cause

`send` serialises a closure with `closure_to_message`
(`crates/lisp/src/process/message.rs:249`): it walks the closure's free vars,
looks each up in the closure's **local env chain**, and errors *"cannot send a
self-referential local closure"* if that walk re-enters the same `ClosureId` (a
cycle). So `send` rejects **iff the closure is *structurally* self-referential**
— i.e. its captured env actually contains a binding to itself.

- **tree-walker `let`**: the closure captures the `let` env *by reference*
  (`bind_sequential`, `crates/lisp/src/eval/mod.rs`), and the binder is `env_define`d
  into that same env, so `f` → closure is in the env → structural cycle → REJECT.
- **VM `letrec`**: round-2 (`b704c13`) makes the closure structurally self-ref via a
  self-name `env_define` (search `self_name` in `crates/lisp/src/eval/compile.rs`,
  `compile_make_closure` / the `MakeClosure` exec arm) → structural cycle → REJECT.
- **VM `let`**: the user's later fix made `let`-self-ref *resolve* `f` at call time
  (so the call works, `:done`) but apparently **not structurally** — `f` is not a
  cycle in the captured env, so `closure_to_message`'s walk never re-enters it →
  ACCEPT. **This is the gap.**

### Why it matters

1. **Correctness**: VM and tree-walker disagree on whether a value is send-able —
   a violation of the "VM ≡ tree-walker" contract the differential harness is
   supposed to guarantee. The harness misses it because it never `send`s a
   RUNTIME-context `let`-self-ref closure (blind spot — see §3).
2. It is exactly what blocks **#2**: when a test-framework test body runs on the VM
   (which #2 does), `(let (f (fn (x) (f x))) … (assert-error (send me f)))` stops
   raising (VM accepts), so the adversarial test *"a self-referential local closure
   send is rejected cleanly"* fails.

### The fix

Make the VM build a `let`-self-ref closure **structurally** self-referential, the
same way `letrec` already does — reuse the self-name `env_define` path so `f`'s
captured env contains `f → the-closure`. Then `closure_to_message`'s cycle walk
finds it and `send` REJECTs consistently with the tree-walker, while the call-time
resolution keeps working (`:done`).

Concretely:
- Inspect the current `let` path in `compile_let` (`crates/lisp/src/eval/compile.rs`)
  — how the user's Position-2 fix resolves `f` for a sequential-`let` binder. It is
  almost certainly resolving `f` *without* adding it to the `MakeClosure` captured
  env (that's why it's non-structural).
- Route it through the same `self_name` mechanism `letrec` uses (the
  `MakeClosure { self_name }` field + the exec-arm `env_define(env, self_name,
  closure)`), so the `let` binder that a closure refers to becomes a real captured
  self-binding.
- After: the §1 probe must show `:let REJECT` on the VM too (matching TW), and
  `let`/`letrec`-self-ref calls still return `:done` on both.

**Decision context (read before choosing a direction):** when asked, the owner
chose *"`let` is strictly sequential (VM is right)"* — i.e. a binder is **not**
visible in its own RHS, use `letrec` for self-reference. BUT a naive enforcement
(make `let`-self-ref *unbound*) breaks ~6 existing tests that deliberately use
`(let (loop (fn …)) (loop …))` and a GC test
(`promotes_cyclic_local_closures_without_crashing`) that *exists to verify the
closure↔env cycle a `let`-self-ref builds. The owner subsequently *implemented*
the opposite (`let`-self-ref **works**, Position-2). So the **de-facto intended
behaviour is: `let`-self-ref works and is a genuine self-referential closure** —
which means the fix above (make it structural so `send` rejects, matching
`letrec`/TW) is the consistent resolution, **not** making it unbound. Confirm this
is still the owner's intent before implementing (the area is in flux).

---

## Task #2 — route native higher-order callbacks through the VM (perf)

### What

`%range-reduce` was changed (commit `4af9d2a`) to call its reducer back through
the VM (`compile::apply_value`) when `vm_enabled()`, instead of `eval::apply`
(tree-walker). Result: **`reduce`/`fold` over a range ~65–67% faster** on the VM
(measured, `reduce_range` bench in `crates/lisp/benches/eval.rs`). The **other**
native higher-order callbacks still tree-walk their user code regardless of
engine — generalize the same routing to them.

### The code (written + reverted twice; trivial to re-apply once #1 lands)

Add a helper next to `apply_value` in `crates/lisp/src/eval/compile.rs`:

```rust
/// Apply `callee` through the active engine: the VM when on (a VM-eligible callback
/// runs compiled), pure tree-walker under BROOD_VM=0 (keeps the escape hatch /
/// differential TW mode honest). `eval::apply` itself must stay pure tree-walker —
/// it's `dispatch`'s fallback, so routing it back through `apply_value` recurses.
pub fn apply_engine(heap: &mut Heap, callee: Value, args: &[Value], genv: EnvId) -> LispResult {
    if vm_enabled() { apply_value(heap, callee, args, genv) }
    else { crate::eval::apply(heap, callee, args, genv) }
}
```

Route these 5 callback sites in `crates/lisp/src/builtins.rs` (currently
`apply(heap, …)`) through `crate::eval::compile::apply_engine(heap, …)`:
- `apply_builtin` — `(apply f args)` — the generic call primitive.
- `try_catch` ×2 — the protected **thunk** and the **handler** (so code inside
  `try`/`catch` runs compiled — likely the biggest win, `try` wraps a lot).
- `binding` — the dynamic-scope **body** thunk.
- `isolate` — the **thunk**.

(`%range-reduce` already does this, hoisting the `vm_enabled()` check out of its
per-element loop — keep that pattern for hot loops; the 5 above are once-per-call,
so the helper is fine.)

### Why it's blocked on #1

Routing `try` thunks through the VM makes **test bodies run on the VM** (the test
framework wraps each test in a `try`/spawned process). That surfaces the §1
divergence: the adversarial tests build `let`-self-ref / `let`-bound-recursive
closures, and on the VM they serialise differently → the suite fails (the
deterministic one is *"a self-referential local closure send is rejected
cleanly"*, plus several `let`-bound `build`/`loop` cases). Once #1 makes VM `let`
closures structurally consistent with the tree-walker, this generalization should
pass — it did **not** before, even after the calling-divergence was resolved,
*specifically* because of the `send` (structural) gap.

---

## Verification protocol (for both)

1. `cargo build` clean (default + `--features perf-stats`).
2. **Differential / full suite**: `make test` (cargo-nextest). Watch the
   `brood::suite brood_suite_passes` in-language case and the **adversarial** tests
   in `tests/adversarial_test.blsp` — those are what broke. Known-flaky:
   `cli::distribution clean_peer_exit_fires_nodedown_promptly` (timing; passes in
   isolation — not a real failure).
3. **GC-stress** for anything touching rooting/closure construction:
   `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` on a closure-heavy + `send`-heavy workload
   (debug build arms the tripwire + verifier).
4. **#2 perf**: confirm the win load-robustly with the engine-grid ratio — add a
   `try`-wrapped workload to `benches/eval.rs` (`engine_grid!`) and run
   `scripts/bench-ratio.sh`; confirm with `perf-stats` that the body actually ran
   on the VM (non-zero `vm_apply`), not a deferring bench (the recurring trap:
   top-level `(fn …)` literals are LOCAL-region and **defer** — use named/`defn`
   closures so the workload is genuinely VM-compiled).

## Close the differential blind spot (do this regardless)

Add a differential/cross-engine test that **`send`s a RUNTIME-context
`let`-self-ref closure** (and a `letrec` one) and asserts the same verdict on both
engines. Its absence is why this divergence shipped unnoticed.

## Status of the surrounding work (already landed, on origin/main)

- VM **profiling harness**: `perf-stats` feature + `(vm-stats)` + `BROOD_PERF_STATS`
  + `scripts/bench-ratio.sh` (+ `scripts/bench_ratio.py`). See `docs/benchmarking.md`.
- `(def x <expr>)` runs its RHS on the VM.
- `%range-reduce` callback on the VM (the #2 precedent), `4af9d2a`.
- Profile verdict: the VM is **dispatch-bound** (IC 99.99% hit, prim2 96% inlined,
  env/alloc minor) — so after #1/#2, the next *structural* lever is **bytecode
  lowering** (ADR-096; the JIT on-ramp), not more micro-opts.
