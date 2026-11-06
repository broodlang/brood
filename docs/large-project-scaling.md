# Large-project scaling — 100k files × 3k lines: what was measured, what it means, what to do

**Status: mostly closed — measured 2026-09-21. Of the queue at the end, items 1, 2, 3, 5 and 6
are done (ADR-380, 382, 386, 419, 426) and item 4 is mostly closed.** "Today" in the tables
below means 2026-09-21. Reproduce every number here with the generator before
believing any of them changed.

## The question

"How performant is Brood for loading 100k files with 3k lines of code each, especially
with the type checker? Is it reasonable?"

The prior calibration (ADR-218, 2026-08) was **16 300 files × ~180 lines** (10× moneyclub,
2.9M lines): warm `nest run` 1.30 s / 219 MB, cold build ~32 s. The question's shape is
different in the way that matters — the *per-file* constants that dominated at 180 lines are
noise at 3 000, and everything that is O(source bytes) becomes the whole cost.

## The rig

```
brood scripts/bench/gen-project.blsp -- 1000 /tmp/brood-big3k 340   # 1 002 files, 3 067 lines each, 59 MB
cd /tmp/brood-big3k
( ulimit -v 32000000; /usr/bin/time -f "wall %es peak %M KB" nest run )            # cold, then warm
( ulimit -v 32000000; /usr/bin/time -f "wall %es user %Us peak %M KB" nest check )
BROOD_NO_CHECK_CACHE=1 …                                                          # the cold check
strace -f -e trace=openat nest run --no-check 2>&1 | grep -c src/mod                # files a WARM run opens
```

`--fns N` is new (2026-09-21): functions per module, 20 = the 180-line calibration shape,
340 ≈ 3 000 lines. The generator had emitted the pre-ADR-302 `fold` order and `println` for
a year with nothing running it — the "generator no gate can see" trap in CLAUDE.md — and is
fixed. `scripts/bench/image-scale.sh` drives the 180-line shape; nothing drives this one yet.

Box: 12 cores, release-fast `nest` at `99099b41`, stdlib image live. Every number is one run
unless stated; the shape of each is what matters, not the digit.

## Measured at 1 000 files × 3k lines (3.07M lines)

| operation | wall | peak RSS | what it scales with |
|---|---|---|---|
| cold `nest run` (parse, load everything, write the image) | 39 s | 3.3 GB | lines, ~13 µs/line, ONE thread |
| warm `nest run` (image hit; entry reaches 2 modules) | **3.0 s** | 288 MB | **source bytes** — a defect (§ below) |
| warm `nest run --no-check` | 2.9 s | 279 MB | the pre-flight is scoped and cheap |
| `nest check`, whole project, cold | **161 s** | **3.8 GB** | lines, ~53 µs/line, ONE thread (user 178 s on 12 cores) |
| `nest check` again, nothing changed | **16 s** — and that was the THIRD run: the second read 154 s (KI-179) | 3.7 GB | now a replay: 0.12 s / 118 MB |

For scale: the same tree at the 180-line shape (16 300 files, 2.9M lines) warm-runs in 1.3 s.
Same line count, 2.3× the warm time, because of the item below.

## Extrapolated to 100k files × 3k lines (300M lines)

300M lines is ten Linux kernels. Nothing compiles that quickly; the honest question is
which costs are *proportional to what you run* and which are *proportional to the codebase*.

| | today | verdict |
|---|---|---|
| cold build, once | ~65 min | linear and unavoidable in kind; parallel loading would divide it by cores |
| warm run | **~5 min** | **not reasonable — and not inherent.** The module index re-parses every file (twice); with the fix below it is O(closure) at any size |
| whole-project `nest check` | **~4.4 h, ~380 GB RSS** | **not reasonable in either dimension.** The per-line cost is fine; sequential + whole-project-resident is not |
| running a program | fine | `nest run` checks only the entry's require-closure (ADR-218: 127 s → 1.2 s at 16k files) and materialises only what it reaches |

So: **reasonable for running, at any size; not reasonable for `nest check` / `nest test` /
the LSP over the whole thing at 100k × 3k.** The 100k × 180-line shape (the calibrated one
×6) is fine today on every row but the whole-project check's memory.

## Finding 1 — the warm run reads every source file, twice — FIXED (ADR-380)

**After the fix, same rig, release-fast `nest`, the 09:37 build of the tree against it:**

| | before | after |
|---|---|---|
| warm `nest run` | 3.0–3.16 s / 282 MB | **0.15 s / 145 MB** |
| warm `nest run --no-check` | 2.99 s / 275 MB | 0.12 s / 120 MB |
| `src/mod*.blsp` opened on a warm run (`strace`) | 2 000 | **0** |
| cold `nest run` | 45.8 s / 3.2 GB | 39.2 s / 3.3 GB |

The warm start is O(files) now — one `stat` per file plus a 152 KB index read — not O(bytes),
and both gates named below hold. `FNS=340 scripts/bench/image-scale.sh 250 500 1000` (release
`brood`, the loader alone, same day) shows the shape across N — `warm lazy` is the `nest run`
start (image install, nothing materialised), `warm all` the `nest test`/`nest check` start
(everything materialised, O(project) by design):

| N (× 3k lines) | load only | load + image write | image | warm all | warm lazy |
|---|---|---|---|---|---|
| 250 | 6.8 s / 707 MB | 9.7 s / 822 MB | 37 MB | 0.86 s / 477 MB | **0.07 s / 126 MB** |
| 500 | 14.4 s / 1 226 MB | 19.4 s / 1 400 MB | 74 MB | 1.75 s / 797 MB | **0.09 s / 132 MB** |
| 1 000 | 27.6 s / 2 286 MB | 55.1 s / 2 803 MB | 149 MB | 4.23 s / 1 471 MB | **0.14 s / 134 MB** |

One reading in that table to attribute before believing: at N=1000 the image WRITE costs
27 s under `brood` (55.1 − 27.6) where `nest run`'s cold build on the same tree reported
`+4.2s image` minutes earlier — reproduced twice under the script. Not this item's; it is a
cold-path (Finding 3) question.

The rest of this section is the finding as measured, kept because it is the shape to
recognise if a scan over every file is ever added to the warm path again.


`strace` on a warm `nest run --no-check`: **2 000 `openat` of `src/mod*.blsp` for 1 000
files**, and `perf` puts the 3.0 s in the READER (`Parser::read_seq`, `Scanner`, `FormPos`
insert/retain, `intern`) — not in the image, not in I/O (7 000 `statx` cost 3 ms). Two
scans each `(reflect/read-all (file/slurp file))` — the whole file, every form built as
values — to find the `defmodule` header:

- `std/tool/package.blsp` `package-module-names-of` — the file → modules index behind
  `module-files` (collision scan, `%require-find`'s map).
- `std/tool/project.blsp` `project-file-module` — the loader's "which feature does this file
  provide" (the `*features*` dedup).

At 180 lines a file this was 80 µs/file and invisible inside ADR-218's 1.3 s. At 3 000 lines
it is 3 ms/file and the whole warm start. Both are O(total source bytes) on a path whose
contract is O(closure).

**The fix (the right one, not the fast one):** cache the file → modules index, keyed per file
by the `(path, size, mtime)` the fingerprint already stats. A warm start then reads zero
source files; a changed file re-indexes itself alone. This is what every build tool does
with its module graph. A `%read-first`/header-only scan would cut the constant (the reader
still has to find form boundaries) but leaves the shape O(bytes); do the cache. **Done as
ADR-380** — `std/tool/module-index.blsp`, a separate `.brood/module-index` beside the image
rather than a section of it, because the image's own fingerprint needs the dependency file
list the index produces, and because the two have different keys (per file vs whole
project) and different lifetimes (a fact about source text vs binary-specific bindings).

Verified: the `strace` count reads **0** on a warm run, and the gate is
`crates/nest/tests/module_index.rs` — a generated project run twice under
`BROOD_IMAGE_TRACE=1`, asserting every `[index] N files: H from the module index, P parsed`
line of the second run has `P == 0` (and an edited project re-parses exactly the edited
files). The count is kept whether or not the trace prints it (KI-171). The warm run fell
from 3.0 s to 0.15 s — below the 16k-file figure, as it should: O(files), independent of
file size.

## Finding 2 — `nest check` is single-threaded and whole-project-resident

161 s wall against 178 s user: one core of twelve. 3.8 GB for 3M lines: every file's forms,
positions and derived facts stay live for the run. Both are design, not bugs:

- The checker's Pass 2.9 joint fixpoint derives signatures ACROSS files (a callee's inferred
  type feeds a caller's verdict), so files cannot simply be checked in parallel; the per-file
  walk *after* the fixpoint can be, and `sigs::SiteCache` (2026-09-17) already memoises it.
- Memory is O(project) because the fixpoint wants every file's derived state at once.

**Options, in order:**
1. ~~**Incremental for real (ADR-129 finished).**~~ **DONE 2026-09-21, ADR-382 / KI-179.**
   Measuring it from a cleared cache found the real defect first: the per-file cache hit only
   image → image (a global's fingerprint fact is its def site; the image carried none), so
   the check after a cold build re-checked everything (154 s) and so did the check after
   any edit (188 s). The image carries def sites now; an unchanged project replays its
   recorded verdict without loading; the require graph reads from the module index; listed
   paths are absolutised. After (release, same rig):

| operation | before | after |
|---|---|---|
| `nest check`, unchanged, right after a cold build | 154 s / 3.2 GB | **0.12 s / 118 MB** (replay) |
| `nest check`, unchanged, steady state | 14.7 s / 3.0 GB | **0.12 s / 118 MB** (replay) |
| `nest check` after a one-function edit | 188 s / 3.8 GB | **40.2 s** — image rebuild 32.6 s, the check 2.7 s (1 file re-checked) |
| `nest check FILE FILE FILE` (listed, unchanged) | every listed file re-checked | 9.3 s, 0 re-checked (4.0 s materialise + 4.6 s lints, both O(project)) |
| require-graph parse per run | 2.9 s | 0.04 s (module index) |
| cold `nest check` | 186 s | 191 s (unchanged in kind: load 37 s + check 146 s, one thread of the check pool aside) |
 What an edit still pays is the whole-image REBUILD
   (Finding 3): 30 s of load before a check that re-derives two files in 20 ms.
2. **Parallel per-file walk** after the fixpoint: 12 cores → ~12× on the 90% of the time that
   is the walk. Needs the checker's per-file state to be `Send`, or a process-per-file model
   with the fixpoint's signatures handed in as data.
3. **Memory:** drop a file's forms after its walk, keep only its derived facts; and/or
   check in shards (the fixpoint over signatures only, then walks per shard).


**Item 3 is done (ADR-386, 2026-09-22) — and it made item 4 more urgent, not less.** The
walk was already parallel; the gate counted FILES. On a 302 × 3 067 rig (18 MB, debug,
verdict cache off on both arms):

| | wall | user | user/wall | peak RSS |
|---|---|---|---|---|
| sequential | 55.9 s | 66.0 s | 1.18 | 1.19 GB |
| parallel | **8.8 s** | 98.3 s | **11.1** | **2.20 GB** |

**Where the memory is NOT: the worker heaps.** Group size is `cores`, so at most `cores`
worker heaps live at once, and the obvious lever is to shrink that group. Measured at
`cores/4`: wall 25.8 s (3× worse) and peak RSS **2.26 GB — unchanged**. So the peak is the
DRIVER's: every file's forms, positions and derived facts, live for the run because the
fixpoint wants them all at once. Item 4 is the surgery the option describes (drop a file's
forms after its walk; shard the fixpoint over signatures only) and nothing cheaper.

### Item 4 (memory), measured — the queue's premise was wrong, and the gate is unreachable as written

Five arms on the 302 × 3 067 rig (18 MB of source, debug binary, verdict cache off):

| arm | wall | peak RSS |
|---|---|---|
| `nest run --check-boot` — load every module, check nothing | 1.2 s | **0.54 GB** |
| `nest check` under `BROOD_NO_CHECK=1` — setup + preload, no walk | 16.2 s | 0.88 GB |
| `nest check`, sequential | 55.5 s | 1.03 GB |
| `nest check`, parallel | 9.7 s | **2.14 GB** |
| `nest check`, parallel, `MIMALLOC_PURGE_DELAY=0` | 10.3 s | 1.64 GB |

Three things follow, and two of them contradict the option above.

1. **The walk is 0.15 GB of the 1.03.** "Drop a file's forms after its walk" is the small
   lever, not the big one: everything before the walk — loading the project and levelling
   the heap for it — is 0.88 GB, six times what the walk adds.
2. **The floor is the project's own loaded code: 0.54 GB here, ~1.8 GB at 1 000 × 3k.** The
   gate as written ("peak RSS at 1 000 × 3k under 1 GB") cannot be met by checker-side work
   at all — loading the code the check is about already exceeds it. Either the gate moves,
   or the question becomes whether a whole-project check must hold the whole project
   (ADR-382's replay already answers the *unchanged* case by not loading at all).
3. **The pool's +1.11 GB is not concurrent worker heaps.** ~0.5 GB is the allocator holding
   freed pages (`MIMALLOC_PURGE_DELAY=0` recovers it for +7% wall — the documented
   trade-off in `runtime-frontier.md` A8). The remaining ~0.6 GB does **not** shrink when
   fewer workers are live: group size `cores/4` reads 2.26 GB retained / 1.69 GB purged
   against `cores`' 2.14 / 1.64, for 2.4–3× the wall. Measured twice, once each way.
   Promotion into the append-only shared region is not it either — 105 226 promotions
   sequential against 105 332 parallel. So it accumulates per CHUNK in the driver, and what
   it is has not been found.

**Where to start**, then, is not "drop forms after the walk": it is (a) restate the gate
against the load floor, (b) find the per-chunk driver accumulation in 3, and (c) decide
whether `nest check` should set the allocator's purge delay for itself — a global policy
question, since the default is a deliberate "spend memory for speed" choice.

### Item 4, re-measured 2026-10-22 — nothing accumulates; the peak is two transient spikes

Release `nest` at `9b82d9be`, 302 × 3 068 rig (`gen-project.blsp -- 300 DIR 340`), verdict
cache off, RSS sampled every 50 ms against the `[check]` phase markers:

| phase | sequential | parallel |
|---|---|---|
| load (`ensure-loaded`, 0–1.4 s) | climbs to ~0.6 GB — the load floor | same |
| whole-project lints (1.4–2.6 s) | ~0.96 GB | ~1.46 GB |
| `project-preload!` (2.6–3.0 s) | **1.17 GB — the peak** | **~1.75 GB** |
| the per-file walk (the other ~45 s / ~12 s) | **~0.70 GB, flat** | ~1.35 GB, flat |

So (b) has an answer, and it is not what the queue assumed: **there is no per-chunk
accumulation.** The walk holds 0.1 GB over the load floor for its whole run; the peak is two
short phases that each parse every file in the DRIVER — `project-preload!`'s
`reflect/source-deps` over all source (only to collect qualified-name prefixes) and the lints'
scan — and leave garbage behind. Their retained data is small: the lints' per-file counts are
109 217 entries for the whole rig, a few MB.

Two policies decide how much of that garbage is resident at once, and each was measured:

- **The nursery.** A minor collection fires at `BROOD_GC_GROWTH × live`, capped at
  `NURSERY_MAX` (8M objects, "a few hundred MB"). The driver's live set is large, so its
  nursery runs near the cap. `BROOD_GC_GROWTH=1.5`: sequential peak 1.19 → 0.95 GB, same wall;
  parallel unchanged (~1.7 GB, its spike is the pool's). **Not a default change:** `make ab`
  on twelve rows reads noise on time, but peak RSS moves both ways — `base64` +10% (97 → 106 MB,
  five runs each), `sort` bimodal with runs at +10%, `bintree` −5–10%. Collecting sooner tenures
  sooner, so the garbage moves to the old generation instead of going away.
- **The allocator's purge delay.** The pool's steady +0.6 GB scales with OS THREADS, not with
  live worker heaps (`BROOD_J`=1/2/4/12: 1.19/1.19/1.69/1.84 GB, while every arm keeps ~150
  files of worker state live at once): it is mimalloc's per-thread pages, freed by green
  processes that migrated. `MIMALLOC_PURGE_DELAY=0` returns them for +4–6% wall.

Both together: sequential **1.15 → 0.79 GB**, parallel **~1.7 → 1.25 GB**.

**(a), the gate restated.** "Peak RSS at 1 000 × 3k under 1 GB" is below that rig's load floor
(~1.8 GB). The measurable gate is **peak ≤ load floor + the walk's own state** — here
0.6 + 0.1 ≈ 0.7 GB sequential. Today it is 1.15; the two spikes are the whole gap.

**What closes it without a global trade:** do not parse every file in the driver twice. The
module index already re-parses a file only when its size or mtime moves (ADR-380); if its
entry carried the file's qualified-reference prefixes, `project-preload!` would read them
instead of re-parsing 18 MB — the same data an O(changed) image rebuild needs (item 5). The
lints' scan can stay in workers, whose heaps die with them. **(c)** stays a policy question:
setting the purge delay for `nest check` alone is possible (`mi_option_set` through
`libmimalloc-sys`'s `extended` feature, before the pool starts) and is worth 0.4 GB parallel
for ~5% wall. Decided against on 2026-10-22: fix the sources, keep the allocator's
memory-for-speed default.

**Done the same day — and the re-parse was NOT the spike.** With `%mem-bytes`/`%mem-peak`
added to the `[check]` trace and markers inside the lints and the preload, the allocator's own
count put the garbage elsewhere: the lints' *aggregation*, not their parse. `lints-unused`
merged 302 per-file count maps into one with `%map-assoc` per entry (+277 MB in 152 ms) and
`lints-duplicates` built `name -> paths` for all 102 302 definitions (+100 MB) — persistent-map
path-copying, which the linear-map rewrite (ADR-360 §6) removes only for the tally idiom
spelled with the prelude's `get`/`assoc`. Both are tallies now: one fold over every file's
`[name count]` pairs, and a per-name definition tally that gathers paths only for names
defined more than once. The module index carries each file's qualified-reference prefixes
(`:refs`, index v3) and `project-preload!` reads them instead of re-parsing — worth no
measurable memory, but it takes 18 MB of parsing out of every warm check.

| | before | after |
|---|---|---|
| sequential peak / wall | 1.15 GB / 50.7 s | **0.80–0.82 GB / 45 s** |
| parallel peak / wall | 1.7–1.8 GB / 15 s | **1.49–1.51 GB / 12 s** |
| whole-project lints | 1.31 s, +367 MB live | 0.86 s, +54 MB |

Sequential is now 0.2 GB over the load floor. The parallel arm's remaining +0.7 GB is the
pool: worker results arriving as messages the driver has not collected yet, and the
allocator's per-thread pages.

## Finding 3 — cold load is one thread (fixed 2026-10-23, ADR-426)

**A pool, the default** (`BROOD_LOAD_SEQUENTIAL=1` opts out; it was opt-in for a day while
bedit's pool-built image differed, which was KI-290 — see ADR-426's amendments). A cold walk `require-one`s its
modules from a pool of processes: on the
302 × 3k rig the load 8.5 s → 2.1 s on 12 cores, a cold `--check-boot` 12.9 s → 6.5 s. The
image write (1.7 s) is now the largest single step. The original finding:

39 s for 3M lines, single-threaded (`Building bigproj … Built in …`). Files in different
subtrees of the require graph could load in parallel processes and be promoted into the
shared RUNTIME region; the image write already serialises the result. Lowest value of the
three — it is paid once — but at 300M lines an hour becomes five minutes on a 12-core box.

## The queue

1. ~~**Module-index cache** (Finding 1)~~ — **DONE 2026-09-21, ADR-380.** Warm start O(files);
   zero source opens on a warm run; the 3k-line rig warm-runs in 0.15 s.
2. ~~**`nest check` incremental** (Finding 2, option 1)~~ — **DONE 2026-09-21, ADR-382.**
   Gate `crates/nest/tests/check_incremental.rs`: an unchanged project replays (no load, no
   parse); a leaf edit re-checks 1 file, a dependency edit 2 (`[check] reuse-test (N to
   re-check)` under `BROOD_DERIVE_DBG=1`).
3. ~~**`nest check` parallel walk**~~ — **DONE 2026-09-22, ADR-386.** user/wall 11.1 on 12 cores (8.8 s against 55.9 s).
4. **Check memory** (Finding 2, option 3) — **mostly closed 2026-10-22.** Gate restated: peak ≤ load floor + the walk's state. The lints' aggregation was the spike; sequential is 0.80 GB, 0.2 GB over the floor. Open: the parallel arm's +0.7 GB (uncollected worker replies, allocator per-thread pages), and re-timing the 2026-10-23 load-path work on the test machine.
5. ~~**The cold load after an edit**~~ — **DONE 2026-10-22, ADR-419** (the module-level half): an
   edit reloads the edited modules and their dependents and materialises the rest; one edited
   module on the 302 × 3k rig 13.2 s → 2.7 s, and with unchanged sections COPIED rather than
   re-encoded (same day) `nest run` after an edit is **0.30 s**; check/test, which need every
   module, ~2.2 s. Parallel loading of a COLD build: **built 2026-10-23, ADR-426** (load 8.5 s → 2.1 s), **the default** since KI-290 explained bedit's differing image (Finding 3). The original note: (Finding 3) — no longer "paid once": since ADR-382 it is what an EDIT
   costs, because the edit invalidates the whole image and the next check loads everything
   from source before re-deriving two files. Parallel loading divides it by cores; module-level
   image staleness (materialise the unchanged modules, load the changed ones) makes it
   O(changed), which is the real fix and the harder one.
6. ~~Add the 3k-line shape to `scripts/bench/image-scale.sh`~~ — **DONE 2026-09-21**:
   `FNS=340 scripts/bench/image-scale.sh 250 500 1000`, with `warm all` (materialise
   everything — the `nest test` start) and `warm lazy` (image install only — the `nest run`
   start) columns. The script had driven ADR-325's OLD function names for a month with
   nothing running it; fixed with the row.

## Load- and check-path work removed, 2026-10-23 — NOT MEASURED

From a read-only review of the per-module and per-reference costs; **not measured — the bedit
startup and idle-check timings need re-taking on the test machine** before any of this is
counted as a win.

- `%refer`'s refer-all and the checker's module walk filter the globals on borrowed spellings
  under the table's lock (`Heap::global_symbols_where`) instead of copying every key and
  allocating every name, per `(:use …)` clause; the export and namespace indexes allocate a
  key per module, not per global.
- `Ctx::bind` consumes the scope: a loop of N binds made N whole-scope copies.
- The macro walkers ask `Heap::list_items` (no error for an atom) instead of `list_to_vec`,
  which allocated a boxed error per leaf; one walk for a four-item `fold`/`reduce` gates all
  three source rewrites, which each walked the whole form.
- `macroexpand_1` looks a head up once unless a require bound something; `resolve_sym`
  memoises `ns/bare` per namespace instead of formatting and re-interning it per reference.
- `require` asks `%builtin-module?` and copies a std module's source only when it loads from
  source, not on the stdlib-image path.
- Embedded modules are found through a hash index, not a scan of every table; a settled
  signature index costs a read lock per query, not the write lock; a curated-signature
  existence check no longer clones the `Sig`.
- The import table is an `ImportTable` that records whether any key may be an `(:alias …)`
  (`short/`) or a `(:use-internals …)` grant (`/internals/mod`), carried with the entries
  through every save/restore; the resolver, the privacy check and the auto-require inference
  skip spelling and interning a `prefix/` key — and the privacy check its grant key — in a
  file that declared none. The privacy check also stopped re-spelling `mod/bare`: unaliased
  it is the reference itself, rooted it is the rooted symbol.
- A free bare reference no longer interns `ns/name` to find it unbound — `mod/first`,
  `mod/map` were permanent entries in the never-freed symbol table, one per namespace per
  prelude name mentioned. It asks `intern_existing` (a spelling that is no symbol binds
  nothing) except where ADR-070's rooted fallback could still hit, and the memo remembers an
  absence stamped with a count of qualified names minted, so it re-asks under the interner's
  lock only after some `a/b` name was minted.
- `%std-edges-for` filters an image module's edges with the native `%baked-module?` (the
  embedded-module index: baked into THIS binary, a mounted bundle not counted) instead of
  building a set of every `reflect/builtin-modules` name per materialised module.

One ordering caveat, stated rather than hidden: the global table iterates in the hash order
of symbol ids, and the refer-all's shadow warnings follow it. Interning fewer names shifts
the ids minted later, so where two warnings fire from one `(:use …)` their relative order can
differ from before — as it already did between an imaged and a source boot (KI-166), and as
any change that interns a different set of names does.

Left as it was, deliberately: the expansion walk still rebuilds unchanged lists — a fresh
pair per occurrence is what keeps a spliced-twice macro argument from becoming one pair
compiled in two lexical contexts under the VM body cache's AST-identity key. (This note first
credited the resolve walk with that too, and both walks with separating the range-fold
fallback's copy of a body; they could not — the rewrites run after the expansion walk, and at
root `resolve` hardly rebuilds. The rewrites now copy what they duplicate themselves, through
`copy_form`, and the expansion walk gives a `quote` form cells of its own: KI-288.) With that,
`resolve_walk` hands back a list, vector, map or set none of whose items it changed
(`reuse_or_rebuild`): its only input is the expansion walk's output, one pair per position, and
returning a pair puts it back where it was. In a module most of a form is unchanged by
resolution — locals, prelude names, literals, quoted data — so most of its pairs, and their
`form_pos` entries, are no longer copied a second time. Not measured.

An incremental module-prefix index was not built: the refer-all's import order (and so the order of its shadow warnings)
is the global table's hash order, which no secondary index reproduces.
