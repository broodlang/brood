# ROADMAP

Brood is the **language and runtime** for a modern, Emacs-like editor — a fast
native app locally, a server for remote instances. The editor app itself is a
separate downstream project (out of scope here); it consumes this language and
the `std/editor/*` framework. Brood's job is the language core, runtime, and
that framework. We get there in milestones (M1–M5), each shippable and useful
on its own.

Guiding constraints (see `CLAUDE.md`): keep the **language core small** — prefer
adding a primitive function or a prelude macro over a new special form — and write
as much as possible *in Brood itself*. Tags: **[kernel]** = needs new Rust;
**[Brood]** = can be written in the prelude.

Legend: ✅ done · 🟡 in progress · ⬜ not started · ❌ tried and reverted

> This is the single canonical roadmap. Deep design lives in the per-topic docs
> under [`docs/`](docs/) (ADRs in [`docs/decisions.md`](docs/decisions.md), a
> dated history in [`docs/devlog.md`](docs/devlog.md)).

---

## Active work — dated findings & backlogs

### Findings from hatch (2026-07-11)

Three runtime/language items surfaced while eliminating a whole *class* of O(n²)
bugs in [`hatch`](../hatch) (the Brood web framework). Every one was the same
shape — `(str acc x)` / `(bytes-concat acc x)` accumulated in a per-read loop,
quadratic in the read count — and every fix was the same manual idiom (cons onto a
list, `reverse` + `join` once), written five times across the HTTP/WebSocket stack
(body drain, head reader, chunked de-chunk, WS reassembly, live-view render). These
would retire the bug class at the language level. See hatch's
[`docs/tcp-http-audit.md`](../hatch/docs/tcp-http-audit.md) §16–§17.

- ⬜ **Iolists — the highest-leverage one.** Let the I/O + join builtins
  (`tcp-send`, `spit`/`append-bytes`, `join`, `str`, `bytes-concat`) accept
  *arbitrarily nested lists of strings/bytes*, flattened only at the write boundary
  — the Erlang/Elixir model, a natural fit given Brood's process/`receive` core.
  You'd describe the structure (`[status-line headers "\r\n\r\n" [s0 d0 s1 …]]`)
  and nothing is copied until the socket writes it flattened. Makes the correct
  thing the default and deletes the whole accumulation bug class. **[kernel]**
  flatten-on-write in the I/O builtins + teach `join`/`str`/`bytes-concat` to
  accept nesting.
- ⬜ **`bytes`-native HTTP/WebSocket parsing (kill the carrier-string bridge).**
  `bytes` is now a first-class value (`byte-at`/`subbytes`/`bytes-index-of`/…), but
  the string parsers predate it, so every socket read does
  `(str buf (bytes->carrier chunk))` — a Latin-1 "carrier string", one codepoint
  per byte. That conversion is *why* the read buffer is a `(str)`-accumulated string
  (the O(n²) source), and the text/binary mode-flip it forces is what caused the
  original U+FFFD live-nav bug. Give `bytes` a fuller search/slice surface, then port
  the parsers. **[kernel]** a few more `bytes` primitives, then **[Brood]** port the
  parsers. (This is the "one bad abstraction", narrowed now that `bytes` exists.)
- ⬜ **A growable read buffer (or `bytes` transient).** The input-side twin of
  iolists: an append buffer that freezes to immutable `bytes` on read would make the
  request head reader, chunked drain, and WS frame gather trivially O(n) — no manual
  list+`join`, no length-drain gymnastics. **[kernel]** a transient/builder value +
  freeze.
- ⬜ Smaller ergonomic wins (all cheap): **`mapv`/`filterv`** (vector-returning
  variants — `map`/`filter`/`fold` return lists, so hatch littered `(into [] (map …))`
  wherever a vector was needed); making the **`foo--private` convention
  link-checked** rather than a runtime unbound-symbol surprise (it bit a cross-module
  call during the hatch work); and either fixing or erroring on **`let`
  vector-destructure of a list value**.

### Elixir-parity performance gaps (2026-07-12, refreshed 2026-07-14)

Benchmarked brood ÷ **Elixir** per row (`../brood-benchmarks`). Elixir is *also*
immutable + GC'd + boxed-float + actor-based, so **every gap here is an
implementation deficiency, not an "immutability tax"** — the bar is "match an
immutable peer," and BEAM proves each is reachable. Ranked by ratio; `[kernel]`
unless noted.

**The four rows where Brood is currently *last of 7 languages* (7/7 on the
2026-07-13 report) are `nbody`, `regex`, `sieve`, and `persistent-map`** — the
priority set below, each with the "how the BEAM does it" lever spelled out.
`json`/`base64` stay excluded as gaps (Elixir/Node win them with native C codecs
against our by-design pure-Brood code — a separate, lower-priority pure-Brood-codec
track), but **`regex` is pulled in as a real entry** because its lever is structural
(engine design), not merely "we lack a C library."

- 🔶 **`nbody` — was 7/7 (~40× Elixir, 5.9s); now ~0.82s (~8× total), ~5× Elixir
  (2026-07-14).** The gap was **not** float-across-calls (the `docs/jit-float.md` premise) —
  it was two things, both fixed:
  1. **Data structure (benchmark).** Bodies were a `(list …)`, so `(f b i k)` =
     `(nth (nth b i) k)` did an **O(i) list walk**, re-walked per field, where every other
     port indexes an O(1) array/tuple (Node `x[i]`, Elixir `elem(b,i)`). Two
     faithful-transcription fixes in `brood-benchmarks/bench/brood/nbody.blsp`: bodies
     `(list …)` → **vector** (~3.3× on the VM) and **bind `bi`/`bj` once** (drop the
     re-walking `f` helper, matching Elixir; +~23%). → 6.65 → 1.25s.
  2. **JIT deopts (kernel — committed, branch `perf/jit-nbody-float`).** At 1.25s the JIT was
     *net-neutral* (jit ≈ no-jit): `BROOD_DEOPT_TRACE` showed `newvel`/`advance-body`
     deopting on ~every call (~498k). Two root causes: **(a)** `inline_vec_ref` deopted on
     any vector past `INLINE_VEC_CAP` (2) — nbody's **7-element** body vectors are
     heap-backed, so every constant-index `(nth v k)` fell to the VM; fixed by falling back
     to the `brood_rt_vector_ref` helper on the non-inline branch (bintree's 2-elem inline
     path unchanged). **(b)** `(nth v k)` yields an `Op::Handle` (type-erased) and
     `op_is_float(Handle)` is `false`, so `(- (nth bi 0) (nth bj 0))` took the integer path →
     `as_int` → deopt on the `Float` tag; fixed by `as_f64(Handle)` tag-checking `Float` +
     extracting, and routing `Handle`-operand arithmetic to the float path in float-context
     arms (`has_float_slot`) — deopt-safe (a wrong guess deopts, never miscompiles), and a
     right guess yields `Op::Float` that cascades unboxed via `store_op`'s `slot_float` mark.
     Also added float `/` to `emit_float_arith` (zero-divisor guard → deopt, matching the
     VM's `(/ x 0.0)` error). `newvel` now runs fully native (deopts 498k → 249k). → 1.25 →
     ~0.82s. Verified: suite **2730/2730**, jit 28/28, differential 2/2, all 13 numeric
     benches bit-identical to `BROOD_VM=0`, `GC_STRESS`+`VERIFY`+`JIT_VERIFY` clean, bintree
     unregressed.

  **Compute is now ~780ms vs Python 729ms — likely still 7/7 by a hair** (or a marginal 6/7;
  re-run the harness to confirm the rank). To pull clearly ahead: **(1)** the residual ~249k
  deopts are `advance-body`, which has no float *param* so the `has_float_slot` gate misses
  it — catching it needs a float-context signal that survives `(nth …)`/call-return type
  erasure (cross-arm return typing, or a compile-time float-global check for `dt`) *without*
  regressing int-vector arms (matmul/nqueens spuriously deopting); **(2)** inline `sqrt` as
  Cranelift `fsqrt` (a boxed Brood call ~1.5M times) via a `PrimOp1::Sqrt` recognised like
  `nth`→`VectorRef`; **(3)** cut the ~1M `global_ic_miss` on `dt`/`sm` reads in call-mediated
  arms. **Layer B (typed cross-arm float ABI) is deprioritised** — the hot calls have no float
  *args*. The next big general win is **full float type-specialization** (profile-drive an
  arm's float slots/stack so vector-read floats stay unboxed everywhere, covering
  advance-body too). **[kernel/JIT + benchmark]**
- ⬜ **`regex` — ~62× vs Elixir (981ms v 16ms), 7/7 (interpreted CPS backtracker).**
  Erlang compiles `~r/…/` once to a PCRE NFA and matches native. Brood's pure-Brood
  engine already **memoises the parse** (`regex--compile`, a Table cache), so the
  residual is *not* re-parsing — it is the CPS backtracking matcher itself: every
  `regex--star`/`seq-match` step allocates a fresh continuation closure `(fn (i2) …)`,
  and set membership is a linear `string-contains?` scan of the class string per char.
  Levers, ascending cost/benefit: **(a)** compile the AST to a closure-free NFA/DFA
  (state table + a flat step loop, no per-step closure alloc — stays pure-Brood, the
  dogfood-correct fix); **(b)** cheaper set membership (a char→bool set built once at
  compile). A native regex primitive is the fast-but-wrong escape hatch (reverses
  "write it in Brood") — keep it out unless (a)/(b) prove insufficient. **[std/Brood]**
- ✅ **`errors-deep` — was 26× (mis-filed as "throw/unwind cost"), FIXED (`3cefcad`,
  branch `perf/errors`, 2026-07-15): 0.28 → 0.07 s (~4×, 5/7 → ~2/7 by compute — past
  Ruby/Node/Python, behind only Elixir).** The diagnosis inverted the premise: throw +
  catch with zero frames between is ~free and the unwind was always cheap — the linear
  ~96 ns/frame cost was the `throw` call **knocking `descend` out of the unboxed-i64
  register worker's subset**, so all 2.5 M frames were *built* on the interpreted VM
  call protocol. Fix: the register worker lowers `(throw <scalar>)` via a
  `brood_rt_i64_throw` callback (park error → sentinel 3 → native unwind → outcome 3),
  with a per-throw runtime check that global `throw` still binds the builtin (a redef
  deopts → the VM runs the redefinition — late binding exact). Verified: 3 engines
  bit-identical; payload identity (int + float workers); non-final-`do` throws; 40 k
  depth-bail; suite 777/777. **[kernel]**
- ⬜ **`sieve` — ~19× vs Elixir (1.0s v 52ms), 7/7 (Table op overhead).** Elixir uses
  `:atomics` — a lock-free fixed-size i64 array, ~one atomic instruction per access.
  Brood has no mutable array (the immutability invariant), so composite marks live in a
  `Table`, and **every** `table-put`/`table-has?` pays: a registry `Mutex` lock + an
  `Arc<Store>` clone + the store's own `Mutex` + a `to_message`/`from_message` deep-copy
  of the key (cross-process safety) — ~3.8M times here. All of that is waste for scalar
  (int) keys on a single-owner table. Levers (each keeps `Table` as the one sanctioned
  mutable, each general to *every* Table user): **(1)** cache the `Arc<Store>` on the
  `Value::Table` handle to skip the registry lock+clone per op; **(2)** fast-path
  immediate keys/values (int/bool are already heap-independent — store them directly,
  skip `to_message`); **(3)** a provably-single-owner path that elides the deep-copy. A
  bitset primitive would beat all three but adds a Rust builtin — defer. **[kernel]**
- ⬜ **`nqueens` — 15× (backtracking allocation).** List/closure allocation per branch;
  overlaps the HOF-fold and allocation paths. **[kernel]**
- ✅ **`ackermann` — was 14× (non-tail double recursion), FIXED (`f90910c`, 2026-07-13):
  4.02 → 0.36s, 7/7 → 3/7.** The i64 unboxed worker's subset checker only matched *non-tail*
  self-calls (fib's arg-position recursion); `ack`'s recursion is in *tail* position
  (`SelfCall`), and its native-recursion depth cap was a stale 1400 (< `ack`'s ~4093 depth).
  Taught the subset about tail self-calls + raised the cap to 32768. Now 3rd, past
  Node/Clojure/Ruby/Python. **[kernel/JIT]**
- ⬜ **`ring` / `pingpong` — 8× / 6× (residual message machinery).** Already cut from
  ~13× (ADR-135 + wake elision); the remainder is per-message `to_message`/`from_message`
  copy + continuation capture/restore. Trim the copy for small scalar messages; shrink
  the capture. **[kernel]**
- ⬜ **`bintree` — 7.5× (GC / allocation pressure).** Build+walk trees; per-node alloc +
  minor-GC throughput vs BEAM. **[kernel]**
- ⬜ **`loop` — 6× (raw iteration overhead).** A JIT'd int tail loop is still ~7×/iter
  vs BEAM (i64 worker engages but gives only ~12%). Per-iteration overhead: overflow-
  checked add, reduction/safepoint tick, frame reset. Incremental JIT-tuning grind (BEAM
  has a 25-yr lead) — expect small wins. **[kernel/JIT]**
- ✅ **`persistent-map` — was 5.2× vs Elixir (612ms v 118ms), 7/7; FIXED by lever (1)
  (2026-07-15, benchmark transcription in `brood-benchmarks`, no kernel change):
  0.71 → 0.16 s locally (~4.4×), harness-scaled ≈ 138 ms → past Clojure's 285 ms
  (7/7 → 6/7), within ~1.2× of Elixir.** The port's hand-written `get`+`assoc` (two
  descents) became `map-int-add` — the same fused single-descent RMW idiom `wordcount`
  already uses, and the faithful counterpart of Elixir's one-call `Map.update/4`.
  Diagnosis notes (measured, 2026-07-15): with `map-int-add` the loop is *already
  optimal* on the kernel side — the LINMAP rewrite turns the accumulator into a private
  Table (`map-int-add` → `table-incr`) and the loop **already runs JIT-native** (the
  letrec-style rewrite emits `SelfCall`, so the gate + back-edge tiering cover it);
  two hypothesized JIT levers (gate relaxation for defn-style tail loops with calls +
  a Call-tail back-edge escape) were implemented, measured **zero win**, and reverted —
  the residual floor is the per-iteration native-call/Table cost, the same shared floor
  as `sieve`/`regex`. Deferred levers (2)/(3) (assoc-path node alloc, general fused
  `update`) remain valid for *non*-linmap map workloads. **[benchmark + measured]**

### Findings from brood-life profiling (2026-06-13)

The four-axis language review from optimising `brood-life` (a GUI Game of Life) was
triaged proposal-by-proposal and the accepted items shipped 2026-07-09 (`clamp`,
`as->`, `{:keys …}`/`:or` map destructuring, lazy seq-view fusion, `read-string`
trailing-form drop, and more), alongside two allocation/GC bug fixes (transient
corruption, allocation serialisation). One item stays deferred:

- ⬜ **JIT float specialisation** — ordinary perf tuning (partial scaffolding in
  `compile/mod.rs`, "type-specialize float arms"); gated on a concrete hot float
  workload, not a completeness gap.

### Stability backlog (2026-07-10)

- ⬜ **Continuous fuzzing (`cargo-fuzz`)** — libFuzzer targets for the
  highest-risk untrusted-input parsers: the **reader/scanner**, **JSON**, the **dist
  wire framing** (`dist/wire.rs`), and the **bundle footer/archive** (`bundle.rs`).
  Highest long-term leverage for keeping the kernel stable as it grows.
- ⬜ **Host-panic hardening (audit residue)** — adversarial input can still panic
  the Rust host: no recursion-depth counter on `expr_ty`/`check_into` (checker stack
  overflow on deeply-nested types), no `catch_unwind` around the worker `run_one`,
  no RAII guard on `check_file`'s panic path.

---

## Done — the foundation

Compressed; per-item history is in [`docs/devlog.md`](docs/devlog.md) and
[`docs/archive/`](docs/archive/), decisions in
[`docs/decisions.md`](docs/decisions.md).

- **Stage 1 — a full functional Lisp.** Reader (lists/vectors/atoms/keywords,
  quasiquote); tree-walking evaluator with proper tail calls, lexical scope,
  closures, Lisp-1; macros (`defmacro`, quasiquote, `macroexpand`, `gensym`);
  `defn`/`&optional`/`& rest`; i64+f64 with overflow-checked arithmetic; immutable
  maps + `{ }` literals (ADR-030); the string, math, and sequence libraries;
  dynamic variables (`defdyn`/`binding`, per-process); pattern matching across
  `match`/`let`/`fn` (ADR-021/022) incl. `{:keys …}`/`:or`; `case`,
  `dotimes`/`dolist`, `letrec`; error handling (`throw`/`try`/`catch`/`error`) with
  source locations; modules (`provide`/`require`, `foo--private`, ADR-019); the
  project model + parallel test runner (ADR-020); reducible lazy `range` and
  transducers.
- **Concurrency — green processes on all cores** (`docs/concurrency.md`).
  `spawn`/`send`/`receive`/`self` with per-process `Send` heaps and copy-on-send;
  green M:N scheduling on a worker pool (corosensei, ADR-018); shared code region
  for cross-process hot reload (ADR-013/014); closures sent between processes and
  across nodes (ADR-033); reduction-counted preemption (ADR-027); selective
  `receive` + `(after ms …)` timeouts.
- **Types — set-theoretic gradual typing** (ADR-078 and follow-ons). Function
  arrows, element/parametric types, structural combinators, narrowing, singleton/
  literal types, map K/V, records/shapes, tuples; the sound half of local inference;
  `(sig …)` contracts + `BROOD_CONTRACTS=1`; the full-soundness-vs-hot-reload
  mechanism (re-check per reload, ADR-123/124/125). LSP tiers 0–2 + a
  dev-ergonomics pass.
- **Execution — closure-compiling VM + tier-1 JIT.** The VM is the default engine
  (ADR-076); a Cranelift template JIT (ADR-101) is a default cargo feature (integer
  arithmetic, fused Prim2, hot-reload epoch guard, in-native inline caches).
- **M2 — editor data model.** Rope substrate (ADR-045); buffer model;
  buffers-as-values; evaluate-the-Lisp-I'm-editing; per-process memory reclamation.
  Collaboration seam (2026-07): buffer *processes* with subscriptions + versioned
  delta pushes, edit-surviving markers (presence cursors ride them, pid-keyed
  cleanup on subscriber death), structured `buffer-splice`/`buffer-marker-move`,
  and concurrent-splice **transforms** (`splice-transform` — exact merges for
  disjoint edits, no CRDT) — what a downstream editor's multiplayer editing runs on.
- **M3 — display protocol + native frontend.** Serialisable render-op protocol
  (ADR-046); input events; in-process terminal frontend; per-op/per-window fonts
  (ADR-079); `nest observe` (inline + remote, ADR-053); telemetry core
  (`std/telemetry.blsp`, ADR-106); resilient `ui-run`.
- **M4 — server / daemon mode.** TCP sockets (ADR-062); TLS *client*/HTTPS;
  distributed nodes (`name@host`, cookies, encryption ADR-089, dual-listen, mesh
  join); userland supervision + a real `gen_server`; an ETS-style in-memory table
  store; `std/task`. `std/editor/serve` (ADR-090): the daemon/emacsclient seam —
  per-client and shared sessions, attach identity, async event pass-through,
  `serve-stop` — plus the exit-signal hardening it forced (ADR-132; pid identity
  across `node-start`).

Runtime housekeeping still open:

- ⬜ **Tracing GC for mid-eval / never-returning loops.** Arena-reset at top-level
  boundaries shipped (ADR-016); a general tracing collector still needs scannable
  roots (coupled with the explicit-value-stack VM step). **[kernel]**, sizable.
- ⬜ **Work-stealing scheduler.** Gated on the `Send` per-process heaps + tracing
  GC above; the root-cause of the earlier scheduler race and the invariants any
  reintroduction must honour are in [`docs/concurrency-v2.md`](docs/concurrency-v2.md).

---

## What's next — by area

### Language core & types

- ⬜ **Merely-wider inference case** — a body typed exactly `number` (int ∪ float)
  declared `int`, e.g. `(/ x 2)`; can't be pinned without occurrence/range analysis
  and flagging it would false-positive on int-valued runs (ADR-011).
- ⬜ **Parameter-type inference from arbitrary body usage** across branches — needs
  guard-aware dominance analysis; stays out until false-positive-clean (ADR-011).
  (The sound tiers of `infer_sig` already ship.)
- ⬜ **First-class set kernel piece** — a `#{…}` reader literal + printing + a
  distinct `set?`/`Tag::Set`; the `(require 'set)` library already shipped (ADR-060).
- ⬜ **Unbounded stream generation** (`iterate` / infinite producers) — lazy
  seq-view fusion already shipped (ADR-111); picks up when an editor feature needs it.
- 🟡 **`std/` curation + frameworks sequencing** (ADR-085/097) — `std/` curated and
  hierarchical module names shipped; the model is batteries-included (frameworks ship
  in the default install, not fetched). ⬜ Next: a future GUI framework ships bundled
  too; gated on the first real consumer.
- ⬜ **Native interop — WASM components** (ADR-071, [`docs/interop.md`](docs/interop.md))
  — a package ships native code as a `:native` WASM component built from source at
  fetch time, hash-pinned in the lock, cached under `_deps/`, instantiated sandboxed
  via embedded `wasmtime`; a `use-native` macro (WIT-driven) binds exports. Needs the
  package manager and the M4 blocking-offload pool first; realistically lands during
  M2+ editor-plugin work.

### VM & JIT

- ⬜ **Fix the `let`-self-ref `send` divergence** — a VM `let`-self-ref closure
  isn't structurally self-referential, so `send` accepts it where the tree-walker
  rejects it (a correctness gap + differential blind spot).
- ⬜ **Route remaining native higher-order callbacks** (`try`/`binding`/`apply`/
  `isolate`) through the VM like `%range-reduce` — blocked on the fix above.
- ⬜ **JIT Stage 4 — RUNTIME compaction survival** (ADR-091) — a constant-pool
  indirection table (ADR-096 §4.C) lets `runtime_collect` rewrite handles without
  invalidating machine code.
- ⬜ **Leaf-callee inlining** (the real call-heavy lever) — splice a small
  non-recursive callee's body into the caller so `(add1 n)` in a hot loop needs no
  call/frame/dispatch. Infra (`shift_slots`/`build_inlined_body`) exists; hot-reload
  safety is free via `compile_epoch`. Measure-first behind `BROOD_JIT_LEAF_INLINE`;
  a fresh focused effort ([`docs/jit-tier2.md`](docs/jit-tier2.md) §7).
- ⬜ **Layer-2 computed-goto dispatch** (`std::arch::asm!`, x86-64, `#[cfg]`-gated,
  pure-Rust fallback) — only if profiling still shows dispatch overhead. Additive.
- ⬜ **Heap-walking benchmark gap** — `bintree`/`nqueens` run interpreted (~39×/187×
  behind Elixir); structure-walking bodies bail the JIT subset. Gated on the
  allocation-elimination work ([`docs/allocation-elimination.md`](docs/allocation-elimination.md));
  higher ceiling than the call-dispatch levers — profile before the next JIT push.

### Tooling & errors

- ⬜ **`nest format --changed`** — whole-tree `nest format` reformats untouched
  files; add a git-aware narrower scope.
- 🟡 **LSP** — tiers 0–2 ship; still next: incremental sync; range/delta semantic
  tokens; **finer finding spans** (arity/type findings anchor to the call head, not
  the offending argument — wants `Pos` threaded through `types/check.rs`'s walk); and
  a **create-missing-`defn`** code action.
- 🟡 **Errors that teach (LLM-native)** ([`docs/llm-native.md`](docs/llm-native.md))
  — first instances landed; more to do: reader-level hints for Clojure/Scheme syntax
  the lexer mis-parses (`(let ((a 1)) …)`, `#{…}`/`#(…)`), the
  `brood.explain-error`/`brood.find-pattern` MCP tools, an intent→idiom cookbook, and
  folding each new repeat mistake into the rule-of-three.
- ⬜ **MCP tooling** — a streaming/progress-notification tier for long-running tool
  output; exposing GC/process *traces* (not just snapshots); tightening the write
  sandbox against symlink escapes (a `canonicalize` primitive).

### Editor (M2) & display (M3)

- ⬜ **Major/minor modes** — how a buffer selects which keymaps are active.
- ⬜ **Mouse / resize input events** — deferred until a feature needs them.
- ⬜ **GPU-window frontend** — a later additive path speaking the same display
  protocol; arbitrary per-px buffer sizing rides with it.
- 🟡 **Telemetry** (ADR-106) — core landed; still to fold in: kernel-internal event
  *sources* (GC collections, scheduler spawn/exit/preempt, dist node up/down — a Rust
  emit seam); unifying `gc-stats`/`vm-stats`/`process-info` snapshots behind the
  stream so `nest observe` + `nest mcp` consume it; `defevent` + checker-validated
  event schemas; built-in aggregators (counter/gauge/summary/histogram) + sampling;
  and the location-transparent remote tier over the dist link.

### Server / daemon (M4)

- ⬜ **Inbound (server-side) TLS** — rustls streams don't split read/write across
  threads; plus a **`mio` reactor** for socket scale.
- ⬜ **OTP near-term** (additive, pure Brood or a thin dist seam, gated on a need):
  **`send-after`/`send-interval`** timers; a synchronous **`remote-spawn` returning
  the child pid** (makes cross-node supervision turnkey); a **`terminate`-style
  worker-cleanup convention** on `[:$stop]`.
- ⬜ **OTP deferred** (ADR-011, gated on a real consumer): **`gen_statem`** state
  machines; an Elixir-style **`Registry`**/via-tuples + **process groups (`pg`)**; an
  **`Application`** behaviour; **synchronous, ordered, rollback-on-failure** supervisor
  startup + per-child intensity counting + child `type`/`significant`/`auto_shutdown`
  metadata.
- ⬜ **Dist refinements** (ADR-011): exact propagated exit reason for a *non-trapping*
  linked peer (reports `:kill` today); a `terminate/2` hook on hard kill; **long-name
  FQDN resolution** (a long name is passed explicitly today, no resolver); Windows
  Unix-socket transport.

### Packaging & ecosystem

- 🟡 **Package manager** (ADR-037, [`docs/packages.md`](docs/packages.md)) — `:path`
  deps end-to-end ✅; ⬜ **`:git` deps** (slice 2); ⬜ **the verbs + auto-fetch**
  (slice 3). **[kernel]** primitives are tiny (`%git-*`/`%sha256`/`%http-get`).
- ⬜ **Single-binary bundling** (ADR-038) — `nest bundle` appends a zip of
  project + `_deps/` to a pre-built `brood`; deferred until the editor needs end-user
  distribution.
- ⬜ **`nest release`** — a self-extracting filesystem for runtime data files, a
  static-musl default, and `.deb`/`cargo install` packaging of the *runtime* (open
  until a real consumer needs it).
- 🟡 **tree-sitter grammar + GitHub recognition** — editor grammars (`nest grammar`,
  ADR-092), the `tree-sitter-brood` parser, and `brood-vscode` all ship; ⬜ **publish
  it** (editor bindings/CI) and file the ⬜ **`github/linguist` PR** (gated on `.blsp`
  adoption across many repos). Today a `.gitattributes` Clojure stopgap.

---

## Design notes (context for the above)

### Types — goal & the hot-reload constraint

The target is Elixir's sound, gating, whole-program checker for the *interior* of
code, kept compatible with Erlang-style hot reload for *globals and module
boundaries*. Globals stay `dynamic()` because hot reload rebinds them via `def`, so a
type proven at check time can be falsified by a later reload; what *can* be gated is
everything local — `let`/`fn`-param bindings, call arity and argument types, `match`
coverage, `sig!` contracts — while global `def`/`defn` types, inter-module flow, and
global-fn return types stay advisory. Slogan: *Elixir's checker for the interior,
Erlang's late binding for globals and module boundaries.* The full-soundness-vs-reload
mechanism has shipped (re-check per reload rather than prove once — ADR-123/124/125);
the "checking never rejects a runnable program" invariant in `CLAUDE.md` and
[`docs/types.md`](docs/types.md) contract #5 needs revising now that it has.

### Telemetry — what we improve over Erlang's `:telemetry`

Async-by-default delivery (handlers run off the emitting process); events as data
with a declared schema (`defevent`, checker-validated) rather than bare atoms; an
immutable, process-backed handler registry; location-transparency over the dist link;
and built-in metric aggregation (counter/gauge/summary/histogram) + sampling — folding
today's ad-hoc `gc-stats`/`vm-stats`/`process-info` instrumentation behind one stream.

---

## Cross-cutting open questions (revisit, don't build yet)

- **Shipping a runtime binary** — a self-extracting filesystem for data files,
  static-musl default, `.deb`/`cargo install` (see `nest release` above); open until a
  real consumer needs it.
- **Publishing the grammar** — the `github/linguist` PR isn't filable day-one; it's
  gated on `.blsp` adoption across hundreds of repos.

---

## Killed directions (don't retry)

- ❌ **Kernel-supervised processes** (ADR-039) — reverted 2026-05-29; it was the bulk
  of the multi-thread scheduler race surface. Userland supervision replaces it;
  named-spawn is intentionally not delivered in the kernel.
- ❌ **JIT Stage 3, Increment 2** (in-IR frame setup + `call_indirect`) — NO-GO,
  confirmed twice. The call-heavy win is leaf inlining, not cheapening the call itself.

---

## Out of scope (deferred, additive later)

- `&key` named arguments (designed — ADR-011) and supplied-p flags
- Hygienic macros / `macroexpand-all`
- Bignums / rationals (i64 + f64 is enough for now)
- True per-file **namespaces** — flat Emacs-style `provide`/`require` is in scope
  (ADR-019); real namespace isolation stays a later, additive Brood macro layer
- Characters as a distinct type (chars are 1-char strings)

---

## Guiding principles

1. **Policy in Brood, mechanism in Rust.** Prefer a primitive + a prelude macro over
   a new special form; write as much as possible in Brood itself.
2. **The frontend is a protocol.** The display seam is serialisable render-ops, so a
   terminal, a GPU window, or a remote client are all just consumers.
3. **Every milestone is usable on its own** — the language stands without the editor,
   the editor without the server.
