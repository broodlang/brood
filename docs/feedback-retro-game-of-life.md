# Writing a Game of Life in Brood — a retrospective

> Feedback from an AI assistant (Claude Code) building a small app in Brood.
>
> **Recorded:** 2026-05-29 19:02 SAST. Findings — including the §8 memory leak —
> reflect the runtime *as of this timestamp*, against the debug build at the path
> below. Re-test against any `nest` binary built after this before assuming an
> item still stands.

### Paths

- **App built:** `~/src/whk/foobar/` (the `foobar` project; `src/life.blsp` is
  the game, ~95 lines).
- **Brood / nest project (the language + tooling being evaluated):**
  `~/src/whk/brood/` — `nest` on `PATH` resolves to `~/.local/bin/nest`, a debug
  build of `~/src/whk/brood/target/debug/nest` (ELF x86-64, not stripped).
  **All findings below are against the *debug* build** — the memory leak in §8
  especially should be re-confirmed against a `--release` build.
- **This document:** `~/src/whk/brood/docs/feedback-retro-game-of-life.md`.

## Summary

Built a 20×20 toroidal Conway's Life with periodic random injection, animated in
the terminal, in one module (`src/life.blsp`, ~95 lines). The *logic* went
smoothly and was correct on first validation. The friction was almost entirely
around **wiring the entry point** and **the absence of an RNG** — not the core
language, which behaved exactly as the docs promised.

**The most serious finding came after the app worked:** external `/proc`
sampling shows the running program **leaks memory linearly at ~100 MB/s and never
plateaus** (§8). For a bounded simulation (≤400 live cells) whose own state
cannot grow, this points at the runtime not reclaiming dead immutable
generations — a GC/allocator bug, not an app bug. It is the highest-priority item
in this document.

---

## 1) Language issues

**No randomness anywhere in the language.** This was the biggest ergonomic gap. I
probed for every name I could think of:

```
rand, rand-int, random, rand-float, randint, shuffle  → all unbound (E0010)
```

For a game that "spawns random new ones," and more broadly for *simulations,
tests, sampling, shuffling, jitter, IDs* — this is a glaring hole. I worked
around it by hand-rolling a glibc LCG and **threading the seed through the game
state** (the idiomatically-correct immutable answer — no global mutable PRNG).
But every user writing anything stochastic will reinvent this, badly, with poor
constants.

**No bitwise operators.** `bit-and`, `bit-xor` are unbound. Minor alone, but it
meant I couldn't bit-mix to improve the low-quality low bits of the LCG; I
compensated with `(quot seed 65536)` to use the high bits. Bit ops are table
stakes for hashing, flags, and PRNG quality.

**No set type.** I modelled live cells as a map `{[x y] true}` and used
`contains?` for membership. Works and is fast, but `{[x y] true}` is a tell — the
natural model for "a set of live cells" is a set. A `#{}` literal with
`conj`/`union`/`difference` would read better and match intent.

**What was genuinely good:** immutability never got in the way once I accepted
"thread the state." `frequencies` over `(mapcat neighbours (keys live))` is a
*beautiful* way to express Life's neighbour-counting — the whole `step` is one
`fold`. Vector keys with structural equality "just worked" as map keys. `mod` on
negatives (`(mod -1 20) → 19`) made the torus wrap trivial.

---

## 2) Tool issues

**The MCP server was the hero.** `eval`/`load`/`lookup` gave me a tight
write→load→test loop. I validated the torus with a wrapping blinker *before* ever
running the app (period-2 across the x=19↔0 seam). This is the right primitive.

**Missing introspection.** I twice reached for tools that don't exist:

- `(all-globals)` → unbound. I wanted to *discover* whether an RNG existed rather
  than guess names one at a time. An `apropos`/`all-globals`/`doc-search` would
  have answered "is there randomness?" in one call instead of several `try`
  probes.
- `lookup-source` → unbound (I guessed; `lookup` is the MCP tool, but there's no
  in-image equivalent I found).

**Reading long-line Lisp through Bash was painful.** `run-project`'s docstring is
one ~600-char line; `sed`/`awk`/`cat -n | fold` produced empty/garbled output and
wasted a parallel batch. `Read` with `offset`/`limit` is the correct tool and
worked perfectly — a lesson for me (§3), but also: very long docstring lines make
shell inspection of std awkward.

**No good way to test a TUI/infinite-loop app via MCP.** I can't `eval (start)` —
it never returns. I fell back to `timeout 1 nest run`. A first-class "run N
iterations / headless / frame-cap" affordance would be cleaner.

**Parallel-batch abort behaviour bit me hard during cleanup.** When one call in a
parallel tool batch exits non-zero, the whole batch is cancelled. `pkill` returns
1 when nothing matches, and `pgrep -c` returns 1 on a zero count — so batching a
"kill leftovers" command alongside edits repeatedly cancelled the edits. Run
cleanup commands solo, or make them always exit 0 (`… || true`, or wrap in a
count).

---

## 3) Familiarity issues

My own gaps — useful as signal for what a newcomer (human or model) trips on:

- **I assumed an RNG existed.** Every language I know ships one. Burned several
  probe round-trips discovering it doesn't.
- **I didn't internalize the flat namespace's consequences.** I *read* "flat
  module system (ADR-019)" and still defined a second `main`. Knowing the fact ≠
  applying it. The implication — *exactly one of every name in the whole project*
  — needs stating as a **naming rule**, not a fact about modules.
- **I guessed the `:main` manifest syntax** instead of looking it up (it's not in
  the scaffolded CLAUDE.md). Cost two iterations.
- **I over-trusted Bash for file reading** out of habit when `Read` was better.

What the docs/skill *prevented*: zero of the "classic" mistakes they warn about —
no `[ ]` in binding position, no bare-symbol-as-literal in patterns,
tail-recursive loop, `:else` not `t`, flat `let`. The `writing-brood` skill
earned its keep. The traps that bit me were the ones *not* in it (RNG,
namespace-collision-on-`main`).

---

## 4) Things to lean into more — the language

1. **Ship a standard PRNG.** A documented, seedable, immutable one: `(rng seed) →
   [value next-seed]`, plus `rand-int`, `rand-float`, `sample`, `shuffle`. Offer
   the pure threaded-seed form *and* maybe a process-backed `*rng*` for scripts
   that don't want to thread. The #1 thing I'd add.
2. **A set type** (`#{}`) with `conj`/`contains?`/`union`/`difference`.
3. **Bitwise ops** (`bit-and`/`or`/`xor`/`shift`).
4. **Lean harder on `frequencies`/`fold`/`mapcat` in teaching material.** Life's
   entire transition being `(fold … (frequencies (mapcat neighbours (keys
   live))))` is a fantastic advertisement for the combinator style — make it a
   worked example. Same for "maps are seqable": folding directly over a `{coord
   count}` map is what made `step` clean.
5. **2D/grid affordances.** A tiny `grid`/`torus` helper (wrapping neighbours,
   render) would make the most common demo genre (cellular automata, roguelikes)
   trivial.

---

## 5) Things to lean into more — nest tooling

1. **Warn on duplicate global definitions across source files.** The big one.
   `run-project` already runs `check-project-sources` as an advisory pre-flight —
   yet it silently let two `main` defns coexist, and alphabetical-last-loaded won
   with **no diagnostic at all**. A warning like `life/main shadows main/main
   (flat namespace)` would have saved the entire second debugging round-trip.
2. **Document `:main` syntax in the scaffolded CLAUDE.md.** It currently says
   only "override via `:main`" with no example. One line — `:main '(module fn)`
   or `:main module` — closes the gap.
3. **A CLI entry override.** `nest run life/start` (or `--main life/start`) for a
   one-off run without editing the manifest.
4. **A headless/iteration-capped run mode** for testing loops (`nest run
   --max-frames 5`), so TUI apps are verifiable in CI without `timeout`.
5. **MCP `apropos`/`all-globals`.** Discovery, not just lookup-by-known-name.

---

## 6) Error details

A mix of excellent and one bad miss.

**Excellent — structured unbound errors:**

```
{:line 1, :kind :unbound, :code E0010, :col 14, :message unbound symbol: rand}
```

Line, column, code, kind, message. The `try`/`catch`-returning-a-value form let
me batch-probe a dozen names in one `eval`.

**Excellent — the manifest validation error:**

```
project.blsp:2:1: error: project: :main must be a module symbol or
  '(module fn), got life/main
    (project
    ^
```

File:line:col, a caret, *and* the message states the valid forms and echoes the
bad value. Self-documenting — the reason I got the syntax right on the second try.

**The bad miss — the silent shadow.** After fixing the syntax to `'(life main)`,
`nest run` printed `hello foobar` and exited **with no error or warning at all.**
Two valid `main`s, last-loaded won, nothing told me. A non-erroring wrong result
is the most expensive failure mode — it sent me reading the std `run-project`
source to learn *why* a syntactically-correct config did the wrong thing. Should
be a warning at minimum (§5.1).

---

## 7) Feedback from running the program

`timeout 1 nest run` captured **7 full generations**:

- **Animation:** clean `\e[2J\e[H` clear+home each frame; whole frames, no
  tearing in the capture.
- **Evolution looks like Life:** live-cell counts moved `77 → 72 → 58 → 68 → 59 →
  53 → 58` — falling as the random soup dies back, bumping up on the gen-8
  injection. Recognizable still-lifes and oscillators form.
- **Wrapping confirmed** in the unit test and visually (column-0 ↔ column-19
  interaction).

Things I'd tune with more time: cell aspect ratio (I render each cell as two
chars to offset the ~2:1 terminal cell; half-block glyphs would double vertical
resolution), density/injection constants (currently guessed), and quit-key
handling (only Ctrl-C; the loop has no input channel).

---

## 8) Memory & CPU stability — a serious leak (external profiling)

After the app ran correctly I profiled it **from outside the process** with
standard Linux tooling (no app instrumentation, no debugger attached).
**The program leaks memory linearly and without bound.**

### What I measured

Sampled `/proc/<pid>/status` (`VmRSS`, `VmHWM`) and `/proc/<pid>/stat`
(`utime`/`stime`) once per second while `nest run` drove the game with output
redirected to `/dev/null` (so the terminal animation isn't a variable). Raw data
was logged to `/tmp/gol-mem.csv`; excerpt:

```
sec   rss_kb     vmhwm_kb   utime  stime
1     488080     488080     75     11
5     1013072    1013072    157    23
10    1506704    1506704    238    35
20    2640100    2640100    430    65
30    3732404    3732404    627    95
33    3980196    3980196    669    102
```

### The finding

- **RSS climbs dead-linearly ~477 MiB → ~3.8 GiB in 33 s** — roughly
  **+100 MB/s**, i.e. **~12 MB per generation** at the 120 ms tick (~8 gens/s).
- **`VmHWM` == `VmRSS` at every sample** — peak tracks current size exactly, so
  memory is *never* reclaimed. Monotonic growth, not GC sawtooth.
- It does not plateau. (62.9 GiB RAM here, but a second uncleaned run later drove
  used memory past 30 GiB — a smaller machine would OOM in under a minute.)
- **CPU is modest:** `utime` 75→669 ticks (`CLK_TCK`=100) ≈ 6.7 CPU-s over ~32 s
  wall ≈ **~21% of one core**; `stime` ≈ 0.9 s. Not a busy-loop — the cost is
  allocation churn, not computation.

### Why this is almost certainly a runtime bug, not an app bug

The game's entire mutable footprint is the live-cell map, **bounded at 400
entries** on a 20×20 board. `life-loop` is tail-recursive; each generation builds
a fresh map and drops the previous one; `seed` and `gen` are integers. **Nothing
in the program retains old generations** — so ~12 MB/generation of permanent
growth means the runtime is holding dead immutable structures (GC not
running / not reclaiming, an allocator that never releases, or a per-iteration
arena that isn't reset). A pure, immutable, bounded-state loop is the *ideal* GC
stress test, and it failed it.

### Caveat & next steps

- This is the **debug build** (`target/debug/nest`). Re-run against
  `cargo build --release` first — but a *linear, unbounded* climb is not
  explained by debug overhead (which inflates the baseline, not the slope).
- To localize it in the Rust runtime (neither tool is installed yet —
  `sudo apt install valgrind heaptrack`):
  - **`heaptrack nest run`** → `heaptrack_gui` / `heaptrack_print`: best native
    heap-profiler UX; allocation backtraces and growth-over-time.
  - **`valgrind --tool=massif nest run`** + `ms_print`: heap snapshots over time;
    slower but precise.
  - Already present: **`perf record -g`** for allocation hot paths; **`pmap -x
    <pid>`** to see which mapping grows (main heap vs. mmap arenas).
- Cheap in-runtime check: log GC invocation count / bytes reclaimed per
  generation. If GC never fires (or reclaims ~0) during the loop, that's the bug.

---

## How I debugged the issues

A record of method, since you asked — roughly in the order problems surfaced.

**1. Validate logic in the live image *before* running the app (MCP server).**
The tightest loop was `write → mcp.load → mcp.eval`. I proved the torus and the
rules with targeted evals rather than by watching the animation:

- A **blinker straddling the x=19↔0 seam** to test wrapping: ran `step` three
  times and asserted it returned to the start (period-2) — would catch any
  off-by-one in the `mod` wrap.
- A **`render` shape assertion** (20 rows, expected width, both corners marked)
  via `string-split` on the output.

Highest-leverage habit here: an infinite-loop TUI app is awkward to inspect, but
its *pure functions* are trivial to unit-test through `eval`.

**2. Discover missing builtins by batch-probing, not one-by-one.**
To learn whether an RNG existed I evaluated *one* expression that
`try`/`catch`-wrapped a dozen candidate names (`rand`, `rand-int`, `random`,
`shuffle`, `bit-and`, …) and returned a vector of `[:name result]` / `:no-name`.
One round trip enumerated the whole gap instead of a dozen unbound-symbol errors.

**3. Diagnose the silent `:main` shadow by reading the std source.**
When `nest run` printed the greeting with no error, eval checks confirmed both
`main` and `start` were `fn?` — so the bug wasn't in my code. I `grep`-ed the
Brood std for the entry-point machinery
(`grep -rn "must be a module symbol" ~/src`), then read
`std/project.blsp` → `run-project`, and saw it `require`s the module then `eval`s
the **bare `fname` symbol** against the flat global table — proving
last-loaded-wins and that two `main`s collide. The fix (a unique entry name)
followed directly from ~20 lines of runtime source.

**4. Profile memory/CPU externally with `/proc`, driven by a background sampler.**
No debugger or app instrumentation:

- Identified the process: `command -v nest` + `file -L` confirmed a single ELF
  (no forked worker to chase), so `$!` from the launch was the PID to watch.
- Wrote a bash sampler (`/tmp/gol-monitor.sh`) that launches the game with output
  to `/dev/null`, then once a second reads `VmRSS`/`VmHWM` from
  `/proc/$PID/status` and `utime`/`stime` (fields 14/15) from `/proc/$PID/stat`,
  appends a CSV row, and `awk`s a min/max/growth summary at the end. Ran it via
  the Bash tool's `run_in_background` and used the `Monitor` tool to wait for a
  `DONE` sentinel.
- The CSV showed the leak by inspection (monotonic, linear) before the summary
  even computed; I killed the process early once the slope was unmistakable.

**Tooling notes from the debugging itself:**

- `Read` with `offset`/`limit` is the right way to inspect long-line Lisp; doing
  the same with `sed`/`awk`/`cat -n | fold` produced garbled/empty output (§2).
- **Foreground `sleep` is blocked by the harness** (it killed an inline cleanup
  with exit 144). Anything that needs to wait — sampling loops, "settle then
  inspect" — must run via `run_in_background` / `Monitor`, not an inline `sleep`.
- **A parallel batch is cancelled if any call exits non-zero.** `pkill`/`pgrep
  -c` return 1 on no-match/zero-count, which repeatedly cancelled sibling edits.
  Run cleanup solo or force exit 0.
- Always confirm the process is dead afterward (`pgrep -af nest`; `free -m`). A
  ~100 MB/s leaker left running between turns is a real hazard — I verified RAM
  recovered before moving on. (One run was left alive across turns and pushed
  used memory past 30 GiB before I caught and killed it — see §8.)

---

## One-line takeaway

The **language core is excellent** for this — immutability plus
`fold`/`frequencies`/`mapcat` made Life elegant, and the MCP image made the logic
verifiable without running the app. But two findings dominate: a **runtime memory
leak** (~100 MB/s, unbounded — §8, **fix first**) and, at the ergonomic edges,
**no RNG** plus a **silent duplicate-`main` shadow**. Fix the leak, ship a PRNG,
and this genre of app is a clean one-shot.
