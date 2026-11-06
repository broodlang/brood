# The empty list is `nil` — what it would take to separate them

**Status: DECIDED 2026-11-06 (ADR-620) — option 3: `()` stays `nil`, and the checker flags
a truthiness test on a value that may be a list or another collection.** The scoping below is
kept as written. A language-design finding, not a
bug. The evidence below was gathered by reading and grepping; counts over Brood source come
from regex heuristics and are marked as estimates. The lint is `:list-truthiness`, in
`check_if` (`types/check/walk/binders.rs`): it reads a test whose type admits an empty list
and a vector, map, set or string. A list alone is not reported.

## The finding

Only `nil` and `false` are falsy, but `()` reads as `nil` (ADR-003), so the empty **list** is
the one collection whose empty value is falsy:

```clojure
(if [] :yes :no)   ;=> :yes
(if {} :yes :no)   ;=> :yes
(if "" :yes :no)   ;=> :yes
(if () :yes :no)   ;=> :no
```

`docs/language.md` §Truthiness calls this "the one asymmetry" and tells users to test with
`empty?`. The reasoning Brood already accepted for failures (ADR-310) applies here too:
`nil` means *absence*, and a value that exists but is empty isn't absent. `(rest '(1))` is a
list with nothing in it, not "no list". Today the same `nil` answers both "nothing was found"
(`get`, `first` of an empty list, `os/env`) and "the list is empty", and the `(list E)`
type has to be spelt `nil | list<E>` (ADR-350).

## What exists today (verified)

**Representation.** `Value::Nil` is discriminant 0 of a `#[repr(C, u8)]` enum
(`core/value.rs:650`); the JIT depends on that layout (`value_layout_is_stable_for_the_jit`).
Proper lists end in `Nil` (`value.rs:675`); `Heap::list` ends with `Value::nil()`
(`core/heap/map_ops.rs:1269-1279`). `Range`/`SeqView` report `Tag::Pair` and return `Nil`
when empty.

**Reader/printer.** `read_seq` starts from `tail = nil` (`syntax/reader.rs:409`), so `()`
*is* `nil`; the printer writes `nil` (`syntax/printer.rs:82`) and stops a list walk on it
(`:281`). `()` prints as `nil` today.

**Builtins returning `nil` for empty.** `rest` ("a one-element sequence yields nil",
`builtins/sequences.rs:39`; confirmed: `(rest '(1))` → `nil`), `map` over empty input
(`%defseq` seeds with `nil`, `prelude/predicates.blsp:246-263`), `take 0`, `drop` past the end
(`prelude/seq.blsp:24-42`), `reverse nil`, `seq`, `(list)`, an empty `string/split`.

**Patterns.** A list pattern ends with `(if (nil? target) …)` (`prelude/match.blsp:396`); a
`()` pattern is the literal `nil` (`:155`); `(p & rest)` binds whatever `rest` returns.
`eval/macros/patterns.rs:68,83,131` read `nil` as an empty parameter list.

**Checker.** `Ty::LIST = Nil | Pair` (`types.rs:837`); `SEQABLE` and `COUNTABLE` include
`Nil`; `truthy() = any ∖ nil ∖ false` (`:869`), so `(if xs …)` narrows a list to `pair`.
ADR-348, ADR-350 and ADR-365 build on "the empty list is `nil`, its own tag".

**Messaging and the image.** `Message::Nil` (`process/message.rs:30`); the wire format has
`M_NIL = 0` plus `M_LIST`/`M_DOTTED` (`dist/wire.rs:492-536`). The prelude image and
`stdimage` reuse that codec (`boot/image.rs:22,300`), so one format change covers all three.

**VM/JIT.** Truthiness is decided in three places: `eval.rs:37`, `exec_chunk.rs:372`, and
`jit_lower/control.rs:366` (`tag == 0 || false-bool`). A new tag would be truthy in all three
without any change. The nil-as-list fast paths are `exec_chunk.rs:436,444,459`
(`first`/`rest`, `nil?`, `empty?`) and `jit_lower/prim.rs:213,742`.

**Kernel counts.** 235 `Value::Nil`, 40 `ValueRef::Nil`, 330 `Value::nil()`. Most of these
mean absence and stay as they are. The end-of-list subset is about 77 `Value::Nil =>` arms,
19 `ValueRef::Nil =>` arms, 23 `alloc_pair(…, nil)` and 13 `while let … Pair` loops.

## Brood code that leans on it (estimated)

| pattern | std (62k lines) | tests (61k) | bedit (86k) |
|---|---|---|---|
| `(if/when/and/or <sym>` on a plural or list-like name | ~44 | ~3 | ~16 |
| `(nil? <list-like sym>)` as an end test | ~88 | ~30 | ~72 |
| branching directly on a list-returning call | ~10 | ~3 | ~3 |
| `(or (map …) [])`-style defaulting | 5 | 1 | 1 |
| `(fold x nil …)` (nil-seeded list accumulator) | 16 | 5 | 3 |
| `& rest` parameters (bind `nil` when no args are passed) | 91 | 13 | — |
| `(empty? …)` total (already correct) | 486 | 127 | 405 |

Representative sites: `prelude/match.blsp:396` (every list pattern), `std/format.blsp:348,362`,
`std/pq.blsp:39`, `std/system.blsp:53` (`(nil? args)` on `& args`), `std/tool/dap.blsp:109`,
`std/tool/test.blsp:638,703,727,1776,1790`, `std/tool/eval-server.blsp:632`.

**The dangerous breakage is silent.** `(if xs (loop (rest xs)) done)` or a `(nil? xs)` end test
becomes an **infinite loop** once `rest` returns a truthy `()`. A tail loop doesn't allocate,
so the 16 GB address-space cap doesn't catch it; only a timeout does. We estimate 120–200
end-of-list tests at risk in std and tests, and about 90 in bedit. That comes from a
list-like-name heuristic, so the real figure could be twice that.

## The decision to make first

1. **Clojure-style (recommended to evaluate first).** `nil` still ends a cons chain, so
   `(cons 1 nil)` is `(1)` and every nil-seeded accumulator keeps working. Only empty *results*
   (`rest`, `map`, `take`, `()` as read) become a distinct, truthy `()`. This keeps 440 `cons`
   calls and `%defseq` as they are, but now two values can end a list, and every walker must
   accept both.
2. **Strict.** Only `()` ends a list; `nil` is never a list. It's cleaner: `LIST = EmptyList |
   Pair` and `nil` drops out of every collection type. It also breaks every `(cons x nil)`,
   every nil-seeded accumulator, and the `& rest` binding, all of which would have to change.
3. **Keep ADR-003.** The asymmetry stays and is documented (which it already is), and the
   checker lint in step 1 below ships anyway, so `(if <list> …)` gets a warning pointing at
   `empty?`. Most of the safety benefit, for a fraction of the cost.

Option 3 plus the lint is the cheap win. Options 1 and 2 are only worth it if we want `nil`
to mean absence everywhere, which is the same argument ADR-310 made about failures.

## What it would take (options 1 or 2)

1. **A transitional checker lint, before anything flips.** Flag `if`/`when`/`and`/`or`/`cond`
   and `nil?` whose test type contains both `pair` and the empty member, for symbols as well
   as calls. ADR-373's constant-condition lint (`types/check/walk/binders.rs:716`) is the
   model: today it only fires on calls with positively known types. Run it under `nest check`
   over std, tests and bedit and fix every hit to `empty?`/`seq`. **This is the step that
   turns the silent infinite loops into reviewed sites.**
2. **A runtime trap flag** (`BROOD_EMPTY_LIST_TRAP`, catalogued in `debug_flags.rs`). It
   reports the site when the empty list reaches a truthiness test or `nil?`, hooked at the
   three truthiness points (the JIT deopts). It's for the sites the checker can't type.
3. **Kernel.** A new `EmptyList` variant and tag **after** `Nil` (so `Nil` stays
   discriminant 0; there are 24 of 32 tag slots in use). Then:
   - the ~100 end-of-list sites;
   - the reader (`()` → empty list) and printer;
   - equality/hash/order (`core/heap/equality.rs:218,289,384,657,673,703,942`);
   - the `first`/`rest`/`empty?`/`nil?` builtins and the VM/JIT fast paths;
   - the codec, with an image/protocol version bump.

   **Estimate: 1–2 weeks.**
4. **Checker lattice.** Move `LIST`/`SEQABLE`/`COUNTABLE` to the new tag and revisit
   ADR-348/350/365. Mechanical. After the flip, ADR-373 flags `(if (rest …))` on its own,
   because a list-typed test is always true.
5. **Prelude.** `%defseq`, the list-pattern lowering in `match.blsp`, `rest`/`take`/`drop`,
   and `& rest` binding (option 2 only).
6. **Doctests are the API checklist.** Every docstring example that shows `→ nil` for an
   empty result (`map`, `take`, `drop`, `rest`, `reverse`) goes red in
   `tests/doc_examples_test.blsp`, which lists the surface to update.
7. **Downstream.** Run the lint over bedit, fix it, push it, then
   `make smoke-bedit ARGS=--bump`.
8. **Docs.** A new ADR superseding ADR-003's `()`-reads-as-`nil` clause; rewrite
   `docs/language.md` §Truthiness, `docs/spec.md:81,89` and `docs/brood-for-claude.md`; add a
   devlog entry.

**Gating.** This touches the GC-visible representation, the codec and the JIT, so it needs
the full proof on the test machine or in CI: `make test-both`, `make gcstress`, a
fuzz-differential pass, and `make ab`/`make ab-vm` for the `first`/`rest` hot path.
