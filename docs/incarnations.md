# Incarnations

A self-improving record of what tripped real agents writing real Brood
programs. Each entry is one session: who, what they were doing, what
blocked them, what surprised them, what they'd tell the *next* agent.

**The next agent reads this first**, right after
[`brood-for-claude.md`](brood-for-claude.md). Over time this becomes the
highest-signal "learn Brood" resource in the repo because it's grounded in
real attempts, not anticipated explanations. Idea originally from
[`llm-native.md` §3](llm-native.md).

The MCP server exposes the index here at `brood://docs/incarnations`; full
findings docs at `brood://docs/<entry-slug>`.

## Format

Add a new entry at the **bottom** under `## Entries`. Keep it short — the
full writeup goes in `docs/<slug>.md`. The headers are the same every time
so the next reader knows where to look:

```markdown
## YYYY-MM-DD — Model — Task

**Goal:** one-line summary of what the agent was trying to do.
**Blockers:** the things that actually stopped or seriously slowed them.
**Surprises:** the "huh, didn't expect that" — both good and bad.
**What I'd tell next-me:** the one or two tips that would have saved an
hour. Cross-reference docs / files where useful.
**Full writeup:** [`./<slug>.md`](./<slug>.md)
```

When an agent finishes a non-trivial session, **append an entry here and
write the full findings into `docs/<slug>.md`**. The dispatcher
auto-publishes both (via `EMBEDDED_DOCS` + MCP `resources/list`); the next
session sees them on first connect.

## Entries

### 2026-05-28 — Claude Opus 4.7 — concurrent Mandelbrot demo

**Goal:** a "VERY complicated" demo program exercising math, recursion,
immutable maps, transducers, processes, `defprocess`/`hatch`, selective
receive, pattern matching, gensym macros, and timing.
**Blockers:** multi-thread scheduler race under default `-j 0` (fan-out
~20+ workers reliably crashes with bogus "unbound symbol" errors and a
Rust panic in `eval/mod.rs`); type-checker noise around
`(require 'hatch)` (five "unbound symbol" warnings on `defprocess` /
`cast` / `!` / `gen-call` look like errors); `nest format` collapses
multi-line `let` / `cond` / `defmacro` bodies onto 100+ char lines.
**Surprises:** `defprocess` lives in `hatch` (not the prelude); `apply`
exists but isn't in the quick-ref; ANSI escapes are first-class
(`examples/life.blsp`); float printing has no precision control.
**What I'd tell next-me:** read `std/prelude.blsp` once end-to-end before
starting a demo (the pocket reference is incomplete on `apply`, `now`,
`gensym`, `for`, `defprocess`, `hatch`, `!`, `gen-call`, `sleep`,
`pr-str`); use `-j 1` for fan-out demos until the scheduler race is
fixed; ignore type-checker warnings on hatch macros for now; pattern
destructure failures surface as Rust panics, not Brood errors — wrap in
`try` when unsure. The 511 lines of full notes are worth a read for
anyone touching the scheduler, formatter, or quick-ref.
**Full writeup:** [`./claude-demo-findings.md`](./claude-demo-findings.md)
