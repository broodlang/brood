# Supervised processes + resume checkpoints

> Status: **designed, not yet implemented.** ADR-039. Core architectural
> change to the process model — every spawned process (including the main
> one running a script) is implicitly supervised by the runtime; uncaught
> errors are caught at the process boundary, the process is restarted
> automatically. The depth of state preservation is mode-gated: full
> in development, none in release.
>
> This doc is the design walkthrough — what changes, why it's safe in Brood
> specifically, what simplifies downstream, and where the performance
> levers are.

## The model in one sentence

> **A process is its current call.** The runtime captures `(callee, argv)`
> at every function call as a *resume slot*; an uncaught error triggers
> the runtime supervisor to re-invoke the slot. Same function. Same args.
> State preserved. Code (after a reload) potentially new.

## Why this works in Brood and not in mutable languages

Erlang's gen_server/supervisor split exists because **a worker that
crashes mid-mutation can't be safely resumed** — the heap is in a
half-modified state, references to it from elsewhere are dangling, the
supervisor can only get to a known-good point by restarting from scratch
(`init/1`). Hence the split: state lives in a `gen_server` that's
extra-careful and rarely crashes; workers are restartable cheap things.

Brood is **immutable**. The only mutation is global-table rebinding (`def`),
and that's atomic — there is no "half-applied `def`" state. Anything else
the eval loop does is *building* values, never mutating them in place. At
any safepoint, the values held by the eval loop are byte-for-byte
equivalent to those values before any iteration started; restarting from
a captured `(callee, argv)` is *transactional* — it can't observe any
partial state because there *is* no partial state.

The thing that makes immutability "boring" for in-place performance is
exactly what makes this design clean: a crash carries no recoverable
debt.

Three properties combine:

1. **Immutability** — no half-mutated state on crash.
2. **Late binding** — function references resolve by name at call time,
   so a reload updates the running code automatically.
3. **The eval loop's `'tail:` continue** — the loop already maintains an
   `(expr, env)` checkpoint at every iteration; capturing `(callee, argv)`
   at function-call boundaries is essentially the same shape.

No mutable language has all three. Brood has them by design.

## Concrete behaviour, from the user's seat

### A long-running worker

```lisp
(defn my-loop (num)
  (println (str "num: " num " v1"))
  (sleep 1000)
  (my-loop (+ 1 num)))

(spawn :worker (my-loop 99))

(receive)
```

What happens:

1. Process P starts at `(my-loop 99)`. Resume slot: `(my-loop, [99])`.
2. P enters the body. Prints `99 v1`. Sleeps 1s. Tail-calls `(my-loop 100)`.
   **Resume slot updates** to `(my-loop, [100])`.
3. P continues: prints `100 v1`, sleeps, tail-calls `(my-loop 101)`. Slot:
   `(my-loop, [101])`.
4. User saves the file with `v1` → `v2`. The reloader's `(load …)` runs,
   `(defn my-loop …)` rebinds `my-loop` globally. The line
   `(spawn :worker (my-loop 99))` is a *no-op* (named-spawn sees `:worker`
   alive). The line `(receive)` is in the launcher, not the worker.
5. P, on its next iteration, looks up `my-loop` globally — gets the new
   closure — runs with the new body. Prints `247 v2`. Sleeps. Tail-calls
   `(my-loop 248)`. Slot: `(my-loop, [248])`.
6. User saves with a buggy version. Reloader rebinds. P's next iteration
   runs the new (buggy) code, throws. **The runtime supervisor catches**,
   logs `[worker:#<pid>] caught: <error>`, sleeps 1ms backoff, **re-invokes
   `(my-loop 247)`** — the resume slot's args.
7. The retry runs the (still buggy) code, throws again. Supervisor catches,
   logs, sleeps 2ms, retries. Exponential backoff. After ~10 retries (or
   ~1s, whichever first), the supervisor gives up — logs
   `[worker:#<pid>] gave up after 10 crashes in 0.5s` — and the process
   exits.
8. User fixes the bug, saves. Reloader rebinds. **But P is gone.** The
   user's `(spawn :worker (my-loop 99))` on the next reload sees `:worker`
   is now *not* alive (P gave up), spawns afresh at `(my-loop 99)`. State
   reset to 99. Cost of finally-giving-up. The user is told via the log.

Step 6 is the win: a transient bad-save doesn't kill the worker; the next
correct save brings it back to life **at the same `num`**. Step 7
prevents an always-broken redefinition from spinning forever. Step 8 is
the recovery path when the user gave up trying to fix on the fly.

### The editor

The editor is just the worker pattern with richer state:

```lisp
(defn editor-loop (state)
  (let (ev        (read-event)
        new-state (handle-event state ev))
    (editor-loop new-state)))

(spawn :editor (editor-loop (initial-state)))

(receive)
```

User redefines `forward-word` buggily. User presses M-f. `handle-event`
looks up `forward-word`, calls it, throws. Runtime catches at the
process boundary. **Re-invokes `(editor-loop state)`** — the exact state
just before the bad key. Editor is alive, all buffers intact, cursor
where it was. Log line surfaced in the minibuffer.

This is structurally what Emacs's `command_loop_1` does, **for free** —
the user wrote no `condition-case` and no command-dispatcher try/catch.
Better than Emacs's version, because Brood's immutability means the
state at the resume point is *byte-for-byte identical* to the
pre-command state; Emacs has to be careful about half-mutated buffers.

### A script

```lisp
;; one-off-script.blsp
(def x 1)
(def y 2)
(println "computing")
(println (* x y))
```

Run as `brood one-off-script.blsp`. The "main" process is supervised
just like any other.

- If line 3's `println` throws somehow (it won't, but suppose), the
  runtime catches, logs, retries `println "computing"`. Exit-on-success.
- If line 4's `(println (* x y))` throws, retries.
- After max-restart-count, the process exits with non-zero (script
  failed).

The resume slot for a script tracks "the current top-level form" rather
than a deep call frame. Side-effect duplication on retry: yes (the
`println "computing"` line might fire twice if `(println (* x y))` is
the one that's transiently broken). Most scripts are idempotent;
non-idempotent ones use `--release` (no supervision, no retry).

### REPL

REPL is the editor pattern with `(read-eval-print-loop state)` as the
recursive call. A throw inside `eval-print` (e.g., the user typed
malformed code) is caught by the runtime, logged, REPL loop re-invokes
with the same state. The user sees the error and types again. Already
how the REPL works in spirit; this makes it the model, not a special
case.

## What disappears

Designs we no longer need:

### `defonce`

Currently in `std/prelude.blsp` as a **transitional shim** until ADR-039
lands. Its two uses, and what replaces each when the supervised model
ships:

- **`(defonce *worker-pid* (spawn (my-loop 99)))`** → replaced by named
  spawn: `(spawn :worker (my-loop 99))`. The spawn primitive's
  idempotence-on-name *is* defonce-for-processes. **This is the
  dominant use of `defonce` today.**
- **`(defonce *cache* {})`** for long-lived top-level state → replaced by
  *state lives in a process*. A cache is a process that holds the map in
  its loop accumulator; clients message it.

Top-level non-process state isn't an antipattern, but Brood's grain is
to put state in processes. Reloads don't touch running processes; state
survives without ceremony.

**`defonce` is not removed today** — removing it before named-spawn
lands leaves users without a way to write a "spawn-once on reload"
pattern. It's kept in the prelude with a docstring flagging it as
transitional; the removal is part of the ADR-039 implementation, in the
same commit that adds named-spawn so users have a working migration
path.

### `live-loop`

The error-trapping macro I was about to propose. Subsumed by the
runtime's process supervisor: plain `(defn worker (state) … (worker
new-state))` *is* a fault-tolerant loop.

### Hand-written restart logic

Today's "monitor + respawn" pattern (the recommended supervision shape
in slice-3 distribution work) becomes a runtime feature for the
single-process case. `monitor` stays — for *cross-process
notifications* ("I want to know when X is done"), not as the restart
mechanism. Distributed monitors are unchanged; they're about
cross-node failure signalling, a different problem.

### Most user-level `try`/`catch`

The Erlang teaching is "let it crash + supervise". Brood's version
becomes "let it crash + the runtime supervises". `try`/`catch` in
user code is for **recovery with context** — not "don't die". Examples
where it's still right:

- A request handler that wants to log *which request* failed (not just
  that something did).
- A parser that wants to fall through to a default on parse error.
- A test framework that wants to record `assert-error` failures
  structurally.

The "I have to wrap my whole loop in `try` so it doesn't die" pattern is
gone.

### Per-test crash isolation in `nest test`

Today, `:isolated` tests run in a private fork of the globals so a `def`
in one test doesn't leak. They also (incidentally) isolate crashes —
because a crashing test in an isolated process doesn't kill the harness.
Under supervision-by-default, that incidental property is universal:
*every* test gets crash isolation. `:isolated` keeps its global-sandbox
meaning; it's no longer also the way to "survive a bad test".

## Mode-gating: pay only when you need it

The cost of supervision is two stores per function call (the resume slot
update). That's tiny in absolute terms — sub-nanosecond — but in a tight
recursive numeric loop it's a few percent. Hot-reload survivability is
worth that during development; it isn't needed when the editor is
deployed.

So: a runtime mode, selected per command.

### `dev` mode (default for `brood`, REPL, `nest run`, `nest test`)

- `spawn` is supervised. Uncaught errors caught at process boundary.
- Resume slot updates **at every function call**.
- On caught error, runtime re-invokes the resume slot with exponential
  backoff.
- Hot reload survives bad saves; state survives crashes.
- Slight overhead per call.

### `release` mode (default for `nest bundle` output, `--release` flag)

- `spawn` is still supervised (errors caught — the process can't kill
  its OS thread), but **no resume slot updates**.
- On caught error, the supervisor logs and **re-invokes the spawn entry**
  — the original `(my-loop 99)`. State is lost (back to 99).
- No per-call overhead. Eval loop is exactly as fast as today.
- Suitable for shipped editor binaries where source isn't being edited.
- The user can still opt into full supervision via `BROOD_MODE=dev` if
  they want hot-reload on a deployed instance.

### `bare` mode (for benchmarks / very specific use cases)

- No supervision at all. Errors propagate to the OS thread; the process
  exits.
- Same eval loop hot path as today (no checkpoint, no catch).
- Useful for the language benchmark suite ("how fast can this run with
  *nothing* in the way"); not the default anywhere.

### How the mode is selected

| Surface                       | Default mode | Override                            |
|-------------------------------|--------------|-------------------------------------|
| `brood file.blsp`             | dev          | `--release` / `BROOD_MODE=release` |
| `brood --test`                | dev          | same                                |
| `brood` REPL                  | dev          | same                                |
| `nest run`                    | dev          | `nest run --release`                |
| `nest test`                   | dev          | `nest test --release`               |
| `nest bundle` output          | release      | bundle-time flag                    |
| `brood --bench`               | bare         | n/a (benchmarks pin the mode)       |

The mode is a single atomic at runtime — read once at startup, branched
on at the supervisor and the resume-slot-update site. No per-call check
(the branch is monomorphised, or compiled into two eval loops).

## Performance: what gets faster, what costs more

### What costs more (dev mode)

**Per-call resume-slot update** — at every `Value::Fn(id)` and
`Value::Native(id)` dispatch in the eval loop, two writes:

```rust
let process = current_process();
process.resume_slot.callee = callee;
process.resume_slot.argv.clear();
process.resume_slot.argv.extend_from_slice(&argv);
```

The `argv` here is the already-built `SmallVec<[Value; 8]>` from the
combination handler. Reusing its storage avoids a fresh allocation; the
`clear()` + `extend_from_slice` is two memcpy's of at most 64 bytes on
the hot path (8 × `Value` size). Cost: a few ns per call.

Estimated impact on a tight recursive numeric loop: ~3–5% slower in
dev mode. On a typical workload (function calls dominated by env_get +
body work, not by the slot update), measurably smaller — likely <1%.

Will be measured properly when implemented; the numbers above are
order-of-magnitude estimates from looking at the existing `tick`
counter's similar cost profile.

### What gets faster

**Less defensive coding in user space.** Every `try`/`catch` removed
from a hot loop is a few cycles back. Today, conservative code in tests
and worker loops wraps iterations in `try`/`catch` "just to be safe".
Under supervision, those become unnecessary. Net: code gets faster *and*
shorter.

**Test suite's `:isolated` mechanism becomes lighter.** Today, an
isolated test forks the globals (snapshots + restores). Under universal
supervision, the "crash containment" half of that isn't needed — only
the "global sandbox" half. The implementation can drop one of the two
concerns. Modest test-runtime improvement.

**Reload doesn't need its own try/catch.** The reload watcher's
explicit `(try (load p) (catch e …))` can degrade to just `(load p)`
with the supervisor catching. Slightly smaller call graph; arguably
cleaner logs (the supervisor's diagnostic format is unified).

**Aggressive inlining becomes safer** (future). When the JIT/compiler
work happens (Stage 2+), supervised tail loops have a clean
"redefinition invalidates inlining" story: the supervisor catches any
post-redefinition crash, so an inlining decision can be optimistic.
Today's tree-walking interpreter doesn't benefit, but it's a real
future enabler.

### Potential follow-up optimisations the design suggests

1. **Coarse-grained resume slot.** Instead of updating at every function
   call, update only at *tail-call boundaries* (where `'tail: continue`
   happens). Deep call chains compute without checkpoint overhead;
   recovery still works at "the loop's last clean iteration" granularity.
   ~4× reduction in update frequency for typical programs. State
   resolution becomes coarser (recovery goes to the iteration start,
   not the innermost frame), which is what most users actually want
   anyway.

   Probably the right shape. Document as "checkpointing at iteration
   boundaries" rather than "at every call".

2. **`Process::resume_slot` co-located with the reduction-count `tick`
   counter.** Both are per-process, both touched on the hot path. Pack
   them into one cache line; the existing `tick` cache miss "covers" the
   resume slot read/write. Net cost approaches the cost of `tick` alone
   (which is already paid).

3. **Argv pooling.** `SmallVec::clear()` + `extend_from_slice` is a
   memcpy. If consecutive calls have the same arity, the inline storage
   is reused — no allocation. The hot loop case (recursive tail call
   with stable arity) is exactly this. Effective cost is two pointer-
   sized writes + a memcpy of at most 8 Values. Probably under 1 ns on
   modern hardware.

4. **The mode branch monomorphises.** Compile two versions of the eval
   loop — `eval_dev` and `eval_release` — and select by mode at startup.
   The dev version has the slot updates inline; the release version
   omits them entirely. Branch-free per-call hot path in both modes.
   Slight binary-size cost (eval loop is duplicated, ~3 KB). Worth it.

5. **The supervisor's catch is rarely-hit** — Rust's `Result` propagation
   does the work. The supervisor wrap is essentially:

   ```rust
   loop {
       match eval(&mut heap, expr, env) {
           Ok(v) => return v,
           Err(e) if mode == Release => exit(e),
           Err(e) => {
               log_error(&e);
               sleep(backoff());
               // Re-invoke from resume slot.
               let (callee, argv) = process.resume_slot.take()
                   .unwrap_or((entry_callee, entry_argv));
               // Tail-call back into eval with apply(callee, argv).
               ...
           }
       }
   }
   ```

   No allocation, no virtual dispatch. Trivial in steady-state (Ok path);
   non-trivial only on actual error.

### Potential simplifications the new model enables (besides what
already disappears)

1. **`std/reload.blsp` simplifies.** The watcher's inner try/catch is now
   a *diagnostic* layer ("which file failed?"), not a survival layer
   ("don't kill the watcher"). The latter is the runtime's job. Smaller
   policy code.

2. **`hatch` (the process-framework module) might unify with this.**
   `hatch` is a Brood-side process framework that provides supervisor
   primitives. Under the new model, it becomes an *opinionated layer*
   over the runtime supervisor — exposing things like *named pools of
   workers* or *one-for-one strategy*. Smaller and more focused.

3. **The `nest test` runner's per-test process becomes lighter.** It
   still spawns a process per test for parallelism, but doesn't need
   the careful "catch any throw from the test body" boilerplate — the
   runtime catches.

4. **The distributed-monitor design (slice 3) simplifies the local-only
   semantics.** Local `monitor` doesn't need to be the "restart on
   death" mechanism anymore; it's purely a *notification* primitive.
   The "respawn on `:down`" pattern in `ensure-link` (in
   `std/prelude.blsp`) is a single example we keep, but it's now an
   *opt-in* — the supervisor would have done it.

5. **`%try` / `try` / `catch` could potentially be simplified.** The
   most common pattern — "evaluate this and don't let it kill me" —
   doesn't need explicit catching anymore. The other patterns
   (recovery with context, value-returning catch, etc.) still need
   the primitive. So this is "less use", not "remove".

## Open design questions

1. **Resume granularity: every call vs tail-call only.** Tail-call only
   is the right answer for performance; recovery to "iteration start"
   is what the user actually wants in practice. Need a test that shows
   a deep call chain crashing → resume goes to outermost loop iteration
   start, not the innermost frame. Documented as "checkpoints are at
   iteration boundaries, not at every frame".

2. **Side-effect duplication.** A `(println …)` followed by a crash
   means the println happened; the resume re-runs it. For most code
   this is harmless; for non-idempotent side effects (network sends,
   payment APIs), it's a footgun. Mode-gating gives an opt-out
   (`--release`). Document; mark as a known characteristic.

3. **What happens to messages sent before the crash?** They were
   delivered. On resume, the same iteration's `(send target msg)` fires
   again; receiver gets a duplicate. Same shape as the println case;
   at-least-once semantics for inter-process messages. Document.

4. **Restart storm protection.** Exponential backoff with `max-restarts
   / max-seconds` is the proven design (Erlang's). Defaults: 10
   restarts in 5 seconds, exponential 1ms → 1s backoff. Tuneable on
   spawn site: `(spawn :worker expr :max-restarts 100 :backoff-base
   :100ms)`.

5. **Where does the supervisor's log go?** Today, `eprintln`. Future:
   a *log channel* per process that other processes can subscribe to
   (an editor's "messages" buffer). Out of scope for this ADR;
   landing the supervisor first, then adding the channel.

6. **The script-mode resume.** What's the resume slot for a top-level
   sequence-of-forms file? Either "the current top-level form" (retry
   that form on crash) or "nothing — exit on crash". The former is
   useful for idempotent scripts (most); the latter is safer for
   non-idempotent. Decision: top-level forms in script mode *don't*
   update the resume slot; an error during a script's top-level
   evaluation exits the script. The supervisor still catches (so the
   exit is logged cleanly); state preservation doesn't apply. Workers
   *spawned* by the script get the full dev-mode supervision.

## Implementation sketch (when it lands)

### Rust changes

**`crates/lisp/src/process.rs`**:

- `Process` gains a `resume_slot: Option<ResumeSlot>` field.
  ```rust
  pub(crate) struct ResumeSlot {
      callee: Value,
      argv: SmallVec<[Value; 8]>,
  }
  ```
- The coroutine entry wraps its `eval` call in a supervisor loop:
  ```rust
  loop {
      let result = catch_unwind_eval(&mut heap, entry_expr, env);
      match result {
          Ok(v) => return v,
          Err(e) if process.mode == Release => {
              log_error(&e);
              return; // exit
          }
          Err(e) => {
              log_error(&e);
              if !backoff(&mut process) { return; } // max restarts hit
              if let Some(slot) = process.resume_slot.take() {
                  entry_expr = build_apply(slot.callee, slot.argv);
                  // env unchanged — the slot's callee captured its own
              } else {
                  // re-run from spawn entry (lost state)
              }
          }
      }
  }
  ```
- `add_monitor` / `MONITORS` table unchanged.
- A new `NAMED_PROCESSES: Mutex<HashMap<Symbol, ProcessId>>` for the
  named-spawn idempotence (or reuse the existing `NAMES` table from
  `dist.rs` — same shape).

**`crates/lisp/src/eval/mod.rs`**:

- At every `Value::Fn(id)` and `Value::Native(id)` dispatch in the eval
  loop, **if in dev mode**, update the current process's resume slot:
  ```rust
  if MODE == Dev {
      current_process().resume_slot = Some(ResumeSlot {
          callee, argv: argv.clone() // small, often inline
      });
  }
  ```
- Mode is a single atomic loaded at process spawn (no per-call branch on
  hot path — monomorphise the loop body if the cost matters).

**`crates/cli/src/main.rs` + `crates/nest/src/main.rs`**:

- `--release` flag; `BROOD_MODE=release` env var; default per command per
  the table above. Set `MODE` static atomic at startup.

**`crates/lisp/src/builtins.rs`**:

- `spawn` builtin updated to accept an optional first-arg name:
  `(spawn name expr)` (idempotent on name) vs `(spawn expr)` (anonymous,
  no idempotence).

### Brood changes

**`std/prelude.blsp`**:

- Remove `defonce`. (Done.)
- The `spawn` macro adapts: `(spawn [name] expr)`.

**`std/reload.blsp`**:

- Remove the explicit `(try (load p) (catch e …))` survival pattern —
  keep an *optional* try for diagnostic context, but the supervisor
  is the survival layer.

**`std/hatch.blsp`** (the process framework):

- Audit and likely simplify. Less hand-rolled supervision, more
  layering on the runtime's.

**`examples/hot-reload/`**:

- Simplify. Drop `defonce` (gone), drop the explicit park-then-spawn
  dance, use `(spawn :ticker …)`.

## Migration & roll-out

The change is invasive on the runtime but additive at the user surface
in the common case (existing `(spawn expr)` calls keep working, just
become supervised). Real risks:

- **`try`/`catch` semantics shifts** — some user code relies on a thrown
  error killing a process so a parent monitor sees `:down`. Under
  supervised default, the process *doesn't* die; the parent never sees
  `:down`. Workarounds: `(spawn expr :supervised false)` for the
  let-it-crash semantics. Document the change loudly.
- **Side-effect duplication** — code that does `(send other-pid msg)` in
  a loop may double-send on resume. Test suite should catch the
  egregious cases; document.
- **Test suite reactions to supervision** — some tests rely on `throw`
  → process dies → harness sees death. Need to either set `:supervised
  false` for those tests or rework them. Audit during implementation.

Roll-out: behind `BROOD_MODE=dev`/`release`. Land in dev mode by default
but with the supervisor *opt-in via the spawn site initially*:
`(spawn :worker expr :supervised true)`. Once we've migrated the test
suite and a few example programs, flip default to `supervised: true`
and add `--supervised false` as the bare opt-out. Two-phase commit
reduces the blast radius.

## See also

- ADR-013 — Hot reload via `def`-rebinding (the *reason* this matters)
- ADR-018 — Green processes / coroutine scheduler (the substrate)
- ADR-026 — Immutability (the *reason* this is sound)
- ADR-033 — Closures as data (cross-process / cross-node shipping)
- ADR-038 — Single-binary bundling (release-mode default consumer)
- ADR-039 — This design's accept-the-decision record
