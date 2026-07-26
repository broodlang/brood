# Roadmap for v1.0 — the language freeze

**Purpose.** [`ROADMAP.md`](../ROADMAP.md) is the canonical list of *everything*
planned. This file is the much shorter list of what must happen **to the language
surface** before 1.0, and — just as importantly — what must *not*, because the point
of a 1.0 is that the surface stops moving.

Written 2026-07-26, at the end of the syntax review that produced ADR-155…163.

---

## The test

Every remaining language item gets sorted by one question:

> **Can this be added later without breaking anyone, and without leaving the whole
> corpus written in a dated idiom?**

- **Yes** → *defer it*. Additive features cost nothing to delay; that is ADR-011's
  rule, applied to the freeze. Shipping 1.0 without them costs a version number.
- **No** → it has to happen now, because "later" is exactly what a freeze gives up.

An item fails the test for one of two reasons, and they are different:

1. **Breaking** — adding it later would change what an existing, valid program
   means. (Reserving reader syntax; a second protocol dispatch axis.)
2. **Idiom-shaping** — adding it later breaks nothing, but every line written before
   it reads dated, so adoption becomes a churn wave the moment you have promised
   stability. (Callable keywords.)

By this test, **three** items qualify. Everything else waits.

---

## 1. Callable keywords — `(:key m)`

⬜ **Status: needs an ADR, then a small kernel change.** **[kernel]**

**What.** Make a keyword callable as a one-argument accessor:
`(:name person)` ≡ `(get person :name)`, and `(:name person "unknown")` ≡ the 3-arg
`get`. Nothing else becomes callable.

**Why it can't wait.** It is additive in the strict sense — today `(:k m)` raises
`cannot call non-function`, so no valid program changes meaning — but it is
*idiom-shaping*, and the corpus is already enormous:

| Shape | Sites |
|---|---|
| `(get x :keyword)` in `brood/std` + `brood/tests` | **1,485** |
| `(get x :keyword)` across the 12 sibling projects | **4,795** |
| Hand-written `(fn (p) (get p :k))` accessor lambdas | **82** |

Those 82 lambdas are the sharp end: `(map :name people)` is the most-reached-for
shape in map-heavy code and currently has no spelling at all. Ship 1.0 without this
and every line of `std/`, every sibling, and every example in the docs is written the
old way — then adding it becomes a migration wave at exactly the moment stability was
promised.

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

## 3. Decide the protocol `:type` dispatch axis — permanently

⬜ **Status: a decision.** **[Brood]**

ADR-158 shipped protocols dispatching on `type-of` of the first argument, which means
every `defrecord` value dispatches as `:map` (records are structural — ADR-130). A
second axis keyed on a `:type` field would fix that, and **adding it later is
breaking**: it changes what an existing map carrying a `:type` key dispatches to.

So before 1.0 this becomes one of:

- **Never** — records dispatch as `:map`, you branch on a field inside that impl.
  Document it and close the question. (This is ADR-158's current, provisional stance.)
- **An explicit opt-in** — e.g. a protocol declares its dispatch function, so nothing
  is captured silently.

Leaving it as "maybe" is the only option that isn't available.

---

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
| Multiple dispatch | Single dispatch on the first argument; `match` covers the rest | ADR-158 |
| Nominal types | `defrecord` is structural sugar over a map | ADR-130 |
| More than one spelling per thing | `lambda`, `let*`, `car`/`cdr`, `concat`, `some?`, `length` all removed | ADR-098, ADR-154, ADR-162 |

---

## Explicitly deferred to post-1.0

All purely additive — deferring costs a version number and nothing else.

- ⬜ **Inline `sig`s** — `(defn f ((x int) -> int) …)`. **One judgment call attached:**
  if types are meant to be *widely* annotated by 1.0, the shape is idiom-shaping and
  belongs in the "now" bucket. If annotations stay sparse and opt-in — three modules
  today (ADR-153) — then `(sig …)` below the definition is fine and this waits. An
  ADR-082 revision touching `defn`, `sig_of`, `defrecord`'s emitted sigs, and `sig!`.
- ⬜ **Re-host the seq protocol on ADR-158 protocols.** The principled fix (policy out
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
- ⬜ **Record-shape protocol dispatch**, *if* item 3 above resolves to "an explicit
  opt-in" rather than "never".

---

## Not language, but 1.0 release blockers

Tracked in [`ROADMAP.md`](../ROADMAP.md); listed here so a release checklist doesn't
miss them.

- 🟥 **The conformance red.** Full `nest test` grinds to the 900 s cap on the two
  100k-deep JSONTestSuite documents (`--exclude conformance` is green in ~30 s). This
  is ROADMAP's one open red item and it predates the review. A 1.0 cannot ship with
  the project's own suite unable to complete.
- ⬜ **`nest format --check` is red on 46 files**, including `project.blsp`. The
  formatter reflows regions the tree writes differently (splitting multi-arg
  `error`/`str` calls), so it needs one deliberate whole-tree pass plus a decision on
  whether that style is wanted — not a drive-by.
- ⬜ **The `nest::registry` tests** fail whenever the signing agent stops approving
  (`commit.gpgsign` + `op-ssh-sign` makes `git commit` hang in the temp repos they
  build). An environment condition, but one that makes CI results untrustworthy.

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
