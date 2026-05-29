# Writing a Game of Life in Brood — a retrospective

> Feedback from an AI assistant (Claude Code) building a small app in Brood.
> Source app: `~/src/whk/foobar/src/life.blsp` — a 20×20 toroidal Conway's Life
> with periodic random injection, animated in the terminal.

## Summary

Built a 20×20 toroidal Conway's Life with periodic random injection, animated in
the terminal, in one module (`src/life.blsp`, ~95 lines). The *logic* went
smoothly and was correct on first validation. The friction was almost entirely
around **wiring the entry point** and **the absence of an RNG** — not the core
language, which behaved exactly as the docs promised.

---

## 1) Language issues

**No randomness anywhere in the language.** This was the single biggest gap. I
probed for every name I could think of:

```
rand, rand-int, random, rand-float, randint, shuffle  → all unbound (E0010)
```

For a game that "spawns random new ones," and more broadly for *simulations,
tests, sampling, shuffling, jitter, IDs* — this is a glaring hole. I worked
around it by hand-rolling a glibc LCG and **threading the seed through the game
state** (which, to be fair, is the idiomatically-correct immutable answer — no
global mutable PRNG). But every user writing anything stochastic will reinvent
this, badly, with poor constants.

**No bitwise operators.** `bit-and`, `bit-xor` are unbound. Minor on its own,
but it meant I couldn't use the usual bit-mixing tricks to improve the
low-quality low bits of the LCG; I compensated with `(quot seed 65536)` to use
high bits. Bit ops are table stakes for hashing, flags, and PRNG quality.

**No set type.** I represented live cells as a map `{[x y] true}` and used
`contains?` for membership. That works and is fast, but `{[x y] true}` is a tell
— the natural model for "a set of live cells" is a set. A `#{}` literal with
`conj`/`disjoint`/membership would have read much better and matched intent.

**Things that were genuinely good:** immutability never got in the way once I
accepted "thread the state." `frequencies` over `(mapcat neighbours (keys
live))` is a *beautiful* way to express Life's neighbour-counting — the whole
`step` is one `fold`. Vector keys with structural equality "just worked" in the
map. `mod` with negatives (`(mod -1 20) → 19`) made the torus wrap trivial. These
are the language at its best.

---

## 2) Tool issues

**The MCP server was the hero.** `eval`/`load`/`lookup` gave me a tight
write→load→test loop. I validated the torus with a wrapping blinker *before* ever
running the app:

```
gen0 ([0 5] [1 5] [19 5]) → gen1 ([0 4] [0 5] [0 6]) → gen2 (back to start), period-2 ✓
```

That caught nothing because the logic was right — but it *would* have caught an
off-by-one in the wrap, and it let me trust the code without running an infinite
loop. This is the right primitive.

**Missing introspection.** I twice reached for tools that don't exist:

- `(all-globals)` → unbound. I wanted to *discover* whether an RNG existed rather
  than guess names one at a time. An `apropos`/`all-globals`/`(doc-search
  "rand")` would have answered "is there randomness?" in one call instead of six
  `try` probes.
- `lookup-source` → unbound (I guessed the name; `lookup` is the MCP tool, but
  there's no in-image equivalent I could find).

**Reading long-line Lisp through Bash was painful.** `run-project`'s docstring is
one ~600-char line. My attempts to extract it with `sed`/`awk`/`cat -n | fold`
produced a cascade of empty/contradictory results and wasted a whole batch of
parallel calls. **This was partly my fault** — `Read` with `offset`/`limit` is
the correct tool and worked perfectly when I used it. Lesson for me (see §3), but
also: very long docstring lines make shell inspection of std awkward.

**No good way to test a TUI/infinite-loop app via MCP.** I can't `eval (start)`
— it never returns. I fell back to `timeout 1 nest run`, which worked great for
capturing frames, but a first-class "run N iterations / headless / frame-cap"
affordance would be cleaner.

---

## 3) Familiarity issues

These are *my* gaps, useful as signal for what a newcomer (human or model) trips
on:

- **I assumed an RNG existed.** Every language I know ships one. Burned several
  probe round-trips discovering it doesn't.
- **I didn't internalize the flat namespace's consequences.** I *read* "flat
  module system (ADR-019)" in the docs and still cheerfully defined a second
  `main`. Knowing the fact ≠ applying it. The implication — *there is exactly one
  of every name in the whole project* — needs to be stated as a **rule about
  naming**, not a fact about modules.
- **I guessed the `:main` manifest syntax** instead of looking it up (it's not in
  the scaffolded CLAUDE.md). Cost two iterations.
- **I over-trusted Bash for file reading** out of habit, when the dedicated
  `Read` tool was right there and better.

What the docs/skill *prevented*: I made zero of the "classic" mistakes they warn
about — no `[ ]` in binding position, no bare-symbol-as-literal in patterns,
tail-recursive loop, `:else` not `t`, flat `let`. The `writing-brood` skill
earned its keep. The traps that bit me were the ones *not* in it (RNG,
namespace-collision-on-`main`).

---

## 4) Things to lean into more — the language

1. **Ship a standard PRNG.** Even a documented, seedable, immutable one: `(rng
   seed) → [value next-seed]`, plus conveniences `rand-int`, `rand-float`,
   `sample`, `shuffle`. Offer both the pure threaded-seed form (idiomatic) *and*
   maybe a process-backed `*rng*` for scripts that don't want to thread. This is
   the #1 thing I'd add.
2. **A set type** (`#{}`), with `conj`/`contains?`/`union`/`difference`. The
   map-as-set workaround is common enough to deserve first-class support.
3. **Bitwise ops** (`bit-and`/`or`/`xor`/`shift`).
4. **Lean harder on `frequencies`/`fold`/`mapcat` in teaching material.** The
   fact that Life's entire transition is `(fold … (frequencies (mapcat
   neighbours (keys live))))` is a *fantastic* advertisement for the combinator
   style — it should be a worked example somewhere. Same for "maps are seqable" —
   folding directly over a `{coord count}` map is what made `step` so clean.
5. **2D/grid affordances.** Optional, but a tiny `grid`/`torus` helper (wrapping
   neighbours, render) would make the most common demo genre (cellular automata,
   roguelikes) trivial.

---

## 5) Things to lean into more — nest tooling

1. **Warn on duplicate global definitions across source files.** This is the big
   one. `run-project` already runs `check-project-sources` as an advisory
   pre-flight — but it silently let two `main` defns coexist, and the
   alphabetical-last-loaded one won with **no diagnostic at all**. A warning like
   `life/main shadows main/main (flat namespace)` would have saved the entire
   second debugging round-trip. The silent shadow is the worst failure mode in
   the whole session (see §6).
2. **Document `:main` syntax in the scaffolded CLAUDE.md.** It currently says
   only "override in `project.blsp` via `:main`" with no example. One line —
   `:main '(module fn)` or `:main module` — closes the gap.
3. **A CLI entry override.** `nest run life/start` (or `--main life/start`) for a
   one-off run without editing the manifest would be very handy, especially for
   projects with multiple runnable entry points.
4. **A headless/iteration-capped run mode** for testing loops (`nest run
   --max-frames 5` or similar), so TUI apps are verifiable in CI without
   `timeout`.
5. **MCP `apropos`/`all-globals`.** Discovery, not just lookup-by-known-name.

---

## 6) Error details

A mix of excellent and one bad miss.

**Excellent — structured unbound errors:**

```
{:line 1, :kind :unbound, :code E0010, :col 14, :message unbound symbol: rand}
```

Line, column, error code, kind, message. Easy to act on. The `try`/`catch`
returning these as values let me batch-probe a dozen names in one `eval`.

**Excellent — the manifest validation error:**

```
project.blsp:2:1: error: project: :main must be a module symbol or
  '(module fn), got life/main
    (project
    ^
```

File:line:col, a caret, *and the message tells you the valid forms and echoes
the bad value.* This is exactly how an error should read — it's self-documenting.
It's the reason I got the syntax right on the second try.

**The bad miss — the silent shadow.** After fixing the syntax to `'(life main)`,
`nest run` printed `hello foobar` and exited **with no error or warning
whatsoever.** Two valid `main`s, last-loaded won, nothing told me. A non-erroring
wrong result is the most expensive kind of failure — it sent me reading the std
`run-project` source to figure out *why* a syntactically-correct config did the
wrong thing. That should be a warning at minimum (see §5.1).

---

## 7) Feedback from running the program

I ran `timeout 1 nest run` and captured **7 full generations**. It works and
looks right:

- **Animation:** clean `\e[2J\e[H` clear+home each frame; no flicker artifacts in
  the capture, frames are whole.
- **Evolution looks like Life:** live-cell counts moved `77 → 72 → 58 → 68 → 59 →
  53 → 58` — falling as the random soup dies back, then bumping up on the gen-8
  injection. Recognizable still-lifes and oscillators form at the edges.
- **Wrapping confirmed both in the unit test and visually** (clusters at column 0
  interacting with column 19).
- **Performance:** trivially fast at 20×20; the 120 ms `sleep` is the only
  pacing. `nest test` runs in ~1 ms.

**Things I'd tune with more time:**

- **Cell aspect ratio.** I render each cell as two characters (`"##"`/`"  "`) to
  compensate for terminal cells being ~2:1 tall — it reads as roughly square,
  which is good. Worth noting this is a hack; a real renderer might use
  half-block glyphs (`▀`/`▄`) to double vertical resolution.
- **Density/injection tuning.** 90 initial cells (~22%) and 4 every 8 generations
  is a guess that produces a lively board; it's not derived from anything. Fine
  for a toy.
- **No quit-key handling**, only Ctrl-C. The loop has no input channel — adding a
  non-blocking keypress check would need a stdin-reading affordance I didn't go
  looking for.

---

## One-line takeaway

The **language core is excellent** for this — immutability plus
`fold`/`frequencies`/`mapcat` made Life elegant. The friction was entirely at the
edges: **no RNG** (language gap) and a **silent duplicate-`main` shadow** (tooling
gap). Fix those two and this exact app would have been a clean one-shot.
