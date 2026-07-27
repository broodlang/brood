# Roadmap for v1.0 — the language freeze

**Purpose.** [`ROADMAP.md`](../ROADMAP.md) is the canonical list of *everything*
planned. This file is the much shorter list of what must happen **to the language
surface** before 1.0, and — just as importantly — what must *not*, because the point
of a 1.0 is that the surface stops moving.

Written 2026-07-26, at the end of the syntax review that produced ADR-155…163.

---

> **Status, 2026-07-26:** item 1 is **done** (ADR-165), and a fourth item was added
> and done the same day — **reserved names** (ADR-166), which belongs in this file
> precisely because of the test below: *relaxing* a restriction later is
> backward-compatible, *adding* one is not, so a language freeze has to decide it
> first. Items 2 and 3 — the two remaining irreversible decisions — are all that
> stands between here and the freeze.

## The test

Every remaining language item gets sorted by one question:

> **Can this be added later without breaking anyone, and without leaving the whole
> corpus written in a dated idiom?**

- **Yes** → *defer it*. Additive features cost nothing to delay; that is ADR-011's
  rule, applied to the freeze. Shipping 1.0 without them costs a version number.
- **No** → it has to happen now, because "later" is exactly what a freeze gives up.

An item fails the test for one of two reasons, and they are different:

1. **Breaking** — adding it later would change what an existing, valid program
   means. (Reserving reader syntax; a second record dispatch axis.)
2. **Idiom-shaping** — adding it later breaks nothing, but every line written before
   it reads dated, so adoption becomes a churn wave the moment you have promised
   stability. (Callable keywords.)

By this test, **three** items qualified. Everything else waits. Two are now
done — callable keywords (ADR-165) and the record dispatch axis (ADR-168) — leaving
**§2, the reader's permanent reservations**, as the one open pre-freeze decision.

---

## 1. Callable keywords — `(:key m)`

✅ **Done — ADR-165** (2026-07-26). Implemented in `eval::apply`, the one function both
engines funnel non-closure callees through, so a keyword is a first-class value the
higher-order ops can take. Keywords only; map/vector/set stay non-callable. The
checker's `relax_param_for_arg` admits a keyword wherever a callable is expected.

> **The performance claim was struck from the justification.** `(:name p)` measures
> 130 ms/1M vs `get`'s 393 ms, but that is the Brood/Rust boundary, not the syntax: the
> breakdown is one Brood closure call (+124 ms) plus a four-branch `cond` (+138 ms),
> and the JIT closes none of it (393 with, 374 without). Implemented in Brood it would
> measure like `get`. Selling it on speed would be the move `CLAUDE.md` warns against.
> It stands on the 67 accessor-lambda sites. What the measurement *did* surface — the
> call + type-dispatch overhead the JIT can't see through — is now its own ROADMAP item.

**What.** Make a keyword callable as a one-argument accessor:
`(:name person)` ≡ `(get person :name)`, and `(:name person "unknown")` ≡ the 3-arg
`get`. Nothing else becomes callable.

**Why it might not wait.** It is additive in the strict sense — today `(:k m)` raises
`cannot call non-function`, so no valid program changes meaning — but it is
*idiom-shaping*.

**The honest size of the case, after checking the actual sites:**

| Shape | Sites | Would `(:k m)` improve it? |
|---|---|---|
| `(fn (p) (get p :k))` passed to a HOF | **67** (59 `map`, 4 `filter`, 3 `sort-by`, 1 `keep`) | **Yes** — `(map :name deps)` |
| `(get x :keyword)` standalone | 4,796 | **No** — `(get m :name)` puts the subject first and reads better |
| `(get (get x :a) :b)` nested | 81 | **No** — that's `get-in`, which exists and is used 166× |
| `(-> m (get :a) (get :b))` threading | **0** | — (no sites; this argument was hypothetical) |

So the case is **67 sites of one shape**: keyword-as-*function*, not
keyword-as-shorter-`get`. An earlier draft of this file cited the 4,796 figure as
justification; that number was doing rhetorical work it doesn't deserve, since almost
none of those sites would be converted even with the feature.

**And there is a cheaper alternative** that captures most of it with **no language
change** — a prelude one-liner, so it can ship in 1.1 as easily as now:

```clojure
(defn getter (k) (fn (m) (get m k)))
(map (getter :name) deps)
```

`partial` can't do this, because `get`'s key is its *second* argument. So the real
question is narrow: **is `(map :name deps)` worth a permanent exception in the call
path, when `(map (getter :name) deps)` costs one prelude function?** Arguments for:
it is the most readable form, it is what any Clojure-fluent reader reaches for, and
`getter` is a *third* spelling for one idea (against "one spelling each"). Against:
67 sites, the only non-function value that becomes callable, care needed in three
engines plus the checker, and the alternative is additive.

**Prerequisite, now done:** ✅ **ADR-164** fixed `get`/`nth`'s diagnostics. Four of
`get`'s five failure modes leaked an error from an internal (`-`, `<=`, `empty?`) and
the fifth returned `nil` silently — and `(:name deps)` where `deps` is a *list* is
exactly the most likely misuse of callable keywords, so it had to say something true
first.

**Scope — keywords only.** Not maps, not vectors, not sets:

- **Callable maps** would make `(m k)` and `(get m k)` two spellings of one thing,
  against ADR-154's "one spelling each".
- **Callable vectors** invite the index-vs-value confusion already refused for
  `contains?` (ADR-156).
- **Callable sets** are the same membership-vs-index trap in a third costume.

One blessed exception to "the head of a form is a function" is a rule you can state
in a sentence. Four exceptions is a different language.

**Expected cost.** Probably near zero on the hot path: every engine already branches
to a "not callable" error at precisely the point the new arm goes, so this populates
an existing branch rather than adding one. The place to *measure* is the JIT — a call
whose callee is not a closure may bail out of native code, and that is the only real
risk of a regression.

**Open questions for the ADR** — see the discussion notes at the bottom of this file.

## 2. Settle what the reader permanently reserves

⬜ **Status: a decision, then a paragraph.** **[kernel]**

Reader syntax is the least forgiving surface to change after 1.0. Most current
absences are safely additive — `#|…|#`, `#_`, `#"…"`, `#'`, `\c` all *error* today
(each with a hint), so adding any of them later breaks nothing.

Exactly one is not additive:

- **`1/2` reads as a symbol today.** If a ratio type is ever wanted, that token has
  to be **rejected now** to reserve it. If it is never wanted, say so in writing —
  and note that `/` is the namespace separator, so the token is doubly spoken for.

Also worth writing down as permanent, since they are already irreversible:
`inf`/`nan`/`-inf` are reader float literals (so those three tokens can never be
names), `|…|` bar-quoting owns the round-trip of odd symbols, and `#{…}` / `#b"…"`
are the only two `#` literals.

## 3. Decide the record dispatch axis — ✅ done (ADR-168)

ADR-158 shipped protocols dispatching on `type-of` of the first argument, which meant
every `defrecord` value dispatched as `:map` (records are structural — ADR-130). A
second axis keyed on a `:type` **field** would have fixed that, but **adding it later
is breaking**: it changes what an existing map carrying a `:type` key dispatches to.
So the question could not be left at "maybe" — it had to be settled before the freeze.

**Settled: neither of the two options originally listed.** The `:type`-field axis is
**permanently rejected** — it captures shapes silently, which is exactly the ADR-011
failure mode. But "records dispatch as `:map`, branch on a field inside" was not
accepted either. Instead **ADR-168** gives a record an *explicit,
construction-time* dispatch identity: a `defrecord*` bakes a `:module/name` keyword
into each value, and `ability` dispatch keys on it. Nothing is inferred from a field,
so a plain map carrying `:type` is never rerouted; and a record's structural
behaviour is untouched (`type-of` is still `:map`, `get`/`assoc`/`=` still
structural), so ADR-130 stands.

That closes the breaking question the freeze was worried about: the identity is opt-in
at the definition site, and the field-sniffing axis is now a documented "never" on the
freeze list below.

---

## 4. Reserved names — ✅ done (ADR-166)

Everything the language ships is **reserved**: the prelude's functions and macros,
every Rust builtin, every function an embedded std module defines. A `(def get …)` is
an error. Your own globals and your packages stay fully redefinable, so ADR-013 hot
reload — the editor updating itself as you use it — is untouched.

It belongs on the *pre-freeze* list by the asymmetry: relaxing this later breaks
nothing, adding it later breaks whoever monkey-patched. Erlang is the precedent (OTP's
modules are sticky; you cannot patch `Enum.map/2`), and it landed *after* protocols
(ADR-158) gave the sanctioned extension point that replaces patching — the same order
Elixir did it in.

Measured blast radius: **two lines** of user code across brood + 12 siblings, both
accidental collisions the rule catches (`(def comp (table))` in a sieve bench,
`(def dec …)` for decoded bytes). The namespace system had already done the work — a
module-scoped `(defn get …)` defines `your/mod/get` and was never a redefinition.

## The freeze list — what Brood permanently is not

This is the deliverable that makes a freeze credible, and it is writing, not code:
every answer below already exists, scattered across ADRs. A 1.0 that states clearly
what it refuses is far more trustworthy than one that leaves it implicit — and this is
the document that stops the questions being re-litigated after the freeze.

Draft, to be ratified as its own ADR before release:

| Refused | Why | Where decided |
|---|---|---|
| Mutation of data — no `set!`, atoms, cells, transients | The whole design rests on it: no write barriers, share-nothing processes, safe freezing | ADR-026, ADR-112 |
| `while` / `loop` / `recur` | Proper tail calls make recursion O(1); `letrec` covers local loops | ADR-154 |
| Named arguments (`&key`) | A trailing options map + `{:keys …}` reads the same and composes with `merge` | ADR-163 |
| Metadata (`^{}`), reader macros, `#(…)`, `#_` | Permanent surface for what a macro already does; `^` is the pattern pin | ADR-150 |
| A character type | A character is a 1-char string; the cursor unit is a grapheme cluster | ADR-159 |
| Ratios | *(decide — see item 2)* | — |
| `contains?` answering by index on a vector | Clojure's trap: `(contains? [1 2] 1)` true for the wrong reason | ADR-156 |
| Strings as seqable | Codepoint vs grapheme is the caller's decision; bridge explicitly | ADR-156, ADR-159 |
| Unbounded laziness / `lazy-seq` | Seq-views fuse pipelines; processes cover unbounded state | deferred.md #2 |
| Alternative *negation* patterns (`(not …)`) | Binds nothing, so it is a guard — `:when` is the slot | ADR-160 |
| `:as` in a map pattern | `(and whole {…})` says it exactly | ADR-160 |
| Multiple dispatch | Single dispatch on the first argument's identity; `match` covers the rest | ADR-158, ADR-168 |
| Dispatch inferred from a `:type` **field** | Would silently reroute any map carrying `:type`; a `defrecord*` identity is explicit and construction-time instead | ADR-168 |
| Nominal *types* | `defrecord` is structural sugar over a map; `defrecord*` adds a dispatch-only identity, not a type — `type-of` is still `:map` and `=` stays structural | ADR-130, ADR-168 |
| More than one spelling per thing | `lambda`, `let*`, `car`/`cdr`, `concat`, `some?`, `length` all removed | ADR-098, ADR-154, ADR-162 |
| Monkey-patching the language | shipped functions are reserved; extend with an ability, shadow with `let`, or namespace it | ADR-166 |

---

## Explicitly deferred to post-1.0

All purely additive — deferring costs a version number and nothing else.

- ⬜ **Inline `sig`s** — `(defn f ((x int) -> int) …)`. **One judgment call attached:**
  if types are meant to be *widely* annotated by 1.0, the shape is idiom-shaping and
  belongs in the "now" bucket. If annotations stay sparse and opt-in — three modules
  today (ADR-153) — then `(sig …)` below the definition is fine and this waits. An
  ADR-082 revision touching `defn`, `sig_of`, `defrecord`'s emitted sigs, and `sig!`.
- ⬜ **Re-host the seq protocol on abilities (ADR-168).** The principled fix (policy out
  of Rust, so a user type can join `count`/`first`/`conj`) but a measured rewrite of
  the hottest paths in the language — exactly the work you don't want against a 1.0
  deadline. No surface change, so post-1.0 is free.
- ⬜ **Transducer early termination** (`reduced` threaded through `fold`) and
  stateful-stage lifecycle. ADR-161 ships the one-arity contract `fold` needs.
- ⬜ **A rope-level grapheme cursor.** ADR-159's three accessors unblock correctness
  everywhere; a large buffer wants a cursor that caches the segmentation. Size it
  against a real editor workload.
- ⬜ **`#|` block comments**, and small helpers like a plain `assert`. Additive by
  construction.
- ⬜ **Monomorphization of ability dispatch** — resolving a call statically when the
  argument's identity is known (the checker already computes that identity for its
  missing-impl warning). A codegen win with no surface change, so post-1.0 is free.
  Likewise **return-type dispatch**, which needs bidirectional inference.

---

## Not language, but 1.0 release blockers

Tracked in [`ROADMAP.md`](../ROADMAP.md); listed here so a release checklist doesn't
miss them.

- ✅ **The conformance red — fixed 2026-07-27 (KI-14).** It was not the documents: the
  RUNTIME collector re-walked a deep process's whole root stack at every safepoint, so
  cost scaled with loaded code, not test size. Full `nest test` including conformance is
  **3592 tests in ~90 s**; `cargo nextest run` is 877/877.
- ⬜ **`nest format --check` is red on 52 files** (was 46 when this was written, so it
  is drifting), including `project.blsp`. The formatter reflows regions the tree writes differently (splitting multi-arg
  `error`/`str` calls), so it needs one deliberate whole-tree pass plus a decision on
  whether that style is wanted — not a drive-by.
- ⬜ **The `nest::registry` tests** fail whenever the signing agent stops approving
  (`commit.gpgsign` + `op-ssh-sign` makes `git commit` hang in the temp repos they
  build). An environment condition, but one that makes CI results untrustworthy.
  Currently **green** (6/6) with the agent unlocked — which is the problem: the result
  tracks whether a desktop app is unlocked, so it should not gate CI at all.

---

## Discussion notes — callable keywords

Questions the ADR has to answer, gathered so the design conversation starts from a
list rather than a blank page:

1. **Arity.** Just `(:k m)`, or also `(:k m default)` mirroring 3-arg `get`? The
   2-arity form is what makes it a drop-in replacement rather than a partial one.
2. **Non-map arguments.** What is `(:k 5)`? `get` on a non-collection currently falls
   through to `nth`-style indexing or errors depending on the kind. The choices are a
   type error (loud, consistent with the rest of the language) or `nil` (Clojure's
   nil-punning, which hides typos).
3. **`(:k nil)`.** Almost certainly `nil` — `(get nil :k)` is already `nil`, and this
   is the shape that makes threading through absent data pleasant.
4. **Is the keyword a first-class function?** Does `(map :name people)` work — i.e.
   is a keyword a *value* that `apply`/`map` can call — or only a syntactic head? The
   whole ergonomic win is the former; that is the point of the change, and it means
   the callable check lives in `apply_value`, not the compiler.
5. **Sets.** `(get #{1} 1)` is now membership (ADR-156), so `(:k some-set)` would
   answer membership too. Coherent, or confusing enough to reject?
6. **The checker.** A keyword in call position currently reads as an error; the
   advisory checker needs to know the new shape so it neither warns falsely nor loses
   the "cannot call" diagnostic for genuine mistakes (`(:k)`, `(:k a b c)`).
7. **JIT behaviour.** Measure whether a keyword callee forces a deopt/bail on the
   native path. If it does, decide whether that is acceptable (accessors are usually
   in interpreted glue code) or worth lowering natively.
8. **Error message when arity is wrong.** `(:k)` and `(:k m x y)` should say something
   better than a generic arity error — they are the two ways this gets typo'd.
