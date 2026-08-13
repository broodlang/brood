# A unified module/symbol index — design

> **Status:** the **index itself is proposed, not built** — a recorded *future direction*,
> deferred under ADR-011 (defer power features until a concrete need justifies them). The
> concrete trigger is **M2+ plugin pressure** (many modules from many authors, an editor that
> navigates by symbol), the same trigger `namespaces.md` §8 names for package-rooting. This
> doc captures the design so it is shovel-ready when that pressure arrives; it does **not**
> authorize building the index now. Backing decision: [ADR-223](decisions.md).
>
> **Update (2026-08-12): the per-region pre-scan and *intra-file* multiple-modules-per-file
> shipped independently of the index (ADR-223 Phase 1).** It turned out the addressing index
> is only needed to `require` a co-located module *by an arbitrary name across files* — within
> one file every module loads together, so multi-module-per-file needs only the per-region
> forward-ref pre-scan (§6), not the index. That pre-scan is now live (`macros::scan_regions`
> → `Heap::ns_known_by_module`; `%in-ns` activates a region), so §6-in-scope and §7-step-5
> below are *partly done* and the §7 "loud load error" interim no longer applies. What remains
> deferred here is the index proper (module ↔ file decoupling, require-by-name).
>
> **Update (2026-08-13): cross-file require-by-name shipped for named projects (ADR-223 Phase 2
> MVP), still without the persisted index.** §7-step-5's payoff — reaching a co-located
> secondary module across files by its own name — reduced for the root project to one scan
> generalization (`package/package-module-files` now records every `(defmodule …)`, not just
> the first), which feeds the existing ADR-070 rooting registry (`*package-module-files*`) that
> `require` already consults. So the *addressing* the index was meant to provide is, for the
> in-project named case, already covered by the rooting registry. What the persisted index still
> adds on top: module↔file decoupling beyond rooting, cross-*author* O(1) symbol location, LSP
> routing, and folding the collision/reserved/duplicate checks into index queries — all still
> deferred to M2 plugin pressure.
>
> **Update (2026-08-13, Phase 2b): nameless projects and bare `brood file.blsp` runs now reach a
> co-located module by name too** — not via a registry but via a `*load-path*`-scan fallback in
> `require` (only on a filename-probe miss). So require-by-name for a co-located module is
> complete across named/nameless/bare; what remains deferred here is still the *persisted index*
> (cross-author O(1) symbol location, LSP routing, checks-as-queries).

This is the design backing for a single, authoritative **index** that answers
"where/what is X" for the whole image — module → file, symbol → definition site,
feature → provider, ability → implementations — replacing the several ad-hoc
half-indexes the runtime and tooling each maintain today.

## 1. The problem: six half-indexes and a filename bijection

Resolution and location are re-derived in many places, each answering one slice of
"where does this name live":

- **Filename bijection** — `module foo → foo.blsp`, resolved by `require-force-in`
  (`std/prelude.blsp`) walking `*load-path*` with `file-exists?` probes. Simple, always
  fresh, human-computable — but it *is* why only one module may live per file (the name
  is the address, so a second module in a file has no file of its own name to be found
  by).
- **`*package-module-files*`** (`std/prelude.blsp`) — a rooted module name (`"foo/b"`) →
  its `.blsp` path. Deps and the root project's own rooted modules already resolve by
  *index*, not by filename.
- **`%builtin-module`** — baked-in std modules, keyed by module name → embedded source.
  A *build-time* index already (the binary is its artifact directory).
- **`*features*`** / **`*require-edges*`** (`std/prelude.blsp`) — runtime load-state: what
  is loaded, and the require-edge graph used to materialize an image.
- **The startup image** — an imaged prelude + modules with a manifest of what it holds.
- **The incremental `nest check` cache** (ADR-129) — check results keyed by file.
- **`*record-ids*`** — record-constructor id registry the `BROOD_MONO` devirtualizer reads
  to prove an ability call site.
- **The LSP** re-derives symbol → site and `namespace → file` by scanning `defmodule`
  forms and `source-location` records on demand (`introspect::resolve_in_source`,
  `file_imports`).

Each is partial and locally maintained. The costs: (a) the same "where is X" is computed
several ways, risking drift between *what the editor thinks* and *what the runtime does* —
the exact consistency `namespaces.md` §4 calls "what keeps a self-editing editor honest";
(b) symbol-level "where is `parse-headers`?" requires a scan or a load; (c) the filename
bijection forecloses multiple modules per file (see `namespaces.md` §8 and the pre-scan
fix in `docs/devlog.md` 2026-08-12).

## 2. Prior art: Elixir makes the artifact layout *be* the index

The instructive precedent. In that ecosystem a source file holds one or many module
definitions; the **compiler emits one artifact per module, named after the module**
(`Elixir.Foo.Bar.beam`). At runtime a module reference resolves by searching a code path
for the artifact whose *name is the module name* — the filename convention survives, it
just moved down to the compiled artifact, where the compiler guarantees `name == filename`
regardless of how the source was arranged. A build manifest records source → modules →
dependency edges for incremental recompilation, and protocol *consolidation* flattens all
known implementations into one build-time dispatch table.

Two lessons carry over:

1. **Don't hand-maintain an index — make the build emit name-keyed per-module entries and
   let that be the index.** Brood already does exactly this for std (`%builtin-module`) and
   deps (`*package-module-files*`); the unified index finishes the pattern for root sources.
2. **Even a system that allows many modules per file treats *nesting* as name-sugar, not
   scope.** A nested module there is merely name-prefixed; it does **not** see the outer
   module's private functions. This validates the scope decision in §6: flat multi-module,
   never lexical submodules.

## 3. The index

One artifact — logically a map — built by the toolchain and consulted by every "where/what"
query. Proposed schema (illustrative, not final):

```
;; symbol → its definition
name  → { :module  "net/http"        ; the qualified module it belongs to
          :file    "std/net/http.blsp"
          :line    128
          :private false             ; ADR-146 recorded fact
          :kind    :fn }             ; :fn | :macro | :def | :record | :ability | :impl

module   → { :file "…" :provides [names…] :uses [modules…] }
feature  → provider-module            ; subsumes the module→file question
ability  → [impl-module…]             ; the consolidated dispatch view (BROOD_MONO)
```

Three properties make it safe and useful:

- **Auto-built, never hand-written.** Produced as a byproduct of the build / `nest check`
  (which is already incremental, ADR-129) and **mtime-invalidated** per file, so it tracks
  the source without a manual step.
- **A cache of live truth, never the truth** — the load-bearing invariant (§5).
- **Static structure only.** It records *where things are defined and what a file
  provides*; it does **not** own runtime load-state. `*features*` / `*require-edges*` stay
  as they are and simply read locations from the index instead of re-deriving them.

## 4. What it unlocks

- **Symbol-level find with no load / no scan.** "Where is `parse-headers`?" is O(1).
  Direct substrate for the ADR-206 auto-import code action, O(1) cross-file go-to-def, and
  turning the "did you mean `(:use net/http)`?" hint from a heuristic into a lookup.
- **Collision / reserved checks fall out for free.** ADR-070 detect-and-reject *is* an
  index build — a duplicate key is the collision. Reserved-name enforcement, prelude-shadow
  warnings, and the duplicate-def pass become index queries rather than separate passes.
- **Ability → impl and feature → provider indexing.** Powers "show all implementations",
  dead-module detection, and — notably — makes `BROOD_MONO` devirtualization a *default*
  rather than an opt-in, since "all impls of this ability" is exactly what it needs.
- **Whole-program queries get cheap.** Every public symbol, every module that `:use`s X,
  modules never required, the full API surface for `nest doc` — index scans, not
  load-the-world.
- **Faster cold `require` + a smarter image.** O(1) module → file beats probing
  `*load-path*`; the startup image gains a precise manifest of what it holds.
- **Module ↔ file decoupling.** The name becomes stable and the file an implementation
  detail: rename/split/merge files without touching module names or call sites, and —
  the original thread — **multiple modules per file** (§6).

## 5. The one invariant: a cache of live truth, never the truth

Brood is a live, hot-reloadable, self-editing system. A stale index that *misdirects* is
worse than no index. So:

- The index **auto-refreshes** (on save / as a `nest check` byproduct / mtime-invalidated).
- Every consumer **falls back to live scanning** when the index is absent or stale — the
  LSP already needs this for unsaved buffers, so the index sits *over* the live path as an
  optimization, not instead of it.
- The **flat interned symbol table stays the runtime truth.** Resolution semantics
  (ADR-065) do not change; the index accelerates *lookup*, it never changes *meaning*. This
  is why the first increment needs no new ADR about semantics.

Get this right and the index is pure upside; get it wrong and it silently sends tools to
the wrong file — which is the whole risk, and why it must never be authoritative.

## 6. Scope: flat multi-module, not lexical nesting

An index solves **addressing** (find the file), not **scoping** (per-module known-names,
visibility). Kept separate deliberately:

- **In scope (what the index enables):** *flat* multiple modules per file, each
  independently addressable by name — the Elixir model. Requires the resolver's
  forward-reference pre-scan to become **per-module-region** (partition a file's forms by
  `defmodule` boundary; each module's known-names = the defs in its region). **This
  pre-scan landed (ADR-223 Phase 1, 2026-08-12)** as `macros::scan_regions` →
  `Heap::ns_known_by_module`, generalizing the pre-module boundary fix — so *intra-file*
  multiple modules already work. What the *index* still adds on top is addressing a
  co-located module **by an arbitrary name across files** (require-by-name); the pre-scan
  alone does not decouple module from file.
- **Out of scope (explicitly not doing):** **lexical submodules** where an inner module
  sees an outer's privates. That needs a `compile_ns` *stack* and a hierarchical privacy
  rule — real core cost for a capability the hierarchical-name model (`gui/window`,
  ADR-085) + `defn-` already cover at the surface, and which even the Elixir precedent
  declines (§2, lesson 2). Revisit only on a concrete driver.

## 7. Migration (when built)

Incremental, one consumer at a time, each behind the live-scan fallback so nothing regresses
mid-migration:

1. **Emit the artifact** from `nest check` / the build; mtime-invalidate per file. No
   consumer reads it yet — verify it matches the live scan.
2. **Route the LSP** resolution + completion + go-to-def through it (fallback intact).
3. **Route `require`'s cold module → file** lookup through it (filename probe as fallback).
4. **Fold in the checks** — collision/reserved/duplicate-def become index queries.
5. The module ↔ file decoupling — **require-by-name** for a co-located module whose name is
   not its file's. (The per-region pre-scan from §6 and *intra-file* multiple-modules-per-file
   already shipped ahead of the index in ADR-223 Phase 1; only cross-file addressing by
   arbitrary name still waits on the index.)

Ordering matters for the remaining decoupling: build the index *first* and prove it against
live truth; do **not** decouple module from file before the index that makes that lookup cheap
exists. (Superseded for the intra-file case: a second `defmodule` in one file is now supported
directly via the region model — no longer a load error — because within one file every module
loads together and the per-region pre-scan resolves each bare reference against its own module.)

## 8. Why deferred, and the trigger to build

- **No concrete driver yet.** The namespace bug hunt (`docs/devlog.md` 2026-08-12) found the
  existing resolution/privacy/collision machinery *robust* — no drift bugs surfaced. The
  advantages above are real but shaped like "faster / more unified", which ADR-011 says to
  defer.
- **Scale isn't there.** The root project is a handful of files today; the editor app is a
  separate downstream project. The filename convention is not a bottleneck at this size.
- **Risk.** It touches load-bearing, concurrency-sensitive machinery (`require`,
  `*features*`, the image, the check cache) that currently works.

**Build it when** M2+ brings many modules from many authors and an editor that navigates by
symbol — the point at which O(1) symbol location, cross-author collision safety, and
file/module decoupling stop being conveniences and become load-bearing. At that point this
doc is the plan; §7 is the order.
