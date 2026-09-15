# Declared properties — `:pure` and `:total` (ADR-351)

> Status: **shipped** 2026-09-15. A `(sig name … :pure :total)` declares what the checker
> holds a function to; `nest check` reports a body that breaks the promise. The two
> properties Idris gives a function — `total` and, through its effect types, purity — as
> declarations over the advisory checker this language already has.

## The declaration

Property keywords follow the type in a `sig`, or stand alone:

```lisp
(sig area (number -> number) :pure)
(sig walk :total)                       ; no type: the parameters stay caller-derived
(sig join (& string -> string) :pure :total)
```

The `sig` macro registers the type (`%register-sig`) and the properties
(`%register-sig-props`) beside each other in the declared-sig store, as `(%sig T prop…)` —
so a loaded module's declaration is read back cross-module like its type is, and an image
carries it. A properties-only sig registers no type and installs no runtime contract. In
the file being checked, `Ctx::declared_props` holds them; `properties::has_prop` asks both.

## `:pure` — no effect

The function performs no effect: no message or process operation, no I/O, no `Table`
write or read (a table is the one mutable value — reading one is reading the world), no
clock, no environment, no randomness, no fresh symbol, no registry write. Checked by
walking the expanded body for an effectful head, **through the functions it calls**: a
same-file function by its form, a loaded one by its closure's arms, a `:pure`-declared
callee trusted (it is checked at its own definition), a recursive function assumed pure
while its own body is walked. A `fn` literal handed to a call is walked (a callback runs);
one returned or bound is not (a pure function may build an effectful closure without
running it). The finding names the chain:

```
quad is declared :pure but performs an effect: calls twice, which performs an effect:
calls log-it, which performs an effect: (io/puts …)
```

The effectful heads are a **deny-list** (`properties.rs`: the guard lint's names, plus
`receive`, `gensym`, the clock, and whole namespaces — `io/`, `file/`, `os/`, `tcp/`,
`table/`, `proc/`, `timer/`, `rand/`, `reflect/`, …). That is the sound direction for a
checker that promises never to flag a valid program: a reported effect is one the body
reaches; what the list does not name is missed, not invented.

**The consumer that needed no declaration.** `ui-memo` (ADR-336) caches a view fragment
while its `deps` are equal, on the assumption the thunk is pure — an effect there runs on
some turns and not others. Every `(ui-memo key deps thunk)` call is checked: a `fn`
literal thunk by its body, a named one through `effect_of_global`. `tests/ui_test.blsp`
counts its recomputations by sending from the thunk on purpose, and opts out with
`(check-allow :pure …)`.

## `:total` — terminates and covers its cases

**Termination.** Every self-call hands some one parameter a structural decrease of itself,
read in the branch scope the call sits in (`sigs::self_call_sites`, which types a site in
its `if` branch):

- `(rest p)` / `(but-last p)` of a `p` the branch knows is non-empty — length at least 1
  and no `nil` (`(nil? xs)`'s else, `(not (empty? xs))`);
- `(- p k)` / `(dec p)` / `(+ p -k)` of a `p` the branch bounds below — `(<= n 0)`'s else
  says `n ≥ 1`; `(= n 0)`'s else says only `n ≠ 0`, and is reported;
- `(+ p k)` / `(inc p)` of a `p` the branch bounds above — by a literal (`(< i 10)`), or by
  the count of an immutable collection (`(< i (count xs))`, or `(>= i n)`'s else with `n`
  a count alias — including one the callers established, ADR-350's parameter relation).
  `(count xs) - i` is the measure; `xs` never changes.

A function with no self-call passes this half. Mutual recursion is not examined.

**Coverage.** Reported by the walk (`walk.rs`, at the `(throw [:match-error …])` a
catch-all-less `match` lowers to) under `Ctx::total_fn`: a `match` failure the checker
cannot prove unreachable — a scrutinee that is not a closed literal type, or a pattern
that is not a literal (`guards::match_coverage` answers `Unknown`) — is a case the
declaration promised and nothing establishes. ADR-118's `Missing` finding is reported
whether or not the function is total; `:total` adds the `Unknown` case.

**What `:total` does not claim.** That the functions it calls terminate (a `total`
function may call `map`), or that no `throw` runs — a total function may raise on purpose;
what it may not do is fall into a case it did not write. Non-tail self-recursion stays the
existing lint's business.

## Opting out

`(check-allow :pure …)` and `(check-allow :total …)` wrap a definition or a call: the
walk's coverage finding reads the `SUPPRESS_TOTAL` bit, the properties pass reads the
`%lint-allow` marker structurally.

## In std

`path/join` is `:pure :total`, its two loops `:total`; `regex`'s two DFA loops are
`:total` — the upward counter under the callers' count relation is the shape that needed
ADR-350's parameter relation to be sayable at all.

## Deferred

- Effect *inference* as a reported property (an `Effects:` line in `nest docs`, a hover):
  the walk computes it; nothing displays it yet.
- Transitive totality across calls, and mutual recursion: a call graph with a measure per
  edge — a different analysis.
- A runtime contract for either property: there is nothing to check at a call boundary.
