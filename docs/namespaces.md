# Namespaces — design

> **Status:** increments 1–3 + α landed (2026-05-30). Inc-1: the resolution substrate
> (resolver pass, forward-ref pre-scan, def-site keying, ns-aware checker). Inc-2:
> **`(:use …)` imports + auto-require** — `(:use mod)` refers a module's public names
> bare, `(:use mod :only [a b])` a subset; the resolver consults the per-file import
> table after the current namespace and before root. Inc-3 (**the big-bang**):
> `defmodule` **is** the single namespace form — `ns` was dropped, a module *is* a
> namespace — and all of `std/` + every test file were migrated in one pass
> (leaf-out, with `test` itself namespaced + `(:use test)` added). **α** (§7) shipped
> in the same pass: the quasiquote walker auto-qualifies free template references to
> the defining namespace, so macros are robust across namespaces without hand-
> qualifying. Decisions in [ADR-065](decisions.md) (namespaces) and
> [ADR-066](decisions.md) (auto-gensym). Supersedes [ADR-019](decisions.md).
>
> **LSP ns-awareness landed (2026-05-30, §6):** a shared resolution seam
> (`eval::macros::resolve_reference` + `introspect::resolve_in_source`/`file_imports`)
> lets the LSP resolve a symbol against a file's namespace + `(:use …)` imports
> exactly as the runtime does — so goto/hover/signature reach qualified + imported
> names, completion offers imported names bare, and **project references/rename are
> namespace-sound** (only occurrences resolving to the *same* qualified global are
> touched). **Package ns-collision policy decided** in [ADR-070](decisions.md):
> flat names + detect-and-reject at dependency-resolution time. **Implemented
> 2026-08-02 (ADR-070):** namespaces are now **package-rooted** — a dependency's modules
> load under a `pkg/…` prefix — so cross-dependency collisions are structurally impossible
> and detect-and-reject narrowed to a within-project check. **Ambient names** — the ones that stay bare/root,
> never namespaced — are those *declared* with `defdyn` (ADR-151, superseding the
> original earmuff-spelling rule), which keeps `defdyn` knobs reachable unqualified
> from any ns. Prelude registries (`*load-path*`, `*features*`, `*module-docs*`) are
> plain root globals with root setters (`set-load-path!`, `record-module-doc!`).
>
> **Fully complete.** The former cosmetic remainders all landed:
> namespace-qualified workspace symbols (ns as container), semantic-token ns
> coloring (a `NAMESPACE` token splitting `ns/name`), and namespace-sound
> cross-file shadow detection (`project--duplicate-def-warnings` now groups by
> resolved `ns/name`, so a short name reused across distinct namespaces no longer
> false-flags). The per-file document outline stays bare by design.
>
> **Since 2026-05-30, later work extended this design** (patched into the sections below,
> not this summary): **ADR-070** package-rooting (2026-08-02); **ADR-146** *enforced*
> def-site privacy — `defn-`/`def-` replace the old `--`-in-name convention, and a
> cross-namespace private reference is now a **hard load-time error**, not an advisory lint;
> **ADR-223** multiple modules per file (the region model); and the **ADR-227** stdlib
> namespacing + **qualified-reference auto-require** (a qualified `mod/name` auto-infers its
> `(require 'mod)`). Where a section below still describes the pre-2026-05-30 model, the
> per-section callouts are authoritative.

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
"Private" means *declared private at the definition + not auto-imported* — **not**
*erased from the runtime*. `observer/observe-internal` (defined with `defn-`) stays
addressable by its full name (like CL's `::`), so any code can still reach and
live-redefine it. That preserves the property the editor is built on; we never add
Racket-style sealing.

> **Superseded (ADR-146): privacy is *enforced*, and declared with `defn-`/`def-`.**
> The original plan (below, and §12) was "advisory lint, never block"; step 1
> (2026-07-23) made it enforced; **step 2 (2026-08-05) moved the marker off the name
> onto the def form** — `defn-`/`def-` define a clean private name (the old `--`-in-name
> convention is gone). A hand-written cross-namespace qualified reference to another
> module's private is a **compile error at load** (`enforce_private_refs` in
> `eval/macros.rs`, judged against the recorded `is_private`), with `(:use-internals
> mod)` as the explicit grant (the `@testable` seam) and top-level/REPL code exempt (the
> live-hacking hatch). `(:use-internals mod :only [a b])` grants access and refers just
> those names instead of a refer-all — the **cycle-safe** form for two mutually dependent
> modules, since a plain refer-all into a still-loading module mid-cycle is rejected by
> the loader while a subset refer resolves lazily. The name is still *reachable at runtime
> by full qualified name* from a granted or same-module site — stronger *soft* privacy, no
> Racket-style erasure. The "auto-imported + `--` + lint" wording in this section and §12
> is the old model.
>
> **One accepted behaviour change from step 2:** a qualified reference into a
> **never-loaded** module can no longer be judged private — the record has never seen it
> — so it degrades from a load-time "module-private" error to an ordinary **unbound
> reference** at call time. More principled: you cannot assert privacy about code the
> image has not loaded.
>
> **Largely superseded by the ADR-227 follow-up (2026-08-14, `a57cc573`):** a qualified
> reference `mod/name` now **auto-requires** `mod` at compile time (`eval/derive.rs`), so a
> reference into a *findable* never-loaded module now **loads** it — the name binds and
> privacy can again be judged. Only a genuinely absent module (or a load cycle) still falls
> through to the unbound reference above; the load is best-effort so an inferred require
> never turns a reference into a compile error.

## 3. The substrate: expand-time resolution over the flat table

The enabling fact: **`/` is already a legal symbol character** (`syntax/atom.rs`
`is_delimiter` excludes it), and global lookup is just "find the full symbol." So
`text/insert` is *already one interned symbol* that defines and calls correctly
today with zero core change.

We therefore implement **the entire Clojure/CL surface as an expand-time rewrite
over the existing flat table** — the core never grows a namespace axis:

- `(defmodule observer …)` sets the **current namespace** — a **per-process `Heap`
  field** (`compile_ns: Option<Symbol>`, set by the `%in-ns` primitive the
  `defmodule` macro emits), *not* a shared global. A global would race across green
  processes (`RuntimeCode` is shared); the per-process field mirrors the existing
  `current_file` slot and `dynamics` stack. File/module loaders (`load`,
  `%load-string`, `eval_source`) reset it to root per file and restore the
  caller's after; the interactive `eval-string` path leaves it **sticky** so a
  REPL `(defmodule foo)` persists across entries. A file may open **more than one**
  `(defmodule …)` (ADR-223 Phase 1): each opens a *region* running to the next
  `defmodule` or EOF, and a bare reference qualifies against the module it is inside
  (the per-region forward-ref pre-scan, `%in-ns` activating each region). Co-located
  modules do not see each other's bare names — they qualify or `:use`. A single-module
  file is the common case and behaves exactly as before. In a **named** (ADR-070 rooted)
  project a co-located secondary module is also reachable *across* files by its own name
  (`(:use secondary)` / `(require 'pkg/secondary)`) — the file→module scan registers every
  declared module, not just the first (ADR-223 Phase 2 MVP).
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
  resolution only touches *free* references; the resolver tracks `let`/
  `letrec`/`fn` binders and over-approximates `match*` pattern binders), **(2)** an
  **ambient** name (declared with `defdyn`, ADR-151), **(3) own-namespace** —
  ns-qualified (`observe` → `observer/observe`) if such a global **already exists**
  *or* the name was **pre-scanned** as a def head this file will create (the
  forward-ref pre-scan) — **the own namespace resolves *before* imports, so a
  same-named local def shadows a `(:use …)`d one**, **(4)** an imported/`:only`'d
  name, **(5)** root/prelude global, **(6)** left bare (an unbound-global diagnostic).
  A symbol that **already contains `/`** is handled ahead of all these (per the bullet
  above: root-escape `/x`, an `(:alias)` prefix, ADR-070 package-rooting, and the
  ADR-227 qualified-reference auto-require). (Code: `resolve_sym`,
  `crates/lisp/src/eval/macros.rs` — the sketch above simplifies; this is the live order.)
- **Quoted / data symbols are never rewritten** (§5).
- **Safety invariant:** never rewrite a binder/param/pattern position. Over-
  qualifying a local is a *silent* miscompile; under-qualifying a free reference
  is at worst a loud unbound error — so the resolver errs toward leaving bare.
- The advisory **checker is ns-aware**: `check_file` resolves under the file's
  `(ns …)` so qualified definitions and references are analysed consistently
  (no false "unbound `foo/bar`"). Def-sites (`source-location`) key on the
  qualified name. Implemented in `eval/macros.rs` (`resolve`/`compile`),
  `core/heap.rs` (`compile_ns`, `def_form_name`), and the loaders.

### Hierarchical module names (ADR-085)

A module name may itself contain `/`: `(defmodule gui/window)` declares the
namespace `gui/window`, loaded from `gui/window.blsp` (nested under a load-path
dir) or the embedded table keyed on the full `"gui/window"` stem. Its
definitions qualify on the **last** `/` — module `gui/window` + def `draw` is the
single interned symbol `gui/window/draw`. This is the **same flat-table
machinery one level deeper**, not a new axis: a qualified name is still one
symbol, `require--find` already path-joins the stem (`gui/window.blsp` resolves
in a nested dir), and `qualify_name` formats `{ns}/{name}` regardless of how many
segments `ns` has. So resolution, `(:use gui/window)` imports, the cross-process
re-intern, and `nest check` all carry over unchanged.

The only sites that *assumed a single separator* — and were corrected — are the
two that **split** a qualified name back into module + name: the LSP semantic
tokeniser (`semantic_tokens.rs`, now `rfind('/')` so the whole `gui/window` path
colours as `NAMESPACE` and `draw` as the name) and the runtime "did you mean
`(:use …)`" hint (`unbound_namespace_hint`, no longer filtering out multi-segment
modules). The `name.contains('/')` "already qualified?" guards in the resolver
need no change — they're separator-count-agnostic. Covered by the *hierarchical
module names* block in `tests/namespace_test.blsp`.

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

Design constraint that falls out: the `ns` / `:use` / `:only` forms must be
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

- **Completion** — `(ns dash (:use [observer :only [observe]]))` declares the
  in-scope set; `observer/` completes that namespace's exports. Flat can only
  honestly offer "every global in the image."
- **Cross-file go-to-def** — `observer/observe` deterministically names the file
  with `(ns observer)` and the `def` of `observe`. The LSP builds a
  `namespace → file` index by scanning `ns` forms (cheap, no eval). (ADR-019 /
  decisions.md noted the flat model can't group defs by module.)
- **Rename** — qualified names make rename *sound*: only references that resolve
  to `observer/observe` change, not every `observe` in the image.
- **Subsumes shadow tooling.** The current cross-file flat-namespace-collision
  warnings (`std/tool/mcp.blsp` `mcp--shadows-for`, the `nest mcp` `load` `:shadows`)
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

### DONE — α: free-reference resolution via auto-qualifying quasiquote

Binding capture (#2) is handled (auto-gensym, above). The remaining hazard was that
**free** references in a macro template would resolve as plain symbols, and with
use-site expand-time rewriting that breaks across namespaces:

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

**Chosen and shipped: α — Clojure-style auto-qualifying quasiquote.** The resolver
now descends quasiquote templates (`resolve_list` skips only `quote`, not
`quasiquote`) and qualifies reference-position symbols to the *defining* namespace's
`compile_ns` at macro-definition time. So `` `(helper ~x) `` in namespace `a` reads
as `` `(a/helper ~x) ``; the expansion is already correct and the use-site pass only
handles names the author wrote bare. The escape is `` `(quote foo) `` / a plain
`'foo` for a bare data symbol; `~expr` (unquote) is ordinary code and resolves
normally; a **declared-ambient** (`defdyn`) name stays bare. Root/prelude names
referenced from a template stay reachable unqualified (resolution falls through to
root). This was implemented in the same big-bang pass as the migration — without it,
namespaced macros like `test/describe` emitting bare helper calls broke in consumer
namespaces (the β-interim wall). β (hand-qualify every cross-ns ref) was rejected:
with packages shipping third-party macros, it makes every macro a latent capture bug.

## 7b. Two modules, one short name (`:use` of both is an error)

Namespacing means two modules may legitimately export the same *short* name — `sexp` and
`editor/treesit` both offer `point-forward` (over a Brood form, and over a tree-sitter tree
with a `lang`); `format` and `template` both `render`. Importing both bare is refused with
**E0099**, naming the two providers, rather than letting one silently win:

```
(:use editor/treesit) refers `point-forward`, but it is already referred as
`sexp/point-forward` from another module — resolve the clash with `:only [...]`
or `:exclude [...]` on one of the uses
```

Three resolutions, in the order usually wanted:

1. **`require` + qualified calls** — the module is loaded, nothing is imported, no name can
   clash. Best when the call sites want to *say* which one they mean anyway (myedit's
   structural commands call `sexp/…` for Brood source and `editor/treesit/…` for a foreign
   language, one line apart).
2. **`:only [...]`** on one use — import just the names wanted.
3. **`:exclude [...]`** on one use — import everything except the clashing names.

Reach for a rename only when the two names mean the *same* thing; the pairs in std do not.
The full set of std overlaps is enumerated and pinned in `tests/namespace_test.blsp`, so a
new one is a decision at review time rather than a consumer's editor failing to load.

## 8. OPEN — namespace-name collision moves up a level

Namespacing solves *symbol* collision but creates a *new* one: two packages can
both declare `(ns parser)`. Prior art: Clojure uses reverse-domain
(`com.foo.parser`); CL has no real answer; ADR-037's `name = URL` gives each dep a
**local name** the importing project controls.

- **Free-for-all ns names** — short (`parser`), collision-prone across packages.
- **Package-prefixed** — the dep's local manifest name becomes a mandatory ns
  prefix (the root project disambiguates two `parser`s by their `[name …]`),
  safe but verbose.

`name = URL` packages make this concrete, not hypothetical. **Decided
([ADR-070](decisions.md)): flat names + detect-and-reject — now implemented.**
Namespace names stay short and free-for-all; the package manager's
dependency-resolution step (ADR-037 `nest fetch`/`add`/the auto-fetch on every
project subcommand) **errors** if two reachable providers declare the same
namespace, naming both sources, rather than silently merging or taxing every call
site with a mandatory prefix. "Providers" includes **your own project's modules**,
not just deps — a dep that shadows a module you wrote is the same silent clobber
(`require` loads whichever `<name>.blsp` is first on `*load-path*`; the loser never
loads and its dependents bind the wrong module). A provider's namespaces are read
from each source file's `(defmodule …)` name. (`std/tool/package.blsp`
`package--check-namespace-collisions`; tested in `tests/package_test.blsp`.)

The per-dep prefix escape shipped as **ADR-070 package-rooting**; the **import-site alias
shipped too** — as a separate `(:alias mod :as p)` header clause (not the Clojure-style
`[parser :as p]` inside `:use` sketched here), `crates/lisp/src/eval/macros.rs` rewriting the
`p/…` prefix. (Only the *inline* `[… :as …]` spelling stayed deferred, ADR-011.) **Package-rooted
namespaces** (the dep's local name as a load-time prefix, `foo/b/…`, making
collisions *impossible*) is the recorded **future direction**, not a rejection — the
detect-and-reject check is the interim. Crucially it's a *loader* change, not a
*source* one: intra-package refs stay short either way, so a package's source is
identical whether filed under `b/` or `foo/b/`, and rooting can land later (M2
plugin pressure) with the flat form kept working — no package-source churn. Full
analysis in ADR-070's *Future direction*.

> **Update (2026-08-02): package-rooting is now IMPLEMENTED for dependencies** — the
> destination arrived early. A dep `foo`'s `(defmodule b)` loads as the global `foo/b`,
> so two deps can both provide a `parser` with no collision. As predicted it was a pure
> *loader* change (per-process `package_prefix`/`package_modules` on the `Heap`; `%in-ns`
> and the `:use`/`:alias` clause targets root via a `%root-module-name` primitive; the
> package manager registers each dep's modules rooted). The old cross-provider
> detect-and-reject narrows to a **within-project** duplicate check (cross-dep collisions
> are now impossible). The **root project's own modules root too**, under its `:name` (the
> **Elixir-uniform model**), prefix *implied* — a `(defmodule buffer)` in project
> `myeditor` is the global `myeditor/buffer`, and intra-project `(:use buffer)` stays
> short. One mechanism serves both deps and the root project: an ambient package context
> (`project-setup`) that roots `%in-ns` / `(:use)` / `(:alias)` / the `:main` entry / the
> checker's import + require-reachability sites / hot-reload. The two follow-ups —
> LSP nav and multi-dep-collision bundling for rooted projects — have both since landed
> (`nest release` embeds a dep's modules under their rooted key, so two deps sharing a
> module name coexist in one bundle; 2026-08-17). Full status in ADR-070's *Update (2026-08-02)*
> and its 2026-08-17 bundle-rooting update.

> **Further direction — a unified module/symbol index (ADR-223, deferred).** One step
> further along this trajectory: replace the several ad-hoc half-indexes (the `module →
> file` filename bijection, `*package-module-files*`, baked-module keys, the LSP's scans,
> …) with a single auto-built index (symbol → site, module → file, feature → provider,
> ability → impls). It makes symbol-level "where is X" O(1), turns collision/reserved
> checks into index queries, and would unlock *flat* multiple-modules-per-file (not lexical
> nesting). Gated on the same **M2 plugin pressure** as package-rooting; recorded as a
> future direction in [module-index.md](module-index.md), **not built**.

## 9. Auto-require

Your `(observer/observe …)` → auto-load idea has precedent: Emacs `autoload` (a
reference loads the file) and Guile (module name ↔ file path). Two flavours:

- **Import-driven** (Guile-ish) — `(ns … (:use observer))` loads `observer` then.
  Explicit-ish; plays well with the lock file.
- **Reference-driven** (Emacs autoload) — a bare `observer/observe` with no import
  loads on first sight. Maximally convenient; couples symbol resolution to
  filesystem side effects.

**Firm line for import-driven:** auto-require **resolves + loads from the load-path;
it never *fetches* a new package.** ADR-037 keeps deps explicit in `project.blsp` so
the lock file stays computable. Auto-require collapses `require`+`use` for code you
*already have* — nothing more.

> **Decided ([ADR-206](decisions.md), 2026-08-02): import-driven stays; reference-driven
> is rejected as a language feature.** Import-driven auto-require (a `(:use …)` loads its
> module) shipped in inc-2 and is the model. **Reference-driven autoload** (a bare
> `mod/name` autoloading on first sight) is *not* built — it couples resolution to
> filesystem side effects, runs inside `nest check` (shared resolver), and makes a
> module's dependency set implicit, blurring exactly the static analysis §6 says
> namespaces exist to enable. The "referenced-but-not-imported" ergonomics are instead
> solved the modern, analyzable way — an **LSP auto-import code action** ("Import `foo`
> from `(:use mod)`" / "Qualify as `mod/foo`", `crates/lsp/src/code_actions.rs`) that
> *writes the explicit import for you*, like Rust-analyzer / TypeScript. The narrow
> live-image niche where autoload is idiomatic (Emacs) is served by the REPL's
> unbound-name hint; a REPL-only autoload could be added there later, scoped away from
> file loads and `nest check`, but is explicitly not the general mechanism.
>
> **Update (ADR-227 follow-up, 2026-08-14):** a **qualified** `mod/name` reference now DOES
> auto-infer `(require 'mod)` (`crates/lisp/src/eval/derive.rs`) — a narrow, analyzable form
> of reference-driven load: the module is *named at the reference*, so the dependency stays
> explicit and statically visible, and it only ever loads code already on the load-path (never
> fetches). Bare-name autoload — an *unqualified* `foo` conjuring its owning module — remains
> rejected, exactly as argued above.

## 10. Migration gradient

- **Prelude = the root namespace** — always visible, unqualified (`map`, `+`,
  `cons`). The ergonomic macros used bare everywhere — `describe` / `test` / `is`
  (`std/tool/test.blsp`), `cond`, `when`, … — stay root. Which std *macros* earn a
  root home vs. a prefix is a per-name call.
- **`defmodule` *is* the namespace form** (inc-3 dropped `ns`). It takes name +
  optional docstring and understands exactly four header clauses — `(:use …)` (with
  `:only`/`:exclude`), `(:use-internals …)`, `(:alias … :as …)`, and `(:implements …)` —
  anything else is a hard error. (The migration-era `:export` clause was never built;
  visibility is def-site privacy, `defn-`/`def-`, ADR-146.) `provide`/`require`/
  `*load-path*`/`*features*` are the loader underneath — not replaced.
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
2. ✅ **Imports + auto-require (inc-2)** — `(:use mod)` / `(:use mod :only [a b])`
   in the `ns` header; a per-file `imports` table on the `Heap` (bare → qualified)
   the resolver consults after the current namespace, before root; `%refer`
   enumerates a module's public (non-private) names or a subset; `:use` emits a
   `(require …)` so it auto-loads (loads-but-never-fetches, §9). Own-namespace defs
   shadow imports. Tested: refer-all, subset, private excluded, own-ns precedence,
   cross-process.
3. ✅ **Unify `defmodule` = namespace; migrate `std/` (the big-bang).**
   - **Import-aware checker** — `check_file` evals the `(defmodule …)` header so
     `(:use …)` imports populate; imported names no longer draw advisory unbound
     warnings.
   - **`defmodule` *is* the namespace form** — the `ns` macro was renamed to
     `defmodule` and `ns` dropped; `defmodule` parses `(:use …)` clauses, emits
     `%in-ns` + `provide`, and keeps `*module-docs*`. No root `defmodule` remains.
   - **All `std/` migrated leaf-out** — every module is `(defmodule X (:use …))`;
     cross-module references are qualified or imported. `test` itself is namespaced,
     so the 40+ test files declare `(defmodule x-test (:use test) …)`. Special cases
     handled: editor/keymap/dispatch tables hold hand-qualified quoted handler symbols
     (`'lineedit/…`); the `project` manifest is read as **data** (not a namespaced
     macro call); circular `:use` (project↔package) broken via lazy `package/…`.
4. ✅ **Hygiene — α (§7)** — auto-qualifying quasiquote shipped in the big-bang
   (resolver descends quasiquote, qualifies free template refs to the defining ns,
   declared-ambient names stay bare). Coordinates cleanly with the ADR-064 quasiquote-to-Brood
   refactor (resolution is a separate pass over the expanded tree).
5. ✅ **LSP ns-awareness (§6, 2026-05-30)** — the shared resolution seam
   (`macros::resolve_reference` + `introspect::resolve_in_source`/`file_imports`,
   §4) wired into goto-def, hover, signature (resolve a Free symbol to its
   qualified global first), completion (offer `(:use …)` imports bare), and
   **namespace-sound project references/rename** (occurrences are matched by
   *resolved qualified identity*, so a different ns's same-named def is never
   touched). Mid-edit-tolerant (falls back to the CST `defmodule` header when the
   buffer doesn't fully parse). The former cosmetic remainders also landed
   (2026-05-30): namespace-qualified **workspace symbols** (ns as container),
   **semantic-token ns coloring** (a `NAMESPACE` token splitting `ns/name`), and
   **namespace-sound shadow detection** (`mcp--shadows-for` /
   `project--duplicate-def-warnings` group by resolved `ns/name`).
6. ✅ **Package integration policy (§8)** — decided in ADR-070 (flat names +
   detect-and-reject at lock time); enforcement lands with the package manager
   (ADR-037), dormant until then.

## 12. Explicitly *not* doing

- **No hard privacy / sealing at the *interner*.** A private name stays a plain
  interned global, reachable via `(def mod/priv …)` for live hot-patching (§2). But
  privacy **is enforced at compile time** (ADR-146, superseding the original
  advisory-lint plan): a name is declared private at its def site (`defn-`/`def-`,
  not the retired `--`-in-name convention), and a cross-namespace reference to it is
  a **hard load-time error** unless granted by `(:use-internals mod)` — see the §2
  callout. "Soft" means live-redefinable, not unenforced.
- **No interner partition.** Symbols stay flat interned `u32` of the full string
  (§3 rejected alternative).
- **No constraint solver / registry for ns names** beyond ADR-037's existing
  direct-ref model.

## 13. Comparison with other languages

Where Brood's namespace system sits against the field. The first table is the broad
landscape; the second is the set of deliberate choices, each naming its closest
sibling and the alternative it rejected. This reflects the design *as implemented*,
including package-rooting (ADR-070) and LSP auto-import (ADR-206).

| Language | Unit | Substrate | Privacy | Live-redefinable? | Cross-package collisions | Auto-load on bare ref? | Macro hygiene |
|---|---|---|---|---|---|---|---|
| **Brood** | module = namespace = file (`defmodule`) | **flat interned `u32` table; namespacing is an expand-time rewrite (no interner partition)** | **soft but *enforced*** (`defn-`/`def-` def-site privacy; cross-ns private ref is a load error; `:use-internals` grant) | **yes** (`def` rebinds globals; hot reload) | **impossible — package-rooted** (`foo/b`); detect-and-reject was the interim | **no** — rejected (ADR-206); LSP auto-import instead | **auto** (auto-gensym `x#` + auto-qualifying quasiquote) |
| Clojure | namespace maps symbols→vars | var indirection per ns | soft (`^:private`; `#'ns/sym` bypasses) | yes (REPL) | convention only (reverse-domain `com.foo.parser`) | no (unloaded `foo/bar` errors) | auto (syntax-quote qualifies + `x#`) |
| Common Lisp | package partitions the interner | interner partition (`pkg:sym`/`pkg::sym`) | soft (`::` reaches internals) | yes | no real answer (flat package names) | no | none (manual `gensym`) |
| Racket | module, statically linked | static module system | **hard** (unexported invisible) | **no** (sealed) | collections / pkgs | no | full (`syntax-rules`/`syntax-case`) |
| Elixir | module (`defmodule Foo.Bar`), atoms | atoms on the BEAM | `def`/`defp` (defp near-hard) | yes (OTP hot code load) | app / dotted names + Mix | ~yes (code server loads on first call) | hygienic-ish (`var!` escape) |
| Rust | module + crate | static resolution | **hard** (`pub`) | no | **impossible — crate-rooted** (`crate::`) | no | hygienic (`macro_rules!`, `$crate`) |
| Go | package = directory | static | hard (capitalization) | no | import paths are URLs (globally rooted) | no | (no macros) |
| Python | module = file, package = dir | dynamic `sys.modules` objects | soft (`_name`, `__all__`) | messy (`importlib.reload`) | flat PyPI names (collision-prone) | no | (n/a) |
| JS / ESM | module = file | static module graph | hard (unexported invisible) | no (HMR is tooling) | npm scoped (`@scope/pkg`) | no (dynamic `import()` explicit) | (n/a) |

The deliberate choices, each vs its closest sibling and the rejected alternative:

| Question | Brood's choice | Closest sibling | Rejected |
|---|---|---|---|
| Sealing vs hot reload | **Soft privacy + live redefinition** (the same trade-off from two sides) | Clojure / CL | Racket's hard sealing (kills self-editing-editor) |
| Core representation | **Flat interned table + expand-time rewrite** | Emacs-flat, upgraded | CL interner partition (large core growth for the same surface) |
| Import clash | **Hard error; `:only`/`:exclude`** + `/name` root escape + prelude-shadow warning | **Elixir** (`import … except:`, `Kernel.foo`) | Silent last-wins (Python/CL merge) |
| Package collisions | **Package-rooted** (`foo/b`), collisions impossible | **Rust** (`crate::`), Go | Mandatory per-call prefixes, or convention-only (Clojure) |
| Referenced-but-not-imported | **LSP auto-import writes the explicit `(:use …)`** | Rust-analyzer / TS | Runtime autoload (Emacs/Guile) — hides deps from tooling |
| Macro reference transparency | **Auto-qualifying quasiquote** (defining-ns) | Clojure syntax-quote | Hand-qualify every cross-ns ref (a latent capture bug per third-party macro) |

**The one-line read.** Brood is **Clojure/CL semantics** (namespaced *and*
live-redefinable, soft privacy) on an **Emacs-flat substrate** (no interner
partition), with **modern ergonomics layered on top** — Elixir's import-clash
discipline, Rust's package-rooting, and Rust-analyzer-style auto-import instead of
legacy autoload. The distinctive combination is *live-redefinable +
collision-proof + statically-analyzable at once*: Rust has rooting + analyzability
but no live reload; Clojure has live + soft privacy but convention-only collisions;
no single other language gives all three.
