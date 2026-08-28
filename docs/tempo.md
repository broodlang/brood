# `std/tempo` — time as an interval, not an instant

`std/tempo.blsp` adapts the **model** of [Tempo](https://github.com/elixir-tempo/tempo)
(Kip Cole, Apache-2.0) to Brood. It is a reimplementation written against Tempo's
public README and design, not a translation of its source.

## Attribution

Required by section 4(b) of the Apache License:

    Tempo
    Copyright 2021 Kip Cole
    https://github.com/elixir-tempo/tempo
    Licensed under the Apache License, Version 2.0
    http://www.apache.org/licenses/LICENSE-2.0

Changes made in this adaptation:

- Reimplemented in Brood Lisp rather than Elixir. No source from the original work
  is reproduced; the design — a single resolution-carrying type, half-open interval
  semantics, Allen-relation comparison, unit-implied enumeration — is what has been
  adapted.
- Reduced in scope: Gregorian calendar only, UTC only, millisecond resolution,
  ISO 8601 Part 1 only, no recurrence rules, no constraint network.
- Renamed `contains?` to `covers?` and the enumeration entry point to `parts`, to
  avoid shadowing Brood prelude names.
- Component values must occupy a contiguous prefix of the unit hierarchy; sparse
  component lists are rejected.

Brood is AGPL-3.0-only. Apache-2.0 is one-way compatible with AGPL-3.0, so the
combination is fine; the attribution above is what has to travel with it.

## What the port keeps

The one idea worth taking is the type. Every mainstream library splits time into
`date` / `time` / `datetime` scalars and leaves each value's *meaning* to the
caller — is `2026-06-15` midnight or the whole day? Tempo answers by making
resolution part of the value and every value a half-open span. That single change
deletes the `end-of-day` helper, the "last day of month" special case and most
off-by-one boundary bugs by construction.

| Tempo | Brood | Note |
|---|---|---|
| `%Tempo{time: [...]}` | `(defrecord tempo (by-unit res))` | a unit map plus its resolution |
| `Tempo.new/1`, `new!/1` | `tempo/new`, `tempo/new!` | component map; `[:ok t]` / `[:error msg]` per ADR-163 |
| `~o` sigil | `tempo/parse`, `parse!` | see "sigil" below |
| `Tempo.to_interval/1` | `tempo/->span` | half-open epoch-ms bounds |
| `Tempo.Interval.endpoints/1` | `tempo/from`, `tempo/to`, `from-ms`, `to-ms` | |
| Allen relations | `tempo/relation` | all thirteen, one function |
| `overlaps?`, `contains?`, … | `intersects?`, `covers?`, `during?`, `before?`, … | |
| `intersection`, `union`, `difference` | same names | closed over interval sets |
| interval sets | `tempo/iset`, `set-of`, `spans`, `bounds`, `gaps`, `total-width` | sorted, disjoint, non-touching |
| `Enum` protocol over a Tempo | `tempo/parts` | next-finer-unit enumeration |
| `Tempo.explain/1` | `tempo/explain` | plain string, not a structured form |
| `Tempo.from_elixir/1` | the `Spanning` ability | `datetime/date` and `datetime/datetime` impl it |
| ISO 8601 Part 1 reduced precision | `tempo/parse`, `tempo/->iso` | |

## Deliberate deviations

**`contains?` → `covers?`.** `contains?` is a prelude predicate over maps and
collections. Defining `tempo/contains?` would shadow it for bare calls *inside the
module*, which is a live trap given the module uses `contains?` on maps in its own
validation. Renamed rather than risked.

**`seq` → `parts`.** Same reasoning, with more at stake: `seq` is a core sequence
function, and prelude macros (`for`, `every?`, `fold`) expand to calls that could
resolve to a module-local `seq` under late binding. `parts` also reads better for
what it does — the parts of June are its days.

**Sigil, not implemented.** Tempo's `~o` does double duty as a compile-time literal
and a *pattern*. Brood has no sigil syntax; the pattern half would be a reader macro
plus matcher integration, which is a language change, not a library one. `parse!`
covers the literal use. A `(tempo/o "2026-06")` macro that parses at compile time is
a cheap follow-up if literals turn out to be hot.

**`:ms`, not `:microsecond`.** Matches `std/datetime`, which is millisecond
resolution throughout. Going finer means changing `datetime`'s epoch conversions
too, and that is a separate decision.

**Contiguous units required.** Tempo permits sparse component lists (a time with no
date, `:day_of_week` without `:day`). This port requires a contiguous prefix of
`[:year :month :day :hour :minute :second :ms]` and rejects `{:year 2026 :day 15}`
outright. Sparse values are what make Tempo's enumeration machinery large; admitting
them later is additive, and rejecting them now is honest.

**Spans and sets, not tempos, out of the set algebra.** The intersection of two
months is rarely a month. This port returns a distinct `iset` of `span`s so a value
never claims a calendar shape it does not have.

## Why it depends on `std/datetime`

`tempo` uses five `datetime` functions and only three ideas: `->epoch-ms` /
`epoch-ms->` (the Hinnant `civil_from_days` / `days_from_civil` pair), `days-in-month`,
and `utc-now` (plus `->iso` for rendering an instant inside `->iso-span`).

That Hinnant pair is the only calendar arithmetic in the system. There should be
exactly one copy of it, and it is already correct — including for pre-epoch dates,
because `dt-fdiv` is real floor division rather than truncating division.
Reimplementing it in `tempo` would mean two copies to keep in step and two places for
a leap-year bug to hide. So `tempo` depends on `datetime`, one direction, no cycle.

`tp-dt` constructs a real `datetime/datetime` record rather than a lookalike map.
`datetime/->epoch-ms` happens to read its fields with `get`, so a map would work —
but relying on that would be duck typing against an undocumented contract.

`datetime/time-of-day` deliberately has **no** `Spanning` impl. A wall-clock time has
no position on the timeline until a date anchors it, so `(tempo/->span some-time)` is
a loud missing-impl error rather than a silent guess at today's date.

## The open decision — `Temporal`

`std/datetime` declares:

```lisp
(defability Temporal
  :sealed [datetime date time-of-day]
  (->iso [self] :-> string))
```

`tempo` ships its own plain `tempo/->iso` function rather than joining `Temporal`.

**Not because it can't.** `:sealed` is an *exhaustiveness checklist*, not an access
control: it makes the checker demand that every id in the list implements every
required op. It does **not** close the ability. A non-member may implement it, from
any module, and both `nest check` and the runtime accept it — verified:

```lisp
(defmodule anywhere (:use datetime))
(impl Temporal tempo/tempo (->iso [t] (tempo/->iso t)))
(->iso (tempo/parse! "2026-06"))              ; → "2026-06", check exit 0
```

The same `impl` placed inside `std/tempo.blsp` also works — the body's `->iso`
resolves to tempo's own function, not back to the ability op, so there is no
recursion and no name clash. One `->iso` covers all four types today, with
`std/datetime` untouched and `(%sealed-members 'Temporal)` unchanged.

So the real choice is narrower than it looks:

1. **Just impl it from `std/tempo.blsp`.** One line, no dependency reversal, no
   `datetime` change. Cost: `tempo/tempo` is not in the sealed list, so the checker
   never *demands* the impl — if someone deleted it, only a call site would complain.
2. **Also widen the sealed list** to `[datetime date time-of-day tempo/tempo]`, to get
   that demand. This is the only option that costs something: `std/datetime` would
   name `std/tempo`, reversing the dependency direction (checker-only — a sealed list
   is a set of symbols and nothing loads `tempo` to read it), and
   `tests/datetime_test.blsp` asserts `(%sealed-members 'Temporal)` is exactly those
   three, so that test changes too.
3. **Leave it.** Two `->iso` ops, callers pick by namespace.

**Shipped as (1).** `std/tempo.blsp` carries `(impl Temporal tempo/tempo (->iso [t] (->iso t)))`,
so `(datetime/->iso x)` renders a tempo, a date, a datetime or a time-of-day. `std/datetime` is
untouched and `(%sealed-members 'Temporal)` still names exactly its original three; the only
thing not gained is the checker *demanding* the impl exist, which is what (2) would buy at the
price of the reversed dependency.

`tempo/->iso` remains as a plain function — the ability op delegates to it, and
`tests/tempo_test.blsp` pins that the two agree at every resolution.

## Defects found and fixed on integration

The module arrived unverified (its author had no Rust toolchain). It built, checked
and passed its own 79 tests unchanged; an independent probe of ~35 edge cases found
three real defects, since fixed with regression tests:

1. **`->iso` / `parse` did not round-trip on negative years.** `->iso` emits `-0044`;
   `parse` split on `-` and read the leading sign as an empty first field, so the
   module could not read its own output. `parse` now strips a leading `-` and negates
   the year.
2. **`parse` accepted a signed field.** `tp-digits?` delegated to `string/->number`,
   which reads a sign, so `"2026-+6"` parsed as June. It now tests for digits.
3. **`truncate` silently no-op'd on a non-unit.** `(truncate t :fortnight)` gave rank
   `-1`, which fell into the "already coarse enough" branch and returned `t`. A typo'd
   keyword now throws.
4. **`now`, `finer`, `coarser` and `unit` had the same hole** — found on later review passes,
   after (3) was already fixed. `now` was the worst: an unguarded unit reaches
   `tp-restrict` at rank `-1`, which keeps *no* units, so `(now :fortnight)` built a tempo
   with an empty unit map and only failed later, as `expected number, got nil` from inside
   `tp-pad` or `datetime/dt-ymd->days` — nowhere near the typo. `finer`/`coarser` returned
   `nil`, conflating "there is no finer unit" (a real answer, for `:ms`) with "that is not a
   unit"; `unit` did the same with the nil that means "`t` does not carry it". All five
   unit-taking entry points — `finer`, `coarser`, `truncate`, `now`, `unit`, enumerated from
   the source rather than recalled — now throw. Fixing one site of a class without sweeping
   for its siblings is the mistake worth naming here; it took two further passes to finish.

   Everything else validates acceptably by house convention: a wrong type reaches a primitive
   and throws (`(parse 42)` → `string/length: expected string`), and a non-`Spanning` value
   gets the ability's own error, which names the impls it does have. `unit` was the only
   entry point that answered wrongly instead of raising.

Cross-process coverage was also missing, which the repo's test protocol requires of anything
carrying values: a `send` deep-copies between per-process heaps, so `tests/tempo_test.blsp`
now round-trips a tempo, a negative year and a multi-span interval set through workers, and
checks that both `Temporal` and `Spanning` dispatch in a process that never registered them.

Checked and *not* found: pre-epoch spans, month shifts across year zero
(`0000-01` → `-0001-12`), ±100-month shifts, day shifts over month and leap-year
boundaries, fractional-second truncation past three digits, the empty-set algebra,
1000-part `:ms` enumeration, and half-open adjacency throughout.

## Not ported, roughly in the order I would do it

1. **ISO 8601 Part 2 / EDTF** — masked digits (`156X`), unspecified components
   (`2026-XX-15`), qualifications (`1984?`, `2004~`), open-ended intervals (`1985/..`).
   This is where "time as interval" starts paying for itself on archival and historical
   data. The interval set is already here, which is what a mask needs to denote.
2. **Timezones.** Requires a tz database in Brood; `std/datetime` has none, so this is
   a bigger lift than the rest combined. Until then everything is UTC, stated plainly
   rather than fudged.
3. **Non-Gregorian calendars.** Tempo delegates to `Calendrical`. A `Calendar` ability
   with Gregorian as the first impl is the natural shape.
4. **Recurrence** — RRULE / cron / iCalendar import. `1985-XX-15` ("the 15th of every
   month") still cannot be represented.
5. **The constraint network** (`Tempo.Network`) — Allen-relation solving over
   partially-known intervals. Genuinely novel, and standalone enough to be a package
   rather than std.
