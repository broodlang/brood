# Runtime frontier — the process/concurrency option book

The full option list for closing the remaining runtime gaps to the BEAM, written after
ADR-175 (shared compiled code) landed. Companion to the benchmark repo's `FRONTIER.md`
(which states the current *position*); this file is the **analysis and the menu** — what
we have, how Erlang/Go/Pony solve the same problems, and every option with its expected
win, cost, and risk, ordered for execution. Tick items off here as they land; move
anything measured-and-rejected to the dead-ends list at the bottom.

Current standing (2026-07-29, all measured this week):

| metric | Brood | BEAM/Elixir |
|---|---|---|
| live process (spawn-live, RSS/proc) | ~6.6 KB | ~3 KB |
| parked-process allocation floor | ~4.5 KB | ~2.7 KB (338 words) |
| spawn+park | ~7.6 µs, 15.8 allocs | ~1–2 µs |
| message round-trip (`pingpong`) | ~2.8× Elixir | — |
| ring hop | ~3.5× Elixir | — |
| parallel scaling (2→12 workers) | 2.5× of a 3.0× ceiling | BEAM 2.4× same box |

## Part 1 — what we actually have (measured, not assumed)

**Per-process anatomy** (parked process, all from the 2026-07-29 profiles):

| item | bytes | notes |
|---|---|---|
| `Box<Process>` | 1840 | `Heap` struct inline (~1.65 KB of *fields*, before any allocation) |
| IC tables | 664 | `vm_call_ics` 384 + `vm_fast_links` 160 + `arm_ic_blocks` 120 |
| slabs (live) | 256 | 11 typed `Vec`s, first-touch 192 B each (Rust min-cap 4) |
| `Suspended` + frames | 384 | the captured continuation |
| `roots` | 192 | operand stack |
| `Arc<Mailbox>` | 184 | |
| misc small | ~500 | |

Three tunings measured and reverted (devlog 2026-07-29): park-trim threshold (zero),
capacity-1 slab first-touch (110 B, `bintree` +4.8%), IC-drop on park (640 B,
`pingpong` +26% / first-park-only still +11.5%). **Conclusion: the floor is working
state.** Only *smaller* state or *shared* state moves it now.

**The message path is TWO full copies.** `send` serializes `Value → Message` (a boxed
intermediate tree; bigints/decimals go through decimal *strings*), the mailbox holds the
`Message`, `receive` deserializes `Message → Value` into the receiver's heap. Both
intermediate trees become garbage immediately. The design is right for the *dist* wire
(heap-independent, node-portable) but local sends pay wire-format cost.

**The receive matcher runs per-candidate `vm_apply`** (via the HOF fast path;
`BROOD_NO_HOF=1` is 197 → 509 ms on pingpong, so the fast path already earns its keep —
but a frame per scanned candidate remains).

**`Heap` carries loader/checker state per process** — `form_pos`, `imports`,
`ns_known_names`, `module_exports_cache`, `known_ns_cache`, `check_dep_rec`,
`compile_ns`, `current_file`, `dynamics` — fields only the root/loader process
meaningfully uses, paid in struct size by every spawned worker.

## Part 2 — how the others do it

**BEAM** (the reference; everything below is standard ERTS design):
- A process starts at **338 words ≈ 2.7 KB total**: PCB + one memory block in which the
  **heap grows up and the stack grows down** — one allocation for both, no separate
  operand stack, no per-type slabs.
- **A process owns no code state.** Code lives in the module area; calls go through the
  export table (one global slot per function, atomically patched on reload). There is no
  per-process inline cache — the export-table entry *is* the shared, always-warm cache.
- **X registers live in the scheduler, not the process** — a yield saves only live
  registers into the process stack. Per-process cost of execution machinery: ~zero.
- **Messages are copied once**, sender → receiver heap (or a heap fragment attached to
  the receiver when its main lock is contended). No intermediate form locally; the
  external term format is only for the wire.
- **Hibernation is opt-in** (`erlang:hibernate/3`): discards the stack, shrinks the heap
  to minimum. Erlang hit exactly the tradeoff our reverted IC-drop hit, and resolved it
  by making it the *programmer's* call. `spawn_opt` exposes `min_heap_size` etc.
- ERTS keeps free-lists and pre-sized allocators tuned for process churn.

**Go**: goroutines start at a 2 KB contiguous stack, grown by copy; the `g` struct is a
few hundred bytes; all code/caches global. **Pony**: an actor is ~250 B plus lazily
pulled size-class chunks from a global pool. Both confirm the same shape: *per-unit state
is a stack + a control block; everything else is shared or scheduler-owned.*

**Where Brood structurally differs from all of them:** per-process typed slab `Vec`s
(11), a per-process operand-stack `Vec` separate from the heap, per-process IC tables,
and a wire-format message hop even locally. Each is an option below.

## Part 3 — the options

Grouped; each with precedent, expected win, cost, risk. Ordered within groups.

### A. Message latency (`pingpong`/`ring`, the widest honest gap)

- [ ] **L1 — single-copy send to a parked receiver.** When `send` finds the receiver
  parked (the `waiter` is owned under the mailbox state lock — the same quiescence
  `trim_parked` already relies on), copy the value graph **directly sender-heap →
  receiver-heap**, skipping `Message` entirely; enqueue a small "already-in-heap"
  variant. Precedent: BEAM's copy-into-receiver-heap. Expected: the dominant shape
  (send-then-block ping-pong) drops from 2 copies + garbage to 1 copy; this attacks the
  2.8–3.5× directly. Cost: a cross-heap copier with rooting/epoch care; the `Message`
  path stays for running receivers and dist. **The highest-value single item on this
  list.** Risk: medium — new GC-visible path, needs GC_STRESS + TSAN + differential gate.
- [ ] **L2 — heap fragments for running receivers** (BEAM's other half): copy into a
  fragment the receiver adopts at its next safepoint, removing the `Message` hop for the
  non-parked case too. Do only after L1 proves the copier.
- [ ] **L3 — matcher without a frame per candidate.** The receive matcher is a compiled
  arm; scanning N buffered messages costs N `vm_apply`-class activations. Options: lower
  the match into a frameless predicate over the staged value (the match-lowering pass
  already exists for `match`), or cache per-message match failure (BEAM's OTP-24 marker
  optimization for selective receive). Measure the scan share first with perf-stats.

### B. Process memory floor (~4.5 KB → toward ~3 KB)

- [ ] **M1 — split `Heap` into hot core + lazily-boxed cold state.** Move the
  loader/checker/ns fields (`form_pos`, `imports`, `ns_known_names`,
  `module_exports_cache`, `known_ns_cache`, `check_dep_rec`, `compile_ns`,
  `current_file`, `dynamics`) behind one `Option<Box<ColdHeap>>` allocated on first use.
  A worker never touches them. Expected: several hundred bytes off `Box<Process>`'s
  1840 B, plus cache-density wins. Mechanical but wide; no semantic risk.
- [ ] **M2 — shared IC entries for sealed callees** (ADR-175 Stage 3, the BEAM
  export-table move). A sealed (ADR-166) binding's resolution is process-independent, so
  its IC entry can live with the *shared arm* (atomics/ArcSwap; `FastLink` is already
  `#[repr(C)]` plain data). Removes most per-process IC slots (664 B floor item) *and*
  starts every fresh process warm — a latency win for spawn-heavy code too. Risk:
  concurrent lock-free structure read by JIT'd code — the one option needing the full
  TSAN/loom treatment. Decide against M3 first: they overlap.
- [ ] **M3 — direct-link sealed callees at compile time** (ADR-175 Stage 1). A call to a
  sealed name needs no site, no probe, no epoch check — bake the callee into the shared
  chunk (`OnceLock<Arc<CompiledArm>>` per site, shared across processes). Kills the IC
  slot AND the per-call validation for 67–99% of sites (measured range, user-heavy vs
  prelude-heavy). Constraint (recorded in ADR-175): must keep an arm fast path, since
  today's IC caches the resolved arm. **M3 subsumes most of M2's win at lower concurrency
  risk; evaluate first.**
- [ ] **M4 — process-shell recycling.** Per-worker free-list of retired
  `(Box<Process>, Arc<Mailbox>)` shells; re-init the cheap fields on spawn. Precedent:
  ERTS allocator free-lists, every thread pool. Expected: spawn 15.8 allocs → ~2, a large
  cut to the 7.6 µs spawn path; floor unchanged. Risk: staleness bugs (epoch stamps
  already guard heap handles; the mailbox needs a generation bump for pid reuse safety).
- [ ] **M5 — `(hibernate)` builtin — the Erlang answer to the reverted experiment.**
  Opt-in: drop IC tables, shrink slabs, trim roots (we *measured* this at 3.89 KB/proc);
  the process pays cache rebuild on wake **by its own choice**, exactly `erlang:hibernate/3`.
  Small, safe, and gives real long-idle apps (connection-per-process servers) the win the
  automatic policy couldn't take. Also consider `spawn` options for initial sizing
  (`spawn_opt` parity).
- [ ] **M6 — scheduler-owned execution scratch** (the BEAM X-register model). Fields
  used only *while running* (`gen_cache`, JIT scratch state) move to the worker; suspend
  saves the little that persists. Shrinks `Process`; touches the run loop. Do after M1
  (which may take the cheap half of the win).
- [ ] **M7 — one memory block per process** (BEAM's heap-up/stack-down). Collapse the 11
  slab `Vec`s + `roots` + `env_roots` into one growable region. The largest structural
  win (allocation count, locality, and the floor) and the deepest refactor — handle
  encoding indexes typed slabs today. **Cost it seriously before starting; only worth it
  if M1–M5 leave us short.**

### C. Spawn/throughput

- [ ] **S1 = M4** (recycling — it is primarily a spawn-time win).
- [ ] **S2 — pre-resolved body blocks.** `spawn`'s thunk is const-cached (done, ADR
  earlier); with M3, a spawned body's call sites need no per-process IC setup at all —
  measure spawn again after M3 lands.

### D. Compute (tracked in benchmark FRONTIER; unchanged by this doc)

X-register call convention (`bintree`/`fib` class), capturing-closure fast-link
(`pipeline`), in-arm alloc + variable-index vector reads (`nqueens`), bytes/codepoint
fast path (`json`/`regex`/`base64`), unboxed float arrays (`nbody`/`matmul`,
immutability-fraught).

## Recommended execution order

1. **M5 `(hibernate)`** — small, safe, Erlang-blessed; converts an already-measured
   reverted patch into a feature.
2. **M4 shell recycling** — bounded, big spawn win, no GC-semantics change.
3. **L1 single-copy send** — the highest-value item; needs its own careful gate.
4. **M3 direct-link sealed callees** — speed + floor together; design vs M2 first.
5. **M1 cold-heap split** — mechanical floor win, any time.
6. **L3 matcher cost** — measure first, then decide.
7. **M2 / M6 / M7 / L2** — re-evaluate after the above land.

## Dead ends (measured; don't re-attempt without new evidence)

- Automatic IC-table drop on park (any policy — every-park or first-park): the latency
  rows pay 12–26%. The opt-in form is M5.
- Park-trim threshold tuning: zero effect at any setting.
- Capacity-1 slab first-touch: 110 B/proc, `bintree` +4.8%.
- Splitting shared code from per-process JIT-tier state: the motivating regression was a
  `make ab` single-core-pin artifact; sharing tier state is a net win.
