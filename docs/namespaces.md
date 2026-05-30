# Namespaces — design

> **Status:** increments 1–2 landed (2026-05-30). Inc-1: the resolution substrate
> (`(ns …)`, resolver pass, forward-ref pre-scan, def-site keying, ns-aware checker).
> Inc-2: **`(:use …)` imports + auto-require** — `(:use mod)` refers a module's
> public names bare, `(:use mod :refer [a b])` a subset; the resolver consults the
> per-file import table after the current namespace and before root. Decision in
> [ADR-065](decisions.md). Supersedes [ADR-019](decisions.md).
>
> **Locked decisions not yet executed:** `defmodule` becomes the *single* namespace
> form and `ns` is dropped (a module **is** a namespace); ubiquitous DSLs like the
> test framework are namespaced + imported Clojure-style (`(:use test)`). This is
> the **next phase** (the `ns`→`defmodule` rename + migrating `std/` module-by-module
> + the 42 test files), gated on inc-2 imports (which now exist). Still open after
> that: macro free-reference resolution (§7, **α**), the ns-aware *import* checker
> (imported names currently draw advisory unbound warnings), LSP Tier 2, and package
> ns-collision policy (§8).

This doc is the design backing for namespaces in Brood. It follows the spectrum
ADR-019 laid out and commits to the *substrate* (how resolution works) while
leaving two policy questions open for a later call.

## 1. The problem (the elephant)

Brood modules today (ADR-019) are **Emacs-flat**: `provide`/`require`/`*load-path*`
over **one shared mutable global table per runtime**. A module is just a `.blsp`
file that `def`s into root; `defmodule` records a docstring + a feature name and
creates **no scope**. Names collide in one global table; the last `require` wins.

That is exactly right for first-party editor code — the whole project exists to
host a self-editing editor *defined* by an openly-redefinable global namespace
(advice, monkey-patching, live redefinition; ADR-013 hot reload is the
Brood-native form of it). It becomes a problem on four fronts at once:

1. **Package collisions (ADR-037).** The package manager loads third-party
   `name = URL` packages into the one flat table. Two packages that both
   `def parse`, or one that shadows the prelude's `map`, silently clobber. The
   package manager is unsafe to ship without an answer here. *(This is the
   pressure that forces the issue.)*
2. **First-party `std/` crowding.** Even our own modules (`buffer`, `display`,
   `http`, `mcp`, …) share one flat namespace; names get noisy and
   collision-prone internally.
3. **Editor plugins (M2+).** Modes / highlighters / plugins from many authors
   must coexist. This is an ecosystem-shape decision you don't want to walk back.
4. **Ergonomics + tooling.** Qualified names (`text/insert`) read better, *and*
   they're what the LSP needs for completion, cross-file discovery, and rename
   (§6).

## 2. The key reframe: soft privacy keeps the grain

"Namespaces" is two different languages. Surveying the Lisps:

| Lisp | Unit | Privacy | Redefinable live? | Auto-load on reference? |
|---|---|---|---|---|
| **Common Lisp** | package partitions the symbol interner; `pkg:sym` / `pkg::sym` | **soft** (`::` always reaches internals) | **yes** | no |
| **Clojure** | namespace maps symbols→vars; `ns/sym` | **soft** (`^:private` is convention; `#'ns/sym` bypasses) | **yes** (the REPL workflow) | no — unloaded `foo/bar` errors |
| **Racket** | module, statically linked | **hard** (unexported = invisible, sealed) | **no** (sealed) | no |
| **Guile** | module `(a b)` ↔ file `a/b.scm` | `#:export`, soft | yes | **yes** (`use-modules` maps name→path) |
| **Emacs Lisp** | flat + `foo-` prefix convention | none (convention) | **yes** | **yes** (`autoload`: a reference loads the file) |

The decisive observation: **Clojure and CL are namespaced *and* openly
redefinable.** The only Lisp that is *not* live-redefinable is **Racket** — and
that's exactly the one with *hard* privacy (truly invisible unexported names).
**Sealing and hot reload are the same trade-off seen from two sides.** ADR-019's
worry that "namespaces fight open redefinition" is true *only* of the Racket end.

So Brood takes the **Clojure/CL position: namespaced, with *soft* privacy.**
"Private" means *not auto-imported + `--` convention + a checker lint* — **not**
*erased from the runtime*. `observer/observe--internal` stays addressable by its
full name (like CL's `::`), so any code can still reach and live-redefine it. That
preserves the property the editor is built on; we never add Racket-style sealing.

## 3. The substrate: expand-time resolution over the flat table

The enabling fact: **`/` is already a legal symbol character** (`syntax/atom.rs`
`is_delimiter` excludes it), and global lookup is just "find the full symbol." So
`text/insert` is *already one interned symbol* that defines and calls correctly
today with zero core change.

We therefore implement **the entire Clojure/CL surface as an expand-time rewrite
over the existing flat table** — the core never grows a namespace axis:

- `(ns observer …)` sets the **current namespace** — a **per-process `Heap`
  field** (`compile_ns: Option<Symbol>`, set by the `%in-ns` primitive the `ns`
  macro emits), *not* a shared global. A global would race across green processes
  (`RuntimeCode` is shared); the per-process field mirrors the existing
  `current_file` slot and `dynamics` stack. File/module loaders (`load`,
  `%load-string`, `eval_source`) reset it to root per file and restore the
  caller's after; the interactive `eval-string` path leaves it **sticky** so a
  REPL `(ns foo)` persists across entries. One `ns` per file (inc-1).
- Inside it, `(defn observe …)` defines the full symbol **`observer/observe`** in
  the one shared global table.
- A **resolver pass** maps reference-position symbols at expand time:
  `observe` → `observer/observe`; imported names via the import/alias table;
  anything unresolved falls through to the **root namespace** (prelude/core),
  which is always visible unqualified.
- The **runtime is unchanged**: still flat interned symbols in one table.
  `def`-rebinding, ADR-013 cross-process hot reload, `send`/promote/freeze, the
  tracing GC — all untouched, because resolution already happened and produced a
  plain global symbol that is late-bound in the table. You can still
  `(def observer/observe …)` live from anywhere.

This buys the *surface* of first-class per-file namespaces (`ns` forms, qualified
names, import/refer lists, soft privacy, auto-require) on a *flat* substrate. The
one thing it deliberately can't do is *hard* sealing — which §2 says we don't want.

### Resolution rules (sketch)

- A symbol that **already contains `/`** is fully-qualified — taken as-is, never
  re-prefixed (so `(def observer/observe …)` from outside works; matches Clojure).
- A bare symbol resolves in order: **(1)** local lexical binding (unchanged —
  resolution only touches *free* references; the resolver tracks `let`/`let*`/
  `letrec`/`fn` binders and over-approximates `match*` pattern binders), **(2)**
  an imported/`:refer`'d name *(inc-2 — not yet)*, **(3)** ns-qualified
  (`observe` → `observer/observe`) if such a global **already exists** *or* the
  name was **pre-scanned** as a def head this file will create (the forward-ref
  pre-scan — without it a reference to a later definition would silently stay
  bare), **(4)** root/prelude global, **(5)** left bare (an unbound-global
  diagnostic, as today).
- **Quoted / data symbols are never rewritten** (§5).
- **Safety invariant:** never rewrite a binder/param/pattern position. Over-
  qualifying a local is a *silent* miscompile; under-qualifying a free reference
  is at worst a loud unbound error — so the resolver errs toward leaving bare.
- The advisory **checker is ns-aware**: `check_file` resolves under the file's
  `(ns …)` so qualified definitions and references are analysed consistently
  (no false "unbound `foo/bar`"). Def-sites (`source-location`) key on the
  qualified name. Implemented in `eval/macros.rs` (`resolve`/`compile`),
  `core/heap.rs` (`compile_ns`, `def_form_name`), and the loaders.

### Rejected alternative: partition the interner (CL-style)

Making `Value::Sym` carry `(ns, name)` in `value.rs` is the "more correct" model,
but it touches `value.rs`, the reader, `eval`/env resolution, `RuntimeCode`
re-keying, `send`/promote re-intern across runtimes, *and* the hot-reload path —
the large core expansion ADR-019 spent its rationale arguing against, for a
result the flat-substrate model already delivers at the surface. Not chosen.

## 4. One shared resolver, used by both eval and the LSP

The resolver is a **distinct stage** (after read, threaded through
`eval/macros.rs`'s compile pass), given `*ns*` + the import table, mapping a
reference symbol to its qualified global. The **evaluator** runs it to produce
runtime symbols; the **LSP** runs the *same* pass to answer "what does this
symbol mean here." **Single source of truth for resolution** — so the editor can
never disagree with the runtime. This is worth more than any individual feature;
it's what keeps a self-editing editor honest.

Design constraint that falls out: the `ns` / `:use` / `:refer` forms must be
**analyzable as plain data from the tooling CST** (`syntax/cst.rs`, `scope.rs`)
without evaluating — they are (just keyworded forms), so the LSP reads scope
statically even though the rewrite is expand-time.

## 5. Correctness line: data symbols are inviolate

The resolver rewrites **only resolved variable/operator positions**, *never*
`quote`d content. `'observe` as a map key, a `receive` pattern tag, or a message
protocol atom is **data**; rewriting it to `observer/observe` would silently break
cross-process protocols — recall symbols travel **by name** and re-intern across
runtimes (ADR-034). Reflective escape hatches (`resolve`, a computed
`(str ns "/" name)`, `apply` of a computed symbol) bypass the resolver and look
up the full name at runtime. Drawing this boundary precisely (against the existing
`quote`/`quasiquote`-are-opaque handling in `macros.rs`) is fiddly but
non-negotiable.

## 6. Namespaces *are* the LSP feature

Everything LSP Tier 2 wants is blocked by flatness and unlocked by namespaces,
*provided* the `ns`/import surface stays statically readable (§4):

- **Completion** — `(ns dash (:use [observer :refer [observe]]))` declares the
  in-scope set; `observer/` completes that namespace's exports. Flat can only
  honestly offer "every global in the image."
- **Cross-file go-to-def** — `observer/observe` deterministically names the file
  with `(ns observer)` and the `def` of `observe`. The LSP builds a
  `namespace → file` index by scanning `ns` forms (cheap, no eval). (ADR-019 /
  decisions.md noted the flat model can't group defs by module.)
- **Rename** — qualified names make rename *sound*: only references that resolve
  to `observer/observe` change, not every `observe` in the image.
- **Subsumes shadow tooling.** The current cross-file flat-namespace-collision
  warnings (`std/mcp.blsp` `mcp--shadows-for`, the `nest mcp` `load` `:shadows`)
  become ns-aware — a same-name def in a different ns is no longer a collision.

## 7. Macro hygiene — the two concerns, and where each is solved

"Hygiene" is two distinct problems, and namespacing only forces one:

- **Concern #2 — introduced-binding capture** (a macro's `tmp` capturing the
  caller's `tmp`). Pre-existing, *independent* of namespaces. **DONE** — solved by
  Clojure-style **auto-gensym `x#`** (ADR-066): a literal template symbol ending in
  `#` becomes a fresh, per-expansion-consistent gensym, so the binder is
  uncapturable both directions without a manual `(gensym)`. Landed ahead of
  namespacing so it's not entangled with it; the advisory hygiene lint
  (`types/check/hygiene.rs`) now treats a `#`-binder as safe.
- **Concern #1 — free-reference transparency** (a template's `helper` / `map`
  resolving to the *definition* site's binding, not the use site's). This is the
  one namespacing creates, and it's the open question below.

### OPEN — free-reference resolution forces a quasiquote decision

Binding capture (#2) is handled (auto-gensym, above). But **free** references in a
macro template are still resolved as plain symbols, and with use-site expand-time
rewriting that breaks across namespaces:

```clojure
(ns a)
(defn helper (x) ...)
(defmacro m (x) `(helper ~x))   ; emits bare (helper …)

(ns b (:use a))
(m 5)   ; output (helper 5) — resolved in b → b/helper?! wrong / unbound.
```

This is precisely what Clojure's syntax-quote solves by **auto-qualifying
template symbols to the macro's *defining* namespace** (`` `helper `` reads as
`a/helper` at definition time), so macro output is already correct and needs no
use-site resolution.

- **Option α — Clojure-style auto-qualifying quasiquote.** Make `quasiquote`
  qualify reference-position symbols to the current `*ns*`, with an escape (`~'foo`
  → emit a bare symbol). Macros become robust across namespaces for free; gensym
  shrinks to true fresh locals. **Cost:** changes what `` `foo `` *means*
  (now `ns/foo`); every `std/` template must be audited (most reference prelude /
  root names, which stay reachable unqualified). Also fixes resolution *order*:
  syntax-quote resolves at macro-body expand time, so the use-site pass only
  handles names the author wrote bare — clean.
- **Option β — stay unhygienic.** Quasiquote emits bare names; macro authors
  hand-qualify cross-ns refs (`` `(a/helper ~x) ``). Zero quasiquote change, but
  every cross-ns macro becomes a latent capture bug — exactly when packages
  (multi-ns) arrive.

**Lean: α**, *because* we're shipping packages — third-party macros expanding in
your namespaces is the common case, and β makes each one a sharp edge. α is the
larger semantic change of the two namespacing pieces; it interacts with the
ADR-064 "quasiquote → Brood" deferred refactor and must be decided deliberately.
Note α is *only* concern #1 (qualify free refs at def time) — concern #2
(auto-gensym) is already done (ADR-066), so α is narrower than it first appears.
**Left open.**

## 8. OPEN — namespace-name collision moves up a level

Namespacing solves *symbol* collision but creates a *new* one: two packages can
both declare `(ns parser)`. Prior art: Clojure uses reverse-domain
(`com.foo.parser`); CL has no real answer; ADR-037's `name = URL` gives each dep a
**local name** the importing project controls.

- **Free-for-all ns names** — short (`parser`), collision-prone across packages.
- **Package-prefixed** — the dep's local manifest name becomes a mandatory ns
  prefix (the root project disambiguates two `parser`s by their `[name …]`),
  safe but verbose.

`name = URL` packages make this concrete, not hypothetical. **Left open** — it's a
policy choice that doesn't block the substrate (§3) and is best decided against
the package manager's real shape.

## 9. Auto-require

Your `(observer/observe …)` → auto-load idea has precedent: Emacs `autoload` (a
reference loads the file) and Guile (module name ↔ file path). Two flavours:

- **Import-driven** (Guile-ish) — `(ns … (:use observer))` loads `observer` then.
  Explicit-ish; plays well with the lock file.
- **Reference-driven** (Emacs autoload) — a bare `observer/observe` with no import
  loads on first sight. Maximally convenient; couples symbol resolution to
  filesystem side effects.

**Firm line either way:** auto-require **resolves + loads from the load-path; it
never *fetches* a new package.** ADR-037 keeps deps explicit in `project.blsp` so
the lock file stays computable. Auto-require collapses `require`+`use` for code you
*already have* — nothing more. (Flavour choice can ride along with §8.)

## 10. Migration gradient

- **Prelude = the root namespace** — always visible, unqualified (`map`, `+`,
  `cons`). The ergonomic macros used bare everywhere — `describe` / `test` / `is`
  (`std/test.blsp`), `cond`, `when`, … — stay root. Which std *macros* earn a
  root home vs. a prefix is a per-name call.
- **`defmodule` evolves into `ns`.** It already takes name + optional docstring;
  it grows `:use`/`:refer`/`:export` and sets `*ns*`. `provide`/`require`/
  `*load-path*`/`*features*` become the loader underneath auto-require — not
  replaced.
- **std modules** get namespaced gradually; **package/user code** is namespaced
  from birth. Greenfield (CLAUDE.md): rename call sites freely, no compat shims.
- **Side benefit:** the doc tool's "can't tell which module a def belongs to" gap
  (decisions.md) closes — the prefix groups defs.

## 11. Phased implementation

1. ✅ **Resolver pass + `ns` form (inc-1)** — current namespace as a per-process
   `Heap.compile_ns` (not a defdyn — a shared global would race across green
   processes); the §3 resolution rules over *non-macro* references; forward-ref
   pre-scan; def-site keying; ns-aware checker. β-interim for macros (§7). Tested
   incl. the mandatory cross-process round-trip.
2. ✅ **Imports + auto-require (inc-2)** — `(:use mod)` / `(:use mod :refer [a b])`
   in the `ns` header; a per-file `imports` table on the `Heap` (bare → qualified)
   the resolver consults after the current namespace, before root; `%refer`
   enumerates a module's public (non-`--`) names or a subset; `:use` emits a
   `(require …)` so it auto-loads (loads-but-never-fetches, §9). Own-namespace defs
   shadow imports. Tested: refer-all, subset, private excluded, own-ns precedence,
   cross-process.
3. ⬜ **Unify `defmodule` = namespace; migrate `std/`** — make `defmodule` the one
   namespace form, drop `ns`; migrate std module-by-module (start with a leaf like
   `set`), namespace `test` + add `(:use test)` to the 42 test files; update the
   formatter/docs/scaffold tooling that hard-codes `defmodule`. Make the checker
   *import*-aware (eval the `(:use …)` header, or statically populate the import
   table) so imported names stop drawing advisory unbound warnings.
4. ⬜ **Hygiene — α (§7)** — auto-qualifying quasiquote + `~'` escape + `std/`
   macro audit; coordinate with the ADR-064 quasiquote-to-Brood refactor. Needed
   once std *macros* live in namespaces and are used across them.
5. ⬜ **LSP Tier 2 (§6)** — ns→file index, scoped completion, go-to-def, rename,
   over the *shared* resolver (§4); make `mcp--shadows-for` ns-aware.
6. ⬜ **Package integration (§8)** — ns-name disambiguation against ADR-037.

## 12. Explicitly *not* doing

- **No hard privacy / sealing.** Soft only (§2). Unexported names stay reachable
  by full qualified name; the checker may *lint* `--` cross-ns use, never block it.
- **No interner partition.** Symbols stay flat interned `u32` of the full string
  (§3 rejected alternative).
- **No constraint solver / registry for ns names** beyond ADR-037's existing
  direct-ref model.
