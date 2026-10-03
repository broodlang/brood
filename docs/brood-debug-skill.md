---
name: brood-debug
description: Use when a Brood (`.blsp`) program crashed, hung, segfaulted, died, raised "recursion too deep", leaked memory, a spawned process "just died", or a GUI/terminal app won't respond or close — the recovery playbook for diagnosing a failed Brood run. Covers telling your program's error from a runtime fault, the crash reporter and crash dump, error codes, dead processes, and GC/scheduler faults. Load this to debug it methodically instead of guessing.
---

# Debugging Brood

First decide **whose** failure it is. Most are your program's: an uncaught raise
(reported by the default crash reporter), runaway non-tail recursion (a catchable
`E0044 recursion too deep`), a process that exited and nobody was watching. A
*runtime* fault — a Rust panic, or a `SIGSEGV` — is rarer and is a kernel bug
(usually use-after-GC). Work the playbook top-down; the cheap checks catch most
failures. `brood --debug-flags` is the index of every `BROOD_*` knob below.

## 1. Read what the run already printed

- **The default crash reporter** (ADR-305) is on under `brood file`, `nest run`, a
  released bundle and the REPL: every process that exits abnormally prints one
  `[crash] #<pid …> exited: <reason>` report (kind, location, trace) per crash
  *site*. It is **off under `nest test`**, and `BROOD_NO_CRASH_REPORT=1` turns it
  off (you get the bare `process N died: …` line instead). So "a worker that throws
  just vanishes" is only true there.
- **A send to a name nobody holds** warns once per name (ADR-232):
  `dist: dropped local message for unregistered name :foo …` — the message is
  dropped (`send` is fire-and-forget), but it is not silent. `BROOD_NO_DROP_WARN=1` silences it.
- **`[sched] STRANDED WORK (KI-88 signature)`** is the default-on watchdog for queued
  work no scheduler worker picks up. Keep the binary and the log; it is a runtime bug.
- **`.brood_crash_dump`** in the cwd: `brood`/`nest` append every Rust **panic** +
  backtrace there (and to stderr) — durable when a TUI scrolls the message away.
  Append-only: read the **last** block. `RUST_BACKTRACE` defaults to `1`; `full` for
  verbose.

## 2. Is it non-tail recursion? (the #1 cause of "recursion too deep")

Deep *non-tail* recursion is a clean, catchable error, not a segfault: the VM raises
`E0044 recursion too deep: exceeded the VM's 1048576-frame non-tail-call limit`
(the tree-walker raises the same code from its byte budget, tuned by
`BROOD_STACK_BUDGET`). It is still a bug — slow to reach and wasteful. Run the linter:

```
nest check path/to/file.blsp
```

It flags `recursive call in non-tail position` at `file:line:col`. Fix: make the
self-call the **last** thing the function does (a tail-recursive accumulator), or
drive the loop with a process. (On this machine the `blsp-check.sh` PostToolUse hook
runs it on every Write/Edit — run it explicitly for a file you didn't just edit.) A
deliberately non-tail helper is acknowledged with
`(check-allow :non-tail-recursion (defn …))`.

## 3. Map the error code

A raised kernel error's `:code` tells you the class (full table in
`docs/error-codes.md`):

| Code | Means | Usual fix |
|------|-------|-----------|
| `E0044` | recursion too deep — runaway non-tail recursion | §2 — restructure to tail recursion |
| `E0043` | crossed the soft memory limit (`BROOD_MEM_LIMIT`) | a loop accumulating without bound; bound it |
| `E0045` / `E0046` | this process's `(proc/flag :max-heap n)` / `:max-mailbox` bound | the process is retaining too much / a receiver cannot keep up |
| `E0020` | arity mismatch | wrong arg count — `lookup` the real arglist |
| `E0010` | unbound symbol | typo, a module name that does not exist or is spelled wrong (`mod/name`), a name used bare without `(:use mod)`, or a shadow |
| `E0030` | wrong type | check `(type-of x)` at the call site |
| `E0070` | `send` saw a message nested past a million levels | a self-referential / runaway structure crossing processes |

## 4. Isolate the form (MCP eval loop)

With `nest mcp` attached, **bisect interactively** instead of re-running the whole
program:

- `eval` the smallest sub-expression that reproduces the failure — halve it until
  the culprit is isolated.
- `macroexpand` (mode `"all"`) any macro in the failing form — a surprising
  expansion (captured binding, a list where you meant a vector) is a common cause.
- `lookup` a name whose arity/type you're unsure of — don't assume the signature.
- `load` the file and read its `:diagnostics` (the same checker as `nest check`).

## 5. A dead spawned process

- **Monitor it from the start.** `(let ([pid ref] (spawn-monitor (work))) …)` delivers
  `[:down ref pid reason]` with the **true** reason. A separate `spawn` then `monitor`
  can lose the race: a child that exits in the gap reports `:noproc` instead.
  `(spawn-link expr)` is the symmetric version (an abnormal exit takes you down, or
  arrives as `[:EXIT pid reason]` after `(proc/trap-exit true)`).
- **Supervise it.** `supervisor/…` for restart strategies
  (`:one-for-one`/`:one-for-all`/`:rest-for-one`) when a process *should* recover.
- **Who is alive?** `(proc/list)` lists live pids, `(proc/alive? pid)` asks about one,
  `(proc/info pid)` gives `:status`, `:mailbox`, `:memory`, `:parent`, … — or the
  `processes` MCP tool. A missing pid confirms it died.
- **Under `nest test`**, `nest test --trace` prints each test as it finishes (outcome,
  time, location) — a test that never prints is the hung one.
- Messages **deep-copy** across heaps; a closure's free *globals* late-bind in the
  receiver, so a name only the sender defined raises `unbound symbol` there.

## 6. GC / scheduler faults (kernel-level)

If the crash is a Rust panic, a raw index panic or a `SIGSEGV` *inside the kernel*
(not your logic), suspect a moving-GC rooting bug or a scheduler race. Build with
debug-assertions and turn rare races deterministic (see `CLAUDE.md` → "Debug tooling"):

```
RUSTFLAGS="-C debug-assertions=on" cargo build --release
BROOD_GC_STRESS=1   # collect at every safepoint
BROOD_GC_VERIFY=1   # walk the live graph each collection; print the root→cell path
```

The per-deref epoch tripwire panics at the *instant* of a stale deref;
`BROOD_GC_VERIFY` catches a stale handle that was *stored* (surfaces at the store
site's next collection, with the path). A `SIGSEGV` leaves no panic and no dump —
run it under `gdb --batch -ex run -ex bt --args <binary> <args>` (`rr` isn't
installed; `valgrind` won't see a *logical* use-after-GC over safe `Vec` slabs).

More levers, each of which names a subsystem when it flips the verdict:
`BROOD_TIER=1` (no JIT) / `BROOD_TIER=0` (tree-walker); `BROOD_J=1` (one scheduler
worker); `BROOD_SCOPE_DBG=1` (processes still live when an `%isolate` rolls the globals
back — the KI-89 tool); `BROOD_TRACE_PROMOTE=1` (closures entering the append-only
shared region — a per-operation promotion is a slow leak); `BROOD_SCHED_DBG=1`
(per-pid scheduling trace). This layer is for kernel work, not everyday `.blsp`
debugging — reach for it only when §1–5 point at the runtime.

## 7. A GUI / TUI app that runs but won't respond or close

A windowed (`gui` feature) or terminal app that *paints* but ignores keys and the
close button is almost never a crash — it's an **input bug**. Don't stare at the
render code; isolate the input path.

**First, split "doesn't run" from "doesn't respond."** Run the cheap layers in
order — they localise the fault before you touch a window:

```
nest check src/app.blsp          # logic / non-tail recursion (§2)
nest test                        # the pure view/update fns
nest run --for 3s                # does it run + exit cleanly? (--for bounds it so a
                                 # hung window can't trap your session)
```

If check/test/`--for` all pass but the live window misbehaves, it's
interactivity, not a crash — go straight to the input path. `BROOD_UI_TRACE=1`
prints each `ui-run` turn's view/draw/update timing; `BROOD_GUI_TRACE=1` the
window's paint.

**You can't click in a headless/agent session — so drive input directly.** GUI
input is delivered as ordinary **mailbox messages** to the process that called
`gui/open` (ADR-059, on the ADR-046 display seam), in the same encoding the
terminal uses:

| Event | Message |
|-------|---------|
| printable key | a **1-char string** — `"a"` |
| special keys | keywords — `:up :down :enter :backspace :escape :ctrl-c` … |
| **window close button (the X)** | **`:close`** — *distinct from* the Escape key `:escape` |
| mouse | `[:mouse action button row col mods]` — `mods` a vector like `[:ctrl]`; a press may append a click count, a scroll its line delta |
| resize | `[:resize cols rows]` |

Because the loop reads its **own** mailbox, you can unit-test the whole input
path with no window: `(send (self) :close)` then call the loop's wait/select
function and assert it quits. That turns "did clicking X work?" into a
deterministic test. The render frame is plain data too, so `(io/inspect (view model
80 24))` shows the emitted ops directly.

**The #1 hand-rolled-loop bug: input starvation on over-budget frames.** A loop
that only polls input *inside* a deadline/timeout branch skips it entirely once a
frame runs over its time budget. Symptom: paints fine, ignores every key and the
close button. The tell is a guard like `(if (>= (os/now-ns) due) … (receive …))`
that bypasses the `receive`, plus a fallback `receive` that matches only the
worker reply and not input. **Fix: scan the mailbox every frame regardless of the
clock**; gate only the *pacing* on the deadline, never the input read.

**Know the close contract.** The X delivers `:close`, not `:escape`. `ui-run`
(`std/editor/ui.blsp`) quits on `:close` automatically, so prefer it — it also owns
pacing and guaranteed teardown (`:leave`/`gui/close` runs even if `view`/`update`
throws). A hand-rolled `(receive)` loop must match `:close` itself (`(:close :quit)`)
or use `editor/ui/quit-request?`. If the app binds Esc to cancel/normal-mode, **only**
`:close` can close it — that's the whole reason the two are separate.

**Build/feature gotchas.** The GUI backend is behind the `gui` cargo feature —
`./configure --with-gui && make install`, or `cargo build -p nest --features
brood/gui`. Without it the `gui/*` primitives raise `gui backend not compiled in —
this brood was built without it` rather than opening a window; an app that "does
nothing" may simply be running a non-GUI build.
