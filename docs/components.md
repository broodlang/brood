# Components

The **component map**: what the parts are, what each one owns, the interface
other parts lean on (its *seam*), and what you need to know to work on one
without touching the rest. This is the "who does what / what can be worked on
independently" view; [architecture.md](architecture.md) is the "why it's shaped
this way" view, and [language.md](language.md) / [primitives.md](primitives.md)
are the language surface.

## The layers

```
                         ┌──────────────────────────── entry points ───────────────────────────┐
                         │  crates/cli (the `brood` binary)      lib.rs  (the `Interp` API)      │
                         └───────────────────────────────┬──────────────────────────────────────┘
                                                          │ embeds
   POLICY (Brood)  ─────────────────────────────────────▼────────────────────────────────────────
        std/prelude.blsp   std/test.blsp   std/project.blsp        ← redefinable at runtime
   ───────────────────────────────────────────────────────────────────────────────────────────────
   MECHANISM (Rust)        language pipeline                          advisory types
        reader → macros → eval → printer                              types  ←  check
                          builtins (the primitive kernel)
   ─────────────────────────────────── substrate ───────────────────────────────────────────────
        value (Value, Tag, handles, interner)      heap (regions, env, promotion, equality)
        error      alloc (byte counter)            process (green-process scheduler)
```

Two boundaries do most of the structural work:

1. **Mechanism (Rust) vs policy (Brood).** Rust supplies primitives and the
   evaluator; everything that *can* be Brood lives in `std/*.blsp` (ADR-006/008).
   The seam between them is the **primitive kernel** — the set of names
   `builtins::register` installs, catalogued in [primitives.md](primitives.md).
   The std layer is written against those names plus the evaluator's special
   forms; it knows nothing of Rust internals.
2. **The `Heap` + `Value` hub.** Almost every Rust component takes `&mut Heap`
   and speaks in `Value` handles (see next section). That makes `Heap` the one
   shared dependency — convenient, but also the main thing to be aware of when
   reasoning about coupling.

## The central seam: `Value` + `Heap`

`Value` (in `value.rs`) is `Copy`: atoms inline, heap objects are small integer
**handles** (`PairId`, `VecId`, …) whose two high bits tag a *region*
(LOCAL / PRELUDE / RUNTIME — see [shared-code.md](shared-code.md)). You never
dereference a handle directly; you ask the `Heap` (`heap.pair(id)`,
`heap.string(id)`, `heap.alloc_pair(a, b)`, …). Consequences worth knowing
before working in any Rust component:

- A function that reads or builds values needs `&Heap` / `&mut Heap`. This is
  why `reader`, `printer`, `eval`, `macros`, `builtins`, `check`, `process` all
  thread it.
- **All heap construction funnels through `Heap`/`value.rs` helpers** (invariant,
  ADR-002/016). Don't allocate `Value` structure any other way — it keeps region
  tagging and the future GC contained.
- A `Heap` is `Send` (plain `Vec`s + `Arc`s, no `Rc`), so a process can move
  between scheduler threads. Keep it that way.

## Rust kernel — substrate

### `value.rs` — the value model · ~305 LOC
- **Owns:** `Value`, the first-class type tag `Tag` + `tag(v)`, the handle types
  and their region encoding, the process-wide symbol interner
  (`intern`/`symbol_name`/`symbol_is`), `Closure`, `Arity`, `NativeFn`, and the
  `sym`/`kw`/`gensym` constructors.
- **Depends on:** `error`, and `heap` only in type signatures (`NativeFnPtr`).
- **Exposes:** the vocabulary every other component is written in.
- **Work here independently:** adding a `Value` kind is the highest-blast-radius
  change in the repo — it needs a matching `Tag` (and a bit in `types.rs`, guarded
  by a test) and touches `printer`, `eval`, `heap`, `process::Message`. Check the
  compatibility contract in [types.md](types.md) first.

### `heap.rs` — heap, regions, environments · ~726 LOC (the heaviest)
- **Owns:** the per-process LOCAL data heap (slab `Vec`s); the shared `SharedCode`
  (PRELUDE) and `RuntimeCode` (RUNTIME) regions + their `Arc`s; allocation
  (`alloc_*`, `list`); access (`pair`/`car`/`cdr`/`vector`/`string`/`closure`);
  the **environment chain** (`env_get`/`env_define`/`env_set`/`env_root`/`new_env`,
  the `GLOBAL` sentinel → shared global table); **promotion** (LOCAL→RUNTIME deep
  copy on `def`/`spawn`); structural **equality** (`equal`, the basis of `=`);
  **memory reclamation** (`checkpoint`/`reset_local_to`); the prelude **freeze**;
  and editor **source metadata** (`form_pos`, `current_file`).
- **Depends on:** `value`, `error`, `boxcar`.
- **Exposes:** `Heap`, `SharedCode`, `RuntimeCode`, `LocalCheckpoint`.
- **Work here independently:** this is several concerns in one file (see the
  assessment below). The hot-reload / region rules in
  [shared-code.md](shared-code.md) are load-bearing — read it before changing
  promotion or the global table.

### `error.rs` — errors & source positions · ~152 LOC
- **Owns:** `LispError`, `ErrorKind`, `LispResult`, `Pos`, the `wrong_type`
  self-identifying constructor, and the GNU `FILE:LINE:COL:` rendering (`located`).
- **Depends on:** `value`, `printer` (to render the offending value).
- **Work here independently:** fully self-contained; the cleanest module to touch.

### `alloc.rs` — process byte counter · ~63 LOC
- **Owns:** the `#[global_allocator]` that tallies live/peak bytes for
  `(mem-bytes)`/`(mem-peak)`.
- **Depends on:** nothing (std only).
- **Work here independently:** isolated; the only coupling is that `lib.rs`
  installs it as the global allocator.

### `process.rs` — the green-process scheduler · ~431 LOC
- **Owns:** `spawn`/`send`/`receive`/`self`, mailboxes, the work-stealing worker
  pool, `corosensei` coroutines, and `Message` (the `Send` deep-copy that crosses
  heaps). Counters behind `spawn-count`/`peak-threads`/`worker-threads`.
- **Depends on:** `heap`, `eval`, `value`, `error`, `corosensei`.
- **Exposes:** the functions `builtins` wraps, plus `set_max_parallel` (CLI).
- **Work here independently:** well isolated behind those builtins. The `unsafe
  impl Send for Process` and the receive/park handshake are the subtle parts —
  see [scheduler.md](scheduler.md) / ADR-018.

## Rust kernel — language pipeline

### `reader.rs` — text → `Value` · ~321 LOC
- **Owns:** the recursive-descent parser; records form source positions into the
  heap for tooling.
- **Depends on:** `heap`, `value`, `error`.
- **Exposes:** `read_all`, `read_all_positioned`, `read_one`.
- **Work here independently:** input side only; round-trips with `printer`, so
  changing surface syntax usually means a matching `printer` change.

### `printer.rs` — `Value` → text · ~129 LOC
- **Owns:** `print` (readable, REPL) and `display` (human, `str`/`print`).
- **Depends on:** `heap`, `value`.
- **Work here independently:** output side only; the inverse contract of `reader`.

### `eval.rs` — the evaluator · ~539 LOC
- **Owns:** the `'tail: loop` tree-walker, **special forms** (`quote if do def
  set! fn/lambda quasiquote defmacro let/let* while`), closure application,
  parameter binding (`&optional`/`& rest`), and the native-call arity gate.
- **Depends on:** `heap`, `value`, `macros` (lazy expansion + `fn`/`let` lowering
  fallback), `printer`, `error`.
- **Exposes:** `eval`, `apply`, `apply_closure`, `truthy`.
- **Work here independently:** **proper tail calls are an invariant** — a new
  body-bearing special form must hand its last form back to the loop
  (`tail_of`), not recurse (guarded by `tail_calls_do_not_overflow`). Keep the
  *core* small (ADR: prefer a prelude macro over a new special form).

### `macros.rs` — expansion & the compile pass · ~294 LOC
- **Owns:** `quasiquote`, `macroexpand[-1]`, and `macroexpand_all` (the compile
  pass run at each top-level boundary), including the **pattern lowering** that
  desugars multi-clause / destructuring `fn` and `let` into the Brood `match*`
  engine.
- **Depends on:** `heap`, `eval`, `value`, `error`.
- **Exposes:** `macroexpand_all` (called by `lib`, `builtins::{eval,load,…}`),
  `quasiquote`, `fn_needs_lowering`.
- **Work here independently:** the `eval`↔`macros` pair is mutually recursive
  (eval calls back for the lowering fallback). Pattern-match *policy* is in the
  prelude; this file only lowers the surface to `match*`.

### `builtins.rs` — the primitive kernel · ~825 LOC (heaviest, multi-domain)
- **Owns:** every Rust-implemented primitive, registered into the prelude builder
  by `register`. Spans ~10 domains: numeric (`%add`…`rem`), pair/sequence,
  vector, string, type reflection (`type-of`), value↔text + I/O, time, memory,
  self-hosting (`eval`/`read-string`/`load`/`eval-string`/`%builtin-module`),
  symbols, filesystem (`cwd`/`file-exists?`/`dir?`/`list-dir`/`make-dir`/`spit`),
  system (`getenv`/`run-process`), macros, the type-`check` hook, source positions,
  errors/control (`throw`/`%try`/`%isolate`), and processes.
- **Depends on:** nearly everything — `heap`, `eval`, `value`, `printer`,
  `reader`, `macros`, `check`, `process`, `alloc`, `error`.
- **Exposes:** `register(&mut Heap, EnvId)` — the single install point.
- **Work here independently:** a primitive is `fn(&[Value], EnvId, &mut Heap) ->
  LispResult` plus one `def(...)` line with its `Arity`. Before adding one, ask
  whether it can be Brood instead (ADR-006). The annotated list is
  [primitives.md](primitives.md).

## Rust kernel — types (advisory; nothing gates on it)

### `types.rs` — the type lattice · ~491 LOC
- **Owns:** `Ty` (a set of `Tag`s; union/intersect/negate; subtyping = inclusion)
  and `GradualTy` (`dynamic()` inside the lattice). Pure algebra + its own tests.
- **Depends on:** `value` (for `Tag`).
- **Work here independently:** no runtime path consumes it except `check`. See
  ADR-023/024 and [types.md](types.md).

### `check.rs` — the advisory checker · ~212 LOC
- **Owns:** a walk over macro-expanded forms that warns on *provably* wrong
  primitive arguments (disjoint types). Never rejects; returns warning strings.
- **Depends on:** `types`, `heap`, `value`, `printer`.
- **Exposes:** `check_form` (behind the `(check 'form)` builtin).

## Embedding + binary

### `lib.rs` — the `Interp` embedding API · ~152 LOC
- **Owns:** building the shared prelude bundle once (`SHARED`), seeding a runtime,
  installing the counting allocator, and the top-level eval loop with
  per-form arena reset. The public face of the whole `brood` library crate.
- **Exposes:** `Interp::{new, eval_str, eval_source, print}`.
- **Work here independently:** this is the contract embedders (and the CLI) use;
  keep it small.

### `crates/cli/src/main.rs` — the `brood` binary · ~314 LOC
- **Owns:** arg parsing (`-j`/`--max-parallel`), the file runner, the
  `rustyline` interactive REPL + plain piped REPL, error rendering with a caret,
  and the `test` / `new` subcommands.
- **Depends on:** only `brood::Interp` (+ `error`, `process::set_max_parallel`)
  and `rustyline`. Cleanly decoupled from kernel internals.
- **Work here independently:** the `test`/`new` subcommands drive Brood by
  embedding source strings (`(require 'project) …`) — a deliberate bootstrap;
  the roadmap goal is to move the REPL/CLI into Brood.

## Brood standard library (policy — redefinable at runtime)

### `std/prelude.blsp` — the core library · ~465 LOC
- **Owns:** `defn`; logic; folding (`reduce`/`map`/`filter`); variadic
  arithmetic & comparison over the 2-arg primitives; control-flow macros
  (`when`/`unless`/`and`/`or`/`cond`); sequence ops; threading macros
  (`->`/`->>`); error handling (`error`, `try`/`catch` over `%try`); the
  **pattern-match compiler** (`match*`/`match`, reused by `let`/`fn`); string &
  path helpers; and the **module system** (`provide`/`require`/`*load-path*`).
- **Baked into the binary** via `include_str!` in `lib.rs`; frozen into PRELUDE.
- **Work here independently:** this is where new language features should go by
  default. Add a test in `tests/suite_test.blsp`; document in `language.md`.

### `std/test.blsp` — the test framework · ~395 LOC
- **Owns:** ExUnit-style `describe`/`test`/`deftest`, the assertions, and the
  parallel-by-default runner with `:serial`/`:isolated` (over `spawn`/`%isolate`).
- **Loaded on demand** via `(require 'test)`; embedded through `%builtin-module`.
- **Work here independently:** see [testing.md](testing.md). Depends on the
  process primitives and `%isolate`.

### `std/project.blsp` — project model, runner, scaffolding · ~209 LOC
- **Owns:** the `project.blsp` manifest, test discovery + `run-project-tests`,
  the user config (`~/.config/brood/config.blsp`), and `brood new` scaffolding.
  The policy behind the CLI's `test`/`new`.
- **Depends on:** the filesystem primitives + `test`. See ADR-020.

## Tests & benches

- **`crates/lisp/tests/basic.rs`** (~607) — Rust end-to-end language tests
  (read→eval→print), plus the `Heap: Send` guard.
- **`crates/lisp/tests/suite.rs`** (~21) — runs the in-language suite via the
  project runner from the repo root.
- **`tests/**/*_test.blsp`** — the in-language suite (pattern matching, modules,
  the main `suite_test.blsp`), discovered by `brood test` / the runner.
- **`crates/lisp/benches/eval.rs`** — `divan` microbenchmarks; archived by
  `scripts/bench.sh` (see [the benchmarks dir](benchmarks/)).

---

## Separation of concerns — assessment

### What's already well separated (work on these in isolation today)

- **`reader` / `printer` / `error` / `alloc` / `types` / `check` / `process`**
  each have one job and a narrow, documented interface. `process` in particular
  hides a lot of complexity behind a handful of builtins.
- **The CLI** depends only on the `Interp` API — kernel internals can change
  underneath it freely.
- **The mechanism/policy (Rust/Brood) split** is the strongest boundary in the
  repo and is well enforced: the std layer is written purely against registered
  primitive names + special forms.

### Coupling hotspots (ranked)

1. **`heap.rs` is a god-object (~726 LOC, ~6 concerns).** It bundles slab
   allocation, the three regions + freeze/promotion, the **environment chain**,
   structural **equality**, memory-reclamation **checkpoints**, and editor
   **source metadata** (`form_pos`/`current_file`). The environment logic used to
   be its own `env.rs` (architecture.md still says so); the source-metadata fields
   are tooling state that has nothing to do with allocation. These are the parts
   most likely to be edited by *different* people for *different* reasons.
   *Recommendation:* split out `env.rs` (the chain operations over heap-stored
   frames) and move `form_pos`/`current_file` to a small `source.rs` (or onto the
   reader/load path). Equality could also move to a `value`-adjacent module.

2. **`builtins.rs` is a 10-domain monolith (~825 LOC) with dead code.** Ten
   domains in one file means edits to, say, the filesystem primitives sit next to
   unrelated numeric code. And `is_nil`/`is_pair`/`is_int`/`is_float`/`is_bool`/
   `is_string`/`is_symbol`/`is_keyword`/`is_vector`/`is_fn` plus `println` are
   **defined but never registered or called** — the predicates moved to Brood
   over `type-of`, but the Rust versions were left behind (dead-code warnings).
   *Recommendation:* delete the dead functions now; optionally split into a
   `builtins/` module-per-domain with a single `register` in `mod.rs`.

3. **`docs/architecture.md` is stale — the component map is wrong.** It lists
   `env.rs` (gone), claims "zero external crates" (now `boxcar`, `corosensei`,
   `rustyline`, `divan`), describes `Rc`/`RefCell` memory (migrated to `Send`
   handle heaps), and omits `heap`/`process`/`types`/`check`/`macros`/`alloc`.
   A wrong map is the single biggest blocker to working independently.
   *(Fixed alongside this doc.)*

4. **CLI ↔ Brood string coupling (low priority).** `main.rs` embeds Brood
   snippets for `test`/`new`. This is an acknowledged bootstrap (roadmap:
   self-host the CLI in Brood); fine to leave until the language can express it.

### Suggested order if we act on the above

1. Delete the dead builtins (trivial, removes warnings, no behaviour change).
2. Extract `env.rs` from `heap.rs` (biggest clarity win; mechanical).
3. Move `form_pos`/`current_file` out of `Heap` into a source/tooling module.
4. (Optional) Split `builtins.rs` into a `builtins/` directory by domain.
