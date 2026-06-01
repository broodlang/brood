# Making Brood LLM-native

A companion to [claude-demo-findings.md](claude-demo-findings.md). That doc
catalogues what tripped me up writing a non-trivial Brood program. This doc
is forward-looking: **what would make Brood a language LLMs genuinely
write well in**, beyond fixing the bugs in the other doc.

The framing matters. An LLM is a different user than a human:

- It reads 100K tokens in one shot but can't poke at the REPL.
- It makes speculative writes and benefits from fast structured feedback.
- It makes *consistent* mistakes (closures, patterns, macros) that a
  language can systematically prevent rather than just document.
- It often runs inside an agent harness with MCP / tool-call surfaces.
- It can't "feel out" idioms the way a human absorbs them from reading
  a codebase; it pattern-matches from what's most visible.

Treat the LLM as a first-class user with its own tool surface and the
opportunities open up dramatically.

---

## 1. Ship a Brood MCP server

Not "a docs site" — a tool surface. Expose these tool calls to any LLM
running in an agent harness:

| Tool | Returns | Replaces |
|---|---|---|
| `brood.lookup(name)` | signature, docstring, file:line, source body | "grep the prelude" |
| `brood.expand(form)` | macroexpanded form | reading `hatch.blsp` to know what `defprocess` does |
| `brood.eval(form, opts)` | `{value, type, stderr, time-ms}` JSON | `brood file.blsp` + parsing stdout |
| `brood.where-is(symbol)` | `"std/prelude.blsp:803"` | full-text search |
| `brood.find-pattern(intent)` | a runnable worked example | reading the entire `examples/` dir |
| `brood.explain-error(text)` | paragraph + linked docs section | guessing what the error means |
| `brood.check(snippet)` | structured lint findings | running and reading panic traces |

This collapses my 30-minute "spelunk through std/prelude.blsp" detour into
one tool call. **Highest ROI of anything in this doc.** Without it, every
LLM that touches Brood reinvents the same indexing work.

Implementation note: the kernel already has all the data — the env table
has names, locations, and arities; the macroexpander already runs. The
MCP server is mostly a thin JSON-RPC wrapper around APIs that exist.

---

## 2. Ship a Claude skill / plugin in the language repo

`.claude/skills/brood-dev/` checked into `mylisp/`. The skill bundles:

- The indexed prelude (names, signatures, examples) as a flat reference.
- A common-bug catalog (the "incarnations" file, below).
- MCP server wiring (#1 above) so the skill's tool calls just work.
- A system-prompt fragment with the do's and don'ts:
  > When writing Brood, prefer `match` over nested `if`; use `_` for
  > wildcard and `_x` for "I'm binding but won't use it"; reach for
  > `defprocess` over raw `receive` loops; use `transduce` instead of
  > chained `map`/`filter`; remember bare symbols in patterns *bind*…

`nest new` could optionally drop this skill into the new project so any
LLM walking into the repo is pre-armed. Cursor, Aider, Continue, etc.
all support similar formats — the work to ship one ports.

---

## 3. The "incarnations" file

A single `docs/incarnations.md` that every LLM appends to after a serious
session — what surprised them, what bit them, what worked. My
[claude-demo-findings.md](claude-demo-findings.md) would be the first
entry.

**The next LLM reads this first.** Self-improving documentation; you
didn't have to write any of it. Over time it becomes the highest-signal
"learn Brood" resource in the repo because it's grounded in real
attempts, not anticipated explanations.

Format suggestion:

```markdown
## 2026-05-28 — Claude Opus 4.7 — concurrent Mandelbrot
**Goal:** demo program exercising processes + transducers + macros.
**Blockers:** scheduler race; type-checker noise; formatter aggression.
**Surprises:** `defprocess` is in hatch (require 'proc/hatch); apply exists.
**What I'd tell next-me:** read std/prelude.blsp once; use -j 1 for
fan-out demos; the type-checker warnings about hatch macros are noise.
```

Tools for #1 could write this entry automatically when an LLM session ends.

---

## 4. Structured error values with stable codes

Today:
```
unbound error: unbound symbol: fold
```

Proposed: an *error value*, not just text:

```lisp
[:error :E0042
        :kind :unbound-symbol
        :name "fold"
        :scope :spawned-process
        :hint "scheduler race under -j 0 — try -j 1"
        :see "docs/concurrency.md#scheduler-race"
        :location "src/mandel.blsp:67"]
```

LLMs branch on the code (`:E0042` is stable across versions and
languages). Humans read the hint. `brood --explain E0042` prints the
full doc page. `try`/`catch` can match on `:kind` for programmatic
handling.

This is also how you make the language self-documenting: an error
*links to* the relevant docs, so an LLM that hits an error already
knows where to read more.

---

## 5. Macroexpand visible everywhere

Most Lisps have `(macroexpand 'form)` and most LLM Lisp code is wrong
because the LLM hasn't seen the expansion. Today `defprocess` is a black
box to anyone who hasn't read `hatch.blsp`.

Make it visible:

- **In the LSP hover** over a macro form, show the first-step expansion.
- **In CLI errors** that originated inside an expansion: "error inside
  expansion of `defprocess collector`; expanded form was: (receive
  ([:$cast …]) …)". This single change would have answered three of my
  debugging questions instantly.
- **Via the MCP tool** (#1 `brood.expand`).
- **In the docstring of every macro**, with a worked-example expansion
  baked in via metadata: `:expands-to '(receive (after ms nil)) nil`.

---

## 6. `brood --watch` as the LLM's REPL

Currently the LLM's edit-test loop is ~30 sec per iteration: edit file,
spawn `nest run`, parse stdout, repeat. A `--watch` mode that:

- re-evaluates the project on file save
- holds the runtime live between iterations
- emits structured JSON-lines: `{file, ok, errors[], top-level-values{}}`

…drops iteration cost by ~10×. The LLM tail-watches a file with
`Monitor`/`tail -f`; the language pushes updates as forms compile and
evaluate. **Functionally a REPL, but the surface is a file the LLM
already knows how to edit.** No "send a form, read a response" loop —
just write code and read trace.

Brood already has hot reload at the `def` level (ADR-026). Watch mode
is the missing wrapper.

---

## 7. A worked-example index keyed by intent

`examples/` today: `life`, `tour`, `processes` — named by *what they
are*. An LLM searching for "actor pool" finds nothing.

Restructure as `examples/by-task/`:

```
examples/by-task/
  actor-pool/              ;; fan out work to N workers, gather results
  concurrent-aggregator/   ;; state owned by a process, queried by call
  state-machine/           ;; tagged-data + multi-clause fn dispatch
  parse-and-transform/     ;; transducer pipeline over input
  cli-tool/                ;; nest run with args, exit codes
  long-running-server/     ;; hatch + supervision + reconnect
  ring-of-processes/       ;; the Erlang ring benchmark
  bench-something/         ;; how to measure correctly
```

Each is runnable and idiomatic. The LLM greps by intent ("I need an
actor pool") not by name ("which example was that again?"). Doubles as
test coverage — these can all run in CI.

---

## 8. Idiom-aware lints (the "you wrote it wrong" pass)

Catalog the LLM-specific mistakes — extracted from the incarnations
file (#3) over time — then catch them statically:

| Lint | Catches |
|---|---|
| `prefer-match` | nested `if` doing tagged dispatch |
| `prefer-and-or` | `if x (if y …)` composing booleans |
| `prefer-transduce` | chained `(filter pred (map f xs))` |
| `no-fn-send` | `(send pid (fn …))` — closures don't cross |
| `pin-or-bind` | bare symbol in pattern that should probably be `'sym` |
| `tail-position` | non-tail recursion on a value that could be large |

`nest check` runs them as advisory. **More valuable for LLMs than for
humans** because LLMs make these mistakes *consistently* — a lint that
fires 1% of the time for humans fires 60% of the time for LLMs.

---

## 9. A property-test syntax baked into the test framework

Brood's structural equality + pattern matching make property tests
*cheap*. LLMs love them because writing a property is shorter than
writing example cases. This is a natural fit you'd offer almost for
free.

```lisp
(prop "addition commutes" (a int? b int?)
  (= (+ a b) (+ b a)))

(prop "reverse is involutive" (xs list?)
  (= xs (reverse (reverse xs))))

(prop "match-and-rebuild" (m map?)
  (let ([k v] (first (map-pairs m)))
    (= (get (assoc m k v) k) v)))
```

Generators (`int?` / `list?` / `map?` / `string?`) ride on top of the
existing type predicates. Shrinking ride on top of structural equality.
The test framework already has `describe`/`test`/`assert=`.

---

## 10. A "Brood gauntlet" with `nest eval-llm`

50–100 small tasks with reference solutions:

```
gauntlet/
  001-fib/              ;; "write a tail-recursive fibonacci"
  002-bounded-queue/
  003-parallel-sum/
  004-mini-lexer/
  ...
```

Used for three things at once:

- **Benchmarking LLMs.** Opus 4.7 hits 84% — which 16% fail and why?
- **Test corpus for language changes.** Does the new `format` builtin
  break any solution? Did the eval rewrite regress process count?
- **Worked-example library.** The LLM greps the gauntlet by intent.

Run via `nest eval-llm --model opus-4-7 --tasks gauntlet/`. The 16%
failures *become* the language improvement backlog.

This is the loop that makes Brood self-improving for LLM use. Without
it, "is Brood getting easier for LLMs to write?" is unmeasurable.

---

## 11. `brood --think-aloud`

A verbose interpreter mode that prints, per form: what it parsed as,
what macros expanded into, what it called, what each call returned,
what types it inferred, what side effects it caused. Human-unreadable
in volume; **perfect for an LLM that wants to understand *why* its
program did what it did**.

Trace-as-tool. The LLM tail-watches the output, finds the line where
its expectation diverged from reality, and self-corrects.

```
$ brood --think-aloud src/mandel.blsp
:parse src/mandel.blsp:15 -> (defn cstep ([zr zi] [cr ci]) …)
:expand (defn …) -> (def cstep (fn …))
:bind global cstep -> Fn{2 args, 2 destructures}
:call cstep([0.0 0.0] [0.3 0.2])
  :destructure z -> {zr: 0.0, zi: 0.0}
  :destructure c -> {cr: 0.3, ci: 0.2}
  :prim %add 0.3 0.0 -> 0.3
  …
  :return [0.3 0.2]
```

---

## 12. Ship a demo prelude

A bundle of the things every demo needs but that aren't worth in the
strict core: `bench`, `format`, `parallel-map`, `pad-left`/`pad-right`,
`repeat`, `string-repeat`, ANSI helpers, `now-ns`, `round-to`. Opt-in
via `(require 'demo-prelude)` so it doesn't bloat the language.

The next demo-writer (human or LLM) doesn't reinvent the same five
helpers. Mine reinvented at least three.

---

## 13. Failure-mode tagging in errors

Beyond stable codes (#4): tag errors with what an LLM (or its harness)
should *do*:

```lisp
[:error :E0042 …
        :recoverable true
        :retry-with [:flag "-j 1"]
        :likely-cause :scheduler-race
        :user-fault false]
```

`:user-fault false` is the key. When the LLM gets an error from inside
the runtime (the multi-thread race), it shouldn't waste cycles trying
to fix its own code — the harness can branch on `:user-fault false` and
report upstream instead.

---

## 14. A "Brood-aware" prompt fragment shipped with the language

A small markdown file in `docs/` titled something like
`docs/prompts/system.md` that any LLM-using project can include in
its system prompt:

```
When writing Brood code:
- Prefer match over nested if for tagged-data dispatch
- Use _x (not _) when you're binding but won't use the value
- Reach for defprocess + hatch over raw receive loops
- Use transduce instead of chained map/filter
- Remember: bare symbols in patterns BIND; use 'sym to match literal
- Closures don't cross process boundaries via send; data does
- For benchmarks, prefer (bench expr) over hand-rolled (now) diffs
…
```

Drop this fragment into any Claude / Cursor / Aider system prompt and
get measurably better Brood code on day one. The maintainer updates it
as the language evolves.

---

## 15. An evaluation MCP tool that runs in-process

Beyond `brood.eval` in #1: a persistent runtime that the LLM connects
to via MCP and holds open across many tool calls. Each call evaluates
in the existing context — so a sequence of `def`/`def`/`call`/`def` is
incremental, like a long-lived REPL session, not a re-spawn-per-form.

This matters because the LLM's natural rhythm is "try a function, run
it, fix it, run it" — currently each cycle pays a startup cost. With
a persistent in-process eval, the LLM can iterate at REPL speed
without ever leaving the agent harness.

---

## Prioritisation: what would I build first?

If you can build only **one** thing on this list: **the MCP server
(#1)**. Every other idea benefits from it — the skill (#2) uses its
tools, the lints (#8) expose findings through it, the watch mode (#6)
is one of its endpoints, the gauntlet (#10) runs against it. It's the
substrate for everything else.

If you can build **three**:

1. MCP server (#1) — substrate.
2. Structured errors with codes (#4) — error values become *data* the
   harness can branch on instead of strings to parse.
3. Worked-examples-by-intent (#7) — the immediate "I need an actor
   pool" lookup that turns the prelude from prose into a recipe book.

If you can build **five**: add the incarnations file (#3) and
macroexpand-on-everything (#5). Both are nearly free to implement and
remove enormous classes of LLM confusion.

If you have a quarter: the gauntlet (#10) plus the watch mode (#6).
The first measures whether you're succeeding; the second tightens the
feedback loop that lets you succeed.

---

## The meta-point

A language that wants LLMs to write it well should treat the LLM as a
first-class user with its own tool surface — different from a human's
needs, not just "a worse human". The single biggest unlock is **the
MCP server (#1) + skill (#2) together**: they give the LLM precise,
structured, fast access to the language's own knowledge. The
incarnations file (#3) means the language gets *smarter about itself
over time* without anyone manually curating docs.

These ideas compound: structured errors (#4) feed the lints (#8) feed
the gauntlet (#10) feeds the next batch of language improvements. The
whole pipeline is self-reinforcing once the substrate exists.

The big bet is that Brood is small and young enough to design *for*
LLM use rather than retrofit. None of these ideas are possible to do
well in a 30-year-old language with a doc-stack that long predates
LLMs. Brood gets the rare chance to bake them in from the start.

---

## Status (2026-05-28)

| § | Item | Status |
|---|---|---|
| **1** | MCP server: `lookup`, `expand`, `eval`, `where-is` (via `lookup`), `check` | ✅ shipped (ADR-036, `docs/mcp.md`) |
| **1** | MCP server: `find-pattern`, `explain-error` | ❌ — need §7 / §4 first |
| **2** | Claude skill / plugin | partial — the `brood-task` MCP prompt (`docs/prompts/brood-task.md`) is the system-prompt fragment; the full skill bundle is TBD |
| **3** | Incarnations file | ✅ `docs/incarnations.md` (with `claude-demo-findings.md` as the first entry); exposed via `brood://docs/incarnations` |
| **4** | Structured error values with stable codes | ✅ shipped (`docs/error-codes.md`) — `LispError` carries `code` + `kind`; `catch` rebinds kernel errors to `{:kind :code :message :file :line :col :hint}` maps; MCP `error.data` projects the same shape; current codes `E0001`/`E0010`/`E0020`/`E0030`/`E0099` |
| **5** | Macroexpand visible: MCP tool, LSP hover, CLI-error expansion context, docstring `:expands-to` | partial — MCP `macroexpand` tool shipped; the rest TBD |
| **6** | `brood --watch` as LLM's REPL | partial — `--watch <file>` flag exists on `brood` and `nest run` (`std/tool/reload.blsp`); structured JSON-lines output TBD |
| **7** | Worked-example index by intent (`examples/by-task/`) | ❌ |
| **8** | Idiom-aware lints (`prefer-match`, `prefer-transduce`, `no-fn-send`, `pin-or-bind`, …) | ❌ |
| **9** | Property-test syntax `(prop "..." (a int?) ...)` | ❌ |
| **10** | `nest eval-llm` gauntlet | ❌ |
| **11** | `brood --think-aloud` | ❌ |
| **12** | Demo prelude | partial — some helpers exist (`format`, ANSI escapes); a packaged `demo-prelude` TBD |
| **13** | Failure-mode tagging in errors | ❌ — substrate (§4) now exists; specific `:hint` / `:see` attachments per error site are still to be added |
| **14** | "Brood-aware" prompt fragment | ✅ `docs/prompts/brood-task.md` — served via MCP `prompts/get` and reusable as a file by other agent harnesses (Cursor / Aider / Continue) |
| **15** | Persistent in-process eval | ✅ — that's the `nest mcp` session model (one long-lived `Interp`, ADR-013 hot reload) |

**Quick map of what's done vs. open per the prioritisation in this doc.**
The prioritisation block at the top picks §1 → §4 → §7 → §3 → §5 as the
order. **§1, §3, §4, §14, §15 are done**; the next-highest-leverage open
items are §7 (examples-by-intent — unblocks `brood.find-pattern`), §6
(`--watch --json`), and §5 finish-out (LSP hover macroexpand + CLI errors
that show the expansion context).

A small follow-up: **`nest new .`** currently fails (`.` isn't a valid
project name and the existence check trips). Allowing it to scaffold into
cwd (deriving the name from the cwd basename, skipping the file-exists
check) is a ~30-minute change in `std/tool/project.blsp` — separate from the
LLM-native plan but related to the agent-attach loop §1 cares about.
Recorded here so it doesn't get lost.

### Re-verification notes (2026-05-28, after this doc landed)

These confirm or refine the table above based on hands-on re-testing.

- **§1 — `nest mcp`** verified end-to-end: `nest new foo` writes `foo/.mcp.json`
  pointing at `nest mcp`. `cd foo && claude` auto-attaches. `eval` / `lookup` /
  `macroexpand` / `format` / `load` / `check` all visible via the JSON-RPC
  surface. Two remaining open items in `mcp.md` are real: `*out*` dynvar /
  `with-out-str` so `eval` returns `:stdout`, and the `find-pattern` /
  `explain-error` tools that wait on §7 / §4.
- **§3 — `incarnations.md`** verified: `claude-demo-findings.md` is the
  inaugural entry under `## Entries`, with the "what I'd tell next-me"
  abstract inline and the full writeup linked. Format conventions
  documented at the top of the file. Resource exposed as
  `brood://docs/incarnations`.
- **§4 — Structured error values** verified: tried `(try (no-such-fn)
  (catch e e))` via MCP `eval` — got back the expected map shape with
  `:kind :unbound :code "E0010" :message …`. The §4 contract is real.
  Process-death path (workers printing `process N died: unbound error: …`
  to stdout) doesn't yet route through this wrapper — separate
  follow-up.
- **§5 — Macroexpand** partial as marked: the MCP `macroexpand` tool
  works on `defprocess` and other hatch macros. The "in CLI errors
  show the expansion" piece and the "LSP hover shows expansion" piece
  aren't there yet. The single highest-leverage remaining piece is the
  CLI-error one — when a worker dies inside a `defprocess`-expanded
  receive, surfacing the expansion would have answered three of my
  debugging questions instantly.
- **§14 — Prompt fragment** verified at `docs/prompts/brood-task.md`.
  47 lines, points the agent at the three resources it should read
  first and the MCP tool surface. Good shape, ready to ship.

A couple of things on §2 (Claude skill) worth noting now that §1 + §14
exist:

- The `brood-task` prompt is the "system prompt fragment" half of §2.
- The "indexed prelude" half could be served as a single MCP
  *resource* (`brood://docs/prelude-index`) — a flat list of `name +
  signature + one-line doc + file:line`, generated from
  `%builtin-doc` plus the prelude. Cheap to produce, big lift for
  cold-start agents that haven't called `lookup` yet.
- The "common-bug catalog" half is now the `incarnations.md` file
  (§3).

So §2 may not need to ship as a single `.claude/skills/brood-dev/`
bundle — the substrate is already in place across §1 / §3 / §14, and
the missing piece is the indexed prelude resource above. Worth
considering before investing in a dedicated skill format.

### Performance — re-measured (informational)

Same three probes as `claude-demo-findings.md` §4, on the same
machine, against today's `brood`:

| benchmark | original | today | delta |
|---|---|---|---|
| 1M `(+ acc 1)` recursion | 3873 ms | 4400 ms | +14% (noise / GC; commit `bd4aa2d` adds mark-sweep GC) |
| 100k vec destruct + rebuild | 560 ms | 649 ms | +16% (same) |
| spawn 1000 processes | 17 ms | 13 ms | −24% (better) |

Spawn got cheaper, the arithmetic path got slightly slower (presumably
GC overhead now that the mark-sweep collector runs). The §4 takeaway
holds: the variadic-`+` 2-arg fast path is still the high-leverage win.
