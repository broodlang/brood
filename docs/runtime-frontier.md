# Runtime frontier — the process/concurrency option book

The full option list for closing the remaining runtime gaps to the BEAM, written after
ADR-175 (shared compiled code) landed. Companion to the benchmark repo's `FRONTIER.md`
(which states the current *position*); this file is the **analysis and the menu** — what
we have, how Erlang/Go/Pony solve the same problems, and every option with its expected
win, cost, and risk, ordered for execution. Tick items off here as they land; move
anything measured-and-rejected to the dead-ends list at the bottom.

Current standing (**refreshed 2026-07-30** from the published run; the rest measured that
week):

| metric | Brood | BEAM/Elixir |
|---|---|---|
| `spawn-live` wall | **2.42 s** | **0.71 s** (3.4×) |
| live process (spawn-live, RSS/proc) | **~5.9 KB** (1.67 GB) | **~3.1 KB** (0.89 GB) |
| parked-process allocation floor | ~4.5 KB | ~2.7 KB (338 words) |
| spawn+park | ~7.6 µs, 15.8 allocs | ~1–2 µs |
| message round-trip (`pingpong`) | ~2.8× Elixir | — |
| ring hop | ~3.5× Elixir | — |
| parallel scaling (2→12 workers) | 2.5× of a 3.0× ceiling | BEAM 2.4× same box |

**Note on the 2026-07-30 benchmark refresh.** The `spawn-live` ports were corrected that day
and the numbers above are from the corrected row, so they are not comparable to earlier
figures cell-for-cell: .NET's `TaskCompletionSource` was resuming continuations *inline on the
setter's thread* (measured 1000/1000, none on the pool), so its "300k concurrent units" were
300k synchronous closure calls; and Node/Python/.NET collected results by returning values
into a pre-allocated array while Brood and Elixir pay for a second copied message per unit.
Both fixed. Also: Brood and Elixir now send a **contiguous** payload (vector / tuple) rather
than a cons list, matching the array ports — worth 11% of wall and ~0% of memory, which is
itself the useful datum: **the 1.67 GB is the process floor, not the messages.**

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
2. **L1 applies to essentially every real message, not just big payloads** — though by
   less than this table suggested. The "payload 0 ⇒ L1 buys ~0" reading was an artefact of
   the probe (its "payload 0" case still sent a 3-element vector; only a *bare atom*
   avoids the copy), and real protocols are tagged tuples. But the ~25% this section
   originally predicted for the latency rows did not materialise: L1 shipped at ≈−5% on
   `pingpong` and within drift on `ring`, because removing one of two copies of a
   2-element vector is a smaller share of a message than the decomposition implied. The
   win is real and grows to −35% with payload — see the L1 entry below.

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

- [x] **L1 — DONE 2026-07-29 (ADR-178). Single-copy send to a parked receiver.** A `send`
  to a local pid whose process is parked copies the value straight from the sender's heap
  into the receiver's, skipping the `Value → Message → Value` round trip. Access is
  licensed by ownership, not a lock on the heap: a parked process *is* its `Box<Process>`
  in `MailboxState::waiter`, so taking it under the mailbox mutex confers exclusive `&mut`
  — the same quiescence `trim_parked` uses. The copy parks in `Heap::msg_roots`, a new
  **traced slot table** (`roots` is the operand stack, truncated from ~109 sites, so a
  message awaiting a selective `receive` cannot live there). A running receiver, a remote
  pid, or a value the copier declines falls through to the unchanged wire path.

  **Measured, pinned, best-of-5, same commit — the win scales with payload**, because what
  is removed is marshalling, not scheduling: −3.2% at payload 0, −9.0% at 16, −24.2% at 64,
  −31.3% at 256, −34.9% at 1024. Roughly a third of a large-message send was pure
  marshalling.

  **The benchmark rows cannot show this** — `ring` sends a bare int and half of `pingpong`'s
  messages are a bare keyword, so those rows are scheduler round-trip with nothing to copy
  (`pingpong` ≈ −5%, `ring` within drift, everything else flat). That is a property of the
  suite's message shapes; don't read the rows backwards and conclude the change is inert.

  Two predictions in the pre-work design were wrong and are corrected here: the fast path
  does **not** miss because of direct handoff (instrumented at **100%** hit on both
  `pingpong` and `ring` via `BROOD_L1_STATS=1`), and the traced root set must be *lazily
  boxed* — an inline 24-byte `Vec` on every `Heap` cost `spawn` **+5.9%**, since a `Heap`
  is inline in `Box<Process>` where bytes run ~2:1 into RSS through mimalloc size classes.
  `Option<Box<Vec<Value>>>` at 8 bytes put `spawn` back to flat.

  Ongoing cost to watch: `copy_cross_heap` must mirror `to_message`'s value coverage, or a
  newly-carryable value quietly takes the slow route; and the copy now runs while holding
  the target's mailbox mutex, which is a real contention change for a many-senders-to-one
  fan-in.

- [ ] **L2 — heap fragments for running receivers** (BEAM's other half): copy into a
  fragment the receiver adopts at its next safepoint, removing the `Message` hop when the
  receiver isn't parked. Only after L1 proves the copier.

**A3 — a sent closure carries its CODE, and a retained one keeps paying for it (measured
2026-07-30).** A1/A2 both measure data payloads. A closure payload is a third case, and the
worst one: the same trivial thunk costs **48 bytes** built and held in-process but **436
bytes** (9×) once it has crossed a `send`. `closure_to_message` deep-copies every arm's
body forms per message and `closure_from_message` re-allocates them in the receiver, so a
process holding N received closures holds N private copies of code its runtime already
shares — ~670 objects each.

The bill lands as **GC**, not copy time, and only when the receiver *retains* the closure.
A dynamic supervisor is the canonical case: it must keep each child's `:start` thunk to
restart it. Attributed per-variant (own process, best-of-5, N=4 000 children, and via the
supervisor process's own `gc-stats`):

| supervisor keeps… | µs per `start-child` | collections | objects copied | GC pause |
|---|---|---|---|---|
| the record minus `:start` | 22.75 | 35 | 68 k | 13.7 ms |
| the full record | **64.75** | **324** | **2.69 M** | **189 ms of 278 ms** |

So retention costs ~42 µs/child — **two thirds of what a supervised `spawn-link` now
costs**, and the reason we sit ~13× off `DynamicSupervisor` after the O(N²) fix (devlog
2026-07-30). Tenuring is not broken; the objects are simply real.

- [x] **Share already-shared closure code on a same-runtime send (shipped 2026-07-30).**
  A closure whose value already lives in the shared code region crosses a local send by
  **handle**, not by copy: both processes read that region through the same `Arc`, which is
  what `spawn` already relies on for its thunk. In `copy_cross_heap` (the L1 parked-receiver
  path — 3996 of 4000 supervisor sends were parked-but-declined *because* of the closure,
  so this is where they were being lost), guarded by `region() == RUNTIME` +
  `shares_runtime_with` + `BROOD_NO_SHARE_FN=1` as the off-switch. Cross-node sends never
  reach it. Worth **2.4×** on supervised `start-child` (600 → 250 ms at N=8000) and takes
  the L1 hit rate on that path from 50% to 100%.

  In practice it covers the idiomatic case, because a closure that captures **no locals**
  is already a RUNTIME value: `(fn () (spawn-link (worker)))` is handed over by handle at
  **6 µs** per send, the same shape capturing a local copies at **54 µs**.

- **Rejected on measurement: promoting a local closure on send to make it shareable.**
  The obvious widening — `promote` any sent closure — was implemented and measured first,
  and it turns a transient closure (sent, used, dropped) into an append-only RUNTIME entry
  that needs a whole aging/drain/free cycle to reclaim instead of dying at the next minor
  GC. Peak RSS over N sent-and-discarded closures: **129 / 190 / 340 / 541 MB** at N =
  100k / 200k / 400k / 800k, against a flat 112–180 MB for the copy — growth proportional
  to closures sent, i.e. a leak in any long-running receiver, and `BROOD_RT_GC_FLOOR=64`
  (aging as hard as it goes) barely dented it. Restricted to already-shared closures the
  same run is flat (150 MB at N=800k). Do not re-attempt the wider rule until the RUNTIME
  collector reclaims promptly (ADR-091 stage 4); the blocker is reclamation, not the
  handoff.

**A4 — the `latency` row: our tail was 13× the BEAM's, and our median 15× (measured
2026-07-30; the placement half is now fixed — see A5).** The benchmark suite gained an open-loop row that holds a fixed 20,000 req/s
arrival schedule and makes every 20th request occupy ~500 µs of CPU, reporting percentiles
over the *other* 95% — i.e. what a busy handler does to everyone else. Offered load is 0.5
cores of twelve, so nothing is capacity-limited and the tail is scheduling, not saturation.

| | p50 | p99 | p99.9 | max | cores | CPU·s |
|---|---|---|---|---|---|---|
| Elixir | 8 µs | 59 µs | 98 µs | 601 µs | 1.9× | 5.28 |
| **Brood** | **121 µs** | **439 µs** | **1300 µs** | 2134 µs | 1.3× | 3.32 |
| Node | <1 µs | 451 µs | 561 µs | 1047 µs | 1.0× | 2.55 |
| .NET | 4 µs | 714 µs | 12627 µs | 15082 µs | 2.4× | 6.04 |

Two separate Brood problems, and they want different fixes:

- **The 121 µs median is per-message cost**, not scheduling — it is A1/A3 showing up again
  (a request here is spawn + send + a collector receive). Elixir's is 8 µs. Same family as
  `pingpong`/`ring`.
- **The 1300 µs p99.9 is scheduling.** A fat handler should cost its neighbours nothing on a
  12-core box at 0.5 cores of load, and it costs them milliseconds. The first hypothesis to
  test is **spawn placement**: processes are placed at spawn and not migrated
  (`docs/scheduler.md`), so a dispatcher spawning every handler can pile them onto its own
  worker, where one 500 µs handler then blocks the queue behind it. Note we also use only
  1.3× cores against Elixir's 1.9× on the same offered load — consistent with work landing on
  too few workers. Not yet investigated.

Worth stating plainly: Node, single-threaded, had a better p99.9 than Brood on this row.

**A5 — spawn placement dominated p50/p99, and the fix is one threshold (shipped 2026-07-30).**
The A4 hypothesis was right. Placement was *always* the spawner's own worker, so a dispatcher
spawning a handler per request piled every one onto a single queue, where one slow handler
blocked the rest; stealing rebalanced **12%** of 20,002 spawns and live migration never fired
(`migrations=0`). `pick_spawn_worker` now spills round-robin once the local queue has any
backlog (`BROOD_SPAWN_SPILL`, default 1) — one `try_lock` on our own queue, not the
O(workers) scan `assign_worker` runs.

| `latency`, medians over 11 runs | p50 | p99 |
|---|---|---|
| always-local (before) | 136 µs | 735 µs |
| **spill ≥1 (now)** | **27 µs** | **256 µs** |

**Correction (same day):** the first version of this table quoted p99.9 as 2902 → 562 µs, a 5×
win. That was a measurement error, not a result — the sweep selected the median run *by p99*
and then printed that one run's whole triple, so the quoted p99.9 was whatever that run
happened to score. Measured properly (per-metric medians), two 11-run samples of the *same*
binaries disagreed on p99.9 by 3× (3574 vs 3028 for the baseline; 1139 vs 3484 for the new
build). **p99.9 is not resolvable on this workload at this sample size**, so no claim is made
about it in either direction. p50 and p99 are robust: every measurement taken agrees.

Always-RR wins p50 and is **not** the answer: it costs `supervisor` **2.6×** (862 → 2223 ms)
by scattering the children of a request/reply spawn across workers. The threshold keeps
locality exactly where the argument for it holds — an empty queue — and spreads only when we
are demonstrably dispatching. Unpinned A/B against `af25b7b3`: `spawn` −10%, `pingpong`/`ring`/
`pfib`/`supervisor` flat, `spawn-live` neutral (a base-vs-base control put that row's own noise
floor at **20.6%**, so it cannot resolve anything smaller here).

What is left of A4 is the **median**, which is per-message cost, not scheduling: 27 µs against
Elixir's 8 µs. That is A1/A3 territory — a request here is spawn + send + a collector receive.

**A6 — the selective-receive scan took the mailbox mutex per candidate (shipped 2026-07-30).**
The tag pre-filter needs nothing but the envelope's tag, but the scan released and re-acquired
the mailbox lock for every rejected message, so a backlogged process paid a lock round-trip per
queued message on *every* selective receive. Now the scan skips all rejected candidates under
one lock hold. Per ref-pinned round trip against a tag-rejected backlog:

| backlog | before | after |
|---|---|---|
| 0 | 3 µs | 3 µs (no cost when there is no backlog) |
| 500 | 16 µs | **6 µs** |
| 2 000 | 48 µs | **13 µs** |
| 8 000 | 176 µs | **44 µs** |

Still **O(backlog)** — 4× cheaper per step, not a different complexity class.

**A7 — the receive-mark: a pinned-ref receive is now O(1) in the backlog (shipped 2026-07-30,
ADR-195).** Every synchronous call in the language is `(let (r (ref)) (send …) (receive
([:reply ^r v] …)))`, and each one walked the mailbox from the front. Envelopes now carry a
monotonic arrival sequence, `(ref)` stamps the sequence current when it mints, and a receive
whose clauses *all* pin that ref binary-searches to the first message that could carry it —
sound because a message enqueued before the ref existed cannot contain it. Per round trip
against a tag-rejected backlog:

| backlog | before today | after |
|---|---|---|
| 0 | 3 µs | 3 µs |
| 500 | 16 µs | **4 µs** |
| 2 000 | 50 µs | **4 µs** |
| 8 000 | 175 µs | **4 µs** |
| 32 000 | 653 µs | **4 µs** |

Flat — O(backlog) → O(1), 163× at 32k. **What is left**: a receive that pins *nothing* (a
server's main loop matching several tags) still walks its backlog; the tag filter and A6 make
that walk cheap, not free. BEAM solves it with a saved scan position per receive *site*, which
is a different mechanism from the per-ref mark and is the remaining piece.

**A8 — there is no reload leak; RSS growth under churn is allocator retention (measured
2026-07-30; this entry has been WRONG TWICE and is now measured properly).**

Two earlier versions of this entry each blamed something different, and both were artifacts of
the same mistake: differencing **time-based** runs. A time-boxed run does a *different number of
iterations* per configuration, and RSS tracks iterations — so every such comparison measured
the iteration count, not the thing under test. Corrected by fixing the **iteration count** and
repeating:

| workload (40 000 iterations, medians of 3) | RSS delta |
|---|---|
| 0 reloads | 94.1 MB |
| **1 000 reloads** | **91.7 MB** |

**Hot reload costs nothing measurable.** In isolation a `def` is ~500 B and does not scale with
process count (0 / 200 / 2000 live processes: 518 / 489 / 452 B per `def`). `:runtime-closures`
never moves in any of these workloads — 68 before and after, threshold 4096 — so the shared code
region is not implicated at all. The earlier "~75 KB per `def`" and "~270 MB/hour" figures were
wrong; there is no ADR-091 reclamation problem visible here.

**What does grow is churn, and it is the allocator holding pages, not us:**

| iterations (1 spawn + 1 round trip each) | RSS delta | per 1k iters |
|---|---|---|
| 20 000 | 51 MB | 2570 KB |
| 40 000 | 96 MB | 2406 KB |
| 80 000 | 158 MB | 1973 KB |
| 160 000 | 284 MB | 1775 KB |

Sublinear — the per-iteration cost falls as the run grows — but no plateau. Meanwhile the live
set is *tiny*: after 2.5 M spawns, 4 live processes and 59 KB of live local heap. Nothing is
retained by the language; the pages are held by mimalloc. `MIMALLOC_PURGE_DELAY=0` recovers
**17%** on this workload and **~2.3×** on a heavier-churn one (the soak, ~33 spawns/iteration),
for ~4% throughput.

**So the open item is not a leak — it is an allocator policy question**, and a smaller one than
this entry twice claimed. Worth doing: measure `mi_collect(true)` at a natural quiescence point
(e.g. when a runtime's live-process count drops sharply), and check whether pooling process-heap
arenas beats freeing them per spawn — that attacks the fragmentation at the source rather than
asking the allocator to give pages back afterwards.

**The lesson, since it cost two wrong entries:** never difference time-boxed runs. Fix the work,
repeat the run, compare medians.

### B. Process memory floor (~4.5 KB → toward ~3 KB)

- [x] **M1 — DONE 2026-07-29. `Heap` split into hot core + lazily-boxed cold state.** The
  loader/checker/namespace fields moved behind `Option<Box<ColdHeap>>`, allocated on first
  use, so a plain worker process never pays for them (`spawn` −13.6%). Continued by M1b/M1c
  below; together `Heap` went 1616 → 1120 B this day.

- [x] **M1b — DONE 2026-07-29. Checker state off the process.** `check_dep_rec` (208 B —
  four `HashSet`s) plus the two checker caches → one lazily-boxed `CheckHeap` (288 → 16 B),
  and `dbg_site_pos` gated to debug builds (32 B of release-dead weight). `Heap` 1616 →
  1376 B; **−47 MB on spawn-live** (−157 B/process). M1 left these inline because they are
  filled through `&self`; a `RefCell<Option<Box<_>>>` with a `RefMut::map` guard is the
  shape that works. Time-neutral, verified against measured per-row noise floors.

- [x] **M1c — DONE 2026-07-29. The old generation is lazily boxed.** `old: Slabs` (264 B,
  eleven `Vec` headers) → `Option<Box<Slabs>>` (8 B). Measured first: only **7 of 300,000**
  spawn-live processes ever promote, so it was pure struct overhead for the rest.
  **−73 MB on spawn-live** (1.847 → 1.774 GB), row time −1.7%. **Costs `fib` +2.6%** (real,
  survives a padding control) because `fib` promotes and pays an extra indirection on
  OLD-handle derefs; taken as a deliberate trade in favour of the process floor.
  `BROOD_GC_VERIFY` was what caught a blanket-rewrite bug here — aggregate collector walks
  need `old_opt()`, not the handle-deref `old()`.

### Where the per-process memory actually goes (measured 2026-07-29)

Staged differencing at 300k processes, baseline RSS subtracted — this is the allocation
profile the earlier entries said was missing:

| stage | B/process |
|---|---|
| fixed structs (`Process` 1208 incl. `Heap` 1120, `Mailbox` 168, `Suspended` 136) | 1512 |
| **hibernated bare shell** (spawn, `(hibernate)`, park) | **3037** |
| bare shell (spawn, park, never messaged) | 3467 |
| + one delivered message | 3630 |
| full `spawn-live` | 5916 |

Three things follow, and they redirect the remaining work:

1. **~1.5 KB per process is allocation that survives `hibernate`** — larger than the IC
   tables (~536 B). Struct-field shaving cannot reach it; it is `Box`/`Vec`/`HashMap`
   *allocations*, not inline bytes.
2. **`hibernate` reclaims only ~430 B on a bare shell (12%)**, not the ~40% recorded
   earlier. That figure came from processes that had *run* enough code to populate their
   caches; a process that only parks has little to give back. Both numbers are right for
   their workload — quote the workload with the number.
3. **Shrinking `Process` keeps paying, at roughly 1:1 — there is no size-class cliff to
   aim at.** An earlier version of this section claimed classes step ~256 B with a boundary
   just above 1272, so the next win was *binary*: cut exactly 184 B or get zero. **That was
   wrong**, over-read from single-sample padding runs. Measuring the allocator directly
   (`crates/lisp/tests/size_class_probe.rs`, 200k live allocations per size) shows mimalloc
   is near-linear in this range — cost ≈ size + ~16 B, with no step anywhere: 1024 → 1039,
   1152 → 1167, 1208 → 1215, 1280 → 1295, 1408 → 1423, 1536 → 1551.

   What *is* reproducible: padding `Process` by +192 B costs +277 B/process (3431–3466 →
   3710–3743, three runs each), i.e. ~1.44× amplification — page-level slack, not a class
   step. Shrinks this session came in at 0.65× (checker state, −240 B struct → −157
   B/proc) and 1.19× (old gen, −264 → −313). So budget struct bytes at roughly 1:1 and
   **keep shaving incrementally**; do not hold cuts back waiting for a threshold, and do
   not expect a jackpot from crossing one.

#### The bare shell, allocation by allocation (measured 2026-07-29)

A temporary size-histogram in the counting allocator (an atomic per allocation — for a
measurement build only; removed, redo it the same way) gives the **complete** per-process
profile of `spawn` + park at 300k processes. It sums to ~3019 B against 3037 B measured,
so this is the whole of it — nothing is unattributed any more:

| bytes | per process | what |
|---|---|---|
| 1184 × 1 | 1184 | the `Box<Process>` (the `Heap` is inline in it) |
| 256 × 2 | 510 | one is `vm_call_ics` (4 × 64 B) |
| 160 × 2 | 319 | one is `vm_fast_links` (4 × 40 B) — **no longer allocated on this path since 2026-08-18**, see below |
| 184 × 1 | 184 | `Arc<Mailbox>` (168 B + Arc header) |
| 180 × 1 | 179 | |
| 84 × 2 | 167 | one is `arm_ic_blocks` |
| 136 × 1 | 136 | `Suspended` (the parked continuation) |
| 116, 96, 56, 32, 24, 16 | 340 | |

Identities come from differencing the same workload with `(hibernate)`, which drops exactly
one 256, one 160 and one 84 — so **the IC tables really are ~500 B/process**, confirming the
536 B estimate by direct measurement rather than by adding up `size_of`s.

> **Updated 2026-08-18 — the `vm_fast_links` row above is now zero for a parked process.**
> The mirror is allocated by its only writer (`Heap::fastlink_slot_grown`) instead of being
> pre-grown in `vm_arm_block`, so a process that never JIT-links a site never allocates it:
> 19,968 of 20,001 `spawn-live` units allocate **0** slots instead of 14. Measured with
> `(mem-bytes)`, the saving is **192.6 B/process** — and the accompanying RSS drop
> (6364 → 6093 B/process, −4.3%) tracked that allocation *fully*.
>
> **Size these items at the parked state, not at teardown.** A teardown slot count read
> 14 sites × 48 B = 672 B and was 3.5× too high, because a unit parked in `receive` has
> entered only ~4 call sites' worth of arms; it reaches 14 only after running its body, by
> which time most units have died and freed. Peak memory is set by the parked state, since
> that is the state all N processes are in at once. Confirmed by construction: adding 24
> call sites to the unit body moved the saving to 1345.6 B ≈ 193 + 24 × 48, i.e. exactly
> **48 B per call site entered**. (An earlier version of this note inferred a "1:0.35
> allocation-to-RSS discount" from that 672 B and told the next reader to size the remaining
> IC fixes at a third of face value. That was a measured-vs-inferred comparison error and is
> **retracted** — there is no discount. `(mem-bytes)`/`(mem-peak)` are the instrument for an
> allocation question; RSS answers a different and noisier one.)

What this changes:

- **M2b is the single biggest reducible item** at ~500 B/process (≈150 MB at 300k), second
  only to the `Box<Process>` itself, which is irreducible without M7. *(2026-08-18: the
  mirror half of that ~500 B is shipped — see the note above. What remains reducible here is
  `vm_call_ics` + `vm_global_ics`, which `vm_arm_block` still grows eagerly per activated
  arm — `vm_call_ics` at a measured 64 B per site; `vm_global_ics` is unmeasured, so size it
  before costing any work against it.)*
- The **second 256 / 160 / 84 of each pair survives hibernate** and is still unidentified by
  *name* (though now pinned by size). Identifying them needs allocation backtraces, not more
  differencing — that is the next measurement, and it is cheap: tag the histogram at
  process-construction boundaries.
- Roughly 950 B/process survives `hibernate` across the small buckets. No single item there
  is worth a risky change; they are only worth attacking as a group, if at all.

> **Hard constraint on M2b, found 2026-07-29 before writing code: IC state may NOT hang off
> a `CompiledArm`.** The obvious design — give each shared arm a `Box<[AtomicUsize]>` of
> per-site slots, since ADR-175 already shares arms per runtime — is **unsound for prelude
> arms**, which are the common call target. `SHARED` is a process-wide
> `LazyLock<SharedBundle>` and every `Interp::new()` does `Arc::clone(&SHARED.code)` while
> constructing its **own** `RuntimeCode`. So a prelude `CompiledArm` is shared across
> *runtimes*, not merely across the processes of one runtime — and an IC entry caches a
> *global* resolution, which is per-runtime by construction. Hanging slots on the arm would
> let one runtime's resolutions serve another's calls: a wrong-callee miscompile, not a
> stale-cache warning.
>
> Shared IC state must therefore live on **`RuntimeCode`**, keyed by `(arm_uid, site)`.
> That reintroduces the per-activation base lookup as the design problem: today
> `vm_arm_block` is a per-*process* `HashMap` probe per activation (no synchronisation);
> a per-*runtime* table needs that probe to stay lock-free, or to be resolved once per
> activation and carried — `BcFrame::ic_bases` already carries exactly this, so the
> resolution point exists.

- [ ] **M2b — sharing the IC tables. Two findings, one of which corrects the other.**
  **Partly shipped 2026-08-18: the `vm_fast_links` mirror is no longer allocated eagerly.**
  `vm_arm_block` pre-grew *both* tables by the arm's total `nsites` on first entry, whether
  or not this process would ever execute those sites — and a spawn-once-call-once process
  never does. The mirror now grows in `Heap::fastlink_slot_grown`, its only writer. That is
  not sharing; it is *not allocating what is never written*, and it took the cheaper half of
  M2b's win (192.6 B/process measured) **without** touching the read protocol below. Safe by
  construction rather than by luck: every reader already tolerated a short table — the VM
  probe reads `.get(abs)`, both publish paths `.get_mut(abs)`, and JIT'd code bounds-checks
  `site < len` against a length it re-fetches after each Brood→Brood call precisely because a
  cold nested call may grow and realloc this table. A missing slot reads exactly like an
  unpublished one. Time-neutral at both ceilings (`fib` is the load-bearing row — the in-IR
  fast link is worth ~20% there, so +0.0% is what proves linking still happens).

  **What is left of M2b is the fat table**, `vm_call_ics`, still grown eagerly per activated
  arm at 64 B/site — plus `vm_global_ics`. Sharing it faces the unchanged difficulty below.
  Two smaller follow-ons were named alongside the mirror work: shrink `CallIcEntry` 64 → ~48 B,
  and share entries for frozen callees. **Cost them at the parked state (48 B per call site
  *entered*, ~4 while parked), not at a teardown slot count** — see the 2026-08-18 note in the
  allocation profile above, which retracts the earlier "size these at a third of face value"
  caution as a measurement error.

  **Still true and useful:** `vm_call_ics` is *never read raw by JIT code* — only
  `vm_fast_links` is, via `vm_fast_links_base()`. Verified by grep: no reference to
  `vm_call_ics` anywhere under `jit/` or `jit_lower.rs`. So the fat table carries none of
  the stable-address / use-after-realloc hazard, and sharing it needs no block allocator.

  **CORRECTED 2026-07-29 (same day, before any code was written):** an earlier version of
  this entry claimed that after the fast-link collapse the fat table is *cold*, and
  proposed an `RwLock<HashMap<(arm_uid, site), CallIcEntry>>` on that basis. **That premise
  is wrong.** The collapse made `CallIcEntry` cold only for the **JIT** path
  (`vm_call_ic_fast_link` now returns from the 40-byte mirror). `vm_call_ic_probe` — which
  reads the fat table — is the primary IC hit path in the **VM dispatcher**
  (`dispatch.rs`, the `call_ic_hit` counter), i.e. hot for *every interpreted call*. An
  `RwLock` + hash probe there would regress every un-JIT'd call site in the system.

  The accurate picture: **both tables are hot, on different engines.** The mirror is hot
  for JIT'd calls, the fat table for interpreted ones. Sharing either needs a lock-free
  read path, not a lock — which is the original difficulty, undiminished. What genuinely
  improved is only that the fat table has no raw-pointer hazard, so its read path can be a
  normal atomic protocol rather than a stable-address block allocator.

  **Settle M2b before M3 — they overlap.** Cost it fresh before starting: worth ~500 B/process
  (measured, see the allocation profile above) (≈77 MB at 300k) plus a warm start,
  against a lock-free structure on the hottest path in the interpreter. `CallIcEntry` is
  confirmed `Send + Sync` (compile-time assertion). Entry content is already enforced
  process-independent (`vm_call_ic_put` refuses a movable callee or LOCAL env), and the
  epoch already lives per-runtime in `Arc<RuntimeCode>`, so the *semantics* of sharing are
  settled — only the read protocol is open.

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
2. ~~**M1 cold-heap split**~~ — **DONE** (`spawn` −13.6%).
3. ~~**L3 selective-receive pre-filter**~~ — **DONE** (backlog 500: ~420 ms → 34 ms).
4. ~~**L1 single-copy send**~~ — **DONE**, ADR-178. Confirmed as predicted: a large win for
   payload-carrying apps (−35% at 1024), ≈−5% on `pingpong` and flat on `ring`. It was
   never the latency fix, and shipping it did not make it one.
5. **M2 shared IC tables** — the largest remaining per-process item (664 B) *and* a warm
   start. Highest value and highest risk; needs a lock-free design plus TSAN/loom.
6. **M3 direct-link sealed callees** — blocked behind M2.
7. **M4 / M6 / M7 / L2** — re-evaluate after the above land. What still owns the
   `pingpong`/`ring` gap is the per-message fixed cost (mailbox mutex, `wake_parked`,
   re-enqueue, one matcher activation), not the copy.

## Dead ends (measured; don't re-attempt without new evidence)

- Automatic IC-table drop on park (any policy — every-park or first-park): the latency
  rows pay 12–26%. The opt-in form is M5.
- Park-trim threshold tuning: zero effect at any setting.
- Capacity-1 slab first-touch: 110 B/proc, `bintree` +4.8%.
- Splitting shared code from per-process JIT-tier state: the motivating regression was a
  `make ab` single-core-pin artifact; sharing tier state is a net win.
