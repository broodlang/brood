# Match redundancy / unreachable-clause detection (ADR-122)

> Status: **shipped**. A `match` clause (or a hand-written `if`/`%eq` chain)
> whose literal test duplicates one already tried earlier in the same chain
> is flagged as unreachable — the earlier occurrence always wins, so the
> later one is dead code. Purely structural, not `match`-specific: this fires
> on any same-symbol `%eq`-literal `if`-chain regardless of where it came
> from.

## Problem

A different, independent problem from exhaustiveness (ADR-118/120):
exhaustiveness asks "did every declared value get a clause?"; redundancy asks
"does any clause duplicate one already handled?" — e.g.

```lisp
(match status
  (:ok "good")
  (:ok "also good?"))   ; dead — the first :ok clause always wins
```

Unlike exhaustiveness, this needs **no scrutinee `Ty` at all** — it's a purely
syntactic property of the compiled clause chain (does literal `X` appear
twice on the same variable before a catch-all/end).

## Design

`match` compiles clauses to nested `(if (%eq m__N lit) then else)` — whichever
test succeeds *first* wins, so a later test against the *same* literal on the
*same* variable can never run. Detecting this needs no new parsing beyond what
the exhaustiveness work already established: walking the compiled `if`/`%eq`
chain directly (`check_if`, `crates/lisp/src/types/check/walk.rs`), reusing
the exact point that function already recognizes a literal `%eq` guard.

**New in `check/guards.rs`:**
- `literal_eq_test_raw(heap, test) -> Option<(Symbol, Value)>` — like
  `literal_eq_guard` (which `guard_assertion` already uses), but returns the
  **raw literal `Value`**, not a `Ty`. Redundancy needs exact value equality
  ("is this the same literal"), not a tag — `guard_assertion` already
  collapsed the literal to `Ty::of_value` for its own purposes, discarding the
  concrete value.
- `literal_values_equal(heap, a, b) -> bool` — `Value` has no `PartialEq`
  derive anywhere in the codebase, so this is a small manual match over the
  literal kinds. Includes `Value::Float` (unlike the enumerable `Ty` literal
  kinds, which stop at keyword/int/bool/string — a `BTreeSet<f64>` has the
  NaN problem, but comparing two literal *tokens* for "did the source write
  the same thing twice" is a simple `==`, no set/`Ord` involved).
- `find_redundant_clause(heap, form, sym, lit) -> Option<Value>` — scans
  *forward* from `form` (an `else`-branch continuation): as long as `form` is
  itself `(if (%eq sym lit2) then2 else2)` for the *same* `sym`, compare
  `lit2` to `lit`; a match returns that `if` form (the duplicate clause).
  Stops silently the moment `form` isn't itself another same-symbol `%eq`-if
  (a catch-all body, a `%match-no-match` throw, or a divergent hand-written
  `if`) — nothing more to reason about there.
- `render_literal_pattern` (already built for exhaustiveness) renders the
  duplicate literal for the warning message.

**Hook** — `check_if` (`walk.rs`), right where it already computes `test`/
`else_form`: if `literal_eq_test_raw(test)` succeeds, call
`find_redundant_clause(else_form, sym, lit)`; on `Some`, push `"match:
unreachable clause — {label} is already handled above"` positioned at the
duplicate `if`.

**Genuinely general, not `match`-specific.** Because the check is purely
structural on the compiled `if`/`%eq` shape, it fires on a hand-written chain
too:

```lisp
(if (%eq x 5)
  :a
  (if (%eq x 5)   ; flagged — same as if this came from a match
    :b
    :c))
```

This isn't scope creep — it's what "no `match`-specific hook, just recognize
the compiled shape" gets for free, the same way ADR-118's exhaustiveness check
is really about the `(throw [:match-error …])` shape rather than `match`
itself.

**No double-reporting, no missed duplicates in a longer run.** `check_if`
already recurses into every nested `if` for its own purposes (the dead-clause
lint, narrowing). Each level's own `find_redundant_clause` call only scans
*downstream* from itself, so a chain testing `p1, p2, p1, p2` produces exactly
two warnings (the second `p1` and the second `p2`, each found by the level
that established the *first* occurrence) — never zero, never four.

## A real corpus finding, not a bug

Verifying this against the whole `std/` + `tests/` corpus surfaced exactly one
new warning: `tests/pattern_matching_test.blsp`'s test named **"first matching
clause wins"** —

```lisp
(test "first matching clause wins"
  (assert= (match 1 (1 :first) (1 :second) (_ :z)) :first))
```

This is a **true positive**: the test deliberately writes a duplicate-literal
match to verify the runtime semantics ("the first clause wins", i.e. the
`:second` clause never executes) — and the new lint correctly identifies that
`(1 :second)` clause as genuinely unreachable. The test still passes (this is
advisory, never gating); it now also carries an accurate static warning it
didn't have before. Left as-is — rewriting a pre-existing, working test to
dodge a *correct* new warning isn't warranted, and the file wasn't otherwise
touched by this session's work.

## Deferred

Nothing significant — this is a small, self-contained, purely structural
check with no obvious follow-on the way exhaustiveness had (mixed-kind
enums). A possible future extension: detecting redundancy across *disjoint*
literal sets a guard has already excluded (e.g. `(if (%eq x 1) a (if (> x 1)
b (if (%eq x 1) c d)))` — the third test is *also* unreachable, but for a
different reason the same-symbol-%eq-chain scan doesn't see). Not pursued;
no evidence it's a real need.

## Soundness

Four targeted tests in `crates/lisp/src/types/check.rs`:
`match_redundancy_flags_an_adjacent_duplicate_clause`,
`..._flags_a_non_adjacent_duplicate_clause` (the duplicate isn't the very next
clause), `..._is_silent_with_no_duplicates`, and
`..._fires_on_a_hand_written_eq_chain_too` (confirms genericity). Given this
touches `check_if` — a very hot, heavily-shared function every `if` in every
program passes through — verified with extra rigor: the full 210-test
baseline stayed green after wiring the hook in, and the `nest check` corpus
diff (disabled vs. enabled) surfaced exactly the one true-positive finding
above and nothing else.

## Tests

`crates/lisp/src/types/check.rs`: the four tests listed above, right after
`match_exhaustiveness_is_silent_for_a_non_literal_scrutinee_type`.
