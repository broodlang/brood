# Incremental check — design for O(changed) `nest check`

Make `nest check` (and the check pre-flight in `nest test` / `nest run`) re-do work
proportional to **what changed since last time**, not to the whole project. Today it
re-reads, re-parses, and re-checks every source + test file on every invocation —
O(all files), even when one file changed. Parallelising it across the worker pool
(2026-07-05, `std/tool/project.blsp`) cut that by a constant (core count); this cuts
the *complexity class* of the common edit→recheck cycle to ≈O(changed + dependents),
which on a no-change re-run is ≈0.

**Status:** **Phase 1 and Phase 2 both shipped** (2026-07-05). Phase 1 (`3c22158`)
caches the two pure CST passes; Phase 2 caches the dominant `check-files` type-check
via per-file dependency fingerprints. Real-repo warm re-check is ~2.3× faster than
cold (both passes cached); cached output is byte-identical to uncached (modulo
compiler gensym numbers, which vary run-to-run regardless of caching). Soundness is
validated by a differential battery — for each edit scenario (a depended-on file's
arity/body changes, a referenced global appears/disappears, a new file makes a
module known, a `:use`d module's exports change, a split-file `(sig …)` edit), the
warm cached output must match a from-scratch uncached check byte-identically.

### Phase 1 as shipped (vs the design below)

- **Key = mtime**, not a content hash: an unchanged file is skipped without being read
  (a bare `stat`). The advisory contract makes mtime's rare imprecision harmless.
- **One parse per file** now produces `[mtime counts privs def-names]`, feeding BOTH
  pure passes (they used to parse separately). `counts` holds only `--`-containing
  symbols (all the unused-private verdict looks up), keeping the shipped/merged map tiny.
- **The cache merges** into the existing manifest (so `nest run`'s sources-only pass and
  `nest check`'s all-files pass sharing one manifest don't evict each other); stamped
  with `(build-id)`+version (dropped on binary change — the extract depends on the Rust
  `parse-source`); stored under `$XDG_CACHE_HOME/brood/check/<sha256 root>/extracts.blsp`;
  opt out with `BROOD_NO_CHECK_CACHE=1`.
- **File-count cap** (`BROOD_CHECK_CACHE_MAX`, default 25 000): above it the single-file
  manifest's `read-string` cost would rival re-parsing the sources, so the cache is
  skipped and the *aggregated* from-scratch passes run (compact `:scan` shipping +
  sequential duplicate-defs) — a huge project is never regressed by the cache machinery.
  A binary manifest could lift this ceiling (future work).
- **Measured:** brood repo (real files) warm re-check ~35 % faster (≈1.5 s → ≈0.95 s);
  cached output byte-identical to uncached; verdicts are never cached (always
  re-aggregated + re-derived), so a change in one file correctly shifts another's. On
  *trivially small* files the manifest overhead can exceed the near-zero parse saving —
  a non-issue for real code, bounded by the cap.

### Phase 2 as shipped (vs the design below)

- The checker records **every global observation** through `obs_*` wrappers in
  `types/check/deps.rs` (env_get-at-global, declared-sig, known-ns, module exports,
  the protocol table) — so the dependency set is complete by construction, and
  inference is captured transitively (walking a callee's body records the callee's
  own references). Exposed as `(check-file-deps path)` → `[warnings dep-keys fp]`
  and `(check-deps-fp dep-keys)` → the re-observed fingerprint.
- A referenced global's fingerprint is its **defining file's mtime** (a body/arity
  change ⟺ that file changed) plus its **declared-sig value hash** (which may live
  in another file), or its kind for prelude/builtin (stable per `build-id`), or
  `"U"` when unbound. A `:use`d module → its export set; the protocol table → a
  structural hash. `def-site`-mtime is coarse (any edit to a file invalidates every
  dependent of any global it defines) but sound; the battery confirms it.
- **Dep-capture (`check-file-deps`) runs sequentially** in the driver process, not
  across the worker pool: the recorder is thread-local and green processes migrate
  across OS threads, so two concurrent checks would clobber it. A warm re-check
  touches only changed files + dependents (cheap); a cold run pays a one-time
  sequential full check. Parallel dep-capture (recorder moved into the per-process
  context) is the deferred optimization.
- Same file-count cap as Phase 1: above it the cache is skipped and the parallel
  from-scratch path runs, so a huge project is never regressed.

The rest of this doc is the original design.

## Why parallelism isn't the end of the story

Measured `nest check`, 100K trivial files, uncontended (2026-07-05, post-parallel):

| Phase | ~time | shape |
|---|---|---|
| startup + load + discover | 0.8 s | serial, small |
| `unused-private` (+ `duplicate-defs`) | ~4.5 s | parallel; **CST-parse-bound** per file |
| `check-files` (per-file type check) | ~7 s | parallel; genuine per-file type-checking |

Both heavy phases already run on every core. But they still process **every file every
run**. Twelve cores hiding an O(n) redo is not the same as not redoing it — the right
fix for scale is to cache per-file results and re-do only the deltas. This design sits
*on top of* the parallel path: the parallel from-scratch run is still the required cold
/ cache-miss path.

## Two subsystems, two invalidation shapes

The check has two independent halves that invalidate differently (this is the crux):

1. **Whole-project CST passes** — `unused-private` + `duplicate-defs`
   (`std/tool/project.blsp`). Each is a **pure function of the files' text**: parse each
   file's CST, extract its `--`-symbol references + private defs (unused-private) or its
   top-level def-names (duplicate-defs), then compute a **whole-project aggregate**
   (global `--`-ref counts / def-name multiset) and derive per-file verdicts from it.
   No dependency on the loaded global image.

2. **Per-file type check** — `check-file` (the Rust checker,
   `crates/lisp/src/types/check.rs`). Resolves cross-module names through the **loaded
   global image**, so a file's result depends on its own text **and** on the signatures
   of the externally-referenced globals (which come from *other* files' text).

These get two phases, easy first.

## Phase 1 — incremental whole-project CST passes (low risk, no dependency graph)

The pure passes need only **content hashing** to be both sound and complete:

- **Per-file extract cache**, keyed by `hash(file bytes)`: store the file's
  `(--symbol-ref counts, private-defs)` (for unused-private) and `(top-level def-names)`
  (for duplicate-defs). These are exactly what the current `:scan` op
  (`project--scan-chunk`) and `project--file-def-names` already compute.
- **On recheck**: for each file, if its content hash is unchanged → reuse the cached
  extract (skip the parse — the dominant cost); else re-parse and refresh its entry.
- **Re-aggregate every run** from the (mostly cached) per-file extracts:
  `project--merge-counts` over all files' `--`-counts, the def-name multiset over all
  files. This is O(files) cheap map ops, **no parse**. Then derive verdicts as today.

Soundness is immediate: the passes are pure functions of file text, so a matching
content hash guarantees an identical extract, and the aggregate step correctly
propagates any changed file's effect across the whole project (a changed file shifts the
global counts, re-deriving every file's verdict from the fresh aggregate). **No
cross-module dependency tracking is needed** — the aggregate *is* the cross-file
coupling, and it is always recomputed. This phase alone removes the parse cost (~40 % of
`nest check`) for every unchanged file.

## Phase 2 — incremental per-file type check (needs a dependency fingerprint)

`check-files` can't use content-hashing alone: a file that didn't change can still need
re-checking because a *global it references* changed signature in another file. So:

- **Record a dependency set.** Instrument `check-file` to emit, alongside its warnings,
  the set of **external global symbols it resolved** plus a fingerprint of *what it
  observed* about each: exists? arity? declared `(sig …)` type? (and, for `(:use m)`
  headers, the module's export set). This is a forward-dependency list per file.
- **Cache key** per file = `hash(content) + hash(sorted dependency fingerprints)`. A hit
  means both the file and everything it depended on are unchanged → reuse warnings.
- **Reverse-dependency map** `global-symbol → {files depending on it}`, built from the
  recorded forward deps. When a changed file's defs change a signature, invalidate its
  dependents through this map and re-check only those.
- **No fixpoint.** Checking is read-only over the image — re-checking a file never
  changes any signature — so invalidation is one-shot: `changed source files → their new
  signatures → dependents to re-check`. It does not cascade further.
- **The loaded image.** `nest check` first `ensure-loaded`s sources (evals them into
  globals); Phase 2's fingerprint reads signatures from that live image. Load is cheap
  (~sub-second) relative to the check; the sibling *evaluated-image* cache
  ([image-cache-plan.md](image-cache-plan.md)) could later skip even that, but it is an
  orthogonal artifact.

Phase 2 is where the real complexity lives (dependency capture in the Rust checker,
reverse-map maintenance), which is why it's deferred behind Phase 1.

## Cache key, storage, staleness (both phases)

Mirror the existing runtime/image cache conventions
(`release.rs::runtime_cache_path`, [image-cache-plan.md](image-cache-plan.md)):

- **Global staleness stamp** invalidating the *entire* cache on mismatch: the **brood
  build id** (`BROOD_GIT_SHA` — the checker's *logic* changes between builds, so results
  are not portable across binaries) + a **`check-cache-format` version int** + a hash of
  the **checker-relevant prelude/std** (a std change can change name resolution). Any
  mismatch → cold path (full parallel check, then rewrite the cache). *As shipped:*
  ADR-129 (2026-07-05) added the binary's own mtime to the stamp — `(build-id)` is now
  `<version>+<git-sha>+<binary-mtime-hex>` — because git-sha alone went stale during
  uncommitted checker changes.
- **Per-file entries** keyed by content hash (the existing hashing primitive).
- **Path**: `$XDG_CACHE_HOME/brood/check/<project-id>/manifest` (+ entries), falling back
  to `~/.cache`, exactly like the image cache. Never inside the project tree.
- **Manifest** = `file-path → { content-hash, warnings, cst-extract, dep-fingerprints }`.

## Interaction with hot reload / late binding (the tricky bit)

Brood is late-bound: globals are redefinable at runtime and names resolve through the
live global table (ADR-013, [shared-code.md](shared-code.md)). Two things keep this from
poisoning the cache:

- The check cache is a **static, on-disk artifact for the `nest` toolchain operating on
  source *files***. It is consulted only by the batch checker, never by a running
  program, and runtime `def` rebinding never touches on-disk files — so runtime hot
  reload is simply out of scope for it.
- Source-level late binding *is* captured: if a dependency's **source** changed, its
  content hash changed → it is re-loaded → its new signature → dependents invalidated via
  the reverse map (Phase 2). The dependency fingerprint is what makes "file A's check
  depends on the current signature of a name defined in file B" explicit and
  invalidatable.
- **The advisory contract is the safety margin.** The checker never rejects a runnable
  program (types.md contract #5). So even a stale-cache *miss* (a warning not re-emitted)
  cannot break a build — worst case is a momentarily-missed advisory diagnostic,
  corrected on the next content change. This lets Phase 2's invalidation be
  *conservative* (over-invalidate freely) with **zero correctness stakes** — a rare
  luxury that makes this far safer than caching a real compiler's output.

## Alternatives considered

- **More parallelism / a faster checker** — constant-factor only; never stops re-doing
  unchanged work. (Already banked what parallelism buys.)
- **Persisted compiled bytecode** — that's the sibling [image-cache-plan.md](
  image-cache-plan.md) (skip *eval* on warm startup), a different axis (program startup,
  not checking). Complementary, not a substitute; note Brood closures are already
  AST-as-data, so there's no separate bytecode format to persist.
- **Whole-project result cache keyed on an all-files hash** — trivially correct, useless:
  any single edit busts the whole thing.
- **Per-file cache for `check-files` without dependency tracking** — unsound, because of
  cross-module name resolution. Hence Phase 1 is restricted to the pure CST passes and
  Phase 2 adds the dependency fingerprint.

## Staging recommendation

1. **Now:** design only (this doc + ADR-119). Nothing built. The parallel from-scratch
   path is the baseline and the permanent cold/miss path.
2. **When a concrete large real project appears:** build **Phase 1** — sound with content
   hashing alone, captures the parse-bound ~40 %, no dependency graph.
3. **If Phase 1 is insufficient:** build **Phase 2** — dependency fingerprints + reverse
   map for `check-files`. This is the hard part; the advisory contract makes conservative
   invalidation safe.
