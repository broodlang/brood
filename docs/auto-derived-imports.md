# Auto-derived stdlib imports — design & build handoff

**Status: SHIPPED (`a57cc573`, 2026-08-14) — but as _qualified-reference auto-require_,
which is NOT the bare-name "Design B" the rest of this document plans.** The mechanism
below (defer the load past the GC/macro-blocked resolver, drain it in the compile driver)
is what shipped; the *trigger* changed. Read "What actually shipped" next; everything from
"The goal" onward is the **superseded original plan**, kept for the design reasoning.

## What actually shipped (read this first)

A **qualified** reference `mod/name` infers `(require 'mod)` — you never write a `require`
line just to satisfy a `mod/…` reference, for **any** module (`seq`/`map`/`math`, `json`,
`set`, your own project modules), not just a curated three. **There is no bare-name magic:**
a bare `sqrt` with neither a `math/` prefix nor `(:use math)` stays unbound. `(:use mod)`
still refers a module's names bare and needs no separate require either. The rule is one
line: *name where something comes from, and it loads on demand.*

Implementation: `crates/lisp/src/eval/derive.rs`, wired into `compile` (`macros.rs`) via
three hooks, each firing only on a `/` in a symbol — `require_qualified_head` (eager, from
`macroexpand_1`, so a qualified **macro** head loads before the macro lookup),
`record_qualified` (deferred, from `resolve_sym`, drained by `drain_pending` before eval —
a qualified **value** reference), and `scan_root_refs` (the root region: a header-less
script / the REPL, where `resolve` is identity — so a top-level qualified value
auto-requires too; gated so it never runs during prelude boot). Consequences of choosing
this trigger over Design B:

- **No curated index, no ambiguity, no collision guard.** The reference is already
  unambiguous (`math/sqrt` names its module), so none of Design B's index/ambiguity/
  no-collision machinery (settled decisions #1, #3, #4 below) is needed or built.
- **It does NOT delete the `(:use …)` lines** (Design B's goal, #5 below) or make bare
  names work. Stages 1–3's explicit imports stay; a file either `(:use math)` or writes
  `math/…` qualified, and the *require* is what's inferred away, not the qualification.
- **Root-region scripts DO auto-require** (`scan_root_refs`), resolving the "bare scripts"
  gotcha below — but for *qualified* references, not bare ones.
- **Works library-wide.** Because the trigger is a `/` and not a curated-name lookup, it
  applies to `json/parse`, `csv/…`, project modules — the reason stage 4 (`json`) could
  drop its `json-` export prefix and rely on `json/parse` self-loading.

The checker's KI-17 *"reference to an unrequired module"* lint (`unrequired_module`,
`walk.rs`) is now permanently obsolete — a qualified reference requires its own module —
and is neutralized to a no-op (its `required_mods`/`raw_qualified` scaffolding retained).

--------------------------------------------------------------------------------
**Everything below is the ORIGINAL Design-B plan — superseded, kept for reasoning.**
--------------------------------------------------------------------------------

## The goal

A file uses `dedupe`, `sqrt`, `update-vals` **bare, with no `(:use …)`**, and it Just Works —
while `seq/dedupe` still resolves qualified, **local names and explicit `(:use …)` always
win**, and a genuinely ambiguous bare name is a **loud compile error**, never a silent wrong
pick. This deletes every `(:use seq/map/math)` line the three stages added, and the whole
class of missed-import bugs (see "Why", below).

## Chosen design: B (lazy auto-derive), not A (eager auto-load)

We evaluated two ways to get bare names working:
- **A — eager auto-load:** at boot, `(:use seq) (:use map) (:use math)` into the root
  namespace; their names become global. *Rejected:* it puts the names back in the global
  namespace (the thing we moved them out of), so the module split becomes cosmetic; it grows
  the global surface and turns any future two-module name collision into a **boot failure**.
- **B — lazy auto-derive (chosen):** an unresolved bare name that exactly one *curated*
  module exports is resolved to that module's qualified name **per use, on demand**; nothing
  is referred until used. Keeps the namespace boundary *semantic* (a file depends on `seq`
  only if it uses an seq name), keeps the global surface small as the library grows, and
  enables dependency-aware tooling / tree-shaking later. Its one cost — a bare name can become
  *ambiguous* if a future curated module adds a colliding name — is small, local, loud, and
  guarded by a "no two curated modules share a name" test.

### Why B is also *more correct* than the explicit `(:use …)` we shipped

Per-name, lowest-priority resolution dissolves the `:use`-level clashes the stage-3 migration
hit. Example: `telemetry_metrics_test` uses both `telemetry/sum` (3-arg) and `math/abs`.
Under explicit `(:use math)` those two `sum`s clashed and needed `:only [abs]`. Under
auto-derive, bare `sum` resolves via the file's explicit `(:use telemetry)` (higher priority),
so `math/sum` never enters — no clash — while `abs` auto-derives. We only pull the *specific*
names a file actually uses, and only when nothing else claimed them.

## Settled decisions (do not re-litigate without reason)

1. **Curated set = `seq`, `map`, `math` only** — the formerly-prelude core. Mark them with an
   opt-in flag (a field on `EmbeddedModule`, or a hardcoded list in the index builder).
   `json`/`csv`/`set`/`http`/… stay explicit `require`/`:use` (auto-deriving them would make
   bare `parse` ambiguously conjure a module).
2. **Lowest priority.** Resolution order stays: locals → slash/alias → ambient (`defdyn`) →
   own-namespace → explicit `(:use …)` → prelude/root → **then auto-derive**. Local and
   explicit always win.
3. **Per-name granularity.** Import the single name used (`add_import(dedupe, seq/dedupe)`),
   not a whole `(:use seq)` refer-all. Lighter, and it's what makes #B-correctness work.
4. **Ambiguity is a loud error** naming the candidates ("qualify, or `:use` one"). Plus a test
   asserting the curated set has **no name collisions** (so ambiguity can't arise silently
   today; the error is future-proofing).
5. **Applied uniformly** — user code, std modules, and (see open question) scripts. After it
   works, strip every `(:use seq/map/math)` line the three stages added.
6. **Checker mirrors it** (it reuses the resolver, so mostly free) — and add a way to *see* a
   file's derived deps so implicit imports stay discoverable (e.g. `nest check` reports them).

## The seams (feasibility findings — file:line anchors; symbol names are the stable part)

Two decisive questions both resolved favorably, so this is **small-to-medium** work.

### Exports known WITHOUT loading — yes (the finding that keeps it small)
- No precomputed "module → public names" manifest exists; every current registry
  (`%refer`'s scan, `unbound_namespace_hint`, `module_public_exports`) scans the **live
  globals**, i.e. post-load.
- **But** ADR-223's `scan_regions` (`crates/lisp/src/eval/macros.rs` ~1018-1043) + `scan_def_form`
  (~1055-1080) already extract a module's def-heads **from source with zero evaluation**. It
  recognises `DEF | DEF_PRIVATE | DEFN | DEFN_PRIVATE | DEFMACRO | DEFDYN` heads.
- Every curated module's **full source is baked into the binary** as `CORE_MODULES`
  (`crates/lisp/src/builtins/system.rs` ~914; `EmbeddedModule { key, source, path }`), reachable
  via `%builtin-module`.
- **⇒ Build a curated `name → module` reverse index once at boot** by parsing seq/map/math's
  embedded source and running `scan_regions` with a **public/private head filter** (skip
  `DEF_PRIVATE`/`DEFN_PRIVATE`; the head string is in hand at scan_def_form). No module load.

### The resolver — where the fallback goes
- `resolve_sym` in `crates/lisp/src/eval/macros.rs` (~1168-1238). Order as implemented: locals
  (1174) → slash/alias/root-escape (1178-1213) → ambient `defdyn` (1214) → own-ns via
  `ns_knows_name` or `env_get(GLOBAL, ns/name)` (1219) → `(:use …)` via `import_of` (1222) →
  `ns_assume_own` REPL-only (1224) → **fall-through `else { s }` at ~1235** ← insertion point.
- Heap state it reads (`crates/lisp/src/core/heap.rs` ~2091-2115): `compile_ns`,
  `ns_known_names`, `ns_known_by_module`, `imports`; plus `env_get(GLOBAL, …)`.
- **Hard constraint:** `resolve_sym` takes `&Heap` (immutable), is documented non-allocating,
  and runs under `GcBlockGuard`+`MacroBlockGuard` (~1098-1099). It can **decide** but cannot
  `require`/mutate. → defer the load (below).

### The one-vs-ambiguous decision — already written
- `unbound_namespace_hint` (`crates/lisp/src/eval/mod.rs` ~1555-1597) already computes
  "exactly one module owns `/foo`" vs "ambiguous", by scanning `global_symbols()` and stripping
  the suffix (skipping `is_private` and `--` paths). Mirror this logic against the **pre-load
  curated index** (not the live-globals scan, which only sees loaded modules).

### Triggering the load — deferred to the compile driver
- Load path: `require` → `require-force-in` (`std/prelude.blsp` ~4694-4715), `%builtin-module`
  branch → `%load-module-source` (Rust, `system.rs` ~690). Cannot be called from the blocked
  resolver.
- **Plan:** the resolver records intent (push `(bare, module)` to a non-GC side buffer —
  `RefCell`/thread-local, no heap alloc), and the **compile driver** (macros.rs ~695-727, where
  `scan_regions`/`set_ns_known_by_module` are installed for each form batch) drains it: run
  `(require 'mod)` once (idempotent) and `heap.add_import(bare, mod/bare)` before the form
  evals. Alternatively the resolver rewrites the symbol to `mod/name` directly (pure) and the
  driver only guarantees the `require` so `mod/name` is bound. **Prototype this seam first** —
  it's the only real plumbing; everything else is table lookups.

### The `:use`/refer mechanics to reuse
- `%refer` (`system.rs` ~2563-2629) → `refer_add` (~2500-2535) → `heap.add_import(bare,
  qualified)` (`heap.rs` ~3836) — that's the `imports` entry the resolver reads at macros.rs
  ~1222. Use a **single-entry** `add_import` per auto-derived name (not a full refer-all).

### The checker — mostly free
- The checker reuses the resolver: `check.rs` ~834 calls `eval::macros::compile(...)`, so a
  `dedupe`→`seq/dedupe` rewrite flows into it automatically (no false "unbound").
- The remaining touch: `is_unbound` (`crates/lisp/src/types/check/walk.rs` ~378-400) gates a
  *qualified* name on `module_is_known`. Teach it (or its curated-sig path) that curated
  modules are known even when not loaded, mirroring the pre-load export index you give the
  resolver. Checker env setup that must stay in sync: `check.rs` ~782-798, ~856-861.

## Build plan (ordered)

1. **Curated reverse index.** A lazily-built `HashMap<Symbol, Symbol>` (name → owning module)
   from the three `CORE_MODULES` sources via `scan_regions` + private-head filter. At build
   time, assert **no intra-curated collision** (two curated modules exporting one name) — this
   is the guard behind decision #4.
2. **Resolver fallback** at `resolve_sym` ~1235: bare name in the curated index → rewrite to
   `mod/name` and record intent on the side buffer. (Ambiguity can't occur given the guard;
   still emit the ambiguous error if the index ever holds >1 owner.)
3. **Compile-driver drain** (macros.rs ~695-727): require each needed curated module once +
   `add_import` per name, before eval. Confirm idempotency and ordering.
4. **Checker:** teach `is_unbound`/`module_is_known` about curated modules (walk.rs ~378-400).
5. **Prelude-boot safety:** the fallback must no-op at root (`compile_ns == None` → `resolve` is
   identity, macros.rs ~1091) and can never fire for prelude names (they resolve earlier). Add a
   guard so it only fires with `compile_ns = Some(...)`. Verify boot is unaffected.
6. **Strip the explicit imports.** Remove every `(:use seq)`, `(:use map)`, `(:use math)` (and
   the `:only [abs]` on `telemetry_metrics_test`, and the `require 'math` + `math/…`
   qualifications in `breakage/chaos_float_pathology.blsp` + `breakage/chaos2_bigint_edge.blsp`)
   added in stages 1-3. Keep the modules. Find them with:
   `grep -rn "(:use \(seq\|map\|math\))" std/ tests/ stress/ && grep -rn "(:use seq)" std/tool/scaffold.blsp`
   (the scaffold **editor template** emits `(:use seq)` inside a string — update the template too).
7. **Tests** (`tests/*_test.blsp`, in-language, per `docs/brood-for-claude.md` + the
   `brood-testing` skill): bare curated name resolves with no `:use`; qualified still works;
   **local name wins** over auto-derive; **explicit `(:use …)` wins** (the `telemetry/sum`
   shape); **ambiguity errors** (construct a synthetic two-module clash to exercise the path);
   the **no-collision guard**; and a cross-process `:isolated` case. Then full `make test` /
   `make test-both`, `nest check` 0 warnings, `nest format --check` clean.

## Gotchas / open questions to decide while building

- **Bare scripts (no `defmodule`) do NOT auto-derive under this design.** Root-region
  resolution is identity (`compile_ns == None`; `resolve` returns early ~1091), so a header-less
  script's bare `sqrt` never reaches `resolve_sym`'s fallback → stays unbound. So `breakage/*`
  scripts would still need `require` + qualify, *or* we extend auto-derive to the root region.
  **Decision needed:** leave scripts explicit (rare, fine) or extend to root. (This corrects an
  earlier claim that auto-derive "fixes bare scripts" — it doesn't, unless extended.)
- **Multi-module files** (ADR-223 regions): auto-derive must key off the *active* region's
  namespace, using `ns_known_by_module`/`activate_ns_region` state already tracked per region.
- **Startup image (ADR-218):** confirm an imaged start reconstructs the auto-derived imports
  (or that the derivation re-runs) — an image that caches resolved forms but not the injected
  requires could break a second run. Test `nest run` twice.
- **Private-head filter** is essential — the index must expose only public (`defn`/`def`/…)
  heads, never `defn-`/`def-`.
- **Discoverability** (decision #6): add `nest check` (or a small command) output listing a
  file's auto-derived module deps, so implicit imports aren't invisible.

## Verification (the bar for "done")

`make test` and `make test-both` fully green; `nest check` 0 warnings; `nest format --check`
clean; the new resolution tests pass; boot + `nest run`-twice unaffected; and the strip in
step 6 leaves **zero** `(:use seq/map/math)` lines in the tree (grep returns nothing).

## Pointers
- Principle + the three stages + the migration lesson that motivated this: **ADR-227**
  (`docs/decisions.md`), and the ⬜ "Auto-derived imports" item in `ROADMAP.md`.
- Module mechanics: ADR-065 (namespaces), ADR-070 (package-rooted), ADR-223 (multi-module
  files / the `scan_regions` pre-scan this feature reuses).
