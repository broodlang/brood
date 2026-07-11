# Intersection of arrows — overloaded/multi-clause return types

> Status: **shipped, including cross-module resolution** (ADR-116). `(and
> (int -> int) (bool -> bool))` in a `(sig …)` annotation now builds a real
> overload instead of silently widening to "any function"; a call site
> resolves the return type of the arm whose domain the argument's static
> type provably matches — whether the call is in the *same* file as the
> declaration or a *different* module entirely (`:use`/qualified). No new
> grammar — reuses the already-shipped `(and …)` intersection syntax.

## Problem

`ROADMAP.md` flagged this as **"the single biggest expressiveness gap"**
in the type checker: a function whose return type depends on which "clause"
matched its argument — the classic overloaded-function shape:

```lisp
(sig f (and (int -> int) (bool -> bool)))
(defn f (x) (if (int? x) (inc x) (not x)))

(string-length (f 1))   ; f's result on an int arg is int — should warn
(f true)                 ; f's result on a bool arg is bool — fine
```

`(and (int -> int) (bool -> bool))` already **parsed** before this change
(`annot.rs`'s existing `(and …)` grammar, folding pairwise through
`Ty::intersect`) but `intersect`'s generic `merge_intersect` helper treated
two *distinct* known `Sig`s as an unresolvable conflict and widened to
`arrow: None` — total information loss. The declared sig was silently
useless for any function overloaded on domain.

## Why no new grammar was needed

Function intersection types are the standard type-theory encoding of
overloading: `f : (A→B) ∧ (C→D)` means exactly "call `f` with an `A`, get a
`B`; call it with a `C`, get a `D`." That's the same `(and …)` conjunctive-
type feature `docs/type-intersections.md` already shipped — a value
satisfying every constituent type simultaneously — just applied to two
*distinct* arrows instead of one arrow plus a flat tag (`(and fn (int ->
int))`, that doc's own example). So the fix lives entirely in `Ty::intersect`'s
arrow-specific handling plus a new checker consumer that resolves a call's
return type per candidate arm — not a new keyword, and it doesn't touch
`(map K V)`/`(vector E)`/`elem`/`fields` intersect logic at all.

## Representation (`types/mod.rs`)

`Ty` gained an `overload: Option<Arc<Vec<Sig>>>` refinement, tagged `FN_BITS`
like `arrow`. It only ever holds **2+ distinct** signatures — a single one
always collapses back to `arrow`, so every existing single-arrow consumer
(the callback-arity check in `check/walk.rs`, `Sig::is_subtype`, etc.) is
untouched for the common case.

```rust
overload: Option<Arc<Vec<Sig>>>,
```

**`Ty::intersect`'s new arrow logic** (`intersect_arrows`, replacing a bare
`merge_intersect(&self.arrow, &other.arrow)` call): extract each side's
*candidate list* — `overload`'s list if present, `[arrow]` as a singleton if
present, else `[]` ("any function", no info):

```rust
fn candidate_sigs(ty: &Ty) -> Vec<Sig> {
    if let Some(sigs) = &ty.overload {
        sigs.as_ref().clone()
    } else if let Some(sig) = &ty.arrow {
        vec![sig.as_ref().clone()]
    } else {
        Vec::new()
    }
}

fn intersect_arrows(a: &Ty, b: &Ty) -> (Option<Arc<Sig>>, Option<Arc<Vec<Sig>>>) {
    let sa = candidate_sigs(a);
    let sb = candidate_sigs(b);
    if sa.is_empty() { return (b.arrow.clone(), b.overload.clone()); }
    if sb.is_empty() { return (a.arrow.clone(), a.overload.clone()); }
    let mut combined = sa;
    for sig in sb {
        if !combined.contains(&sig) { combined.push(sig); }
    }
    if combined.len() == 1 {
        (Some(Arc::new(combined.into_iter().next().unwrap())), None)
    } else {
        (None, Some(Arc::new(combined)))
    }
}
```

A side with zero candidates ("any function") leaves the other's untouched —
reproducing today's exact behavior for `(and fn (int -> int))` and for two
*identical* arrows (which dedup to length 1 and collapse straight back to
`arrow`). Two *distinct* arrows now build a genuine 2-element overload
instead of widening to nothing.

**`Ty::union` needed no new logic.** The existing generic `merge_union`
helper already treats any `Option<Arc<T: PartialEq>>` as an opaque
equality-comparable blob; threading `overload` through it exactly like
`map_kv`/`fields` were threaded in the record-types work (ADR-115) gives
correct "widen unless identical" union semantics for free.

**`negate`** — one-line addition, same pattern as every other refinement:
keep the `FN_BITS` tag if `overload.is_some()`.

**`is_subtype`** — generalizes the old single-arrow check rather than adding
a parallel branch: `other`'s candidate list is `[the one arrow]` when it
carries no overload, so the same code path reproduces the old exact-`Sig`
check unchanged. For a genuine overload, `self` must satisfy *every*
signature `other` requires — for each, at least one of `self`'s own
candidates must be a `Sig::is_subtype` of it (self may carry extra arms
beyond what's required — width-like). **Sound but not complete**
(`docs/types.md` contract #5), the same conservative shape as
`record_fields_is_subtype` (ADR-115): a missed subtype relation is fine, a
false one is not.

**`is_disjoint`** — untouched, tags-only, per the whole lattice's convention.

## Declaration storage & call-site resolution (`check/annot.rs`, `ctx.rs`, `guards.rs`)

Mirrors the existing `SigWithVars` parallel-path pattern (`docs/type-variables.md`'s
type variables): `parse_sig_decl` (single arrow) already can't produce a `Sig` for a
genuine overload (`Ty::as_arrow()` is `None`), so a new
`parse_sig_decl_overload` reads `.overload_sigs()` instead and is recorded in
a new `Ctx::declared_overloads: HashMap<Symbol, Vec<Sig>>`, populated
alongside `declared`/`declared_vars` in `check.rs`'s file-scan pass.

`expr_ty`'s call-form handling (`guards.rs`) checks `ctx.declared_overload(s)`
at the same priority as `declared_sig_with_vars`/`declared_sig` (a user's
explicit declaration wins over curated/inferred fallbacks), resolving via a
new `resolve_overload_ret`:

```rust
pub(super) fn resolve_overload_ret(sigs: &[Sig], arg_tys: &[Option<Ty>]) -> Ty {
    let mut matched: Option<Ty> = None;
    for sig in sigs {
        let compatible = arg_tys.iter().enumerate().all(|(i, arg_ty)| {
            let Some(arg_ty) = arg_ty else { return true }; // unknown never rules out
            match sig.param(i) {
                Some(param_ty) => arg_ty.is_subtype(&param_ty),
                None => false,
            }
        });
        if compatible {
            matched = Some(match matched {
                Some(acc) => acc.union(sig.ret.clone()),
                None => sig.ret.clone(),
            });
        }
    }
    matched.unwrap_or(Ty::ANY)
}
```

Exactly one matching arm → the precise per-clause return type. Several match
(ambiguous, e.g. some args unknown) → the union of their return types — still
a sound superset, still an improvement over the old total-information-loss
fallback. Zero match → widens to `Ty::ANY` rather than ever fabricating a
return type for a call that fits no declared arm.

### Cross-module resolution

The `Ctx::declared_overloads` path above only covers a call *in the same
file* as the declaration — `check_file` allocates a fresh `Ctx` per file, so
nothing in it survives across files. But a plain single-arrow `(sig …)`
*does* already work cross-module, via a wholly separate mechanism: the `sig`
macro expands to `(%register-sig 'f '<type-expr>)`, which runs at **load
time** and writes the raw, unparsed type-expression form into a shared
heap-level store (`RuntimeCode::declared_sigs`); `declared_heap_sig`
(`check/sigs.rs`) reads it back and calls `.as_arrow()`. Since
`project--ensure-loaded` loads every project file before any file is
checked, that store is fully populated project-wide by the time checking
starts — so this path is what makes a declared sig visible from a *different*
module in the first place.

The first cut of this feature only extended the `Ctx` path, so a genuine
overload was invisible outside its declaring file — worse than a plain
single-arrow sig, which already crossed files fine. The fix needed **no
storage change**: the heap store already holds the opaque raw form
regardless of what it represents. It only needed a reader:

```rust
/// The overload counterpart of `declared_heap_sig` — extracts
/// `.overload_sigs()` instead of `.as_arrow()`, so a genuine 2+-arm overload
/// (which has `arrow: None`) isn't silently discarded.
pub(super) fn declared_heap_overload(heap: &Heap, sym: Symbol) -> Option<Vec<Sig>> {
    let type_value = heap.declared_sig_value(sym)?;
    annot::parse_type(heap, type_value)?.overload_sigs().cloned()
}
```

wired into the same three sites that already fall back to `sig_of`/
`declared_heap_sig` for return-type resolution: `expr_ty`'s call-form
handling (the final fallback, after `ctx.declared_overload` and the
sequence-aware rules all miss) and `callback_ret` (a named global function
passed as a HOF callback, e.g. `(map f xs)`). The one `sig_of` call site
that *wasn't* touched — `check/walk.rs`'s argument/arity-checking loop — is
intentionally out of scope; see Deferred below.

Verified with a Rust test that actually *evaluates* a declaration (so
`%register-sig` really fires) before typing a call against a **fresh, empty
`Ctx`** — simulating a second module with zero local knowledge of the first
(`overload_resolves_cross_module_via_the_heap_store`, `types/check.rs`) —
and, end-to-end, with a real two-file `nest new` project (`hello.blsp`
declaring an overloaded `clamp`, `main.blsp` calling `hello/clamp` via
`(:use hello)`): `nest check` correctly flagged a genuine mismatch
(`(string-length (clamp 0 10 5))`) and stayed silent on the correct call
(`(+ 1 (clamp 0 10 5))`).

## Argument check (shipped — the second hook)

**Flagging an argument that fails every overload arm** — e.g. `(f "oops")`
against `(and (int -> int) (bool -> bool))` — is now implemented, the second
hook in `check/walk.rs`'s `check_into` the original slice deferred. When a
callee has no single `sig` but *does* have a declared overload
(`ctx.declared_overload` or `declared_heap_overload`), `overload_arg_mismatch`
runs: it flags the call only when **every arity-relevant arm is ruled out**,
where an arm is ruled out only if some *known* argument is provably **disjoint**
from that arm's parameter. This mirrors the single-`sig` loop's discipline, so
it's false-positive-free by construction:

- an **unknown** argument type (or a `NEVER` unreachable-branch type) never rules
  an arm out — matching the single-sig loop's `is_never` skip;
- **disjointness**, not subtyping, is the test (an arg merely *wider* than a
  param never triggers a warning);
- an arm whose **arity** can't accept the call is skipped, so a pure arity
  mismatch is left to the dedicated arity check rather than double-reported; if
  *no* arm has a fitting arity the whole check defers.

Message: `f: no overload clause accepts these arguments`. Verified zero new
warnings across the whole `std/` + `tests/` corpus.

## Soundness

Every new piece has a targeted unit test in `crates/lisp/src/types/mod.rs`
(`intersect_of_two_distinct_arrows_builds_an_overload`,
`intersect_of_identical_arrows_collapses_to_a_single_arrow`,
`intersect_with_any_function_keeps_the_others_candidates_unchanged`,
`intersect_accumulates_three_distinct_arrows`,
`overload_renders_each_arm_joined_by_and`,
`overload_subtyping_is_conservative_but_sound`,
`overload_is_disjoint_only_on_tags_like_every_other_refinement`) and a
checker-level test in `crates/lisp/src/types/check.rs`
(`overload_refinement_flows_through_checker`) covering the exact-match,
alternate-arm-match, mismatched-sink, and unknown-arg-widens cases; plus
`overload_resolves_cross_module_via_the_heap_store`, which actually
*evaluates* a declaration before typing a call against a fresh `Ctx` to prove
the heap-store path independently of same-file `Ctx` state.

Since this changes the semantics of every `(and …)` annotation that happens
to combine two distinct arrows and every call to a function with a declared
overload, it was verified the same way the record-literal inference was
(ADR-115): `nest check` run across the whole `std/` + `tests/` corpus with
the new `intersect_arrows` logic and the new `expr_ty` branch disabled vs.
enabled produced a byte-identical warning list — zero new warnings anywhere
in the existing codebase (there are, unsurprisingly, no overloaded-arrow
declarations in the corpus yet, but the diff proves the change is inert for
everything that doesn't use it).

## Tests

`crates/lisp/src/types/mod.rs`: the 7 unit tests listed above, under the
"overloaded arrows (intersection of arrows) — ADR-116" section, right after
the existing arrow tests. `crates/lisp/src/types/check.rs`:
`overload_refinement_flows_through_checker`, right after
`record_field_refinement_flows_through_checker`.
