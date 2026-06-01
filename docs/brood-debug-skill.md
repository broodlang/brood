---
name: brood-debug
description: Use when a Brood (`.blsp`) program crashed, hung, segfaulted, overflowed the stack, leaked memory, a spawned process "just died", or a GUI/terminal app won't respond or close — the recovery playbook for diagnosing a failed Brood run. Brood has no exception unwind for stack overflow and (by default) no supervisor, so the default failure mode is a silent dead process. Load this to debug it methodically instead of guessing.
---

# Debugging Brood

The default failure mode in Brood is **"the process just died"** — no
supervisor, and a stack overflow in a green process is an *uncatchable* SIGSEGV,
not a `throw` you can `catch`. Work the playbook top-down; the cheap checks catch
most failures.

## 1. Is it non-tail recursion? (the #1 cause)

A green process has a **small coroutine stack**. Deep *non-tail* recursion blows
it — and without the byte-budget guard armed (below) that's a SIGSEGV with no
backtrace. **Before anything else, run the linter:**

```
nest check path/to/file.blsp
```

It flags `recursive call in non-tail position` at `file:line:col`. Fix: make the
self-call the **last** thing the function does (a tail-recursive accumulator), or
drive the loop with a process. (The PostToolUse hook already surfaces this on
every save — but run it explicitly when debugging a file you didn't just edit.)

Arm the guard so the overflow becomes a **catchable `E0044`** with a location
instead of a segfault:

```
BROOD_STACK_BUDGET=8388608 nest run file.blsp     # raise/lower the byte budget
```

## 2. Read the crash artifact

`brood`/`nest` append every Rust **panic** + backtrace to **`.brood_crash_dump`**
in the cwd (and stderr) — durable when a TUI animation scrolls the message away.
`RUST_BACKTRACE` defaults to `1`; use `full` for verbose. Check it first when a
TUI/`nest run` swallowed the error.

**Caveat:** a `SIGSEGV` (coroutine stack overflow) leaves **no panic**, so
nothing lands in the dump. For those:

```
gdb --batch -ex run -ex bt --args ./target/debug/<test-binary>
```

(`rr` isn't installed; `valgrind` won't see a *logical* use-after-GC over safe
`Vec` slabs.)

## 3. Map the error code

If you *do* get a thrown error, its `:code` tells you the class (full table in
`docs/error-codes.md`):

| Code | Means | Usual fix |
|------|-------|-----------|
| `E0044` | stack budget exceeded — runaway non-tail recursion | §1 — restructure to tail recursion |
| `E0043` | crossed the soft memory limit (`BROOD_MEM_LIMIT`) | a `cons`/`string-repeat` loop accumulating; bound it |
| `E0020` | arity mismatch | wrong arg count — `lookup` the real arglist |
| `E0010` | unbound symbol | typo, missing `require`, or a load-order/shadow issue |
| `E0030` | wrong type | check `(type-of x)` at the call site |
| `E0070` | message nested too deep for `send` | flatten/chunk the data crossing processes |

## 4. Isolate the form (MCP eval loop)

With `nest mcp` attached, **bisect interactively** instead of re-running the whole
program:

- `eval` the smallest sub-expression that reproduces the failure — halve it until
  the culprit is isolated.
- `macroexpand` (mode `"all"`) any macro in the failing form — a surprising
  expansion (captured binding, a list where you meant a vector) is a common cause.
- `lookup` a name whose arity/type you're unsure of — don't assume the signature.
- `load` the file and read its `:diagnostics` (the same checker as `nest check`).

## 5. GC / use-after-GC faults (kernel-level)

If the crash is a raw index panic or SIGSEGV *inside the kernel* (not your logic),
suspect a moving-GC rooting bug. Build with debug-assertions and turn rare races
deterministic (see `CLAUDE.md` → "Debug tooling"):

```
RUSTFLAGS="-C debug-assertions=on" cargo build --release
BROOD_GC_STRESS=1   # collect at every safepoint
BROOD_GC_VERIFY=1   # walk the live graph each collection; print the root→cell path
```

The per-deref epoch tripwire panics at the *instant* of a stale deref;
`BROOD_GC_VERIFY` catches a stale handle that was *stored* (surfaces at the store
site's next collection, with the path). This layer is for kernel work, not
everyday `.blsp` debugging — reach for it only when §1–4 point at the runtime.

## 6. A dead spawned process

There's no supervisor by default, so a worker that `throw`s or overflows just
vanishes. To see it:

- **Monitor it.** `(monitor pid)` (or spawn with a link) so the parent
  `receive`s a `:down`/exit message with the reason instead of silence.
- **Supervise it.** `(require 'proc/supervisor)` for restart strategies
  (`:one-for-one`/`:one-for-all`/`:rest-for-one`) when a process *should* recover.
- **`processes`** (MCP) or `(list-processes)` shows who's still alive — a missing
  pid confirms it died.
- Remember messages **deep-copy** across heaps: a value that worked in the parent
  can still fail to build in the child if the child lacks a `require`d module.

## 7. A GUI / TUI app that runs but won't respond or close

A windowed (`--features gui`) or terminal app that *paints* but ignores keys and
the close button is almost never a crash — it's an **input bug**. Don't stare at
the render code; isolate the input path.

**First, split "doesn't run" from "doesn't respond."** Run the cheap layers in
order — they localise the fault before you touch a window:

```
nest check src/app.blsp          # logic / non-tail recursion (§1)
nest test                        # the pure view/step fns
nest run --for 3s                # does it run + exit cleanly? (--for bounds it so a
                                 # hung window can't trap your session)
```

If check/test/`--for` all pass but the live window misbehaves, it's
interactivity, not a crash — go straight to the input path.

**You can't click in a headless/agent session — so drive input directly.** GUI
input is delivered as ordinary **mailbox messages** to the process that called
`gui-open` (ADR-058), in the same encoding the terminal uses:

| Event | Message |
|-------|---------|
| printable key | a **1-char string** — `"a"` |
| special keys | keywords — `:up :down :enter :backspace :escape :ctrl-c` … |
| **window close button (the X)** | **`:close`** — *distinct from* the Escape key `:escape` |
| mouse | `[:mouse action button row col]` |
| resize | `[:resize cols rows]` |

Because the loop reads its **own** mailbox, you can unit-test the whole input
path with no window: `(send (self) :close)` then call the loop's wait/select
function and assert it quits. That turns "did clicking X work?" into a
deterministic test (see `foobar/tests/life_test.blsp` → "wait-frame stays
responsive"). On this GNOME/Wayland box the screenshot D-Bus is access-denied to
the agent, so this *is* the verification path — plus the render frame is plain
data, so `(println (render …))` lets you inspect the emitted ops directly.

**The #1 hand-rolled-loop bug: input starvation on over-budget frames.** A loop
that only polls input *inside* a deadline/timeout branch skips it entirely once a
frame runs over its time budget — and a big board + small font + interpreted
render easily exceeds the frame period. Symptom: paints fine, ignores every key
and the close button. The tell is a guard like `(if (>= (now-ns) due) … (receive …))`
that bypasses the `receive`, plus a fallback `receive` that matches only the
worker reply (e.g. `[:gen g]`) and not input. **Fix: scan the mailbox every frame
regardless of the clock**; gate only the *pacing* on the deadline, never the input
read.

**Know the close contract.** The X delivers `:close`, not `:escape`. `ui-run`
(`std/editor/ui.blsp`) quits on `:close` automatically, so prefer it — it also owns
pacing and guaranteed teardown (`:leave`/`gui-close` runs even if `view`/`update`
throws). A hand-rolled `(receive)` loop must match `:close` itself (`(:close :quit)`)
or use `editor/ui/quit-request?`. If the app binds Esc to cancel/normal-mode, **only**
`:close` can close it — that's the whole reason the two are separate.

**Build/feature gotchas.** The GUI backend is behind the `gui` cargo feature —
`cargo build -p nest --features brood/gui`. Without it the `gui-*` primitives
return a clear `gui backend not compiled in; rebuild with --features gui` error
rather than opening a window; an app that "does nothing" may simply be running a
non-GUI build.
