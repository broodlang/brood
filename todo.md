# TODO

Running scratch list of work to pick up. Promote items to `docs/roadmap.md` /
an ADR once they're committed to. Newest section at the top.

## Process `kill` primitive + per-test timeout (30s) (2026-05-30)

Two linked pieces. The timeout depends on `kill` (without it a timed-out test can
only be abandoned as a background zombie — there's no way to stop it).

### A. `(exit pid reason)` — terminate a green process (kernel; ADR-worthy)

**API: Erlang `(exit pid reason)`** (chosen 2026-05-30). `reason = :kill` is the
**untrappable hard** kill; any other reason is the **soft/trappable** signal.

**Validated mechanism** (against the current `scheduler.rs`; no coroutine can be
aborted mid-compute on another worker, so the kill is checked at yield points):

- **Per-mailbox kill flag.** Add to `Mailbox` an `AtomicBool kill_pending` + the
  reason (a `Message`) under the existing `mailbox.state` lock. `exit` looks up the
  target via `REGISTRY` and sets both. Cheap to check; reason read only when the
  bool is set.
- **`Suspend::Kill(reason)`** — new variant alongside `Preempt`/`Receive`. In
  `preempt()` (already the yield path, reached every ≤`REDUCTION_BUDGET`=2000
  reductions), after refreshing the budget check the current proc's `kill_pending`;
  if set, `(*yptr).suspend(Suspend::Kill(reason))` instead of `Suspend::Preempt`.
  `run_one`'s match handles `Kill(reason)` by `deregister(proc.pid, reason)` and
  **dropping** `proc` (never re-enqueue) → coroutine + heap freed. Untrappable **by
  construction**: it bypasses Brood `%try` entirely (scheduler-level, like the
  existing overflow-guard / panic→`:killed` paths). This is the hard `:kill`.
- **Parked target** (suspended as `mailbox.state.waiter`, not running, so `tick`
  never fires): `exit` must, after setting the flag, take the `waiter` under the
  state lock and `deregister(reason)` directly (it never resumes). If RUNNABLE in a
  worker queue, it'll resume, hit `preempt`, and self-kill — fine.
- **Soft (reason ≠ `:kill`)**: check the flag at `%receive` too (the per-iteration
  boundary) so a server loop dies *between* messages with `reason`; a trap-exit
  flag to *handle* it instead of dying is a later add (ADR-011 — defer).
- `:down` carries `reason` (`:killed` for `:kill`). Self-exit, double-exit,
  exit-of-dead-pid: idempotent no-ops. Remote pids: error for now (defer dist).
- Tests: hard-kill a tight CPU loop (the case soft can't catch); soft-kill a
  receive loop; kill a parked process; monitor sees the right `:down` reason;
  across cores.

### A′. MCP terminal-output corruption — ✅ DONE (2026-05-30)

The actual `term-draw` "hang" was **stream corruption**, not a hang: `term-draw` /
`term-emit` write crossterm escapes straight to fd 1, which under `nest mcp` is the
JSON-RPC channel (the Brood-`print` capture didn't catch them — different write
path). Fixed in `builtins.rs`: both now build into a buffer and go through
`write_term_bytes`, which **diverts into the active MCP stdout-capture** (riding
back in the result content) instead of the raw fd. Test:
`mcp::tests::term_draw_under_mcp_diverts_escapes_…`. (A timeout wouldn't have helped
— term-draw returns in ms; the damage was the bytes.)

### C. MCP `eval`/`load` timeout = **30s** — ✅ DONE (2026-05-30, inline deadline)

A runaway MCP `eval`/`load` no longer wedges the server. **Not** via spawn-and-kill
(that relocates execution and breaks the dispatcher's error/panic/output handling —
proved: it failed 4 core MCP tests). Instead an **inline deadline**: the dispatcher
`set_deadline(now+30s)` around the call (`crates/nest/src/mcp.rs`), and eval's
`'tail:` loop checks `process::deadline_exceeded()` per combination (clock read only
every ~1024 ticks — the no-deadline path is one `Cell` get). A runaway surfaces as an
ordinary error ("evaluation exceeded its time limit"), so the dispatcher's existing
error/panic/capture handling is untouched. Verified:
`mcp::tests::eval_deadline_aborts_a_runaway_inline` (a `(ginf)` infinite loop aborts
at ~300ms). **Limit:** a *native* blocking call still can't be interrupted (it never
reaches the check — same as `(exit … :kill)`); that's what Fix A (A′) already covers
for the term-draw case. Also kept from this work: **output capture is now
process-scoped + inherited** (scheduler `Ctx.capture`), so an MCP eval that *spawns* a
printing process still diverts that output off the JSON-RPC channel.

#### (historical) earlier dead-end — kept for the record

The watchdog **logic is built + tested**: `mcp-run-guarded` (std/mcp.blsp) spawns the
handler, `(receive (after ms) → (exit p :kill) + error)`; verified directly (a
runaway is killed at ~205ms, fast returns, ms≤0 inline). It is **not wired into
`call_tool`** because of the capture problem below.

**The blocker (proven 2026-05-30):** a killable handler must run in a SPAWNED process
(worker thread), but its `print`/term output must still be captured off the JSON-RPC
channel. Tried making `STDOUT_CAPTURE` **global** (cross-thread) so the spawned
handler's output diverts — it **races under concurrent captures**: cargo runs the
MCP tests in parallel (each its own MCP server, one global buffer), and `begin`/`take`
clobbered each other → 5 tests failed + term-draw escapes leaked. Reverted to
thread-local. So:
- thread-local capture: safe under concurrency, but doesn't reach a spawned worker
  (handler output escapes) ✗
- global capture: reaches the worker, but races overlapping captures ✗

**The correct fix** is **per-capture-session** state: `begin_stdout_capture` mints an
`Arc<Mutex<String>>`, the spawned child is given a clone (plumbed through `spawn` into
process state) so its output appends to the SAME buffer, `take` drains it. No global,
no race, reaches the worker. That's a kernel change (process-state capture handle +
spawn plumbing) — a focused follow-up. Until then, **Fix A (A′) already resolves the
actual reported wedge** (term-draw corruption); a *hung infinite-loop eval* via MCP is
the only remaining gap, and it's rare.

For a genuinely infinite eval (`(loop)` via MCP), the synchronous handler hangs the
server; `:kill` *can* stop an infinite Brood loop (it ticks reductions), so a
watchdog works — but only for `eval`/`load` (the `run-tests` MCP tool legitimately
runs >30s; don't wrap it). **Decision needed before building:** the handler must run
in a spawned process to be killable, but `STDOUT_CAPTURE` is **thread-local to the
MCP thread**, so a spawned handler's `print`s would escape to JSON-RPC. Options:
(a) make capture cross-thread (`AtomicBool active` + `Mutex<String>` — ~free when
off, one atomic load per print); (b) a `%with-output-capture` primitive the spawned
child calls, shipping captured text back. (a) is cleaner for `call_tool` but changes
the global `print` path. Confirm approach before touching the print hot path.

The `nest mcp` `eval` tool can block indefinitely — e.g. `(term-draw …)` needs a
TTY and hangs headless, wedging the whole MCP session (observed 2026-05-30). Run
the tool body in a spawned green process, `monitor` it, and `(receive (after
30000 …))`; on timeout `(exit pid :kill)` it and return a "timed out after 30s"
error result instead of hanging. Same deliver-to-mailbox shape as the test runner;
matches the 30s test budget. Depends on `exit`. (Make it a shared
`*mcp-tool-timeout-ms*` knob, default 30000, like the test thresholds.)

### B. Per-test timeout = **30s** (uses `kill`) — ✅ DONE (2026-05-30)

Implemented in `std/test.blsp`: `*test-timeout-ms*` (default 30000), threaded as a
batch deadline into `collect-loop`'s `(receive … (after …))`. On timeout the
straggler workers are `(exit pid :kill)`'d and reported as "timed out after Ns"
failures; `:unit-result` is now hardened to ignore late messages from killed/zombie
workers (so they can't corrupt a later batch's count). Override via
`(run-tests :timeout MS)`. Verified: a 2.5s test under a 1s budget is hard-killed at
1.0s and fails; all spawn/receive suites (exit/set/maps/concurrency) still green.
Original design notes below (kept for reference).

A test/unit running > 30s **fails** with "timed out after 30s" and the slow worker
is killed (hard) so it stops consuming a worker. Default ON at 30s; overridable
`(run-tests :timeout MS)`.

- Thread `timeout` (default 30000) through `drain-runner → run-driver → run--step
  → collect-units → collect-loop`. Workers in a batch start together, so a wall
  deadline `= collect-start + timeout` ≈ per-test in the default one-unit-per-test
  case (per-unit for `:serial`/`:isolated`).
- `collect-loop`: bare `(receive)` → `(receive (msg …) (after (max 0 (- deadline
  (now))) <kill still-unreported workers; fail their units as timeouts>))`.
- **Harden `:unit-result`**: ignore a result whose pid isn't in the current step's
  `workers` or is already `reported` (so a late message from a killed/zombie worker
  can't corrupt a later step's count). `:down` already validates pid; `:unit-result`
  is currently trusted unconditionally.

Not started — `kill` first (kernel), then wire the timeout to it. Do it with the
full suite green to regression-test the (delicate) collector. NB the suite already
has a ~104s test group — a 30s per-test budget would need that group's individual
tests to each be < 30s (likely fine; confirm).

## Error message: a value in head position should hint C-style call syntax (2026-05-30)

`(println println("foo"))` — i.e. someone wrote `println("foo")` (C/JS call
syntax), which *reads* in Brood as two forms `println ("foo")`, so the inner
`("foo")` evaluates `"foo"` as a call head:

```
brood> (println println("foo"))
type error: cannot call non-function: "foo" (line 1, col 17)
```

The message is technically right but unhelpful — it doesn't surface the actual
mistake. When the "cannot call non-function" head is a **literal** (string /
number / etc.), that almost always means `name(args)` C-style call syntax
mis-parsed. Enrich it, e.g.: *"cannot call non-function: "foo" — a value can't be
called. In Brood the function goes inside the parens: `(f x)`, not `f(x)`."* Even
better if the reader/checker can see the adjacent `name(` (a symbol immediately
followed by `(` with no space) and say *"`println(...)` looks like a call —
write `(println ...)`."* (Reader-level adjacency detection is the most robust spot;
relates to the function-as-value lint just added to the checker.)

## GoL findings 2026-05-30 (`docs/gol-findings-2026-05-30.md`) — to action

- ✅ **`contains?` was O(n)** (their #1, the headline — ~100× slower than `get`;
  the real cause of "very slow"). Fixed in `std/prelude.blsp`: `contains?` now
  probes via the O(1) `map-get` hash path (two-sentinel trick) instead of scanning
  `(map-pairs m)`. *Verify once the build is green* (`tests/maps_test.blsp` should
  drop sharply in time/RSS; the set module's membership rides on this).
- ⬜ **`[DBG] child N …` spam on every `spawn`** (their #2) — leftover `eprintln!`
  on the spawn/coroutine path; corrupts TUI/`nest run` output. Locate (likely the
  process/scheduler spawn path) and gate behind a debug flag or remove. **NB:** may
  be the maintainer's *active* debugging (cf. the `BROOD_TRACE_SAFEPOINT` trace in
  `eval/mod.rs`) — confirm before deleting.
- ⬜ **Spawned-process GC threshold vs the depth-1 path** (their #4) — a render
  loop under a `spawn`/supervisor shows a bounded ~1.1 GB sawtooth, while the same
  loop at the depth-1 entry path runs ~flat (~5 MB). Bounded + correct, but the two
  GC thresholds should probably converge so "move the loop under a supervisor"
  doesn't silently 200× the high-water.
- ⬜ **Unused-`require` lint** (their #5) — a dead `(require 'x)` (module's symbols
  never referenced) goes unflagged. Cheap checker addition; same advisory channel
  as the function-as-value lint.
- 📝 **Concurrency teaching** (their #3) — naïve per-generation fan-out lost to
  serial (coordination + serial fan-in merge + per-`send` deep copy swamp a small
  parallel region). The honest "how to parallelise a CA" is spatial tiling + halo
  exchange. Worth a teaching note; *not* a "make it concurrent to make it fast"
  reflex. (The "`nest test` gives false confidence" + "use the MCP `eval` loop"
  notes are already folded into the `writing-brood` skill this session.)

## BUG: receive loops weren't TCO'd → coroutine-stack SIGSEGV ✅ FIXED (2026-05-28)

A server driven through ~60 interleaved cast + call cycles segfaulted: `%receive`
*ran* the matched body thunk itself (`eval::apply`) and returned its value, so a
loop that tail-called back into `receive` nested a `receive_match` per message and
blew the green-process ~128 KB coroutine stack. Fix (a trampoline): `%receive` now
**returns** the matched/timeout thunk, and the `receive` macro applies it in tail
position — `((%receive …))` — so eval's existing TCO loops it in O(1) native stack.
`receive--split` always supplies a do-nothing timeout thunk so the wrap always has
a fn to apply. Regression test: a server handling 500 interleaved cast+call cycles
(`tests/concurrency_test.blsp`). Unblocked `examples/life.blsp`.

## Idea: a better way to do module docs

Function/macro/`defprocess` docstrings are solid now (`(doc f)`), but module-level
docs are still just "a bare string as the first top-level form." For anything
bigger we want a real mechanism — e.g. a `defmodule`/`module` form (name + doc +
maybe exports), or a doc form the `nest doc` walker recognises — so a module's
purpose is queryable the way a function's is, not a loose string. Not committed;
revisit when the module story grows.

## Possibility: compile a `nest` project into a standalone binary

Status: **idea, not committed** (discussed 2026-05-27). Captured here so the shape
doesn't have to be re-derived.

**Key call — bundle, not AOT.** Brood is a tree-walker and `def`-rebind hot reload
(the shared *mutable* RUNTIME table) is load-bearing, so "compile to a binary"
means *embed the runtime + the project's code image into a self-contained
executable* (the `deno compile` / Erlang escript model) — **not** AOT-to-machine
code, which would fight the late binding that's the whole point.

Most machinery already exists:
- `include_str!` already bakes `.blsp` into the binary — prelude (`lib.rs:152`),
  std modules (`builtins.rs` `BUILTIN_MODULES` + `%builtin-module`). A project's
  modules would just become baked-in modules like the std ones.
- Boot path is `Interp::new()` + `eval_str`; a bundled `main()` is ~10 lines.
- `nest new` already scaffolds `src/main.blsp` with `(defn main ())` + `(provide 'main)`.
- `run-process` can drive `cargo` from Brood, so build *policy* stays in
  `std/project.blsp` (ADR-006), Rust only hosts the launcher template.

Missing pieces:
1. An `argv` / command-line-args primitive (~10 lines; there's `getenv`, no argv).
2. A run contract — the binary loads the project main module and calls `(main args)`;
   let `project.blsp` optionally declare `:main module/fn`. This also yields a
   **`nest run`** (doesn't exist yet) — really step 0.
3. A launcher-crate template (generated `Cargo.toml` + `main.rs` depending on the
   `brood` lib, embedding the project image as a name→source table).
4. A `nest build` driver — mostly Brood (reuse the `nest doc` source-walk): emit
   bundle + launcher, `(run-process "cargo" ["build" "--release"])`, move binary out.

Phasing: **P0** `nest run` (½ day) → **P1** `nest build` source-bundle (a few days;
reuses all the above — needs a Rust toolchain at build time, output ≈ `brood` size,
re-parses project source each launch). Later/optional: **P2** a frozen
post-macroexpand `SharedCode` image (skips parse/expand at startup — real
serialization infra, pairs with the tracing-GC / send-functions-between-processes
work); and a no-toolchain appended-payload stub (the `deno compile` trick).

Caveats: no dependency manager yet (flat `require`/`*load-path*` + baked std), so
only **std-only projects** are bundleable until the deps story lands; and the
generated launcher must reference the `brood` lib crate (path dep locally;
publishing hits the crates.io `brood` name collision noted in project notes).

## Supervision / process-framework track (the "OTP-in-Brood" idea)

Build an Erlang/OTP-style process + supervision layer, but as **Brood policy** on
a minimal kernel (ADR-006). Decisions taken with the user (2026-05-27):

- **M0 — kernel: process monitors (monitors-only, no links yet).** ✅ DONE
  (2026-05-27). `(monitor pid)` returns a `ref`; when `pid` dies the caller gets
  `[:down <mref> <pid> <reason>]` (`:normal` / `[:error msg]` / `:noproc`).
  `(demonitor mref)` stops it. See `docs/devlog.md` + `docs/language.md`.
- **M1 — `hatch`: the Brood process-framework library.** ✅ DONE (2026-05-27).
  `std/hatch.blsp` (embedded, `(require 'hatch)`): `defprocess` (state + `cast`/
  `call` clauses), `hatch` (spawn), `!` (cast), `gen-call` (synchronous, ref-
  tagged). cast body => next state; call body => `[reply next-state]`. Tested in
  `tests/hatch_test.blsp`; `examples/life.blsp` ported to it.
  - TODO (M1.x): a clean **stop**/terminate path (a clause that doesn't recurse);
    today a hatch process loops forever. Needed before supervisors can shut
    children down. Also: a `keep` shorthand for "no state change" (vs returning
    the state var), and init args beyond the single state value.
- **M2 — `hatch` supervisor.** spawn + monitor children, restart per strategy
  (`:one-for-one` / `:rest-for-one` / `:all-for-one`), checkpoint/resume,
  topologies (`:grid-2d`). API follows current Brood idiom (no `&key`).
- **M3 — surface sugar, later.** Each its own ADR.

**Explicitly rejected (keep current surface, ADR-011):** no Clojure-isms — no
callable collections `(board cell)`, no `#(…)` reader fn, no set type `#{}`. Stay
with current primitives.

## Plan: make `examples/life.blsp` simpler

The Game of Life (board = live cells as a map `[x y] -> true`) exposed friction.
Goal here is *simpler code*, not raw speed (HAMT is a separate perf item). The
target is to shrink the two central functions and drop the local `range` helper:

```clojure
;; AFTER tiers 1+2:
(defn neighbour-counts (board)
  (frequencies (mapcat neighbours (keys board))))          ; was an 8-line nested fold

(defn step (board)
  (reduce-kv (fn (next cell n)
               (if (or (= n 3) (and (= n 2) (contains? board cell)))
                 (assoc next cell true) next))
             {} (neighbour-counts board)))                 ; was (keys …) + per-cell (get …)
```

### Tier 1 — prelude only, no kernel change ✅ DONE (2026-05-27)

- [x] **`range`** — `(range hi)` / `(range lo hi)` / `(range lo hi step)`, plus a
  full sequence library (`take`/`drop`/`take-while`/`zip`/`partition`/`sort`/…).
- [x] **`mapcat`** — `(apply append (map f coll))`.
- [x] **`frequencies`** — `(fold (fn (m x) (assoc m x (inc (get m x 0)))) {} coll)`.
- Result: `examples/life.blsp` `neighbour-counts` is now
  `(frequencies (mapcat neighbours (keys board)))`, and the local `range` helper
  is gone. Tests in `tests/sequence_test.blsp`.

### Tier 2 — one kernel change ✅ DONE (2026-05-27)

- [x] **`map-pairs` is now the single map enumerator (replaced `map-keys`).**
  Returns `[[k v] …]` in one O(n) pass; `keys`/`vals`/`contains?`/`reduce-kv` and
  `empty?`/`count`-on-maps are all Brood over it. The map kernel stays five
  primitives (hash-map/map-get/map-assoc/map-dissoc/map-pairs) and the O(n²) `vals`
  is gone. `examples/life.blsp` `step` now uses `reduce-kv`. (Did not add `entries`
  — defer until something needs it.) See `docs/devlog.md` 2026-05-27.

### Out of scope / deferred

- ~~First-class set type `#{}`~~ — **rejected** (decision above: keep the current
  surface, board stays a map `[x y] -> true`).
- [ ] **HAMT persistent map** — O(log n) `get`/`assoc` instead of the O(n) assoc
  vector. This is the *perf* fix, not a simplicity one (surface unchanged), so
  it's separate from this plan; pairs with the tracing-GC migration (ADR-002).

## Done: `sleep` (pure Brood, in `hatch`)

- ✅ `(sleep ms)` in `std/hatch.blsp` — NOT a Rust primitive. A Rust `thread::sleep`
  would block a scheduler worker and starve other green processes; instead `sleep`
  pins a fresh `(ref)` in a `receive` (a clause no message can match) with an
  `(after ms)` timeout, so it parks the process on the scheduler timer and leaves
  the mailbox untouched. The naive `(receive (after ms nil))` was wrong — it eats
  the next queued message. Can move to the prelude once the freeze landmine (below)
  is fixed, since it uses `receive`.

## Bug: docstring dropped on functions with a destructured parameter ✅ FIXED (2026-05-27)

- [x] `(defn f ([x y]) "doc" body)` kept its docstring. Fixed in `lower_fn`
  (`crates/lisp/src/eval/macros.rs`): peel a leading docstring (string + more
  body) before the refutable-bind/`do` wrap and re-insert it as the lowered `fn`'s
  first body form, where `make_closure` looks. Regression test in
  `tests/introspection_test.blsp`. (Multi-clause docstrings remain unsupported —
  separate, pre-existing.)

## Bugs found building the comprehension features (2026-05-27) — ✅ FIXED

- [x] **Multi-clause `defn` couldn't carry a docstring.** Fixed in `lower_fn`
  (`crates/lisp/src/eval/macros.rs`): the multi-clause path now peels an optional
  leading docstring and re-emits it as the lowered fn's first body form;
  `fn_needs_lowering` peels it too so the eval fallback still detects multi-clause.
  `examples/life.blsp` `check-cell` got its docstring back. Test in
  `tests/introspection_test.blsp`.
- [x] **Binding-position names were expanded as macro calls.** Root cause of the
  whole `doseq`/`binding` saga: `macroexpand_all` walked a `fn`/`defmacro` param
  list (and `let` targets) generically, so a name there whose spelling is a macro
  — e.g. `binding` (the dynamic-var form) — got expanded as a call (`first` on
  `&`). Fixed: `fn`/`lambda` and `defmacro` now expand only their body
  (`expand_tail`), and an ordinary `let` expands only binding *values*, not
  *targets* (`expand_let`). So `binding`/`let`/`when`/… are usable as ordinary
  names again. Test in `tests/dynamic_test.blsp`.
- The "`defmacro` template with a multi-param `(fn (~a ~b) …)` mis-lowers" item
  was a **misdiagnosis** — it was the param-name collision above in every case; a
  multi-param fn template expands fine with non-colliding names. Removed.

## Done: `for` / `doseq` / `times` / `iterate-times` / `enumerate` / `into` ✅ (2026-05-27)

- [x] Comprehension `for` (multi-binding + `:when` + destructuring, macro over
  `mapcat`), `doseq`, `times`/`iterate-times`, `enumerate`, `into` — all pure-Brood
  in `std/prelude.blsp`. `examples/life.blsp` rewritten to use them: `step` is a
  `for`+`into`, `render`/`render-row` use `for`, `nth-gen` is `times`, `animate` is
  `doseq`+`enumerate`+`iterate-times`+`clear-frame`, and `check-cell` is multi-clause
  dispatch (no explicit `match`). Tests in `tests/sequence_test.blsp`.

## Concurrency / runtime follow-ups (from the `ref` work, 2026-05-27)

- [ ] **`match`/`receive` can't be used inside a prelude-level function** (debug
  builds): their macro expansion executes lambda-building library fns (`=`,
  `map`, the match compiler) at the prelude's own compile pass, stranding
  closures that `heap.freeze_as_shared_code`'s `debug_assert!(c.env.is_none())`
  rejects. That's why `call`/`reply` live in `examples/life.blsp`, not `std/`.
  Real fix: freeze-time reachability (drop unreachable closures) — falls out of
  the tracing-GC migration. See `docs/devlog.md` 2026-05-27.
- [ ] (revisit) `await`/process monitors (`link`/`monitor`, Erlang phase 6,
  `docs/concurrency.md`). Decided *not* needed for now — synchronous call/reply
  over `ref` covers "wait for a result". Reconsider if fire-and-forget
  supervision becomes a real need.
