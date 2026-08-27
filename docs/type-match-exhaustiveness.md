# Match exhaustiveness checking over literal-enum types (ADR-118, generalized in ADR-121)

> Status: **shipped, generalized to mixed-kind enums** (ADR-121). A `match`
> over a scrutinee whose declared type is a *pure* enumerable literal type —
> any combination of keyword/int/bool/string literals plus `nil`
> (`(or :ok :error :pending)`, `(or 200 404 500)`, `(or :ok 5)`,
> `(or :ok :error nil)`, `(or true false)`, `(or "GET" "POST")`) — is flagged
> when its clauses don't cover every member, unless a catch-all clause
> (`_`/bare bind) is present, which makes it trivially exhaustive. `case`
> doesn't exist in Brood (confirmed dead/vestigial — see Problem), so the
> scope is `match` only.

## Problem

Keyword-literal (ADR-105) and int-literal (ADR-117) types give `Ty` a precise
enumerable set, but nothing consumed it for the thing literal types are
usually *for*: catching a `match` that forgot an arm.

```lisp
(sig describe-status ((or :ok :error :pending) -> string))
(defn describe-status (status)
  (match status
    (:ok "all good")
    (:error "something broke")))
; forgot :pending — silently throws :match-error at runtime instead
```

Initial scoping assumed this needed a new `match`-clause parser: the checker
has no correct view of `match`'s real clause shape today (the one existing
"match-shape" code, `gradual_of_compound` in `check/walk.rs`, assumes a wrong
flat `pat1 body1 pat2 body2…` layout and is effectively dead for genuine
`match` forms — a pre-existing, minor bug, not fixed by this work). That
framing would have made this a 2-3 slice effort. A much smaller design was
found instead by reading the actual compiler.

Also worth noting up front: **`case` doesn't exist in Brood.**
`crates/lisp/src/eval/mod.rs` explicitly tells users "Brood has no
`case`/`condp` — use `match` (patterns) or `cond`." `kw::CASE` is referenced
only by the checker itself and is vestigial. So this feature is `match`-only,
correctly — there's nothing else to wire it into.

## The insight that made this small

`match` (`std/prelude.blsp`, `match*`/`match` macros, `%match-build-from`/
`%match-no-match`) compiles

```lisp
(match expr
  (p1 body1)
  (p2 body2))
```

to

```lisp
(let (m__N expr)
  (if (%eq m__N p1) (do body1)
    (if (%eq m__N p2) (do body2)
      (throw [:match-error 'match m__N '(p1 p2)]))))
```

Two facts about this shape make it a ready-made exhaustiveness signal with
**zero new parsing**:

1. **The throw only exists when there's no catch-all.** An irrefutable clause
   (a wildcard `_` or a bare bind, no `:when` guard — `%match-irrefutable?`)
   compiles to its body directly, with no further `if` — so `(throw
   [:match-error …])` is syntactically *absent* from the compiled tree
   whenever the match is already covered by a catch-all. Finding this exact
   shape at all is already "this match lacks a catch-all" — no separate
   detection needed.
2. **The full list of tried patterns is embedded in the throw as data.**
   `%match-no-match` is `(throw [:match-error (quote ~context) ~target (quote
   ~patterns)])` — `patterns` (every clause's raw pattern) is quoted literal
   data sitting in the 4th vector slot. No clause-boundary reconstruction
   needed — just read it off.

And critically: **the else-branch of a `(%eq m__N lit)` test doesn't narrow
`m__N`'s type.** `guard_assertion` marks an `%eq`-literal guard `then_only:
true` — being false proves nothing about the tag in general (`m ≠ "x"`
doesn't mean `m` isn't a string) — so `check_if`'s `else_ctx = ctx.clone()`
when the guard is `then_only`. This means `m__N`'s ctx type at the final
`throw` is **exactly its original declared type**, unchanged the whole way
down the chain (bound once at the enclosing `let` via `expr_ty`, per
`check_let`). No narrowing-through-the-chain logic needed either — just read
`target`'s current ctx type directly.

So the whole check reduces to: recognize `(throw [:match-error _ target
(quote patterns)])` in the **normal, already-macroexpanded** checker walk;
read `target`'s declared type via the existing `expr_ty`; if that type is a
*pure* literal-enum, diff it against the literal patterns actually tried;
report whatever's missing.

## Design

**`match_exhaustiveness_gap(heap, throw_arg, ctx) -> Option<String>`**
(`check/guards.rs`, alongside `guard_assertion`/`literal_eq_guard`):

1. `throw_arg` must be a 4-element vector `[:match-error, (quote context),
   target, (quote patterns)]` (checked structurally — element 0 is the
   keyword `:match-error`).
2. `target` must be a bare symbol; get its type via `expr_ty`.
3. **Purity check, generalized (ADR-121)** — the type must be *entirely*
   covered by the five enumerable kinds (keyword/int/bool/string literals,
   plus `nil` — a natural one-inhabitant singleton, no literal-set field
   needed for it): build `coverable = Ty::of(Keyword).union(Int).union(Bool)
   .union(Str).union(Nil)` once; `target_ty.is_subtype(&coverable)` — since
   `coverable` carries no refinements, `is_subtype`'s per-bit refinement
   checks never fire, so this reduces to a plain tag-subset check ("is every
   tag in `target_ty` one of these five"). This is what lets a *mixed-kind*
   enum (`(or :ok 5)`) or one with a trailing `nil` (`(or :ok :error nil)`)
   through — the original ADR-118 version required exactly one bit.
4. **Declared-set construction, over string labels** (sidesteps needing a
   combined Rust sum-type across 4 different payload types): for each of the
   five tags `target_ty` contains, render its members to canonical labels
   (`:name` / bare digits / `true`/`false` / `"content"` / `nil`) into one
   `BTreeSet<String>`. An unrefined occurrence of any of these tags (the
   literal-set accessor is `None` while the tag is present) still bails —
   can't enumerate an open set.
5. Unwrap `(quote patterns-list)` and walk the raw pattern list, rendering
   each with `render_literal_pattern` (the same label format). **Any
   non-literal pattern (destructuring, guarded bind, pin) bails to `None`**
   rather than half-reasoning about coverage.
6. `missing = declared_labels - tested_labels`. Non-empty → `"match: not
   exhaustive — missing {sorted, comma-joined labels}"`.

**Hook** — `check_into`'s existing generic call-handling in `check/walk.rs`
(the same spot the function-as-value lint and callback-arity check already
live): when the call head's spelling is `"throw"` and there's exactly one
vector argument, call `match_exhaustiveness_gap` and push a warning if `Some`.
No `SPECIAL_HEAD` entry, no new pass — this rides the *existing* generic call
path, which already recurses into every sub-form (including a `throw` call's
argument) in the normal macroexpanded walk.

**Why this doesn't touch `Ty`, doesn't need a new pass, and doesn't revisit
the reverted `of_value` extension from ADR-117 (which later shipped as Gap B0,
2026-07-10 — see [type-gating.md](type-gating.md)):** the scrutinee's type comes
from its *declared* `(sig …)` type via the exact same `ctx.declared_sig` →
`sig_params` → `expr_ty` pipeline every other check already uses. Nothing
about literal-in-*code* inference (`of_value`) is touched, so the
warning-message wording-churn problem that made the `of_value` extension get
reverted (7 unrelated tests broke on exact wording) doesn't recur here at
all — this check only ever fires on the one specific compiled `throw` shape.

## Deferred

- **Redundancy/unreachable-clause detection** — shipped separately, see
  [`type-match-redundancy.md`](type-match-redundancy.md) (ADR-122).
- **Matches mixing a literal pattern with a destructuring/guarded pattern** —
  bailed to `None` entirely; no coverage reasoning attempted.
- **`gradual_of_compound`'s pre-existing wrong-shape assumption** — noted, not
  fixed here (this feature doesn't go through it at all).

## Soundness

Verified the same two ways as every literal/arrow slice this session:
1. Six targeted tests in `crates/lisp/src/types/check.rs`
   (`match_exhaustiveness_flags_a_missing_keyword_arm`,
   `..._is_silent_when_every_arm_is_covered`,
   `..._is_silent_with_a_catch_all_clause`,
   `..._flags_a_missing_int_arm`,
   `..._declines_a_destructuring_clause_mixed_in`,
   `..._is_silent_for_a_non_literal_scrutinee_type`) plus a real end-to-end
   demonstration through the `brood` CLI (a 4-case scratch file: missing arm,
   full coverage, catch-all, int-literal version) producing exactly the
   expected 2 warnings.
2. `nest check` across all of `std/` + `tests/` with the hook disabled vs.
   enabled — byte-identical, zero new warnings (no non-exhaustive
   literal-enum matches exist in the corpus today).

ADR-121 (the mixed-kind generalization) added five more:
`match_exhaustiveness_flags_a_missing_arm_in_a_mixed_kind_enum` (`(or :ok 5)`),
`..._flags_a_missing_arm_with_a_trailing_nil` (`(or :ok :error nil)`),
`..._flags_a_missing_bool_arm`, `..._flags_a_missing_string_arm`, and
`..._is_silent_when_a_mixed_kind_enum_is_fully_covered` — plus the same
`nest check` corpus diff, again byte-identical.

## Tests

`crates/lisp/src/types/check.rs`: the eleven tests listed above, right after
`int_literal_return_type_flows_through_checker`.
