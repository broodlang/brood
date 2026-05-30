# The execution-engine plan — a closure-compiling VM

> **Status (2026-05-31): Stage 0–3 done — the VM is the default engine (~1.6–2.3×).**
> The design record is **ADR-076**; this file is the long-form companion. Nothing
> here changes the language — it is purely an **execution-engine** swap; `std/*.blsp`
> and user code are untouched, and the full Rust + in-language suite is green on
> **both** engines. Stage 0–1 (mechanism + passthrough redirect), 2a (`let`/`letrec`),
> 2b (multi-arity), **2c (local-capturing closures — the GC-critical one)**, and
> source-position threading are all merged. **Stage 3 cutover (done):
> `eval::compile::vm_enabled` now defaults the VM *on*; `BROOD_VM=0` is the
> tree-walker escape hatch (retained for ≥1 release).** A core-vocabulary closure
> runs on the VM; anything else transparently defers to the tree-walker per-form, so
> the language is unchanged. **Still worth doing:** a differential test mode (run the
> suite through both engines, assert identical output) as a standing CI guard, and
> widening VM coverage (variadic / patterns / prelude closures — pure perf). See
> [As-built](#as-built-stage-01-2026-05-30) for the numbers and the §7 plan.

This is the project's "big lever" for performance: closing the tree-walker's
structural ~50–220× tax (ADR-069's measurement) over the Node/Elixir range. It is
the commitment ADR-069 deferred when it said *"the honest fix for a tree-walker's
structural tax is a bytecode / closure-compiling VM … revisit [lexical addressing]
when we commit to the compilation step."* **This plan is that commitment, and
lexical addressing is its Stage 1, not a side-quest.**

The priority is unchanged from ADR-069: **"stay in Brood" beats raw speed.** The
VM closes most of the gap *while preserving every GC / TCO / preemption / hot-reload
invariant* — that is worth more here than the last 2× a hand-tuned bytecode loop
might extract.

---

## As-built (Stage 0–1, 2026-05-30)

Implemented in `crates/lisp/src/eval/compile.rs` behind the `BROOD_VM` env flag
(off by default; the tree-walker remains the engine), on branch
`worktree-bytecode-vm`. Three commits: Stage 0 (scaffolding), Stage 1 (mechanism),
and the primitive-redirect increment.

**What runs on the VM (the bounded slice).** A call reaches the VM only when the
top-level form compiles to a core-vocabulary `Node` chain down to it (the seam is
in `eval_str`/`eval_source`, after `macros::compile`). An eligible callee is a
**top-level, single-arm, exact-arity, global-capturing** closure whose body is
built only from `Const` / `Local` / `Global` / `If` / `Do` / `Call`. Its params
are dense frame slots **on `Heap::roots`** (so `arena_flip` relocates them — no new
root set); a param ref is a slot index (`Node::Local`), not an `env_get` scan; tail
calls reuse the frame (TCO). Anything else — deeper/local-capturing closures,
`let`/`letrec`/`match`/other special forms, multi-arity, patterns — **defers to the
tree-walker**, which is always correct.

**`dispatch` does the ADR-069 passthrough redirect.** A call whose callee is a
thin-wrapper prelude op (`(< n 2)` → `<`'s 2-arg arm `(%lt n 2)`; `+`→`%add`, etc.)
redirects straight to its inner `%native` via `call_native`, late-binding-safe
(re-resolves the live closure each call). This was **decisive** — see the finding.

**Numbers (release, bare top-level call, i7-14700HX):**

| bench | tree-walker | VM | speedup |
|---|---|---|---|
| `fib 32` (non-tail recursion) | 4.22 s | 2.15 s | **~2.0×** |
| `countdown 20M` (tail loop, TCO) | 13.76 s | 6.85 s | **~2.0×** |

**Verified:** 167 lib + 1035 in-language tests green under `BROOD_VM=1`; lib green
under `BROOD_VM=1 BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` (the R1 crux — frame slots
survive constant relocation); correctness under the full stress gate; VM off
(default) unchanged.

**The honest finding (why the redirect mattered).** The Stage-1 mechanism *alone*
was **~10 % slower** on fib: it ran fib's frame on the VM but **delegated every
primitive op back to the tree-walker via `eval::apply`** (a frame alloc + param
bind + body eval per `<`/`+`/`-`), and `eval::apply` even *misses* the passthrough
fast-path that `eval`'s own combination dispatch uses — so the VM did *more* work,
not less, while fib's 1-param frame gave almost no `env_get` saving to offset it.
The win only appeared once the VM reached primitives directly (the redirect). The
lesson: **a VM frame that delegates primitives can't win — the speedup comes from
keeping the hot loop off the tree-walker.** This is exactly what a bounded first
slice is for: prove the mechanism (the GC-rooting crux), then learn where the win
actually lives.

### Stage 2a — `let`/`letrec` (done)

Lexical scope is **flattened into the single activation frame**: a compile-time
`Scope` (name→flat-slot, sequential, shadowing; high-water = `nslots`) replaces the
param-only list. `Node::LetBind` evaluates each rhs and writes it into its frame
slot (`Heap::set_root_at`), in order, then runs the body; `vm_apply` pushes the args
then nil-fills to `nslots`. `let`/`let*` are sequential, `letrec` pre-allocates all
slots (init nil). Top-level forms get a frame too. Binding forms accept a list or
vector; pattern binders defer. **A `let`-body tail loop (30M iters): 25.5 s →
10.9 s (~2.3×)**; `let` slots on `Heap::roots` survive GC-stress.

### Stage 2b — multi-arity (done)

`compile_closure` compiles *every* exact-arity arm into a `CompiledClosure {
arms: Vec<Arc<CompiledArm>> }`; `dispatch` selects by argument count (exact arms
have distinct arities, matching eval's preference). Variadic arms / non-core
bodies defer.

### Stage 2c — local-capturing closures (done)

The unlock: closures created *inside* a function (callbacks, let-bound helpers)
that capture a local frame — so the VM engages in real programs, not just
top-level chains. Two halves, both behind `BROOD_VM`:

**Calling** a local-capturing closure. `dispatch` now runs a VM-eligible closure in
its **own** captured env, not the caller's: `genv = closure.env.unwrap_or(global)`.
A free var resolves by name through that env (`Node::Global(sym)` →
`env_get(genv, …)`), so the body compiles identically whether the capture is global
or local — and it's robust to the shipped-closure flatten/reorder
(`lexical-addressing-gotchas.md` #1), since nothing addresses by frame offset across
a process boundary. `Step::Tail` carries the callee env so a tail call can cross
into a closure with a *different* captured env.

**The two GC-critical parts, as solved:**
- *(a) caching.* A LOCAL closure's `ClosureId` index is recycled by the collector,
  so it can't key the compile cache (`docs` §2c warned of a stale-body miscompile).
  Keyed instead by the **body-code handle** — the closure's body forms live in the
  immovable RUNTIME code region (they're sub-forms of a promoted top-level fn), so
  `arms[0].body[0]`'s handle is stable and identifies the source. `VmCacheKey`
  namespaces the two spaces (`Runtime(id)` vs `LocalBody(handle)`). A LOCAL closure
  whose body was built from *movable* forms (conased by `eval`/quasiquote) has no
  stable key and would put movable handles in the cached `Node` tree, so it defers.
- *(b) env rooting (R1).* The captured env is a **movable LOCAL `EnvId`**. `vm_apply`
  roots it on `env_roots` (`root_env`, inline+free for the global sentinel) and
  re-reads the live handle via an `EnvRoot` after every collection — at the
  safepoint *and* inside nested calls (`exec_node` carries the `EnvRoot`, not a raw
  `EnvId`). Gated green under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 BROOD_VM=1`.

**Creating** a local-capturing closure on the VM (`Node::MakeClosure`). A `(fn …)`
inside a compiled body builds a closure over a **flat snapshot** of the enclosing
lexical environment: a fresh env frame (parent = global, so true globals + dynamics
stay live and late-bound) filled from a compile-time capture list — current-frame
lexicals as `Local` slot reads, names inherited from outer closures (found by
walking the closure's captured env at compile time, `Heap::env_chain_names`) as
`Global` reads. Brood's immutability makes a value snapshot equivalent to capturing
the env by reference. The `(fn …)`'s arms are parsed at run time by the shared
`eval::make_closure` (so multi-arity / `&optional` / docstrings all work). **Deferred
to the tree-walker:** a `(fn …)` capturing a not-yet-finalized `letrec` binder — a
value snapshot can't express letrec's recursive late-binding (mutual recursion),
so those run correctly on the interpreter, just unaccelerated.

**Numbers (release, Raptor Lake-S):** `fib 32` unchanged (4.22 s → 2.29 s, ~1.8×,
no closures in-loop, confirming no regression); **build-a-capturing-closure-and-call
it, 5M iters: 7.72 s → 4.72 s (~1.6×)**; **call a pre-built captured closure in a
20M-iter tail loop: 17.71 s → 9.06 s (~1.9×)**. Full Rust + in-language suites green
under `BROOD_VM=0/1`; the suite green under the full GC-stress gate.

**Source positions in VM diagnostics (done, 2026-05-31).** The compiled `Node` tree
discards source forms, so the VM used to tag an error with the top-level form's
position instead of the failing inner combination (6 `basic.rs` diagnostic tests —
pre-existing on the 2a/2b base, not a 2c regression). Fixed by capturing each
combination's `line:col` at **compile time** (`Node::Call { pos }`, from
`Heap::form_pos` while the LOCAL form is still live) and tagging an error with it on
the way out — innermost wins, exactly like the tree-walker's `or_form_pos`, and
collection-safe (`Pos` is plain data, not a movable handle). RUNTIME (promoted) body
forms carry no recorded position in *either* engine, so those stay untagged
identically. The full suite is now green under `BROOD_VM=0` **and** `=1`, removing
the last blocker on the Stage-3 default-on cutover.

---

## 1. Where the time goes today

`eval::eval` (`crates/lisp/src/eval/mod.rs`) is a `'tail:` trampoline. Every
combination re-pays, *per call*:

- a special-form `SymbolMap` lookup + enum match (`special_form`);
- an env-chain **name scan** — `Heap::env_get` walks frames doing
  `frame.vars.iter().rev().find(…)`, an assoc-list linear scan **per variable
  reference**;
- a fresh env-frame allocation per call (`new_env` → `bind_params`);
- repeated cons-spine walking to read operands off `rest`;
- the operand-stack rooting dance (`Heap::root`/`read_root`) around every nested
  `eval` — executed by *interpreting the tree* rather than by straight-line
  compiled code.

ADR-069 already banked the cheap dispatch wins (the thin-wrapper passthrough arm
cache + the global inline cache) and **explicitly deferred lexical addressing**
because, among other reasons, a `(depth,index)` reference as a runtime value would
bump the type-system compatibility contract (a new `Value` kind needs a `Tag` + a
`Ty` bit + GC/printer/wire support). The closure-compiling design below dissolves
that objection (§2.3).

---

## 2. Decision: closure-compiling over a lexically-addressed IR

**Compile each form once into a tree of compiled nodes** (a `Node` enum, the hot
cases inlined to avoid dynamic dispatch; cold cases may box a `dyn Fn`), executed
by a trampoline structurally identical to today's `'tail:` loop. Tail positions
compile to a `TailCall` outcome the trampoline loops on rather than recursing.

This is chosen over a **flat bytecode + central switch** VM for four reasons
specific to *this* codebase:

### 2.1 It solves the hardest constraint — GC rooting — for free

A moving collector relocates LOCAL handles; anything holding a live handle across
a collection must be an enumerable, relocatable root. A flat bytecode VM needs its
own **operand stack that is itself a GC root array** — a *second* structure
`Heap::arena_flip` must relocate, on top of `Heap::roots` / `Heap::env_roots`. That
forces a rewrite of the single most subtle already-correct code in the system (the
operand-stack rooting in `eval_arguments`).

Closure-compiling keeps the operand stack exactly as it is. A compiled node that
evaluates sub-expressions before a call pushes their results onto `Heap::roots`
using the **existing** `root`/`read_root`/`advance_root`/`root_at` API, so
`arena_flip` already relocates them in its `self.roots.iter_mut()` loop **with zero
new code**. The VM "value stack" *is* the operand stack.

**The crux, stated plainly:** the VM introduces **no new root set**. A call's frame
slots are allocated as a contiguous region of `Heap::roots` (the frame records its
base offset); locals are addressed as `root_at(base + index)`; the region is
truncated on return. `arena_flip`'s existing root-relocation walk covers every live
frame slot automatically.

### 2.2 It keeps the trampoline that already enforces the invariants

The `'tail:` loop and its per-iteration `tick()` / `deadline_exceeded()` /
`gc_due()` checks are the load-bearing invariant enforcers (TCO, green-process
preemption, the GC safepoint). The closure-compiling trampoline is structurally
identical — the loop body runs a compiled node instead of pattern-matching `expr`.
A bytecode VM would replace that loop with an opcode switch and re-derive every
invariant check at a new instruction boundary — a larger, riskier diff (and the
ADR-069 passthrough watchdog bug showed exactly how a dispatch path that bypasses
the loop top can escape the deadline).

### 2.3 Lexical addressing needs no new `Value` tag

The `(depth,index)` coordinate is baked into the **compiled node's** state, never
appearing as a runtime `Value`. No new `Tag`, no `Ty` bit, no printer/wire/message
changes. This is the cleanest way to land the deferred ADR-069 Inc-3 given the
constraint that flagged it.

### 2.4 Multi-arity, passthrough, macros already work on the closure structures

Compilation is **per `ClosureArm`** (`crates/lisp/src/core/value.rs`); the
arg-count dispatch (`Closure::select_arm`) is unchanged. The compiled body attaches
alongside (or replaces) `ClosureArm::body`.

**The cost vs bytecode:** slightly worse i-cache behaviour and somewhat higher
per-node call overhead than a tight bytecode loop. If profiling later shows
dispatch overhead dominating, the compiled-node enum can be lowered to bytecode as
an *internal* change with no semantic impact.

---

## 3. Lexical addressing — how it folds in

It slots into the existing compile pass, `eval::macros::compile`, which already
runs `macroexpand_all` then `resolve` (namespace qualification) before `eval`. Add
a **third sub-pass, `lex_resolve`**, after `resolve`:

- Walk the expanded + namespace-resolved form carrying a compile-time **scope
  stack** (`Vec<Vec<Symbol>>`) mirroring the runtime frame chain that
  `let`/`fn`/`letrec`/`bind_params` build. The binder-aware traversal already
  exists in spirit in `resolve` (e.g. `resolve_fn`/`resolve_let`/`collect_param_syms`,
  which thread a `locals: &[Symbol]`); reuse that structure.
- A reference resolved to `(depth,index)` becomes a `Node::Local`. A reference not
  found in any lexical level becomes `Node::Global(Symbol)` (resolved via the inline
  cache — §5).
- `letrec`'s frame is handled by assigning each name a dense slot index at compile
  time (the compiler controls slot layout), so the bind phase writes in place —
  cleaner than the current append-and-shadow.

Frame slots become a **dense `Vec<Value>` per activation** instead of the assoc-list
`EnvVars: SmallVec<[(Symbol,Value);4]>`. This is the single biggest win: it removes
the per-reference name scan *and* makes frame allocation a known-size
`Vec::with_capacity(n)`.

**Fallback (correctness floor):** references that can't be statically addressed —
forms reached via the `eval` builtin, quasiquote-built forms, lazy macro
re-expansion — compile to a `Node::SymbolRef` that does a runtime `env_get` exactly
as today. Lexical addressing is an **optimization layer, never a correctness
dependency**: an un-addressable ref degrades to the current scan, not to a bug.

---

## 4. Data structures (sketched against existing types)

```rust
// crates/lisp/src/eval/compile.rs (new) — the IR / compiled node.
// NOT a Value; never escapes to the language. No Tag, no Ty bit, no wire/printer.
enum Node {
    Const(Value),                       // literal / quote result
    Local { depth: u16, index: u16 },   // lexical-addressed local read
    Global(Symbol),                     // global read via inline cache (late-bound)
    SymbolRef(Symbol),                  // un-addressable fallback: runtime env_get
    If(Box<Node>, Box<Node>, Box<Node>),
    Do(Box<[Node]>),                    // all-but-last for effect, last in tail
    Call     { callee: Box<Node>, args: Box<[Node]> },  // non-tail
    TailCall { callee: Box<Node>, args: Box<[Node]> },  // tail position
    MakeClosure(Arc<CompiledTemplate>),
    LetBind { rhs: Box<[Node]>, body: Box<Node>, nslots: u16 },
    // def / defmacro / quote / quasiquote: defer to the interpreter (cold, top-level)
}

// Compiled counterpart of a ClosureArm. Stored alongside the existing arm.
struct CompiledArm { nslots: u16, body: Node }
```

**GC interaction of the `Node` tree itself.** For *global / promoted* closures
(`Closure.env == None` — the hot path), the body forms are already in
RUNTIME/PRELUDE and immovable, so the compiled `Node` tree holds only immovable
handles and needs no rooting (mirrors the `is_movable` / `Root::Stable` fast path).
For a *local* closure compiled on the fly, its `Node` tree is reachable from its
`ClosureId` and must be traced — handled by extending the closure tracer
(`push_value`/`flush_value` and `promote_closure`) to walk `CompiledArm` bodies
(Risk R1).

**Activation record.** Frame slots live as a region of `Heap::roots` (§2.1); a
frame records its base offset and its parent's base, so `depth` is resolved by
chaining base offsets. No raw `Vec<Value>` the collector can't see.

---

## 5. Hot-reload / late binding

`Node::Global(sym)` compiles a global reference to a lookup through
`Heap::global_lookup_cached` — the **same** version-stamped inline cache the
tree-walker uses. The compiler must **never** hard-bind a callee to a `ClosureId`:
the call site stores only the `Symbol`; each invocation re-resolves through the IC,
which re-reads `runtime.globals` when `runtime.version` has bumped. So a `def` after
compilation is seen on the next call — `live_redefinition` stays green. (The IC cost
is already measured negligible, and is *lower* in compiled code: the `Symbol` is in
the node rather than re-extracted from a cons head each time.)

Macros compile away strictly *after* `macroexpand_all`, so the IR never contains
macro calls; macro hot-reload keeps its current re-expand semantics. `def` /
`defmacro` / `quote` / `quasiquote` compile to a node that **defers to the existing
interpreter** — they're cold and top-level, which also shrinks the initial diff.

---

## 6. Invariant-preservation checklist

| # | Invariant | How the VM keeps it |
|---|---|---|
| 1 | **Proper tail calls** (`tail_calls_do_not_overflow`, 100k) | Tail positions compile to `TailCall`; the trampoline reuses the frame region instead of recursing — structurally identical to today's `continue 'tail`. |
| 2 | **Generational copying GC + operand-stack rooting** | Frame slots + intermediate args live on `Heap::roots`/`env_roots`; `arena_flip` relocates them in place. The safepoint stays at the trampoline loop top. **No new root set.** (The crux — §2.1.) |
| 3 | **Green-process preemption + deadline** | The trampoline keeps `tick()` + `deadline_exceeded()` at every tail iteration / call boundary; `receive` suspend/resume works because frame state lives in the (`Send`) `Heap`. The compiled-frame base offset is saved/restored across suspend like `GC_BLOCK`/`STACK_BASE`. |
| 4 | **Hot-reload via late binding** | `Node::Global` → version-stamped inline cache; never hard-bind a `ClosureId` (§5). |
| 5 | **Multi-arity dispatch** (ADR-047) | Compile per `CompiledArm`; `select_arm` selects by argc unchanged. |
| 6 | **Immutability** (ADR-026) | No mutation introduced; frame slots are write-once at bind, like `env_define`. The VM is mechanism only. |
| 7 | **Macros + macro hot-reload** | Compile after `macroexpand_all`; macro forms never reach the IR (§5). |
| 8 | **The language is unchanged** | Same reader, `Value`, primitives, `std/*.blsp`. The engine swap is invisible to the surface. |

---

## 7. Staged rollout — each stage shippable and test-green, behind `BROOD_VM`

**Stage 0 — Scaffolding + benchmark harness (small).** Add `eval/compile.rs` with
the `Node` enum and a `compile_form` that *currently* produces faithful IR-mirror
nodes executed by a thin interpreter calling existing helpers. Behind a `BROOD_VM`
env flag (default off). Lock in the ADR-069 benchmark set (fib(32), loop(3M),
collatz, reduce(1M)) so every later stage reports a delta. *Ship:* flag-off no-op;
flag-on at tree-walker parity; full suite green under both.

**Stage 1 — Lexical addressing (the down payment, medium).** Add `lex_resolve` to
the `compile` pipeline and `Node::Local{depth,index}` + dense frame slots on
`Heap::roots`. This is the deferred ADR-069 Inc-3, landed as compiled-closure
capture (no `Value` tag). Param/let-heavy bodies (fib/loop) show the biggest single
jump. *Correctness gate:* the entire std `.blsp` suite + `crates/lisp/tests/` green
under `BROOD_VM=1`, **and** under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 BROOD_VM=1`
together (to flush rooting bugs in the new frame regions). **This is the
first-milestone deliverable** — it de-risks the GC-rooting crux (R1) before any
larger compiler investment.

**Stage 2 — Full compiler + trampoline VM (large).** Compile
`if`/`do`/`let`/`letrec`/`fn`/call/tail-call into real nodes with the
closure-generating trampoline; `def`/`defmacro`/`quote`/`quasiquote` defer to interp.
Wire `MakeClosure` to store a `CompiledArm` per `ClosureArm`. Keep the tree-walker
as the fallback for any node that declines to compile. *Ship:* flag-gated; both
engines pass the suite. TCO-via-frame-reuse and the tick/deadline/safepoint checks
move into the VM trampoline here.

**Stage 3 — Cutover (medium).** Flip the default to the VM; `BROOD_VM=0` forces the
tree-walker. Extended soak: green-process fan-out (the scheduler-race scenarios),
`receive` suspend/resume across compiled frames, hot-reload (`live_redefinition`,
`defonce_preserves_state_across_reload`). *Ship:* VM is the engine; tree-walker
retained as a one-flag escape hatch for at least one release.

**Keeping both correct during transition — a differential test mode:** run every
test form through both engines and assert identical printed results, gated by an env
flag, run in CI for Stages 1–3. Cheap insurance against semantic drift, feasible
precisely because the language is unchanged (invariant #8).

---

## 8. Risk register

- **R1 — VM stack as GC roots (highest).** Mitigation: do **not** invent a new
  stack; allocate frame slots as regions of the existing `Heap::roots`, reusing
  `root`/`read_root`/`root_at`/`truncate_roots` so `arena_flip` relocates them with
  no new code. Residual risk: the compiled `Node` tree of a *local* closure holding
  movable `Value`s — extend the closure tracer (`push_value`/`flush_value`,
  `promote_closure`) to walk `CompiledArm` bodies. Gate with
  `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` under `BROOD_VM=1`.
- **R2 — TCO regression.** Gate: `tail_calls_do_not_overflow`; add tail-position
  tests through `if`/`do`/`let`/`apply` (the `apply` unfolding must reproduce as a
  compiled tail form). Differential mode catches stack-growth divergence via timeout.
- **R3 — Preemption/deadline coverage.** The passthrough watchdog bug showed a
  dispatch path bypassing the loop top can escape the deadline. The VM trampoline
  must `tick()` + check the deadline at *every* tail iteration and call boundary, not
  just outermost. Test: a compiled infinite tail loop must still be preempted and hit
  the deadline.
- **R4 — Hot-reload indirection cost.** Reuse the inline cache; never hard-bind. Cost
  already measured negligible. The risk is *forgetting* and binding a `ClosureId` for
  speed — forbid in review; `live_redefinition` is the gate.
- **R5 — Macro/compile boundary.** Compile strictly after `macroexpand_all`+`resolve`.
  Runtime fallbacks that re-expand must compile the re-entered form or defer to
  interp. Test: macro defined-and-used in one top-level form; macro hot-reload.
- **R6 — Fallback path correctness.** Keep the tree-walker fully functional and
  reachable (`BROOD_VM=0`) through Stage 3 and one release after. Any node that can't
  compile defers to interp per-form, so partial compilation is always safe.

---

## 9. First milestone (concrete)

> Add `crates/lisp/src/eval/compile.rs` with the `Node` enum and a `lex_resolve`
> pass wired into `eval::macros::compile` that produces `Node::Local{depth,index}`
> for lexically-bound references and `Node::Global`/`SymbolRef` otherwise. Execute
> `Node`s with a thin trampoline that allocates frame slots as `Heap::roots` regions
> and reads locals via `root_at`. Gate behind `BROOD_VM=1`.
>
> **Success criterion:** the full `crates/lisp/tests/` + std `.blsp` suites pass
> identically under `BROOD_VM=0/1` and under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1
> BROOD_VM=1`, and `fib(32)`/`loop(3M)` show a measurable speedup from eliminating
> the `env_get` name scan — proving the frame-slot-as-root mechanism (the crux)
> before any larger compiler investment.

This de-risks the single hardest constraint (R1) and lands the deferred ADR-069
Inc-3 win, while the engine is still 100% the tree-walker by default.

---

## References

ADR-076 (the decision record), ADR-069 (eval dispatch perf — passthrough + inline
cache; the lexical-addressing deferral this resolves), ADR-061 (collect at any eval
depth — the operand stack the VM frames reuse), ADR-054/055/072 (generational
copying GC — what `arena_flip` relocates), ADR-047 (multi-arity dispatch), ADR-022
(the macroexpand-all compile pass), ADR-026 (immutability), ADR-011 (defer power
features). Key files: `crates/lisp/src/eval/mod.rs` (trampoline, `eval_arguments`,
`bind_params`), `crates/lisp/src/core/heap.rs` (`arena_flip`, `roots`/`env_roots`,
`root_at`, `global_lookup_cached`, closure tracer), `crates/lisp/src/eval/macros.rs`
(`compile`/`resolve` — where `lex_resolve` slots in), `crates/lisp/src/core/value.rs`
(`Closure`/`ClosureArm`/`select_arm`), `crates/lisp/src/process/scheduler.rs`
(`tick`/`deadline_exceeded`, coroutine suspend/restore).
