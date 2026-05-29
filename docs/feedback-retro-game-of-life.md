# Writing a Game of Life in Brood — a retrospective

**The prompt:** *"can you write a game of life application 80x40 with wrapping in
a loop"*

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

## Status — what's been addressed (updated 2026-05-29)

A first pass landed the contained, high-value items; the deeper ones are scoped
for follow-up. See the 2026-05-29 devlog entry and ADR-050.

> **Re-verification, 2026-05-29 (later run, newer binary).** The app was rebuilt
> from scratch (`foobar/src/life.blsp`) and every claim below was checked against
> the live image via the MCP server. The `nest` on PATH is now a fresh build
> (`~/.local/bin/nest`, rebuilt 21:12 — after this retro's 19:02 timestamp;
> 3.6 MB, `nest 0.1.0`, so leaner than the original *debug* build).
> - **All five "Done" items confirmed present and working** — `bound?`/
>   `all-globals` show `rng`/`rand-int`/`rand-float`/`rand-seed`/`shuffle`/
>   `sample`, the six `bit-*` ops, and `apropos`/`all-globals`/`doc-search`
>   (in-language + MCP); the scaffolded `CLAUDE.md` documents `:main`, and
>   `nest run --for 2s` exercised the infinite loop and exited cleanly. The PRNG
>   and `--for` were used directly in the rebuild.
> - **§8 leak is NOT fixed.** Re-sampled `/proc/<pid>/VmRSS` once per second over
>   a `nest run --for 20s` run (output to `/dev/null`): **41 MB → 1333 MB in
>   19 s**, dead-linear, monotonic, never plateauing — **~80 MB/s** (≈10 MB/gen at
>   the 120 ms tick). Same signature as below; only the baseline is lower (41 MB
>   vs 488 MB) on the leaner binary — **the slope, which is the bug, is
>   unchanged.** `--for` does bound it: the process exited at 20 s and RAM fully
>   recovered. Remains the highest-priority open item.
> - **Set type still absent** — `set`/`conj`/`union`/`difference` all unbound.

> **Re-verification, 2026-05-29 (third pass — a wrapping-grid rebuild, the leak's
> intended fix now exists but is a footgun).** Built the game again with wrapping
> against the live image (started 40×40, then 60×30, then 80×40 over successive
> "make it wider" / resize follow-ups).
>
> **The prompt** (verbatim): *"can you write a game of life application 40x40
> with wrapping in a loop."* The session then unfolded as a string of short
> follow-ups, each verbatim:
>   1. *"i don't think we need hibernate anymore. remove it"* — which my own
>      before/after measurement contradicted (a plain loop leaks ~600 MB/s), so I
>      surfaced the data and kept `hibernate`, just relocated it into a spawned
>      process where it's valid.
>   2. *"can you update this document with all your findings pleaes:
>      feedback-retro-game-of-life.md"* (this third-pass block).
>   3. a screenshot of a wrecked terminal: *"i get this and then the terminal
>      becomes unusable."* — the `hibernate`-escape crash skipping `term-leave`.
>   4. *"also mention what the prompt was and how long it took to actually finish
>      it."* (the note just below).
>   5. *"can you make it wider? the board?"* — generalised the single `*size*`
>      into `*cols*`/`*rows*` and went 40×40 → **60×30**; tests were rewritten to
>      derive expectations from the constants rather than hard-code 40.
>   6. *"then updat the prompt"* — i.e. keep this very record in sync.
>   7. *"on top of the doc, tell me the prompt 80x40"* — put the prompt at the top
>      of this doc and resized again to **80×40** (160 cols wide at 2 chars/cell);
>      with `*cols*`/`*rows*` already factored out, a two-constant change.
>
> **How long it took / effort shape.** No reliable wall-clock was captured, but
> the telling metric is the gap between "logic done" and "actually finished": the
> pure engine (`step`/`neighbors`/wrapping/`random-board`) was **correct on first
> validation** via the MCP image and never changed afterward. Everything *after*
> that — three run-it-and-fix iterations, each triggered by actually executing
> the program, not by reasoning — was the real work:
>   1. first `nest run --for` failed because `term-enter` needs a TTY;
>   2. switched to `hibernate` for the leak → it crashed at the script top level
>      (`hibernate escaped`) and, with `term-enter` still in, left the user's
>      terminal unusable;
>   3. moved the loop into a spawned process *and* dropped raw-mode for an
>      `(ansi-clear)` + `print` render → then hit `#<fn ansi-clear>` on screen
>      because the `ansi-*` helpers are functions, not strings.
> A later "make it wider" turn was by contrast a clean ~5-minute change
> (`*size*` → `*cols*`/`*rows*`, tests re-derived from the constants) — exactly
> the kind of edit that *is* fast, because it's pure logic with no runtime edge.
> Each of the three above, though, was a one-line-of-understanding fix that only
> surfaced by *running* it. The
> lesson echoes the first retro: the algorithm is the easy 20%; the long pole is
> the runtime/tooling edges (leak workaround, TTY, terminal teardown, doc
> mismatches) that a pure-function unit test never exercises.
>
> New findings, in priority order:
>
> - **`hibernate` now exists and *does* bound the leak — but it's undocumented
>   and only valid inside a spawned process.** `(hibernate fn & args)` discards
>   the call stack, flushes the local arena, and tail-calls `(apply fn args)` in
>   a fresh heap. Used as the loop's tail call it holds RSS **flat at ~81 MB over
>   12 s** (the high-water of one generation's allocation), and a *pure* spawned
>   hibernate loop ran **4.6 M iterations in 5 s at a flat 6 MB**. This is the
>   manual `flush` the §8 note predicted. Three sharp edges, though:
>   1. **It's undocumented.** `nest doc gui-draw`/`hibernate` print
>      `_Undocumented._`; `lookup`/`arglist` return `null`. The one primitive
>      that works around the headline bug has no signature or docstring — I found
>      its arity and arg type only by trial-and-error (and by an error message
>      that revealed `gui-draw` wants a *frame* vector while `hibernate` wants
>      `fn & args`).
>   2. **It crashes at a script top level.** Called directly from a
>      `nest run FILE` top-level form (which runs inline, *not* as a managed
>      process) it aborts the whole program with `internal: hibernate escaped:
>      hibernate`; the same "escape" happens in MCP `eval`. It only works once
>      the loop is wrapped in `(spawn (life-loop …))` with `main` parked on
>      `receive`. So the fix for the #1 bug is itself a footgun: the obvious
>      `(defn start () (life-loop board 0))` *crashes*, with an opaque message and
>      no hint that `spawn` is required.
>   3. **Combined with `term-enter` it wrecks the terminal.** The escape crash
>      skips `term-leave`, leaving raw mode + the alternate screen active — the
>      terminal is unusable until `reset`. (Screenshot reported by the user.)
> - **The leak itself is still unfixed and is board-size-sensitive.** A plain
>   tail-recursive loop on the 40×40 board climbed to **~3.7 GB in 6 s (~600
>   MB/s)** — far steeper than the 20×20 run's ~80 MB/s, consistent with
>   ~10–12 MB *per generation* times more generations and more cells. Same
>   monotonic-no-plateau signature.
> - **Doc bug: `ansi-clear` / `ansi-home` are zero-arg *functions*, not string
>   constants.** The `writing-brood` skill table calls them "escape strings you
>   print", so I `(str ansi-clear …)`-ed them and the screen filled with
>   `#<fn ansi-clear>`. They must be *called*: `(ansi-clear)` returns
>   `"\e[2J\e[H"` (clear **and** home, so `ansi-home` is redundant after it).
>   Fix the wording to show the call, or make them constants.
> - **`term-enter` / `term-draw` require a real TTY.** Under a non-tty
>   `nest run` (piped output, CI) they fail with `terminal: No such device or
>   address (os error 6)`, so a `display`/`term-*` TUI can't be exercised
>   headless. An `(ansi-clear)` + `print` render loop (no raw mode, no alt
>   screen) runs fine headless **and** survives Ctrl-C / `--for` without leaving
>   the terminal broken — the more robust default for this genre. The shipped app
>   now uses it.
> - **`step` is the per-frame cost.** The elegant
>   `(frequencies (mapcat neighbours (keys live)))` transition runs ~23 ms/gen on
>   40×40 (1000 steps = 23 s). Fine behind an 80 ms tick, but it's allocation-
>   heavy and is what feeds the leak.
>
> **Asks this pass:** (1) document `hibernate`/`gui-draw`; (2) make `hibernate`
> either work at a script top level or fail with a message that says "use inside
> a spawned process"; (3) have `nest run --for` run terminal teardown on the time
> cap (and a panic/Ctrl-C hook restore the alt screen, not just raw mode); (4)
> fix the `ansi-*` "string vs function" wording; (5) still — fix the underlying
> leak so `hibernate` isn't mandatory for every render loop.

> **Re-verification, 2026-05-29 (fourth pass — first build *after* the §8 leak
> fix; the leak is gone, but a doc trap this document already flagged bit me
> again, verbatim).** Built the game once more in `foobar/src/life.blsp`
> (~115 lines, one `life` module + `main`).
>
> **The prompt** (verbatim): *"can you write a game of life application 80x40
> with wrapping in a loop on a 60x40 grid."* Note the **internal contradiction**
> — "80x40" then "60x40 grid". I resolved it by asking a single clarifying
> question (80×40 chosen) rather than guessing; that was the right call and cost
> nothing. The session was otherwise two turns: build it, then one bug report.
>
> **The good news — §8 is genuinely fixed and needed no workaround.** This is the
> first pass on a binary with the entry-depth fix (§8 RESOLVED note). I wrote the
> obvious naive shape — `(defn life-loop (g gen) (do …(sleep …)(life-loop (step g)
> (+ gen 1))))` called straight from `main`, **no `spawn`, no `hibernate`** — and
> `nest run --for 1s` ran it and exited cleanly. No `hibernate escaped` crash, no
> terminal wreckage, no climbing RSS in the brief run. The third-pass footgun
> (hibernate-only-in-a-spawn) is simply gone: the naive loop is now the correct
> loop. Big improvement — the single highest-friction item across the first three
> passes no longer exists for `nest run`.
>
> **What I got wrong — the `ansi-*` string-vs-function trap, *again*, exactly as
> the third pass warned.** This is the headline of this pass. Both the
> `writing-brood` skill table and `docs/brood-for-claude.md` describe the helpers
> as "escape **strings** you `print`" (`ansi-clear`/`ansi-home`/`ansi-hide-cursor`).
> I trusted that wording, wrote `(print ansi-home)` / `(print ansi-clear)`, all six
> unit tests passed, the checker was clean, and I shipped it. On the user's screen:
> `#<fn ansi-home>` printed literally **and the animation never refreshed** (the
> bare symbol never emitted the cursor-home escape, so every frame just scrolled).
> They are zero-arg *functions* — `(ansi-home)`, `(ansi-clear)`. The third pass
> (line ~145) recorded this exact bug and asked (ask #4) to "fix the wording to
> show the call, or make them constants." **That fix was never applied, and it
> caused the only user-visible defect in the very next session.** Two consecutive
> passes, same trap, same root cause — this is now the strongest, most repeatable
> signal in the whole document.
>
> **My own process misses (both would have caught it pre-ship):**
>   1. **I didn't use the MCP `eval` loop the skill explicitly prescribes.** A
>      one-line `eval ansi-home` returns `#<fn …>` and `eval (ansi-home)` returns
>      `"\e[H"` — the bug is *visible in a single eval* at write time. I went
>      straight to `nest test`/`nest run` over Bash instead. The skill calls the
>      MCP image "your coding loop … how you check the code you're about to write
>      actually works"; I skipped it and paid for it.
>   2. **My verification hid the bug.** I ran `nest run --for 1s 2>&1 | tail -3`,
>      saw a grid and a `gen` line, and called it verified — but `#<fn ansi-home>`
>      was *in that very output*, masked by piping a full-screen TUI to `tail` and
>      by the grid overwriting the view. The check that actually proves a render
>      loop works is inspecting the **raw bytes for escape sequences**, which I
>      only ran *after* the user complained: `nest run --for 600ms 2>&1 | cat -v |
>      grep -oE '\^\[\[[0-9;]*[A-Za-z]'` → confirmed `^[[2J` once + `^[[H` per
>      frame. That assertion belongs *before* shipping, not after. Note the
>      deeper point: **`nest test` cannot catch this** — the tests exercise pure
>      functions (`step`/`render`/wrapping, all correct first try), never the
>      render loop's escape output. A green suite gave false confidence.
>
> **A smaller miss:** I modelled the board as a flat boolean vector with a full
> O(w·h·8) rescan per `step`, and never reached for the `(frequencies (mapcat
> neighbours (keys live)))` idiom this doc celebrates (§4.4). It's correct and
> readable, but it's evidence that the elegant combinator idiom still isn't
> surfacing at the point of writing — §4.4's "make it a worked example" ask is
> still open in practice.
>
> **Effort shape.** No wall clock, but the familiar pattern held even harder this
> pass: the *logic* was correct on first validation (6 tests green on first run,
> `nest check` clean) and never changed. The entire gap between "logic done" and
> "actually done" was **one known, documented, still-unfixed doc trap** plus the
> one user round-trip it forced. With the leak fixed, this genre is now *one
> doc-line away* from a clean one-shot.
>
> **Asks this pass, in priority order:**
> 1. **Apply the third pass's ask #4 now.** Fix the `ansi-*` wording in *both* the
>    `writing-brood` skill table and `docs/brood-for-claude.md` to show the **call**
>    form — `(ansi-clear)`, `(ansi-home)`, `(ansi-hide-cursor)` — and stop calling
>    them "strings." Every code reference should carry the parens. This is the one
>    change that turns this app into a first-try success; it has now demonstrably
>    cost two sessions.
> 2. **Make the failure mode self-evident — nothing in the toolchain catches a
>    function used as a value.** This is the deeper gap, and (per the maintainer)
>    a *previously-noticed, recurring* one, not specific to ansi. Concretely, in
>    this pass: my `src/life.blsp` contained `(print ansi-home)` and
>    `(print ansi-clear)` — `ansi-home`/`ansi-clear` are arity-0 functions, so
>    these print the *function object* (`#<fn ansi-home>`), never the escape
>    string. Yet:
>    - **`nest check` was completely clean** (empty output) on that file.
>    - **All six tests passed**, and the advisory type checker said nothing.
>    - There was **no warning, error, or hint at compile/check time** that a bare
>      function symbol was being passed where a value was wanted, or that a
>      zero-arg helper was referenced but never called.
>
>    The bug only manifested as literal `#<fn ansi-home>` text *on the user's
>    screen at runtime*. Because Brood is dynamic and a function is a
>    first-class value, `(print ansi-home)` is perfectly legal — but
>    passing a *function* to `print`/`str`/`println` (output sinks that want a
>    string-able value) is almost never intentional, and `(some-fn)` accidentally
>    written as `some-fn` is a classic LLM/typo slip. This is exactly the kind of
>    "silent-wrong" the duplicate-`main` warning (§5.1) was added to kill, and it
>    deserves the same treatment. Options, cheapest first:
>    - **Lint at check time:** flag a bare reference to a *known zero-arity*
>      global in argument position (especially to `print`/`println`/`str`/`format`)
>      as "function used as a value — did you mean `(ansi-home)`?" The checker
>      already knows arities; this is a pattern match over the CST.
>    - **Loud at runtime:** have `print`/`str` render a fn as `#<fn ansi-home —
>      call it: (ansi-home)>` (a hint in the very output the author is staring at),
>      instead of a bare `#<fn …>`.
>    - The type checker, if it tracked "this sink expects a stringable", could even
>      mark `(print <fn>)` advisory-wrong directly.
>
>    Either guard would have turned this from a shipped, user-reported defect into
>    a diagnostic at `nest check` — the difference between a one-shot success and a
>    round-trip. Generalize it: **using a function as a plain value is rarely
>    intended and currently has zero diagnostics; that's worth a lint regardless
>    of the ansi case.**
> 3. **Teach the render-loop verification reflex** (skill one-liner): "verify a
>    TUI/animation by grepping raw output for escape sequences (`cat -v | grep`),
>    not by eyeballing a piped frame — and remember `nest test` only covers the
>    pure functions, not the loop."
> 4. Surface the `frequencies`/`mapcat` neighbour-count idiom at point-of-writing
>    (skill example), per §4.4 — still not reached for by default.

> **Re-verification, 2026-05-29 (fifth pass — concise rewrite + supervisor; a new
> memory finding).** Follow-ups: *"make the code VERY concise … below 50 lines and
> use supervisor process"* and *"confirm we have stable memory afterwards."*
> Rewrote the game set-based (live cells = `{[x y] true}`) with the §4.4
> `(frequencies (mapcat neighbours (keys live)))` idiom — **49 lines**, 6 tests
> green, checker clean. The animator now runs as a child of a `std/supervisor`
> one-for-one supervisor (`start-supervisor [{:id :life :start (fn () (spawn
> (life-proc)))}]`), with `main` parked on `receive`. The set/`frequencies`
> formulation was validated through MCP `eval` *before* writing the file (the loop
> the third/fourth passes said I should use — this time I did, and it caught that
> `into {}` over a filtered `frequencies` keeps the *counts* as values, so I added
> a `map … [cell true]` to return a clean set).
>
> **New finding — the supervisor (spawned process) trades a flat profile for a
> bounded-but-spiky one.** I sampled `/proc/<pid>/VmRSS` once/sec over a
> `nest run --for 22s` run (output to `/dev/null`):
>
> ```
> 570 1140 114 657 1180 257 803 252 533 1078
> 234 770 215 512 1041 237 772 266 605 948   (MB)
> ```
>
> This is a **sawtooth, not a leak**: RSS climbs, the per-process GC fires, drops
> back to ~115–265 MB, repeats — and the *peaks do not trend upward* across the
> run (~1140 @2 s … ~948 @20 s). Bounded; the process exited cleanly at the cap
> and RAM recovered. **But** the high-water is ~1.1 GB for a game whose live state
> is ~800 cells — far spikier than the §8-fixed *top-level* loop, which runs nearly
> flat (~5 MB). The difference is **where the loop runs**: the §8 entry-depth fix
> makes a top-level `nest run` loop collect at the depth-1 safepoint, but here the
> loop runs inside a **spawned** green process (supervisor → `spawn` → `life-proc`),
> whose collector reclaims at a much looser threshold. So: **moving a render loop
> under a supervisor (or any `spawn`) silently changes its memory profile from flat
> to a ~1.1 GB sawtooth.** Bounded and correct, but worth a look — the
> spawned-process GC threshold appears far higher than the depth-1 path's, and the
> two should probably converge. (Left as-is at the maintainer's request; noted
> here only.) Allocation churn is also inherent to the elegant `mapcat`/
> `frequencies` `step` (~8 fresh coord vectors per live cell per generation) — a
> second reason the idiom we want to teach (§4.4) is allocation-heavy.

**Done:**
- **Standard PRNG** (§1, §4.1) — `rng`/`rand-seed`/`rand-int`/`rand-float`/
  `shuffle`/`sample`, pure & seedable (`[value next-seed]`), in `std/prelude.blsp`.
  ADR-050.
- **Bitwise operators** (§1, §4.3) — `bit-and`/`-or`/`-xor`/`-not`/`-shift-left`/
  `-shift-right` (Rust primitives; the PRNG is built on them).
- **Discovery / introspection** (§2, §5.5) — `apropos`/`all-globals`/`doc-search`
  in-language, and as three `nest mcp` tools. Answers "is there an RNG?" in one
  call.
- **`:main` in the scaffold** (§5.2) — `nest new`'s CLAUDE.md now shows the syntax
  and states the one-name-project-wide rule.
- **Duplicate-global warning** (§5.1, §6 "the bad miss") — `check-project`/
  `check-project-sources` now parse each source file's top-level def-style forms
  and warn (advisory, to stderr) when one name is defined in more than one file,
  so the silent two-`main` shadow surfaces at `nest run`/`nest test` pre-flight.
- **Bounded run mode** (§5.4, §2 "no good way to test a TUI/infinite-loop app") —
  `nest run --for DURATION` (`2s`/`500ms`/bare-ms) runs a loop/TUI for a bounded
  time then exits cleanly. The first-class `timeout Ns nest run`; makes the §8
  leak (and any time-based behaviour) reproducible in CI.

**Done (round 2, 2026-05-29 — a *second* GoL pass still spent ~30 probes on
builtin signatures; round 1 only solved discovery-of-existence. See the devlog
entry of the same date):**
- **Complete signature reference** — `nest doc --all` prints every public global
  in a fresh image (≈340 builtins + prelude fns/macros) with signature + one-line
  summary, generated live so it never drifts. The intended fix for probing names
  one at a time: read it once. `nest doc <module>` still covers opt-in modules.
- **`concat`** (§1 reflex) — variadic alias of `append` (folds over lists *and*
  vectors), so the universal Clojure reflex resolves to a real binding.
- **Simple terminal output** — `std/ansi.blsp` (opt-in): `ansi-clear`/`-cursor`/
  `-home`/`-hide-cursor`/… escape *strings* to `print`, the lightweight
  counterpart to the `display` render-op protocol. Plus the clarification that
  **`print` flushes every call** (the "no flush primitive" worry was a non-issue).
- **`--main`/CLI entry override** (§5.3) — `nest run --main module/fn` runs a
  one-off entry without editing the manifest; warns when a FILE is also given.
- **Skill gotcha table** — `docs/writing-brood-skill.md` now lists the reflexes
  that don't carry over (concat/conj/set!/loop-recur/flush/ANSI/RNG) and points
  at `nest doc --all`.

**Still open (mapped, not yet built):**
- **No diagnostic for a function used as a value** — a *recurring, previously-
  noticed* gap (maintainer), surfaced again in the fourth pass. `(print ansi-home)`
  on an arity-0 function passes `nest check` clean, all tests green, no
  warning/error — and prints `#<fn ansi-home>` at runtime. Same "silent-wrong"
  class as the duplicate-`main` shadow (§5.1) that already got a warning. Wants a
  check-time lint (bare reference to a known zero-arity global in
  `print`/`println`/`str`/`format` arg position → "did you mean `(ansi-home)`?")
  and/or a hinted runtime render. See fourth-pass ask #2 for detail. Highest-
  leverage of the open items because it's general, not ansi-specific.
- **Set type `#{}`** (§1, §4.2).

**Resolved since first recorded:**
- **`ansi-*` "string vs function" wording — FIXED 2026-05-29 (fourth-pass ask #1,
  third-pass ask #4).** The `writing-brood` skill table, `docs/brood-for-claude.md`,
  and the `std/ansi.blsp` module docstring now show the **call** form
  (`(print (ansi-clear))`) and state explicitly that the helpers are zero-arg
  functions that *return* an escape string — calling out that `(print ansi-clear)`
  prints `#<fn …>` and emits nothing. The skill `SKILL.md` symlinks to the docs
  file, so both share one corrected source. The deeper, general item below — *no
  diagnostic for a function used as a value* — remains the open follow-up.
- **§8 memory leak — FIXED and confirmed in the fourth pass.** The entry-depth fix
  (run `nest run <file>` through `eval_source` at depth 1) landed; §8's RESOLVED
  note has the root cause. The fourth-pass rebuild used the **naive** top-level
  tail loop — no `spawn`, no `hibernate` — and `nest run --for 1s` ran and exited
  cleanly with no RSS climb. `hibernate` is no longer needed in render loops.

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

> **RESOLVED 2026-05-29 (Stage B + the entry-depth fix).** Root cause finally
> pinned down: it was **not** that the app loop allocates (a tail loop *should*
> run flat) — it was that `nest run <file>` evaluated the program via
> `(load "path")`, which runs the file's forms **one eval frame deeper**
> (`gc_block_depth >= 2`). Stage B's automatic copying GC (ADR-055) only fires at
> the depth-1 eval safepoint, so a loop run that way never collected → the linear
> climb measured below. `brood <file>` never had the leak because it uses the
> depth-1 `eval_source` form loop. **Fix:** `nest run <file>` now evaluates the
> entry through `eval_source` at depth 1, exactly like `brood`. Same life-style
> loop, measured after the fix: **166 collections, ~5 MB live, flat** (was 0
> collections / 1.16 GB). `(hibernate)` is **no longer needed** for `nest run` —
> remove it from render loops. (Remaining depth-≥2 edges — `--watch`/`--for`
> wrapping and `:main` — still run one frame deep; see `docs/memory-review.md`.)

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

- This was the **debug build** (`target/debug/nest`). Re-run against
  `cargo build --release` first — but a *linear, unbounded* climb is not
  explained by debug overhead (which inflates the baseline, not the slope).
  **Update 2026-05-29:** re-sampled against a newer/leaner build (3.6 MB,
  rebuilt 21:12) — baseline dropped (~488 MB → ~41 MB at 1 s) exactly as a
  release-grade binary would, **but the slope was unchanged (~80 MB/s, linear,
  no plateau)**, confirming the prediction: the leak is in the runtime's
  generation reclamation, not in debug overhead.
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
