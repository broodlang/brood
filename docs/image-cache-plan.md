# Image cache — plan for fast warm startup

Skip re-evaluating a project's modules on every launch by caching the **evaluated
global table** (the "image") to disk and re-hydrating it when the sources are
unchanged. Cold launch stays correct; warm launch skips the eval.

## Motivation (measured on a 25-module downstream app)

`nest run` → `main`-entry is ~**1.17 s**; the in-`main` setup (config, project,
buffer, `gui-display`, model, theme, logging) is only **53 ms**. The rest is the
**module load, before `main` runs**:

| Phase | Cost | Cacheable by this? |
|---|---|---|
| nest base (prelude) | 40 ms | — |
| std modules the editor uses | +40 ms | (part of image) |
| parse all 25 files (1213 forms) | 10 ms | no point |
| top-level macroexpand (1213 forms) | 31 ms | no point |
| **eval of the editor's 25 modules** | **~1.0 s** | **yes — this is the target** |
| `gui-display` (window + GPU) | 50 ms | — |

Parse and macroexpand are trivial; the ~1 s is **eval** — creating ~1200
closures and running module-level construction (199 `defcommand` registrations,
265 `keymap-bind` calls, face/mode/layer registries). No form-level cache helps;
only skipping the eval does, which means snapshotting the evaluated image.

## Why this is tractable (feasibility findings)

- The global table is `RwLock<SymbolMap<Value>>` — `Symbol(u32) → Value`
  (`crates/lisp/src/core/heap.rs`). `Value` (`core/value.rs`) is mostly heap
  **handles** (`Str/Rope/Pair/Vector/Map/Fn/Macro/…Id`) plus immediates
  (`Bool/Int/Float/Sym/Keyword`) and `Native(NativeId)`.
- **A closure's body is `body: Vec<Value>` — AST-as-data, not compiled
  bytecode** (`Closure`/`ClosureArm` in `value.rs`). The "compiled" function *is*
  serializable Brood data. No bytecode format to invent.
- `Native(NativeId)` is a builtin — **re-linkable by identity**, not serialized.
- Brood is **late-bound**: functions reference each other by *symbol* (global
  lookup), not by heap pointer — so the object graph rooted at globals is largely
  a DAG; cross-function references need no special handling.
- Existing machinery to build on: `Heap::snapshot_globals` / `GlobalsSnapshot` /
  `restore_globals` (the in-memory clone/restore of the globals table, hardened in
  `d22619d`); heap arenas keyed by typed ids; `crates/lisp/src/bundle.rs` (an
  existing id-based binary (de)serializer to model the format on); env frames
  `EnvFrame { bindings, parent: Option<EnvId> }` for captured closures.

## Design

### Cache key (staleness)
Hash over: (a) the content of **every source file actually loaded** (project
`src/` + transitively-required `std` modules), (b) the **brood build id**
(`BROOD_GIT_SHA` — pins builtin/`NativeId` layout *and* the `Value` repr), and
(c) an **image-format version** integer. Path mirrors the runtime cache
convention (`release.rs::runtime_cache_path`):
`$XDG_CACHE_HOME/brood/images/<project-id>/<hash>.img` (fall back to `~/.cache`).
Any mismatch → cold path (eval, then rewrite the image).

### What the image contains
Roots = the globals `SymbolMap` after load. Walk the reachable heap graph and
emit, id-based (each heap object gets a serial id; references become ids):

- immediates (`Bool/Int/Float/Sym/Keyword`) — inline;
- `Str/Bytes/BigInt/Decimal` — by value;
- `Pair/Vector/Map/Range/SeqView` — recurse (shared structure preserved via ids);
- `Fn/Macro` (`Closure`) — `name`, `arms` (`params`, `optionals`, `rest`,
  `body: Vec<Value>`, `docstring`), and `env` (`None` → global; `Some(EnvId)` →
  serialize the reachable `EnvFrame` chain). **Skip `passthrough`** — re-derive it
  on load via the `alloc_closure` path;
- `Native(NativeId)` — emit the builtin's stable identity (name/index); relink on
  load. (Because the key pins `BROOD_GIT_SHA`, a verbatim `NativeId` is also safe,
  but by-name is more robust to reordering.)
- **Symbols are serialized by NAME**, re-interned on load into a remap table;
  every `Symbol` reference is rewritten through it (u32s will differ per run).

**Non-serializable guard.** `Ref` (process), `Socket`, `Subprocess`, `Table`
(mutable ETS), and live `Rope` handles are runtime resources that must not appear
in a freshly-*loaded* image. If the serializer meets one reachable from globals,
**abort caching** for this project (fall back to cold eval, log once). For the
editor, module-level globals are functions + immutable data, so this shouldn't
fire; the guard is a safety net, not an expected path.

### Load (deserialize)
1. Verify format version + key hash; on any mismatch/corruption → cold path.
2. Re-intern all symbol names → symbol remap.
3. Relink natives by identity.
4. Two-pass rebuild: allocate every heap object (strings, collections, closures,
   env frames) with placeholder refs, then patch refs by serial id.
5. Re-derive each closure's `passthrough`.
6. Install the globals `SymbolMap` (a `restore_globals`-style install, but sourced
   from the deserialized table rather than an in-memory snapshot).

### Integration point
`std/tool/project.blsp::project-load-sources` (the eager loader `nest run` /
`repl` / `test` all funnel through), or a Rust wrapper in the run bootstrap
(`crates/nest/src/main.rs`, the `project-load-sources` call site):
- compute key → if a valid image exists, **hydrate globals and skip source eval**;
- else eval sources as today, then **write the image best-effort** *after* first
  frame / off the critical path, so a cold launch is never slowed by caching;
- flags: `--no-image-cache` and `BROOD_NO_IMAGE_CACHE` to bypass entirely.

## Staged implementation

- **Stage 0 — feasibility spike (GO/NO-GO).** Serialize the editor's loaded
  globals to bytes and re-hydrate into a *fresh* runtime; assert a sample of
  functions are callable and return identical results to a source-loaded runtime;
  measure warm-hydrate time vs the ~1 s cold eval. If closure-env or native relink
  proves intractable, **stop here** — the feature isn't worth it and the daemon
  remains the fallback.
- **Stage 1 — serializer.** Graph walk from globals roots; all `Value` kinds;
  symbol-by-name; native identity; env frames; non-serializable guard → abort.
- **Stage 2 — deserializer.** Two-pass alloc + patch; symbol remap; native relink;
  `passthrough` re-derive; install globals.
- **Stage 3 — keying + invalidation.** Source + build + format-version hash; cache
  path; cold-path rewrite kept off the critical path (best-effort, never fatal).
- **Stage 4 — integration + flags.** Hook the load path; bypass flags;
  write-on-cold / read-on-warm; corrupt/partial image → clean cold fallback.
- **Stage 5 — validation.** The full brood suite + a real multi-module app's suite pass when the image is
  loaded from cache; a diff test that a hydrated runtime's globals are behaviorally
  identical to a source-evaled one; warm-startup target **< 150 ms**; stale/corrupt
  image → fallback, never a crash or a wrong result.

## Correctness invariants
- Hydrate-from-image MUST be **behaviorally identical** to source eval. Test:
  load both ways, diff the globals (names → structurally-equal values, functions
  produce equal outputs on sampled inputs).
- Any anomaly — unknown `Value` kind, missing native, hash mismatch, truncated
  file — is a **silent fallback to cold eval**, never a crash and never a wrong
  answer. The image is a *cache*: deletable anytime, never authoritative.

## Interaction with the in-flight "file loading time" work
Complementary, ship both: faster eval speeds the **cold**/first load and every
cache-miss rebuild; the image cache removes eval on **warm** loads. Caveat: if the
load-time work changes the `Value`/`Closure` layout or builtin table, **bump the
image-format version** so old images auto-invalidate.

## Risks & fallbacks
- Module-level closures that capture non-trivial `let` locals → serialize env
  frames; if an env reaches a non-serializable value, abort → cold path.
- `Value`/native layout drift across builds → key includes `BROOD_GIT_SHA` +
  format version.
- Mutable module-level state (`Table`/atom) → guard aborts caching if present.
- **The fallback is always cold eval**, so the feature can only fail to *speed up*
  — it can never change behavior or break a build.
