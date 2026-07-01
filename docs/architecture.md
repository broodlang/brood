# Architecture

## Why this project exists

The goal is a modern, Emacs-like text editor whose defining property is the one
that makes Emacs Emacs: **the editor is written in the language it hosts, and a
running editor can redefine itself on the fly.** To get there we first need the
language. Brood is that language, implemented in Rust.

Two requirements shape everything:

1. **Locally it must feel fast and native.** One runtime, no mandatory network
   hop, native rendering.
2. **The same runtime must be able to act as a server for other editor
   instances**, and later expose a web-style frontend.

The architecture is built so those two are the *same* code path, not two ports.

## The seam: one runtime, frontend-as-a-protocol

```
            ┌─────────────────────────────────────────────────┐
            │                 one runtime process              │
            │                                                  │
   keys/    │   ┌──────────┐   ┌───────────┐   ┌───────────┐   │
   events ─▶│   │  Lisp    │◀─▶│  editor   │◀─▶│  display  │   │── display ops ─▶ frontend(s)
            │   │  core    │   │  model    │   │  protocol │   │◀─ input events ─
            │   │ (today)  │   │ (later)   │   │ (later)   │   │
            │   └──────────┘   └───────────┘   └───────────┘   │
            │         ▲                              ▲          │
            │         │                              │          │
            │   live redefinition            in-proc native    │
            │   (eval / load / REPL)         frontend + TCP/WS server
            └─────────────────────────────────────────────────┘
```

Two commitments we make from the start, even though only the left box exists today:

1. **The Lisp owns the editor.** Buffers, cursors, keymaps, commands, and UI
   will be Lisp values and Lisp functions. Rust supplies primitives (rope
   operations, drawing, I/O); *policy* lives in Lisp and is hot-swappable.

2. **The frontend is a protocol, not a library.** The display layer will emit a
   serialisable stream of "render this" operations and consume input events. The
   local native frontend implements that protocol *in-process* (the fast path);
   a remote or web frontend implements the *identical* protocol over a socket.
   That is what lets "fast native locally" and "server for other instances" be
   the same code.

Why this is achievable for free: because the editor is written in Brood,
hot-reload is just *re-evaluating definitions into the live global
environment* — no host-language hot-reload machinery required.

## Crate layout

```
Brood/
  Cargo.toml            workspace
  crates/
    lisp/               the language (this is the substance today)
      src/              the directory tree mirrors the layers (see lib.rs)
        lib.rs          the `Interp` entry point; bundles the prelude
        core/           substrate — what everything is addressed through
          value.rs        Value, Tag, handle types, symbol interner, Closure/Arity
          heap.rs         per-process heap + shared regions, env chain, promotion, equality
          alloc.rs        process-wide byte-counting global allocator
        syntax/         surface syntax (reader/printer round-trip + the tooling CST)
          reader.rs       text -> Value (recursive-descent parser)
          printer.rs      Value -> text
        eval/           the evaluation engine
          mod.rs          tree-walking evaluator + special forms + tail calls
          compile/        the closure-compiling bytecode VM — the DEFAULT engine (ADR-076)
          macros.rs       quasiquote, macroexpand, the compile pass + pattern lowering
        types/          advisory types (nothing gates on it)
          mod.rs          the Ty / GradualTy set-theoretic lattice
          check.rs        advisory type checker over expanded forms
        builtins/         the primitive kernel — Rust functions, split by area
                          (numeric / sequences / io / terminal / system / bytes)
        process.rs + process/   green-process scheduler (spawn/send/receive/monitor)
        dist.rs + dist/         distributed nodes (handshake/heartbeat/wire, ADR-033/034)
        jit/              tier-1 Cranelift JIT callback surface (feature = "jit", ADR-101)
        net.rs            non-blocking TCP socket mechanism (ADR-062)
        gui.rs, audio.rs, treesit.rs   optional feature-gated frontends/parsers
        bundle.rs         single-binary app bundling (ADR-038)
        error.rs          LispError / LispResult / source Pos
        cli_support.rs    file-runner / error-reporting shared by the binaries
      tests/            Rust e2e + the .blsp suite (see the tests/ tree)
    cli/                the `brood` binary: REPL + file runner + `--test`
      src/main.rs
    nest/               the `nest` binary: project tooling — `new` / `test` / `doc` (ADR-028)
      src/main.rs
    lsp/                the `brood-lsp` binary: language server (ADR-025, lsp.md)
      src/main.rs
  std/                  grouped (ADR-085); bare module names despite the folders
    prelude.blsp        the core library, written in Brood itself
    tool/               the toolchain — test framework, project runner, docs, repl, …
    editor/             the buffer/display/ui/keymap framework (M2/M3)
  tests/                the in-language suite (`tests/**/*_test.blsp`)
  docs/                 you are here  (see components.md for the full map)
```

For a per-component breakdown — responsibilities, interfaces, and what's safe to
work on independently — see [components.md](components.md). Later milestones add
`crates/editor`, `crates/server`, and a frontend crate; the `lisp` crate stays
the foundation.

## How evaluation works

`eval(expr, env)` is a tree-walker, but the load-bearing detail is that it is a
**`'tail: loop`, not plain recursion**. For any expression in *tail position* —
the last form of a body, a chosen `if`/`cond` branch, the call in a tail call —
the evaluator reassigns its `expr`/`env` locals and loops instead of recursing.
The upshot: deeply recursive Brood code (the natural way to write loops in a
Lisp) does **not** grow the Rust call stack. The test suite proves this by
summing to 100,000 via tail recursion (kept at 100k rather than millions because
arithmetic is now defined in Brood itself, and so is slower than a native loop).

Dispatch order for a list form `(head ...)`:

1. If `head` is a symbol naming a **special form**, handle it directly
   (`quote`, `if`, `fn`, `let`, `def`, …). Special forms control evaluation of
   their arguments.
2. Otherwise evaluate `head` and the arguments, then **apply**: a `Native`
   builtin is called directly; a `Fn` closure binds its parameters in a child of
   its captured environment and its body's last form is evaluated in tail
   position.

Symbols are **interned** to `u32` ids (see `core/value.rs`), so lookups and equality
are integer operations and the spelling is stored once.

## Memory model (and the plan to change it)

Heap values are no longer `Rc` pointers: `Value` is `Copy` and its heap variants
are integer **handles** into a per-process `Heap` of plain `Vec` slabs (so a
`Heap` is `Send` and a process can move between scheduler threads). Reclamation
today is **arena reset at top-level boundaries**: between top-level forms the
LOCAL heap holds nothing live but the result, so `eval_str` / the REPL truncate
it back (globals live in the shared PRELUDE/RUNTIME regions, never in LOCAL).

What's still open: bounding memory inside *long-running tail-recursive
computation* that never goes through `receive`. As of 2026-05-29
(commit `f90f0de`) the allocator is **bump-only** — slots are never
recycled, so a stale handle can't observe a different value, which closed
most of the scheduler race surface — but a tight loop that doesn't pass
through `receive` grows unboundedly per process. **Phase 2** (not yet
landed) is an *arena flip on `receive`*: deep-copy the surviving state to
a fresh slab, drop the old. The migration stays contained because *all
heap construction goes through the helpers in `core/heap.rs` /
`core/value.rs`*. See [memory-model.md](memory-model.md) and
[shared-code.md](shared-code.md) for the regions and hot-reload story.

## Dependencies

The early "zero external crates" rule has been relaxed (ADR-005 superseded): a
well-scoped crate is allowed when it removes real complexity from the *runtime
substrate* — but Lisp-callable behaviour still belongs in Brood, not a crate.
Current set:

- `boxcar` (lisp) — lock-free, append-only vector backing the shared RUNTIME
  code region (stable refs under concurrent `def`; see shared-code.md).
- `corosensei` (lisp) — stackful coroutines for the green-process scheduler, so
  the recursive evaluator parks at `receive` without a rewrite (scheduler.md).
- `rustyline` (cli) — line editing / history for the interactive REPL. A
  dev/UX dependency in the binary, not the library.
- `ropey` (lisp) — the text rope backing editor buffers (M2, ADR-045): an
  `Arc`-shared B-tree giving O(1) clone + copy-on-write edits, so immutability is
  free. The one irreducible text mechanism; everything above it is Brood.
- `crossterm` (lisp) — the terminal frontend for the M3 display/input seam
  (ADR-046): raw mode, the alternate screen, key events, styled output. The
  `term-*` primitives are the in-process frontend that paints the render-op
  protocol (which is itself Brood data); a remote frontend speaks the same ops.
- `divan` (dev only) — the microbenchmark harness; the released library pulls
  nothing extra.

More will arrive with the features that need them: `tokio` + `serde` for the
server/protocol.
