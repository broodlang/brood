# Wilhelm's review — language core & standard library

Status marks added 2026-08-28. `[x]` resolved, `[ ]` open. A note under an open item is
what was *checked*, not a decision — the call is still yours.

# Language Core

## Math and Numbers

- [ ] **is `compare` Math and Number or is it an ability? how does that compare with `==`?**
      Checked: `compare` is a **kernel builtin** (`builtins/mod.rs:571`), a 3-way
      `-1/0/1` structural comparison over any two values. There is **no `==` in Brood** —
      `=` is the only equality, and `%ord-compare` is the record-aware wrapper that
      `sort`/`sort-by` use (a record defers to its `Ord` `compare-to`). So the surface is
      three things wearing one idea: `=`, `compare`, `compare-to`. Worth a doc section at
      minimum; possibly `compare` belongs with `Ord` rather than under "math".
- [ ] **where is `==`, and what is `compare` then? add an example** — see above; needs a
      `docs/language.md` entry once the naming above is settled.
- [ ] **`dev/inc` can be under math** — checked: `inc`/`dec` are **bare in
      `std/prelude/core.blsp:325`**, not in `dev`. If a `dev/inc` is showing up somewhere
      it is a doc/catalogue artefact, not a definition. Worth confirming where you saw it.
- [ ] **`num-div` does not make sense to me** — it is a `defmulti`
      (`std/prelude/tools.blsp:2251`), part of the `Num` ability that lets a *record*
      (money, complex, 2-D vector) answer `+`/`-`/`*`/`/`. Deliberately not called from the
      operators — a Brood branch in `+` cost ~195x — the kernel `%div` calls it only on its
      cold fallback when an operand is a record. The name is the problem, not the mechanism.

## Strings and text

- [x] **`index-of` and `last-index-of` seem to be `string/index-of` etc.** —
      **`last-index-of` → `string/last-index-of` (done).** It is string-only (calls
      `%str-last-index-of`, defaults `before` to `(string/length s)`), so ADR-230's own
      boundary rule puts it in the module. **`index-of` deliberately stays bare**: it is
      genuinely polymorphic over strings, lists and vectors, which is the bare-name rule.
      The pair *looked* symmetric and was not — that asymmetry was the real finding.

# Standard Library

## gen

- [ ] **`defprocess` does not make sense here; `def*` is usually part of the language core**
      Agreed as an observation — every other `def*` (`defn`, `defmacro`, `defrecord`,
      `defability`, `defmulti`, `defdyn`, `defmodule`) is core, and `defprocess` is the one
      that lives in a module (`std/proc/gen.blsp:213`). Either it moves to the prelude or
      it gets a non-`def` name; leaving it is the inconsistency you spotted.

## datetime

- [ ] **don't we have the `?` predicates in language core?** — ADR-251 already settled the
      general rule (*"a type predicate does not move — the predicates are one family,
      recognisable by their `?`"*), so `datetime`'s predicates are a live counter-example.
- [ ] **can we consolidate some of these functions to be not so specific?**
- [ ] **how do timezones work?** — needs an answer in `docs/` either way.

## json

- [x] **is `decode` not the opposite of `encode`, not `parse`?** — **done: `json/parse` →
      `json/decode`**, pairing with `json/encode`. 46 call sites plus its `sig`, the module
      docstring, and the prose in `json_test`/`grammar_test`. Note `std/csv.blsp` keeps
      `csv/decode` / `csv/encode` — that pair is *internally* coherent (reader/writer), so it
      was left alone rather than churned; flag it if you want one convention across both.

## seq

- [ ] **can we consolidate some of these functions?** — partially touched this pass
      (helpers moved out of the prelude, `sig`s added, `iterate-times` published), but the
      *surface* was not reduced. 37 public functions is still the question you're asking.

## stdimage

- [ ] **is some of this not just private?** — not audited yet.

## uuid

- [ ] **can `uuid/zero` not just be `nil`? or `zero`?** — checked: `uuid/zero`
      (`std/uuid.blsp:78`) returns the *string*
      `"00000000-0000-0000-0000-000000000000"`, not a nil value — so `nil` would be wrong
      and `zero` (or `uuid/zero`) reads better than `uuid/zero`.

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
