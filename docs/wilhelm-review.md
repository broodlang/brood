# Wilhelm's review — language core & standard library

Status marks added 2026-08-28. `[x]` resolved, `[ ]` open, `[~]` partly. A note under an
open item is what was *checked*, not a decision — the call is still yours.

**This file quotes the original question wording, so it is a record — exclude it from any
bulk rename.** Three renames this session (`json/parse`, `uuid/nil-uuid`, `num-div`) each
swept through it and rewrote the very names the questions were asking about, which reads as
if the question had never been asked. Same rule as `decisions.md` and `known-issues.md`.

# Language Core

## Math and Numbers

- [x] **is `compare` Math and Number or is it an ability? how does that compare with `==`?**
      Checked: `compare` is a **kernel builtin** (`builtins/mod.rs:571`), a 3-way
      `-1/0/1` structural comparison over any two values. There is **no `==` in Brood** —
      `=` is the only equality, and `%ord-compare` is the record-aware wrapper that
      `sort`/`sort-by` use (a record defers to its `Ord` `compare-to`). So the surface is
      three things wearing one idea: `=`, `compare`, `compare-to`. Worth a doc section at
      minimum; possibly `compare` belongs with `Ord` rather than under "math".
- [x] **where is `==`, and what is `compare` then? add an example** — **`==` now exists**,
      and the reason it should is stronger than familiarity. Brood already had *two*
      equalities and they disagreed: `(= 1 1.0)` is false (strict, deliberate, tested in
      three places) while `(<= 1 1.0)`, `(>= 1 1.0)` and `(compare 1 1.0)` all say equal.
      Only the strict one had a name — so `a <= b` and `b <= a` could both hold while
      `(= a b)` was false, and **antisymmetry was quietly untrue**. `==` names what the order
      operators already believed; it is not a third notion.

      Defined over `compare`, not beside it, so there stays exactly one comparison engine.
      `not==` is its negation. Documented at the place `language.md` already explained the
      strict rule, and the antisymmetry law itself is now a test.
- [x] **`dev/inc` can be under math** — **already true, no change needed.** `inc`/`dec` are
      bare in `std/prelude/core.blsp` and catalogued `:math`, so the reference already lists
      them under "Math and numbers". `std/dev.blsp` has no `inc` at all — it is 19 runtime
      diagnostics (`mem-bytes`, `gc-stats`, `vm-stats`, …).
- [x] **`num-div` does not make sense to me** — **done: the family is now `num/add`,
      `num/sub`, `num/mul`, `num/div`**, with `std/num.blsp` declaring and documenting the
      namespace. `num-*` was a four-name hyphen prefix — exactly what ADR-251 calls "a
      namespace spelled by hand: it groups the names for a reader and not for the language".

      The mechanism is unchanged and worth stating, since that was half the question: these
      are multimethods dispatching on **both** operands, which the kernel's
      `%add`/`%sub`/`%mul`/`%div` call on their **cold fallback** when an operand is a
      record — so a money, complex or 2-D vector type can answer arithmetic. The operators
      deliberately do not test record-ness themselves; a Brood branch in `+` cost ~195x.

      Two latent gaps this exposed, both now closed. `numeric.rs` maps operator →
      multimethod by **bare string** (`"+" => "num/add"`), which is ADR-251's recorded
      rename hazard in its purest form: a miss would not fail the build, it would fail at a
      user's first `(+ record record)`. There is now a test pinning that table to the actual
      bindings. And the prelude-hygiene lint did not know `defmulti` defines a name — nothing
      had noticed, because no prelude multimethod had ever had a slash in it.

## Strings and text

- [x] **`index-of` and `last-index-of` seem to be `string/index-of` etc.** —
      **`last-index-of` → `string/last-index-of` (done).** It is string-only (calls
      `%str-last-index-of`, defaults `before` to `(string/length s)`), so ADR-230's own
      boundary rule puts it in the module. **`index-of` deliberately stays bare**: it is
      genuinely polymorphic over strings, lists and vectors, which is the bare-name rule.
      The pair *looked* symmetric and was not — that asymmetry was the real finding.

# Standard Library

## gen

- [x] **`defprocess` does not make sense here; `def*` is usually part of the language core**
      — **checked: it is not an outlier, so no change.** Three module-level `def*` macros
      exist and agree: `gen/defprocess`, `test/deftest`, `telemetry/defevent`. And
      `defprocess` genuinely cannot move — its expansion calls `gen-clause` and builds a
      gen-server receive loop, so it is a `gen` form, not a language construct. The working
      rule is "`def*` means this form defines something", which a module may legitimately do;
      it is not reserved to the core.

## datetime

- [x] **don't we have the `?` predicates in language core?** — **done: `datetime?`, `date?`
      and `time-of-day?` are now bare in the prelude**, beside `queue?`/`pq?`/`multimap?`.
      ADR-236's carve-out 2 already stated the rule and the prelude comment already spelled
      out the reasoning ("a value's type should read the same way whatever its type, rather
      than doubling as `queue/queue?` inside its own module"); datetime's three had simply
      been missed. They are pure record-id checks, so they need no module loaded.
      `before?`/`after?`/`same?`/`leap-year?` stay in the module — those are comparisons,
      not type predicates.
- [x] **can we consolidate some of these functions to be not so specific?** — **done, and the
      over-specific thing turned out to be an operator in disguise.** `before?`, `after?`,
      `not-before?`, `not-after?` and `same?` were `<`, `>`, `>=`, `<=` and `=` computed over
      `->epoch-ms` — five public functions doing what the operators do, and *invisible to
      them*: ordering a record routes through the `compare-to` multimethod, which the module
      never registered, so `(sort dates)` raised `%no-method` while `datetime/before?` worked
      beside it. Registering `compare-to` for `datetime`/`date`/`time-of-day` (and for
      `tempo`, which had the same gap as a plain function) deleted all five and bought `<`,
      `<=`, `>`, `>=`, `sort`, `compare`, `math/min` and `math/max` instead — ADR-286.
      Same-type only: a date and a datetime do not compare, since that would have to invent a
      time of day.

      The seven field accessors (`year`, `month`, … — each `(get dt :field)`) were considered
      and **kept**: 35 call sites across the repos, and nothing anywhere does `(:use
      datetime)`, so the `second` shadowing they can cause is latent and already
      self-documenting in the warning.
- [x] **how do timezones work?** — **answered in the module docstring, and one real defect
      fixed behind the question.** There is no zone type and no zone database: a value never
      carries a zone, so "09:00 in Berlin" is not representable and the questions needing a
      database cannot be asked. What is supported is the boundary case that actually matters:
      `parse-iso8601` now reads a numeric offset (`+02:00`, `-05:30`, `+0200`, `+02`) and
      applies it, so an API timestamp becomes the UTC instant it denotes. It returned **nil**
      before — a valid ISO 8601 string reported identically to garbage. Rendering stays UTC.

## json

- [x] **is `decode` not the opposite of `encode`, not `parse`?** — **done: `json/parse` →
      `json/decode`**, pairing with `json/encode`. 46 call sites plus its `sig`, the module
      docstring, and the prose in `json_test`/`grammar_test`. Note `std/csv.blsp` keeps
      `csv/decode` / `csv/encode` — that pair is *internally* coherent (reader/writer), so it
      was left alone rather than churned; flag it if you want one convention across both.

## seq

- [~] **can we consolidate some of these functions?** — **measured, and the honest answer is
      "less than it looks".** First count said seven were dead; that was wrong — it only saw
      qualified `seq/x` calls, and files that `(:use seq)` call them bare. Re-counting both
      shapes: all 37 are used.

      What the numbers do show: **18 of 37 have zero uses inside `std/` itself.** That is
      expected for a module ADR-227 created as helpers for *downstream* code, so it is a
      product question about surface breadth rather than a defect — the per-function table is
      in the devlog if you want to cut. **The one structural defect is fixed:** `third` was in
      `seq` while `first` (kernel) and `second` (prelude) were elsewhere — one trio, three
      homes. `third` is now bare beside `second`.

## stdimage

- [x] **is some of this not just private?** — **audited: it is already right, no change.**
      8 of the 13 definitions are `defn-`; of the five public ones, every one has an external
      caller (`install` 12, `image-path` 3, `ensure` 3, `ensure-built` 2, `build` 1). The only
      other public is `%std-impls-by-module`, whose `%` already says internal (ADR-250).

## uuid

- [x] **can `nil-uuid` not just be `nil`? or `zero`?** — **done: `uuid/nil-uuid` →
      `uuid/zero`.** It returns the *string* `"00000000-…-000000000000"`, not a nil value,
      so `nil` would have been actively wrong. It was an ADR-236 violation in its own right
      too: the `-uuid` suffix is the redundant module-name prefix that ADR dropped everywhere
      else (`queue/push`, not `queue/queue-push`).

---

## Also fixed this pass, not on the list above

- [x] `name` folded into `->string` — one spelling, and the word `name` is a user's again
      (ADR-258).
- [x] `sort` no longer autoloads the whole `seq` module to reach a helper three lines
      above it.
- [x] `std/seq.blsp`'s header and published docstring claimed `distinct`/`zip` were bare;
      they are not.
- [x] ADR-236 corrected: the empty constructor shipped as `queue/empty`, not `queue/new`.
- [x] A prelude macro template could be captured by a user module defining the same name
      (`defrecord` + a user `get` returned `:CAPTURED`). Fixed for every prelude macro,
      `receive` included, which meant finishing the `/name` root escape so it resolves in a
      module, at root, and at runtime macro expansion (KI-73).

## Found while answering the datetime items (2026-08-28)

Four defects that were not on the list, all surfaced by probing the two datetime questions
rather than reasoning about them.

- [x] **`sort` was a silent no-op over maps and sets.** `(compare {:a 1} {:a 2})` returned
      **0**, and `(sort maps)` handed back its input in input order with no error — both
      types fell through `value_cmp` to the cross-kind tag compare, where they rank the same.
      This is the KI-75 failure shape (a `compare` calling unequal values equal, a `sort` that
      no-ops instead of failing) on a different type, and `%ord-compare`'s docstring had been
      promising the order was "deterministic **and** total" throughout. Now ordered by content
      (ADR-285). Records are untouched: they still route to `compare-to` and stay strict.
- [x] **`(defrecord p (x y) :derives [Ord])` — the `defrecord` docstring's only example of
      `:derives` — was an expansion error.** `Ord` has never existed: `(%ability-ops 'Ord)` is
      nil. The `defability` docstring illustrated provided-vs-required ops with the same
      fictional ability, spelling its required op `compare-to`, which is a live multimethod.
      Both examples replaced with working ones.
- [x] **`%ord-compare`'s comment contradicted itself** — "the default is the kernel's
      structural `compare`" three lines above "there is NO `:default`". The code has always
      been strict; only the prose disagreed.
- [x] **`:derives` conflated two faults**, reporting "ability X is not derivable (declares no
      `:derive-record` recipe)" for an ability that is not loaded at all — which sends you
      looking for a recipe on something that does not exist. The two cases now read
      differently.
