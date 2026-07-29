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

### A. Message cost — TWO separate problems, measured apart

A payload ping-pong (20 000 round trips, `/tmp/pp_payload.blsp` shape) separates them:

| payload | wall | what it shows |
|---|---|---|
| 0 elems | 80 ms | **~2 µs/message of park + wake + match** — no copy at all |
| 16 | 113 ms | copy ≈ 29% |
| 64 | 173 ms | copy ≈ 54% |
| 256 | 485 ms | copy ≈ **84%** |

(That table is at 20 000 round trips; see the methodology note below — the *ratios*
between payloads hold, the absolute per-message figure at scale is ~1.3 µs.)

So **copy cost is proportional to payload, and the latency rows carry none** — `pingpong`
sends a bare keyword (`Message::Keyword` is an enum variant, no allocation) and `ring` a
small token. The 2.8–3.5× gap to Elixir on those rows is the **2 µs of park/wake/match**,
not serialization. Both problems are real; they need different fixes.

**Decomposed further 2026-07-29 — and it corrects the split above.** Self-send (mailbox
+ match, never parks) against cross-process ping-pong, and then by message shape:

| measurement | per send+receive | reading |
|---|---|---|
| self-send, no park/wake | 1.19 µs | the mailbox/copy/match path alone |
| cross-process ping-pong | 1.37 µs | **park + wake + enqueue + pickup is only ~0.18 µs (13%)** |
| `:ping` (bare atom) | 580 ns | fixed: lock, queue, match activation, dispatch |
| `[:ping i]` | 1160 ns | **+580 ns for a 2-element vector** |
| `[:ping i i i]` | 1380 ns | +~110 ns per extra element |

Two conclusions, both against what this file previously said:

1. **L4 (park/wake) is not where the time is** — 13%. Deprioritise it.
2. **L1 applies to essentially every real message, not just big payloads.** The earlier
   "payload 0 ⇒ L1 buys ~0" reading was an artefact of the probe: its "payload 0" case
   still sent `[:ping from p]`, a 3-element vector. Only a *bare atom* avoids the copy.
   Since real protocols are tagged tuples (`[:call ref args]`, `[:EXIT pid reason]`), the
   two-copy path costs ~50% of a typical small message, and halving it is worth **~25%**
   of send+receive — on the latency rows too, not only payload-heavy ones.

**A1 — the per-message fixed cost (owns the latency rows).** Decomposed at 200 000 round
trips / 400 000 messages, payload 0 (≈1.3 µs per message at baseline):

| config | wall | reading |
|---|---|---|
| baseline | 527 ms | |
| `BROOD_NO_HANDOFF=1` | 1003 ms | **direct handoff is worth 1.9×** — already the single biggest win on this path, and already shipped |
| `BROOD_NO_HOF=1` | 1574 ms | **the HOF matcher fast path is worth 3.0×** — likewise shipped |
| `BROOD_NO_JIT=1` | 528 ms | the JIT is **neutral** here; this path is not compute |

Both of the large levers on this path have already been taken. What is left is the
irreducible-looking remainder: one mailbox `Mutex` acquisition, `wake_parked`, a
re-enqueue, worker pickup, and one matcher activation per candidate.

- [x] **L3 — DONE 2026-07-29 (fix 1, the structural pre-filter). Selective-receive rescan was O(rounds × backlog). Measured 2026-07-29, and
  it is the bigger half of this item.** The receive loop rebuilds *every* non-matching
  candidate into the heap (`from_message`, allocating garbage) on *every* scan, then runs
  the matcher on it. 2000 receive rounds against a static backlog:

  | backlog | wall | per rescanned message |
  |---|---|---|
  | 0 | 20 ms | — |
  | 50 | 64 ms | 0.44 µs |
  | 200 | 171 ms | 0.38 µs |

  Linear in backlog × rounds, so a process with a busy mailbox that waits on a specific
  tag (a `gen` server, a supervisor filtering `[:EXIT …]`) pays quadratically in the
  backlog. `MailboxState::scanned` already avoids re-running a *parked* scan for messages
  behind the mark, but a freshly-entered `receive` starts from zero every time.

  **Shipped:** fix 1 below. Backlog 0/50/200/500 went 20/64/171/~420 ms → 21/23/26/34 ms
  (12× at 500), for `pingpong` +1.3% / `ring` +2.6% on a one-message mailbox — the decode
  is lazy (only when `queue.len() > 1`), which keeps the trivial case near-free. Fix 2
  (receive markers) remains available for ref-addressed protocols.

  Two independent fixes, either worth doing:
  1. **Don't rebuild to reject.** ✅ shipped The expensive part per candidate is `from_message` +
     a matcher activation. A cheap structural pre-filter on the `Message` (before it
     becomes a `Value`) would reject most non-matches without allocating — the common
     patterns are tag-led (`[:want _]`), so comparing the first element's keyword against
     the clause tags is enough. Needs the compiler to expose each `receive` clause's
     leading tag, which the `match` lowering already knows.
  2. **BEAM's OTP-24 receive markers** for the ref-addressed case: a `gen`-style call
     stamps a unique ref, so the scan can start at the marker instead of the queue head.
     Narrower (only helps ref-carrying protocols) but removes the rescan entirely there.

  The original framing — "remove the per-candidate frame" — is the *smaller* half: with
  the HOF fast path already shipped (worth 3.0× on the message rows), a single match on a
  1-message mailbox is close to floor. The rescan is where the remaining time is.
- [x] **L4 — park/wake path: MEASURED AND RULED OUT (2026-07-29).** Isolating it
  (self-send vs cross-process, same message shape) puts the entire park + wake + enqueue +
  worker-pickup path at **~0.18 µs of a 1.37 µs send+receive — 13%**. Direct handoff
  already took the large win here (1.9×). Not worth further work while the copy path is
  ~50%.

**Methodology note, learned the hard way twice now:** size these microbenchmarks so JIT
compilation amortizes. At 20 000 round trips `BROOD_NO_JIT=1` looked *18% faster*, which
is pure non-amortized compile cost — the same artefact that produced the phantom `collatz`
regression under `make ab`'s single-core pin. At 200 000 it is a dead heat, and on the
real `ring` row the JIT is 7% **faster**. Always cross-check a micro against the
benchmark row before believing it.

**A2 — the per-byte copy cost (owns real payload-carrying apps, not the microbenchmarks).**

- [ ] **L1 — single-copy send to a parked receiver. Design complete (2026-07-29), ready
  to execute; not yet written.** Sketch, with the three questions that decide it already
  answered in code:
  1. *Can the sender touch the receiver's heap?* **Yes** — `deliver_envelope` takes the
     mailbox state lock and `wake_parked(&mut st)` hands back the receiver's
     `Box<Process>`, giving exclusive `&mut` on its `Heap`. Same quiescence `trim_parked`
     uses. Sender holds `&Heap`, receiver `&mut Heap` — two distinct objects, so borrowck
     is satisfied.
  2. *Does the copier need incremental rooting?* **No** — and this is the finding that
     makes L1 tractable. `from_message` already builds graphs in a receiver heap by
     accumulating children in an ordinary off-heap `Vec<Value>` and then calling
     `heap.list(vals)`. That is only sound because **allocation never collects** in this
     runtime (collection runs at eval safepoints, never inside `alloc_*`). A cross-heap
     copier can use the identical pattern.
  3. *Where does the copied value live until the receiver pops it?* **This is the hard
     part, and the first answer here was wrong.** The mailbox is not a GC root, so it
     cannot hold a bare `Value` — correct. But the proposed fix (push it onto the
     receiver's `roots` and carry the index) is **unsound**: `roots` is the *operand
     stack*, not a general root set. `truncate_roots(n)` is called from **109 sites** —
     every frame pop, every error unwind — so a long-lived mailbox value parked there is
     silently dropped the moment the receiver returns from a call. Indices are stable
     across a *collection* (which is what `Suspended` relies on), but not across the
     stack discipline, and those are different guarantees.

     So L1 needs a **new traced root set**: a per-heap `mailbox_roots: Vec<Value>` (or
     equivalent) that `collect` walks and relocates, with a slot freed when a message is
     consumed. That is a real GC change, not a bookkeeping detail, and it costs a vector
     per process — partially offsetting the memory work in group B. The alternative is
     BEAM's actual design: let the message live in the receiver's heap and have collection
     scan the mailbox itself, which means taking the mailbox lock during collection.

     **Re-cost:** this is no longer "~300 lines and a copier". Budget a GC root-set change
     with the full GC_STRESS/GC_VERIFY/TSAN gate, and settle the root-set-vs-scan-mailbox
     question first.

  Work: a `copy_cross_heap(src: &Heap, dst: &mut Heap, v: Value) -> Value` mirroring
  `to_message`/`from_message`'s shape over every `Value` kind, the new traced root set
  from (3), an envelope variant naming the slot, and the two ends of the send/receive
  path. `Message` stays for running receivers and every dist send. Gate with GC_STRESS,
  GC_VERIFY, TSAN, and the differential fuzzers. Original entry: Local `send` today is *two* full
  copies through the wire-format `Message` (`Value → Message → Value`), with both
  intermediates becoming garbage. BEAM copies **once**, straight into the receiver's heap.
  **Feasibility confirmed:** `deliver_envelope` already takes the mailbox state lock and
  `wake_parked` hands the sender the receiver's `Box<Process>` — so the sender holds
  exclusive `&mut` on a parked receiver's heap, the same quiescence `trim_parked` uses.
  **Design constraint found:** the mailbox is *not* a GC root (today's queue holds
  heap-independent `Message`s), so a copied `Value` must be rooted in the receiver — push
  it onto the receiver's `roots` and carry the index in the envelope. That is sound
  because a collection while parked relocates roots *in place*, keeping indices valid —
  the identical invariant `Suspended` relies on (ADR-100 §8). Expected: halves the copy
  cost, i.e. up to ~40% of a 256-element send; **~0 on `pingpong`/`ring`**. Falls back to
  `Message` for running receivers and all dist sends. Risk: medium-high — a new
  cross-heap copier on a GC-visible path; full GC_STRESS + TSAN + differential gate.
- [ ] **L2 — heap fragments for running receivers** (BEAM's other half): copy into a
  fragment the receiver adopts at its next safepoint, removing the `Message` hop when the
  receiver isn't parked. Only after L1 proves the copier.

### B. Process memory floor (~4.5 KB → toward ~3 KB)

- [ ] **M1 — split `Heap` into hot core + lazily-boxed cold state.** Move the
  loader/checker/ns fields (`form_pos`, `imports`, `ns_known_names`,
  `module_exports_cache`, `known_ns_cache`, `check_dep_rec`, `compile_ns`,
  `current_file`, `dynamics`) behind one `Option<Box<ColdHeap>>` allocated on first use.
  A worker never touches them. Expected: several hundred bytes off `Box<Process>`'s
  1840 B, plus cache-density wins. Mechanical but wide; no semantic risk.
- [ ] **M2 — shared IC entries for sealed callees** (ADR-175 Stage 3, the BEAM
  export-table move). **Promoted over M3 by the analysis above:** every field an IC entry
  caches — `(sym, argc, epoch, callee, arm, env)` — is process-independent *within a
  runtime*, now that arms themselves are shared (ADR-175 Phase C). So the whole per-process
  IC table is arguably a per-runtime structure wearing the wrong hat: sharing it removes
  all 664 B **and** starts every spawned process warm, which is the latency half. The
  blocker is unchanged and real — the tables are read by JIT'd code on the hot path, so
  they need a lock-free design (`FastLink` is already `#[repr(C)]` plain data built for
  raw reads, and `jit_code_cache` is already an `RwLock` shared across processes, so the
  precedent exists) and the full TSAN/loom gate. This is the highest-value *and*
  highest-risk item in group B. A sealed (ADR-166) binding's resolution is process-independent, so
  its IC entry can live with the *shared arm* (atomics/ArcSwap; `FastLink` is already
  `#[repr(C)]` plain data). Removes most per-process IC slots (664 B floor item) *and*
  starts every fresh process warm — a latency win for spawn-heavy code too. Risk:
  concurrent lock-free structure read by JIT'd code — the one option needing the full
  TSAN/loom treatment. Decide against M3 first: they overlap.
- [ ] **M3 — direct-link sealed callees at compile time** (ADR-175 Stage 1). **Design
  analysed 2026-07-29; it does NOT deliver the memory win on its own.** An IC hit yields
  four things: `(callee, arm, env, callee_bases)`. For a sealed callee the first three are
  process-independent and permanent, so they bake into the shared chunk (a `OnceLock` per
  site, filled once per runtime). But `callee_bases` — which IC block the callee's sites
  use *in this process* — is inherently per-process (Phase A gives each process its own
  block), and it is currently a `HashMap<uid, (u32,u32)>` lookup. So a direct-linked call
  still needs a per-process lookup per call, and the 544 B of IC slots stays.
  **Prerequisite, and worth doing anyway:** give each shared arm a *dense per-runtime
  index* (it has a `uid` from a global counter today) so the per-process block table
  becomes a `Vec` index instead of a hash lookup. That makes the base resolution cheap
  enough to direct-link against, and shrinks `arm_ic_blocks` at the same time. Only then
  does M3 pay. Original entry: A call to a
  sealed name needs no site, no probe, no epoch check — bake the callee into the shared
  chunk (`OnceLock<Arc<CompiledArm>>` per site, shared across processes). Kills the IC
  slot AND the per-call validation for 67–99% of sites (measured range, user-heavy vs
  prelude-heavy). Constraint (recorded in ADR-175): must keep an arm fast path, since
  today's IC caches the resolved arm. **M3 subsumes most of M2's win at lower concurrency
  risk; evaluate first.**
- [ ] **M4 — process-shell recycling. DEPRIORITISED 2026-07-29 — premise measured wrong.**
  The claim below was "a large cut to the 7.6 µs spawn path". Decomposed, spawn is not
  where the time is:

  | stage | µs/proc |
  |---|---|
  | spawn + immediate exit | **0.90** |
  | + body work | 1.13 |
  | + park, 300k staying resident | **5.17** |

  So the spawn machinery is 17% of a spawn-and-park workload, and recycling addresses only
  the ~15.8 allocations inside it (~0.4 µs) — a ceiling near 8%. Meanwhile it is the
  *riskiest* item on this list (pid reuse, monitor/link references to a recycled mailbox,
  epoch discipline on a reused heap). Poor risk/reward until the 4.04 µs of park+residency
  is addressed. **Residency is the real cost, and it scales with per-process memory** —
  which is why `hibernate` made the same workload *faster* (0.68 → 0.45 s), and why M1/M3
  now outrank this. Original entry: Per-worker free-list of retired
  `(Box<Process>, Arc<Mailbox>)` shells; re-init the cheap fields on spawn. Precedent:
  ERTS allocator free-lists, every thread pool. Expected: spawn 15.8 allocs → ~2, a large
  cut to the 7.6 µs spawn path; floor unchanged. Risk: staleness bugs (epoch stamps
  already guard heap handles; the mailbox needs a generation bump for pid reuse safety).
- [x] **M5 — `(hibernate)` builtin — DONE 2026-07-29.** Measured: a process that works
  briefly then parks goes **8.18 → 4.94 KB/proc (−40%)**, and the run is *faster*
  (0.68 → 0.45 s at 100k processes) because the smaller footprint eases allocator
  pressure. `Heap::hibernate` collects, shrinks slabs + root vectors, and drops the IC
  tables, the block registry and the compiled-body cache (all pure caches, epoch-validated
  — shared arms re-install from the runtime). Tests in `tests/hibernate_test.blsp`.
  Original entry:
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

Revised after the payload measurement above moved L1 off the latency gap.

1. ~~**M5 `(hibernate)`**~~ — **DONE** (8.18 → 4.94 KB/proc).
2. **M1 cold-heap split** — promoted: shrinks the 1840 B `Box<Process>` for every worker,
   and *residency is latency* on this workload (78% of spawn-and-park is park+residency,
   which scales with per-process memory).
3. **M3 direct-link sealed callees** — removes the 664 B of IC tables per process on top.
4. **L3 measurement, then the fix** — split the 2 µs/message into lock / wake / match.
   This is what actually owns the `pingpong`/`ring` gap, and the measurement is cheap.
5. **L1 single-copy send** — big win for payload-carrying apps, ~0 for the benchmark
   latency rows. Worth doing on its merits, but it is not the latency fix.
7. **M2 / M6 / M7 / L2** — re-evaluate after the above land.

## Dead ends (measured; don't re-attempt without new evidence)

- Automatic IC-table drop on park (any policy — every-park or first-park): the latency
  rows pay 12–26%. The opt-in form is M5.
- Park-trim threshold tuning: zero effect at any setting.
- Capacity-1 slab first-touch: 110 B/proc, `bintree` +4.8%.
- Splitting shared code from per-process JIT-tier state: the motivating regression was a
  `make ab` single-core-pin artifact; sharing tier state is a net win.
