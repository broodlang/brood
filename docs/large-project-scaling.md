# Large-project scaling — 100k files × 3k lines: what was measured, what it means, what to do

**Status: OPEN — measured 2026-09-21; item 1 (Finding 1) FIXED the same day, ADR-380.** The
queue is at the end, in order. Reproduce every number here with the generator before
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
python3 scripts/bench/gen-project.py 1000 /tmp/brood-big3k --fns 340   # 1 002 files, 3 067 lines each, 59 MB
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
| `nest check` again, nothing changed | **16 s** | 3.7 GB | should be ~0 — the ADR-129 cache re-verifies everything |

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
1. **Incremental for real (ADR-129 finished).** An unchanged project must re-verify from the
   cache in the time it takes to fingerprint it — seconds, not 16 s per 3M lines — and a
   change must re-derive only the changed files plus the files whose verdict depends on them
   (the ADR-119 dep recorder already knows the edges). This is the one that makes the
   day-to-day loop right at any size and is worth doing first.
2. **Parallel per-file walk** after the fixpoint: 12 cores → ~12× on the 90% of the time that
   is the walk. Needs the checker's per-file state to be `Send`, or a process-per-file model
   with the fixpoint's signatures handed in as data.
3. **Memory:** drop a file's forms after its walk, keep only its derived facts; and/or
   check in shards (the fixpoint over signatures only, then walks per shard).

## Finding 3 — cold load is one thread

39 s for 3M lines, single-threaded (`Building bigproj … Built in …`). Files in different
subtrees of the require graph could load in parallel processes and be promoted into the
shared RUNTIME region; the image write already serialises the result. Lowest value of the
three — it is paid once — but at 300M lines an hour becomes five minutes on a 12-core box.

## The queue

1. ~~**Module-index cache** (Finding 1)~~ — **DONE 2026-09-21, ADR-380.** Warm start O(files);
   zero source opens on a warm run; the 3k-line rig warm-runs in 0.15 s.
2. **`nest check` incremental** (Finding 2, option 1). Gate: unchanged project re-checks in
   ≤ 2× its fingerprint time; one edited file re-derives its dependents only (count the walks
   under `BROOD_DERIVE_DBG=1`).
3. **`nest check` parallel walk** (Finding 2, option 2). Gate: user/wall ≥ 6 on 12 cores.
4. **Check memory** (Finding 2, option 3). Gate: peak RSS at 1 000 × 3k under 1 GB.
5. **Parallel cold load** (Finding 3). Last; paid once.
6. ~~Add the 3k-line shape to `scripts/bench/image-scale.sh`~~ — **DONE 2026-09-21**:
   `FNS=340 scripts/bench/image-scale.sh 250 500 1000`, with `warm all` (materialise
   everything — the `nest test` start) and `warm lazy` (image install only — the `nest run`
   start) columns. The script had driven ADR-325's OLD function names for a month with
   nothing running it; fixed with the row.
