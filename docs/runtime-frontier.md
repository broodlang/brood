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

### B. Process memory floor (~4.5 KB → toward ~3 KB)

- [ ] **M1 — split `Heap` into hot core + lazily-boxed cold state.** Move the
  loader/checker/ns fields (`form_pos`, `imports`, `ns_known_names`,
  `module_exports_cache`, `known_ns_cache`, `check_dep_rec`, `compile_ns`,
  `current_file`, `dynamics`) behind one `Option<Box<ColdHeap>>` allocated on first use.
  A worker never touches them. Expected: several hundred bytes off `Box<Process>`'s
  1840 B, plus cache-density wins. Mechanical but wide; no semantic risk.
- [x] **M2a — DONE 2026-07-29. One fast-link representation, not two.** `CallIcEntry.fast`
  (a 32-byte `Cell<Option<(code, nslots, env)>>`) and the `FastLink` mirror held the *same
  fact*, written in lockstep — the code said so itself. `FastLink` already carries
  `sym`/`argc`/`epoch`, so the VM's hot probe never needed the fat entry: it now reads the
  one 40-byte flat table that JIT'd code reads, and a hit never touches `CallIcEntry` at
  all. `CallIcEntry` 96 → 64 B.

  Measured: **−157 B per live process** (spawn-live 1.94 → 1.89 GB, −47 MB over 300k), and
  the compute rows *improve* — pfib −3.5%, collatz −2.7%, fib −1.3%, bintree −0.8%,
  nqueens flat, spawn-live −0.8%, `spawn` +1.9%. Removing the memo was expected to cost
  the hot recursive call a second table touch; it didn't, because reading one 40-byte flat
  slot beats loading a 96-byte entry.

  Method note: the *sweep* reported `spawn` +5.8% and `collatz` +2.8%; solo re-runs gave
  +1.9% and **−2.7%**. Believe the solo run, as `ab-bench` says.

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
| 160 × 2 | 319 | one is `vm_fast_links` (4 × 40 B) |
| 184 × 1 | 184 | `Arc<Mailbox>` (168 B + Arc header) |
| 180 × 1 | 179 | |
| 84 × 2 | 167 | one is `arm_ic_blocks` |
| 136 × 1 | 136 | `Suspended` (the parked continuation) |
| 116, 96, 56, 32, 24, 16 | 340 | |

Identities come from differencing the same workload with `(hibernate)`, which drops exactly
one 256, one 160 and one 84 — so **the IC tables really are ~500 B/process**, confirming the
536 B estimate by direct measurement rather than by adding up `size_of`s.

What this changes:

- **M2b is the single biggest reducible item** at ~500 B/process (≈150 MB at 300k), second
  only to the `Box<Process>` itself, which is irreducible without M7.
- The **second 256 / 160 / 84 of each pair survives hibernate** and is still unidentified by
  *name* (though now pinned by size). Identifying them needs allocation backtraces, not more
  differencing — that is the next measurement, and it is cheap: tag the histogram at
  process-construction boundaries.
- Roughly 950 B/process survives `hibernate` across the small buckets. No single item there
  is worth a risky change; they are only worth attacking as a group, if at all.

- [ ] **M2b — sharing the IC tables. Two findings, one of which corrects the other.**

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

  Cost it fresh before starting: worth ~256 B/process (≈77 MB at 300k) plus a warm start,
  against a lock-free structure on the hottest path in the interpreter. `CallIcEntry` is
  confirmed `Send + Sync` (compile-time assertion). Entry content is already enforced
  process-independent (`vm_call_ic_put` refuses a movable callee or LOCAL env), and the
  epoch already lives per-runtime in `Arc<RuntimeCode>`, so the *semantics* of sharing are
  settled — only the read protocol is open.

- [ ] **M2b — shared IC tables across a runtime's processes** (ADR-175 Stage 3, the BEAM
  export-table move). **Blocker found 2026-07-29 by reading the code, and it is bigger than
  "needs a lock":** `vm_fast_links_base()` hands JIT'd code a **raw pointer** into the
  table, sound today only because `SAFETY: single-threaded per process` — nothing can grow
  or clear it while an arm runs. Sharing the table across processes voids that outright: a
  peer growing it reallocates under a live raw pointer (use-after-realloc, not merely a
  torn read). So a shared table must be **block-allocated with stable addresses** — each
  arm gets a contiguous block, allocated once per *runtime*, that never moves; the JIT
  then holds a pointer to that arm's block. `boxcar` is the in-tree precedent (it already
  backs the shared RUNTIME code region for exactly this stable-ref reason).

  What is now settled in favour: entry content is *enforced* process-independent
  (`vm_call_ic_put` refuses a movable callee or a LOCAL env), the epoch already lives
  per-runtime in `Arc<RuntimeCode>`, and after M2a the thing worth sharing is a 40-byte
  `#[repr(C)]` plain-data slot rather than a 96-byte entry holding a `Cell`. Publication
  can be epoch-last-release, and since all writers resolve identically, a racing writer
  writes the same bytes.

  Note the base assignment is **not** independently shippable: bases are per-process
  first-touch today (`vm_arm_block`), and making them runtime-global while tables stay
  per-process would size every process's table to the runtime's *total* site count — a
  process touches only ~4 sites. Runtime-global bases and the shared table must land
  together.

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
