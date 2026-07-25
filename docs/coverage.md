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

That is a deliberate choice, not an unfinished stepping stone. The question "which
functions does my suite never touch at all?" is the one that changes what you write
next, and it is answerable with **no kernel support at all** — see below. Line
coverage is the stricter second tier, `--cover-lines`, documented at the end of this
document: it costs a compiler seam and a JIT-free run, so it stays opt-in.

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

## The second tier: line coverage (`--cover-lines`)

Function coverage answers "was this ever entered". Line coverage answers "did this
line run", and it shipped as a separate, opt-in tier — a stricter number, at the cost
of a compiler seam and a diagnostic-only run.

```
nest test --cover-lines               # which executable lines ran
nest test --cover --cover-lines       # both tiers; they measure different things
nest test --cover-lines --cover-min 80
```

Output is a per-file percentage, then the lines that never ran:

```
Line coverage (executable lines that ran):
  src/cov.blsp                                  33%  (1/3)

Never ran in src/cov.blsp (2 lines):
  6, 7

coverage: 33% of 3 executable lines (1 ran)
```

### How it records

Recording is a **bytecode instruction**, `Inst::RecordLine`, emitted at COMPILE time
and only when `BROOD_COVERAGE` is set. An ordinary run's bytecode is byte-for-byte
what it always was — the interpreter never sees the opcode, and there is no
per-instruction runtime check to pay for. The instruction carries the line only; the
file comes from the executing arm's `CompiledArm::src_file`, which `exec_chunk`
already holds, so nothing new is threaded through the hot executor. Hits land in one
process-wide set (`crates/lisp/src/coverage.rs`) because green processes are
multiplexed across OS threads: a line executed by any process counts.

The flag has to be set **before anything builds an `Interp`** — the prelude is
compiled during construction, and a chunk compiled without the flag has no
`RecordLine` in it. `nest` sets it in `main`, before subcommand dispatch. Getting this
wrong fails silently, with no instrumentation and no error.

`--cover-lines` also sets `BROOD_NO_JIT=1`. An instrumented arm bails JIT lowering
anyway (`Inst::RecordLine` is not lowerable), but turning the JIT off outright keeps
the measurement from depending on which arms happened to tier up. **A `--cover-lines`
run is a diagnostic run, never a timing one.**

### What "executable line" means, and why the denominator is not counted from source

Only **positioned nodes** are instrumented — calls and inlined prims, the same set
that tags a runtime error with a line. So "line 12 never ran" means "no call on line
12 ran", and a function whose body is a bare literal (`(defn greeting () "hi")`)
contributes no measurable lines at all. A file of nothing but literal-bodied functions
is omitted from the report rather than shown as 0%.

The denominator therefore comes from the compiler, via `%coverage-instrumented`, not
from reading the source. Two earlier attempts got this wrong in opposite directions,
and both looked plausible:

1. **Counting lines that hold a form.** A fully exercised fixture reported **14%**: a
   `defmodule` header, a docstring and a `defn`'s own line all hold forms and none is
   an instrumented node, so the two halves of the ratio described different
   populations.
2. **Counting what had been instrumented, without forcing compilation.** Arms compile
   on first **call**, so a function nothing calls was missing from the denominator as
   well as the numerator, and the same fixture reported **100%** with a dead function
   in it.

Hence `coverage-line-begin!`, which runs before the suite and forces every project
function to compile (`%coverage-precompile`) without calling it. A never-called
function then has its lines in the denominator and in nothing else — which is the
whole point of the measurement.

**Known under-count:** a nested `(fn …)` inside a body compiles when the enclosing
body runs, so an unexecuted body's inner closure stays unmeasured. Strictly smaller
than not forcing at all, and it errs toward reporting less coverage, not more.

### A side-effect worth knowing: std module attribution

Building this surfaced a pre-existing misattribution. Baked-in std modules are loaded
from an embedded string with no path, and their forms inherited whatever file was being
loaded when the `require` ran. Observed: a 21-line `src/main.blsp` credited with
`std/log`'s lines 127-131, 150-152 and 175. The same field (`CompiledArm::src_file`)
also names the file in `:trace` frames, so the misattribution was not confined to
coverage.

`%load-string` now takes an optional name and `require--force` passes
`<std>/log.blsp`: honest that there is no openable path, and no longer someone else's
name. (`source-location` was never affected — definition sites are recorded
separately.)

## The two tiers together

They answer different questions and can be used together. Function coverage needs no
kernel support and no special build, which is why it ships as the default tier;
`--cover-lines` is stricter and more expensive. With both on, `--cover-min` gates on
the **line** percentage, that being the stricter number.
