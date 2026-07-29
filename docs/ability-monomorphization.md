# Ability dispatch monomorphization — design note

> Status: **Tier 1 shipped in full** (ADR-182, 2026-07-29), off by default behind
> `BROOD_MONO`. Both syntactic shapes are devirtualized at the `compile_node` seam
> (`eval/compile/inline.rs::mono_devirtualize`): a **literal** first arg (identity = its
> `type-of` kind) and a **direct record-constructor call** (identity = the record's baked
> `:module/name` id, proven via the `*record-ids*` registry `defrecord` populates — a
> same-named non-record fn is rejected). Every uncertainty declines the rewrite. Flag-off is
> provably inert (one cached bool); flag-on is byte-identical across all dispatch shapes and
> GC-safe under stress (the baked impl fn is a promoted RUNTIME handle). `BROOD_MONO_DBG=1`
> traces each devirtualization. **Tier 2** (inferred-*variable* devirtualization — the
> checker→compiler channel, the real hot-loop win and the real miscompile surface) remains
> deferred. Picking it up: read this note, then the anchors in [§Anchors](#anchors).

## The problem

An `ability` op call (`(area shape)`) is ~4.1× the cost of a direct function call. The
op is a plain `defn` (emitted by `defability`) whose body *is* the dispatch machinery
(`std/ability.blsp:202-210`):

```
(defn <op> (<args>)
  (let (id   (ability/identity-of <first-arg>)
        impl (ability/impl-for '[<A> <op>] id))
    (if impl
      (impl <args...>)
      (ability/no-impl '[<A> <op>] id))))
```

Per call the runtime does: `identity-of` (one `map?` + one `get :__id__`, else `type-of`)
→ `impl-for` (two `*impls*` map fetches: op's method set, then by id, maybe `:default`)
→ the `if` → a direct call to the resolved fn. When the target identity is **statically
known**, all of that — `identity-of`, both registry fetches, the branch — is redundant:
the call can go straight to the impl.

## Why this isn't trivial (the two hard constraints)

1. **The checker is fully decoupled from the compiler.** The type checker already
   proves the target identity at a call site — `arg_identity` (`types/check/protocol.rs:529`)
   for a literal or a direct constructor call, and `check_ability_call_inferred`
   (`protocol.rs:818`) for a *variable* whose inferred `Ty` is a record (via
   `ty_record_id`). `AbilityInfo` (`protocol.rs:559`, `build_ability_info`) precomputes
   which globals are op fns and which `[ability op id]` triples are covered. **But it
   throws all of this away as advisory warnings.** `nest check` is a separate pass
   (`crates/nest/src/main.rs:934`); nothing feeds checker types into the VM/compiler.
   There is **no existing channel** — one has to be built.

2. **Impl fns are anonymous.** The `impl` macro (`std/ability.blsp:214-238`) registers
   each method as a bare `(fn …)` stored directly in `*impls*` — no global symbol names
   it. So a devirt target is the fn **value** (captured from the live registry at
   compile time), not a symbol we can emit a named call to. Capturing the value at
   compile time is what creates the late-binding trade-off (below).

## The pipeline (where a rewrite would live)

`parse → macros::compile (macroexpand-all + ns-resolve) → compile::run` (the closure-VM,
`BROOD_VM` default on) or `eval::eval` (tree-walker, `BROOD_VM=0`). Dispatch point:
`crates/lisp/src/lib.rs:463-483`.

The compiler lowers each macroexpanded form into a `Node` IR via `compile_node`
(`eval/compile/mod.rs:736`). An ability call is an ordinary `Node::Call` on
`Node::Global(op)` (`compile/mod.rs:984-1015`). There is already a Node→Node optimizer
seam — `eval/compile/inline.rs` (linear-map rewrite, self/leaf inlining;
`call_head_sym` at `inline.rs:26` extracts a call's resolved global symbol and is
directly reusable). **The rewrite belongs here**: when the flag is on and the target is
proven, rewrite `Node::Call{callee: Global(op), …}` into a direct call to the resolved
impl, bypassing the `identity-of`/`impl-for` body.

Recognizing an op fn in the compiler: read the `*abilities*` registry
(`std/ability.blsp:43`, `ability name → op specs`) from the heap — the compiler has heap
access — to know which global names are ability ops; or match the op body's fingerprint
shape. `*impls*` (`ability.blsp:46`) gives the id → fn value to inline.

## The late-binding trade-off (why it's flag-gated)

Impls are open and late — re-registered from any module, any time (drivers-as-values,
hot reload). A compile-time devirt captures the impl fn **value** at compile time; if
that id's impl is later re-registered, the devirtualized call is **stale** (still calls
the old fn). This is the coherence-vs-late-binding tension.

Gating behind `BROOD_MONO` **off by default** resolves it cleanly: default builds keep
100% dynamic semantics (full late binding); opt-in trades that for speed, exactly like
`-O2` assuming no UB. Document the caveat. (A partial rewrite that keeps `impl-for` live
— skipping only `identity-of` — would preserve late binding but wins little; not worth a
separate tier.)

## The two tiers (the open scope decision)

**Tier 1 — syntactic devirtualization. Low risk, self-contained, little hot-loop payoff.**
The compiler proves the id *itself* from syntax when the first arg is a literal
(`(size 5)`) or a direct constructor call (`(area (circle 2))`) — mirroring
`arg_identity`'s syntactic cases, **no checker integration**. Rewrite those to a direct
impl call. Lands the `BROOD_MONO` flag plumbing, op-recognition, the rewrite, and the
validation harness. **Weakness:** does almost nothing for the case that actually costs —
a variable in a hot loop (`(map area shapes)`), where the arg isn't syntactically known.

**Tier 2 — inferred-variable devirtualization. The real win, the real risk.** Where the
4.1× loop cost lives. Requires building the checker→compiler channel so a call on a
`sig`-typed / inferred variable can be devirtualized. This is the miscompile surface:
a wrong devirt silently calls the wrong impl. The off-by-default flag contains it, but
it's a materially bigger, riskier build (getting inferred types to the compiler without
duplicating the whole `Ctx`-threading inference walk is the crux — running the checker's
inference at compile time, or emitting a checker-produced side-table the loader consumes).

**Recommendation (as of writing):** build Tier 1 first as a vertical slice (proves the
flag + mechanism + validation end-to-end, matching how the rest of brood was built),
then decide on Tier 2 once the mechanism is proven — with eyes open that Tier 1 alone
won't move hot-loop benchmarks. *Decision pending — the user is returning to this.*

## Flag pattern

Off-by-default, cached, Rust-side — copy `hof_fast_enabled` (`eval/compile/mod.rs:1710`):

```rust
fn mono_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_MONO").is_some())
}
```

Closest precedent for a *type-driven* off-by-default gate: `BROOD_CONTRACTS`
(`std/prelude.blsp:262-296`), which gates `(sig …)` runtime enforcement.

## Validation plan

- Flag **off**: full suites green, zero behaviour change (the pass is inert). This is the
  safety guarantee — default builds are untouched.
- Flag **on**: full suites green (correctness of the rewrite), plus a microbenchmark
  showing the dispatch-cost win on the covered call shapes. Run the whole broodlang
  fleet (hatch 750, brood-chat, hatch-demo, store-postgres, willem, mylife, mitch) with
  the flag on to catch any real-program miscompile.
- A targeted test: an ability with a `:default` and a specific impl, devirtualized call
  vs. dynamic call must return identical results; a re-registered impl under the flag is
  the documented stale case (assert the caveat, don't "fix" it).

## Anchors

- Pipeline dispatch: `crates/lisp/src/lib.rs:463-483`
- VM overview + `BROOD_VM`: `crates/lisp/src/eval/compile/mod.rs:1-24`, `57-77`
- AST→IR call lowering + call-site IC: `eval/compile/mod.rs:736`, `984-1015`
- Node→Node optimizer seam (rewrite home): `eval/compile/inline.rs:1-33`
- Runtime call dispatch: `eval/compile/dispatch.rs:185`
- Ability op expansion (the dispatch to monomorphize): `std/ability.blsp:171-212`
- `impl-for` / `*impls*` / anonymous impl fns: `std/ability.blsp:124-128`, `214-238`
- Static proof already computed by the checker: `types/check/protocol.rs:529`
  (`arg_identity`), `818-838` (`check_ability_call_inferred`), `559-610` (`AbilityInfo`)
- Flag patterns: `eval/compile/mod.rs:1710` (`BROOD_NO_HOF`), `std/prelude.blsp:262-296`
  (`BROOD_CONTRACTS`)
- Related: [protocol-dispatch-design.md](protocol-dispatch-design.md) (the facility this
  optimizes), [language.md §Polymorphism](language.md) (reference docs).
