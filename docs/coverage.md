# Test coverage

`nest test --cover` reports **function-level** coverage: which of the project's
functions the test suite never called.

```bash
nest test --cover              # run the suite, then report coverage
nest test --cover-min 80       # ...and fail the run if it's under 80%
```

```
Coverage (functions entered at least once):
  src/main.blsp                                 57%  (4/7)

Never called (3):
  src/main.blsp:7: main/never-used
  src/main.blsp:9: main/also-never
  src/main.blsp:15: main/main

coverage: 57% of 7 functions (4 covered)
```

Coverage is reported **even when the suite fails** — a red run is exactly when you
still want to know what went unexercised. `--cover-min` is checked *after* the
suite result, so a failing test reports itself rather than being masked by a
coverage complaint. `--cover-min` implies `--cover`.

## What it measures, and what it doesn't

A function counts as covered the moment it is **entered once**, however little of
its body ran. This is *not* line or branch coverage: a 40-line function with a
dozen untaken branches counts fully covered after one call.

That is a deliberate choice, not a stepping stone we forgot to finish. The
question "which functions does my suite never touch at all?" is the one that
changes what you write next, and it is answerable with **no kernel support at
all** — see below. Line coverage costs a compiler seam and a slower VM in coverage
mode; it is recorded as future work at the end of this document.

Two more limits worth internalising:

- **Hit counts are a lower bound, not a profile.** A self-recursive tail call is
  counted **once**: the VM compiles it to `SelfCall`, which deliberately bypasses
  the global lookup (and therefore the instrumentation). Coverage asks "was this
  entered", which stays correct; don't read the counts as call frequency.
- **A `--cover` run is not a timing run.** Instrumentation adds a frame per call
  and defeats JIT inlining of the wrapped call. Never benchmark under `--cover`.

Only **functions** are counted. Macros are excluded (they are expanded, never
called — wrapping one would break it), as are native primitives and non-callable
data. Only globals whose recorded `source-location` falls inside the project's
`:source-paths` are counted, so std and the prelude never inflate the denominator.

## How it works — hot reload as the instrumentation seam

There is no coverage flag in the VM, no instrumented build, and no new primitive.
`std/tool/coverage.blsp` is pure Brood policy over three things the language
already has:

1. **`global-names` + `source-location`** enumerate the denominator: every global
   that is a function defined in this project's source dirs.
2. **`def` rebinding + late binding (ADR-013)** are the instrumentation. Each
   target is rebound to a shim that records a hit and forwards to the original.
   Because callers resolve globals late, every already-loaded caller — including
   ones running in other processes — picks up the shim with no reload.
3. **`Value::Table` (ADR-107)** collects the hits. Tests run across many green
   processes with separate heaps; a table is shared by identity and `table-incr`
   is atomic under the table lock, so concurrent hits from parallel tests cannot
   lose an update. This is the sanctioned mutable structure being used for exactly
   what it exists for.

Instrumentation happens **after** the project's sources are loaded and **after**
`check-project` (whose `%isolate` would otherwise roll the rebindings back), and
before the suite runs.

### Why the shim is variadic

The shim is `(fn (& args) … (apply original args))` — it does **not** mirror the
original's parameter list. It cannot: `arglist` reports only **one arm** of a
multi-arm function, so a shim built from it would silently break the arities it
never saw. Given

```lisp
(defn describe-arity ((x) :one) ((x y) :two))
```

`arglist` answers `(x y)`, and an arity-preserving shim would make
`(describe-arity 1)` fail. Variadic forwarding is correct for every shape — fixed,
`&optional`, `& rest`, and multi-arm alike — and `tests/coverage_test.blsp` pins
each of them.

The cost of that correctness: an arity error now surfaces from inside the shim
rather than at the call site, and every rebind legitimately changes the function's
arity, which trips the hot-reload arity diagnostic. `nest test --cover` therefore
sets **`BROOD_NO_RELOAD_DIAG=1`**, which silences the `[reload] arity changed …`
and `[reload] macro … redefined` lines for that process. It is an off-switch only:
the default stays on, so an *accidental* reload mismatch is still reported.

## Future work: line coverage

Line coverage is a separate tier, deliberately not built. If it is wanted, the raw
material already exists:

- Compiled IR nodes carry `pos: Option<Pos>` and `CompiledArm` records its source
  file (`crates/lisp/src/eval/compile/ir.rs`), so the VM already knows where it is.
- `Value::Table` already solves cross-process aggregation.

The shape would keep the same mechanism/policy split: **compile-time**
instrumentation (when coverage mode is on, the compiler emits an extra
record-position instruction) rather than a runtime branch per instruction, so a
normal run is byte-for-byte unchanged instead of paying a check everywhere. The
JIT must be disabled in that mode — native code bypasses the hook — which
`--cover` can do via `BROOD_NO_JIT=1`. Reporting would extend
`std/tool/coverage.blsp` rather than replace it.

The two tiers answer different questions and can coexist: function coverage needs
no kernel support and no special build, which is why it ships first.
