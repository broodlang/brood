# Architecture

## Why this project exists

Brood began as the language for a modern, Emacs-like text editor whose defining
property is the one that makes Emacs Emacs: **the editor is written in the language
it hosts, and a running editor can redefine itself on the fly.** It has since grown
into a general-purpose Lisp and runtime, implemented in Rust. This repo holds the
language core, the runtime (processes, distribution, VM + JIT), the standard library
and the `std/editor/*` framework for interactive applications; the editor app itself
is a separate downstream project.

Two requirements from that origin still shape everything:

1. **Locally it must feel fast and native.** One runtime, no mandatory network
   hop, native rendering.
2. **The same runtime must be able to act as a server for other instances.**

The architecture is built so those two are the *same* code path, not two ports.

## The seam: one runtime, frontend-as-a-protocol

```
            ┌─────────────────────────────────────────────────┐
            │                 one runtime process              │
            │                                                  │
   keys/    │   ┌──────────┐   ┌───────────┐   ┌───────────┐   │
   events ─▶│   │  Lisp    │◀─▶│  editor   │◀─▶│  display  │   │── display ops ─▶ frontend(s)
            │   │  core    │   │  model    │   │  protocol │   │◀─ input events ─
            │   │          │   │           │   │           │   │
            │   └──────────┘   └───────────┘   └───────────┘   │
            │         ▲                              ▲          │
            │         │                              │          │
            │   live redefinition            in-proc native    │
            │   (eval / load / REPL)         frontend + TCP/WS server
            └─────────────────────────────────────────────────┘
```

Two commitments the design rests on, both built (milestones M2–M4):

1. **The Lisp owns the editor.** Buffers, cursors, keymaps, commands, and UI
   are Lisp values and Lisp functions (`std/editor/*`). Rust supplies primitives
   (rope operations, drawing, I/O); *policy* lives in Lisp and is hot-swappable.

2. **The frontend is a protocol, not a library.** The display layer emits a
   serialisable stream of "render this" operations and consumes input events. The
   local native frontend implements that protocol *in-process* (the fast path);
   a remote frontend implements the *identical* protocol over a socket
   (`nest attach`, ADR-090). That is what lets "fast native locally" and "server
   for other instances" be the same code.

Why this is achievable for free: because the editor is written in Brood,
hot-reload is just *re-evaluating definitions into the live global
environment* — no host-language hot-reload machinery required.

## Crate layout

```
Brood/
  Cargo.toml            workspace
  crates/
    lisp/               the language and runtime (the substance of the repo)
      src/              the directory tree mirrors the layers (see lib.rs)
        lib.rs          the module map + the `Interp` entry point
        boot.rs + boot/ how a runtime comes to hold the prelude (image / cache / source);
                        boot/image.rs the startup-image mechanism (ADR-218)
        core.rs + core/ substrate — what everything is addressed through
          value.rs        Value, Tag, handle types, symbol interner, Closure/Arity
          heap.rs + heap/ per-process heap + shared regions; env chain, promotion,
                          equality, gc, freeze, positions as child modules
          alloc.rs        process-wide byte-counting global allocator
        syntax.rs + syntax/   surface syntax (reader/printer round-trip + the tooling CST)
          reader.rs       text -> Value (recursive-descent parser)
          printer.rs      Value -> text
          scanner.rs, atom.rs, cst.rs, scope.rs   the tooling CST
        eval.rs + eval/ the evaluation engine
          eval.rs         tree-walking evaluator + special forms
          machine.rs      the tree-walker's heap frame stack and tail calls (ADR-416)
          compile.rs + compile/   the closure-compiling bytecode VM — the DEFAULT engine (ADR-076) —
                          and the tiering glue + Cranelift lowering for native code
          macros.rs + macros/     quasiquote, macroexpand, the compile pass + pattern lowering
        types.rs + types/   advisory types (nothing gates on it)
          types.rs        the Ty / GradualTy set-theoretic lattice
          check.rs + check/   advisory type checker over expanded forms
        builtins.rs + builtins/   the primitive kernel — one Rust file per domain, each
                          owning its implementations AND its registrations
        process.rs + process/   green-process scheduler (spawn/send/receive/monitor)
        dist.rs + dist/         distributed nodes (handshake/heartbeat/wire, ADR-033/034)
        jit.rs + jit/     the JIT ABI, backend contract and `brood_rt_*` callbacks
                          (feature = "jit", ADR-101/221)
        host.rs + host/   feature-gated machine bindings: net (ADR-062), subprocess,
                          wasm, treesit, clipboard, audio, text_width, gui + gui/ (the frontend)
        diagnostics.rs + diagnostics/   coverage, perf counters, profiler, debug flags
        bundle.rs         single-binary app bundling (ADR-038)
        renames.rs, introspect.rs, stack.rs   the rename ledger (ADR-304), reflection, stack growth
        error.rs          LispError / LispResult / source Pos
        cli_support.rs    file-runner / error-reporting shared by the binaries
      tests/            Rust e2e + the .blsp suite (see the tests/ tree)
    cli/                the `brood` binary: REPL + file runner + `--test`
      src/main.rs
    nest/               the `nest` binary: a thin shell over std/tool/nest.blsp (ADR-028, ADR-322)
      src/main.rs
    lsp/                the `brood-lsp` binary: language server (ADR-025, lsp.md)
      src/main.rs
    playground/         the wasm-bindgen in-browser playground
    tab/                `brood-tab`: runs a bundled program in a browser tab (ADR-414)
  std/                  grouped (ADR-085); bare module names despite the folders
    prelude/            the core library, written in Brood itself (13 files, in the order boot.rs lists)
    tool/               the toolchain — test framework, project runner, docs, repl, dev, …
    editor/             the buffer/display/ui/keymap framework (M2/M3)
  tests/                the in-language suite (`tests/**/*_test.blsp`)
  docs/                 you are here  (see components.md for the full map)
```

For a per-component breakdown — responsibilities, interfaces, and what's safe to
work on independently — see [components.md](components.md).

## How evaluation works

There are three engines, and `BROOD_TIER=0|1|2` (ADR-222) is the ceiling on which of
them runs: the tree-walker (0), the bytecode VM (1) and native code (2, the default).
A form is macro-expanded and compiled to a bytecode chunk (`eval/compile/`), and hot
arms tier up to native code through Cranelift. A form or closure outside the
compiler's vocabulary defers to the tree-walker, which is also the reference
semantics the differential tests compare the VM against.

The tree-walker's `eval(expr, env)` is **a loop over a heap frame stack, not
recursion** (ADR-416, `eval/machine.rs`). A non-tail sub-form pushes a continuation
naming what to do with its value and runs on the same loop, so a process body parks
at a `receive` under tier 0 as it does on the VM. A form in *tail position* — the
last form of a body, a chosen `if`/`cond` branch, the call in a tail call — pushes
nothing. The upshot: deeply recursive Brood code (the natural way to write loops in
a Lisp) does **not** grow a stack on any engine. `tail_calls_do_not_overflow`
proves this by summing to 100,000 via tail recursion.

The tree-walker's dispatch order for a list form `(head ...)`:

1. If `head` is a symbol naming a **special form**, handle it directly
   (`quote`, `if`, `fn`, `let`, `def`, …). Special forms control evaluation of
   their arguments.
2. Otherwise evaluate `head` and the arguments, then **apply**: a `Native`
   builtin is called directly; a `Fn` closure binds its parameters in a child of
   its captured environment and its body's last form is evaluated in tail
   position.

Symbols are **interned** to `u32` ids (see `core/value.rs`), so lookups and equality
are integer operations and the spelling is stored once.

## Memory model

Heap values are not `Rc` pointers: `Value` is `Copy` and its heap variants
are integer **handles** into a per-process `Heap` of plain `Vec` slabs (so a
`Heap` is `Send` and a process moves between scheduler threads). Reclamation is
a **per-process, generational, semi-space copying GC** (ADR-055/061/072):
a nursery + tenured old generation, collected at the eval safepoint at *any*
depth — stop-the-world for that one process only, since heaps are never
shared. Slots are **never reused in place** (survivors relocate; handles carry
a generation epoch, so a stale handle trips a debug tripwire instead of
aliasing — the invariant that closed the scheduler race surface, `f90f0de`).
The shared `def`'d code region is reclaimed separately by the
two-generation RUNTIME collector (ADR-091); large immutable byte strings live
in the refcounted cross-process blob heap (`core/blob.rs`). The whole model
stays contained because *all heap construction goes through the helpers in
`core/heap.rs` / `core/value.rs`*. See [memory-model.md](memory-model.md) and
[shared-code.md](shared-code.md) for the regions and hot-reload story.

## Dependencies

The early "zero external crates" rule has been relaxed (ADR-005 superseded): a
well-scoped crate is allowed when it removes real complexity from the *runtime
substrate* — but Lisp-callable behaviour still belongs in Brood, not a crate.
Current set:

- `boxcar` (lisp) — lock-free, append-only vector backing the shared RUNTIME
  code region (stable refs under concurrent `def`; see shared-code.md).
  (`corosensei` used to sit here for the green-process scheduler; it was
  removed 2026-06-08 when suspension became state capture — ADR-100.)
- `cranelift-*` (lisp, `jit` feature — a default feature) — the native-tier JIT
  backend (ADR-101; tier 2 of `BROOD_TIER`, ADR-222).
- `mimalloc` (lisp) — the global allocator; allocation throughput is
  load-bearing for the copying GC.
- `arc-swap` (lisp) — the RUNTIME code-generation slots for the ADR-091
  two-generation collector.
- `clap` (cli, nest) — argument parsing for the binaries. (`rustyline` used to
  sit here too; the REPL is Brood now, `std/tool/repl.blsp`, ADR-048.)
- `ropey` (lisp) — the text rope backing editor buffers (M2, ADR-045): an
  `Arc`-shared B-tree giving O(1) clone + copy-on-write edits, so immutability is
  free. The one irreducible text mechanism; everything above it is Brood.
- `crossterm` (lisp) — the terminal frontend for the M3 display/input seam
  (ADR-046): raw mode, the alternate screen, key events, styled output. The
  `term-*` primitives are the in-process frontend that paints the render-op
  protocol (which is itself Brood data); a remote frontend speaks the same ops.
- `divan` (dev only) — the microbenchmark harness; the released library pulls
  nothing extra.

Later features brought their own (no `tokio`/`serde` — the socket layer is
hand-rolled non-blocking TCP over `mio`, ADR-062, and the wire codec is ours): `rustls` +
`rcgen` for TLS (ADR-089), `tree-sitter` (+ grammars, `treesit` feature) for
foreign-language parsing, `image` for decode/thumbnail, the `winit`/
`softbuffer`/`cosmic-text` stack behind the `gui` feature (ADR-046) and `wgpu`
behind `gui-gpu`, `wasmtime` for the WASM host (ADR-071), `stacker` for native
stack growth, `regex-automata` for the native regex engine (ADR-389), and the
`unicode-width`/`unicode-segmentation` pair for the terminal/rope seam.
