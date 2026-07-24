# ROADMAP

Brood is a general-purpose **language and runtime** — born as the substrate
for a modern, Emacs-like, self-editing editor, grown past that intent into a
language in its own right. This repo is all of it: the language core, the
runtime, the standard library, and the `std/editor/*` framework for
interactive applications. The foundation came in milestones (M1–M4), each
shippable and useful on its own.

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

### Structural / code-organization cleanup (2026-07-24)

Findings from a full-repo structural review (9 parallel reviewers over all of
`crates/` + `std/`). **Verdict: the codebase is well-structured** — coherent
module boundaries, disciplined naming (zero `do_`/`normalize_`), no dead-code
piles / commented-out blocks / TODO rot, clean feature-gating, the two-parser and
tiered-evaluator hazards both *managed*. These are "sharpen a good thing" items,
none urgent. Ranked by payoff. All **[kernel]** unless marked.

**Tier 1 — maintainability hazards (real payoff):**

1. 🟡 **Split `eval/compile/jit_lower.rs` (5437 → 4576)** — partial, done
   2026-07-24. Extracted the self-contained **unboxed i64/f64 scalar worker**
   (`Scalar`/`I64Ctx`, the `i64_*`/`lower_i64_*` family, and `jit_lower_i64_arm`,
   ~860 lines) — a cluster used only by the `jit_lower_arm` dispatcher, never by
   `jit_lower_arm_inner` — into `eval/compile/jit_lower/i64.rs`, re-exported so the
   tiering glue's `jit_lower::…` paths are unchanged. Behaviour-identical (a
   relocation of independent fns). ⬜ The **`jit_lower_arm_inner` decomposition**
   (the ~2,185-line function → `Call`/`Prim`/`SelfCall`/control helpers threading
   the Cranelift lowering state) is the high-risk core and stays open — it needs a
   dedicated pass with JIT-differential + `BROOD_JIT_DUMP_IR` + benchmark
   verification, not a mechanical extraction. Verified: differential 2/2, jit
   34/34, compaction 3/3 (all under `JIT_VERIFY`); suite 3057/3057; both feature
   configs compile.
2. 🟡 **Split `process/scheduler.rs` (2080 → 1824)** — partial, done 2026-07-24.
   Investigation found the roadmap's clean preempt/pool/lifecycle carve isn't
   achievable cheaply: the **reduction budget** (`REDUCTIONS`/`reduction_budget`)
   is shared across the preemption core, the capture-driver glue, *and* the worker
   loop, so a full carve needs accessor-fn refactors through the KI-1 concurrency
   code. Took the low-risk win instead: extracted the self-contained execution
   guards — GC-block/macro-block depth + RAII guards and the stack-overflow byte
   guard — into `process/scheduler/guards.rs`, re-exported so `scheduler::…`/
   `process::…` paths are unchanged. Then (2026-07-24) extended it: the shared
   scheduling state (run queue, worker pool, pid/parent tables, counters) stays in
   the root, but the **process lifecycle** — `spawn`/`spawn_linked`/`spawn_impl`/
   `spawn_root_program`, `exit`/`exit_propagate`/`exit_with`, `deregister`,
   `proc_descr` — moved to `process/scheduler/lifecycle.rs` (statics-in-root, so a
   pure relocation reached via `use super::*`; public surface re-exported). Root
   2080 → ~1400. ⬜ The remaining **worker-pool** machinery (queue/stealing/worker
   loop) stays in the root; carving it would need the accessor layer for the shared
   reduction budget and is deferred (not worth the concurrency risk yet).
3. ✅ **Split `core/heap/gc.rs` (4520 → 2689)** — done 2026-07-24. Extracted the
   RUNTIME shared-region collector (ADR-091 — two-generation aging, single-process
   compaction, node-liveness drain, live-globals migration + the `RuntimeForward`/
   `flush_rt_*` helpers, ~1830 lines) to `heap/gc_runtime.rs`. The two regions were
   self-contained (no cross-calls into the LOCAL collector), so behaviour-identical.
   Verified: 21 heap tests under `GC_STRESS`+`GC_VERIFY`; suite green.
4. ✅ **`register()`/`PRIMITIVE_DOCS` drift guard** (`builtins/mod.rs`) — done
   2026-07-24. Added a unit test that registers every primitive into a fresh env,
   enumerates the natives, and asserts every **user-facing** (non-`%`) primitive
   has a `PRIMITIVE_DOCS` entry and no doc entry is an orphan. Surfaced 12
   genuinely-undocumented user-facing primitives (`bytes`/`byte-at`/`byte-length`/
   `bytes->list`/`bytes-concat`/`bytes-index-of`/`subbytes`, `max`/`min`,
   `current-ns`/`seqview?`/`demonitor-node`) — docs added. `%`-prefixed internal
   ops stay exempt (wrapped by prelude fns/macros). **[kernel]**

**Tier 2 — real duplication to dedupe: 5/6/7/9 done 2026-07-24 (suite 2985/2985,
`nest check` zero-warning, 238 type-lattice Rust tests green). 8 deferred with
rationale (below).**

5. ✅ **`types/mod.rs` 4-way literal-refinement copy-paste** — the per-kind blocks
   in `union`/`intersect`/`is_subtype`/`is_disjoint`/`negate` now go through four
   generic helpers (`merge_union_lit_set`/`intersect_lit_set`/`lit_is_subtype`/
   `lit_disjoint`, `T: Ord + Clone`), and all ten `Ty` constructors use
   struct-update over `Ty::flat(tags)`. ~250 lines removed; behaviour identical.
6. ✅ **`lib.rs` `eval_str`/`eval_source` near-dups** — factored the private
   `eval_forms(Vec<(Value, Option<Pos>)>)` core carrying the load-bearing
   GC-rooting/namespace/reset logic once; the two public fns are now 3-line
   adapters. The restore now runs exactly once on every path (was mirrored).
7. ✅ **`std/` path + url duplication** **[Brood]** — **url:** `net/http`'s
   `parse-url` now wraps `url/parse-url` (the one RFC-3986 parser) + HTTP defaults,
   instead of a lossy reimpl. **path:** resolved as *not* true duplication — a
   deliberate keep-both. `path.blsp` is the full public path API; the prelude
   `path-*` subset is the necessary bootstrap layer (modules aren't loadable at
   boot, and the two have different contracts — `path/basename` strips a trailing
   slash, `path-basename` doesn't; `path/join` is variadic with absolute-reset,
   `path-join` is 2-arg). Documented as such in both files.
8. ⬜ **`gui.rs` ↔ `gui_gpu.rs`** — **deferred:** `gui_gpu.rs` is a declared
   *prototype* (solid quads; "Cursor / zones: not drawn in the GPU prototype"), so
   the missing `Cursor`/`ScrollRegion`/underline are *unimplemented GPU features*,
   not diverged geometry. Making the GPU path draw them (ScrollRegion op-reprocess,
   GPU text underline) is feature work needing a live display to verify — out of
   scope for a mechanical cleanup pass.
9. ✅ **`eval/compile/inline.rs` `node_*` predicate family** — collapsed
   `node_has_selfcall`/`node_has_self_call`/`node_has_make_closure` into one
   generic `node_any(node, &pred)` combinator.

**Tier 3 — quick wins (verified, safe/mechanical): ✅ all done 2026-07-24.**
(Suite 2979/2979 green; built with gui+treesit-grammars+jit.)

- ✅ Deleted dead `parse_jobs_args` (`cli_support.rs`) and `Scanner::set_pos`
  (`syntax/scanner.rs`) — both had zero callers.
- ✅ Fixed doc-comment misattachments: the crash-dump doc now sits on
  `install_crash_dump` (was on `fmt_utc_ms`); `syntax/cst.rs` string paragraph
  moved onto `fn string`; the orphaned `mailbox-size` doc removed from
  `terminal.rs` (the primitive is documented in `PRIMITIVE_DOCS` + `mailbox.rs`).
- ✅ Fixed stale/misleading headers: `builtins/io.rs` "terminal frontend" banner
  → "process introspection"; corrected path comments in `std/editor/ansi.blsp`
  and `std/net/http.blsp`. **[Brood]**
- ✅ `std/` consistency **[Brood]**: added `scaffold`'s `defmodule` docstring;
  renamed the `treesit` module to `editor/treesit` (+ its callers); moved
  `agent.blsp` → `std/proc/agent.blsp` and renamed the module to `proc/agent`
  (+ registration, benches, tests).

Also en route: fixed a broken build — `heap::stall_guard` was referenced by
`gui.rs` but not re-exported after the `heap/gc.rs` split (only `stall_guard_pid`
was); added it to the `pub(crate) use self::gc::{…}` list.

**Tier 4 — policy-in-Rust notes (judgment calls, "Rust=mechanism, Brood=policy"):**

- ⬜ `gui.rs` hardcodes UI policy — Catppuccin colors duplicated with `theme.blsp`
  (drift-prone), a named-color palette `face.blsp` could own, a kinetic-scroll
  physics model.
- ⬜ `builtins/io.rs` (1929) is a 13-concern grab-bag — split crypto+hashing and
  the package-manager git/tar mechanism; transcendental math is misfiled in
  `sequences.rs` (belongs in `numeric.rs`).
- ⬜ `nest`'s `cmd_run` (217 lines) carries more policy in Rust than the other thin
  subcommand handlers — candidate to push into a Brood `project/run` entry.

### Runtime-feature parity program — BEAM / .NET / Node (2026-07-22)

The distilled, ranked program for closing the remaining runtime *feature* gaps
against the peer runtimes, from the 2026-07-11 capability audit plus the
2026-07-18 robustness survey below (most of that survey's ranked items are now
closed). The architecture is at/above parity already — scheduling, isolation,
per-process GC, distribution, hot reload; live continuation migration,
encrypted-by-default dist, and OSR exceed BEAM — and cached-boot startup
(~6.5 ms, ADR-138) beats Node (~17 ms). What remains, by leverage:

**Tier 1 — scheduled, in order:**

1. ✅ **Binary pattern matching / bit syntax + the parser port** — BEAM's
   flagship remaining capability, both halves shipped 2026-07-22.
   **The pattern (ADR-140), pure Brood:** the byte-granular `(bytes seg…)`
   pattern gained typed integer segments — `(x :u16)`/`(x :i32-le)`/`(_ :u32)`,
   u/i × 8/16/32/64 × be/le, big-endian default — lowered onto new prelude
   reads `bytes-uint`/`-le`/`bytes-int`/`-le` + encoders `int->bytes`/`-le`;
   `:u64` past `i64` auto-widens to a big integer (exact Erlang semantics).
   Sub-byte widths / float / UTF-8 segments deferred (ADR-140).
   **The parser port (ADR-141):** binary mode is now inbound-only — send
   string leaves are ALWAYS UTF-8, the Latin-1 carrier send rule is deleted —
   and `std/net` is bytes-native end to end (server sockets binary-for-life,
   no flip-back race; client responses byte-faithful `bytes` + `body-text`;
   `tcp-drain` returns `bytes`; SSE deliberately stays text-mode). Remaining
   seam: `tls-request` is string-typed both ways — rides item 3.
   **[Brood + a kernel rule deletion]**
2. ✅ **Growable read buffer — resolved by NOT building it (ADR-142,
   2026-07-22).** A mutable buffer value is a transient (forbidden, ADR-026),
   and the chunk-list + `bytes-concat`-once idiom is already O(n) in copies;
   what was still quadratic was the head reader's per-chunk rescan — fixed
   with an incremental `bytes-index-of :from` scan + a 64 KiB head cap
   (slow-loris guards, `std/net/http.blsp`). **[Brood]**
3. ✅ **`mio` reactor + TLS everywhere — shipped 2026-07-22 (ADR-143).** One
   reactor thread multiplexes every socket (plaintext, TLS client+server,
   listeners), replacing thread-per-socket; same mailbox contract. `tcp-send`
   is queued with drain-before-close (the truncation footgun is gone; 16 MiB
   cap bounds slow readers); TLS streams honor `tcp-set-binary`; `tls-request`
   takes iolists + an optional `ca-pem` trust anchor (private CAs — and the
   first in-tree e2e TLS tests, `tests/tls_test.blsp`); `http-get`/`post`
   accept `:ca` and are byte-faithful over https; `serve-loop` serves https
   unchanged when handed a `tls-listen` socket. **[kernel]**
4. ✅ **Dirty-CPU offload pool — shipped 2026-07-22 (ADR-144).** `%offload`
   runs an allow-listed blocking native (git/kdf/digest/file-IO/keygen) on a
   small OS pool via the ADR-059 copy-out → message-back seam; the prelude
   `offload` wrapper parks the caller in a selective receive — a process
   waits, never a worker. The package manager's clones/ls-remotes ride it.
   Opens the ADR-071 WASM-interop gate. Deferred: BEAM-style process
   *migration* to dirty schedulers (for heap-sharing natives) until a real
   consumer needs it. **[kernel mechanism, Brood policy]**

**Tier 1 is complete** (2026-07-22): bit syntax + the parser port, the
read-buffer non-build, the socket reactor with TLS everywhere, and the
offload pool all landed the same day.

**Tier 2 — real gaps, each gated on a first consumer (ADR-011):** the cluster
**registry** (`Registry`/via-tuples, `:global`, `pg` — "OTP deferred" below);
**mailbox bounds / backpressure** (survey item below); the **observability
remainder** (aggregators + node up/down done 2026-07-24; still `defevent` schemas,
the remote tier, `nest observe`/`nest mcp` consuming the stream — "Telemetry" below);
**`gen_statem`** and an **`Application` behaviour**.

**Tier 3 — cheap ergonomic parity:** a **grapheme-correct string API**
(codepoint-vs-grapheme indexing is a real divergence vs Elixir's `String`;
`unicode-segmentation` is already a dep, wired only to display-width);
**protocols/multimethods** (replace hand-written `type-of` cascades);
**string interpolation**; **`&key` args** (designed — ADR-011); the dist
**`terminate/2` hook** + **FQDN long names** (dist refinements below). (The
parked-`receive` mailbox-slot leak that used to sit here was fixed
2026-07-23 — survey housekeeping above.)

**Explicitly no work:** the JSON/base64 native-codec rows (by-design pure
Brood vs C codecs); and the residual message-latency gap vs BEAM (~3–6×) is
*performance*, not features — its deep lever (inline receive compilation) is
tracked under the Elixir-parity gaps below.

### Robustness gaps vs BEAM / .NET (2026-07-18 runtime survey)

A structured survey of the runtime against Erlang/BEAM and the .NET CLR
(scheduler, fault isolation, GC/JIT, diagnostics — code-verified with file
refs). The shape that emerged: Brood is **architecturally BEAM-class already**
(per-process generational GC, reduction preemption with JIT back-edge batching,
links/monitors/`trap-exit`, selective receive, real OSR — which BeamAsm doesn't
have). What remains are targeted gaps, ranked here by leverage. Each keeps the
mechanism/policy split: kernel primitive, Brood policy.

- ✅ **Stack traces in error values — the biggest debuggability gap.** Shipped
  2026-07-18. Every `LispError` now accumulates a `trace` as the raise unwinds:
  the VM walks its live `BcFrame`s (a caller's `code[ip-1]` is the `Inst::Call`
  with the call-site pos; arms carry `fn_name`/`src_file`), the tree-walker
  attaches one entry per eval frame that entered a closure (tail entries rename
  the frame, first entry keeps the call site — matching the VM's frame reuse
  exactly; `apply_closure` seeds the tracker for native-boundary callbacks).
  Caught kernel errors surface it as `:trace` — innermost-first
  `{:fn [:file :line :col]}` maps, capped at 32 — and uncaught errors print
  `at fn (file:line:col)` lines (CLI + REPL). En route the ADR-135 program-exit
  seam was upgraded from a flattened string to the structured error, so file
  runs now render the caret/hint/trace they previously lost. Engines
  agree (error_format_parity extended by the suppression of information-free
  synthetic frames); JIT'd arms trace via their deopt re-raise. The follow-up
  also shipped same day: **process death reasons carry the structured error**
  — an uncaught error retires the process with `[:error {:kind :message …
  :trace}]` (`message::error_reason`, heap-independent so it deep-copies to
  monitors/links and crosses the dist wire), BEAM's `{Reason, Stacktrace}`
  parity for supervisors.
- 🟡 **Per-process resource limits (BEAM `max_heap_size` / mailbox bounds).**
  Lever (1) shipped 2026-07-18: **`(process-flag :max-heap n)`** (Erlang
  `process_flag/2` shape; positive int sets, nil clears, absent reads, returns
  previous). Mechanism: `Heap::proc_mem_limit` checked at the end of both
  collection paths against the *live* (post-GC nursery+old) footprint — a
  sticky flag the eval/VM safepoints raise as a catchable `E0045` **in that
  process only** (uncaught → kills just the offender; the ADR-043 hard cap
  still aborts the whole OS process). Policy is Brood: set the flag first
  thing in the spawned fn. Tests `tests/process_limit_test.blsp` (7 cases,
  green on VM/TW/no-JIT/GC_STRESS). ⬜ Remaining lever (2): optional mailbox
  bounds — a `send` to a full mailbox drops/errors by policy (accounting
  exists: `process-info` `:mailbox`); deferred per ADR-011 until a concrete
  consumer picks the policy (drop vs error vs park has real design surface,
  incl. remote delivery which can't error the sender). **[kernel mechanism,
  Brood policy]**
- ✅ **Startup image snapshot (ReadyToRun / `.beam` analogue).** Shipped
  2026-07-19 as the **expanded-prelude boot cache** (ADR-138). Cold start
  re-parsed + re-expanded the prelude every run (~31 ms, of which
  macro-expansion was ~27 ms — 744 expander invocations of genuine Brood work,
  already VM-run; see the 2026-07-19 devlog measurements). The fix: the source
  boot prints each post-`compile` (expanded + resolved) prelude form to
  `~/.cache/brood/prelude-expanded-<hash>.blsp`, keyed by `build-id` (the
  ADR-129 staleness key — the prelude is `include_str!`'d, so any binary
  change invalidates), and the next boot reads those forms and skips
  `eval::macros::compile` entirely. **Measured: ~38 ms source boot → ~6.5 ms
  cache hit** — single-digit-ms target met with no binary heap format (freeze
  is only 0.7 ms, so full `SharedCode` serialization stays unnecessary). The
  design-care items both handled: the raw prelude is still read positioned so
  `note_definition`/LSP `M-.` are identical on both paths, and the caching
  boot's final gensym counter is stored in the header + floored at cache boot
  (`gensym_floor`) so runtime gensyms can't collide with cached expansions.
  Per-form print→read→print fixpoint check gates writing (an unprintable form
  poisons the cache and the source boot just runs); any read/eval failure
  deletes the file and falls back. `BROOD_NO_BOOT_CACHE=1` opts out.
  **[kernel]**
- 🟡 **Observability: timing tier + trace pipeline + profiler.** Slice 1
  shipped 2026-07-18 — the survey's two named holes are closed: **GC pause
  durations** (`gc-stats` `:pause-total-us`/`:pause-max-us`/`:pause-last-us`,
  timed around `collect`), **scheduler counters** (`(sched-stats)` —
  spawned/exited/preempts/steals/migrations/workers/peak), and the **sampling
  CPU profiler** (`profile-start`/`profile-stop`): an epoch ticker + a
  frame-boundary probe in `vm_run_bc` that records each process's reified
  named-frame stack into a histogram — no signals, one relaxed load per frame
  boundary when off (exactly what the state-capture rewrite made possible).
  JIT-resident loops attribute at their quantum preempt; the tree-walker isn't
  sampled. `tests/observability_test.blsp`. Slice 2 shipped 2026-07-19 — the
  **kernel event stream** (ADR-137): `(system-monitor pid opts)` pushes
  `:gc`/`:spawn`/`:exit`/`:deopt` events to one subscriber as
  `[:system kind subject-pid detail]` messages (BEAM `system_monitor` shape,
  `:gc-min-pause-us` = `long_gc`); `telemetry/watch-runtime` re-emits them as
  `[:runtime kind]` telemetry events, so runtime + app events share the
  ADR-106 attach seam. `tests/sysmon_test.blsp`. Slice 3 (2026-07-24): the
  **aggregators** landed — counter/sum/gauge/summary/`sample-every`, then the
  **distribution/histogram** aggregator + `metric-percentile`, and **node up/down**
  folded into the `[:runtime kind]` stream via `watch-nodes` (poll-and-diff `(nodes)`;
  see "Telemetry" under M3). ⬜ Remaining: `defevent` schemas, the
  `nest observe`/`nest mcp` consumers (unify the snapshot builtins behind the stream),
  and the remote tier. **[kernel sources, Brood aggregation]**
- ✅ **Distribution self-healing: auto-reconnect + backoff.** Shipped
  2026-07-18. Brood policy: **`std/net/reconnect`** — a named, idempotent
  watcher process per node spec that connects, arms `monitor-node`, and on
  `[:nodedown]` retries `(connect spec)` with exponential backoff
  (`:min-ms`/`:max-ms`), re-arming + notifying subscribers `[:nodeup name]`.
  Kernel seam: `route` reports link-missing and `send` raises a catchable
  **E0060 noconnection** when the sender opted in via
  `(process-flag :send-errors true)` (queue-and-retry instead of silent drop;
  process liveness stays Erlang-silent). End-to-end test
  `reconnect_watcher_heals_a_fallen_link` (down → raise → heal → message
  flows). The cluster **global registry / `pg`** stays in "OTP deferred"
  below (gated on a real consumer). **[Brood + kernel seam]**
- ✅ **Dirty-CPU accounting for long native builtins.** Shipped 2026-07-18 as
  a **`BROOD_STALL_MS`-armed diagnostic** (revised same day by measurement):
  when the stall tracer is armed, `call_native` times each builtin in a green
  process, `scheduler::charge_native` charges the elapsed time against the
  reduction budget (~2 reductions/µs, the BEAM NIF model; ≥~1 ms drains the
  quantum), and a trip **names the builtin** (`[stall] native %range-reduce
  took 766ms`). Always-on per-call charging was tried first and **rolled
  back**: the A/B measured **8–22% on the message-heavy rows**
  (pingpong/ring/json — two `Instant::now` per native call) while buying
  almost nothing, because reduction preemption already bounds post-native
  hogging to ~one quantum (~1 ms); the un-preemptible time *inside* a long
  native is only fixed by the offload pool — ✅ **shipped 2026-07-22 (ADR-144,
  the parity program's item 4)**. This item is complete. **[kernel]**
- ✅ **Housekeeping found by the survey — all closed:** permanently-parked
  `receive` waiters no longer leak in an embedded host (fixed 2026-07-23:
  `Interp::drop` runs `shutdown_runtime_parked`, which routes each of the
  dropped runtime's parked waiters through the normal death path — pinned by
  `crates/lisp/tests/interp_teardown.rs`, including the
  other-runtimes-untouched case). **Deep-structure hardening of the recursive
  heap walkers** fixed 2026-07-20 (`stacker::maybe_grow` segmented growth in
  promote/GC-flush/equal/hash; `tests/deep_values_test.blsp` pins 20k–60k
  depth), and extended to the CODE walkers (expander/resolver/checker) on
  2026-07-23. **[kernel]** ✅ Exit-signal propagation fixed (2026-07-18):
  kill **hardness** is now a request property separate from the reason
  (`MailboxState.kill_hard`), so link propagation stays hard (dies at the next
  reduction tick) but carries the **originating reason** — a cascading death
  reports why the tree fell (`[:error {… :trace}]` end to end), BEAM
  semantics; the sticky-latch guarantee keys on hardness. **[kernel]**

### Findings from hatch (2026-07-11)

Three runtime/language items surfaced while eliminating a whole *class* of O(n²)
bugs in [`hatch`](../hatch) (the Brood web framework). Every one was the same
shape — `(str acc x)` / `(bytes-concat acc x)` accumulated in a per-read loop,
quadratic in the read count — and every fix was the same manual idiom (cons onto a
list, `reverse` + `join` once), written five times across the HTTP/WebSocket stack
(body drain, head reader, chunked de-chunk, WS reassembly, live-view render). These
would retire the bug class at the language level. See hatch's
[`docs/tcp-http-audit.md`](../hatch/docs/tcp-http-audit.md) §16–§17.

- ✅ **Iolists — the highest-leverage one. Shipped 2026-07-19 (ADR-139).**
  `tcp-send`/`proc-send`/`spit`/`spit-append`/`spit-bytes`/`append-bytes`/
  `bytes-concat` accept arbitrarily nested string/bytes/byte-int trees
  (`[status-line headers "\r\n\r\n" body]`), flattened exactly once at the
  write by one shared iterative walker — the Erlang model; immutability means
  no cycles, so termination is structural. Additive (all previously rejected
  lists). The ADR-139 Latin-1-per-string-leaf clause for binary-mode sockets
  was superseded 2026-07-22 (ADR-141: string leaves are always UTF-8).
  `str`/`join` deliberately stay display-rendering (see the ADR) — an
  explicit in-memory materialiser beyond `bytes-concat` is a future call.
  The `std/net` response builders were ported onto iolist sends 2026-07-19.
- ✅ **`bytes`-native HTTP parsing (the carrier-string bridge is dead).**
  Shipped 2026-07-22 (ADR-141) — see the parity program above: `std/net` reads
  and parses `bytes` end to end, the Latin-1 carrier send rule is deleted from
  the kernel, and the text/binary mode-flip races are structurally gone. The
  WebSocket half lives downstream in hatch (no WS in this repo) — its port
  onto `bytes` + bit syntax is hatch work, now unblocked. **[done]**
- ⬜ **A growable read buffer (or `bytes` transient).** The input-side twin of
  iolists: an append buffer that freezes to immutable `bytes` on read would make the
  request head reader, chunked drain, and WS frame gather trivially O(n) — no manual
  list+`join`, no length-drain gymnastics. **[kernel]** a transient/builder value +
  freeze.
- ✅ Smaller ergonomic wins — all closed: **`mapv`/`filterv`** shipped
  2026-07-18 (prelude one-liners over `into`; `tests/sequence_test.blsp`);
  **`let` vector-destructure of a list value** verified 2026-07-18 to raise a
  clean `[:match-error :let …]`; and the **`foo--private` convention** went
  past "link-checked" to **enforced module privacy** (2026-07-23, ADR-146):
  a cross-module qualified private reference is a compile error at load,
  `(:use-internals mod)` is the explicit test/tooling grant, top-level/REPL
  stays unrestricted, and a module's macros may expand to its own privates.
  14 genuinely-shared helpers were promoted to public API en route.

### Elixir-parity performance gaps (2026-07-12, refreshed 2026-07-18)

Benchmarked brood ÷ **Elixir** per row (`../brood-benchmarks`). Elixir is *also*
immutable + GC'd + boxed-float + actor-based, so **every gap here is an
implementation deficiency, not an "immutability tax"** — the bar is "match an
immutable peer," and BEAM proves each is reachable. Ranked by ratio; `[kernel]`
unless noted.

**The 2026-07-13 priority set — the four rows where Brood was *last of 7
languages* (`nbody`, `regex`, `sieve`, `persistent-map`) — is now cleared
(2026-07-17):** nbody left 7/7 with the `fsqrt` inline (2026-07-15), sieve is
3/7 after the dense-Table work (2026-07-16), persistent-map is 6/7 (2026-07-15),
and regex left 7/7 at ~92 ms compute, past Clojure (2026-07-17). The remaining
open rows are `nqueens`, `ring`/`pingpong`, `bintree`, and `loop`.
`json`/`base64` stay excluded as gaps (Elixir/Node win them with native C codecs
against our by-design pure-Brood code — a separate, lower-priority pure-Brood-codec
track); `base64` is the residual coin-flip last place.

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

  **OFF 7/7 (2026-07-15):** lever (2) shipped — `sqrt` inlines as Cranelift `fsqrt`
  (0.74 → 0.54 s, "kills the last coin-flip 7/7"), and the closure-arm
  call-profitability gate + deopt feedback took another −28% (2026-07-16). Still
  ~a few × Elixir; the remaining levers: **(1)** the residual `advance-body`
  deopts — no float *param*, so the `has_float_slot` gate misses it; catching it
  needs a float-context signal that survives `(nth …)`/call-return type erasure
  (cross-arm return typing, or a compile-time float-global check for `dt`)
  *without* regressing int-vector arms; **(3)** cut the `global_ic_miss` on
  `dt`/`sm` reads in call-mediated arms. **Layer B (typed cross-arm float ABI)
  is deprioritised** — the hot calls have no float *args*. The next big general
  win is **full float type-specialization** (profile-drive an arm's float
  slots/stack so vector-read floats stay unboxed everywhere, covering
  advance-body too). **[kernel/JIT + benchmark]**
- ✅ **`regex` — was ~62× vs Elixir (981ms), 7/7 (interpreted CPS backtracker);
  OFF 7/7 (2026-07-17): ~92 ms compute, past Clojure (103 ms) — and it stayed
  pure-Brood.** Lever (a) shipped first: the AST compiles to a **lazy DFA**
  (closure-free state table + flat step loop; catastrophic patterns now linear;
  1.03 → 0.69 s, 2026-07-14), then `re:compile` discipline + the JIT learning
  keyword `=` (2026-07-15), dropping a dead `(:use editor/buffer)` (578 → ~301 ms
  wall, RSS 182 → 65 MB), and the 2026-07-17 round: memo-cache split (hot object
  out of the deep-cloning Table read), a 6-slot vector hot object, and fixing the
  **self-tail arg-position-`if` deopt storm** (a lazy `Op::Slot` materialised as
  an int-guarded payload at the block boundary — the regex loops now bind the
  branch in a `let`). Engine follow-up recorded: per-leader stack-shape analysis
  would make the natural nested-`if` style equally fast; until then any self-tail
  loop threading an opaque value through an arg-position branch hits this cliff.
  A native regex primitive stayed out — the dogfood-correct fix won. **[std/Brood]**
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
- ✅ **`sieve` — was ~19× vs Elixir (1.0s), 7/7 (Table op overhead); now 3/7
  (2026-07-16, ~0.06 s — at Clojure's heels).** The levers landed as a
  Table-general series (every Table user benefits, `Table` stays the one
  sanctioned mutable): **dense int-keyed Table storage + fused table-op prims**
  (0.88 → 0.15 s, 2026-07-15), a **lock-free registry + fast scalar hash**
  (2026-07-15), the lock-free dense store + resume-tier fix (sieve −33%, loop
  −75%, 2026-07-16), and the **JIT inlining dense table ops** (0.10 → 0.06 s,
  4/7 → 3/7, 2026-07-16). No bitset primitive needed. **[kernel]**
- ⬜ **`nqueens` — was 15× (backtracking allocation); −31% from routing closure
  arms through the call-profitability gate + deopt feedback (2026-07-16);
  re-measure the ratio.** Residual: list/closure allocation per branch; overlaps
  the HOF-fold and allocation paths (see
  [`docs/compute-frontier.md`](docs/compute-frontier.md)). **[kernel]**
- ✅ **`ackermann` — was 14× (non-tail double recursion), FIXED (`f90910c`, 2026-07-13):
  4.02 → 0.36s, 7/7 → 3/7.** The i64 unboxed worker's subset checker only matched *non-tail*
  self-calls (fib's arg-position recursion); `ack`'s recursion is in *tail* position
  (`SelfCall`), and its native-recursion depth cap was a stale 1400 (< `ack`'s ~4093 depth).
  Taught the subset about tail self-calls + raised the cap to 32768. Now 3rd, past
  Node/Clojure/Ruby/Python. **[kernel/JIT]**
- ⬜ **`ring` / `pingpong` — residual message machinery.** Already cut from ~13×
  (ADR-135 top-level-as-green-process, 6.5 → 3.3 µs/RT + wake elision), then
  closure arms shared behind an `Arc` (ring 2.02 → 1.50 s, pingpong ~18%,
  2026-07-13) and closure-template caching (2026-07-11), then the mailbox
  mutex trimmed to ONE acquisition per matched message (was three: peek+copy
  under lock, remove, deadline-clear — optimistic first-candidate pop +
  copy-outside-lock; pingpong ~2–4%, 2026-07-19); `type-of` became a compiled
  prim (matcher dispatch cheaper everywhere; type-dispatch loops ~25–30%,
  pingpong ~1–2%, 2026-07-19 — profiling REFUTED the copy hypothesis: the
  `to_message`/`from_message` copy is ~2% of a pingpong RT). The measured
  remainder is the matcher execution protocol itself (`hof_apply_step` →
  full `vm_apply` per candidate + a fresh body-thunk closure per match) and
  scheduler/capture — the deep lever is a BEAM-style inline receive (clauses
  compiled into the owning arm's bytecode, no matcher closure, no thunk).
  **[kernel]**
- ⬜ **`bintree` — GC / allocation pressure.** Build+walk trees; per-node alloc +
  minor-GC throughput vs BEAM. Inline small-vector storage (2026-07-01) and the
  checkpoint purity exemption + nursery capacity seeding (2026-07-18) trimmed it;
  the 2026-07-18 profile says what's LEFT is the deferred big-ticket JIT items
  (~17% `jit_run_fast_link` + ~11% frame staging — the "true call inlining"
  lever — and ~10% allocation FFI), not regressions. **[kernel]**
- ⬜ **`loop` — raw iteration overhead.** Was 6×; the resume-tier fix took −75%
  (2026-07-16). Residual per-iteration overhead: overflow-checked add,
  reduction/safepoint tick, frame reset. Incremental JIT-tuning grind (BEAM
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

- ✅ **Continuous fuzzing (`cargo-fuzz`)** — all five libFuzzer targets ship
  (`crates/lisp/fuzz/fuzz_targets/`): **reader**, **evaluator**, and (added
  2026-07-23) **JSON** (through a persistent `Interp`), the **dist wire
  framing** (the unauthenticated surface, via `dist::fuzz_decode_frame`), and
  the **bundle footer/archive** (`bundle::fuzz_parse`) — alongside the July
  stress kit (`make stress`). `make fuzz T=<target>` runs one; the fuzz dep is
  lean (`default-features = false` + `system-alloc` — ASAN must own
  allocation) and `ASAN_OPTIONS=symbolize=0` avoids a 90 s system-symbolizer
  stall per exit. First smoke: ~62 M total execs across the three new
  targets, zero findings. En route, a repo-wide build bug fell out: the build
  script's relative `rerun-if-changed=.git/HEAD` never existed, so **every
  build of every profile recompiled `brood`** — fixed (absolute + existing
  paths only).
- ✅ **Host-panic hardening (audit residue)** — closed 2026-07-23. The checker
  now runs its whole analysis under `catch_unwind` with the compile-ns /
  known-names / imports / GC-roots state restored on both paths (a panic
  degrades to one "checker internal error" diagnostic — brood-lsp and `nest
  check` survive); `expr_ty` already had its depth cap, and the remaining
  recursive walkers (`check_into`, `collect_def_names`, `check_recursion`,
  `check_macro_hygiene`, `collect_syms_into`, plus the expander's
  `macroexpand_all_depth`/`resolve_walk` — deep CODE, the sibling of the
  2026-07-20 deep-VALUE fix) grow the native stack in heap-backed segments
  (`stacker`). Pinned by a 30k-deep-form test that previously aborted the
  host.
- ✅ **Prelude freeze vs boot-expanded `receive`** (found + fixed 2026-07-22):
  the freeze's dangling-env assert swept the whole closure slab, including
  boot *garbage* (the builder heap never collects), so a dead
  captured-frame closure from a boot-time receive-matcher expansion killed
  boot. Fixed with a **reachability mark pass** at freeze: reachable closures
  keep the hard assert (a live captured frame really would dangle);
  unreachable ones get their env scrubbed (unobservable). The prelude
  `offload` now deliberately sits *after* the `receive` macro, so every boot
  regression-tests the fix; `BROOD_BOOT_TRACE=1` reports the scrub count.

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
  green M:N scheduling on a worker pool (originally corosensei coroutines,
  ADR-018; replaced 2026-06-08 by state-capture continuations with general
  work-stealing + live migration, ADR-100); shared code region
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

Runtime housekeeping (both items landed):

- ✅ **Tracing GC for mid-eval / never-returning loops.** The per-process
  generational semi-space copying collector (ADR-055/061/072) fires at the eval
  safepoint at **any** depth — roots are the explicit operand/env stacks the VM
  reified. Superseded the ADR-016 arena-reset.
- ✅ **Work-stealing scheduler.** Landed 2026-06-08 via the state-capture
  rewrite (ADR-100): corosensei deleted, a paused process is relocatable heap
  data, stealing is general (any queued process) and live cross-worker migration
  works. History + invariants: ADR-100 in [`docs/decisions.md`](docs/decisions.md).

---

## What's next — by area

### Language core & types

- ⬜ **Merely-wider inference case** — a body typed exactly `number` (int ∪ float)
  declared `int`, e.g. `(/ x 2)`; can't be pinned without occurrence/range analysis
  and flagging it would false-positive on int-valued runs (ADR-011).
- ⬜ **Parameter-type inference from arbitrary body usage** across branches — needs
  guard-aware dominance analysis; stays out until false-positive-clean (ADR-011).
  (The sound tiers of `infer_sig` already ship.)
- ✅ **First-class set kernel** — shipped 2026-07-24. A distinct `Value::Set`/
  `Tag::Set` backed by the CHAMP trie (`element → true`): the `#{…}` reader literal
  (evaluates + dedups its elements), `#{…}` printing, `set?`, `type-of` → `:set`,
  order-independent equality with a set **never** `=` to a map, seqable
  (`count`/`first`/`rest`/`map`/`fold`/`into`), cross-process round-trip
  (`Message::Set` + wire codec), and full GC/promote/compaction survival (the
  compatibility contract — GC trace, copy-on-send, hash, `ConstVal` handle, type
  lattice). Kernel ops `%set`/`%set-add`/`%set-remove`/`%set-has?`/`%set-count`;
  the `set` library (`std/set.blsp`) is now Brood sugar over the kernel type
  (constructor + `conj`/`disj` + `union`/`intersection`/`difference`/`subset?`).
  Tests: `tests/set_test.blsp` (18, incl. cross-process), `reader_hints_test`;
  green under `GC_STRESS`+`GC_VERIFY`, differential, `nest check` zero-warnings,
  suite 2921/2921 (ADR-060).
- ⬜ **Unbounded stream generation** (`iterate` / infinite producers) — lazy
  seq-view fusion already shipped (ADR-111); picks up when an editor feature needs it.
- 🟡 **`std/` curation + frameworks sequencing** (ADR-085/097) — `std/` curated and
  hierarchical module names shipped; the model is batteries-included (frameworks ship
  in the default install, not fetched). ⬜ Next: a future GUI framework ships bundled
  too; gated on the first real consumer.
- 🟡 **Native interop — WASM components** (ADR-071/145,
  [`docs/interop.md`](docs/interop.md)). ✅ **Slice 1 shipped 2026-07-22
  (ADR-145): the sandboxed host.** Embedded `wasmtime` (default-on `wasm`
  feature), `%wasm-load`/`%wasm-call`/`%wasm-exports`/`%wasm-close` with
  WIT-typed marshalling + fuel metering, `std/wasm.blsp` (`wasm-load`,
  `wasm-call`, `wasm-call-blocking` on the ADR-144 pool, `use-native` binding
  every export as a Brood fn), the `:unbound` checker category, and
  toolchain-free WAT-component tests. ✅ **Slice 2 (bytes marshalling) shipped
  2026-07-24:** a `list<u8>` parameter accepts a Brood `bytes` value directly
  (one-pass octet lower — the byte-oriented calls: hashing, compression, codecs,
  binary parsing), and a non-empty `list<u8>` result lifts back to `bytes` (an
  int vector still lowers; an empty `list<u8>` result stays an empty vector — the
  documented ambiguity edge). Copy-based (zero-copy read-mapping is still
  deferred); `crates/lisp/src/wasm.rs` `lower`/`lift` + `tests/wasm_test.blsp`
  (`blob-echo`/`byte-sum` over a `list<u8>` WAT component). ⬜ Remaining slices:
  the package-manager `:native` manifest/lock/build-on-fetch integration
  (`%wasm-build`) — the delivery vehicle, recommended next; WASI capability grants
  (gated on that manifest); guest `resource` handles; epoch preemption (low value
  — fuel already bounds runaways); and the blob **zero-copy** read-mapping
  optimization (over today's copy).

### VM & JIT

> **Status (2026-07-24): this track is at rest — its frontier is effectively mined
> out.** A full sweep found the remaining items are either already shipped or
> not worth doing: native-HOF-callback routing (already done via `apply_engine`);
> the allocation frontier `bintree`/`nqueens` (representation-capped by the boxed
> 24-byte `Value` — no sound quick win, `compute-frontier.md` 2026-07-24 block);
> and JIT Stage 4 (no-go — gated off in production, below). The VM+JIT is in good
> shape; further compute-perf work is high-effort/capped-payoff. Reopen only for a
> concrete need (e.g. closure-arm inlining if a real workload demands it).

- ✅ **The `let`-self-ref `send` divergence no longer reproduces** (verified
  2026-07-19): a `let`-bound self-recursive closure sent to a pid is rejected with
  the same "cannot send a self-referential local closure" error by BOTH engines in
  every shape tried — top level, created inside a VM-compiled `defn`, `send`
  executed from inside a VM arm, and via `spawn` (identical die-uncaught behavior).
  Presumably fixed en route by the capture/closure-template unification work; if a
  diverging shape resurfaces, it belongs in the differential fuzzer corpus.
- ✅ **Native higher-order callbacks route through the VM** (`try`/`binding`/
  `apply`/`isolate`) — done. Each dispatches its Brood callback through the shared
  `apply_engine` selector (`crates/lisp/src/eval/compile/mod.rs`), which runs the
  callback compiled on the VM when it's the active engine and on the tree-walker
  only under `BROOD_VM=0` — the once-per-call analog of the `use_vm`/`apply_value`
  branch `%range-reduce` uses per element. Call sites: `%try`/handler
  (`builtins/system.rs` `try_catch`), `%binding` thunk (`binding`), `%isolate`
  thunk (`isolate`), and the `apply` builtin's target (`apply_builtin`). The
  cached-arm hot path (`hof_resolve`/`hof_apply_step`) `%range-reduce` also uses is
  deliberately absent here: it amortizes cost across a tight loop and buys nothing
  for a thunk invoked once. The divergence above (since verified fixed) confirms
  the VM-routed path is behaviour-identical to the tree-walker.
- 🛑 **JIT Stage 4 — RUNTIME compaction survival** (ADR-091) — **investigated
  2026-07-24; NO-GO, near-zero real payoff.** The idea was a constant-pool
  indirection table (ADR-096 §4.C) so `runtime_collect` could rewrite handles
  without invalidating native code. Findings (verified in-code + measured):
  (1) RUNTIME **data-handle** relocation ALREADY survives in native code — the JIT
  bakes a pointer to the `ConstVal` and reads live bits via `brood_rt_const_load`;
  compaction rewrites those in place (`rewrite_arm_handles` over `live_vm_arms`),
  and runtime-service *function* addresses already sit in a per-arm indirection
  table. (2) The only native-invalidating step is the **epoch bump** in
  `runtime_collect_with` (`heap.rs` bumps `runtime.version` = `global_epoch()`,
  which nulls every arm's `jit_code` on next tier). (3) BUT that single-process
  compaction path is gated on `Arc::get_mut` (unique runtime ownership), which
  **never holds during real execution** — `spawn_root_program` keeps the runtime
  `Arc` shared for the whole run, so `(runtime-collect)` returns `:ran false` and
  the auto-safepoint compaction is skipped too. Production reclamation goes through
  the 2-generation path, which frees whole generations **without** rewriting handles
  or bumping the epoch → it does not invalidate native code at all. So the
  invalidating path is off precisely when it would matter; building the
  epoch-decouple/IC-remap machinery buys ~nothing. Left behind: the missing
  end-to-end coverage was added — `crates/lisp/tests/jit_runtime_compaction.rs`
  (a JIT'd RUNTIME-handle arm stays correct across a real relocation; green under
  `GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY`).
- ✅ **Leaf-callee inlining** (the real call-heavy lever) — **implemented 2026-07-19;
  DEFAULT ON since the same evening (`BROOD_NO_LEAF_INLINE=1` opts out)** after the
  gating measurements came back flat everywhere they had to (boot / 100×-hello
  batch, `require`-heavy loads incl. editor/buffer+sexp+json+regex, `nest check`
  over std/, the in-language suite wall, every benchmark row) and the wins held:
  ~30% on the scalar-helper loop shape, a further ~8% on type-predicate dispatch
  compounding with `PrimOp1::TypeOf` (predicates are call-free now, so they
  splice into hot matchers). A hot fixed-arity `defn`
  whose non-tail static-head calls all resolve to small, calls-free, non-capturing
  callees gets a stored derivation (args → `LetBind` into shifted callee slots,
  callee body spliced above the caller's frame) that rides the existing two-stage
  deferred-upgrade channel. Soundness: derivation happens once at arm-compile time
  (heap access for callee resolution, reentrancy-guarded), is epoch-stamped, and
  the lowerer refuses any other epoch — hot reload wins by construction (tested:
  a post-warm `def` of a spliced callee takes effect). The inlined engine has no
  deopt checkpoint, so derivation requires ZERO residual non-tail calls (from-ip-0
  re-run stays effect-free); `jit_ckpt_read` now also refuses the inlined engine
  (the small layout's ckpt slot lies inside the spliced range — a real Int there
  faked a journal). `inline_nslots` is floored at the small frame (spill+ckpt
  reserves made it possible for the "grow" to be an underflowing shrink).
  **Measured: ~30% on the scalar-helper loop shape** (`(+ acc (sq (add1 i)))`
  1.65 → 1.2 s); benchmark-suite rows flat (they're recursive/HOF/alloc-bound —
  the remaining shapes need closure-arm support (defn-gate today) + Phase 3/4).
  Gates green with the flag on: JIT≡VM differential under GC_STRESS+VERIFY,
  VM≡TW differential, 3 dedicated tests incl. hot-reload + residual-call gate.
  ⬜ Next: closure arms (needs a fast-link invalidation story without a defn
  name) — the expansion/`require` measurement + default-flip are done.
- ⬜ **Layer-2 computed-goto dispatch** (`std::arch::asm!`, x86-64, `#[cfg]`-gated,
  pure-Rust fallback) — only if profiling still shows dispatch overhead. Additive.
- ⬜ **Allocation / GC frontier — `bintree` + `nqueens`** (the two worst compute
  rows). **Profiled 2026-07-24 — no sound quick win; representation-capped.** Both
  are heavily JIT'd, NOT interpreted (JIT ~3.5×/~2.9× over the plain VM: bintree
  ~422→~119 ms, nqueens ~286→~100 ms local); bintree runs 100% native, nqueens' hot
  paths (the `reduce` step lambda + `safe?`/`solve`) run native too. The ~9.5× gap
  (bintree 102 vs Elixir 10.4 ms N=200; nqueens 83 vs Node/Elixir 8.7/9.0 ms N=10)
  is the **boxed 24-byte `Value` allocation floor + GC churn**, not dispatch:
  bintree churns ~819K escaping 48-byte tree-node vectors/run. Measured dead ends:
  `BROOD_GC_FLOOR` sweep *regresses* (not GC-frequency-bound); the cells **escape**
  so JIT escape analysis is inapplicable; nqueens' lambda already runs native
  (`BROOD_NO_HOF=1` A/B confirms — no stuck slow path). The only remaining lever is
  a **narrower cell representation**, which spends a core invariant (new `Value`
  kind / brushes the NaN-boxing line `types.md`/compute-frontier §2 reject) for a
  capped payoff. **Defer** unless that invariant spend is explicitly wanted — better
  ROI in JIT Stage-4 / closure-arm inlining. Full analysis:
  [`docs/compute-frontier.md`](docs/compute-frontier.md) (2026-07-24 RESUME block).

### Tooling & errors

- ✅ **`nest test` selection — `mix test` parity** — shipped 2026-07-24. The suite
  had **no way to run a subset by name**: 2965 tests, and the only narrowing was a
  file path. Now `--only`/`--exclude`/`--include SELECTOR` (a tag, `test:substr`,
  or `describe:substr`), `FILE:LINE` (the covering test), `--failed` (last run's
  failures, kept as a set difference in the project cache dir), `--seed` (shuffle,
  seed echoed for replay), `--partitions N --shard K` (stable-hash CI shards),
  `--max-failures`, `--repeat-until-failure`, `--timeout`, `--slowest N`,
  `--no-trace`. Tags come from `:tags [kw …]` on `describe`/`test`, merged
  group→test. Selector parsing + all selection logic live in Brood
  (`std/tool/test.blsp`, `test--make-filter`); `nest` only forwards argv.
  `tests/test_selection_test.blsp` (54 cases, incl. cross-process spec round-trip).
  ⬜ Still unmapped from `mix test`: `--cover` (needs a coverage mechanism — see
  "Test coverage" below), `--stale` (needs a per-test dependency graph),
  `--formatter`, `--breakpoints`.
- 🟡 **Test coverage** — ✅ **function-level `nest test --cover` shipped 2026-07-24**
  (ADR-148, [`docs/coverage.md`](docs/coverage.md)): which project functions the
  suite never entered, plus `--cover-min PCT` to fail the run under a floor.
  Implemented as **pure Brood policy with zero kernel support** — `global-names` +
  `source-location` for the denominator, `def` rebinding + late binding (ADR-013)
  as the instrumentation seam, and `Value::Table` (ADR-107) for cross-process hit
  aggregation. The shim is variadic *because* `arglist` reports only one arm of a
  multi-arm function; one new off-switch (`BROOD_NO_RELOAD_DIAG=1`) silences the
  arity diagnostic the rebinding legitimately trips.
  ⬜ Still: **line/branch coverage**, a separate tier. The raw material exists —
  IR nodes carry `pos: Option<Pos>` and `CompiledArm` records its file
  (`eval/compile/ir.rs`) — and the shape is settled (compile-time instrumentation
  rather than a per-instruction runtime check, so an ordinary run stays unchanged;
  JIT off in that mode; reporting extends `std/tool/coverage.blsp`). Deferred until
  something concrete needs per-line detail: function coverage answers "what does my
  suite never touch", which is the question that changes what you write next.
- ✅ **`nest format --changed`** — shipped 2026-07-23. A git-aware narrower
  scope: formats only the `.blsp` files git reports not-committed-clean
  (modified/staged/untracked), via a new `%git-changed-files` primitive
  (returns the paths, or `:not-a-repo`); falls back to the whole project
  outside a git repo, and `--check` still scans everything (CI's clean-tree
  gate). `crates/nest/tests/format_changed.rs`.
- 🟡 **LSP** — tiers 0–2 ship. ✅ **finer finding spans** shipped 2026-07-23:
  a type-mismatch / callback-arity finding now anchors at the offending
  **argument** when it is a positioned sub-form (a nested call —
  `(string-length (+ 1 2))` points at `(+ 1 2)`), falling back to the call
  head only for a bare literal/symbol (which the pair-keyed position table
  doesn't record). No `Pos`-threading rewrite needed — the argument value
  already carries its position. ✅ The **create-missing-`defn`** code action
  already ships (verified 2026-07-23 — `create_defn_action` in
  `crates/lsp/src/code_actions.rs`, a stub `(defn foo (a b …) nil)` with arity
  matched to the call site, tested; the roadmap line was stale). ✅
  **incremental document sync** shipped 2026-07-24: the server advertises
  `TextDocumentSyncKind::INCREMENTAL` and splices each `didChange` range into the
  stored buffer via the UTF-16 `LineIndex` (`apply_content_change`, per-edit index
  rebuild so a batch compounds correctly); the parse stays whole-document (cheap).
  2 new tests (ranged-splice offset precision; multi-edit batch compounding); 116
  LSP tests green. ⬜ Still next: range/delta semantic tokens (deferred — the token
  walk is already off a cached CST, marginal payoff until profiling shows it hurts).
- 🟡 **Errors that teach (LLM-native)** ([`docs/llm-native.md`](docs/llm-native.md))
  — first instances landed. ✅ **reader-level hints** for the Clojure/Scheme
  syntax the reader mis-parses shipped 2026-07-23: `#{…}` (set literal),
  `#(…)` (anonymous-fn reader macro), and `#'` (var-quote) now raise a clean
  parse error with a `:hint` naming the Brood idiom (was "odd map" / "unbound
  #"); Scheme/Clojure **nested `let`/`letrec` bindings** `((a 1) (b 2))` raise
  a hint to flatten (`tests/reader_hints_test.blsp`). ✅ **The
  `explain-error` / `find-pattern` MCP tools + the intent→idiom cookbook**
  shipped 2026-07-23 (`std/tool/explain.blsp`, a new baked-in `explain`
  module): `explain-error` maps a stable E-code (or a caught error's `:code`,
  or a kind keyword) to `{:summary :causes :fix :example}` — the
  Brood-idiomatic fix, not just the message; `find-pattern` keyword-searches a
  curated intent→idiom cookbook ("loop / mutate / build a string / spawn /
  parse binary …"). Both are pure Brood data + wrappers, wired as `nest mcp`
  tools (now 20 in a project context). `tests/explain_test.blsp` + `mcp_test`. ✅ **cookbook expanded
  2026-07-24** — five confirmed Clojure/Scheme reflexes folded in: keyword-as-fn
  `(:k m)` → `(get m :k)`, char literal `\c` → 1-char string / `int->char`,
  discard `#_` → `;`, regex `#"…"` → `(require 'regex)` + `regex/match?`, and the
  `#{…}` set entry updated to the now-first-class kernel literal. ✅ **reader hints
  for the last three silently-mis-parsed forms shipped 2026-07-24**: `#_` (discard →
  `;` comment), `#"…"` (regex literal → `(require 'regex)` + `regex/match?`), and `\c`
  (character literal → 1-char string / `int->char`), each a clean parse error with a
  `:hint` naming the Brood idiom (`read_hash` + a new `'\\'` arm in `read_form`;
  `tests/reader_hints_test.blsp`, language.md table). ⬜ Still: folding each new repeat
  mistake into the rule-of-three (ongoing curation, no named gap).
- 🟡 **MCP tooling** — ✅ the write sandbox is **symlink-escape-proof** (shipped
  2026-07-23): a new `canonicalize` primitive (real-path resolution — symlinks
  + `.`/`..`, works for not-yet-existing targets) backs a second sandbox gate
  in `mcp--project-path`, so a project-relative `..`-free path that resolves out
  of the tree through a symlinked directory is now rejected, not just the
  lexical cases (`crates/lisp/tests/mcp_sandbox.rs`). ✅ **the streaming /
  progress-notification tier** shipped 2026-07-23: a `tools/call` carrying an
  MCP `_meta.progressToken` arms a sink so a handler's `(mcp-progress progress
  total message)` streams `notifications/progress` to the client *during* the
  synchronous call (via the reentrant stdout lock); the core `check` tool
  reports per-file, and `%mcp-progress` is a no-op elsewhere so any handler
  can call it safely (tests in `mcp.rs`). ✅ **GC/process *traces* exposed**
  (shipped 2026-07-24): a new `watch-runtime` MCP tool arms the kernel
  `system-monitor` on the handler process for a bounded window (`:ms`, capped
  5 s, with an optional `:filter` kind selector), then returns the collected
  `[:runtime kind pid detail]` stream — GC pauses, spawn/exit churn, JIT deopts —
  a *trace*, not a snapshot, complementing `processes`/`node`. Pure Brood over the
  telemetry/`system-monitor` seam (`std/tool/mcp.blsp`); tests in
  `tests/mcp_test.blsp`. ⬜ Still: none named — the snapshot-vs-stream gap is now
  closed.

### Editor (M2) & display (M3)

- ⬜ **Major/minor modes** — how a buffer selects which keymaps are active.
- ⬜ **Mouse / resize input events** — deferred until a feature needs them.
- ⬜ **GPU-window frontend** — a later additive path speaking the same display
  protocol; arbitrary per-px buffer sizing rides with it.
- 🟡 **Telemetry** (ADR-106) — core landed; the kernel event *sources* shipped
  2026-07-19 (ADR-137: `system-monitor` → `telemetry/watch-runtime`, GC/spawn/exit/
  deopt as `[:runtime kind]` events). ✅ **Metric aggregators + sampling** shipped
  2026-07-24: `counter`/`sum`/`last-value` (gauge)/`summary` (running
  count/mean/stddev/min/max) + `sample-every` (1-in-N), the Elixir
  `Telemetry.Metrics` set folded entirely in Brood over the `attach` seam — zero
  new kernel surface. State is a shared `table` (ADR-107); folds run serially in the
  one listener so a read-modify-write is race-free and stays bounded (no sample
  retention). `metric`/`metrics-snapshot` readers poll it atomically.
  `tests/telemetry_metrics_test.blsp`. ✅ **Distribution/histogram aggregator +
  node up/down shipped 2026-07-24** (both pure Brood, zero new kernel surface):
  `distribution` buckets a measurement into explicit ascending upper bounds
  (Prometheus / Elixir-`distribution` shape) with bounded per-bucket counts (no
  samples retained, like `summary`), and `metric-percentile` estimates a quantile by
  in-bucket linear interpolation (`histogram_quantile` — bounded memory for approximate
  percentiles); `watch-nodes` polls `(nodes)`, diffs the peer set, and re-emits
  `[:runtime :nodeup]`/`[:runtime :nodedown]` through the same `[:runtime kind]` seam as
  `watch-runtime` (polling because the kernel has no `[:nodeup]` event — it catches both
  inbound peers and outbound `connect`s). `tests/telemetry_metrics_test.blsp` now 15.
  ⬜ Still to fold in: unifying `gc-stats`/`vm-stats`/`process-info` snapshots behind the
  stream so `nest observe` + `nest mcp` consume it; `defevent` + checker-validated event
  schemas; and the location-transparent remote tier over the dist link.

### Server / daemon (M4)

- ✅ **Inbound (server-side) TLS + the `mio` reactor** — shipped 2026-07-22
  (ADR-143, the parity program's item 3 above): one reactor thread for every
  socket, full-duplex TLS driven sans-io (the read/write-split constraint
  dissolved), `serve-loop` serves https unchanged. M4 is complete.
  - ✅ **Reactor reap hardening** (2026-07-24): a **TLS handshake-completion
    timeout** (default-on, 30 s — a peer that stalls mid-handshake holds an fd the
    app can't reclaim, since it never sees the socket until the handshake
    finishes), and an **opt-in per-socket idle timeout** (`tcp-set-idle-timeout`,
    default off — slow-loris protection a server arms on untrusted connections;
    off by default so a legitimately long-idle stream — SSE, long-poll, the editor
    daemon — is never reaped). The defence-in-depth complement to the client-side
    `tcp-close` fixes (2026-07-24 validation round 3).
- ✅ **OTP near-term** — all three closed as of 2026-07-18:
  **`send-after`/`send-interval`/`cancel-timer`** shipped (pure Brood in the
  prelude — a timer is a green process on the scheduler's timer wheel; the
  interval variant monitors its target and self-cleans; `tests/timer_test.blsp`).
  The other two turned out to be **stale roadmap entries** — a synchronous
  **`remote-spawn-sync` returning the child pid** and the **`[:$stop]`
  graceful-teardown convention** (supervisor `:shutdown` policies + `defprocess`
  `terminate`) had both already shipped.
- ⬜ **OTP deferred** (ADR-011, gated on a real consumer): **`gen_statem`** state
  machines; an Elixir-style **`Registry`**/via-tuples + **process groups (`pg`)**; an
  **`Application`** behaviour; **synchronous, ordered, rollback-on-failure** supervisor
  startup + per-child intensity counting + child `type`/`significant`/`auto_shutdown`
  metadata.
- 🟡 **Dist refinements** (ADR-011): ✅ exact propagated exit reason for a
  *non-trapping* linked peer (fixed 2026-07-18 — hardness split from the reason,
  see the survey housekeeping item above; the shared `deliver_exit_to` covers
  remote links too). Still ⬜: a `terminate/2` hook on hard kill; **long-name
  FQDN resolution** (a long name is passed explicitly today, no resolver);
  Windows Unix-socket transport.

### Packaging & ecosystem

- ✅ **Package manager** (ADR-037/147, [`docs/packages.md`](docs/packages.md)) —
  `:path` deps end-to-end, **`:git` deps** (slice 2), and **the verbs + auto-fetch**
  (slice 3: `nest fetch`/`update`/`tree`/`add`/`remove`) all shipped 2026-05-30
  (`%git-clone`/`%git-resolve-ref` in `builtins/io.rs`, policy in
  `std/tool/package.blsp`). ✅ **v2 shipped 2026-07-24 (ADR-147):** **`:tarball`
  deps** (`[name :tarball URL :sha256 HEX]` — download via `std/net` `http-get` or
  `file://`, mandatory sha256 verify, strip-extract via the new `%untar-gz` shell to
  `tar` on the offload pool) and a **git-backed registry** — the index is just a git
  repo of `packages/<name>.blsp` metadata (no hosted server), with `nest publish`
  (append the project's entry, no auto-commit), `nest search`, and `[name :version
  "X"]` deps resolving an exact version to its git source. ⬜ Remaining (ADR-011):
  semver ranges, tarball sources *inside* registry entries, signed packages, cloned-
  index auto-refresh.
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
mechanism has shipped (re-check per reload rather than prove once — ADR-123/124/125),
and the old "checking never rejects a runnable program" invariant has been revised
throughout (`CLAUDE.md`, [`docs/types.md`](docs/types.md) contract #5): the checker
never gates the live image; the one hard reject is batch/CI (`nest check` exits
nonzero on any warning).

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
- Rationals (ints already auto-widen to big integers on overflow; f64 +
  decimals cover the rest)
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
