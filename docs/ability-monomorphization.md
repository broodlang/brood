# Ability dispatch monomorphization — design note

> Status: **Tier 1 is always on** (ADR-182; the `BROOD_MONO` opt-in is gone, ADR-398), at
> every compiled tier — the tree-walker (`BROOD_TIER=0`) never runs a rewritten call and is
> the differential reference. Both syntactic shapes are devirtualized at the `compile_node`
> seam (`eval/compile/inline.rs::mono_devirtualize`): a **literal** first arg (identity = its
> `type-of` kind) and a **direct record-constructor call** (identity = the record's baked
> `:module/name` id, proven structurally from the constructor's body). The rewritten head is
> `(%dispatch-proven op <op> *impls* '[A op] :id [ctor <ctor>])`: the impl is resolved through
> the epoch-stamped `%dispatch` cache (ADR-294), and the op's CURRENT binding is called
> instead whenever the op or the constructor has been rebound since compile time or the id
> has no impl — so the answer is always the op's own. Declined outright: an argument that can
> run user code, a head that is not the op `defability` emitted (a rebound name, a contract
> shim), an arity the op rejects, and any run with contracts armed. `BROOD_DEVIRT_DBG=1`
> traces each devirtualization. **Tier 2** (inferred-*variable* devirtualization — the
> checker→compiler channel) is deferred: it was only ever a plan (`BROOD_MONO=2` in
> perf-handoff Task 5, never built), and that task found no workload with a site. Picking it
> up: read this note, then the anchors in [§Anchors](#anchors).

## The problem

An `ability` op call (`(area shape)`) is ~4.1× the cost of a direct function call. The
op is a plain `defn` (emitted by `defability`, `std/prelude/abilities.blsp`) whose body *is*
the dispatch machinery:

```
(defn <op> (<args>)
  (let (id   (%identity-of <first-arg>)
        impl (%dispatch *impls* '[<A> <op>] id))
    (if impl
      (impl <args...>)
      (%no-impl '[<A> <op>] id))))
```

Per call the runtime does: `%identity-of` (the record's id, ADR-542, else `type-of`) →
`%dispatch` (`%impl-for` through the per-op inline cache: on a miss two `*impls*` map fetches,
op's method set, then by id, maybe `:default`) → the `if` → a direct call to the resolved fn.
When the target identity is **statically
known**, all of that — `identity-of`, both registry fetches, the branch — is redundant:
the call can go straight to the impl.

## Why this isn't trivial (the two hard constraints)

1. **The checker is fully decoupled from the compiler.** The type checker already
   proves the target identity at a call site — `arg_identity` (`types/check/protocol.rs`)
   for a literal or a direct constructor call, and `check_ability_call_inferred`
   for a *variable* whose inferred `Ty` is a record (via `ty_record_id`). `AbilityInfo`
   (`build_ability_info_visible`) precomputes which globals are op fns and which
   `[ability op id]` triples are covered. **But it throws all of this away as advisory
   warnings.** `nest check` is a separate pass (`std/tool/project-check.blsp`); nothing feeds checker types into the VM/compiler.
   There is **no existing channel** — one has to be built.

2. **Impl fns are anonymous.** The `impl` macro (`std/prelude/abilities.blsp`) registers
   each method as a bare `(fn …)` stored directly in `*impls*` — no global symbol names
   it. So a devirt target is the fn **value** (captured from the live registry at
   compile time), not a symbol we can emit a named call to. Capturing the value at
   compile time is what creates the late-binding trade-off (below).

## The pipeline (where a rewrite would live)

`parse → macros::compile (macroexpand-all + ns-resolve) → compile::run` (the closure-VM,
the default) or `eval::eval` (tree-walker, `BROOD_TIER=0`). Dispatch point: the
`eval::macros::compile` … `run_top_form` chain in `crates/lisp/src/lib.rs`.

The compiler lowers each macroexpanded form into a `Node` IR via `compile_node`
(`eval/compile/lower.rs`). An ability call is an ordinary `Node::Call` on
`Node::Global(op)`. There is already a Node→Node optimizer
seam — `eval/compile/inline.rs` (self/leaf inlining; `call_head_sym` extracts a call's resolved global symbol and is
directly reusable). **The rewrite lives here**: when the target is proven, the
`Node::Call{callee: Global(op), …}`'s head becomes the guarded `%dispatch-proven` call,
bypassing the op's `%identity-of`.

Recognizing an op fn in the compiler: read the `*abilities*` registry (`ability name → op
specs`) from the heap — the compiler has heap access — to know which global names are ability
ops; or match the op body's fingerprint shape. `*impls*` gives the id → fn value to inline.

## Late binding: why the rewrite proves an identity, never an impl

Impls are open and late — registered and retracted from any module, any time, and names are
rebound by hot reload. A rewrite that baked the resolved impl fn into the chunk would be
stale, and the window opens before the FIRST registration: a body is compiled before it
runs, so `(do (impl Show rec …) (render (rec 7)))` would capture the impl from before its own
`impl` line (ADR-294). So the rewrite proves only what syntax can prove — the receiver's
identity — and everything that can change at run time is re-checked on every call:

| What can change after compile time | What the rewritten call does |
|---|---|
| an impl registered / retracted | resolves through `%dispatch`, whose cache is epoch-stamped |
| the id loses its impl | calls the op, which raises its own `%no-impl` |
| the op name is rebound (`def` wins) | `%dispatch-proven` sees a different binding, calls it |
| the constructor is rebound (KI-90) | same, on the constructor's binding |

What cannot be re-checked cheaply is declined at compile time: an argument that can run user
code (it could register an impl after the rewrite resolved one), a head bound to anything
but the op `defability` emitted, an arity the op rejects, and armed contracts (the op checks
its declared return there). `crates/cli/tests/devirt_differential.rs` holds a case for each
row, at every tier, against the tree-walker.

## The two tiers

**Tier 1 — syntactic devirtualization (shipped, always on).**
The compiler proves the id *itself* from syntax when the first arg is a literal
(`(size 5)`) or a direct constructor call (`(area (circle 2))`) — mirroring
`arg_identity`'s syntactic cases, **no checker integration**. **Weakness:** does almost nothing for the case that actually costs —
a variable in a hot loop (`(map shapes area)`), where the arg isn't syntactically known.

> **Scoped differently since 2026-08-29 — see [dispatch-speculation.md](archive/dispatch-speculation.md).**
> The framing below ("build the checker→compiler channel") put an unsound data source in a
> load-bearing position, which is exactly how Tier 1 came to miscompile (ADR-294). Behind a
> runtime **guard**, a static fact is one prior among several and a wrong one costs a deopt,
> so the guard is what to build first and the channel is optional and last.

**Tier 2 — inferred-variable devirtualization. The real win, the real risk.** Where the
4.1× loop cost lives. Requires building the checker→compiler channel so a call on a
`sig`-typed / inferred variable can be devirtualized. This is the miscompile surface:
a wrong devirt silently calls the wrong impl. It's a materially bigger, riskier build
(getting inferred types to the compiler without duplicating the whole `Ctx`-threading
inference walk is the crux — running the checker's inference at compile time, or emitting
a checker-produced side-table the loader consumes). **Deferred:** perf-handoff Task 5
(2026-09-17) measured the ceiling and found no benchmark row or real program with a site.

## Validation

`crates/cli/tests/devirt_differential.rs`: the ability suite at tiers 1 and 2 against the
tree-walker, byte-identical; one fixture per row of the late-binding table above, at every
tier, against a literal expected answer; an argument that registers an impl; and a hot loop
that tiers to native. Every gate also asserts the rewrite FIRED (or, for armed contracts,
that it declined), since a differential in which nothing was rewritten is two identical
dynamic runs (ADR-280).

## Anchors

- Pipeline dispatch: `crates/lisp/src/lib.rs`
- VM overview + engine selection (`BROOD_TIER`): `crates/lisp/src/eval/compile.rs` module doc
- AST→IR call lowering + call-site IC: `eval/compile/lower.rs` (`compile_node`)
- Node→Node optimizer seam (rewrite home): `eval/compile/inline.rs` (`mono_devirtualize`,
  `ability_op_key`); the per-call guard: `%dispatch-proven` in `builtins/sequences.rs`
- Runtime call dispatch: `eval/compile/dispatch.rs` (`dispatch`)
- Ability op expansion (the dispatch to monomorphize), `%impl-for`, `*impls*` and the
  anonymous impl fns: `std/prelude/abilities.blsp` (`defability`, `impl`)
- Static proof already computed by the checker: `types/check/protocol.rs`
  (`arg_identity`, `check_ability_call_inferred`, `AbilityInfo`)
- Related: [protocol-dispatch-design.md](protocol-dispatch-design.md) (the facility this
  optimizes), [language.md §Polymorphism](language.md) (reference docs).
