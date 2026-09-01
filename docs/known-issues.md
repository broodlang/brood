# Known issues

The condensed record of every bug this runtime has had: what it was, how it was found, how
it was fixed, and **which test now guards it** — so a recurrence is recognizable rather than
rediscovered. Deeper rationale lives in the cited `## ADR-NNN`
([decisions.md](decisions.md)) or topic doc; the day-by-day narrative is in
[devlog.md](devlog.md); the scheduler race has a long-form writeup in
[claude-demo-findings.md](claude-demo-findings.md).

## Filing an entry

**1. Take the next free number — check first.** Grep the headings, do not eyeball the index:

```bash
grep -oE '^## KI-[0-9]+' docs/known-issues.md | grep -oE '[0-9]+' | sort -n | tail -1
```

Two sessions numbered different issues **KI-70** within minutes on 2026-08-27 because both
read the index instead. A duplicate is worse than a dangling reference: every later citation
of that number is ambiguous *forever*, including in commit messages and release tags, which
cannot be corrected. `doc_refs::no_two_entries_claim_the_same_number` now fails the build on
a collision — if you hit it, **renumber the newer entry**, and check what already cites the
older one (`grep -rn KI-N`).

**2. Write both halves.** An index row *and* a `## KI-N — <one-line symptom> <status> <date>`
section. The index row alone is not enough — the file's own header sends the reader to the
section, and `doc_refs::every_ki_reference_resolves_to_a_known_issue` enforces it.

**3. The section answers five questions, in this order.** They are the questions the next
person actually has, and every entry that skipped one cost time later:

| | | why it matters |
|---|---|---|
| **Symptom** | what was observed, verbatim — the error text, the failing test name, the wrong value | this is what a recurrence will look like; it is the only part that is greppable from a future failure |
| **Cause** | the actual mechanism, not the layer | "a rename wave" is not a cause; "`check_into_inner` returned for any form that was not a `Pair`" is |
| **Why it survived** | which gates were green while this was broken, and *why* each one could not see it | the most valuable section, and the one most often missing — it is where the *next* bug is hiding |
| **Fix** | what changed, and any deliberate non-fix with its reason | a recorded non-fix stops the next person re-litigating it |
| **Guard** | the test that now fails if this returns, **sabotage-verified** | an unverified guard is a guess; see below |

**4. Sabotage-verify the guard, and say so in the entry.** Re-introduce the bug on purpose,
confirm the new test goes red, restore, confirm green. Write the observed red output into the
entry. This is not ceremony — KI-68's whole lesson is that a gate nobody broke on purpose may
never have been able to fail. Guards recorded without this have been wrong before.

**5. Statuses.** `✅ fixed <date>` · `☑️` for retracted / not-a-bug / no-longer-reproduces /
superseded (keep the entry — a wrong diagnosis is worth recording) · `⚠️ watching` **only**
when it genuinely cannot be reproduced on demand, and only with the diagnostic armed and
named. Anything else is open, and per `CLAUDE.md` an open bug is the work — before any
feature.

## Where a finding goes

- **A bug in the runtime, toolchain or stdlib** → here, plus a dated
  [devlog.md](devlog.md) entry for the narrative.
- **A design choice** → an ADR in [decisions.md](decisions.md); cite it from here.
- **A trap in how to measure or verify something** → [handoff.md](handoff.md), which is
  replaced each session and is what someone reads cold.
- **A user-visible break** → [CHANGELOG.md](../CHANGELOG.md), under the release that ships it.

## Before you call the tree green

```bash
make green        # completed CI runs + the local gates `make check` skips
make green-all    # …plus the examples and stress corpus gates
```

Do not hand-read the run list. A **cancelled** CI run is not evidence of anything: every run
is cancelled by the next push to the same ref (the workflow's `concurrency` group), so a busy
day shows a wall of cancellations with no red in it while the last several *completed* runs
were all failures — which is exactly how KI-68/KI-69 stayed hidden for two days. And
`make check` is clippy + tests; CI additionally runs `nest format --check`, the zero-warning
checker gate, and the examples and stress gates, so it is possible to be entirely green
locally and red in CI (v0.14.0 was tagged that way). `make green` covers both halves, scopes
the CI half to the **CI workflow** (`Release` succeeds on commits whose CI failed), and says
**"no verdict"** rather than "green" when CI has concluded nothing recent.

And per `CLAUDE.md`, "passed once" is not green for anything touching concurrency, the
scheduler, dist, GC or the JIT — run it repeatedly.

## Index — status per issue (⌘F the `KI-N` to jump)

| # | What | Status |
|---|---|---|
| KI-100 | **a ~5-6% compute regression: two clean branches, a slow merge** — every benchmark compute row 4-10% slower than the published 0.19.1 column, checksums unchanged. Separately, boot +2.8ms (+14.5%), which tracks the stdlib growing (5199 -> 5332 image bindings) and reads as feature cost | ⚠️ **OPEN 2026-09-01** — **bisected**: the first bad commit is the MERGE `0f57e30b`, and both parents are fast (`2dc7d2e6` ADR-302 data-first, ratio 1.016; `25a558d4` mainline with §7.5 JIT increments 1-3, ratio 0.992; the merge 1.061, reproducible). `std/` is IDENTICAL across the merge, so the delta is kernel-side: ADR-302's std is fast on the old kernel and the new kernel is fast on the old std — only the combination is slow. Increment 3 excluded (`BROOD_NO_XCALL` doesn't close it); present at **tier 1 too** (1.046), so not codegen — **§7.5 costs ~5 points on ADR-302's std and 0 on the old std**; RootsBuf (`115faead`) alone reproduces about half of that (1.030 -> 1.052), confirmed contributor but not the whole story. **MECHANISM FOUND**: instruction-fetch pressure, not work — icache misses +48%, iTLB +96%, dcache FLAT, instructions only +1.25%. §7.5 emits more code per arm; ADR-302 doubles the arms that lower (158 vs 76); together they spill the L1 icache/iTLB. `fib` (small footprint) is unaffected at 1.0010. Fix direction: less code per arm, or JIT code locality (hot/cold split, huge pages). Probe harness in `target/ki100/` |
| KI-98 | **`process_limit_test.blsp:114` ("the handler can drain and clear the bound — the process recovers") timed out at 30 s under a full `nest test`, twice in five runs** — the flooded worker's `[:recovered …]` never arrived: either the parked receiver never re-entered `receive_match` after its breach armed (a missed wake), or the E0046 raise/drain hung. Full-suite context only — **falsified 2026-09-01**, see the status cell | ⚠️ **WATCHING 2026-08-31** — not reproducible on demand: 16 solo runs green, 10 runs under 8-way CPU load green; only full-suite context shows it (~3/8 — the third sighting was a full tree-walker half, so it is engine-independent). Sighted on a tree carrying the KI-91/92 mailbox fixes, but those touch the scan/consume path, not delivery/wake, and a 3-run pre-fix control neither fired nor rules anything out (samples too small either way). If it recurs: the run's own log names it; capture whether the worker's E0046 was raised at all (`BROOD_SCHED_DBG=1` run/park lines for the worker pid is the next probe). **Sighting 2026-09-01: CI's `make gcstress` step, run 5b20b307** — that step runs the file ALONE (a debug build, `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`), so the "full-suite context only" reading is wrong: it fires solo given the right timing, and GC stress on a loaded 2-core runner supplies it. Not reproducible here — 25/25 green under the same flags, and `make gcstress` clean in a full local pass — so the missing ingredient is machine load, not suite context. That makes CI's gcstress step a cheaper repro surface than a full suite run |
| KI-91 | **`receive`'s consume path removed the matched message by a STALE INDEX** — a clause `:when` guard running a consuming nested `receive` shifts the queue with the mailbox lock released (the documented `reinsert_at_seq` hazard), and the *match* path still did `queue.remove(*i)`: a neighbouring message was silently deleted while the matched one stayed queued to be delivered again | ✅ **FIXED 2026-08-31** — a candidate's identity is its arrival `seq`: the consume path re-identifies by seq (O(1) fast path, binary-search fallback), and each scan-loop top re-anchors the cursor against the last examined seq. Guard `tests/receive_consume_test.blsp` case 1, sabotage-verified (`[:dup 1]` + a lost `[:tail 2]` with `remove(*i)` restored) |
| KI-92 | **an L1-delivered `nil` message aliased a FREE msg-roots slot** — the slot table's free sentinel was `Value::Nil`, i.e. slot *content*, and `nil` is a legal message: the next delivery reused the slot and two queued envelopes read one slot (the receiver saw the second message where `nil` belonged and `nil` where the second belonged) | ✅ **FIXED 2026-08-31** — freeness is tracked out of band (`MsgRoots { slots, free }`), which also makes `msg_root_add` O(1) instead of an O(live) scan under the sender-side mailbox lock; a double-free now trips a `debug_assert`. Guard `tests/receive_consume_test.blsp` case 2, sabotage-verified |
| KI-93 | **the net reactor thread's death was SILENT** — no `catch_unwind`, no restart, and `Reactor::cmd` discarded the channel error, so after any reactor panic or fatal `poll` error every `tcp-send`/`tcp-listen` kept returning `Ok(())` into a dead channel, no `[:tcp-closed]` was ever emitted again, and every socket-owning process parked in `receive` forever with zero diagnostics | ✅ **FIXED 2026-08-31** — the death is loud and terminal: a `catch_unwind` wrapper runs `reactor_died` (a `REACTOR_DOWN` flag, one stderr line, and a registry sweep failing every socket at its owner with `[:tcp-error]` + `[:tcp-closed]`); `connect`/`listen`/`tls-listen`/`tls-request`/`send` gate on the flag and the creators re-check after insert. Restart is deliberately NOT attempted — the `Poll`, fd registrations and TLS state died with the thread. Guard `crates/lisp/tests/net_reactor_death.rs`, sabotage-verified |
| KI-94 | **a green process's death ORPHANED its OS subprocesses** — `subprocess::close`'s only caller was the `proc-close` builtin and `retire_pid_tail` had no subprocess counterpart to `close_process_sockets`, so an owner that exited without closing leaked the OS child (never killed), its registry entry (forever), and both reader threads (draining into a dead pid) | ✅ **FIXED 2026-08-31** — `Proc` records its owner (the spawn subscriber) and `retire_pid_tail` calls `close_process_procs(pid)`: Erlang port semantics, a child dies with its owner (a deliberate semantic change). Guard in `tests/proc_test.blsp`, verified red (`:wrote`) on the pre-fix binary |
| KI-95 | **`promote` forwards only closures/envs — DAG-shaped data is duplicated once per referrer, exponentially with nesting** — `heap.rs`'s `PromoteForward` comment reasons "acyclic ⇒ a finite tree to re-copy", but acyclic ≠ tree: immutable path-copying code produces DAGs everywhere, so every `def`/`spawn` of a value with shared substructure copies the shared part per reference into the append-only RUNTIME region, `2^n` with sharing depth, no cap | ✅ **FIXED 2026-08-31** — `PromoteForward` forwards pairs/vectors/maps/strings too (mirroring the GC flush tables), keyed on the handles' canonical identity (also closing a latent nursery/old `index()` collision in the closure/env tables); long spines stride-register (every 8th cell) so a bulk `def` stays cheap while growth stays O(n). Guards: 5 `promote_sharing_tests` (17 cells where pre-fix copied 131 071). Measured: `sort`/`spawn`/`startup`/`spawn-live`/`supervisor` all floor-level; `spawn`/`supervisor` **fewer** instructions than base |
| KI-96 | **a remote monitor's `PENDING_REMOTE` entry survives its own `[:down …]`** — nothing removes the entry when the watched remote target dies and the peer's DOWN arrives, so (a) a long-lived watcher leaks one entry per dead remote monitor, and (b) a later node-down fires a SECOND `[:down mref pid :noconnection]` for an mref that already delivered — breaking the one-shot guarantee a `gen/call`-style pinned receive relies on | ✅ **FIXED 2026-08-31** — the DOWN now rides a dedicated `Frame::Down` (wire v7) instead of an ordinary `Send`, giving the watcher's node the hook the entry lacked: `deliver_remote_down` retires the pending entry, then delivers. Guard: `a_delivered_remote_monitor_does_not_fire_again_on_node_down` (two-node; sabotage-verified — retire disabled reproduces `SECOND-DOWN-BUG :noconnection`) |
| KI-97 | **consolidated hardening gaps from the 2026-08-31 stability audit** — pre-auth handshake trickle DoS (per-read timeout, no total deadline: 128 slow sockets silently disable inbound dist), untimed blocking calls on scheduler workers (`proc-send`, `os/run-process` with inherited stdin, `read-line`, `%node-connect` DNS), thread-spawn panic classes (`Once` poisoning in the timer, `LIVE_EXECUTORS` stranding in `ensure_workers`, gossip thread-per-peer unwinding the dist acceptor), and smaller items | ⚠️ **OPEN 2026-08-31; item 1 FIXED, items 1 + 3 FIXED, item 2 three-of-four 2026-09-01** — the section carries the full list with file:line; none observed in the wild, all confirmed by reading. **Item 1 (the pre-auth handshake trickle DoS) is closed**: a whole-handshake `Deadline` shim on both sides + a rate-limited shed warning, sabotage-verified. **Item 2**: `run-process`'s inherited stdin (a `git` credential prompt pinned a worker uncatchably) and `%node-connect`'s unbounded DNS resolve are both closed, each sabotage-verified; `proc-send`'s `write_all` now goes through a per-child writer thread (bounded queue, `dist`'s shape) — only `read-line`'s stdin lock remains, and that one is ADR-059 Phase 2 rather than a patch. **Item 3 fully closed**: the timer's poisoning `Once` (one EAGAIN broke every `sleep` forever), `ensure_workers` stranding `LIVE_EXECUTORS` above reality, the dist acceptor dying from a refused thread, and the poison-intolerant locks. **Item 4 fully closed**: the pre-auth 64 MiB allocation, unbounded wire-symbol interning, the ADR-232 dedup set, the edge-triggered accept drain, `tls_request`'s untimed connect + close-before-connect race, the never-reaped half-closed stream, `record_remote_link`'s missing liveness check, and unreaped sysmon subscriptions. **The only thing left in KI-97 is `read-line`'s stdin lock**, which is ADR-059 Phase 2 — a feature, not a patch |
| KI-88 | **one spawn of a warm burst is created, promoted, registered — and never scheduled** — exactly one reader of a 50-process burst never executes its first instruction; no death line, and the collector times out. Gates `BROOD_TW_REENTRY`'s default (60× on the viral defer shape, measured and waiting) | ⚠️ **WATCHING 2026-08-31 — DORMANT.** Seen many times, root cause never found, and no reconstructable binary exhibits it: 10/10 pass at `62eac84c` with the router confirmed live, on top of session 4's 15/15 + 8/8 (incl. a pristine rebuild of the commit that failed 3/3 hours earlier). One candidate mechanism — `run_one`'s unprotected post-quantum tail, whose unwind produces this exact signature — was found and **closed** in session 5, so a future sighting is known not to be that. Next sighting: PRESERVE THE BINARY, arm `BROOD_SCHED_DBG=1`, take a core in the window |
| KI-99 | **`a_dropped_send_to_an_unregistered_name_warns_once` failed try 1 under a full `make test`** — B warned 0 times instead of 1, with `dist: incoming connection failed: failed to fill whole buffer` on B's stderr: the handshake hit EOF mid-frame under full-suite load, so the inbound send that should have been dropped-and-warned never arrived | ⚠️ **WATCHING 2026-08-31** — retry-absorbed (try 2 passed), 6/6 green solo after. One sighting, but the failure output was **captured** this time (KI-80's lesson), and it names the mechanism: a connection that never completed, not a dedup miscount. If it recurs, the question is why the handshake EOFs under load — `MAX_HANDSHAKE_FRAME`/`accept_link` timeout interaction is the place to look, and KI-97 item 1 touches that code |
| KI-87 | **The checker diverged — `nest run` at 54 GB, three 19 GB `types::` test processes.** `InferGuard::enter` ended in `.then_some(InferGuard(sym))`; `bool::then_some` builds its argument eagerly, so a REFUSED entry built and dropped a guard whose `Drop` removed the in-flight symbol's mark — every cycle refusal un-guarded the symbol it refused (latent since 2026-07-07). The demand walk consulting a callee's inferred sig (`0f64f600`) made `require-one` ⇄ `%require-await` nest without bound, one stack segment per level | ✅ **FIXED 2026-08-29** — the guard is constructed only on the success path (`entered.then(\|\| InferGuard(sym))`). Guards sabotage-verified: a refused third `enter` stays refused, and the mutually recursive pair reproduces the exact `mmap failed to allocate stack` OOM under `ulimit -v` with the bug restored. Build uncapped, run capped |
| KI-86 | **`runtime_collector`'s three promotion tests failed under `cargo test`** — `expected ≥3000 promoted closures, got total=231`, with the count-based collector switched OFF by the test and one closure promoted per iteration | ⚠️ **WATCHING 2026-08-29** — precondition removed, mechanism inferred: `BROOD_RT_GC_FLOOR` is read once per process (`OnceLock`), two tests in the binary `set_var` it to 128/256, and under plain `cargo test` the leaked floor armed the `Interp`'s scheduler WORKER heaps (which share the runtime region) — a worker safepoint aged the runtime and the main heap's `cur_code()` count dropped. Fix: `Heap::set_rt_gc_floor` per test heap, no env. Not reproducible on demand on a quiet box (a worker has to wake); 4/4 green under load since |
| KI-85 | **The checker produced a FALSE POSITIVE — its one hard invariant.** `(takes-str (first (fold (fn (a x) (cons x a)) [0] ["t"])))` warned `expects string, got 0` on a value that was the string `"t"` at runtime. A tuple shape (`[0]` → `(tuple 0)`) refines the VECTOR member only, but once the fold's result merged into one `pair \| vector` term, `elem_ty`/`tuple_elems` reported the tuple's elements for the whole term, pair member included | ✅ **FIXED 2026-08-29** — both accessors answer only for a term whose seq members the shape actually describes (`tuple_elems` only on a pure vector — a `nil` member makes `first` nil, a `pair` member makes it unknown). Found by an adversarial probe of the set-theoretic model; the same session closed a second gap the probe exposed — a lambda LITERAL passed as a callback was never checked at all (`callback_sig` answered `None`), so `(g (fn (x) (str x)))` against `((int -> int) -> int)` was silent; it is now typed under the arrow's own domain and the existing result-disjointness rule catches it, with no `⊆`-on-an-inferred-return false positives (`(+ x 1)` under `x : int` is `int`). Guards sabotage-verified |
| KI-84 | **An imaged start of a project lost every buffer type's layers** — `nest test` in bedit: the run that WROTE `.brood/image.bin` passed 1306/1306, every run that READ it failed 99 (`*git-status*` not read-only, `.blsp` buffers not in brood-mode, gutters gone). `editor/layers/*type-layers*` was `{}` at module-load time on a warm start | ✅ **FIXED 2026-08-29** — the **stdlib** image's `editor/layers` section carries the module's registries as their pristine seeds (`{}`), and materialising an embedded module was a raw define: it overwrote the 26-entry registry the PROJECT image had just restored. A source load runs `defonce` there and keeps the binding; the image did not. Fix: with `reserve` (an embedded module from the pristine image) a bound DATA global keeps its binding; a bound FUNCTION (an ADR-246 stub, KI-72) is still replaced, and a project image (later state) still overwrites. Guard sabotage-verified |
| KI-83 | **The monomorphization differential compared TIMING CHATTER as if it were an answer.** Under full-suite load, `cli::mono_differential` failed with "monomorphization changed an ANSWER" while both arms reported `92 tests, 92 passed, 0 failed` — the diff was one line: the framework's per-test slow annotation (`concurrent impl registration (KI-22) › … 13.9s`), printed only when a test crosses `*test-slow-ms*` (1 s), which under 4-way nextest parallelism one arm's nested run did and the other's did not. `without_timings` stripped only `ms wall`/`Slow tests` lines | ✅ **FIXED 2026-08-29** — the filter now also drops any line whose last token is a duration (`13.9s`, `2ms`); a real divergence in such a line is theoretically maskable, but a *failing* test already fails the `*_ok` asserts before the comparison runs. Sabotage-verified offline on the exact captured outputs: the old comparison fails on them, the new one passes, and a `92 passed`→`91 passed` mutation still diverges. Same species as [KI-80](#ki-80): a nested suite run under load emitting load-dependent output that an outer gate treats as signal |
| KI-82 | **The hosted playground cannot run its own front-page example.** `https://brood.fly.dev` — the pipeline snippet returned `recursion too deep: used 14021552 bytes of stack, over the 12582912-byte budget` three frames deep, trace `{:fn %require-force-in} {:fn %require-force}` — wasm-only (native answers `165`) | ✅ **FIXED 2026-08-29** (`b6706120`) — not `require` and not ADR-290/291: `WORKER_STACK_BYTES` was a hard-coded 16 MiB (a native worker's stack) while wasm runs on a ~1 MiB shadow stack, so a bogus 13.4 MiB reading landed in the gap between the budget (raised) and the stale-base backstop (never fired). Both are now target-aware; reproduced deterministically via the page's own `completions()`-then-`run()` sequence, verified red-then-green on native, lean-native and wasm32+node. **Residual:** the deployed site answers wrong until hive redeploys with `BROOD_REF` ≥ `b6706120` — a deploy step, not a runtime bug |
| KI-81 | **`BROOD_CONTRACTS=1` was unusable on a cold boot cache** — one run panicked in prelude expansion — `prelude expand: unbound error: unbound symbol: take` (`lib.rs:359`), on a file using `defability`/`impl`/`sig`. the cause is prelude LOAD ORDER, not a rename: `sig!` lives in `core.blsp` and `take` is defined in `seq.blsp`, which is concatenated later | ✅ **FIXED 2026-08-29** (ADR-293) — **never a flake**: `touch target/release/brood` reproduces it 100%, because the boot cache is keyed on the executable's mtime, so the first run after any rebuild is cold and every run after replays the cache without executing the macro bodies. The twelve clean runs were twelve warm ones, and the image hypothesis was wrong. THREE independent cold-only defects: `sig!`'s expansion-time `take`/`nth`/`map`/`range`/`count` (not yet defined that early — `core.blsp` expands before `seq.blsp` is concatenated); the shim closing over a **let-bound local**, which the prelude's freeze step rejects; and `defrecord` emitting its constructor `sig` **above** the `defn` it rebinds, making every record fatal in that mode. Root cause of all three: the mode had **no end-to-end test**, so `crates/cli/tests/contracts_mode.rs` now cold-caches deliberately via `XDG_CACHE_HOME` — without that it passes on a broken build |
| KI-80 | **`brood_suite_passes` flaked once under a loaded `--test-threads 4` run** — failed try 1, passed try 2, on the run that first included a new CPU-heavy type test. Matches the class this binary's `retries = 1` was added for verbatim (the in-language suite holds cases that talk to a local node, and one blown deadline reddens all ~1200 of them) | ⚠️ **WATCHING 2026-08-29** — **not reproduced in 10 runs since** (6 loaded 4-thread, 3 solo, 1 loaded before the fix). No diagnosis is possible because **the failure output was discarded at the terminal, not by the tooling**: nextest names a flaky case and prints its output, and it was piped through `tail`. That is the trap `never-truncate-test-output` already records, and it is the whole finding here. The one contributing factor found and fixed: the new `arrow_subtyping_is_sound` rebuilt a `Ty` and recomputed a denotation 1596 times inside its inner loop, ~2.5M times over — precomputing both took it 3.4s → 2.0s and removed that much contention. **If it recurs, capture the whole run to a file and read the `---- ... stdout ----` block** — which in-language case failed is the entire question, and a summary line cannot answer it |
| KI-76 | **`make green` ran the `.blsp` gates against a binary no documented command refreshes, and reported two failures that did not exist.** It gated on `target/release/nest` while its own advice said to run `make release`, which builds `RELEASE_DIR=target/release-fast` — a *different* binary. The one it read was **9 commits behind** (`464b6c57`), so it carried a pre-rename `std/` baked in and reported `defserver` (renamed from `defprocess` since) and `third` as `unbound symbol` — 8 warnings, all phantom. Both names exist; the current binary returns **zero warnings**. The staleness guard could not have caught it either: it fired only when `std/` or `crates/` had *uncommitted* changes, i.e. never on the clean tree you have right before a push, which is exactly when this gate is consulted | ✅ **FIXED 2026-08-28** — `green.sh` now picks whichever of release-fast/release reports **HEAD's sha** (the `--version` mechanism `make doctor` already used), and a binary that is stale *or* older than any `std/`/`crates/` source is a **failure**, not a note: a stale binary's verdict is meaningless in both directions, so it must not be possible to read a green — or a red — off the wrong `std/`. Sabotage-verified: with uncommitted `std/` edits it prints "the .blsp gates DID NOT RUN" in place of a verdict. **Addendum 2026-08-29:** the same defect verbatim in `check-examples`/`check-stress`/`check-corpora` (fixed inline in `green.sh` only), which on a lean `make release` brood additionally reported an absent DEV_MODULE (`reload/on-change`) as *rename rot* — the exact class the gate exists to find. All three now share `scripts/lib/gate-binary.sh`, which resolves by sha and separates "this build lacks the module" from "this name is gone" |
| KI-79 | **`live_migration::deep_receive_continuations_resume_correctly_across_workers` failed once in CI, on the commit that moved the JIT preempt handler.** The test runs up to 400 bursts and fails unless `migrate_count() > 0` — it asserts a scheduler event was **observed**, not that results are right. In the failing run the per-burst correctness assertion passed **400/400**; only the "was a migration seen" assertion fired, which is the case the test's own message anticipates ("if this is the only failure and the machine was loaded, suspect scheduler starvation"). Suspicious anyway, because `12b31fc2` outlined `jit_run_fast_link`'s cold arms — including the **preempt** outcome that live migration depends on | ⚠️ **WATCHING 2026-08-28** — not reproduced in 18 local runs (10 unpinned + 8 pinned to 2 cores, matching CI's core count). The change is provably a **verbatim** move: a line-by-line diff of the 117 moved lines against the original shows zero semantic differences, and the only new code is `if outcome == 0 { … return }` ahead of the delegation. It also cannot change *when* a preempt happens — the native arm's tick poll decides that, and only the handling moved. **25 further pinned (2-core) runs on 2026-08-29: 0 failures.** Mitigated rather than closed: `live_migration` now carries `retries = 1`, the gap the `distribution` override already documents. **If it recurs, get whether the correctness assertion also failed** — that is the line between starvation and a real capture-machinery bug |
| KI-78 | **CI never builds a stdlib image, so the entire suite tests the load path users do NOT get.** The image is **default-ON** since v0.15.0 (`f114d01e`), and default-ON is safe by construction: with no image on disk `install` returns nil in ~30 µs and everything loads from source. Nothing in `ci.yml` builds one, so that is exactly what every CI job does — all ~1222 tests exercise the source path, and the shipped default is untested there. Worse than uniformly untested: `image_matches_source.rs` (ADR-280) *does* build one and writes it to `~/.cache/brood`, so any test scheduled after it in the same job runs imaged — nextest gives each case its own process in no guaranteed order, making the coverage **order-dependent and nondeterministic** | ✅ **FIXED 2026-08-28** — nextest's setup scripts now build the image (`scripts/build-std-image.sh`, registered beside `warm-boot-cache`), so `make test` and CI both run the default; ci.yml's tree-walker job sets `BROOD_NO_STDIMAGE=1` — the script's own off switch — so one job keeps deliberate source-path coverage instead of the two paths trading places. Verified both ways at **1222/1222**. Found while fixing the KI-72 guard, which had the same hole one level down (`autoload_race` never built an image either; fixed in `6e52528a`). The suite is *known green imaged* — verified locally at 1218/1218 and 1222/1222 with an image live — so this is a coverage gap, not a suspected failure. The fix is to build the image in nextest's existing setup script (the repo already runs one, `warm-boot-cache`, for KI-38) so `make test` and CI both exercise the default, and to keep one job on the source path so both are covered rather than trading one for the other |
| KI-77 | **the `loop` benchmark row is ~2-3% slower than v0.14.1, and it survives every check that usually kills such a signal.** `loop` is a pure integer self-tail loop — the simplest JIT'd shape — so a real regression there is wide. Persists **unpinned** (so it is not the background-JIT-on-one-core artifact ADR-175 records) and persists under **interleaved** measurement (so it is not thermal/session drift): +3.4% pinned and +2.8% unpinned interleaved, +3.3% against a base-vs-base floor of 0.0%, +2.2% interleaved vs both v0.14.1 and 464b6c57. `make ab` reports it as `noise` because with a ~0% floor its rule falls back to a 5% absolute threshold, which is arguably too lenient for a row this quiet | ☑️ **NO LONGER REPRODUCES 2026-08-28** — real when filed, gone at v0.15.0 (`e9c54606`): `loop` is now **-2.2%** against v0.14.1 and **-6.4%** against `dfcddc4f`, the very tree the regression was measured on. It was not fixed by chasing it — v0.15.0 carries a ~5-6 ms FIXED per-run saving that lands on every row (`startup` **-16.7%**, 36 -> 30 ms; `loop` -6 ms, `sieve` -6 ms, `fib` -5 ms, `collatz` -5 ms), which swamped it. Not attributed to a commit. Original detail below — not bisected. Localized to `464b6c57..HEAD` only; the intermediate binaries (v0.14.1, 6b172c1d, 464b6c57) are within 1.1% of each other. **The measurement trap that blocks a bisect is the finding here:** this row's ABSOLUTE numbers drift ~3% between measurement sessions — the same `6b172c1d` binary read 90 ms in one interleaved pair and 93 ms in the next — so a single-shot per-step bisect on a 3% signal reads pure noise. Bisect it with *same-session interleaved* pairs only, or not at all |
| KI-72 | **a stdlib-image section replaced a module's autoload stub before the rest of the module was bound, so a racing process took the real function and died on an unbound helper.** `string/blank?` is public and stubbed; `whitespace?` is `defn-` and is called from its body. Installing the real `blank?` removed the ADR-246 stub — the one door that routes a caller into `require-one` and makes it *wait* — while `whitespace?` was still unbound, so 17 of 24 `spawn`ed children died `unbound symbol: string/whitespace?` and the test's root waited forever for replies that could never total 24. **A wrong answer presenting as a hang.** Two sessions read it as a scheduler/mailbox stall and chased the 5 ms poll, the non-latching condvar, the `code_server` model and the two wake paths — all symptoms. The source path cannot produce it: `load` evaluates in file order, where a helper precedes its caller | ✅ **FIXED 2026-08-28** (ADR-279) — a section now defines names with **no current binding first and already-bound names (the stubs) last**; deferring is enough, atomicity is not needed. Sabotage-verified: deferral off 9/12 hang, on **0/24**. Acceptance load (12 parallel copies at `--test-threads=4`, 90 s): image ON **0/12**, image OFF 0/12, against 12/12 vs 0/12 before. Guarded by ADR-280's `image_matches_source.rs` differential (source vs image must agree on name, kind, privacy and sig — it found a **sixth** divergence on its first run: materialising dropped privacy, 1448 names, 0 after). Two things hid this for two sessions: **libtest captures a test's stderr and discards it when the test never completes**, so the `process N died` line was written and thrown away (`--nocapture` shows it), and the amplifier switches itself off — `BROOD_STDLIB_HASH` covers every `std/**/*.blsp`, so any edit (even someone else's, mid-run) silently turns the image arm into the no-image arm. **Verify the image per run, not once.** **The image is now DEFAULT-ON** (v0.15.0, `f114d01e`; opt out with `BROOD_NO_STDIMAGE=1`) — this fix plus ADR-280's differential is what made that shippable, after the first flip was reverted the same day it landed |
| KI-73 | **a prelude macro's template can be captured by a user module that defines the same name.** A template is spliced into the user's file, where a bare reference resolves against *their* namespace first — so `(defmodule m) (defn get (b k) :CAPTURED) (defrecord pt (x y))` made every accessor return `:CAPTURED`. Silent wrong value: right arity, nothing unbound, `nest check` quiet. Fixed for `defrecord`/`for`/`defonce`/`with-err-str` with the `/name` root escape (ADR-236), which this pass also had to *finish implementing* — it was a resolve-time rewrite only, so `/get` was unbound at root. `receive` included | ✅ **FIXED 2026-08-28** — the escape is now total: resolved in a module (`resolve_sym`), at root (`macros::strip_root_escapes`), and at **runtime macro expansion** (`eval/mod.rs`, for a macro defined after its use site or at the REPL). `but-last` moved to `core.blsp` and `sleep` below `receive` so `receive` expands at prelude compile time. Gated by `tests/prelude_capture_test.blsp` — a static scan asserting ZERO offenders plus four behavioural probes |
| KI-74 | **One `cargo test -p brood --lib` run reported a failure it would not name** — `607 passed; 1 failed`, no failures list captured; 20 clean runs followed | ✅ **FIXED 2026-08-29** — reproduced 1-in-40 under a 4-core spin load and named: `jit_tier_compiles_a_hot_arm_then_runs_native`, whose 400×2 ms poll (~0.8 s) for the background compiler is a deadline a loaded box misses — and libtest's shared process queues every other test's compiles ahead of it (nextest: 30/30 clean under identical load, which is why only libtest ever saw it). Both polling loops now use a 60 s wall-clock bound. The cache-race hypothesis was wrong |
| KI-75 | **`compare` reported values as EQUAL that are not, two ways — and `sort` is built on it.** (1) `(compare nan x)` was `0` for every `x`, so one NaN silently turned `sort` into a no-op: `(sort [3.0 nan 1.0 2.0])` returned its input unsorted, no error. (2) the `Int`-vs-`Float` arm used a lossy `as f64` cast while every other cross-type arm was exact, so past 2^53 two different integers compared equal — `(compare 9007199254740993 9007199254740992.0)` was `0` while `=` and `>` both said otherwise | ✅ **FIXED 2026-08-28** — NaN now sorts LAST via `float_total_cmp` (Rust's `total_cmp` / Java's `Double.compare` choice), and `Int`/`Float` routes through the same exact base-10 path the BigInt/Decimal/Ratio arms already used. `<`/`<=`/`>` stay IEEE deliberately — a sort key and an arithmetic predicate are different questions. Guarded by `tests/comparison_test.blsp` (25 cases) plus three Rust unit tests |
| KI-71 | **a reversed-args rename is invisible to every gate** — `seq/remove-nth` correctly moved to index-first, but arity is unchanged and no symbol is unbound, so `nest check` is clean and the type warning is advisory. In bedit it surfaced as SEVEN failures in `buffers_eval`/`hosted`/`tutor` that read as buffer-lifecycle bugs; the raise happened inside `ed-kill-at` and the caller absorbed it | ☑️ **not a bug (2026-08-27)** — the rename is right; fixed downstream with `nest rename --swap`. ✅ **The class now HAS a gate (same day):** the checker already catches a reversed call precisely, per argument — it was silent here only because `seq/remove-nth` had **no declared `sig`**. The index/collection functions (`remove-nth`, `take-last`, `drop-last`, `chunk-every`, `split-at`, `sample`, `shuffle`, `vector-ref`) now carry one, so the reversal is a warning and CI's zero-warning gate makes it a hard failure. Argument types precise, return `any` — the reversal is an argument mistake, and a narrow return would false-positive at every call site. Zero new warnings across std/ + tests/. Guards `a_reversed_index_and_collection_call_is_flagged` + `the_correct_index_first_order_stays_silent`, sabotage-verified by deleting one `sig` |
| KI-70 | **the checker never looked inside a vector or map literal**, so every expression in Hiccup-shaped code was unchecked. `check_into_inner` opened with `let Value::Pair(_) = form else { return }` — a `[…]` or `{…}` in value position ended the walk, though its contents are ordinary evaluated code. hive's `/docs` renderer carried `(str (max 2 …))` for weeks after `max` moved to `math`: `nest check` green, `nest test` green, and only rendering the page raised it. One level out from KI-67 — not a form that suppressed the lint, a form the walk never reached | ✅ **fixed 2026-08-27** — `Value::Vector` and `Value::Map` descend into their elements (map **keys** as well as values). No false positives: the checker runs on macroexpanded forms, so a `match` pattern vector is already lowered to `let`/`if` binders, and `quote`/`quasiquote` return at `SpecialHead::SkipBody` before their data is ever handed down. std/ + tests/ stayed at **zero warnings** apart from one real find — `std/tool/mcp.blsp`'s `callers` tool called the module-private `project-all-files` from inside a map literal, the **fifth** dead `project-*` call site and the one KI-67's sweep could not see. Guards `unbound_inside_a_vector_or_map_literal_is_flagged` + `descending_into_a_literal_does_not_read_data_as_code`, sabotage-verified |
| KI-69 | **two `jit_plan` guards failed on every `main` push**, so the `differential (tree-walker)` job had been red since KI-64's fix landed. `block_argument_spills_never_reach_the_deopt_journal` and `the_block_argument_want_is_clamped_to_the_reserve` assert on VM-compiled arms, and the job runs `BROOD_VM=0` — nothing compiles, so the first inspected 0 chunks and the second saw no arm to clamp. Both fail loudly by design (a vacuous green would mean nothing), which is why they failed rather than passing hollowly | ✅ **fixed 2026-08-27** — both pin `set_forced_ceiling(Some(Tier::Native))`, the fix `compile/tests.rs` already documents for its two native tests since ADR-222 made the ceiling coherent. The guards are new (2026-08-26) and simply missed the pin |
| KI-68 | **the fuzz-differential gate was HOLLOW — it had been comparing dead programs.** `stress/fuzz_programs.py` writes Brood source itself, and the rename waves retired every name it emitted (`table`, `rem`, `bit-and`, `bit-xor`, `table-get`/`put`/`incr`/`count`, `quot`, `min`/`max`, `println`, `map-get`/`map-count`/`map-dissoc`/`map-int-add`). Every engine died identically on `unbound symbol` at line 1, so all four configs *agreed*, every seed printed `ok`, and the run ended "all configs agree". The generator is Python, so `nest check` and the `.blsp` suite could never see it | ✅ **fixed 2026-08-27** — names updated (60 seeds, 0 unbound, all configs agree with real digests) **and the shape gated**: an `unbound symbol` on stderr from a GENERATED program is now a hard failure naming the dead names, and a run where not one seed reached a clean exit fails as "the corpus is dead, not the engines agreeing". Sabotage-verified in the exact original shape — reverting `(table/new)` to `(table)` prints `DEAD PROGRAM seed=1 … : table` instead of `ok` |
| KI-67 | **`nest check` was silent about unbound symbols inside a `try` body**, so a rename could leave a call site dead and every gate stayed green. `hatch`'s spool write was `(try (bytes/append path piece) (catch e …))`; brood renamed that to `file/spit-bytes-append`, `nest check` reported nothing, and the repo shipped with every spooled upload broken — visible only as four tests timing out with no error to read | ✅ **fixed 2026-08-27** — `try`/`%try`/`error-of`/`assert-error` now DESCEND, keeping only the unbound-symbol diagnostic and dropping every other lint. Filtering happens at the collection point, not lint-by-lint, so a lint added later is suppressed here by default. Opt out with `(check-allow :unbound …)`. Found two real dead call sites on the first run: `bytes-concat` in `http_test` and four module-private `project-*` names in `std/tool/mcp.blsp` |
| KI-66 | **nothing in the default gate verified that a project still BOOTS.** `nest check` resolves names and `nest test` runs the suite, but neither evaluates `main` — and that is exactly where a stale dependency dies. hive went down twice on the same shape: `unbound symbol: int->char` and then `unbound symbol: os/getenv`, both raised on the first line of `main`, both after a clean check and a green suite | ✅ **closed 2026-08-27** — never a missing capability: `nest run --for 6s` already does it (exit 1 with the error if the entry point raises, exit 0 if it survives). The gap was wiring, now wired: hive's `bin/ci` runs it, and the shared `package-ci` workflow takes an opt-in `boot-check` input with `boot-seconds` (2b51de93). Off by default on purpose — a library has no `:main`, and `bedit`/`pong` need a GUI a runner has not got |
| KI-64 | **the JIT miscompiles `json/encode` under sustained load** — hive's `/api/v1/packages*` returns 500 after ~60 requests and then fails until the machine restarts, while `/health` and every web page keep serving. The error is `empty?: expected collection, got int (1114114)` inside `emit-pairs`/`emit-list`, i.e. an int where the recursion expects a list; 1114114 is NOT a codepoint (that reading was a coincidence) — it is a packed deopt-journal word, `17 << 16 \| 2`. `BROOD_NO_JIT=1` makes it 120/120 clean | ✅ **fixed 2026-08-26** — a block-argument spill slot was landing on the deopt checkpoint, because `jit_spill_reserve` gated the WHOLE reserve on having ≥2 non-tail calls while block-argument slots depend only on the operand depth at a block leader. Not shared code, not concurrency, not load: it reproduces in one process on the fourth call. Fixed by clamping the spill window to its reserve (ADR-248); `BROOD_NO_JIT=1` is no longer needed in hive |
| KI-63 | ~~loading std modules taxes JIT'd hot loops~~ — **RETRACTED 2026-08-25, the effect does not exist.** After a discarded warm-up run in-process, a 20M loop is 23-24 ms whether or not `format` is loaded. Every earlier figure measured the FIRST run, i.e. JIT warm-up, which is variable and shape-sensitive — the same loop read 25 ms in one file and 40-51 ms in another differing only by having three call sites | ☑️ **retracted** — no bug. What is real, and is the reusable part: a whole-process benchmark of a short row measures tiering, not steady state |
| KI-62 | **the stdlib startup image was unusable on the build that ships.** It is keyed on `stdlib-id` — the stdlib's CONTENT — deliberately identical for `brood`/`nest`/`brood-lsp` so one copy is shared; but those binaries do not bake in the same MODULES. A lean runtime (`nest release`, `make install INSTALL_FEATURES=RUN_FEATURES`) has no dev-tools, and `std/tool/project.blsp`'s recorded require-edges name `test`. Replaying that edge made the very next `require` die with `cannot find module 'test'` — so installing the image BROKE `require`, and its advertised 4-33x was never reachable where it matters | ✅ **fixed 2026-08-25** — `merge-require-edges!` drops a dep this binary cannot load. Filtered at INSTALL, not at build: the image may have been written by a different binary from the one reading it, which is the whole point of sharing the key. Measured on release: `require format` **62.0 -> 12.8 ms (4.8x)**, `require datetime` **3.3 -> 0.39 ms (~9x)**. Guard in `tests/stdimage_test.blsp`, sabotage-verified |
| KI-61 | **startup is +82% since 0.3.11 (13.6 -> 24.8 ms), and it is a per-wave tax, not a one-off.** Each namespacing wave that moves prelude names into a module forces that module to be force-loaded from source at every boot — the prelude's qualified refs are late-bound and boot's namespace-resolve does not auto-require for the root prelude. Two steps, both proven: `1f613d23` (`(require-one 'string)`) **+4.0 ms**, and the v0.11.0 wave (`(require-one 'seq)`) **+7.5 ms** — the latter measured by deleting the line and rebuilding (24.3 -> 16.8 ms). It also deflates every other published row, since `compute = wall - startup` | ✅ **fixed 2026-08-26** — by not loading the modules at boot at all: the prelude's `string/`/`seq/` references are autoload stubs that load on first call (ADR-246), and prelude def-sites now travel in the boot cache instead of a second positioned read of the prelude (ADR-247). Warm boot 22.8 -> 11.6 ms, base RSS 55.6 -> 50.7 MB, `startup` -28.9%, every other row ~11-13 ms faster in absolute wall. The std image + registration replay is still the right way to make the *lazy* load fast; the two compose |
| KI-60 | **every `:to *err*` in the stdlib wrote to stdout with ` :to #<native %write-err>` appended.** The `io/` wave (ADR-230-era) gave ports an ability with `(impl Port :fn …)`, but `*err*` and `*out*` are `%write-err`/`%write-out` — **natives**, whose `type-of` is `:native`, not `:fn`. So `port?` was false, `split-target` read the trailing `:to <port>` pair as ordinary values, and log / the test runner / supervisor / repl / telemetry all lost stderr | ✅ **fixed 2026-08-25** — `(impl Port :native …)` beside the `:fn` one. Found because `origin/main` was **red on three `nest` tests**; `declared_sig_is_authoritative_cross_module` reads warnings from stderr and got none. Attribution verified by reverting just this impl: that test fails, the other two pass |
| KI-59 | **`nest run --for` reported failure for a program that succeeded.** The wrapper was `(%spawn …)` then `(monitor p)` — two steps. A program that finished before the monitor attached fired a synthetic `:noproc`, `(= :noproc :normal)` was false, and the run exited 1 after printing its output correctly. Worst in the mode documented as the CI-friendly way to exercise an app | ✅ **fixed 2026-08-25** — `%spawn-link` + `trap-exit`, atomic by construction: the kernel already names this race on `%spawn-link` itself ("no spawn->link :noproc race", ADR-067). Reproduced ~1 run in 6 under load, 0/20 after. Reading `:noproc` as success would be wrong the other way — a program that *crashed* before the monitor attached is indistinguishable |
| KI-58 | **the namespacing silently killed the `table-put` call-site inline — `sieve` 11.6× slower.** `resolve_prim3` accepted only a *direct* native head, on the stated grounds that `table-put` "has no prelude wrapper to follow"; the v0.9/v0.10 waves made the head `table/put`, a `std/table.blsp` wrapper, so the call stopped inlining and became an ordinary `Call` in the hot arm. The **2-ary** `resolve_prim` follows its wrapper, so `table/has?` went on inlining beside it — the asymmetry is visible in one IR dump | ✅ **fixed 2026-08-25** — `resolve_prim3` follows a thin wrapper like the 2-ary path, requiring the identity argument map (`Node::Prim3` has no permutation field, so a reordering wrapper must decline rather than store under the wrong key). `make ab`: **457 → 68 ms, −85.1%**. Guards: `table_put_call_site_inline_recognizes_the_namespaced_wrapper` (sabotage-verified) and the new `every_inlinable_head_still_reaches_its_primitive`, which caught a **second** dead inline on its first run — `table/get`, whose `&optional` head is not a thin wrapper; fixed in `std/table.blsp` with two arity clauses |
| KI-57 | **a use-after-GC on every selective receive with a backlog.** `scan_mailbox` took the clauses' leading-keyword vector as a bare `Value` and decoded it **lazily, inside the scan loop** — so on any iteration after the first the decode dereferenced a handle held across a matcher `apply`, which can collect at any eval depth (ADR-061). The `matcher` beside it is rooted at `rbase+0` and re-read per candidate for exactly this reason; `tags` was not | ✅ **fixed 2026-08-25** — `tags` is rooted at `rbase+1` and re-read at the decode, like `matcher`. Found by running `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` by hand while verifying ADR-245: `use-after-GC: vector handle … is from epoch 12, but that generation is now epoch 13` out of `collect_receive_tags`. **No CI job could have caught it** — every one collects on a threshold, so the collection has to land inside the window by luck; the new `make gcstress` step closes that, and is verified red on the pre-fix code and green on the fix |
| KI-56 | **a large L1 send head-of-line-blocks unrelated mailbox operations**, linear in payload: an unrelated `mailbox-size` probe sits at **p50 ~5 ms** for a 1.6 MB send and 7–13 ms at 4 MB, against a wire path flat at **4–10 µs** across a 500× payload range. Onset between 8 KB (nothing) and 80 KB (p90 ~25×). Needs a *parked* receiver, so it is the synchronous request/reply shape, not fan-in | ✅ **fixed 2026-08-25** (ADR-245) — a **work budget** on the L1 copy (one heap node = one unit, default 4096, `BROOD_L1_BUDGET=0` to uncap): past it the copy declines and takes the wire path, whose heavy work is already outside the lock. The `st.waiter` invariant is untouched. Every container kind also declines *before materialising* — the first cut checked only per node, so a 100k vector still paid `to_vec` under the lock and measured p99 243 µs; with the early-out it is 5.4 µs. Same probe as the measurement below: **p99 1 875 → 5.4 µs at ~1.6 MB, p50 2 360 → 3.9 µs at ~3.9 MB**, indistinguishable from the wire arm at every size. **The second site (selective-receive's peek-in-place rebuild) is fixed too** — same budget on the wire form, p50 1 252 → 0.8 µs and p99 5 569 → 11.2 µs at a backlog of 8 × 40k |
| KI-55 | **closure-shipping across nodes broke for every namespaced std name.** Auto-require runs on the node that *compiles* a form, never on the node that *receives* an already-compiled closure — so a shipped closure whose body calls `reflect/form-pos`, `seq/…`, `dev/…` raises `unbound symbol` on the receiver. Before the v0.9.0/v0.10.0 namespacing these were bare prelude names and always bound, so closure-shipping "just worked" | ✅ **fixed 2026-08-25** — the **sender** names the modules its closure's body references (`ClosureMsg::modules`, a `(module, probe)` pair per module, encoded in the wire's `M_CLOSURE` record — protocol `BRD\x06`), and the receiver weaves a `(if (bound? 'probe) nil (require-one 'module))` guard into the rebuilt body for each module it lacks, so the load runs at the closure's own **call site** — where errors are catchable and no mailbox lock is held. A module this node cannot load now raises `this closure was shipped from another runtime and needs module \`zz\`…`, not a bare `unbound symbol`. Guards: `cli::distribution::a_shipped_closure_requires_its_modules_on_the_receiver` (new) + `source_positions_survive_a_cross_node_send` with its `(require-one 'reflect)` workaround removed; both sabotage-verified |
| KI-54 | **`main` was already red**: making the gen_server framework core-and-**bare** in the prelude (`7cb796f0`) seized ten generic global names — including `call`, `cast` and `stop` — into the *reserved* set, breaking `basic::spawned_process_picks_up_redefinition` (`(def call …)` is now refused); the same move dropped `gen` out of `(builtin-modules)`, failing `namespace_test`, and bundled `gen.blsp` without declaring it, failing `prelude_manifest` | ✅ **both fixed 2026-08-24** — `PRELUDE_MODULES` restores the reservation (with a bundle-checked honesty test); the `basic.rs` helper renamed `call` → `ask`. ⚠ **the name seizure itself is left as-is** — it follows from ADR-166 and the deliberate "bare" decision, but see the note below |
| KI-50 | **the JIT leaf inliner silently miscompiled the most idiomatic loop in the language.** `(defn sum-down (n acc) (if (<= n 0) acc (sum-down (dec n) (+ acc n))))` returned **6251217600 instead of 20000100000** for `(sum-down 200000 0)` on the default build, and at 400000 raised `type error: -: expected number, got nil` blaming `dec`. `std/repeat` is this exact shape, so `(count (repeat 200000 :a))` gave **28033** | ✅ **fixed 2026-08-24** — a leaf-spliced frame was read as the *small* layout, so the small body's journal slot (a live loop counter in the spliced layout) was decoded as a deopt journal. Guard `tests/jit_leaf_frame_layout_test.blsp`, sabotage-verified |
| KI-51 | `macroexpand-1` held a heap handle across an auto-`require` that loads a module (arbitrary eval → GC), then dereferenced it — **use-after-GC on a user-reachable path**: an epoch-tripwire abort in debug, silent heap corruption in release (a 5-element list reported an arity of 31309) | ✅ **fixed 2026-08-24** — roots `form`/`env` across the require and re-derives the tail |
| KI-52 | `msg_roots` — the L1 delivered-message slot table — was in **every LOCAL collector root set and no RUNTIME one**, so a shared closure sent by handle to a parked receiver could have its code compacted or freed while still queued: a **use-after-free of shared code** | ✅ **fixed 2026-08-24** — seeded into `runtime_collect_with`, `seed_phase1_and_walk`, `runtime_evacuate` and `runtime_live_closure_count` |
| KI-53 | a `Ref` crossed the wire as a **bare u64 counter starting at 0 on every node**, so two nodes minted colliding refs — and a ref is what a pinned `receive` matches and what identifies a monitor, so a collision could hand a caller a reply meant for a peer | ✅ **mitigated 2026-08-24** — per-runtime random top 24 bits (`next_ref`); node-qualified refs remain the principled fix, blocked on `Value`'s JIT-pinned layout |
| KI-45 | `examples/editor` calls `eval-command/eval-last-sexp`, but that module moved to the sibling `brood-edit` project on 2026-05-31 (`650eb89f`) — so the example has referenced a module this repo lacks for 2.5 months; `nest test` there is 4/5. Nothing gates `examples/` | ✅ **fixed 2026-08-17** — deleted the stale `examples/editor` (brood-edit is the real editor project) |
| KI-44 | `nbody` died with `unbound symbol: sqrt` (and `json` on the dropped `json-` prefix) — ADR-227 moved `sqrt` to `std/math.blsp` and the separate benchmarks repo was never migrated, so a published harness run would fail. Fixing it the correct way then exposed that the `sqrt` **call-site inline** was dead: it required a bare head resolving to a PRELUDE closure, and neither spelling qualifies now — **~1.8× on the row** | ✅ **fixed 2026-08-17** — correctness (both rows run, checksums match) + the inline restored via a structural identity for `math/sqrt` (321 ms vs 905 ms on a 3M-iter loop) |
| KI-49 | the tagged-tuple `receive` matcher type-deopted 16x and was latched onto the interpreter for the process — 454 ns vs 59 ns for the keyword matcher, and the whole `pingpong`/`ring`/`supervisor` gap to Elixir | ✅ **FIXED 2026-08-21.** Root cause: `as_block_arg` forced every non-`Int` block argument through `as_int`, so the matcher deopted on **every** activation and the sixteen-deopt rule latched it to the interpreter. Fixed by spilling boxed args to position-derived frame slots (`ParamRepr`). `hof_decline_bailed` 98 981 → 0, `jit_link_done` 0 → 187 328, `ns_match_run` 457 → 179 ns/msg, **pingpong −12.9%** (reproduced 3x; spawn/fib/collatz flat) |
| KI-48 | JIT tail dispatch read past the roots stack — `root_at(9)` on a len-8 stack, twice on 2026-08-20, from `jit_dispatch_tail`; an audit then found a **second live instance** in `dispatch.rs` and a **false safety argument** in `vm_cache.rs` | ✅ **root-caused, both instances fixed, and the anti-pattern gated 2026-08-21** — the dispatcher re-derived the frame size from `active_nslots()` (the KI-26/ADR-210 anti-pattern), which the background inline swap can change mid-flight; **measured live on 123 arms**, `fold` at nslots=13 vs inline_nslots=25 (a 12-slot overshoot). Now passed the size the trampoline built the frame to. Original crash never reproduced on demand, so the causal link is strong but not proven |
| KI-47 | the `differential (tree-walker)` CI job went red on **every** run from the ADR-230/231 namespacing merge onward, always as the same three `adversarial_test.blsp` heap-allocation cases dying on `E0043`. Those cases were not the cause: the suite's **process-wide** allocation had reached **1.145 GB** against the **1 GiB** `TEST_DEFAULT_SOFT` backstop, and a threshold failure names whichever tests are running when the line is crossed | ✅ **fixed 2026-08-19** — backstop raised to 2 GiB soft / 3 GiB hard (`core/alloc.rs`), which is what it documents itself to be: a *host-survival* guard, not a working-set budget. ⚠ **Leaves an unresolved question**: the cap was sized against a ~240 MB suite peak and the suite now peaks ~1 GB (4.8×), so this restores a ~2× margin, not the intended 4× |
| KI-46 | **deadline margin, not a failure yet.** KI-39 was a fixed cost sitting under a fixed deadline for weeks while reading as a random flake, so every case's CI margin against nextest's 120 s hard kill was audited from the 2026-08-17 logs. `nest::bin/nest mcp::tests::std_check_tool_returns_structured_diagnostics_or_an_error` is now the worst at **87 s = 1.38× margin** — it invokes the MCP `check` tool, which is cwd-based, so it type-checks **this whole repository**, and the cost grows as the repo does | ✅ **fixed 2026-08-18, the real way** (87 s → **2.5 s**) — the three cheap fixes were all worse than the problem: `BROOD_NO_CHECK=1` guts what the case proves, a temp-dir `set_current_dir` is process-global and would race its siblings under plain `cargo test` (trading a slow test for a nondeterministic one), and a bigger nextest budget is what hid KI-39. So the real fix was done instead: `check-project-structured` gained an optional `from` root, and `mcp-check-tool` now passes **`*project-root*`** — the root the server already pins its write sandbox to and that every other project-scoped tool already read. `check` was the one tool taking its project from cwd. The three next-worst (`scaffold_quality`, 89/80/77 s) **were** fixed the same day by splitting one case per template |
| KI-43 | `remote_attach_reads_snapshot_then_sees_disconnect` killed the target after a **fixed 5 s sleep**, but the observer needs 5.9–9.2 s under load to boot + `require 'observer'` + connect — so the target died first, `connect` refused, stdout empty. Failed BOTH retries in a loaded `make test`, passed standalone: the signature that gets written off as noise | ✅ **fixed 2026-08-14** — waits for the observer's attach report instead of a stopwatch; **8/8 under saturating load**, and 3.5 s instead of ~11 s |
| KI-42 | the `breakage/` suite had rotted to **9 of 23 files failing** and nobody knew, because it is outside `make test` and had no CI job — a pin-syntax change (`~ref`→`^ref`), a renamed `string-contains?`, an assertion predating exact rationals, and a TCP file whose every phase was dead | ✅ **fixed 2026-08-13** — all 23 files pass and gate, nothing skipped; CI job added so it cannot rot silently again |
| KI-41 | concurrent `require` of the same feature could **double-load** its file: a claimant whose `(contains? *features* key)` guard read the per-process global inline cache **missed** a racing loader's just-committed `provide` (the cache is version-gated on a `Relaxed` counter, no happens-before), won the released load-once claim, and reloaded the module. Surfaced as the ADR-225 co-located-secondary `nest test` flake (~1/77); reproduced on demand at 20 files × 40 requires | ✅ **fixed** 2026-08-13 — `require-one` re-checks `*features*` with a new cache-bypassing `%registry-member?` (reads the shared globals table directly) before loading; guard `breakage/chaos_concurrent_require_double_load.blsp` |
| KI-40 | concurrent green processes running the **same** shared compiled arm on the VM contended on that arm's single `Arc<CompiledArm>` refcount — one cache line, N cores — costing **3.2×** wall on a 100-way fan-out and leaving the cores stalled at 769% instead of 1150% | ✅ **fixed 2026-08-13** (ADR-224 — a process-local `ArmHandle` interposed on the call path; `pfib` 54.4 s → 17.1 s) |
| KI-39 | the CI `differential (tree-walker)` job failed intermittently (3 of 11 runs) with nextest exit 100; **0/15** in the faithful local shape, cold-boot-herd hypothesis measured dead, and whether it is still present is genuinely unknown (4 green runs is 28% likely either way) | ✅ **fixed 2026-08-18** (was: watching — recurred 2026-08-17, run 32032421650, exit 100, the only red job of five). The self-reporting added for it **did not work**: its annotate step is `grep … \| … \| while` under `shell: bash` (`-eo pipefail`), so a non-matching grep exits 1 and kills the step before it prints anything — the run named no case. Hardened with `\|\| true` (all six annotate pipelines, CI-shell-verified), and that was NOT enough — run 32054863012 failed with the annotate step SUCCEEDING and still emitting nothing. **DIAGNOSED AND FIXED 2026-08-18** once `gh auth` was restored and the artifact could be read: the log is ANSI-COLOURED, so `Summary \[` could never match `Summary<ESC>[0m [`, and the case was a **TIMEOUT**, a shape no pattern covered. The failure itself was `nest::complete completion_never_fails_however_it_is_called` — 96 subprocess spawns × a completion that loaded all of `project` (770 ms of 950 ms each), 64 s locally and past the 300 s cap under CI contention. Not intermittent: 8 of 8 runs that day. |
| KI-38 | three tests that wait for a freshly spawned debug `brood` to boot fail together under peak suite load — a **cold expanded-prelude boot cache** (11x a warm boot, all macro-expansion) times the concurrent herd | ✅ fixed 2026-08-08 (warm the cache before the fan-out) |
| KI-37 | an imaged start never followed a module's require edges, so a transitively-reached module was never materialised — `nest run` died on the second run | ✅ **fixed** 2026-08-07 |
| KI-36 | `reconnect_watcher_heals_a_fallen_link` failed once at 22.6 s and passed on retry, during a suite run with a 4000-module image build beside it | ✅ **fixed 2026-08-19** — reproduced at last (3rd sighting, `make test` run 3 of a repeated-run gate), and it was **never the nodedown stall this entry inferred**: B2 opened its listener *before* registering `:echo`, so A's ping-on-nodeup could arrive at a name that did not exist yet and be silently dropped. `register` now precedes `node-start` |
| KI-35 | `*method-from*` was never imaged, so an imaged start stopped reporting cross-module `defmethod` conflicts | ✅ **fixed** 2026-08-07 |
| KI-34 | the startup image was written on every cold start and **never read from** — two defects, either sufficient | ✅ **fixed** 2026-08-07 |
| KI-33 | fully consuming a stream leaked its producer process — an exhausted stream parked in `stream-done-loop` forever instead of exiting | ✅ **fixed** 2026-08-07 |
| KI-32 | a selective `receive` corrupted a skipped **local** (L1-delivered) message to `nil` — a stream request/reply pipeline deadlocked intermittently | ✅ **fixed** 2026-08-06 |
| KI-31 | a foreign-ecosystem version range compiled to its FIRST term — `">=1.0.0 <2.0.0"` became `>=1.0.0` | ✅ **fixed** 2026-08-06 |
| KI-30 | seven `temp-dir` prefixes were never purged — 4484 dirs / 168 MB of `/tmp` litter | ✅ **fixed** 2026-08-05 |
| KI-29 | node/observe tests orphan `brood` children — one found alive **9 days** later, ~15% CPU each | ✅ **fixed** 2026-08-05 |
| KI-28 | `clean_peer_exit_fires_nodedown_promptly` failed once, then passed on retry; output not captured | ☑️ **superseded by KI-38** — recurred twice, both in `wait_until_listening` (a *boot* failure), and KI-38's fix is verified holding (2026-08-12) |
| KI-27 | node tests drew their port from the OS **ephemeral** range, so an unrelated process could take it | ✅ **fixed** 2026-08-05 |
| KI-25 | five JIT/VM suites cannot be re-run in one image (`--repeat-until-failure` fails on iteration 2) | ✅ **fixed** 2026-08-04 |
| KI-24 | `eval`'d code cannot forward-reference a name a later `eval` defines (regression, 97d63eda) | ✅ **fixed** 2026-08-01 |
| KI-23 | the KI-22 lost-update shape also exists in ~10 std-module registries | ✅ **fixed** 2026-08-02 |
| KI-22 | concurrent registration lost ~40% of registrations (15 prelude registries) | ✅ fixed 2026-08-01 |
| KI-21 | `nest run --for` / `--watch` generated a legacy `~p` pin — failed on any file | ✅ fixed 2026-07-30 |
| KI-20 | a JIT fast link ran the callee against the *caller's* IC block (cold cache) | ✅ fixed 2026-07-30 |
| KI-19 | VM resolved a call's free-global head *after* its arguments | ✅ fixed 2026-07-30 |
| KI-18 | a JIT deopt could re-run a `table-put` (effect duplication) | ✅ fixed 2026-07-30 |
| KI-17 | `nest check` validated qualified names against its load set, not per-file reachability | ✅ fixed 2026-07-30 |
| KI-16 | the LSP still matched the retired `defprotocol`/`defimpl` | ✅ fixed 2026-07-27 |
| KI-15 | `impl` silently misregistered a **bare** record id | ✅ fixed 2026-07-27 |
| KI-14 | RUNTIME collector re-walked a deep process's whole root stack every safepoint | ✅ fixed 2026-07-27 |
| KI-13 | cross-module return-type inference blew up exponentially in branch count (checker hang) | ✅ fixed 2026-07-27 |
| KI-12 | a frozen prelude global's inner handle resolved to the wrong object | ✅ fixed 2026-07-26 |
| KI-11 | JIT tail-chain recursion escaped the native-depth cap | ✅ fixed 2026-07-26 |
| KI-10 | `receive` compile cliff at the 13th arm | ☑️ no longer reproduces (2026-07-25) |
| KI-9 | arity error from a closure shipped in a `spawn` body | ☑️ transient build artifact; not in committed code |
| KI-8 | RUNTIME form-position table stranded by compaction | ✅ fixed 2026-07-03 |
| KI-7 | declared `(sig …)` type-expressions corrupted by RUNTIME compaction | ✅ fixed 2026-07-03 |
| KI-6 | `%isolate` snapshot/restore not RUNTIME-compaction-safe | ✅ fixed 2026-07-03 |
| KI-5 | `nest test` OOMs — shared RUNTIME region accumulates every test file's code | ✅ fixed 2026-07-03 |
| KI-4 | bitset stored as a non-UTF-8 `Value::Str` corrupts the GC on promote | ✅ fixed 2026-06-15 |
| KI-3 | RUNTIME compaction strands live VM / tree-walker constants | ✅ fixed 2026-06-01 |
| KI-2 | `nest test` flaky / hangs when parallel tests share heavy global lookups | ✅ fixed 2026-05-29 |
| KI-1 | multi-thread scheduler race: green processes can't resolve globals | ✅ fixed 2026-05-29 |

**2026-08-24 — a general review found five kernel bugs the suite could not see** (KI-50 … KI-54),
all now fixed. The one that matters most is **KI-50**: the JIT produced *silently wrong arithmetic*
on the default build for the ordinary counting loop, and `std`'s `repeat` was already corrupted by
it. It is worth being precise about why five CI jobs were green through it. Every case in the
`std` suites is small — grepping the eight non-`datetime` suites for `100000`/`200000`/`50000`
returns nothing — and this corruption needs **one long activation**, not many calls: a loop calling
the same function eleven times at 1 000 → 100 000 is entirely correct, while a single 180 000
-iteration call is not. So the bug lived in the gap between "we test that it works" and "we test
that it still works at scale", and no amount of re-running the existing suite would have found it.
The lesson generalises past this bug: **a size sweep is a test dimension, not a stress test.**
Asserting the same closed-form answer at 10³/10⁵/10⁶ *and* across `BROOD_TIER` 0/1/2 would have
caught it the day the leaf inliner defaulted on, and costs milliseconds.

The other three are the same shape as bugs this file already records — a handle held across an
operation that can collect (KI-51, the bug-#2 / KI-48 class), a root set that one collector knows
about and another does not (KI-52), and an identity that is unique per node but not across nodes
(KI-53).

**KI-64** (a JIT block-argument spill landed on the deopt journal, surfacing as `json/encode` failing under load) is **fixed 2026-08-26** — the cause was neither shared code nor concurrency, which the entry had inferred: it reproduces in one process on the fourth call. **KI-61** (startup +82%, a per-wave namespacing tax) is **fixed 2026-08-26** — by making the prelude's library references autoload lazily rather than force-loading at boot (ADR-246), plus moving prelude def-sites into the boot cache (ADR-247): warm boot 22.8 → 11.6 ms, base RSS 55.6 → 50.7 MB, `startup` −28.9%. No open items. **KI-60** (the stdlib lost stderr) and **KI-59** (a successful `nest run --for` could exit 1) and **KI-58** (the namespacing killed the `table-put` inline; `sieve` 11.6×) was found by the first cross-language harness run on 0.11.0 and **fixed 2026-08-25**. **KI-57** (a use-after-GC in the selective-receive scan) was found and
**fixed 2026-08-25**, along with the CI gap that let it survive — see `make gcstress`.
**KI-56** (a large message blocked unrelated mailbox operations) was
**fixed 2026-08-25** — ADR-245's budget, at **both** sites: the L1 send-side copy and the
selective-receive peek-in-place rebuild. **KI-55** (a shipped
closure could not call a namespaced std name on the receiving node) was **fixed 2026-08-25** — the closure now carries the modules its body references and the
receiver loads them at the call site. Otherwise KI-49 (the tagged-tuple receive matcher latched onto the interpreter) was root-caused and **fixed 2026-08-21** — `pingpong` −12.9%. KI-48 (JIT tail dispatch read past the roots stack) was root-caused and fixed 2026-08-21 — though never reproduced on demand, so watch for a recurrence. Before that, no open items — KI-36 was reproduced and fixed 2026-08-19, KI-47 the same day. `main` is green on all five CI jobs at `c8dbf0ea` (run 32247618122) — the first fully green run since the ADR-230/231 namespacing merge. KI-44 (the `sqrt` call-site inline, worth ~1.8× on `nbody`) and KI-45 (the stale `examples/editor`) were both fixed 2026-08-17. KI-43 (a fixed-sleep race in the remote-attach test) was found and fixed 2026-08-14. KI-28 is **no longer a watch item — it recurred twice
and is folded into KI-38**, which is the larger pattern it turned out to be part of: three tests
that wait for a freshly spawned debug `brood` to finish booting, failing together under peak suite
load. **Diagnosed, reproduced deterministically, and fixed on 2026-08-08**: the expanded-prelude
boot cache is keyed on each binary's own mtime, so a rebuild colds it for every binary at once, a
cold boot costs ~11x a warm one (all macro-expansion), and that cost times the concurrent herd
walked straight through the helpers' 20 s / 30 s deadlines. Warming the cache once before the
fan-out takes the three tests from a 20.1 s failure to 1.9–2.6 s. No bug in the *language or
runtime* was implied at any point — every sighting was a boot wait, never an assertion about
behaviour under test. (KI-37 was open for a few hours on 2026-08-07 and is fixed.)

---

## KI-100 — a ~5-6% compute regression: two clean branches, a slow merge ⚠️ OPEN 2026-09-01

**Symptom.** Refreshing the benchmark suite's Brood column at 0.22.0 (last measured at
0.19.1, `8a2aaa01`) found **every compute row 4-10% slower**, with checksums unchanged on
every row — the programs compute exactly what they did, only time moved.

**Bisected, 2026-09-01: the first bad commit is the MERGE `0f57e30b`, and both of its
parents are fast.**

| commit | what | mandelbrot vs a fixed reference |
|---|---|---|
| `2dc7d2e6` | ADR-302 data-first (the branch tip) | ratio **1.016 — good** |
| `25a558d4` | mainline (carries §7.5 JIT increments 1-3) | ratio **0.992 — good** |
| `0f57e30b` | **the merge of those two** | ratio **1.061 — BAD**, reproducible |

So neither side is slow on its own; the combination is. `git bisect run` over the window
found it (probe below), and the merge was re-probed to be sure.

**What the merge actually changes.** `git diff 2dc7d2e6 0f57e30b -- std/` is **empty** — the
std tree is identical across it. The whole delta is kernel-side: the mainline brought
`115faead` (RootsBuf — `Heap.roots` from `Vec<Value>` to a `#[repr(C)]` buffer with
(ptr,len,cap) at fixed offsets), `f832928f` (`BROOD_XCALL`, opt-in) and `3dc971d4` (hot
re-lowering, default ON). Read the other way: **ADR-302's std is fast on the old kernel and
the new kernel is fast on the old std; only ADR-302's std running on the new kernel is slow.**

**Decomposition** (mandelbrot at `BENCH_N=1400`, min of 11, all interleaved against the
same reference binary `2c822875`; synthetic merges built to order):

| tree | ratio |
|---|---|
| `2dc7d2e6` — ADR-302's std alone | 1.021 |
| ADR-302 + mainline **without** §7.5 | 1.030 |
| ADR-302 + **RootsBuf only** (`115faead`) | **1.052** |
| ADR-302 + all of §7.5 (the real merge) | **1.080** |
| `25a558d4` — §7.5 **without** ADR-302's std | 0.992 |

Read down the column: **§7.5 costs ~5 points on ADR-302's std and nothing at all on the old
std** (0.992). `115faead` (RootsBuf) reproduces about half of that on its own, so it is a
confirmed contributor rather than the whole story. The remaining split between increments 2
and 3 is ~2 points against ~1% run-to-run noise, so it is **not** resolved — and note it sits
awkwardly with `BROOD_NO_XCALL=1` (increment 3's off-switch) failing to close the gap. Do not
build on that last split without more samples.

**Narrowing so far.**
- **Increment 3 is excluded**: `BROOD_NO_XCALL=1` does not close the gap (ratio 1.0595 with
  it set vs 1.0570 without). `BROOD_NO_INLINE=1` does not either (1.0548).
- **It is NOT JIT-specific**, which corrects this entry's first draft: on *this pair* the gap
  is **1.046 at tier 1** and 1.064 at tier 2. The earlier "tier 1 is flat" reading compared a
  different, confounded pair (an old commit against the current tree). A tier-1 signal rules
  out codegen and points at something the VM path also pays — which is what makes
  **`115faead` (RootsBuf)** the standing suspect: the GC root stack is used at every tier.
- The merge inherits ADR-302's much larger lowering volume (160 arms lowered vs 88 on the
  mainline side), so a plausible shape is "the new root-stack representation costs more per
  frame, and ADR-302's std pushes many more frames" — unverified.

**MECHANISM FOUND 2026-09-01: instruction-fetch pressure, not work.** `perf stat` on the
culprit pair (mandelbrot, `BENCH_N=1400`):

| metric | `2dc7d2e6` (good) | `0f57e30b` (bad) | delta |
|---|---|---|---|
| instructions | 18.01 G | 18.24 G | **+1.25%** |
| cycles | 6.12 G | 6.41 G | **+4.7%** |
| L1-icache-load-misses | 12.3 M | 18.2 M | **+47.7%** |
| iTLB-load-misses | 76 K | 149 K | **+96%** |
| L1-dcache-load-misses | 5.57 M | 5.60 M | +0.5% (flat) |

So the binary is not doing meaningfully more *work* (+1.25% instructions) — it is **stalling
on instruction fetch**. IPC drops 2.94 → 2.85. Data cache is untouched, which rules out the
obvious "worse data layout" reading.

Three independent confirmations:
- **Monotonic across three trees.** icache 12.5 M → 16.1 M → 19.0 M and iTLB 77 K → 117 K →
  155 K for good → synthetic-without-§7.5 → the real merge, matching their 1.021 / 1.030 /
  1.080 ratios. Instructions over the same three are 16.7 / 18.0 / 17.0 G — not monotonic,
  which is the point.
- **A small-footprint row is completely unaffected**: `fib` measures **1.0010** on the same
  pair where mandelbrot measures 1.0548. Exactly what a footprint effect predicts and what a
  per-operation cost would not.
- **The growth is in RUNTIME-emitted code, not the kernel.** Both binaries are the same size
  (34.06 vs 34.08 MB) and lower the same number of arms (158 vs 159 — `std/` is identical
  between them), so what grew is the machine code emitted *per arm*.

**Why it needs both halves, finally explained.** §7.5 emits more machine code per JIT'd arm;
ADR-302's std causes roughly **twice as many arms to lower** (158 vs 76 on the old std). The
old std's 76 fatter arms still fit; ADR-302's 158 do not, and the working set spills out of
the L1 icache and the iTLB. Neither change crosses the threshold alone, which is exactly why
both parents measure clean.

**Fix direction.** Reduce emitted code per arm, or improve JIT code locality (hot/cold
splitting, or huge pages for the JIT region — the iTLB doubling specifically suggests the
latter is worth a try). Note `BROOD_NO_XCALL=1` does **not** help, so this is not the
deferred re-lowering ceremony; `115faead` (RootsBuf) reproduces about half the slowdown and
about half the icache growth, consistent with its inlined root-stack manipulation being the
larger part of the per-arm growth.

**Superseded next step** (kept for the record): why does the new root-stack representation
cost ~2% *only* when ADR-302's std is what is running? The merge inherits ADR-302's much
larger lowering volume (160 arms vs 88) and presumably a different frame/rooting profile, so
"more frames pushed through a costlier root stack" is the shape to test — `perf stat` on the
two binaries, or the `ns_*` counters under `--features perf-stats`. Then account for the
~2 points that RootsBuf does not explain.

**Reproducing.** The harness is left in `target/ki100/` (gitignored): `probe.sh` is a
`git bisect run` probe, `measure.sh` the raw timer, `bin/` the built binaries. The
discriminator is a **ratio against a fixed reference binary, measured interleaved** — not an
absolute time, because this box's governor wanders between turbo plateaus and an absolute
threshold bisects the governor instead of the code. `BENCH_N=1400` so compute dominates and
the separate boot effect (below) stays under 1%; `BROOD_NO_STDIMAGE=1` on every candidate for
symmetry, which also avoids a per-worktree `nest` + image build.

**A second, separate effect: boot +2.8 ms (+14.5%).** It tracks the stdlib growing (startup
image 5199 -> 5332 bindings) and is best read as feature cost, not a defect. It is excluded
from the measurement above by the large `BENCH_N`.

**How it was verified before being believed** (the box makes a single number worthless):
min-of-3 interleaved harness invocations; build-parity `make ab` (mandelbrot **+8.1% against
a 0.9% floor**, solo at N=11); an **unpinned** re-run (**+6.8%**), because `make ab` pins to
one core and the new binary lowers *more* arms, exactly the confound CLAUDE.md warns of; and
an output check — every binary prints checksum `6129302`, so the arms do equal work.

**Why nobody saw it for three releases.** Nothing in the benchmarks repo measures timing
except a hand-run harness; its daily gate proves the rows *run* and that checksums *agree*,
and both stayed green throughout. Hardening landed there: `bench/staleness.py` compares the
commit the published column was measured at against the binary under test and fails on a
version boundary (sabotage-verified). It measures nothing deliberately — a timing gate on a
shared CI runner cannot separate a 7% regression from a turbo plateau, and a gate that cries
wolf gets ignored.

**A trap worth keeping.** `git log A..B` orders by date, not topology: `2dc7d2e6` appears
inside the window while being an ancestor of *neither* endpoint measured earlier, so reading
that list as a bisect order wrongly "excluded" ADR-302 — which turned out to be half the
interaction. Check `git merge-base --is-ancestor` before reasoning about a range with merges
in it, and bisect with `git bisect`, which understands the graph.

## KI-99 — a dist handshake EOF'd under full-suite load, so a drop-warning test saw no send ⚠️ WATCHING 2026-08-31

**Symptom.** In a full `make test`, `cli::distribution
a_dropped_send_to_an_unregistered_name_warns_once` failed try 1 (passing on try 2):
`assertion left == right failed: B should warn exactly once for the repeated inbound send
(dedup)` — B warned **0** times. Its captured stderr names the cause:

```
--- B stderr ---
dist: incoming connection failed: failed to fill whole buffer
```

**Reading.** Not a dedup miscount — the link never came up. `failed to fill whole buffer`
is a `read_exact` hitting EOF mid-frame during the handshake, so the inbound send that
should have been dropped-and-warned never reached B at all. Under full-suite load the
box is running ~nproc test processes plus this test's two nodes.

**Status.** One sighting, retry-absorbed; 6/6 green solo afterwards. Recorded rather than
chased because the *output was captured this time* (KI-80's standing lesson) and it points
somewhere specific. If it recurs: the question is why a local handshake EOFs under load.
`accept_link`'s per-read `SO_RCVTIMEO` and `read_frame_capped` are the code — the same
pair KI-97 item 1 rewrites for the trickle-DoS deadline, so that fix should re-test this.

## KI-98 — `process_limit_test.blsp:114` timed out under a full `nest test`, twice in five runs ⚠️ WATCHING 2026-08-31

**Symptom.** In a full `nest test`, "proc/flag :max-mailbox › the handler can drain and
clear the bound — the process recovers" fails with `code = :timeout` after its 30 s
`after`: the flooded worker (parked on `(:never nil)` with `:max-mailbox 8`, flooded
with 32) never sent `[:recovered …]`. Two sightings in five full runs on 2026-08-31
(one on a stale pre-KI-89-fix binary, one on the fixed tree); the sibling cases in the
same file stayed green.

**What it needs to fail.** The worker's breach flag arms at delivery 9
(`note_mailbox_bound`); ANY subsequent wake re-enters `receive_match`, whose entry
check raises the catchable E0046 (`mailbox.rs`, the ADR-307 probe). `:timeout` means
the worker never re-entered the scan for 30 s with 24+ undelivered messages queued —
a missed-wake shape (the KI-88 family), or an E0046/drain path that hung.

**Not reproducible on demand:** 16 solo runs of the file green; 10 runs under 8-way
CPU load green; only the full-suite context shows it. Sighted on a tree carrying the
KI-91/92 mailbox fixes — those touch the receive scan/consume and the msg-roots slot
table, not delivery or wake, and a 3-run pre-fix control (worktree at `e569ca4f`)
didn't fire it — but three runs is not a base rate; neither direction is established.

**If it recurs:** the failing run's log already names the case; the missing fact is
whether the worker EVER re-entered `receive_match` after the breach armed. Arm
`BROOD_SCHED_DBG=1` (per-pid run/park/end lines) and keep the whole log — the
question is one process's lifecycle between its 9th delivery and the timeout.

**Fourth sighting, 2026-08-31 (KI-97 session):** same case, same `:timeout`, in a full
`make test` on the merged tree — try 1 only, absorbed by the retry, and solo re-run green
(13/13). Consistent with the established shape; no new information, and no
`BROOD_SCHED_DBG` armed. Base rate now ~4 in 10 full runs.

**Third sighting, 2026-08-31 (KI-96 session):** the same case, same `:timeout`, under a
full **tree-walker** suite half (`BROOD_VM=0 cargo nextest run`, 16 GB cap) — first time
seen at ceiling 0, so it is not VM-specific. Solo re-run under the same cap and engine:
green (13/13). Still full-suite-context only; no `BROOD_SCHED_DBG` was armed (the run was
a KI-96 verification pass, not a hunt). Base rate so far: ~3 in 8 full runs across both
engines.

## KI-91 — `receive`'s consume path removed the matched message by a stale scan index ✅ FIXED 2026-08-31

**Symptom** (constructed — found by a code audit, never observed as a failure): a `receive`
whose matching clause carries a `:when` guard that runs a *consuming* nested `receive`
delivers wrongly, silently: the guard's consumption shifts the queue, and the outer
receive's consume path then `queue.remove(*i)`s by the pre-shift index — deleting a
**neighbouring** message (lost, and for an L1 `Payload::Local` its msg-roots slot is
wrongly freed) while the **matched** message stays queued and is delivered again by the
next receive. A second shape from the same root: after a *peeked* non-match whose guard
consumed entries ahead of the cursor, `*i += 1` skipped an unexamined message, so a
matchable message read as absent until two more arrivals.

**Cause.** The matcher runs with the mailbox lock RELEASED (deliberately — the deep copy
must not stall senders), and the scan identified its candidate by queue *position* across
that window. The soundness comment claimed "only the owner removes from its own mailbox,
so `*i` is stable" — refuted by the owner itself: a guard's consuming nested receive.
`reinsert_at_seq` (2026-07-31) had already documented and fixed exactly this hazard for
the NON-match re-insert path; the match path and the cursor advance still trusted the
index.

**Why it survived.** Every receive test's guards are pure except `mailbox_order_test.blsp`
— and that one *declines* its clause, so only the (already fixed) non-match path was ever
exercised. The match-path bug needs match + shift in the same scan, which no test built.

**Fix** (`process/mailbox.rs`): a candidate's identity is its arrival `seq`, never its
index. The consume path re-identifies by seq (O(1) fast path when unshifted, else a
binary search over the seq-ordered queue; `None` — the guard consumed the candidate
itself — removes nothing), and each scan-loop top re-anchors `*i` against the last
examined seq (same fast-path-then-search shape as `reinsert_at_seq`).

**Guard.** `tests/receive_consume_test.blsp` "a matched candidate is removed by seq, not
by the stale scan index". **Sabotage-verified**: with the consume path reverted to
`remove(*i)`, it fails `actual: [:dup 1]` (duplicate delivery) and the `[:tail n]` sweep
loses `2` (the neighbour the stale index removed).

## KI-92 — an L1-delivered `nil` message aliased a free msg-roots slot ✅ FIXED 2026-08-31

**Symptom** (constructed — audit finding): send `nil` to a process parked in a selective
receive, then more messages while it stays parked: the receiver observes the later
message where `nil` belonged and `nil` where the later message belonged — an
order-and-value swap; other interleavings give duplication or loss. Silent.

**Cause.** `Heap::msg_roots` (the ADR-177 parked-message slot table) marked a free slot
by its *content* — `Value::Nil` — and `nil` is a legal message value that
`copy_cross_heap` passes through as an atom. A delivered `nil` therefore wrote a slot
indistinguishable from a tombstone; the next `msg_root_add` reused it; two queued
envelopes then read one slot, and the first consume tombstoned it under the second.

**Why it survived.** A bare `nil` message is rare (the idiom is tagged vectors), the
aliasing needs the L1 path (receiver parked) with the `nil` still queued when the next
send lands (a selective receive), and even then most consumption orders are observably
equivalent — only order-sensitive reads diverge.

**Fix** (`core/heap.rs`): freeness is tracked **out of band** — `MsgRoots { slots,
free }`, a free list beside the traced slots. A freed slot is still overwritten to `nil`
so the GC never keeps a consumed message alive; a double free now trips a
`debug_assert`. Bonus: `msg_root_add` is O(1) instead of an O(live-slots) scan performed
under the sender-side mailbox lock (the KI-56 hold-cost class).

**Guard.** `tests/receive_consume_test.blsp` "an L1-delivered nil message does not alias
a free slot". **Sabotage-verified**: with the nil-content sentinel restored, the
double-free tripwire fires in the victim (debug builds) and the case fails `:NO-REPLY`;
the release-shape failure is the `[:result [:a 1] nil]` swap.

## KI-93 — the net reactor thread's death was silent: every socket hung, `tcp-send` kept succeeding ✅ FIXED 2026-08-31

**Symptom** (constructed — audit finding): any panic inside `reactor_loop` (every socket
in the runtime multiplexes on that one thread) or a fatal `poll` error ended the thread;
`Reactor::cmd` discarded the channel send error, so `tcp-send`/`tcp-listen`/`tcp-connect`
kept returning `Ok(())` into a dead channel, no `[:tcp-closed]`/`[:tcp-error]` was ever
emitted again, and every socket-owning process parked in `receive` forever. Zero
diagnostics anywhere.

**Cause.** `net.rs` spawned `reactor_loop` bare — no `catch_unwind`, no death hook —
where `dist/heartbeat.rs` deliberately respawns on panic; and `cmd()`'s
`let _ = self.tx.send(cmd)` encoded "a send can only fail if it panicked, in which case
every socket is dead anyway" without making that state *observable*.

**Why it survived.** It needs a reactor bug to fire, and none is known — this was the
blast-radius hole around every future one, exactly the class where the failure is a
silent hang three layers from its cause.

**Fix** (`net.rs`): dead-loud, deliberately NOT restarted (the mio `Poll`, every fd's
registration and all rustls state died with the thread). A `catch_unwind` wrapper runs
`reactor_died`: set `REACTOR_DOWN` (SeqCst), one stderr line, then drain the registry and
fail every socket at its owner — `[:tcp-error id "net reactor died"]` then the terminal
`[:tcp-closed id]`, so either receive-loop shape terminates. `connect`/`listen`/
`tls_listen`/`tls_request`/`send` gate on `reactor_up()`, and the three creators re-check
the flag after inserting (flag-before-sweep + recheck-after-insert closes the race — an
entry can never slip in behind the sweep and hang its owner).

**Guard.** `crates/lisp/tests/net_reactor_death.rs` — its own test binary (the reactor is
a process-global singleton; nextest gives it its own process), driven by a
debug-assertions-only `Cmd::DieForTest` that panics the reactor exactly as a real
event-loop bug would. **Sabotage-verified**: with the `catch_unwind` + hook removed,
`listen` keeps succeeding after the death and the test fails its 10 s deadline — the
silent-hang behaviour, reproduced.

## KI-94 — a green process's death orphaned its OS subprocesses ✅ FIXED 2026-08-31

**Symptom** (constructed — audit finding): a process that `os/spawn`ed a child and exited
— crash or normal return — without `os/close` left the OS child running (never killed,
never reaped), its registry entry alive for the life of the runtime, and both its reader
threads draining output into a dead pid's mailbox (a no-op delivery) for the child's
whole life. A supervisor restart loop accumulated one orphan child + two threads per
restart.

**Cause.** `subprocess::close`'s only caller was the `proc-close` builtin
(`builtins/io.rs`), `Proc` recorded no owner, and `retire_pid_tail` — which already
closes a dead process's *sockets* — had no subprocess counterpart.

**Why it survived.** Every proc test closes explicitly, and nothing asserted on the OS
child's lifetime after an owner crash; the leak is invisible to correctness gates.

**Fix.** `Proc.owner` (the spawn-time subscriber) + `subprocess::close_process_procs(pid)`
called from `retire_pid_tail` beside `close_process_sockets` — Erlang port semantics: a
child dies with its owner. **This is a deliberate semantic change**: code that relied on
a child outliving its spawning process must hand the handle to a longer-lived process
first (there is no `proc-controlling-process` yet; the socket layer has one).

**Guard.** `tests/proc_test.blsp` "an exited owner's subprocess handle is closed and its
child reaped". Verified red on the pre-fix binary (`actual: :wrote` — the orphaned `cat`
accepted the write), green after.

## KI-95 — `promote` forwards only closures/envs: DAG-shaped data duplicates per referrer, exponentially with nesting ✅ FIXED 2026-08-31

**Status:** ✅ fixed, exactly along the fix shape below. `PromoteForward` now forwards
pairs, vectors (shared with ranges/seq-views — one `VecId` slab), map/set trie nodes and
strings, keyed on the **handle types themselves** rather than `index() as u32` — handle
`Eq`/`Hash` use the canonical identity (region + index + the LOCAL nursery/old AGE bit),
which also closes a latent collision in the pre-existing closure/env tables: a nursery
and an old-gen closure at the same slab index are distinct objects and could resolve to
one RUNTIME copy. Guards: `core::heap::promote_sharing_tests` (5 tests) — the 16-level
pair DAG promotes **17** cells where the pre-fix code promoted **131 071**, plus a
shared-tail case, a vector DAG, a map/string sharing case, and the stride bound below.

**The cost, measured (the entry demanded it).** Registration is a per-copied-node map
insert on the `def`/`spawn` path. Three rounds:
- SipHash was ~2% of the `spawn` row's instructions → replaced with a multiplicative
  `HandleHasher` (the `table.rs` `IdentityHasher` pattern); `spawn` then measured
  **fewer** instructions than base (the closure/env tables shed their SipHash too).
- A bulk `def` of a long list (`sort`'s 375k-cell spine) paid ~107 instructions/cell in
  `HashMap::insert` (+5% on the row, confirmed by interleaved `perf stat`) → long spines
  (> 64 cells) register every **8th** cell (`SPINE_REG_STRIDE`), head always registered.
  A walk re-entering the spine re-copies at most 7 cells per referrer before joining the
  registered copy — growth stays O(n), the exponential class stays closed, and `sort`
  came back to +0.7% on a 0.7% floor.
- Final `make ab --floor`: `sort`/`spawn`/`startup`/`spawn-live`/`supervisor` all read
  noise at their floors; `spawn`/`supervisor` instructions are *below* base per
  interleaved `perf stat` (the concurrency-row wall deltas were the documented ±
  cross-binary layout noise, checked as CLAUDE.md prescribes).

Original entry:

**Symptom** (constructed — audit finding, not yet observed): a `def` (or `spawn`
argument) of a value whose substructure is *shared* — which immutable path-copying code
produces routinely — copies the shared part once per reference into the append-only
RUNTIME region, and the duplication compounds: `(let (a …) (b (list a a)) (c (list b b))
…)` promotes `2^n` copies of `a`. No depth or size cap (the message path has
`MAX_MESSAGE_DEPTH`; promote has nothing). Presents as RUNTIME-region growth / slow
memory climb on `def`-heavy or spawn-heavy code, never a crash or wrong answer.

**Cause.** `PromoteForward` (`core/heap.rs`) maps only closures and envs. The comment
beside it reasons pairs/vectors/maps are "acyclic by construction so they need no
forwarding (they'd only ever be a finite tree to re-copy)" — but acyclic ≠ tree, and
`promote_list` stops only at an already-**shared** cell, not an already-**copied** one.
The GC's own flush path forwards all of these (`flush_pair`/`flush_vector`/`flush_map`
in `gc.rs`), so the collector handles DAGs and the promoter does not.

**Why it survives.** Correctness is unaffected — the values are immutable and the
duplicates are `equal` — so every gate is green; only memory and promote time observe it,
and only on DAG-shaped values.

**Fix shape** (not started): extend `PromoteForward` with source-id → promoted-id maps
for pairs/vectors/maps/strings, mirroring the flush tables. Promote is on the
`def`/`spawn` path, so the extra hashing must be measured — `spawn`, `spawn-live`,
`startup` rows at minimum — and the maps should stay lazy/small for the dominant
no-sharing case. A guard at fix time: promoting an n-level self-sharing structure must
grow `cur_code().pairs` by O(n), asserted directly.

## KI-96 — a remote monitor's `PENDING_REMOTE` entry survives its own DOWN ✅ FIXED 2026-08-31

**Status:** ✅ fixed, along the entry's own "clean seam": the dist inbound path now
*recognises* a monitor DOWN because the DOWN is no longer an ordinary send — it travels
as a dedicated **`Frame::Down`** (wire protocol v7; `watcher_pid`/`mref`/`target_pid` +
the `Message` reason). The watched node's `fire_down` ships it (`dist::send_down`); the
watcher's node handles it in `deliver_remote_down`, which retires the `PENDING_REMOTE`
entry FIRST, then delivers the `[:down mref pid reason]`. The dying pid is node-qualified
by the *authenticated* peer on the receiving side, never wire data — same rule as the
other coupling frames. Bonus closures: the `:noproc` immediate-fire path retires its
pending entry the same way (it leaked identically), and `drop_pending_remote` now prunes
emptied node keys. Guard: the two-node test
`a_delivered_remote_monitor_does_not_fire_again_on_node_down` (monitor → target dies →
DOWN received → node killed → `[:nodedown]` → assert no second `[:down]` on that mref),
sabotage-verified — with the retire commented out it fails `SECOND-DOWN-BUG
:noconnection`, the entry's exact predicted duplicate. Verified: distribution suite 4/4
loops + a `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` pass, full suite green both engines
(modulo the documented wasm-under-cap exception and one KI-98 recurrence, logged there),
clippy on CI's flags.

Original entry:

**Symptom** (constructed — audit finding): entries in `PENDING_REMOTE`
(`process/monitor.rs`) are removed by `demonitor`, by the watcher's own death, or by
node-down — but **not** when the watched remote target dies and the peer's
`[:down mref …]` arrives (it lands as an ordinary send; no hook runs). Two consequences:
a long-lived watcher leaks one entry per completed remote monitor, and a later node-down
for that peer fires a **second** `[:down mref pid :noconnection]` for an mref that
already delivered — breaking the one-shot guarantee, which a `gen/call`-style receive
pinned on that ref can mis-route (the recorded receive-mark makes a stale pinned message
cheap to hit).

**Fix shape** (not started): remove the entry when the DOWN actually delivers. The clean
seam is the dist inbound path that recognises a monitor DOWN frame; failing that, a
tombstone consulted by `handle_node_down`. Needs a two-node test: monitor, kill target,
receive DOWN, then drop the node — assert no second DOWN.

## KI-97 — consolidated hardening gaps from the 2026-08-31 stability audit ⚠️ OPEN 2026-08-31

One entry so the audit's remaining confirmed-by-reading findings have a number; none has
been observed in the wild. Ranked. Each is independently fixable; strike through items
here as they close.

1. ~~**Pre-auth handshake trickle DoS**~~ ✅ **FIXED 2026-08-31.** The 10 s handshake
   timeout was `SO_RCVTIMEO` — a bound per `read`, not per handshake — so `read_exact`
   stayed alive on one byte per 9 s (~10 h for a 4 KiB frame); 128 such sockets exhausted
   `HandshakeSlot` and every further inbound link was shed **with no log**. Both halves
   are closed as the entry prescribed. A `Deadline` `Read`/`Write` shim now wraps the
   whole exchange (`HANDSHAKE_DEADLINE`, 15 s) on **both** the accept and the dial side —
   an absolute instant no amount of trickled progress can restart, checked before *and*
   after each call so `read_exact`'s short-read loop cannot run past it. Writes are
   deadlined too: a peer that completes its side and stops reading would otherwise park
   us in `write_all` on a full socket buffer, holding the same slot from the other
   direction. The per-read `SO_RCVTIMEO` stays — the two bound different things (a
   *silent* peer between bytes vs a *slow* one across them). Shedding now reports itself
   (`note_shed_handshake`): reaching the cap means inbound distribution is effectively
   down, which the node used to say nothing about (the KI-36 lesson); rate-limited to one
   line per 60 s with a cumulative count, since a per-shed line would be its own
   amplification vector. Guards: 4 tests in `dist::tests`, sabotage-verified — with the
   deadline no-op'd the trickle test fails *and takes 4.4 s delivering the whole 4 KiB
   frame one byte at a time*, which is the attack itself. One real bug found while
   testing: `now_millis()` counts from process start, so 0 is a legitimate timestamp in
   the first millisecond and the obvious `0` "never warned" sentinel would have swallowed
   the first warning of a flood that began at startup — the sentinel is `u64::MAX`.
2. **Untimed blocking calls on scheduler workers** (ADR-059 violations). **Three of the four are fixed
   2026-09-01; the fourth is ADR-059 Phase 2, not a patch.**
   - ~~`os/run-process`'s `status()` with **inherited stdin**~~ ✅ **FIXED** — it now hands
     the child `/dev/null`, so a `git` credential prompt reads EOF and fails fast instead
     of pinning a worker forever with no timeout or `try` able to recover it. This matches
     the analogue rather than departing from it (Emacs `call-process` uses `/dev/null` when
     INFILE is nil), and every in-tree caller is `git`/`sh` needing no stdin — including
     `std/tool/workspace.blsp`, which runs `git` across sibling repos and is exactly where
     a credential prompt appears. Guard: `crates/cli/tests/run_process_stdin.rs`,
     sabotage-verified. It has to spawn `brood` with a **pipe** as stdin that is never
     written or closed: under an ordinary harness stdin is already at EOF, so a naive test
     passes either way and proves nothing. Without the fix it hangs the full 20 s.
   - ~~`%node-connect`'s unbounded DNS resolve on the worker~~ ✅ **FIXED** —
     `connect_timeout` bounded the connect but never the *lookup*, and `to_socket_addrs` is
     a blocking libc call with no timeout of its own, so an unreachable DNS server pinned
     the dialing worker for the resolver's own timeout (tens of seconds, longer with
     retries across several `nameserver` lines). `dist::resolve_timeout` now runs it on a
     throwaway thread under a 5 s wall-clock bound. The thread is deliberately **detached**
     on timeout: a blocking `getaddrinfo` cannot be cancelled, and detaching is precisely
     what keeps the caller bounded; it touches nothing after its send, and the call rate is
     set by a user-initiated `node/connect` or `reconnect/watch`'s backoff, not by inbound
     traffic.
   - ~~`proc-send`'s `write_all` under the per-child mutex~~ ✅ **FIXED** — a pipe write is
     bounded by the OS buffer, so a child that stops draining its stdin blocked the calling
     thread forever; the old comment justified this as "the blocking contract `tcp-send`
     also has", but `tcp-send` went async in ADR-143, so nothing held that up. Each child
     now has a **writer thread** fed by a bounded channel — `dist`'s shape, for the same
     reason — and `proc-send` queues instead of writing. Deliberately **not** a per-call
     write timeout: timing out mid-`write_all` would leave a *partial* message in the
     child's input stream, silently corrupting its protocol, which is worse than the hang.
     A full queue is reported (the child has stopped reading) rather than buffered without
     bound; dropping the sender still closes stdin, so EOF semantics are unchanged. Guard:
     `tests/proc_test.blsp` writes ~1.6 MB to a `sleep 30` that never reads — a **proof**
     rather than a smoke test, since a pipe buffer is ~64 KiB and a synchronous `write_all`
     of that size into a non-reading child cannot return.
   - **Still open: `read-line` holding the global stdin lock** (`builtins/io.rs`). Unlike
     the other three this is not a patch: doing it properly *is* **ADR-059 Phase 2**
     (terminal input on a reader thread delivering to a mailbox), which changes `read-line`
     from a blocking call into a park. Left whole rather than half-done.
   - Not affected, checked while here: `%os-cmd` uses `Command::output()`, which nulls
     stdin. It stays untimed, but a long-running child there is doing what was asked.
3. ~~**Thread-spawn panic classes**~~ ✅ **FIXED 2026-09-01.** `std::thread::spawn`
   *panics* when the OS refuses a thread (EAGAIN under thread/fd pressure), and the runtime
   spawned threads at attacker-influenced rates while treating that as impossible. Every
   site now uses `Builder::spawn`, which returns the error instead:
   - **The timer's `Once` was the worst.** `TIMER_STARTED.call_once` is poisoned by a panic
     inside it, so one refused spawn made *every later* `call_once` panic — and `arm_timer`
     backs `sleep` and every `receive … (after ms …)`, so a single transient failure broke
     all timeouts runtime-wide, permanently. It is now a plain CAS that a failed spawn
     **releases**, so a later call retries; queued deadlines are late, never lost. Guard:
     `the_timer_thread_start_is_retryable_not_a_once`, sabotage-verified — and note the
     first version of that test asserted only the *flag* and passed against a `Once`-shaped
     sabotage. It now asserts a genuinely new thread reached `timer_loop` (a one-increment
     counter), which is the only assertion that separates "marked started" from "started".
   - **`ensure_workers` stranded `LIVE_EXECUTORS` above reality** — it seeded the gauge with
     `n` and its fallback used the *panicking* `spawn`. Two consequences: a second EAGAIN
     panicked inside `call_once` (poisoning `WORKERS_STARTED` and unwinding whoever started
     the pool), and a short pool left the gauge above the truth, so `enqueue`'s safety net —
     which only fires at `LIVE_EXECUTORS == 0` — could never spawn a drainer and work was
     stranded with nothing alive to run it. Now both gauges are corrected to the number that
     actually started, the fallback retries at the default stack size, and a reduced pool
     says so.
   - **The dist acceptor no longer dies from one EAGAIN.** A refused per-connection thread
     panicked *inside the accept loop*, unwinding the acceptor and closing the listener for
     good — the node stopped accepting inbound links until restart. `spawn_bg` reports and
     sheds instead; dropping the closure closes the accepted socket and releases the
     `HandshakeSlot` on its own. Same treatment for the gossip dial (one thread per gossiped
     peer, up to 4096 per `Peers` frame — the highest-rate spawn in the runtime, and a
     refusal there now also clears the `PENDING_DIALS` marker so the peer stays dialable)
     and the per-link reader/writer threads.
   - **Poison-tolerant locks**, per `core/sync.rs`'s own policy: the timer thread's condvar
     waits (a poisoned wait killed every deadline runtime-wide) and the `net.rs` /
     `subprocess.rs` registry takes now recover instead of `.unwrap()`/`.expect()`.
4. ~~**Smaller, same families**~~ ✅ **ALL FIXED 2026-09-01.**
   - ~~`session::open` allocates up to 64 MiB from a length prefix before the AEAD tag is
     checked~~ ✅ **FIXED** — `vec![0u8; len]` was the obvious spelling and the wrong one:
     `len` is four bytes off the wire and the Poly1305 tag that proves the frame genuine is
     inside the bytes not yet read, so the allocation happened strictly *before* anything
     about the frame was authenticated. A peer spent 4 bytes to make us commit 64 MiB, then
     stalled — ~16-million-to-one amplification, repeatable per link. `read_claimed` now
     grows the buffer a 64 KiB chunk at a time *as bytes arrive*, so the cost is
     proportional to what is delivered. Guard: `a_claimed_length_is_not_an_allocation`,
     driven through `OpenKey::open` and sabotage-verified. (An earlier version called the
     helper directly and passed with `open` reverted — it guarded the helper while the bug
     sat at the call site. Exercise the entry point.)
   - ~~every wire-decoded symbol interns permanently~~ ✅ **FIXED** — the interner is
     append-only by design (`NAMES` is a lock-free `boxcar::Vec`; nothing frees an id),
     which is right for a program's own symbols and wrong for wire symbols, whose spellings
     the *peer* chooses: a stream of distinct names grew `NAMES`, the global id map and every
     thread's intern cache, permanently. Refusing to mint is not available (a legitimate peer
     may send a symbol we have not seen), so the bound is on the count —
     `MAX_WIRE_SYMBOLS` (2^20, far above any real workload), past which the frame is rejected
     and the reader tears that link down. A **known** name never touches the counter, so an
     established link is unaffected. Guard: `a_peer_cannot_mint_symbols_without_limit`,
     driven through `decode_frame` and sabotage-verified; it also pins that a refused name is
     *not* interned, which is the leak itself.
   - ~~the ADR-232 dedup set beside it~~ ✅ **FIXED** — same shape: for an inbound drop the
     distinct names are the peer's to choose. Capped at 4096, past which it warns without
     recording. A flood of distinct names is itself worth seeing, and the alternative is a
     remote-controlled set that never shrinks.
   - ~~a non-`WouldBlock` accept error breaks the edge-triggered drain~~ ✅ **FIXED** —
     `Err(_) => break` ended the drain on any error, and the registration is
     **edge-triggered**, so whatever was already queued in the backlog was stranded until
     some *later* arrival happened to re-arm us. Silently. `ConnectionAborted` (a peer that
     died between the readiness event and `accept`) is a fact about one connection, so it
     now `continue`s; anything else still breaks but *says so*, since a listener that had
     stopped accepting was previously indistinguishable from one nobody was connecting to.
   - ~~`tls_request`'s untimed connect + the close-before-connect race~~ ✅ **FIXED** —
     the connect is now bounded (`TLS_CONNECT_TIMEOUT`, 5 s, per resolved address) instead
     of waiting out the kernel's multi-minute SYN timeout while holding the caller's
     registry entry and request buffer. And the race is closed: the registry entry is
     inserted *before* the connect, so an owner closing meanwhile removed it while the
     thread went on to hand the socket to the reactor — installing a live connection under
     an id nothing owned and nothing could close. The thread now re-checks the registry and
     drops the stream instead. Its `.expect("spawn tls connect thread")` is gone too (item
     3's panicking-spawn class).
   - ~~a half-closed stream is never reaped~~ ✅ **FIXED** — the idle branch required
     `!c.read_done`, which excluded a half-closed stream from the only reap that could
     collect it: `accepted_at` is cleared once the owner claims the connection and `closing`
     is only set by an explicit close, so a peer that shut its write half while the owner
     never closed the socket leaked the entry and its fd for the runtime's life. `read_done`
     now counts as quiet, gated on nothing being queued outbound — a half-close is a
     legitimate "I am done sending, you may still reply", and reaping with unwritten data
     would discard that reply.
   - ~~`record_remote_link` skips the liveness check~~ ✅ **FIXED** — its doc had always
     claimed it "returns whether `local_pid` is currently alive"; it returned `()`. The
     check matters on the **inbound** side, where `to_pid` is wire data: a peer naming a
     dead or never-existent pid created a `REMOTE_LINKS` entry nothing would ever remove,
     because the sweep runs from `deregister`, which for that pid has already happened or
     never will. Now checked inside the critical section (the shape `link` and
     `add_monitor` already use), and a dead target gets `:noproc` sent back to the peer —
     exactly what a *local* `link` to a dead pid delivers.
   - ~~sysmon subscriptions are never reaped while `ARMED == 0`~~ ✅ **FIXED** — `clear_if`
     gated on `armed()`, which is the mask of *selected kinds* and so is 0 for a subscriber
     that selects nothing; such a subscriber was never reaped at all. Gated on the
     subscription count now. Guard: `a_subscriber_selecting_nothing_is_still_reaped`,
     sabotage-verified, and it asserts the `!armed()` precondition explicitly so the test
     cannot quietly stop exercising the trap.

Also recorded by the same audit, elsewhere: the four fixed bugs above (KI-91–94), the two
correctness items KI-95/96 (both fixed later the same day), and the performance
candidates in `compute-frontier.md` §7.8.

## KI-88 — one spawn of a warm burst is created, promoted, registered — and never scheduled ⚠️ WATCHING 2026-08-31 (DORMANT since 2026-08-30 — no reconstructable binary reproduces it; still gates the tree-walker→VM router's default)

**Status note (2026-08-31, KI-96 session):** the *header* of this entry advertised a
"deterministic repro" for a day after session 4 had already found it dormant — which is
how it got picked up as the next item. It does not reproduce: the canonical repro (the
full chaos2 file, `BROOD_TW_REENTRY=1`, router confirmed live at 394 routed closures)
passes **10/10** at `62eac84c`, on top of session 4's 15/15 and 8/8. Read the session 5
note at the end before spending time here.

**Symptom.** In `breakage/chaos2_process_genserver` P47 (50-reader burst against a 10k-entry
map server), exactly ONE early reader (index 2–4, stable within a config) never runs: its
spawn returns a pid, `BROOD_TRACE_PROMOTE` shows all 50 thunk promotions, the mailbox is
registered — but the process never executes its first instruction. No death line (a body
that never runs exits nothing). The collector then times out and the section fails with
`nth: expected a collection, got keyword (:timeout)`.

**Exposure, not cause: the tree-walker→VM router** (`tw_vm_route`, 2026-08-30). With
`BROOD_TW_REENTRY=1` the failure is 10/10 deterministic; without it 6/6 pass — but the bug
predates the router's *commit lineage entirely*: a worktree at `c4af2feb` fails 3/3 the
same way (its CI pass was runner luck). The router only makes the burst run at VM speed
from a shape that used to tree-walk, which lands the spawns in the window.

**The shrink trail (all preserved in scratch, method notes inline):**
- Needs FOUR warm sections: P43+P44+P45 then P47 fails; every 3-section subset passes.
  Threshold-shaped (cumulative arms/promotions/pool state), not one interaction.
- The loop shape is irrelevant (hand-rolled `my-dotimes` fails identically); the burst
  alone in a fresh process passes at both engines, router on or off.
- `f` (the per-iteration closure) is CALLED all 50 times — instrumented — and every
  `%spawn` runs (promotion trace); the loss is between enqueue and first schedule.
- `BROOD_NO_HANDOFF=1`, `BROOD_SPAWN_RR=1`, `BROOD_SPAWN_SPILL=999999` all still fail —
  not placement policy. **`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` makes it PASS** (timing
  moves), and the armed per-deref tripwire never fires — so not a caught use-after-GC.

**Where to look next** (the KI-1 invariant family): the enqueue→first-run window under a
worker whose green process dirty-blocks a nested `receive` right after a local spawn —
`enqueue`'s self-wake elision, `wake_a_parked_peer`'s `spawns_since_park >= 2` gate, the
steal path's STEALABLE accounting against `dirty_block`'s executor bookkeeping. The repro
is cheap (`--test` on the four concatenated sections, seconds); shrink further before
theorising — tonight's session got it to "created but never scheduled" and stopped there
deliberately (scheduler sessions want fresh eyes, and the router is gated off meanwhile).

**Session 2 (2026-08-30, later) — three layers peeled, canonical repro narrowed:**
- The wedge is NOT a lost condvar notify: `MailboxState.wake_pending` (the BEAM-style
  latch, landed with this update) closes every notify-ordering hole by construction, fixed
  the four-section combo 10/10 — and the FULL chaos2 file still wedges one reader.
- The router now carries the `%receive` fence (`arm_calls_receive`): a receive-bearing arm
  is never routed, so no receive becomes a nested-vm dirty block that tree-walking would
  have handled. Necessary (a core dump showed a reader wedged at
  `receive_match ← %receive ← vm_apply ← tw_vm_route`), not sufficient.
- The surviving wedge, fully instrumented on the full file: one reader of fifty parks at
  its `receive` having executed NEITHER of the two forms before it (a kernel-table probe
  and the `send` — the server counts 49 gets; the probe table holds 49 entries and no
  nil-keyed one), while a core dump shows no thread inside it — i.e. **execution entered
  the body mid-way, at the receive**, which smells like a resume/continuation applied to a
  first-run process or an ip/frame mix-up in the capture machinery, not messaging at all.
  `BROOD_SCHED_DBG=1` (catalogued) traces the per-pid lifecycle that found this.

**Held back by it:** `BROOD_TW_REENTRY` stays opt-in (60× on the viral defer shape,
startup −6.9% — measured and waiting). The default path (router off) passes the full
suite, the whole breakage suite, and the wake-sensitive loop ×5 with the latch + fence in.
**Session 3 (2026-08-30, later still) — the observation is now maximally sharp, and it is
the strangest one yet.** With park/resume ip tracing added (`Suspended::dbg_line`, printed
under `BROOD_SCHED_DBG`), the wedged pid shows: ONE `enq`, ONE `run` (resume=false), then
NOTHING — no `end` (the outcome print is the unconditional first statement of
`handle_capture_outcome`), no `park` (`store_resume` never ran), no resume. Global counts:
10,203 `run` lines, 10,202 `end` lines. And a full core taken inside the wedge window
shows **no thread anywhere inside that quantum** — 16 LWPs: 12 idle `worker_loop`s, timer,
jit channel, the main-eval thread in `collect`'s timed receive, the main thread joining.
A Rust frame cannot evaporate mid-function without unwinding through the instrumented
tail, so one of the session's models is wrong at a level below ordinary control flow —
frame/stack corruption in the capture machinery is now the working suspicion (the
debug-assertion tripwires stay silent, so if it is memory it is valid-but-wrong, the
hardest class). Every earlier layer (wake ordering, routing of receives, message loss,
captures) has been either fixed or excluded with instruments that remain in the tree.

Fast repro: the four-section c3 combo (fixed by the latch — keep for regression); canonical
repro: the full `chaos2_process_genserver` with `BROOD_TW_REENTRY=1`, wedging one reader
per run at a stable-ish early index. The next probe for fresh eyes: a counter pair
entering/leaving `run_one_timed` (not prints — a per-pid atomic ledger), and
`BROOD_GC_VERIFY=1` on the full file (GC_STRESS alone passes, which may just be timing).

**Held back by it:** `BROOD_TW_REENTRY` stays opt-in (60× on the viral defer shape,
startup −6.9% — measured and waiting). The default path (router off) passes the full
suite, the whole breakage suite, and the wake-sensitive loop ×5 with the latch + fence in.

**Session 4 (2026-08-30 evening) — DORMANT: the repro died with the binary that carried
it.** The quantum ledger (a per-thread `(pid, started)` map set around `drive()`, with a
watchdog naming any quantum older than 3 s — now in-tree, armed by `BROOD_SCHED_DBG`) was
built to catch the vanishing quantum in the act. It never fired: after the 196-file format
reflow (c8184ab3) the full chaos2 file passes 15/15 with the router on (all-core and
2-core pinned), and a PRISTINE rebuild of b5ad1888 — the commit that failed 3/3 the same
afternoon — passes 8/8. The failure was keyed to one incremental build's layout/timing and
no reconstructable binary exhibits it. Seen many times with the root cause unfound, it is
real until proven otherwise: the router stays opt-in. If the wedge is next seen anywhere,
arm `BROOD_SCHED_DBG=1` immediately (run/end/park/resume lines + the ledger watchdog),
take a core inside the wedge window (`kill -ABRT`; apport keeps it), and PRESERVE THE
EXACT BINARY — the artefact is the whole case.

**Session 5 (2026-08-31) — still dormant; one candidate mechanism found and CLOSED.**
Re-confirmed dormant (10/10, above). What the session did produce is a structural hole in
the exact code path this entry implicates, found by reading rather than by reproducing:
**`run_one`'s post-quantum tail ran unprotected.** `catch_unwind` wrapped `drive()` only,
and everything after it — `save_ctx`, `finish_quantum`, and the outcome routing
(`store_resume`/`park_on_receive`/`deregister`/`enqueue`) — was plain statements, with no
catch in `worker_loop` either. A panic there did two silent things at once: killed that
worker thread for good (the pool shrinks permanently; nothing restarts a worker), and
dropped the `Box<Process>` during the unwind, so the process vanished with **no
`deregister`** — no death line, no monitors, no `[:down …]`, and anything waiting on it
waited forever.

That is *exactly* this entry's signature: a `run` with no `end` (the `end` print is the
first statement of `handle_capture_outcome`, which the unwind skips), a ledger entry no
thread is inside (`ledger_exit` is likewise skipped), no death line, and a collector
timing out. Session 3's "a Rust frame cannot evaporate mid-function without unwinding
through the instrumented tail" was right about unwinding and wrong about the tail: the
tail is not a `Drop` guard, so an unwind skips it silently.

**It is not, however, a diagnosis of KI-88** — the mechanism is *loud* (the panic hook
prints and appends `.brood_crash_dump`), and no sighting ever carried a panic message. It
is now hardened and guarded regardless (`crates/cli/tests/quantum_tail_panic.rs`, with the
`BROOD_FAULT_QUANTUM_TAIL=<n>` injection knob; sabotage-verified — reverting the catch
fails the guard while its no-fault control still passes). The value for this entry is
**elimination**: if the wedge is seen again, it is not this.

One trap worth keeping from building that guard: a short program can exit *while the
panicking worker is still symbolizing its backtrace*, so the recovery's own line never
prints and the run looks like a silent vanish. The guard sleeps and runs two further waves
to outlive the unwind. A wedge investigation that ends "no diagnostic appeared" should
check it did not simply out-run one.

## KI-87 — the checker's cycle guard released the symbol it refused: `nest run` at 54 GB, three 19 GB test processes ✅ FIXED 2026-08-29

**Symptom.** `nest run` on bedit sat at 100% CPU with nothing running, RSS climbing past
54 GB until the OOM killer took it; `cargo nextest run -p brood types::` put three test
processes at ~19 GB each and swapped the machine to a halt three sessions in a row. Under a
memory cap the same runs die cleanly with `stacker … mmap failed to allocate stack: Cannot
allocate memory` inside the advisory checker: `types::check::tests::unused_use_import_is_flagged`
(it `:use`s `io`, so a loaded module's functions are in play) and `nest check src/commands.blsp`
in bedit both reproduce in ~3–10 s.

**Cause.** `sigs::InferGuard::enter` — the re-entry guard that breaks a recursive or mutually
recursive inference chain — ended in `.then_some(InferGuard(sym))`. `bool::then_some` builds
its argument **eagerly**, so on the refusal path (symbol already in flight, or the depth cap) a
guard was constructed and dropped at once, and its `Drop` removed `sym` from the in-flight set
— the *outer* inference's mark. The refusal un-guarded the very symbol it refused. Latent since
`aadd10c1` (2026-07-07): a body referencing its partner once was refused and moved on, and the
memo eventually bounded the re-walks. `0f64f600` made the parameter-demand walk consult a
callee's inferred signature, which turned every mutually recursive pair whose bodies reference
each other *twice* — `require-one` ⇄ `%require-await` in the prelude — into unbounded nesting:
refusal, un-mark, second reference re-enters, each level a fresh 1 MB stack segment, until
memory ran out. The trace that found it: `[TRACE-DROP] ->seq` immediately before `->seq` was
re-entered at the same depth, with the same interned symbol id.

**Fix.** Construct the guard only on the success path (`entered.then(|| InferGuard(sym))`).
Nothing may drop a guard that was never entered. No other `then_some` in the tree carries a
`Drop` type.

**Guard.** `sigs::guard_tests::a_refused_enter_does_not_release_the_in_flight_mark` (a third
`enter` after a refusal must still be refused) and `…the_depth_cap_refuses_without_releasing_anything`;
`check/tests.rs` `mutually_recursive_loaded_functions_infer_in_bounded_time` — two loaded
functions referencing each other twice, both sigs resolved and a call site still flagged.
Sabotage-verified by restoring `then_some` under `ulimit -v 4000000`:

```
a_refused_enter_does_not_release_the_in_flight_mark ... FAILED
    the refusal released the in-flight mark (the eager-drop bug)
mutually_recursive_loaded_functions_infer_in_bounded_time ... FAILED
    mmap failed to allocate stack: Cannot allocate memory (os error 12)
```

Restored: the whole `types::` set 431/431 serially at 325 MB peak; bedit's `commands.blsp`
checks in 1.4 s at 180 MB; `nest check std/ tests/` (342 files) zero warnings in 5.0 s.

**Method note.** Put `ulimit -v` in front of any test run that exercises inference — it turns
a machine-swapping OOM into a ten-second clean failure with the panic site named. And don't
let the cap leak into a *build*: the linker inherited it and died with `LLVM ERROR: out of
memory`. Build uncapped, run capped.

## KI-86 — `runtime_collector`'s three promotion tests failed under `cargo test` ⚠️ WATCHING 2026-08-29 (precondition removed; mechanism inferred, not reproduced on demand)

**Symptom.** Under a loaded box, `cargo test -p brood --test runtime_collector`: 17 pass, 3
fail — `superseded_global_versions_are_reclaimable`, `evacuation_copies_only_live_code_and_verifies`
(`expected ≥3000 promoted closures, got total=231`), `in_place_collect_reclaims_and_preserves_correctness`
(`expected ≥2000 promoted, got 239`). The redef loop completes (`:done`); a probe shows
exactly one closure promoted per iteration and a threshold of 4096, so 3000 redefs cannot
trip the count-based collector — and the tests switch it off anyway
(`set_rt_auto_collect(false)`). Yet `runtime_closure_count()` — which counts only
`cur_code()`, the CURRENT generation — comes back 231.

**Cause (inferred).** The runtime region is per-`Interp`, but the `Interp`'s scheduler
WORKER heaps share it, and a worker's `rt_gc_threshold` is the process-wide `rt_gc_floor()`
— a `OnceLock` read from `BROOD_RT_GC_FLOOR` once per process — not the test heap's
`usize::MAX`. Two tests in the same binary set that variable to 128/256 for themselves
(their comment: safe "per-test process under nextest" — true in CI, false under plain
`cargo test`, one process and parallel threads). When a floor-setting test won the race to
first construct a heap, every worker in every later `Interp` carried a 128 floor; a worker
safepoint on the shared runtime then ran the multi-generation path (`advance_runtime_multigen`
→ `age_runtime`), the main heap's `cur_code()` flipped, and the superseded closures were in
the aged-out generation the count does not see. Load matters because workers only reach a
safepoint when something schedules them. Not reproduced on demand: with the box quiet,
`BROOD_RT_GC_FLOOR=128` process-wide does NOT trip it, which is what "needs a worker to
wake" predicts.

**Why it survived.** CI runs nextest. A bisect named an unrelated expander commit
(ADR-297) "first bad" — it reshaped scheduling enough to make the race reliable, which is
what bisecting a race does — and a "clean stash" reproduction confirmed the wrong conclusion
the same way. Recorded here so the next person does not re-run that bisect.

**Fix (the precondition).** A per-heap override, `Heap::set_rt_gc_floor(n)`; the two tests
set it on their own `Interp` and touch no environment; every re-arm reads `self.floor()`.
The env var stays as the runtime A/B lever it was meant to be. 4/4 green under load since;
0 failures in the crate-wide run.

**If it recurs.** Capture the failing `Interp`'s `(%gc-stats)` `:runtime-threshold`, the
process's `BROOD_RT_GC_FLOOR`, and whether a drain was active (`drain_active()`); a
generation flip with the count-collector off is the signature, and a worker heap is the
only thing that can cause one.

**Non-reproduction evidence (2026-08-29, second session).** Attempted with the entry's own
command on the exact filed commit and could not make it fail:

- `cargo test -p brood --test runtime_collector` on a pristine worktree of **`70549e80`**
  (the commit the entry says reproduces): **20/20, and 0-for-10 in a loop.**
- Same on `origin/main` (`802bf33d`), both the default test profile and `--release`, and
  under `BROOD_VM=0`, `BROOD_NO_JIT=1`, `BROOD_NO_STDIMAGE=1`, `BROOD_TIER=1`: all 20/20.
- The suspect the entry names (`8fa9f2f7`, deopt feedback) does not touch promotion, and the
  other candidate from that day — the promote-on-8th-sighting change `7979c7a4` — is on a
  path this test never takes: its `fn` forms are freshly-consed LOCAL lists, which
  `make_closure_cached` refuses to key on (non-RUNTIME `fn_rest`), so each `def` RHS goes
  through the uncached parse and `def`'s own promote-and-dedup, unchanged by that commit.

**A confound worth knowing when re-attempting:** this machine's root filesystem hit **100%**
during this day's sessions (17 GB of `make ab` worktrees; `make ab-clean` freed it), and in
that same window an unrelated `make test` produced ten phantom in-language failures plus a
TLS-serve failure that all vanished with disk space. If the 231-of-3000 runs happened in that
window, ENOSPC is a candidate environment for it. **If it recurs on a healthy disk:** capture
the `RUNTIME-GC estimate` stderr lines (they carry total/live/baseline) and the full env, and
loop it — 0-for-10 here says one clean pass is not enough to close the question either way.

## KI-85 — the checker false-positived: a tuple shape leaked onto a `pair` member ✅ FIXED 2026-08-29

**Symptom.** With `(sig takes-str (string -> any))`:

```brood
(takes-str (first (fold (fn (a x) (cons x a)) [0] ["t"])))
;; warning: takes-str: argument 1 expects string, got 0 (…)
;; at runtime: (first …) => "t"
```

`reflect/expr-type` on the fold showed why: `(list | vector)<0>` — a sequence whose every
element is `0`, for a value whose first element is a string. The system's governing
invariant is "the checker is advisory and sound — it warns only on a *provable* misuse and
never false-positives" (docs/type-system-status.md). This is a false positive.

**Cause.** `[0]` types as the tuple `(tuple 0)` — a refinement of the VECTOR member. The
fold's result is `init ∪ step`; the step `(cons x a)` with the accumulator over-approximated
as `any` is a bare `pair` (unknown elements). `union_term` merged the two into ONE term with
tags `pair | vector`, keeping the tuple refinement (correctly — it still describes the vector
member). But `Ty::elem_ty` fell back to the tuple's elements when no `elem` refinement was
set, and `Ty::tuple_elems` returned the shape unconditionally — so both answered `0` for a
term whose `pair` member could hold anything. `first` then typed as exactly `0` (the tuple
rule deliberately drops `nil` for an in-range position), a precise literal, and the `⊆`
check against `string` fired.

**Why it survived.** The lattice corpus tests (`lattice_laws_hold`, `subtyping_agrees…`)
check the RELATIONS against each other, and every relation here was self-consistent — the
defect is in an *accessor* that projects a per-tag refinement onto the whole term, which
no relation-vs-relation test exercises. `std/` and `tests/` at zero warnings could not see
it either: the shape needs a tuple literal as a fold init *and* a later positional read,
which the corpus never does. And the checker's probe corpus (type-system-status.md) is
built from misuses that should warn, not from valid programs that must stay silent — the
direction this bug lives in.

**Fix.** `elem_ty` answers the tuple fold only when the term's seq members are exactly the
vector (`tags & SEQ_BITS == VECTOR_BIT`); `tuple_elems` answers only for a pure-vector term
(`tags == VECTOR_BIT`) — a `nil` member makes `first` nil, a `pair` member makes it unknown,
and unknown is the sound answer. A second gap the same probe exposed was closed alongside:
a lambda literal passed as a callback was never checked (`callback_sig` → `None`, "the arity
check covers those"), so its result was never compared. `lambda_sig_under` now types the
literal's body under the arrow's *declared domain* and hands the existing disjointness rule
a signature — sound because disjointness tolerates an over-approximated return, where the
obvious alternative (an inferred `(any -> R)` arrow compared by `⊆`) would false-positive on
`(fn (x) (+ x 1))` (`number ⊄ int`, though `int` under `x : int`).

**Guard.** `types/tests.rs` `elem_of_a_tuple_shape_unioned_with_a_bare_pair_is_unknown`;
`check/tests.rs` `a_tuple_shape_does_not_leak_onto_a_pair_member_through_first` (the exact
program, asserting NO warnings), `a_lambda_callback_whose_result_is_disjoint_is_flagged`
and `a_lambda_callback_with_a_merely_wider_result_is_not_flagged`. Sabotage-verified by
removing both accessor guards and the `lambda_sig_under` wiring:

```
a_lambda_callback_whose_result_is_disjoint_is_flagged ... FAILED
a_tuple_shape_does_not_leak_onto_a_pair_member_through_first ... FAILED
elem_of_a_tuple_shape_unioned_with_a_bare_pair_is_unknown ... FAILED
test result: FAILED. 403 passed; 3 failed
```

Restored: 406/406; `nest check std/ tests/` still at zero warnings.

## KI-84 — an imaged start lost every buffer type's layers (the stdlib image reset a registry the project image had restored) ✅ FIXED 2026-08-29

**Symptom.** In bedit, `nest test` passed 1306/1306 on the run that wrote `.brood/image.bin`
and failed 99 on every run that read it, deterministically, with sources untouched. The
failures were one thing wearing many faces: `special-mode's :activate hook makes the buffer
read-only` (`(is (read-only? (typed :git-status)))` → `false`), every language-mode test
(`a .ex buffer is in elixir-mode…`), the gutter tests, the tutorial's read-only prose. Probed
at module-load time: `editor/layers/*type-layers*` held **26** entries on a cold start and
**0** on a warm one. `BROOD_NO_STDIMAGE=1` made it pass. Minimal repro, in a bare process:

```brood
(def zz-alias editor/layers/*type-layers*)               ; the same 26-entry map
(%image-write "/tmp/x.bin" (list ["" (list 'editor/layers/*type-layers* 'zz-alias)]) "FP")
;; new process, load the section, then read both:
;;   restored qualified: 0     restored alias: 26
```

**Cause.** Two images, and the older one won. The **project** image's root section restores
`editor/layers/*type-layers*` with everything the app registered. Later, the first reference
to `editor/layers` materialises that module from the **stdlib** image — whose writer puts
every global under the module's prefix into its section, so the three registries are in it as
their pristine seeds (`{}`, `()`, the values at stdlib-build time, before any program ran).
`image_load_section` then `env_define`d the seed over the restored registry. Loading the same
module from **source** runs `(defonce *type-layers* {})`, which leaves an existing binding
alone; materialising had no `defonce`. The alias survived because a map is immutable and only
the *binding* was overwritten — which is why the repro reads as "a namespaced global does not
image" and why every value-shape experiment round-tripped perfectly.

**Why it survived.** Every gate that ran loaded `editor/layers` from source *before* any
registration, so `defonce` ran and the seed never landed on top of anything: the cold run of
any suite, `nest check`, `brood_suite_passes` in this repo (whose own registries are written
*after* the module loads). The brood repo has no project image of its own to read back, so
the two-image interaction never occurred here at all. The only place it could show was a
downstream project's *second* run — and `nest test`'s own "run it again" loop reports the
same test names, so a red second run read as flakiness, not as a different program. Three
earlier hypotheses were each ruled out by experiment before this one was found: a value shape
that does not round-trip (a synthetic keyword→list-of-symbols map round-trips), a `defonce`
that resolves the wrong name on the imaged path (an explicit qualified `bound?` guard changed
nothing), and the KI-72 deferral parking heap `Value`s in an unrooted `Vec` (a real latent
hazard, but switching it to defer the Rust-owned `Message` changed nothing either).

**Fix.** `image_load_section`'s deferred pass — the entries whose name is already bound —
now keeps an existing **data** binding when `reserve` is set. `reserve` is already passed
exactly when an embedded module materialises from the stdlib image (`%require-force-in`,
`(not (nil? src))`), which is the same fact: this image is pristine, older than the heap it
restores into, so a binding that already exists is later state and must win. A pre-existing
**function** is an ADR-246 autoload stub and is still replaced — the whole reason the
deferred pass exists (KI-72). A **project** image passes no `reserve` and still overwrites:
it describes a later state than the fresh process, and the `basic` test (`def` 41 → `def`
`:clobbered` → restore → 41) depends on that. Two `defonce`-shaped registries in bedit
(`interactive/*commands*`, found the same day) were a separate reset-on-reload of the same
family and are fixed there.

**Guard.** `tests/startup_image_test.blsp` — `a pristine (reserve) materialise keeps a bound
data global; a stub is still replaced`, plus its converse `a project (non-reserve)
materialise still overwrites a bound data global`. Sabotage-verified by deleting the
`if reserve { … continue }` block and rebuilding:

```
tests/startup_image_test.blsp:62: test failed: … a pristine (reserve) materialise keeps a bound data global; a stub is still replaced
    assert: (assert= {:brood [1 2 3]} si-reg)
    actual: {:brood [1 2 3]}
    expect: {}
19 tests, 18 passed, 1 failed
```

Restored: 19/19. End to end: bedit `rm .brood/image.bin; nest test` ×4 — cold write then
three warm reads — 1306/1306 each, where run 2 onward previously failed 99.

## KI-50 — the JIT leaf inliner miscompiled the ordinary counting loop ✅ FIXED 2026-08-24

**Status:** ✅ fixed. Guard: `tests/jit_leaf_frame_layout_test.blsp` (sabotage-verified — 4 of its
6 cases fail with the fix reverted).

**Symptom.** On the **default build**, silently wrong arithmetic:

```lisp
(defn sum-down (n acc) (if (<= n 0) acc (sum-down (dec n) (+ acc n))))
(sum-down 200000 0)   ;; => 6251217600, want 20000100000
(sum-down 400000 0)   ;; => type error: -: expected number, got nil   … at dec
```

The wrong value varied run to run (7004720064, 6251217600, 4138068672 …) because it depends on
where the background compile lands. `BROOD_TIER=1`, `BROOD_NO_JIT=1`, `BROOD_TIER=0`,
`BROOD_NO_LEAF_INLINE=1` and `BROOD_NO_DEOPT_RESUME=1` were all correct; `BROOD_NO_INLINE=1`
(the *self*-inliner) was **not**, which is what isolated it to the leaf-callee inliner.

**`std` was already corrupted by it.** `repeat-acc` (`std/prelude/seq.blsp`) is exactly this shape
— `(repeat-acc (dec n) x (cons x acc))` — so `(count (repeat 200000 :a))` returned **28033** and
`(string/length (apply str (repeat 200000 "x")))` returned **0**.

**Mechanism.** A tiering arm has two frame layouts: the small body's (`nslots`) and, once the
deferred upgrade installs, a leaf-spliced derivation's (`inline_nslots`). On a deopt or preempt
the runtime must know which one the live frame was built to, because each keeps its deopt journal
at its own slot. It decides **by frame size** — deliberately, because the `inline_installed` flag
is flipped by `jit_tier` between the sizing and the deopt (that is KI-26, and reading the flag
there caused an out-of-bounds `root_at`).

Two things then combined:

1. `leaf_nslots = d.nslots.max(nslots_total)` came out **exactly equal** to the small `nslots`
   whenever the spliced callee needed no slot beyond the caller's own. `(dec n)` is that shape —
   measured `nslots=4, inline_nslots=4`. So the size test could not tell the layouts apart, even
   though every comment around it asserted a leaf layout is *strictly* larger.
2. Splicing **removes the residual `Call`**, which makes the derivation `pure_self` and therefore
   *unjournalled* (`ckpt_slot == u32::MAX`), while the small body — which still has the call —
   journals at a real slot. The old predicate asked "is this frame leaf-spliced **and**
   journalled?", answered *no* for that pair, and fell through to the **small** layout's slot.

In the spliced layout that slot is an ordinary local. It held the live loop counter, so
`jit_ckpt_resume` read `Int(165825)`, saw a positive integer, and decoded it as a journal word:
resume ip `n >> 16` = 2, operand depth `n & 0xFFFF` = 34689. The loop resumed at a garbage
instruction with thousands of phantom operands.

**Fix.** Two halves, both in the "make the invariant true rather than work around it" direction:

- `leaf_nslots = d.nslots.max(nslots_total + 1)` (`eval/compile/mod.rs`) — one reserved slot, so a
  leaf layout is **strictly** larger than the small one and the size test means what the
  surrounding comments already claimed.
- `jit_frame_is_leaf_spliced`/`jit_ckpt_slot` replaced by a `FrameLayout` enum +
  `jit_frame_layout(arm, frame_nslots)` (`eval/compile/jit_runtime.rs`), so a frame reads the
  journal slot of the layout it was **built to** and never the other one's. An unjournalled layout
  now yields `None` (resume from ip 0, effect-free by construction for exactly the arms that
  decline to journal) instead of falling back to a foreign slot.

**Why nothing caught it.** See the note under the index: the corruption needs one *long*
activation, and no `std` suite tests a large input. It is invisible below ~10⁵ iterations and a
loop that calls the same function eleven times at increasing sizes stays correct throughout — so
repetition, the usual flake defence, does not help. The missing dimension is a **size sweep**
(same closed-form answer at 10³/10⁵/10⁶, across `BROOD_TIER` 0/1/2), which the new guard does.

---

## KI-67 — `nest check` could not see inside a `try` body ✅ FIXED 2026-08-27

`nest check` does not report an unbound symbol used inside a `try`. The skip is deliberate
and has a test guarding it — `skips_error_testing_forms` — because a test that deliberately
calls a missing name should not be flagged. The cost is that it blinds the checker to
exactly the shape a rename wave produces.

**How it presented.** hatch's spooled-upload write is:

```lisp
(try
  (bytes/append path piece)
  (catch e (do (spool-cleanup path) (error (str "spool write failed: " (error-message e))))))
```

brood renamed `bytes/append` to `file/spit-bytes-append` (it writes a FILE). `nest check`
reported nothing. The repo was green, and every spooled upload was broken. It surfaced as
four tests timing out — `:timeout`, `file/slurp-bytes: … No such file` — with no error
naming the cause, because the `catch` swallowed the unbound-symbol error and re-raised it
as a generic "spool write failed".

**Why it is worth fixing rather than documenting.** A `try` is where I/O lives, and I/O is
where the renamed primitives live (`file/*`, `os/*`, `bytes/*`). So the blind spot lines up
precisely with the code most likely to be moved by a wave.

**Candidates**, cheapest first:

- Flag an unbound symbol in a `try` **whose `catch` does not itself mention it** — that
  preserves `skips_error_testing_forms` (those tests catch the very name they call) while
  covering the rename case, which never does.
- A `nest check --strict` that reports them in a separate section, so the default stays
  quiet and a rename wave has one command to run.
- Have the checker treat an unbound symbol as an error but a *deliberate* one as suppressed
  by the existing `(check-allow :unbound …)` directive, which already exists for this shape.

Found twice in one session: this, and brood-terminal's `run-process` earlier.

---


**Fixed.** `try` / `%try` / `error-of` / `assert-error` moved off `SpecialHead::SkipBody`
onto a new `ErrorTesting` arm that **descends** into the body and keeps **only** the
unbound-symbol diagnostic. Everything else — arity, type misuse, exhaustiveness, no-method
— is dropped, because deliberately exercising a failure is what these forms are *for*.

The filtering happens **at the collection point**, not lint-by-lint: the arm walks into a
scratch `Vec` and retains messages starting with `UNBOUND_PREFIX`. That was the second
attempt. The first gated each lint on a `SUPPRESS_*` bit and immediately proved too coarse —
exhaustiveness and `no num-mul method` have no bit, so `nest check` went from 0 to 6
warnings, four of them false. Filtering at the collection point means a lint added later is
suppressed here **by default**, which is the correct default for these forms.

A test that really does assert on an unbound name opts out with `(check-allow :unbound …)`.

**It found two real dead call sites on its first run:**

| where | stale name | swallowed by |
|---|---|---|
| `tests/http_test.blsp:63-64` | `bytes-concat` (it is `bytes/concat`) | `assert-error` — the test passed on the *unbound* error, never exercising the CRLF-injection refusal it claimed to test |
| `std/tool/mcp.blsp:78-83` | four `project-*` names that are `defn-` (module-private) | `(catch _ nil)` — so `mcp`'s per-file shadow report always returned nil |

The second is the more instructive one: it is shipped `std/`, not a test, and the fix was to
add a public `project/file-shadow-warnings` rather than let `mcp` reach for four private
names. Guarded by `unbound_inside_an_error_testing_form_is_still_flagged` and
`only_unbound_survives_an_error_testing_form` in `types/check/tests.rs` — sabotage-verified
(restore `SkipBody` for `try` and the first fails).

## KI-66 — nothing verified that a project still boots ✅ CLOSED 2026-08-27

`nest check` resolves names; `nest test` runs the suite. Neither loads `main`. Module-load
is precisely where a project with a stale dependency dies, so both gates can be green while
the app cannot start.

hive went down twice on the same shape:

| | error | raised at |
|---|---|---|
| v130 | `unbound symbol: int->char` | `hatch/web/live.blsp` during `require`, before `main` |
| v134 | `unbound symbol: os/getenv` | `web/application/default-logger-opts`, first line of `main` |

Both times `nest check` was clean and the suite was green, because the failing module was a
pinned dependency that the local working tree did not match, and because a suite never
executes the entry point. The Rust runtime built fine — it is the BUNDLE that could not
start, which is a different artifact from anything the gates look at.

**The tool already exists.** `nest run --for <DURATION>` — documented for exercising a TUI
or animation loop in CI — is exactly a boot check, and was verified on a fixture:

| entry point | `nest run --for 1s` |
|---|---|
| `(defn main () (this-name-does-not-exist))` | **exit 1**, with `unbound symbol: …` |
| `(defn main () (io/puts "booted") (sleep 60000))` | **exit 0**, having printed `booted` |

So this is not a tooling gap, it is a wiring one: nothing suggests running it and no gate
does. Note what it catches that a module-load check would NOT — the second outage raised
inside `default-logger-opts`, called from `main`'s body, so loading the module was never
going to be enough. You have to actually run the thing.

**Wired into hive's `bin/ci`.** The open question is the shared package-ci workflow. It
cannot simply be enabled everywhere: `bedit` and `pong` need a GUI, and a library has no
`:main` at all. It wants an opt-in input, the way `postgres: true` is.

**Closed.** That opt-in landed in `2b51de93`: `package-ci.yml` takes a `boot-check` boolean
(default **false**) plus a `boot-seconds` string (default `6s`), and runs `nest run --for`
before the suite when set. Default-off is deliberate and not laziness — a library has no
`:main` to run, and `bedit`/`pong` open a window a runner has not got. hive's `bin/ci` runs
it unconditionally, which is where the two outages happened.

**Related, and cheap alongside it:** a bundled app cannot say what it is. Answering "which
brood built this, with which features" required `grep -ac cranelift` on the binary over SSH,
and the first attempt used `strings`, which is absent from `debian:bookworm-slim` and so
reported 0 for *command not found* — indistinguishable from "no JIT". A `--build-info` on
the bundle (brood commit, features, module count) removes that guesswork entirely.

**Both follow-throughs landed 2026-08-27 ([ADR-257](decisions.md)).**

- **`--build-info` shipped** as `myapp --brood-build-info` — brood version, build-id,
  features, app name/version, module count. The `--brood-` argv prefix is **reserved by
  the runtime** (two names, first position only) so the bundle's "argv belongs to the app"
  contract survives intact, and it reads only the manifest and module directory — loading
  no module, so it still answers on a bundle whose modules are broken, which is when it
  is asked.
- **A first-class boot check shipped too**, `nest run --check-boot` and the one `nest
  release` runs on each binary it writes (opt-in `--smoke` until 2026-08-29; now the
  default, with `--no-smoke` to skip), alongside the `nest run --for` wiring recorded above. The distinction between
  them is the useful part of this entry and is now written down as a table in ADR-257:
  `--check-boot` loads every module and resolves `:main` **without invoking it**, so it
  catches the v130 class (raised during `require`) and *not* the v134 class (reached from
  `main`'s body) — exactly the point this entry made. In exchange it is always safe to
  run: no window, no port, no side effects, and valid for a library with no runnable
  `:main`, which is precisely why `package-ci.yml`'s `boot-check` had to default to false.
  The release check additionally runs the **artifact**, which carries a dependency snapshot
  no source-tree check can see — and deletes it if it does not boot.

---

## KI-64 — a JIT block-argument spill landed on the deopt journal ✅ FIXED 2026-08-26

**Status:** ✅ **fixed** — and the mechanism is *not* the one this entry inferred. It is
neither shared code nor multi-process nor load: it reproduces in **one process, on the
fourth call**, with no `spawn` and no JSON. `BROOD_NO_JIT=1` is no longer needed in hive.

**Root cause.** A JIT'd arm's frame is `[locals | spill slots | checkpoint journal]`, and the
spill area has two independent halves — **call-result** spills (a heap handle left live below
a later call, the `fib` shape) and **block-argument** spills (an operand crossing a block
boundary that is not a profiled `Int`, ADR/KI-49). `jit_spill_reserve` sizes both and opened
with:

```rust
if non_tail_call_count(code) < 2 { return 0; }
```

which is right for the call-result half and wrong for the other one. Block-argument slots are
needed whenever the operand stack is deep where a block boundary falls, and that happens with
a **single** non-tail call as soon as an `if` sits inside that call's argument list:

```lisp
(walk-list (rest xs) (step (first xs) (if first? acc (cons "," acc))) false)
```

Those arms reserved nothing, and the lowering's `blockarg_spill_base` was a *clamped*
subtraction — with a zero reserve it collapsed onto `spill_base`, which for a journalling arm
**is the checkpoint slot**. The native then wrote block arguments over the deopt journal, and
a later block-argument read returned the packed journal word `(resume_ip << 16 | depth)` as
if it were a live value. `17 << 16 | 2` = **1114114** — the number in the error, and the
reason it never varied with the payload. Arms with no journal were worse: they wrote past the
frame top entirely (`register-impl-check-arity` wanted 11 such slots against a 10-slot frame).

Affected arms were not exotic: `fold-loop`, `index-of-seq-from`, `json/emit-list`,
`json/emit-items`, `any?`, and most of the `match-*` predicates.

**The fix** is `jit_plan::blockarg_spill_window` — the window is the *top* `len` slots of the
reserve and `len = max_leader_depth.min(reserve)`, so it can never reach the journal. Where
the reserve exists (the ≥2-non-tail-call shape KI-49 measured) `reserve >= want` and nothing
changes; where it does not, `len` is 0 and a `Handle` crossing a boundary falls through to
`ParamRepr::Int`, which tag-checks and **deopts** rather than corrupting — the pre-KI-49
behaviour, correct but slower. A `spill-area-exceeds-frame` bail in `jit_lower_arm` backstops
it; it is unreachable by construction and stays because *nothing* checked before this.

**Growing the frame instead was measured and rejected.** Reserving `max_leader_depth` for
every lowerable arm is the obvious fix and costs **+2 to +7% on nearly every benchmark row**
(`errors-deep` +7.5, `primes` +6.8, `loop` +6.1, `spawn` +5.3, `collatz` +4.4, `fib` +3.1),
because `max_leader_depth` counts int/bool merges that need no slot at all — `not`, `int?`,
`inc`, `dec` and every small type predicate. The clamped form measures flat: full
`ab-bench --all`, every row inside its own floor (`reduce` +2.8% vs a 5.6% floor, `pingpong`
+0.9% vs 0.4%, `ring` +0.8% vs 1.8%). `supervisor` read +17.2% then −13.5% across two runs
with a 12–17% base-vs-base floor, i.e. **not resolvable** on that row — no claim either way.

**Guarded by** `tests/jit_blockarg_spill_test.blsp` (the behavioural end-to-end case, which
fails only if the clamp *and* the backstop are both removed — verified by sabotaging both) and
two invariant tests in `jit_plan` that fail on the clamp alone, naming the arms.

**What this entry got wrong, and why.** Every inference in the original diagnosis pointed at
shared compiled code, and each was reasonable and wrong:

- *"the multi-PROCESS dimension looks load-bearing"* — it is not. `spawn` merely got the arm
  hot faster. The single-process repro fails on call 4.
- *`BROOD_NO_SHARED_ARMS=1` made it clean* — a timing accident, and the most expensive false
  signal here. With the reproduction narrowed to one process the same flag **still fails**.
  A flag that fixes a bug is evidence about *scheduling*, not about mechanism, until the
  repro is minimal.
- *"the first ~60 requests succeed, then permanent"* — read as a load threshold; it is just
  the tier-up point, and the sticky part is `BAILED` deopt feedback.

The thing that actually localised it was **`BROOD_NO_DEOPT_RESUME=1`**, the one flag in the
matrix that names the machinery rather than a policy — and then one `eprintln` at the failing
`empty?` showing `raw=[0x2,0x110002,…]`, i.e. a *packed journal word* rather than a plausible
datum. Decode the bad value before theorising about how it travelled.

---

**Original report (superseded above).**

**What it looks like from outside.** hive's package API returns 500 while the site looks
healthy: `/health` answers, every web page renders, the database is fine. A machine restart
fixes it, for a while. This is what "hive hangs silently and recovers on restart" has meant
for months — it was read as flaky infrastructure and it is a compiler bug.

**Measured** (locally, against a real Postgres with the production data shape):

| | with JIT | `BROOD_NO_JIT=1` |
|---|---|---|
| 40 sequential API requests | 8 ok, 32 failed | — |
| 40 concurrent API requests | 0 ok, 40 failed | — |
| 120 sequential API requests | — | **120 ok, 0 failed** |
| 40 sequential WEB requests (same rows, same `registry/search`) | 40 ok, 0 failed | — |

The first ~60 requests succeed; after that it is permanent until restart. That shape — fine
while interpreted, broken once hot, sticky thereafter — is the tiering signature.

**The error**, from instrumenting the handler (the server logs nothing on a 500, which is
its own gap):

    empty?: expected collection, got int (1114114)

`emit-pairs` and `emit-list` in `std/json.blsp` recurse on a list and call `empty?` on it.
An `int` arriving there means the argument slot held a non-list. **1114114 is 0x110002 —
one past the Unicode maximum (0x10FFFF)**, which points at a codepoint counter leaking into
the slot rather than an arbitrary garbage value.

**Ruled out by measurement, not argument:**

- *the connection pool* — Postgres reports 5 idle connections throughout; the pool is size 5
  and healthy while the API is failing;
- *the database, the query, `ilike`* — the WEB path reads the same rows through the same
  `registry/search` and is 40/40 clean while the API is failing;
- *`json/encode` itself* — `/health` encodes JSON on every request and never fails;
- *hive's own code* — the only difference on the failing path is that it JSON-encodes the
  package rows.

**Not yet reproduced standalone.** 20 000 `json/encode` calls of the same payload in ONE
process (including the multi-byte em dash two package descriptions carry) stay identical.
hive reproduces it in about sixty requests, so the multi-PROCESS dimension looks load-bearing
— each request runs in its own green process against shared compiled code (ADR-175/215).
That is where to look next: a shared arm compiled in one process and adopted by another.

**Next step.** Run hive's API under `BROOD_JIT_DUMP_IR=1` and `BROOD_DEOPT_TRACE=1` to name
the arm, then `BROOD_NO_SHARED_ARMS=1` to test the shared-code hypothesis directly — if that
makes it clean, the fault is in adoption rather than in lowering.

## KI-63 — loading modules taxes JIT'd hot loops ☑️ RETRACTED 2026-08-25 (no such effect)

**Status:** ☑️ **RETRACTED — there is no such effect.** Kept in full because the way it was
wrong is worth more than the claim was: three successive measurement methods each produced a
confident number, and each was an artifact.

**The refutation, first.** Run the loop once and discard it, then time a second run in the same
process:

| | 20M loop, after a discarded warm-up |
|---|---|
| no module loaded | min 23, med 24, max 25 ms |
| `format` loaded first | min 23, med 24, max 26 ms |

Identical. Every figure below measured the **first** run of the loop — i.e. JIT tiering — not
steady-state execution. Running the loop three times in one process shows it plainly: with
`format`, 50 / 24 / 24 ms; without `format`, 51 / 24 / 24 ms. The first run is slow either way.

**And first-run timing is not even stable against program shape.** The identical loop measured a
median of 25 ms as the only statement in a file, and 40-51 ms as the first of three call sites in
another — same code, same binary. That sensitivity is what produced the "`format` costs +92%"
result: `format` was never the variable, the file's shape was.

**What is actually true, and worth keeping:** a whole-process benchmark of a short row measures
tiering as much as it measures the code. The harness does one discarded warm-up run *per
language*, which warms the boot cache — but every row runs in a fresh process, so the program's
own functions tier from cold on every measured run. For rows in the tens of milliseconds that is
a large and variable share of the number.

Everything below is the retracted investigation, kept for the traps it documents.



**What.** The same allocation-free 20M-iteration loop, natively compiled, runs measurably slower
if std modules were loaded first, and the penalty roughly **doubled** since 0.3.11. Timed
**in-process** — `os/now-ns` either side of the loop — so module-load time is outside the
measurement entirely. Unpinned, 25 runs:

| | 0.3.11 | 0.12.0 |
|---|---|---|
| 0 modules | min 23, med 23, p90 23 | min 25, med 26, p90 29 |
| 4 modules (`json format datetime csv`) | min 22, med 26, p90 30 | min 28, med 33, p90 50 |
| **tax** | **−4.3% min / +13.0% med** | **+12.0% min / +26.9% med** |

**Read the direction, not a single number.** Three samples of the same comparison gave a 0.12.0
tax of +27.9%, +25.0% and +12.0% on min, against a 0.3.11 tax of −5.8%, +0.0% and −4.3%. The
*ratio* is consistently about 2x and the sign is consistent; the magnitude is not stable enough
to quote one figure, and the p90 gap (30 vs 50 ms) says the distribution is what moved most.

It is **JIT-only**: under `BROOD_NO_JIT=1` or `BROOD_TIER=1` the same
comparison reads 942 → 939 ms and 949 → 949 ms, i.e. 0.0%. The interpreter does not care; native
code does.

This is on the common path, not a corner: after the namespacing waves *every* program loads
modules — `io/puts` alone pulls `io`.

**Method, because the naive versions are both wrong.**

1. **Time in-process; differencing two programs is not reliable enough here.** Loop time is
   `os/now-ns` either side of the loop. The obvious alternative — `wall(file with the loop) −
   wall(the identical file without it)` — cancels module-load cost in principle, and it is what
   this entry first used, but it fails once the non-loop part is large and variable: with 2000
   `defn`s in both files it reported the loop taking **4 ms**, and it manufactured a "+64% at
   2000 functions" threshold that in-process timing shows does not exist (flat 24–26 ms from 0
   to 4000 extra functions). Subtracting the harness's `startup` row is worse again: that row is
   `(io/puts 0)`, which loads `io` but not `os`/`string`, so it under-subtracts for every real
   row.
2. **Do not pin.** Pinned to one core the same measurement reads **+68.2%** rather than +27.9%,
   because the background JIT compiler competes for that core and loading modules increases
   compilation volume — precisely the trap CLAUDE.md documents for `make ab`. The 0.3.11 side
   inflates too (+27.1% pinned vs −5.8% unpinned), so a pinned reading exaggerates *both* sides
   and the regression as well.

**What it is not.** Each of these leaves the tax in place, so none is the cause:
`BROOD_NO_SHARED_ARMS=1` (75.5%), `BROOD_RT_GC_FLOOR=99999999` (73.4%), `BROOD_NO_INLINE=1`
(87.4%), `BROOD_NO_LEAF_INLINE=1` (78.4%) — pinned figures, compared against 87.1% pinned
default.

**Reproducer.** Two files per point, `N` modules `require-one`'d at the top, then
`(defn- go (i acc) (if (>= i 20000000) acc (go (+ i 1) (+ acc i)))) (go 0 0)`; the paired file
has the same requires and no loop. Difference the walls.

**Why it matters more than 28%.** It compounds with KI-61: the namespacing both forces more
modules to be loaded *and* makes each loaded module tax the code that runs afterwards.

---

## KI-62 — the stdlib image was unusable on the shipped build ✅ FIXED 2026-08-25

**Status:** ✅ fixed. Guard: `stdlib image: fidelity › a replayed edge naming a module this
binary lacks does not break require`, sabotage-verified.

**What.** Installing the stdlib image made the *next* `require` fail:

```
error: require: cannot find module 'test'
```

The image is keyed on `stdlib-id`, a hash of the stdlib's **content**, and that is deliberate:
`brood`, `nest` and `brood-lsp` built from one tree report the same id so they share one ~2 MB
image instead of writing three. But sharing a key across binaries assumes they can load the same
things, and they cannot — a **lean** runtime (what `nest release` and
`make install INSTALL_FEATURES=RUN_FEATURES` produce, i.e. what actually ships) bakes in 88
modules with **no dev-tools**, and `std/tool/project.blsp`'s recorded require-edges name `test`:

```
project -> (sexp coverage test hash reflect dev package version io os path file seq string)
```

`stdimage/install` replays those edges — correctly, and for a good reason: a restored module
defines its bindings and evaluates nothing, so without the edges `url` comes back with no
`path`. But replaying an edge to a module this binary has no source for poisons `require`
outright.

**So the image's headline number has never been available on the build users run.** Measured on
release, once the edge is filtered:

| | from source | from image | |
|---|---|---|---|
| `require format` | 62.0 ms | **12.8 ms** | 4.8x |
| `require datetime` | 3.3 ms | **0.39 ms** | ~9x |

**Fixed at install, not at build**, and the distinction is load-bearing: the image may have been
written by a different binary from the one reading it — that is the entire point of sharing the
key — so the *reader* is the only party that knows which modules it can load. Filtering at build
time would produce an image correct for its writer and wrong for its readers.

**Why it went unnoticed.** The image is built by `make install` and by `nest stdimage`, but it is
not installed at boot (see KI-61), so nothing in the normal path ever replays those edges. It is
dead code until someone calls `stdimage/install` — which is exactly what a boot-install
experiment does, and what this was found by.

---

## KI-61 — startup is a per-wave namespacing tax ✅ FIXED 2026-08-26

**Status:** ✅ **fixed** — and not the way this entry predicted. The recorded fix was "make a
module load cheap at boot" (the std image + a registration replay). The actual fix was to **not
load the modules at boot at all**: the prelude's references into `string`/`seq` are now
**autoload stubs** that load their module on first call (ADR-246), so boot loads nothing and a
future namespacing wave costs one declaration instead of a few more milliseconds on every
invocation, forever. A second, unrecorded half went with it — the warm boot also read the
prelude a *second* time, positioned, purely to recover def-sites for `M-.`; those now travel in
the boot cache (ADR-247).

Measured, warm, `taskset`-pinned, best-of-9 through `scripts/ab-bench.sh` against `7bf57f52`:

| | before | after |
|---|---|---|
| prelude boot (`BROOD_BOOT_TRACE`) | 22.8 ms | **11.6 ms** |
| — of which the two `require-one`s | 12.1 ms | 0 |
| — of which the raw positioned read | 3.5 ms | 0 |
| `startup` row | 45 ms | **32 ms** (−28.9%) |
| base RSS, bare program | 55.6 MB | **50.7 MB** |
| bare `brood empty.blsp` | ~26 ms | **~15 ms** |

Every other row moved by the same absolute ~11–13 ms (`fib` −10.5%, `loop` −10.2%, `reduce`
−22.2%, `strings` −34.7%), which is the point: this was a cost on every invocation, not on a
benchmark. The std image remains the right way to make the *lazy* load fast when it happens, and
the registration-replay design recorded below is still the work that would let it be installed at
boot — the two compose. What follows is the original diagnosis, kept because the measurement
traps in it are the reason the fix landed on the second try.

---

**Original diagnosis (2026-08-25).** Every figure below is best-of-15, `taskset`-pinned, on an
empty program, with **each binary warmed first** — the boot cache is keyed on build-id *and the
executable's mtime*, so a freshly copied binary's first run measures cache population (~1.2 s
cold against ~0.11 s warm) and would have made the whole sweep meaningless.

**What.** The first cross-language run on 0.12.0 read `startup` at **29.9 ms** against 0.3.11's
16.5. A sweep of 8 points across the 244 commits shows two clean **steps**, not a ramp — which
is what makes this bisectable at all, unlike the `primes` hunt FRONTIER records:

```
d5572d61  13.9      07538dea  13.5      3d734516  17.5
4fb903ce  13.5      cf97b1d4  17.6 <-   0f326ead  24.8 <-
                    7505e44a  17.5      c39ca87c  24.8
```

Both steps are the same mechanism, and it is not a bug in either commit:

| step | commit | cause | cost |
|---|---|---|---|
| 1 | `1f613d23` "move the string surface into a `string` library module" | `(require-one 'string)` | **+4.0 ms** |
| 2 | the v0.10.0/v0.11.0 waves | `(require-one 'seq)` | **+7.5 ms** |

Step 2 was proven by deleting the single line and rebuilding: **24.3 -> 16.8 ms, -30.8%**.

**Corroborated by the published harness, 2026-08-26.** The first full cross-language run on
0.13.0 reads `startup` at **31.2 ms** (5th of eight, behind .NET) — consistent with the sweep and
still climbing. It also names a **second symptom this entry had not recorded: base RSS is 56.1 MB**
(6th of eight, heavier than Node's 42.9), against ~19-22 MB at 0.3.11. Same cause: the force-loaded
modules are materialised into every boot. Both figures are warm — the boot cache is populated, so
this is not the cold/warm bimodality `../brood-benchmarks/BENCHMARKS.md` warns about. It also
deflates every other published row, since `compute = wall - startup`.

**Why the load is there.** A prelude helper referencing `seq/find` or `string/char-at` is
late-bound — a qualified name in a function *body* resolves when the function is called — but
boot's namespace-resolve is a no-op for the root prelude, so those refs never auto-require. The
module therefore has to be force-loaded before anything can call such a helper.

**Why this matters more than 11 ms.** It is a **per-wave tax**. The core has gone 613 -> ~280
published names across these refactors, each tranche leaving another module the prelude must
force-load, and each costs a few ms of *every* `brood` invocation forever. It also deflates
every other published benchmark row: `compute = wall - startup`, so a bigger startup makes each
row's reported compute *smaller* than its wall-clock regression.

**A suspect that measurement cleared.** `7bbf979d feat(stdimage): a startup image for the
standard library` sits inside step 2's range and is **flat against its parent**. A startup
regression in a window containing a commit named "startup image" is exactly the coincidence one
would bank without checking.

**The fix is not to revert a wave.** It is to make a module load cheap at boot — which is
precisely what the std image does (4-33x) and precisely why it is not installed at boot:
materialising a module defines its bindings and evaluates nothing, so every registration its
load would have performed is skipped, and the suite fails 131 of 4873 with errors that
type-check perfectly clean.

**The recorded blocker is narrower than it reads, and there is a precedent in the same file.**
The note rules out *snapshotting* `*impls*` — correct, since a closure nested in a snapshotted
value does not round-trip. But `stdimage/install` **already replays** `*require-edges*` through
a `%std-edges` section, for exactly this reason ("a restored module defines its bindings but
evaluates nothing, so without them `url` comes back with no `path`"). The extension is to record
the registration **forms** — `(impl Port :fn (write [f s] (f s)))` — rather than the resulting
values: a form is symbols and lists all the way down, so it images cleanly, and evaluating it on
materialise rebuilds the closure *and* performs the registration. Bounded: 56 `impl` forms
across 13 files, 24 `defability`, 35 `defrecord`; `*record-ids*` (plain symbols) already
restores correctly.

**The scoping question is now answered with current data, and the recorded premise holds.**
Measured 2026-08-25 by actually installing the image at boot and running the suite:
**150 failures of 4920** (the note's figure was 131 of 4873, taken before the `%std-edges` replay
and the root-global attribution fix — so those changed nothing here). Every failure is
registration-shaped:

```
10  io: ports are Port impls › port? is true for a fn port
 8  io: ports are Port impls › a port record prints as itself
 6  the temporal types are records › every sealed member satisfies …
 6  log: the stock backend is a record › a backend prints as …
 6  http: responses are records with a Response impl
```

**And the prize is measured, not estimated.** Installing the image before the two boot requires
takes a debug-build startup from **286.9 → 180.1 ms (−37%)**. Doing it needs one more thing
besides replay: `stdimage/install` reaches for `os/getenv`, `path/join` and `file/exists?`
through `os`/`path`/`file`, none of which are loaded that early, so it dies with
`unbound symbol: os/getenv`. A prelude-only twin using `%getenv` + `str` + the `file/exists?`
native (all bound at that point) boots fine — that part is done and works; it is only the 150
registration failures that make it unshippable.

So the remaining work is exactly the replay, and it is now sized: record each module's
registration **forms** at image-build time (56 `impl`, 24 `defability`, 35 `defrecord` across
std), store them as data — symbols and lists image cleanly, unlike the closures a value snapshot
would need — and evaluate them when the module materialises. The hook is the image branch of
`require-one` in `std/prelude/tools.blsp`, immediately after the `*require-edges*` fold that
already replays the header's requires for the same reason.

> **Superseded 2026-08-27 — the replay is DONE, and the design sketched below was wrong on its
> central point.** See **ADR-256**. Three things it got wrong, all worth knowing:
>
> - **The "150 of 4920" figure, and the later "170 of 4888", are both artifacts.** The warning
>   two paragraphs down was right and applies to those numbers too: installing the image from a
>   PROGRAM cannot work, because the framework and its dependency tree have already loaded the
>   library from source. Instrumented, that configuration reports **99 sections installed and
>   materialises zero modules**. Installed at BOOT — from the prelude, the only place early
>   enough — the honest figure is **157 of 4917**.
> - **Registrations did NOT have to travel as forms.** The claim below that a value snapshot
>   cannot carry them, because "a closure nested in a snapshotted value does not round-trip", is
>   measured false: it round-trips and the impl calls correctly. Forms would have needed each
>   defining module's namespace re-established to resolve their bodies' bare names — the hard
>   part of that design, and it was never needed.
> - **The registration gap was not even the largest fault.** 112 of the 157 were a
>   **concurrency race**: the image branch provided the module before following its
>   require-edges, so a racing process saw it as loaded with its dependencies still missing.
>   Every affected file passed when run alone.
>
> Result: **4917/4917 with the image installed at boot**, `json` loads 6.5 → 1.7 ms, `http`
> 12.0 → 3.6 ms, the `json` benchmark row −5.6%. Opt-in as `BROOD_STDIMAGE=1`.

**Do not try to validate this with a post-boot probe — it cannot work, and it will tell you the
bug is fixed.** A qualified name auto-requires *at compile time*, so every module a probe
mentions is loaded from source before its first line executes; the probe then measures a
source-loaded module and reports every `satisfies?` green. Routing through `eval` does not help
(the `eval`'d forms are read at runtime but the enclosing file still names the module), and
neither does installing the image first. Three separate probes here — `queue`, `io`, `datetime`
— all reported "no registrations lost" while the suite under boot-install reported 150 failures.
The only measurement that means anything is the suite with the image installed at boot.

---

## KI-60 — the stdlib lost stderr: `Port` was implemented for `:fn`, but `*err*` is a native ✅ FIXED 2026-08-25

**Status:** ✅ fixed — **independently and concurrently**, here and upstream in
`2b6b1672 fix(io): the ports the language ships with were not ports`, with the identical
one-line impl. The merge brought both in and they had to be de-duplicated; upstream's comment
is the one kept. Found here while merging `origin/main`, which was **red on three `nest`
tests** — verified red at `f390bd56` with none of this branch's work present.

Upstream's note adds the half this entry lacked, and it is the better half: **every test passed
anyway**, because `with-err-str` rebinds `*err*` to a Brood closure — a `:fn` — so the tests
only ever exercised the working case and the *default* was the broken one.

**What.** `io/print` and friends take their destination as a trailing `:to <port>` pair, and
`split-target` only treats it as a destination when `(port? (nth xs (- n 1)))`. `port?` is
`(satisfies? 'Port x)`, and the ability is implemented as `(impl Port :fn (write [f s] (f s)))`
— "a bare 1-arg sink fn is a port".

But `*err*` is not a `:fn`:

```lisp
(def *err* %write-err)     ; std/prelude/control.blsp
(type-of *err*)  ;=> :native      (port? *err*)  ;=> false
(type-of (fn (s) s))  ;=> :fn     (port? (fn (s) s))  ;=> true
```

A Rust primitive's identity is `:native`, so the `:fn` impl never matched it. Every
`(io/print … :to *err*)` in the stdlib — `log`, `std/tool/test`, `supervisor`, `repl`,
`telemetry`, `format` — therefore failed the port test, and the pair fell through as ordinary
values: the text went to **stdout** with a literal ` :to #<native %write-err>` appended.

```
Building mf (1 file) :to #<native %write-err>
src/main.blsp:3:15: warning: unbound symbol: print :to #<native %write-err>
```

`*out*` is `%write-out` and has the identical shape, so an explicit `:to *out*` was equally
broken; it just looked right because the fallback destination is also stdout.

**Why the tests are the ones that caught it.** `declared_sig_is_authoritative_cross_module`
asserts on warnings read from **stderr** — with the diagnostics diverted to stdout it saw none.
That is a much better tripwire than the two that merely used a renamed `print`, and reverting
only this impl confirms it: that test fails, the other two pass.

**Fix.** `(impl Port :native (write [f s] (f s)))` beside the `:fn` one. One line, in Brood.

**The lesson is about ability dispatch, not about ports.** `impl` is keyed on *identity*, and
`:fn` and `:native` are two identities for things that are both "callable with one argument".
Any ability meant to cover "a function" needs both, and nothing warns when it covers only one —
the failure is a silently-declined dispatch, which is the same failure mode ADR-182's
monomorphization notes and the stdimage boot-install note both describe: bindings present,
registration missing, type-checks clean.

---

## KI-59 — a successful `nest run --for` could exit 1 ✅ FIXED 2026-08-25

**Status:** ✅ fixed. Guard: `nest::cli_failure_reporting::run_for_exits_nonzero_when_the_program_dies`,
extended with an instant-exit case that asserts the printed *reason*, not just the code. Read the
caveat below before trusting it as a gate.

**What.** `nest run --for 5s ok.blsp` on a program that prints and exits printed its output, then
`[exit] :noproc`, and exited **1**. The wrapper `nest` generates was:

```lisp
(let (p (%spawn (fn () …))) (monitor p)
  (receive ([:down _ ^p reason] (println "[exit]" reason) (= reason :normal)) …))
```

`%spawn` and `monitor` are two steps. If the program finishes in the window between them,
monitoring an already-dead pid fires a **synthetic `:noproc`** — the monitor never saw the real
exit — so `(= :noproc :normal)` is false and a program that *succeeded* reports failure.

The kernel already names this exact race, one field over: `%spawn-link`'s own docstring says it
"atomically links the child to the caller before it runs (**no spawn->link :noproc race**)"
(ADR-067). The link half was solved; the monitor half kept the two-step form.

**Why not just treat `:noproc` as success.** Because it is ambiguous in the direction that
matters: a program that *crashed* before the monitor attached produces the same `:noproc`, and
the whole reason this wrapper exists is the exit-nonzero contract beside it — a comment in
`main.rs` records that `nest run --for 3s boom.blsp` once printed a crash and reported success.
Mapping `:noproc` to 0 would restore that bug for fast-crashing programs.

**The fix is atomicity, in Brood rather than the kernel.** `%spawn-link` establishes the link
*before the child runs*, so the child's real reason always arrives; `trap-exit` turns it into a
trappable `[:EXIT pid reason]` message instead of killing the driver, which is what `monitor` was
chosen for. No new builtin — the primitives were already there.

**Honest caveat about the guard.** The race is timing-dependent and the test is load-sensitive,
not deterministic: with the fix reverted it fails ~1 run in 6 under 12 spinning cores, and
adding an instant-exit case did **not** measurably raise that. What prevents the bug is the
structural change; the test is a tripwire that will catch a regression eventually, not on the
next run. Do not read a single green run of it as proof.

---

## KI-58 — the namespacing killed the `table-put` call-site inline ✅ FIXED 2026-08-25

**Status:** ✅ fixed. Guard:
`eval::compile::tests::table_put_call_site_inline_recognizes_the_namespaced_wrapper`,
sabotage-verified against the pre-fix resolver.

**What.** `sieve` ran **11.6× slower** than on 0.3.11 — 34 → 394 ms in the cross-language
harness — with the benchmark program changed only from `(table)` to `(table/new)`, i.e. the
same algorithm. The cause is in a comment that had quietly become false:

> `resolve_prim3` — *"Only a **direct** native binding qualifies (its one member,
> `table-put`, has no prelude wrapper to follow)."*

True when written. The v0.9/v0.10 namespacing waves moved the table API into `std/table.blsp`,
so the head is now `table/put`, a closure whose whole body is `(%table-put t k v)`. The
resolver stopped matching, `(table/put …)` compiled to an ordinary `Call` inside the hot arm,
and the JIT-lowered `PrimOp3::TablePut` — which `sieve` is a benchmark *of* — was never
emitted. Nothing errored. No test failed.

**What hid it.** The **2-ary** `resolve_prim` already follows its wrapper through
`passthrough_arm`, so `table/has?` in the very same loop went on inlining as `Prim2`. One
`BROOD_JIT_DUMP_IR` dump of `mark` shows both states side by side:

```
before:  … GlobalIc Local Const Call  Pop Local Prim2SlotSlot SelfCall
after:   … GlobalIc Local Const Prim3 Pop Local Prim2SlotSlot SelfCall
```

The arm lowered to native in both cases, which is why no bail trace and no JIT-vs-no-JIT
ratio flagged it — the loop was native, it just called out per element.

**The fix** mirrors the 2-ary wrapper-following into `resolve_prim3`. Two properties are
inherited rather than added: `head` stays the *original* call head, so any deopt dispatches
the real wrapper with bit-identical errors, and the existing epoch `guard` re-validates on
every `global_epoch` change, so rebinding `table/put` or `%table-put` cleanly drops the
inline. The identity argument map is **required, not applied** — `Node::Prim3` carries no
permutation (unlike `Node::Prim2`'s `map`), so a wrapper that reorders its parameters must
decline; inlining one would store the value under the wrong key. That case is the second half
of the guard.

`make ab`, best-of-7 against the parent commit: **457 → 68 ms, −85.1%**, with all 29 other
rows noise. (The sweep flagged `spawn-live` +5.4%; it does not survive — that row executes no
table code, and a base-vs-base control reads a 7.5% floor with the new binary landing
*between* two base samples. Reversing the arm order alone moved the verdict from +8.9% to
+4.5%.)

**A second instance, found immediately by the gate this prompted.** Writing
`every_inlinable_head_still_reaches_its_primitive` — one table asserting that each spelling a
program actually writes (`+`, `nth`, `first`, `table/get`, …) still reaches its primitive —
failed on its first run: **`table/get` did not resolve either.** Different cause, same class.
Its wrapper is `(defn get (t k &optional default) (%table-get t k default))`, and an
`&optional` head is not a thin wrapper — it binds a default before forwarding, so a 2-arg
`(table/get t k)` had no passthrough to follow. `table/has?` beside it, a plain 2-ary forward,
never lost its inline, which is exactly what kept the gap invisible.

Fixed **in Brood, not the compiler**: `get` becomes two arity clauses, so the 2-arg arm is a
pure forward. `%table-get` is `Arity::range(2, 3)` documented as "default (nil if omitted)", so
the 2-arg call is exactly equivalent and the existing resolver follows it unchanged.

**The class matters more than the instance.** This is the third time a rename has silently
retired an inline — KI-44 was the same shape on `sqrt`, and its fix note names the same
requirement ("a **bare** head resolving to a **PRELUDE** closure"). A call-site inline keyed
on how a name is *spelled* or where it is *bound* is a performance cliff that no test, no
checker and no CI job can see, because the program stays correct. The structural,
wrapper-following resolutions (`sqrt`, the 2-ary prims, and now this) are the shape that
survives a rename; a direct-binding check is the shape that does not.

---

## KI-57 — a stale `tags` handle in the selective-receive scan ✅ FIXED 2026-08-25

**Status:** ✅ fixed. Guard: `make gcstress` (new), verified **red on the faithful pre-fix code
and green on the fix**, twice each. Wired into CI's breakage job.

**What.** `scan_mailbox` received the `receive` clauses' leading-keyword vector as a bare
`Value` and decoded it **lazily** — the decode sits *inside* the scan loop, guarded by
`st.queue.len() > 1 && ntags.is_none()`, so it does not run on a one-message mailbox. That
laziness is the bug: on any iteration after the first, the decode runs *after* a matcher
`apply`, which can collect at any eval depth (ADR-061) and relocate the LOCAL vector. The
handle was captured before that collection and dereferenced after it.

```
use-after-GC: vector handle (nursery slot 3) is from epoch 12, but that generation is now
epoch 13 — a handle held across a collection without being re-rooted (handle 0xc00000003)
  Heap::vector <- mailbox::collect_receive_tags <- scan_mailbox <- receive_match
```

The galling part is that the fix was already written down eight lines above, for the value
next to it: `matcher` is pushed to the roots stack at `rbase+0` and **re-read each
candidate**, with a comment explaining that `apply` can collect and relocate it. `tags` needed
identical treatment and was passed unrooted instead. It now sits at `rbase+1` and is re-read
at the decode.

**Blast radius.** Any selective receive whose mailbox has more than one message — i.e. the
ordinary backlogged case, on a user-reachable path with no unsafe code in sight. Under
debug-assertions it aborts the worker at the bad deref (what we saw). In release there is no
tripwire: the decode reads whatever now occupies the old slot, and the tag filter then
silently rejects a message it should have matched — a receive that misses a message it was
waiting for, which `BROOD_NO_RECV_MARK`'s comment calls the hardest failure mode to attribute.

**Why every green run missed it, and the real finding.** Collections are threshold-driven, so
a handle held across one is only caught when a collection happens to land inside the window.
Nothing in CI changes that: the breakage job arms the per-deref tripwire but still collects on
a threshold. `BROOD_GC_STRESS=1` collects at *every* safepoint, which turns the window into a
certainty — and it had never been run in CI at all, only by hand during an investigation.

`make gcstress` now runs the twelve process/mailbox-heavy test files under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`, and CI runs it. It is a **debug** build deliberately,
and that was verified rather than assumed: `--release` with `-C debug-assertions=on` (what the
breakage step beside it uses) reports the faithful pre-fix code **clean**, because optimisation
moves where allocations and safepoints land and the collection stops falling inside the window.
A gate that cannot fail on the bug it was written for is not a gate.

**Found while verifying something else.** This came out of the post-ADR-245 GC-stress pass, not
from a failing suite — the full suite was 1061/1061 green with the bug present, on the very
tree that was about to be pushed.

---

## KI-56 — the L1 under-lock copy: the hazard was real, but not where it was claimed ✅ FIXED 2026-08-25

**Status:** ✅ fixed (ADR-245), **both sites**. Guards: seven cases in
`process::message::copy_budget_tests` (the boundary both sides, every container kind declining
attributably, the string/blob coupling, and the env override's three rules), six in
`message_fits_tests`, and five in `tests/receive_under_lock_test.blsp` — the last
sabotage-verified: dropping the re-inserted candidate instead of re-queueing it fails four of
the five. Plus a `const _` build assertion tying the budget to `SHARED_BLOB_THRESHOLD`.

This entry is kept mostly for the **measurement**, because a plausible-sounding review finding
was half wrong and only a measurement could say which half.

**The claim.** `try_deliver_local` (`process/mailbox.rs`) performs the whole cross-heap deep copy
while holding the receiver's mailbox mutex — "a latency/contention hazard on the hottest lock in
the system". A first investigation then *retracted* it, correctly noting that the obvious fix
(take the waiter out, copy, re-lock) is unsound because `shutdown_runtime_parked` reaps parked
waiters and would skip the process during the window.

**What the measurement found.**

1. **The fan-in framing is wrong, and provably so.** Under fan-in, L1 fires **0.4–1.2 %** of the
   time (measured across 1/2/4/8 senders). The reason is structural, and worth internalising: **a
   contended mailbox has a runnable receiver by definition, and L1 requires a *parked* one.** The
   shape everyone pictures when they say "hottest lock" is precisely the shape where this code
   almost never runs.
2. **L1 is cheaper than the fallback everywhere**, 1.09× (tiny) to 2.58× (100k-element vector),
   with no crossover across a call-count sweep. So "hazard" is the wrong word for throughput.
3. **But there is a real, large latency effect** in synchronous request/reply (where the receiver
   *is* parked — 61–81 % hit rate). An unrelated `%mailbox-size` probe — a pure lock-acquire, zero
   message work, so a stall can only be lock wait — measures:

| payload | L1 p50 | L1 p99 | wire p50 | wire p99 |
|---|---|---|---|---|
| ~8 KB | 4.8 µs | 11 µs | 4.4 µs | 7.5 µs |
| ~80 KB | 4.9 µs | **1 106 µs** | 4.9 µs | 8.7 µs |
| ~1.6 MB | **5 011 µs** | 13 994 µs | 7.3 µs | 15.0 µs |
| ~4 MB | **6 777 µs** | 16 946 µs | 7.6 µs | 17.1 µs |

The wire arm is **flat across a 500× payload range** because its heavy work happens outside the
lock. Effects are 25×–1000× against a ≤3 % noise floor.

**The fix (landed).** A work budget on the L1 copy — one heap node is one unit, default 4096,
`BROOD_L1_BUDGET=0` to uncap — declining past it and falling through to the existing wire path. It
bounds the lock hold **without touching the `st.waiter` invariant at all**, so the soundness
objection that killed the first redesign does not apply. It keeps the win where essentially all
sends live and gives it up only on the large send that is causing the stall.

**A per-node charge alone was not enough, and the measurement is what said so.** The first cut
decremented the budget as the walk visited each node — correct-looking, and it still measured p99
**243 µs** at ~1.6 MB against the wire path's 5.4. The reason is that every container arm
*materialised before descending*: `src.vector(id).to_vec()` copies the whole element array, and
`map_entries` / `set_elems` / `list_to_vec` do the same, so the O(n) cost was paid under the lock
and only *then* declined. Each kind now checks against the budget **before** materialising —
`len()` for a vector, `map_size` for a map (doubled for key+value) and a set, `range_len` for a
range, and a bounded spine walk for a cons list, which has no O(1) length. That is what moves the
tail the rest of the way.

The result, same probe and same harness as the table above (best of 200 trials per row, on one
box, the *same binary* with only `BROOD_L1_BUDGET` changed — no rebuild, so nothing else can
differ):

| payload | uncapped p50 | uncapped p99 | capped p50 | capped p99 | wire p99 |
|---|---|---|---|---|---|
| ~8 KB | 0.76 µs | 3.8 µs | 0.77 µs | 3.5 µs | 3.2 µs |
| ~78 KB | 0.73 µs | 89.7 µs | 0.80 µs | **3.2 µs** | 3.0 µs |
| ~1.6 MB | 2.9 µs | 1 875 µs | 2.5 µs | **5.4 µs** | 7.7 µs |
| ~3.9 MB | 2 360 µs | 13 029 µs | 3.9 µs | **5.7 µs** | 7.7 µs |

The capped arm is indistinguishable from the wire arm at every size, and the ~8 KB row is
unchanged — which is the point: below the budget nothing happens at all.

**Throughput on the fast path: no resolvable cost.** Synchronous request/reply at 4 clients,
best-of-9 per-request: capped reads +2.9 % at a 10-element payload and +9.0 % at 100 elements,
against a **same-config base-vs-base spread of 6.4–9.4 % and 2.8–14.5 %** on the same box. Neither
clears `max(5 %, 2 × floor)`, so this measurement cannot resolve a difference — it is not evidence
of a cost, and it is not evidence of none either. `BROOD_L1_STATS=1` confirms the mechanism fires
exactly where intended: 0 over-budget at a 100-element payload, 48 of 404 sends over-budget at
20 000.

**The second instance — measured, and worse than the first.** The *receive* side already
implements the mitigation the first investigation ruled unsafe on the send side: the
optimistic-pop path pops under the lock, then rebuilds with the mutex released. But its sibling
**peek-in-place** branch called `from_message` **under** the lock — a selective-receive scan that
has skipped the head holds the mutex across a deep rebuild *per candidate*, because a candidate
that may not match has to stay queued.

Measured the same way (probe by name in both arms, so the payload is wire-format either way and
the L1 path cannot flatter one side):

| backlog × payload | peek-in-place p50 | p90 | p99 | popped p50 | p90 | p99 |
|---|---|---|---|---|---|---|
| 4 × 8 000 | 0.5 µs | 112 µs | 950 µs | 0.4 µs | **0.5 µs** | 12.2 µs |
| 4 × 40 000 | 0.7 µs | 952 µs | 2 179 µs | 0.4 µs | **0.9 µs** | 2.4 µs |
| 8 × 8 000 | 0.5 µs | 254 µs | 586 µs | 0.5 µs | **0.7 µs** | 2.0 µs |
| 8 × 40 000 | **1 252 µs** | 2 800 µs | 5 569 µs | 0.8 µs | **1.5 µs** | 11.2 µs |

The fix is the same budget asked of the wire form (`message_fits`), and it needed no new
machinery — a candidate past the budget takes the route the optimistic branch already takes.
What made it clean rather than a trade is that the comment defending peek-in-place ("the scan's
lock count stays ≤ the peek-only scheme's for every backlog length") predates the
**leading-keyword filter**, which rejects a message no clause could match on its tag without
rebuilding it. Pop/re-insert therefore applies only to candidates that could match, never to
backlog length. Scan throughput over small candidates is within its own noise floor
(+0.9 %/+0.4 %/+0.2 % at backlogs of 4/16/64, floor 0.6–1.0 %).

**Method note, since it is reusable:** no source change was needed to A/B this. `send` tries L1
only for a `Value::Pid` target, so `(send pid v)` takes L1 and
`(send {:name :r :node (%node-name)} v)` takes the wire path — same binary, same value, same
parked receiver. `BROOD_L1_STATS=1` confirms the split every run.

---

## KI-55 — a shipped closure could not call a namespaced std name on the receiver ✅ FIXED 2026-08-25

**Status:** ✅ fixed. Guards: `cli::distribution::a_shipped_closure_requires_its_modules_on_the_receiver`
(new) and `source_positions_survive_a_cross_node_send`, whose `(require-one 'reflect)` workaround is
gone. Both sabotage-verified — with the sender-side scan disabled they fail with exactly the
original `unbound symbol: math/sqrt` / `unbound symbol: reflect/form-pos`.

**What.** Auto-require (ADR-227/229) fires when a form is **compiled**. A closure shipped to
another node arrives *already compiled*, so nothing on the receiver triggered the require, and any
namespaced global in its body was unbound there:

```
unbound symbol: reflect/form-pos
```

Before the namespacing these were bare prelude names, bound on every node by construction, so
closure-shipping worked without anyone having to think about it. The refactor silently moved that
guarantee: `seq/`, `dev/`, `reflect/`, `os/`, `table/`, `proc/`, `math/` are all module globals now.

**The fix, in three parts.**

1. **The sender names the modules.** `closure_to_message` walks the arms' body and
   optional-default forms — the pass it is already deep-copying — collecting every *qualified*
   symbol outside a `quote`/`quasiquote` subtree, then resolves each **distinct** one through
   `derive::module_to_require`. A candidate is kept only when it is **bound as a global on the
   sender**, which is what separates a real reference (auto-require loaded its module when the
   body was compiled *here*) from a qualified-looking symbol sitting in the body — and is why an
   unloadable module on the receiver is a genuine error rather than a false alarm. The result is a
   `(module, probe)` pair per module in `ClosureMsg::modules`. Skipped entirely when the
   destination is **this** runtime (its processes share our globals) and for the startup image.
2. **It rides the wire.** Appended to the `M_CLOSURE` record (`dist/wire.rs`); protocol magic
   bumped `BRD\x05` → `BRD\x06`, because a v5 peer would read the new count as the start of
   `captured` — a silent mis-decode of every closure sent, which is exactly what a version byte is
   for.
3. **The receiver loads it at the closure's own call site.** `closure_from_message` filters the
   list against `env_get` (one allocation-free lookup — a runtime that already has the module pays
   only that) and, for each module it lacks, weaves a guard into the rebuilt body:

   ```lisp
   (do (if (bound? 'math/sqrt)
         nil
         (%try (fn () (require-one 'math))
               (fn (e) (throw (str "this closure was shipped from another runtime and needs
                                    module `math` …: " (if (map? e) (get e :message) e))))))
       <original first body form>)
   ```

   Built from primitives and core special forms only, because a rebuilt body is **never
   macroexpanded** on the receiver — `unless`/`try` would survive to the compiler as unexpanded
   calls. Optional defaults are wrapped the same way: they are evaluated at frame setup, before
   the body.

**Why the load is not run at deserialize time**, which is the obvious place and is wrong twice
over:

- `from_message` rebuilds a value graph whose half-built lists/vectors/maps sit in **unrooted Rust
  locals**, and a module load evaluates arbitrary top-level code, which collects. That is the
  KI-51 shape.
- Worse, the selective-`receive` scan calls `from_message` **while holding the mailbox lock** (the
  peek-in-place branch of `mailbox::scan`), and `require-one` **sleeps** while another process
  finishes an in-flight load of the same feature — a `sleep` is a `receive`, so that is a deadlock,
  reachable whenever two processes on a node get shipped closures needing the same module.

Woven into the body, the load runs in ordinary evaluation context, on the call that needs it, and
its failure is a catchable Brood error naming the module, the reference and the fact that the
closure was shipped — instead of an `unbound symbol` at whatever line touches the name first.

**Cost.** The sender-side scan is the only new work on a hot-ish path, and it does not run for a
same-runtime send at all. Measured on a deliberately reference-dense closure (8 forms, 10
qualified references, 20 000 serialisations through `table/put`, best of 5): **51 ms → 75 ms**,
i.e. +1.2 µs per closure serialisation. Against a cross-node send — the only path that needs it —
that is noise beside the wire encode and the TCP hop. Resolving once per *distinct* symbol rather
than per occurrence is what keeps it there: `module_to_require` interns and consults the import
table, and doing that inside the walk cost 4× as much.

**Related, found the same way:** bare **`require` is not a bound name** — the callable is
`require-one` (`git log -S"defmacro require"` finds no definition in any commit). Several places
in `CLAUDE.md` and the ADR-065 note describe writing `(require 'test)`; that guidance does not
work against this tree.

---

## KI-54 — `main` was red, and a "core, bare" prelude module seized ten generic names ✅ FIXED 2026-08-24

**Status:** ✅ both failures fixed. ⚠️ One judgement call is deliberately left to the owner.

This entry exists because the sentence above the index said **"No open items… `main` is green on
all five CI jobs"**, and it was not true. **Three** tests fail on a pristine `28e8eeb2` checkout,
verified by building HEAD in a clean worktree rather than inferred:

- `brood::basic spawned_process_picks_up_redefinition`
- `namespace_test` — `(is (reserved-package-name? 'gen))`
- `brood::prelude_manifest prelude_const_is_exactly_the_split_files`

**One commit caused all three.** `7cb796f0` made the gen_server framework core and **bare** in the
prelude. Three consequences followed, two of them in opposite directions:

1. **It reserved too much.** A prelude name is un-redefinable (ADR-166), and `std/proc/gen.blsp`
   defines these *bare*: `cast`, `call`, `call-timeout`, `stop`, `code-change`, `spawn-server`,
   `spawn-server-link`, `spawn-server-named`, `gen-clause`, `defprocess`. So `(def call …)` — a
   perfectly ordinary thing to write, and what that test did — is now refused outright.
2. **It reserved too little.** `reserved-package-name?` derives its set from `(builtin-modules)`,
   which lists only `embedded_module!` entries. Moving `gen` into the prelude bundle removed it
   from that list, so the name `gen` silently stopped being reserved and a package could claim it —
   while its siblings `supervisor` and `agent`, still embedded, stayed reserved.
3. **It bundled a file without declaring it.** `std/proc/gen.blsp` was added to `lib.rs`'s
   `concat!` but not to `prelude_manifest.rs`'s hand-maintained `EXTRA_PRELUDE_FILES`, so the
   manifest test — which exists precisely to make "a file joined the prelude" a decision rather
   than a drift — went red and stayed red. It did its job; nobody was reading the result.

**Fixes.** (3) is one line: `gen.blsp` added to `EXTRA_PRELUDE_FILES` with the rationale.
For (2), a `PRELUDE_MODULES` table (`builtins/system.rs`) feeds `builtin_modules`
alongside `CORE_MODULES`/`DEV_MODULES`. It is kept honest by `prelude_modules_are_bundled`, which
checks the source file exists *and* that `lib.rs` still `include_str!`s it into the prelude — so a
module moving out cannot leave an entry reserving a name nothing owns. Note the test deliberately
does **not** probe for a `gen/…` global: these modules define their functions bare, so no such
namespace exists, and the reservation is about the *module name a package could claim*. For (1),
the test's helper was renamed `call` → `ask`; the name was incidental to what it proves.

**Left for the owner.** Whether `call`/`cast`/`stop` should be permanently un-redefinable is a
design call, not a bug — it follows correctly from ADR-166 plus the deliberate "bare" decision.
But it is a real cost that the commit may not have priced: these are among the most likely names
for user code to want, the failure is a hard refusal at `def`, and the only escapes are a local
`let` or a `defmodule`. If that is too aggressive, the options are to qualify them (`gen/call`),
or to exempt a small set from sealing.

**The meta-lesson.** The green-tree rule says a known failure *is* the work. Both of these had
been red since the commit landed, and the file recording known issues asserted the opposite — so
the claim was load-bearing and wrong, which is worse than no claim. Re-verify "green" against a
clean checkout before writing it down.

---

## KI-51 — `macroexpand-1` dereferenced a handle across a module load ✅ FIXED 2026-08-24

**Status:** ✅ fixed. `eval/macros.rs`'s `macroexpand_1` now roots `form` **and** `env` across
`require_qualified_head` and re-derives the tail from the relocated pair.

`macroexpand_1` read `let (head, tail) = heap.pair(p)`, then — for a qualified head into a
not-yet-loaded module — called `require_qualified_head`, which **loads a module**: arbitrary eval,
therefore a collection that relocates the nursery. It then dereferenced the stale `tail`.

The compile pass is safe (it holds a `MacroBlockGuard`), but the **`macroexpand-1` / `macroexpand`
builtins run with MACRO_BLOCK off** — so this was reachable from ordinary Brood, from the MCP
`macroexpand` tool, and from editor/LSP tooling. Debug builds hit the epoch tripwire
(`use-after-GC: pair handle (nursery slot 33) is from epoch 0, but that generation is now epoch 3`);
**release silently walked relocated memory** and reported a garbage list length — a 5-element form
came back as `arity error: expected 1 argument, got 31309`.

The neighbouring `macroexpand` loop already roots `env` and argues in a comment that `cur` "needs
no slot". That argument was correct when written and was invalidated by the ADR-227 auto-require
landing inside `macroexpand_1` — the recurring shape being **a rooting argument that a later call
quietly falsified**.

---

## KI-52 — `msg_roots` was a root set for one collector and not the other ✅ FIXED 2026-08-24

**Status:** ✅ fixed. `msg_roots` is now seeded into `runtime_collect_with`,
`seed_phase1_and_walk`, `runtime_evacuate` and `runtime_live_closure_count`.

`Heap::msg_roots` is the L1 delivered-message slot table. `copy_cross_heap`'s share-fn path
(default on; `BROOD_NO_SHARE_FN` reverts) hands an **already-shared RUNTIME closure to a parked
receiver by handle**, and `try_deliver_local` parks that handle directly in a `msg_roots` slot —
not in any LOCAL slab. A selective `receive` may leave the slot occupied indefinitely.

The LOCAL collector treats `msg_roots` as a root set in all three of its walks. **No RUNTIME-side
walk did**, so the shared code a queued message points at could be compacted or freed underneath
it: a use-after-free of shared code, surfacing either as a panic on a dead handle or — after aging
recycles the slot — as a silent dispatch to an unrelated closure.

The share-fn path's own soundness note says a shared handle retained in the receiver's LOCAL data
is safe because "the drain's Phase 2 walks the whole local heap". That is true for a handle
*embedded in copied data* and false for the **top-level value sitting in the slot table**, which is
the case this path actually produces.

The structural cause is worth keeping: RUNTIME root enumeration is duplicated across four
functions with no single "these are the heap's roots" visitor, which is exactly how a root set
added later (`msg_roots`, ADR-177) got into the LOCAL lists and none of the RUNTIME ones.

---

## KI-53 — a `Ref` was unique per node, not across nodes ✅ MITIGATED 2026-08-24

**Status:** ✅ mitigated (not closed by construction) — `process/monitor.rs`'s `next_ref` now
randomises the top 24 bits per runtime, leaving a 40-bit counter.

A `Pid` crosses the wire node-qualified (`{node, id}`). A `Ref` crosses as a **bare u64**
(`dist/wire.rs`'s `M_REF`) and every node's `NEXT_REF` starts at **0**, so two nodes running the
same code mint ref 1, ref 2, … in lockstep. A ref is what a pinned `receive` matches on and what
identifies a monitor, so a collision can match a peer's message in a caller's pinned receive —
delivering the wrong reply and stranding the real one.

**Why this is a mitigation rather than a fix.** Erlang qualifies the ref itself by node. Here
`Ref` is a bare `u64` inside `Value`, whose layout is pinned for the JIT
(`value_layout_is_stable_for_the_jit`), so widening it would ripple through the JIT ABI. The
random prefix makes an overlap ~6e-8 per node pair and leaves over a trillion counter values (a
million refs a second for twelve days). Node-qualified refs remain the principled fix if `Value`
ever gains room.

---

## KI-46 — the next test sitting under a deadline (a margin audit) ✅ FIXED 2026-08-18

**Not a bug report. A margin.** KI-39 turned out to be a fixed cost under a fixed deadline that
looked random for weeks, so once it was fixed every case's CI margin was audited from the
2026-08-17 logs (both engine jobs) rather than waiting to be surprised by the next one.

The ranking after the KI-39 fix, worst first, against nextest's **120 s** default hard kill
(`slow-timeout = { period = "60s", terminate-after = 2 }`):

| case | CI time | margin | status |
|------|---------|--------|--------|
| `mcp::tests::std_check_tool_returns_structured_diagnostics_or_an_error` | 87 s | 1.38× | ⚠️ this entry |
| `scaffold_quality::…ships_passing_tests` | 89 s | 1.35× | ✅ split per template |
| `scaffold_quality::…scaffolds_check_clean` | 80 s | 1.50× | ✅ split per template |
| `scaffold_quality::…scaffolds_format_clean` | 77 s | 1.56× | ✅ split per template |
| `gc::collects_below_the_outermost_eval` | 80 s | 1.50× | real GC work, single operation |
| `complete::completion_never_fails_…` | was 300 s+ | timed out | ✅ KI-39, now 10.5 s |

The `scaffold_quality` three were one case each looping over five templates — five scaffolds plus
five toolchain runs summed against one deadline. Splitting into one case per (template, gate)
pair costs no coverage at all, takes each case to ~18 s, lets nextest run them in parallel, and
names the failing template in the case name. A macro generates them, and a generated
`GATED_TEMPLATES` const is asserted equal to `SELF_CONTAINED` so the list cannot drift.

**Why the remaining one is left alone.** `std_check_tool_…` invokes the MCP `check` tool, and
`check-project-structured` is **cwd-based** — so it type-checks this entire repository, and the
cost grows with the repo. Every cheap fix is worse than the problem:

- `BROOD_NO_CHECK=1` returns nil before doing any work, so the case would stop exercising the
  implementation it exists to prove is wired up.
- Scaffolding a one-file temp project needs `set_current_dir`, which is process-global. Under
  nextest (a process per case) that is safe, but under plain `cargo test` — a documented way to
  run this suite — it would race every sibling case in the same binary. Trading a slow test for
  a nondeterministic one is the wrong direction.
- Raising its nextest budget is precisely what let KI-39 hide for weeks.

### Done the real way, same day

`check-project-structured` gained an optional `from` root (defaulting to `(cwd)`, so no caller
changes), and `mcp-check-tool` now passes **`*project-root*`** rather than relying on the ambient
cwd. That is not a test accommodation — it is a consistency fix. The MCP server serves exactly
one project: `*project-root*` is what its write sandbox is pinned to (`mcp-project-path`) and what
every other project-scoped tool already reads (`project-all-files *project-root*`). `check` was
the one tool that took its project from wherever the host process happened to be standing, so it
could disagree with the rest of the server.

The case is now scoped to a two-file temp project: **87 s → 2.5 s**, and it asserts more than it
used to, not less. The old version accepted `{:diagnostics [...]}` *or* `{:error …}` and so never
proved the diagnostics path emits anything at all. The new one plants a deliberate unbound-symbol
warning and requires it back with `:file`/`:line` populated, then asserts no diagnostic comes from
outside the temp root.

That pair is deliberate, and it exists because the first version of this test was too weak. When
the "tool ignores `*project-root*` and falls back to cwd" sabotage was run against it, the test
still **passed** — it merely got slow again (2.9 s → 21.7 s), which is exactly the failure mode
this entry is about. Planting a warning that only exists inside the temp project makes that
regression a hard failure ("the planted unbound-symbol warning is missing: []") instead of a
number nobody reads. Both sabotages — a stubbed tool, and the cwd fallback — are verified to fail
the test.

---

## KI-45 — `examples/editor` references `eval-command`, which left the repo in May

**Status:** ✅ **fixed 2026-08-17 — `examples/editor` deleted** (option (a) below). `brood-edit`
is the real editor project, so the in-repo duplicate — which had referenced a module this repo
lacks for two and a half months — was removed rather than kept limping. `make check-examples`
no longer skips a known-red project (`SKIP_PROJECTS` is now empty), and the `layers.md` /
`system.rs` pointers to `examples/editor/src/` were repointed at `brood-edit`. Original write-up
kept below.

`examples/editor/src/brood-mode.blsp` calls `eval-command/eval-last-sexp` for its `C-x C-e`
binding, and `examples/editor/project.blsp` still advertises the example as built on
"std/buffer, std/keymap, std/layers, std/sexp, **std/eval-command**". That module was moved
**out of this repo on 2026-05-31** (`650eb89f`, "move eval-command (editor policy) out of std →
the myedit project") and now lives in the sibling project — `../brood-edit/src/eval-command.blsp`.
So the example has referenced a module this repo does not contain for two and a half months.
`nest test` in `examples/editor` is 4 of 5, failing only that case with `unbound symbol:
eval-command/eval-last-sexp`.

Nothing gates it: `examples/` is outside `make test`, `nest check` and the breakage suite — the
same blind spot as KI-42 (breakage), KI-43 (a suite outside the gate) and KI-44 (the benchmarks
repo). This is the fourth instance of that one pattern.

**Why this is not fixed here.** The three options are each a judgement about what the example is
*for*, which belongs to the owner: (a) delete `examples/editor`, since `brood-edit` is now the
real editor project and this is a stale duplicate; (b) give the example its own small
`eval-command` module, since the 650eb89f rationale — "editor policy belongs to an app, not
std" — applies to an example app just as well; or (c) drop the `C-x C-e` feature and its test and
correct the manifest comment. Option (a) also removes 9 of the ADR-229 migration's edits.

What *was* done: the example's `require` call sites were migrated (ADR-229), which is a strict
improvement — the missing module now fails at its one call site instead of at load, so the other
four tests run. `text-mode` is loaded with `require-one` in both `brood-mode` and the test,
because it is reached only through **quoted** keymap symbols (`'text-mode/forward-char`) and a
quoted symbol is not a reference, so load-by-inference cannot see it — ADR-229's pure
effect-load case.

---

## KI-44 — the `nbody` benchmark was dead, and the `sqrt` JIT call-site inline died with it

**Status:** ✅ **fixed 2026-08-17.** Correctness fixed first (benchmark runs again, checksum
verified); the ~1.8× performance half is **now fixed too** — the `sqrt` call-site inline was
restored for the moved `math/sqrt`, keyed on a structural identity instead of the old
sealed-PRELUDE one. See "the performance fix" below.

**Two defects, one cause: ADR-227 moved `sqrt` out of the prelude into `std/math.blsp`, and
nothing outside the brood repo was migrated.**

**1. The row was broken outright.** `brood-benchmarks/bench/brood/nbody.blsp` calls `sqrt`
bare, twice, on its hot path, and is a header-less script — so since 2026-08-14 it died with
`unbound symbol: sqrt`. A published `bench/harness.py` run would have failed on it. `json.blsp`
was dead too, calling `json/json-parse`/`json/json-encode` after stage 4 dropped the `json-`
export prefix. Both fixed by referencing the module qualified — `math/sqrt`, `json/parse`/`json/encode` —
which loads it by inference (ADR-229 removed the user-facing `require` a day later, and that
removal broke `base64`/`json`/`regex` in the same repo for the same reason; fixed together), and
all verified against the other ports' checksums rather than merely "it runs now".

> ⚠️ **The two checksums this paragraph used to quote (`nbody` −169063618, `json` 364568836)
> are stale and should not be treated as canonical.** Re-audited 2026-08-25: neither is
> reproducible at any `BENCH_N` (an `nbody` sweep from 1k to 500k produced −169087605 …
> −169096567 and never that value). `nbody`'s checksum is **N-dependent** — it is an
> energy sum over a step count — so a figure quoted without its N means nothing. The
> canonical values are the ones in `brood-benchmarks/results/results.json`, and all eight
> ports agree with them. Quote a checksum with its N, or point at `results.json`.

This is the **KI-42 pattern**: a suite that gates nothing rots silently. `brood-benchmarks` is a
separate repo, so the ADR-227 migration sweep — which did cover `breakage/`, `examples/`,
`stress/`, `crates/`, `std/` — could not see it, and no CI job runs the benchmark programs for
*correctness*. Worth fixing structurally: a cheap "every bench row still runs at BENCH_N=50"
check would have caught both in seconds.

**2. The `sqrt` call-site inline is now dead, worth ~1.8× on the row.** `resolve_prim1`
(`eval/compile/mod.rs`) lowers `sqrt` to `PrimOp1::Sqrt` only when the head symbol is **bare
`sqrt`** *and* resolves to a **PRELUDE** closure — a deliberately narrow test, so a user
`(def sqrt …)` cleanly disables it. After the move neither spelling can satisfy it: bare `sqrt`
is not bound at global (a `(:use math)` refers it per-module), and `math/sqrt` fails the name
test and is a RUNTIME module closure. So every `sqrt` now pays a closure call plus the wrapper's
two `cond` comparisons, where it used to be one inlined instruction.

Measured (release, pinned):

| | wall |
|---|---|
| 3M-iteration `%f64-sqrt` loop (what the inline gave) | 406–410 ms |
| same loop through `math/sqrt` | 754–755 ms |
| **`nbody` row, `%f64-sqrt`** | **0.38–0.40 s** |
| **`nbody` row, `math/sqrt` (shipping)** | **0.66–0.74 s** |

≈1.85× on the microbenchmark (~115 ns per call) and **≈1.8× on the published row**. Note the
inner `%f64-sqrt` still inlines *inside* the wrapper's own body — what was lost is skipping the
wrapper at the call site.

**The performance fix (2026-08-17).** The call-site inline was restored for `math/sqrt`, but the
identity is now **structural, not name+region**. `region == RUNTIME` alone cannot distinguish the
canonical wrapper from a user's own `foo/sqrt` (both are RUNTIME closures), and `math/sqrt` itself
turns out to be a **reserved name** — `(def math/sqrt …)` is refused (E0030), so the *global*
`math/sqrt` cannot be hot-reload-rebound at all. So `resolve_prim1` (`eval/compile/mod.rs`) now
recognizes any `…/sqrt` (or bare `sqrt`) head bound to a closure whose single 1-param arm is
*exactly* `(if (< n 0) _ (if (<= n 0) _ (%f64-sqrt n)))`, with `<`/`<=` the canonical PRELUDE
comparisons and `%f64-sqrt` the native. That shape is what makes the x>0 shortcut sound (a positive
argument provably returns `%f64-sqrt(n)`); every other argument still deopts to the live wrapper via
the stored head. Three safety properties fall out: (1) a user's own `foo/sqrt` computing something
else fails the match and never inlines — no miscompile; (2) `Inst::Prim1` re-runs `resolve_prim1` on
every `global_epoch` change, so a rebind of `<`/`<=`/`%f64-sqrt` drops the inline; (3) any rewording
of `std/math`'s `sqrt` simply stops inlining (degrades, never miscompiles) — guarded by the unit
test `sqrt_call_site_inline_recognizes_the_moved_math_wrapper`, which also pins the no-inline case.
Measured (release, pinned, 3M iterations): inline **321 ms** vs the wrapper-dispatch path **905 ms**,
so the row's regression is recovered.

**The generalisation worth keeping.** A kernel fast path keyed on a *bare stdlib name* is a
hidden coupling to the stdlib's shape: moving the function is a source-compatible change that
silently deletes the optimisation. `resolve_prim`/`resolve_prim1` and the checker's
`symbol_is`/curated-sig tables are all in this class — the same ADR-227 move left stale bare
keys in `sigs.rs`, `infer.rs` and `walk.rs` (fixed 2026-08-17), one of which had been masking
the unbound lint. When a stdlib function moves, grep the kernel for its bare name.

---

## KI-43 — `remote_attach_reads_snapshot_then_sees_disconnect` killed the target on a stopwatch

**Status:** ✅ **fixed 2026-08-14.** The test now waits for the observer's own attach report
before killing the target, and passes **8/8 under saturating load** (14 busy loops on 12 cores)
where the old form would have failed all eight. It also got *faster* — 3.5 s idle instead of
~11 s, because a 5 s unconditional sleep is gone.

**Not a flake, despite looking exactly like one.** It failed **both** tries in a full `make test`
(the real-TCP group carries `retries = 1`) and passed in 10.8 s standalone, which is the classic
signature people write off as load noise. The observer's failure was:

```
observer.blsp:4:1: connect: Connection refused (os error 111)
    (def peer (connect "app@127.0.0.1:26720"))
--- target stderr ---
dist: incoming connection failed: failed to fill whole buffer
```

**The mechanism.** The test spawned the observer and then slept a **fixed 5000 ms** before
killing the target. But the observer must boot a debug `brood`, `node-start`, `require 'observer'`
(a large module) *and* connect inside that window. Measured under saturating load, the whole case
needs **5.9–9.2 s** — every sample above the 5 s deadline. So the target was killed *before* the
observer's `connect`, which is why the port refused and why stdout was **empty** (it never
reached its first `println`). Under load this was not unlucky, it was arithmetic.

**Two red herrings worth recording, because both cost time here.**
- `dist: incoming connection failed: failed to fill whole buffer` is **not** a fault. It is
  `wait_until_listening`'s readiness probe: that helper proves liveness with a bare
  `TcpStream::connect` and drops it, and a *dist* listener accepts it, tries to read a
  handshake, and gets EOF. Expected noise on every node test.
- It does **not** kill the acceptor, which was the first hypothesis and was wrong:
  `spawn_acceptor` (`dist.rs`) wraps each connection in its own thread plus
  `catch_unwind`, logs the error and keeps looping. Verified before believing it.

**The lesson, which generalises past this test.** Two earlier sessions had already bumped this
constant (1500 → 5000 ms) — the same fix applied twice to the same wrong idea. A fixed sleep
standing in for "the peer is ready" cannot be tuned right, because the quantity it approximates
scales with machine load. Wait for the *event*: the marker read makes the wait proportional to
the box, and it strengthens the assertion, since "attached before the kill" is what the case
actually means and is now checked rather than assumed. The 60 s deadline that remains is a
hang backstop, not a timing assumption.

**Three more fixed-deadline waits, seen once, not reproduced (2026-08-17).** Under a
*self-inflicted* 2x load — two `make test-both` invocations overlapping on one box and one
target dir, which is not a supported configuration — three in-language cases failed on the
first attempt and passed on retry: `tcp: activity resets the timer` (got `:reaped-BUG`: the
activity messages lost the race with the idle timer), and two `proc` cases waiting on a `cat`
subprocess (`:timeout`). A clean run of the same tree was 983/983 on both engines with **no
flaky marker at all**, so there is nothing to fix on today's evidence. They are recorded
because they are the same *shape* as this issue — a deadline standing in for an event — and so
are the next candidates if any of them is ever seen again: `tests/tcp_test.blsp:115`,
`tests/proc_test.blsp:39` and `:57`.

**Found in the same pass:** `completion_never_fails_however_it_is_called` timed out at the 120 s
default for a related reason — it is 96 child-process spawns, measured at 59–61 s solo, so it had
2× headroom against a load factor of ~2×. Given its own budget in `.config/nextest.toml`, with
the measurements and the reason recorded there.

---

## KI-42 — the breakage suite had rotted: 9 of 23 files red, unnoticed for months

**Status:** ✅ fixed 2026-08-13 — all **23 of 23** files pass and gate, nothing skipped, and a
CI job runs them so this cannot rot silently again.

**What.** `make breakagetests` is deliberately outside `make test` (slow, abusive by design).
The consequence nobody had priced in is that **nothing ever ran it**, so it rotted. Found while
gating ADR-224; all nine failures reproduce identically on the pre-change baseline, so none of
them were the change under test. The causes were mundane and all in the *tests*, not the
runtime — which is the point: an ungated test suite decays into noise, and then a real finding
in it cannot be distinguished from the noise.

| cause | files | fix |
|---|---|---|
| the pin pattern moved from `~ref` to `^ref`; these predate it | `chaos2_process_{crash_propagation,genserver,links,ring}`, `chaos_pattern_hell` | mechanical (9 pins), leaving quasiquote `~`/`~@` alone |
| `string-contains?` was renamed | `chaos_string_cancer` | → `includes?` (strings = substring) |
| `(assert= 4.8 (/ 24 5))` predates exact rationals | `jit_breakage_test` | → `(assert= 24/5 …)`; brood's `=` is exactness-sensitive, so this had failed on **every** build since |

**`chaos2_tcp_stress` — every phase was dead, and it is now fixed.** The file's own header
states the model: `tcp-listen` delivers `[:tcp-accept …]` to *the calling process's* mailbox. It
then created the listener in the parent and accepted inside a spawned server in **all** of
P36–P40, so every accept went to the wrong mailbox and timed out — `P36 ok=0 bad=50`, P37–P40
all `:timeout`, with the timeout keyword then reaching `tcp-controlling-process` and surfacing
as a type error that hid the cause. (An earlier draft of this entry described it as two
unguarded call sites; it was the whole file.) Each server now opens its own listener and hands
the port back to the parent, which waits for it before connecting. P36–P42 all pass.

That fix also exposed a second stale assumption in **P38**, the phase that claims to round-trip
raw bytes `0x00`–`0xFF`: it built a *string* of codepoints 0–255 and sent that, but a string
goes over the wire **UTF-8-encoded**, so 256 "bytes" arrived as 384 (visible as `\xc2\x80…`).
It now sends `(apply bytes (range 256))` and compares as bytes — which is what the phase always
claimed to be testing.

**`chaos_map_volcano` — a sizing question, and a self-inflicted false alarm.** Its 1 000 000
-entry map peaks at **~3.0 GB RSS** (13 s), against the ~1 GiB soft ceiling the test runners
default on (ADR-043) so an adversarial test cannot take the machine down. It now runs with a
per-file allowance in the Makefile (`BREAKAGE_ENV_map_volcano`, soft 4 GB / hard 6 GB) and
passes; the default ceiling is untouched for every other file.

**An earlier version of this entry claimed a robustness bug here, and it was wrong.** The
report was that at a 2 GiB limit the *allocator aborts* (`memory allocation of 981893 bytes
failed`) instead of raising the clean catchable limit error. That is the **documented backstop
working exactly as designed**: `core/alloc.rs` enforces the *hard* limit inside `alloc` by
returning null (so Rust's OOM handler aborts and the host survives any allocation pattern),
while the *soft* limit is checked at an eval safepoint and raises `E0043`. The soft limit must
therefore sit **below** the hard one. The abort was induced by setting `BROOD_MEM_LIMIT` and
`BROOD_MEM_SOFT_LIMIT` to the *same* 2 GiB value, which leaves the safepoint check no headroom.
Verified: soft 1.5 GB / hard 6 GB raises the clean error at 1.9 GB allocated, as documented.
The ordering rule is now recorded beside the allowance in the Makefile, since getting it wrong
converts a graceful failure into an abort and looks like a runtime bug.

**Why it could rot at all, and what now stops it.** Three things hid it, and each is worth
knowing separately:

1. **No runner.** Now a `breakage` job in `.github/workflows/ci.yml`, guarded to main pushes
   and `workflow_dispatch` exactly like the tree-walker differential, with failing files
   emitted as annotations and the log uploaded as an artifact.
2. **Truncated output.** The suite prints one `===== file =====` block per file and the
   failures scroll; reading it through `tail` shows the *last* failure and reads as "one
   failure". That is how this was first mis-scoped as a single broken assertion.
3. **A file that dies still says little.** Most breakage files register no `deftest` at all —
   their chaos runs at load and the exit code is the whole signal — so `--test` prints
   `0 tests, 0 passed` whether the file did its job or died on line 3. The exit code is
   correct and is what the runner checks; just do not read the per-file test counts as
   coverage.

---

## KI-41 — a concurrent `require` of the same feature could double-load its file

**Status:** ✅ **fixed 2026-08-13.** Guard:
`breakage/chaos_concurrent_require_double_load.blsp`.

**What.** Two processes racing to load the same feature could both load it. The claimant's
`(contains? *features* key)` guard read the **per-process global inline cache**, which is
version-gated on a `Relaxed` counter with no happens-before, so it could miss a racing
loader's just-committed `provide` — win the released load-once claim, and re-load the module.

**How it surfaced.** As the ADR-225 co-located-secondary `nest test` flake, about 1 run in 77
— then reproduced on demand at 20 files × 40 requires, which is what turned it from a
sighting into a bug.

**Fix.** `require-one` re-checks `*features*` through a new cache-bypassing
`%registry-member?` (which reads the shared globals table directly) before loading.

---

## KI-40 — shared compiled arms serialise concurrent VM execution: one refcount, N cores

**Status:** ✅ **fixed 2026-08-13** (ADR-224). A process-local
[`ArmHandle`](../crates/lisp/src/eval/compile/ir.rs) is interposed between the inline cache
and the runtime-shared arm, so the per-call clone lands on an allocation only one process
touches. **`pfib` (100 × `fib(32)`, tier 1) 54.4 s → 17.1 s (3.19×)**, per-task CPU 6408 →
2006 ms, and now at **parity with `BROOD_NO_SHARED_ARMS=1`** (16.8 s) — the contention is gone
rather than reduced. Single-threaded rows unchanged (8-row A/B, every row inside its own noise
floor); `spawn-live` pays **+1.1%** for the handle allocation, a measured and accepted trade
(ADR-224 — which also records why an earlier +1.8% reading was measured in the wrong
configuration, and why interning the handle was tried and rejected). Guarded by `arm_handle_clone_does_not_touch_the_shared_arm_refcount`, sabotage-verified.
The diagnosis that follows is kept because the *shape* of it is the reusable part.

`BROOD_NO_SHARED_ARMS=1` was the diagnostic lever throughout and was never a shippable
workaround (sharing exists for the `spawn-live` 4.5 GB footprint and ~25% spawn CPU).

**What.** When several green processes run the *same* VM-compiled arm at once, per-task CPU
inflates far beyond what the same parallelism costs anywhere else. On `pfib` (100 × `fib(32)`,
`BROOD_TIER=1`, 6-core/12-thread i5-11500H):

| | wall | CPU% | CPU per task |
|---|---|---|---|
| default | 52.7 s | 1179% | 6213 ms |
| `BROOD_NO_SHARED_ARMS=1` | **17.6 s** | 1172% | **2058 ms** |

Serial `fib(32)` on the VM is 790 ms, and 12 *independent OS processes* each running it inflate
to 1998 ms (2.5×) — pure SMT + all-core clock drop. So 2058 ms **is** the machine's floor, and
everything above it was contention. The CPU figure is the tell: at 12 tasks the default runs at
**769%** against 1148% with sharing off — the cores are stalled, not busy.

**This is not tier-1-only.** At the default ceiling, an arm that simply does not lower keeps
paying it: 24 × `fib(30)` under `BROOD_NO_I64=1 BROOD_NO_INLINE=1` runs 3110 ms, and 1540 ms
with sharing off — **2.0×**. Any concurrent workload whose hot arms stay on the VM is exposed,
which is the normal case for non-numeric server code (a Hatch request handler, not `fib`).

**The 2×2 that isolates it.** 24 concurrent processes, `fib(32)`, tier 1, ms of CPU per task:

| | sharing **on** | sharing **off** |
|---|---|---|
| **same** arm | **3881 (4.9×)** | 1925 (2.4×) |
| **distinct** arms (24 separately-defined `fib_i`) | 1992 (2.5×) | 2046 (2.6×) |

Only one cell is slow. Three independent cells land on the 2.4–2.6× machine control, so the
cost needs *both* a shared arm object *and* multiple threads touching it. That rules out
sharing's bookkeeping (distinct arms + sharing on is fast), concurrency as such (every cell is
24-way), and resident-process count (12 vs 24 resident measure the same, 4381 vs 4201).
`BROOD_GC_FLOOR=2000000` makes it **worse** (3044 vs 2345 ms/task), so it is not GC frequency —
the same negative result `compute-frontier.md` records for `bintree`/`nqueens`.

**Mechanism.** `Heap::vm_call_ic_probe` (`core/heap/vm_cache.rs:507`) returns the cached arm as
`a.clone()` — an `Arc<CompiledArm>` refcount increment, matched by a decrement when the caller
drops it — and that is the **IC-hit** path, i.e. the hot one, taken on every VM closure call.
The IC table is per-process, but ADR-175 Phase B publishes the arm itself to `shared_closures`,
so every process's IC entry points at *one* allocation. N workers then RMW one cache line per
call. With sharing off each process compiles privately, the refcount is core-local, and the
traffic disappears.

**There is a second contended clone, and it is the one real code hits.** `vm_apply`
(`mod.rs:2700`) also does `live_arm_push(arm.clone())` per activation — but *only* when
`arm.has_runtime_handles`, so `fib`'s pure arithmetic body skips it and the benchmark above
measures the IC clone alone. An arm carrying a `ConstVal::Handle` or a `MakeClosure` — i.e.
anything that touches a string, a keyword or a lambda, which is most real code — pays both.
Same recursion shape with a string constant in the body, 24-way, tier 1: 2278 ms/task shared
against 991 ms unshared, a **2.30×** sharing penalty against **2.02×** for pure `fib`. Worse,
but not by much, so the IC clone remains the dominant site and `live_arm_push` is a second one
to fix, not the headline. (That the variant is handle-bearing is inferred from the constant, not
read from `has_runtime_handles` — there is no flag that reports it.)

Note `gc_runtime.rs:683` already documents the shape from the GC side: recursion "occupies one
entry per *active frame* — a 100 000-deep parse holds 100 000 entries that are all the same
`Arc`", and that walk was already given a distinct-arm dedup for the same reason.

The cache line itself was **not** measured — `perf_event_paranoid=4` here, the same limit
`compute-frontier.md` records — so the mechanism is an inference from the 2×2 plus the code,
not a hardware-counter proof. What the 2×2 does establish without counters is the shape: the
cost needs one shared object and many threads, and scales with threads, not with objects.

The tree already names the cost and already fixed it for the other engine:
`vm_call_ic_fast_link` (`vm_cache.rs:518`) returns only `Copy` data and documents itself as
avoiding *"the one real atomic-RMW the hot recursive call (`fib` &c.) otherwise pays per call
(~30M times)"*. That path is `#[cfg(feature = "jit")]` and serves native-to-native links only,
which is exactly why tier 2 is immune and the VM is not.

**How it was fixed (ADR-224), and why not the other way.** The route below — making shared
arms immortal so the call path could borrow — was **not** taken: it is sound for a sealed
PRELUDE arm but not for a RUNTIME one, which `shared_closures_clear` and the free-epoch stamp
exist to invalidate. Interposing a per-process `Arc<ArmHandle>` instead needs no immortality
argument, because it holds the shared `Arc` for at least as long as a direct clone would —
liveness is strictly *stronger* than before, and `runtime_collect` still reaches the same arms
via `ArmHandle::arc()`. All three per-call clone sites move to the handle at once. Cost: one
extra allocation per (process, call site) at IC-fill time, and one pointer hop on arm reads,
which measured inside the noise floor on every single-threaded row.

**Why it was not a *small* fix.** The self-tail path only pointer-compares the arm
(`std::ptr::eq(compiled.as_ref(), arm)` in `exec_chunk.rs`), so *that* clone is pure waste and
could go today — but `fib` recurses **non-tail**, and the general call path hands the arm onward
as an owned `Arc`. Removing the traffic there means the callee chain can borrow instead, which
needs the arm to be immortal — sound for a sealed PRELUDE arm, not obviously sound for a
RUNTIME-region one, which `shared_closures_clear` and the free-epoch stamp exist to invalidate.
The shape is an `ArmRef` that is either `Owned(Arc<…>)` or `Immortal(&'static CompiledArm)`,
touching the ~42 `Arc<CompiledArm>` sites across 11 files. `live_vm_arms` wants the same
treatment as a second step — a stack of raw pointers beside a small distinct-owner table, which
is close to what its GC walk already reconstructs per collection with `seen_arms`. Both must
clear the GC-stress and
hot-reload gates, not just the differential — the VM's answer stays correct either way, so
**only a benchmark moves**, the same blind spot ADR-221 hit.

**Repro.** `BENCH_N=32 TASKS=100 BROOD_TIER=1 brood pfib_k.blsp`, with and without
`BROOD_NO_SHARED_ARMS=1`, where `pfib_k.blsp` is `bench/brood/pfib.blsp` with the task count
read from `TASKS`. Compare CPU% between the two, not just wall. Measure with
`/usr/bin/time -f "%e %P"` — **not** `timeout(1)`, which rounds wall up to a 100 ms grid and
silently flattens every sub-second row (the published harness uses Python's
`subprocess(timeout=)` and is unaffected).

---

## KI-39 — the tree-walker CI job fails intermittently, and the log was unreadable ✅ FIXED 2026-08-18

**Recurrence 2026-08-17, and the diagnostic was broken.** Run `32032421650` failed this job
with nextest exit 100 — the only red job of five (breakage, rustfmt, `examples still run` and
`clippy + test` all green). The annotations added on 2026-08-13 so a sighting would name its
failing case produced **nothing**: only "Process completed with exit code 1" and "exit code
100".

The cause is the annotate step itself. GitHub runs `shell: bash` as `bash -eo pipefail`, so
`grep -hoE '…' log | sort -u | head -25 | while read; do echo ::error::…; done` **exits 1 when
the grep matches nothing** — killing the step before the later greps run — and `head -25` can
SIGPIPE the grep (141) for the same effect. Verified under the exact flags: the old pipeline
exits 1 and never reaches the following line; with `|| true` appended it continues. All six
annotate pipelines (this job, `breakage`, `examples`) are hardened.

So this sighting is *still* undiagnosed, and the artifact that would name it
(`tree-walker-nextest-log`) needs an authenticated download, which is blocked — see the note
on `gh` below. **One candidate, unproven:** the deep-JSON case fixed the same day
(`jit_tail_chain_depth_test.blsp`, native-stack guard at a 1.6 KB margin) is *more* likely to
trip under `BROOD_VM=0`, where everything runs tree-walked and native stack use is far higher.
It reproduces only in suite context, so it could not be confirmed against this run.

**Second recurrence, 2026-08-17 (run `32054863012`, commit `f1c4336d`) — and the hardened
diagnostic STILL said nothing.** Four of five jobs green (`breakage`, `rustfmt`, `examples still
run`, `clippy + test`); only this one red, exit 100 again. The `|| true` fix demonstrably worked
— the annotate step's own "exit code 1" annotation is gone and the step reports success — but it
produced no annotation at all, **not even the `Summary [...]` line nextest always prints**. So
the CI log differs from every local log in a way all three patterns miss, and the run stayed
undiagnosable because the artifact needs an authenticated download.

Local evidence continues to say the suite itself is fine: `BROOD_VM=0 cargo nextest run` passes
**975/975** here, twice on different trees.

Hence a third iteration on the instrument: an **unconditional fallback** that, when none of the
patterns match, annotates the log's last 30 lines verbatim plus its size. A diagnostic that
reports nothing when its patterns miss is precisely the failure this step exists to prevent —
three sightings have now been lost to it.

**Blocked on:** ~~`gh`'s stored token is invalid~~ — **unblocked 2026-08-25.** `gh auth
status` reports a valid token (scopes `repo`, `admin:org`, `gist`, `project`) and
`gh api rate_limit` returns the authenticated 5000/hour rather than the anonymous 60, so
artifact and log downloads no longer 401. The next sighting is readable, which is the whole
point of the artifact upload above: nothing else is waiting on this entry.

**Status:** ⚠️ watching (2026-08-12). Not reproduced locally. The diagnostic gap is closed:
the failing case is now uploaded as a CI artifact, so the next occurrence is readable.

**What.** The `differential (tree-walker)` job (`cargo nextest run --no-fail-fast` with
`BROOD_VM=0`) fails with **exit 100** — nextest's "tests failed" — on some runs and passes on
others, with no relevant change between them. Observed on `main`:

| commit | tree-walker | note |
|---|---|---|
| `aeac1ae0` | ✅ | |
| `14b1db40` | ❌ | two test fixes (registry, tls) |
| `79e7e555` | ❌ | ADR-222, the tier ladder |
| `d7508f8c` | ✅ | **workflow file only — identical test code to `79e7e555`** |

That last row is what makes it a flake rather than a regression: same tests, opposite result.
Two consecutive failures had made a deterministic cause look likely; it was not.

**What has been ruled out.** The suite passes locally at every configuration tried:
`make test-both` 977/977 + 977/977; `BROOD_VM=0 cargo nextest run` 977/977 on 12 cores;
the same under `taskset -c 0,1` 977/977. The two test files changed in `14b1db40` were each
hammered 15× at ceiling 0 (`BROOD_TIER=0`) with **0 failures**. It is not the 1200s→2700s
wall-clock budget either: the failing jobs ran ~30 min end to end, well inside it.

**One caveat on the local attempts, worth not repeating.** `taskset -c 0,1` limits *affinity*
but not what nextest sees — it still detected 12 CPUs and started 12 test threads onto 2 cores,
which is **more** oversubscribed than the runner, not equivalent. GitHub's `ubuntu-latest` has 4
CPUs and nextest defaults its thread count to that, so the faithful local shape is
`taskset -c 0-3 … -j 4` together.

**What was done instead of guessing.** Reading a run's log needs **admin** on the repository
(`/actions/runs/:id/logs` → 403; `rerun-failed-jobs` → 401), so a failure here was
undiagnosable to anyone without those rights — the wrong property for the only gate that catches
tree-walker-only divergence. The step now tees its output and uploads it on failure
(`actions/upload-artifact`, 14-day retention); artifacts download with plain read access via
`gh run download`. Nothing is uploaded on a green run.

**Update 2026-08-12 — a mechanism hypothesis raised and killed by measurement.** The best theory
was a **cold-boot herd the KI-38 fix does not cover**: `scripts/warm-boot-cache.sh` warms `brood`
and `nest` but explicitly not the ~50 test binaries (each is keyed on its own mtime), so on CI —
where every commit rebuilds and colds every key — each test binary pays a cold prelude expansion,
and if that expansion ran on the *tree-walker* it would cost ~10× and reproduce KI-38's herd in
the one job that flakes.

**It does not.** Measured, same binary, trivial program:

| | cold | warm |
|---|---|---|
| default (ceiling 2) | 1213 ms | 98 ms |
| `BROOD_VM=0` | **1274 ms** | 98 ms |

Prelude macro-expansion does not go through the selected engine — `BROOD_BOOT_TRACE` shows
`expand=1.20s` either way, and the cache key (`build_id_string`: version + git sha + executable
mtime) is engine-independent, which is *why* one cache file serves both. So the tree-walker job is
no more exposed to cold-boot cost than the VM job, and this line of attack is closed. Recorded so
it is not re-derived.

**What the retry policy narrows it to.** Only three filters carry `retries = 1` (`binary(suite)`,
`binary(distribution)`, `binary(serve_attach) | binary(observe_attach)`); everything else gets no
retry. The failing runs showed exit 100 with **no `FLAKY` row**, so the failing case is either one
of those three failing *both* attempts — which would make it reproducible-within-a-run rather than
a coin flip — or any un-retried test failing once. That is a genuine narrowing, and it argues
against "marginal timing" for the retried binaries.

**Not reproduced in six local configurations:** `make test-both`; `BROOD_VM=0` alone on 12 cores;
`taskset -c 0,1` with 12 nextest threads; `taskset -c 0-3` with `-j 4` (the faithful runner shape);
plus the two changed test files hammered 15× each at ceiling 0. All 977/977 or 0 failures.

**Update 2026-08-13 — the local hunt is exhausted: 0 failures in 15 runs, and that is where it
stops.** Fifteen runs of the *single* most faithful shape (`BROOD_VM=0`, `taskset -c 0-3` **and**
`-j 4` together, prelude cache colded each iteration as a fresh CI commit does): **978/978 every
time**, wall time 978-1015 s, a 3.7% spread with no outlier and no extra slow-test warning.

Chosen deliberately over "one run each of several configurations", which is what the earlier
attempt did and why its negative result was weak (see the correction above). Fifteen clean runs
put the miss probability at **0.8%** — so on this machine, in this shape, the flake is not there
to find. That is a statement about the machine, not about the code.

**Do we know the bug is still present? No — and four green CI runs do not say otherwise.** At the
observed 27% rate, four consecutive greens have a **28%** probability: exactly what you would see
anyway if nothing had changed. "Fixed" and "still there" are both unfalsified. Three live
possibilities:

1. **Still present, and we have been lucky** — ~28% likely on its own.
2. **Fixed in passing.** Weak: the last failure (`79e7e555`) came *after* the registry and tls
   fixes landed in `14b1db40`, so those are not the explanation, and nothing else in that window
   plausibly touched it.
3. **Runner-dependent** — a property of which machine in GitHub's fleet the job lands on, which
   would explain 15 identical local passes and 4 CI passes against 3 earlier failures.

Most weight on 1 and 3.

**What would settle it, either way.** To call it *gone* wants roughly **10 consecutive green CI
runs** (0.73^10 = 4%); there are 4 so far, and they accumulate for free with ordinary pushes. To
call it *found*, the annotations added in `2312d4a1` name the failing case the moment it fires —
now the only live path, since the local avenue is closed. **Do not repeat the local hunt:**
15 runs at ~16.5 min is ~4 hours, and this entry is the record that it returned nothing.

**Update 2026-08-13.** The specific CI-faithful shape this entry recommends — `taskset -c 0-3
… --test-threads 4`, the one earlier runs (12-core, `-c 0,1`) had *not* actually used — was
looped **3× clean** (981/981 each, ~15 min/run), plus a `make test-both` tree-walker pass the
same day. So the faithful shape is now on record as *also* returning nothing; the four failing
CI observations remain unreproduced locally in any shape tried. Still a watch item, not a
blocker — the CI-artifact path stays the only live avenue.

**Next step when it recurs.** `gh run download <run-id>` → `tree-walker-nextest.log` names the
failing case. Until then this is a watch item, not a blocker: the VM job and rustfmt have been
green throughout, and the tree-walker gate exists to catch engine divergence, which
`tests/differential.rs` and `tests/gabriel_engines.rs` also cover per-expression inside the
normal run.


### Resolved 2026-08-18 — two bugs, one hiding the other

`gh auth login` was restored, the artifact downloaded, and both halves fell out at once.

**Why the diagnostic said nothing (three sightings lost).** nextest colours its output under
CI even when piped to a file, so the log holds

    <ESC>[31;1m     Summary<ESC>[0m [1922.084s] 976 tests run: 975 passed, 1 timed out, 3 skipped

Every pattern in the annotate step anchors on `WORD [` — `Summary \[`, `FAIL \[` — and none of
them can match `Summary<ESC>[0m [`. Verified against the real log: **both patterns match zero
lines.** Local logs are uncoloured (not a TTY, and no CI env), which is why the greps looked
correct here and were dead there — and why "it prints nothing" was never a clue about the
tests. The tail-dump fallback added the day before *did* fire and *did* carry the answer, but
its `sed 's/[[:cntrl:]]//g'` stripped only the ESC byte, leaving `[31;1m` litter in the output.

Fixed three ways: `--color never` on the nextest invocation (deterministic log at the source),
the annotate step now greps an ANSI-stripped copy (correct even if colour returns), and
`TIMEOUT`/`SIGSEGV`/`SIGABRT`/`ABORT`/`LEAK` joined `FAIL` in the pattern set. Re-verified by
replaying the real coloured log through the patched step under `bash -eo pipefail`: it exits 0
and annotates `TIMEOUT [ 300.010s] (901/976) nest::complete
completion_never_fails_however_it_is_called` plus the summary line.

**What was actually failing.** `nest::complete completion_never_fails_however_it_is_called`
timed out at 300 s. It spawns 96 `nest complete` subprocesses, and every one that reaches a
value position booted an interpreter and then `require`d `complete` — which opened with
`(:use-internals project)` and so loaded all 2967 lines of `project`, plus `scaffold`, plus
`project` again behind it. Measured 2026-08-18 in a debug build: **950 ms per completion, of
which 770 ms was that module load**; 96 × 950 ms = the 64 s the test took *alone on an idle
16-core box*, against nextest's 60 s slow line. On a 2-core runner sharing the machine with
`brood_suite_passes` it crossed 300 s. Fixing the load cost (see the devlog) took the test to
**10.5 s** and the completion to 121 ms.

**It was never intermittent.** The original entry called this a 3-of-11 flake. With the log
finally readable, every one of the 8 CI runs on 2026-08-17 failed this job, and the two jobs
that run the suite (`clippy + test` and this one) failed on the *same* case. What varied was
only whether the box was slow enough to cross the cap — the test had been sitting one
contention spike away from red since it was written. A "flake" that is really a fixed cost
against a fixed deadline looks random and is not.


## KI-38 — three boot-wait tests fail together under peak suite load · **fixed 2026-08-08**

**This is what KI-28 turned out to be part of.** KI-28 was recorded as "a single unexplained
`nodedown` flake" with a standing record of 0 failures in 260 runs, and said in terms: *"A
recurrence after this date — on a box with the KI-29 leak fixed — is a real signal worth
chasing."* It has now recurred twice, and both times it brought two siblings with it.

**The three tests, and the one thing they have in common:**

| test | wait helper | deadline | panic |
|---|---|---|---|
| `cli::distribution clean_peer_exit_fires_nodedown_promptly` (was KI-28) | `wait_until_listening` (`crates/cli/tests/support/mod.rs:193`) | 20 s | `server never started listening on port N` |
| `cli::child_cleanup a_brood_child_dies_when_its_spawning_thread_exits_ki29` | `wait_until_up` (`crates/cli/tests/child_cleanup.rs:66`) | 30 s | `child never wrote its marker at …` |
| `cli::child_cleanup the_drop_guard_kills_a_running_brood_child_ki29` | `wait_until_up` (same) | 30 s | `child never wrote its marker at …` |

Every one is a **boot wait** — a helper waiting for a freshly spawned *debug* `brood` to become
ready. None is an assertion about the behaviour the test exists to check. So the common factor is
**not distribution**, which is how KI-28 was framed and why the pattern stayed invisible: two of
the three are child-spawn tests that have nothing to do with nodes.

**The sightings.** Both are full `make test` runs, and in both the three fail together in the same
dense region of the schedule:

- **2026-08-06 18:41** — all three failed: 34.675 s / 35.142 s / 35.145 s, at position 840/966.
- **2026-08-07 21:20** — `…_ki29` failed at 35.521 s; `clean_peer_exit…` failed try 1 at 35.476 s
  **and passed on retry in 0.827 s**; at 841/974. The immediately following run of the same suite
  on the same commit was 974/974 green, and `make test-both` was 1948/1948 green.

**KI-28's open question is answered.** That entry said a recurrence should separate *"B never bound
its port"* from *"A could not connect to a listening B."* Both recurrences panic in
`wait_until_listening`: **B never bound its port.** It is a startup failure — not a failed
`connect`, and not the late-`[:nodedown]` shape the entry speculated about (that speculation was
already withdrawn there for having nothing under it; this closes it properly).

### Diagnosed 2026-08-08: a COLD expanded-prelude boot cache, times the herd

**The mechanism is contention after all — but on work nobody had measured, so the earlier
"it is a stall, not a slow boot" conclusion was drawn from a distribution that excluded it.**

`build_id_string()` = version + git sha + `binary_stamp()`, **the running executable's own
mtime**. The expanded-prelude boot cache (`~/.cache/brood/prelude-expanded-<hash>.blsp`) is
therefore invalidated by **every rebuild**, for `brood`, `nest` and all ~50 test binaries at
once. The first suite run after a build is a fully **cold** one — and that is the run you do
after committing, i.e. the one you are watching when you see the flake.

**A cold boot costs ~11x a warm one, and it is all macro-expansion** (`bootcost.sh`, isolated
via `XDG_CACHE_HOME`, idle box):

| | single boot | 16 concurrent |
|---|---|---|
| cold | 1227–1361 ms | max **4313 ms** |
| warm | 107–114 ms | max 359 ms |

`BROOD_BOOT_TRACE=1` names it: cold is `expand=1.102851931s` of `total=1.227s (source boot,
cache written)`; warm is `cache hit — total=93.9ms`.

**Every boot sample the previous entry rested on was warm.** The 151 ms idle / 4066 ms worst
figures came from a sampler run alongside and after suite runs on an already-built tree. The
cold path was never sampled, so "a 20–30 s timeout would have to be ~5x worse than the worst of
4915 samples" compared the deadline against the wrong distribution.

**Dose-response, and it is linear** (`doseresponse.py`, N cold boots sharing one cold cache,
12 cores):

| herd | 8 | 16 | 24 | 32 | 48 | 64 | 96 | 128 |
|---|---|---|---|---|---|---|---|---|
| cold, worst boot | 2.3 s | 4.2 s | 6.4 s | 8.8 s | 13.7 s | **18.4 s** | **27.3 s** | **36.5 s** |
| warm, worst boot | — | 0.32 s | — | 0.64 s | — | 1.30 s | — | — |

The 20 s deadline is crossed at a herd of ~70, the 30 s one at ~105 — and the observed failures
(34.7 / 35.1 / 35.1 / 35.5 s) sit where the curve puts ~120. Memory is **not** involved anywhere
on this curve: 24.7 GB still available at n=128, no sustained D state.

**Confirmed in situ, on the tests themselves.** Each row is a full suite run; the cold rows had
`~/.cache/brood/prelude-expanded-*.blsp` removed first, which is exactly the post-rebuild state:

| condition | `clean_peer_exit` (20 s) | `drop_guard` (30 s) | `pdeath` (30 s) |
|---|---|---|---|
| idle, standalone, warm | 0.492 s | 0.156 s | 0.206 s |
| suite `-j12`, warm | 1.110 s | 0.524 s | 0.414 s |
| suite `-j12`, **cold** | 4.714 s | 4.135 s | 3.723 s |
| suite `-j32`, **cold** | 7.123 s | 11.038 s | 11.484 s |
| suite `-j64`, **cold** | **FAIL 20.119 s** | 28.363 s | 26.697 s |

This also satisfies KI-36's methodological trap in the direction it demands: the load demonstrably
inflates the tests' own durations (0.5 s → 4.7 s → 7.1 s → past the deadline) before anything is
concluded from it.

### Reproduced deterministically 2026-08-08 — the first reproduction

```
rm -f ~/.cache/brood/prelude-expanded-*.blsp
cargo nextest run --no-fail-fast --features brood/treesit-grammars -j 64
```

`cli::distribution clean_peer_exit_fires_nodedown_promptly` — **TRY 1 FAIL [20.119s]**, panicking
in `wait_until_listening` at `support/mod.rs:199`, then **FLAKY 2/2, passing on retry in 2.850 s**.
That retry is itself confirmation: by then the herd had written the cache, so the retry booted
**warm**. The 2026-08-07 sighting has the identical signature (failed try 1, passed on retry in
0.827 s). The other two tests came in at 26.7 s and 28.4 s — the same regime, saved only by their
larger 30 s budget.

The stall report fired and reads: **`loadavg: 62.00`, `MemAvailable: 24865760 kB`, `SwapFree:
282200 kB`** — 24 GB free, swap untouched. CPU contention, not a stall, not memory, not I/O.

`-j 64` on 12 cores also broke `brood::gc spawned_process_reclaims_too` and timed out 3 cases.
**That is over-subscription damage at loadavg 80, not a regression** — the same commit is
974/974 green at the default `-j`, twice over (see below).

### Fixed 2026-08-08 — warm the cache once, before the fan-out

`scripts/warm-boot-cache.sh`, wired as a **nextest setup script** so it covers a bare
`cargo nextest run` and not just `make test`:

```toml
experimental = ["setup-scripts"]      # root-level; nextest 0.9.137 still gates this

[scripts.setup.warm-boot-cache]
command = 'scripts/warm-boot-cache.sh'

[[profile.default.scripts]]
filter = 'all()'
setup = 'warm-boot-cache'
```

It boots each spawned binary **once** (~2.4 s total) so the herd hits a warm cache. Two
details worth keeping: `brood` and `nest` carry **separate** cache files (different mtimes,
different keys), and `nest --version` does **not** boot the prelude while `nest complete --`
does — so warming `nest` needs the latter. Every failure path in the script exits 0: warming
is an optimisation and must never redden a run.

**The herd-miss is directly demonstrable**, which is the whole reason one boot up front is
worth ~2.4 s. Twelve children launched simultaneously against a *cold* shared cache — every one
misses, because none has finished writing it — then twelve more against the same cache once warm:

```
cold, 12 at once:  3.71 3.72 4.14 4.21 4.23 4.35 4.39 4.54 4.58 4.60 4.86 4.86   (seconds)
warm, 12 at once:  0.24 0.24 0.25 0.25 0.29 0.32 0.33 0.34 0.35 0.35 0.36 0.37
```

13–15x, and note the cold spread is *tight*: they are not queueing behind one another, they are
each doing the full 1.1 s expansion in parallel and contending for cores while they do it.

**Verified against the reproduction**, same command, same `-j 64`, same cleared cache:

| test (deadline) | before | after |
|---|---|---|
| `clean_peer_exit` (20 s) | **FAIL 20.119 s** | **2.599 s** |
| `drop_guard` (30 s) | 28.363 s | **1.926 s** |
| `pdeath` (30 s) | 26.697 s | **1.991 s** |

10–14x faster, and from 94% of the deadline consumed to 13%. The whole `-j 64` run also
improved from *1 failed, 3 timed out, 1 flaky* to *1 failed, 1 timed out, no flaky* — the two
that remain are the over-subscription damage described above (`gc spawned_process_reclaims_too`
failed in the pre-fix `-j 64` run too), not a regression; both pass at the default `-j`.

**What this does not cover, stated so nobody assumes otherwise.** Each of the ~50 test binaries
also boots the prelude **in-process**, and each has its own cache file keyed on its own mtime —
so they cannot be warmed without running them, which is what the suite is doing anyway. Those
boots still pay ~1.2 s once each after a rebuild, and they still contribute to the herd. What
the fix removes is the *repeated* cost: the dozens of spawned children that were each paying
the expansion because they all started before any of them had written the cache. That is the
part that scaled with the herd, and it is the part the deadline was racing.

**A cheaper fix was considered and rejected**: key the cache on the prelude's content rather
than the binary's mtime, so all ~50 binaries could share one file. The mtime is there precisely
because an uncommitted Rust change to the *expander* changes the expansion without changing the
prelude text or the git sha — content-keying would serve a stale cache during exactly the
development loop this repo lives in. Not worth the correctness risk to save ~50 boots.

### What the sighting rate means

~1 in 11 is what this mechanism predicts with nothing exotic added: **one cold suite run per
rebuild**, every other run in the session warm. It also explains why the cluster is *only* these
three tests. Many tests spawn a `brood` child, but these are the only ones that wait for a booted
child **against a deadline**; everywhere else a slow boot is absorbed as a slightly slower test.

A single default-`-j` suite run peaks at **27–29 concurrent `brood`** (measured, 1 Hz), which the
curve puts at ~8 s — short of the deadline. So a real sighting needs the herd roughly 2–4x that,
from load *beside* the suite. The handoff records exactly such a condition 24 minutes before the
2026-08-07 sighting: two debug suites running at once, which ended with the editor process dying
on memory at 20:56.

### `stall_report` was blind in the way that mattered — FIXED 2026-08-10

The armed diagnostic could not have distinguished the candidates it was built for. It read
`/proc/<pid>/stat`, i.e. **the main thread only**, and a `brood` runtime parks its root thread
on a futex while worker threads do the work. So in the reproduction every process — including
the children burning CPU — printed `S futex_do_wait`, and `D` / `R` / dead all presented
identically. Its filter was over-broad too: `cmd.contains("/brood")` matches every binary under
the repo path `…/broodlang/brood/`, so the report was mostly test binaries and the invoking
shell.

**Fixed:** it now reads **per-thread** state from `/proc/<pid>/task/*/stat` and prints every
thread's state char, the `wchan` of the first non-`S` thread, and the process's total CPU ms;
and it matches on argv[0]'s **file name** (`brood`/`nest`) rather than a substring of the path.
Verified by sabotage — the child's marker write removed so the wait really times out:

```
live brood/nest (pid states-by-thread cpu-ms wchan cmd):
  3469426 [S,S,S,S,S,S,S,S,S,S,S,S,S,S,S] cpu=1380ms S:futex_do_wait  …/brood …/park.blsp
```

Two processes listed instead of ~40 lines of harness, and the child reads as *booted then
parked idle* (15 threads all `S`, CPU flat). A stall now shows a `D` thread with its `wchan`,
and contention shows an `R` thread with CPU rising between two samples — the discrimination
the report existed for.

**Do not merge KI-36 into this.** Different test, and its 22.6 s was analysed against its own three
deadlines (nodedown 15 s / pong 20 s / nodeup 45 s) and points at the nodedown branch. It may be
the same family; nothing yet says it is, and a premature merge would bury one of the two.

---

## KI-37 — an imaged start never followed a module's require edges · **fixed 2026-08-07**

**Found 2026-08-07** while scoping `nest run`'s cold pre-flight, and confirmed within the hour
by an independent sighting on a real project: `hatch-demo` (24 files plus a `_deps/hatch`
dependency) ran fine cold and died on the **second** run with
`unbound symbol: hatch-demo/db/start`.

**The defect.** Materialising an image section **defines** a module's bindings; it does not
**evaluate** its source. So the `(defmodule shapes (:use geom))` header — whose expansion is
what runs `(require 'geom)` — never runs. `require-force`'s image branch restored the section
and `provide`d the feature, and stopped there. An imaged start therefore built a heap with
holes and the program died at the first call across a missing edge. Third defect in the same
loader seam after KI-34's two, and the same family as KI-35: the image carried a module's
*bindings* and silently dropped a piece of its *behaviour*.

**It looked like it worked, and here is why — the part worth keeping.** `nest run`'s advisory
pre-flight `check-file`s each source file, and checking a file incidentally `require`s its
header's deps. That propped up **exactly one level** of the graph, for free, invisibly. So:

- a project whose entry `:use`s its modules directly worked, and every fixture in
  `crates/nest/tests/startup_image.rs` was exactly that shape (`app (:use lib)`,
  `app (:use shapes)`, `app (:use libdep/util)`) — five tests, all blind to it;
- a two-level chain died on the second hop;
- **the correctness of `nest run` depended on an advisory pass**, and `BROOD_NO_CHECK=1` did
  not disable it (that flag suppresses warnings, not the checker's requires);
- `nest test` / `nest check` / the LSP were never affected — they call
  `project-materialize-all`, which requires every section, so every edge is satisfied trivially.

Isolating it needed the pre-flight removed entirely. With it out, a warm `nest run` on
`main → shapes → geom` fails at the **first** hop: `FEATURES: (demo/main)`,
`unbound symbol: demo/shapes/area`.

**Fix: record the edges during the load and replay them on materialisation.** A new root
registry `*require-edges*` maps feature → the features its body required.
`require-record-edge!` runs at the **top** of `require-one`, before its already-loaded
short-circuit — the edge is a property of the requiring module, not of who won the race to
load the target — and the image branch does
`(fold (fn (_ d) (require-one d)) nil (get *require-edges* key))` after `provide`.

Two details that cost a build each:

1. **The parent is `(current-ns)`, not a dynamic var set by `require-force`.** The first
   version bound `*require-parent*` around the load in `require-force`, which records edges
   for std modules and **none at all for the project's own sources** — those are `load`ed
   directly by the bulk loader, never through `require-force`. `current-ns` is the module
   whose *file* is being loaded, is set for every top-level form in it, and reads **nil at
   runtime**, so a runtime `require` is correctly not recorded as a load-time edge.
   `*require-parent*` survives only as the fallback for a file with no `(defmodule …)`.
2. **Recorded, not re-derived.** Re-parsing headers at materialisation time
   (`%module-direct-requires`) would work and costs ~2 ms/file — ~34 s on a 16 300-module
   project, which is the whole cost ADR-218 exists to remove. Recording is one CAS per
   *distinct* edge (a `member?` read is the lock-free fast path for repeats), and being a
   registry it rides into the always-materialised root section automatically via KI-35's
   derived set — so no image-format version knows about it.

A spurious edge costs one extra materialisation and nothing else; the error direction is
one-sided on purpose.

**Guard:** `crates/nest/tests/startup_image.rs::an_imaged_start_follows_transitive_require_edges`.
**The chain is inside a dependency deliberately** — a dep's modules resolve outside
`:source-paths`, so the pre-flight never checks them and cannot prop the test up the way it
propped up the bug. **Verified by sabotage**: with the replay removed it fails with
`unbound symbol: libdep/helper/triple` and the other five image tests still pass, which is
the measurement of how blind they were.

A companion, `an_imaged_start_terminates_on_a_require_cycle`, covers an imaged require cycle.
**It is a behaviour test, not a gate on the `provide`/replay ordering** — reordering those two
lines was tried and it still passed, because `require-one`'s `*features-loading*` marker is
what actually breaks the cycle. Recorded so a later reader does not credit it with more than
it does.

---

## KI-36 — a single `reconnect_watcher_heals_a_fallen_link` failure · ✅ **FIXED 2026-08-19**

**Resolved.** The third sighting finally carried its output, and the answer was not what the
analysis below predicted. **The failing branch was `TIMEOUT-no-pong`, not the nodedown deadline.**

**Root cause — a race in the test's own round-2 script, not in the runtime.** B2 ran

```
(node-start :b …)                                  ; listener open → A's watcher connects
(let (echoer (spawn …)) (register :echo echoer) …)  ; …but :echo exists only from here
```

A pings `{:name :echo :node up}` the *instant* it receives `[:nodeup]`, and by that point it has
already restored `(process-flag :send-errors nil)`. So a ping that lands before `register` is
**silently dropped** — no error, no reply — and A then waits out its full 20 s pong deadline for a
message that was never queued. No deadline extension could ever have fixed it.

**Why only this test, out of 16 sites with the same ordering.** Everywhere else the peer is
spawned *after* `wait_until_listening`, and booting a fresh `brood` costs ~150 ms–4 s — far longer
than B's spawn+register, so B always wins. This test is the sole case where the peer is **already
running and hot-looping a 100–400 ms backoff `connect`**, so it can land within milliseconds of the
listener opening. The other 15 are the same shape with no exposure; left as-is deliberately.

**Fix:** `register` precedes `node-start`, so the name is live before any peer can reach the node.

**Verified by sabotage, in both directions:**

| condition | result |
|---|---|
| pre-fix, 3 s delay inserted between `node-start` and `register` | **100% fail**, `TIMEOUT-no-pong`, byte-identical to the wild failure |
| post-fix, that 3 s delay **retained** | pass (1.44 s) |
| post-fix, 6 CPU burners at equal priority | **10/10 pass** |
| post-fix, 12 burners at *higher* priority than the test | failures are all `wait_until_listening gave up` (boot-class); **zero** `TIMEOUT-no-pong` |

**The original inference below was wrong, and worth reading as a lesson.** It reasoned that 22.6 s
"can only be the nodedown deadline (15 s + startup), not the nodeup deadline, which would have
taken at least 45 s." The flaw: reaching the *pong* branch does not cost the 45 s nodeup deadline —
nodeup fires promptly, because the harness restarts B2 ~400 ms after B1 exits. So the arithmetic is
setup (~2.6 s) + the 20 s pong deadline ≈ 22.6 s, matching the original sighting exactly. **The
hunt was aimed at a stall that never existed**, which is the most likely reason 25 idle runs and 14
loaded runs all came back clean. When a deadline analysis rules a branch out, check the *time to
reach* that branch, not just the branch's own timeout.

**Two red herrings, both recorded so the next reader discards them fast:**

1. `dist: incoming connection failed: failed to fill whole buffer` in B2's stderr is **benign
   harness noise**: `wait_until_listening` opens a bare `TcpStream::connect` and drops it with no
   handshake bytes, which is exactly an EOF mid-read on B2's accept loop. It appears on passing
   runs too.
2. This entry's own **method trap fired again**, on me, in the same shape it warns about. A first
   verification loop classified on "did the success line appear" and reported 8/12 failures — all
   of which turned out to be my 12 default-priority spinners starving a `nice -n 19` test's child
   boots (a priority inversion I created), not the race. **Capture and classify the output; a
   failure you cannot name is not evidence.**

The original entry follows, unaltered.

---

### (original entry, superseded) — **watching, seen once 2026-08-07**

**Seen once**, in the full `make test` that gated the dependency-imaging change: `TRY 1 FAIL
[22.567s]`, passed on nextest's retry, suite otherwise 970/970. The box was **not idle** — a
4000-module image build (≈2.9 GB RSS) was running beside the suite, which is the same condition
the 2026-08-06 devlog records as producing a false `division by zero` failure in
`brood_suite_passes`.

**Not reproduced: 0 failures in 25 idle runs** (retries off, one at a time) and **0 in 10 runs
under synthetic load** (six CPU burners plus repeated cold image builds). That second number is
weak evidence, and worth saying so: those runs took 1.6–1.9 s each against 2.58 s idle, i.e. the
load never actually reached the test's path.

**What the 22.6 s says.** The test has three liveness deadlines — nodedown 15 s, pong 20 s,
nodeup 45 s. A 22.6 s failure can only be the **nodedown** one (15 s plus node startup), not the
nodeup deadline, which would have taken at least 45 s. That branch is A failing to observe
`[:nodedown]` after B1 exits *cleanly*, where the socket EOF should fire it immediately and the
heartbeat path (`DOWN_AFTER` 6 s, 2 s ticks) is only the backstop — so 15 s is roughly 2× the
worst case, and exceeding it means a real stall, not a marginal deadline.

**Diagnostic to arm next time:** the failing run's stdout was lost because the suite output was
piped through a `grep` for summary lines. The test prints exactly which branch fired
(`TIMEOUT-no-nodedown` / `NODEDOWN-OK` / `NOCONNECTION-OK` / …) precisely so this question is
answerable after the fact — capture the full nextest output on any suite run that is expected to
be evidence.

**A clean re-run is green:** 970/970 with no flaky row, on an idle box, same commit. So the
sighting stands alone, exactly as KI-28's does.

**Update 2026-08-12 — the condition this entry called untested has now been tested: 0/14.** The
weakness recorded above was that the load hunts never reached the test's path (synthetic CPU
burners; runs came out *faster* than idle). So the real thing was reproduced instead: a
`gen-project.py 4000` fixture, its `.brood` image deleted every iteration, loaded in a loop by
the prebuilt `nest` — measured at **186% CPU and 352 MB RSS** with a load average of ~3 while the
test ran. **14 runs, retries off, one at a time: 0 failures.** Idle baseline the same day: 0/10.

That does not explain the sighting, but it does retire the "we never tried the real condition"
caveat: the 4000-module image build beside the suite is now a *tested* negative, not an untested
suspicion.

⚠️ **Method trap, recorded because it produced a convincing false positive first.** The first
attempt drove the load with `cargo run -q -p nest …` against the same workspace. That takes the
**cargo build lock** on the target dir the foreground `cargo nextest run` needs, so every
foreground run failed on lock contention — and a `grep -E "FAIL|failed"` matched *cargo's* error
text. It read as **12/14 → then 12/12 failures**, i.e. a reproduction, and it was nothing of the
kind: with the load stopped the same test passed in 1.3 s. Drive background load with a
**prebuilt binary**, never through cargo, and assert on the test harness's own summary line rather
than on any line containing "fail".

**Related:** KI-28 is the same shape in the same suite — a single unexplained dist failure that
passed on retry and has never recurred. Two independent one-off failures in the dist tests, both
under load, is worth correlating if a third appears; neither reproduces on demand today.

**Update 2026-08-07 (evening): KI-28 recurred and became [KI-38](#ki-38); this one did not.** A
further **25 consecutive idle passes** (retries off, one at a time, ~2.0 s each) plus four whole-
suite passes on the same commit — but note that only *repeats* this entry's own "0 failures in 25
idle runs" and therefore adds a third idle confirmation and nothing more. **It does not touch the
condition that produced the sighting** (a 4000-module image build running beside the suite), which
is exactly the weakness recorded above. This stays a watch item, deliberately **not** folded into
KI-38: KI-38's three tests all fail in a *boot* wait, whereas this one's 22.6 s points at the
nodedown branch, and merging on a resemblance would bury one of the two.

---

## KI-35 — `*method-from*` was never imaged · **fixed 2026-08-07**

**Filed separately from KI-34 on purpose**, for the reason KI-30 was filed separately from
KI-29: found while fixing another issue, and a bug recorded as an aside inside a fixed one is
invisible. This is also the *third* recurrence of one shape — a registry missing from the
startup image's hand-maintained list — after `declared_sigs` and the seven ability/multimethod
registries of 2026-08-07 morning.

**The defect.** `register-method` records `[multimethod key] → registering ns` in
`*method-from*` and warns when a *different* module re-registers the same key
("redefined from … by … (last wins)"). The image never carried that registry, so an imaged
start began with it empty: `has-prev` is false for every key, and the conflict warning stops
being emitted. Nothing crashes and dispatch still works — the last registration simply wins,
silently, which is exactly the class this list keeps producing.

**The fix is the mechanism, not the entry.** The set is derived now rather than named —
`%registry-update!`/`%registry-cas!` are the only ways a registry is written, so the kernel
records the names they write and `(%registry-names)` reports them (see KI-34 and ADR-218).
A registry added later is carried without anyone remembering to. What remains in
`std/tool/project.blsp` is an *exclusion* list, where a forgotten entry costs a redundant load
rather than a wrong answer.

**Guard:** `crates/nest/tests/startup_image.rs::an_imaged_start_keeps_what_loading_registered`
runs a project twice and asserts multimethod dispatch, ability dispatch, record identity,
method provenance and module docs all survive the imaged start.

---

## KI-34 — the startup image was written every cold start and never read from · **fixed 2026-08-07**

**Found 2026-08-07** while auditing KI-35's neighbourhood. ADR-218's per-module lazy
materialisation had not worked since it shipped (118f745a): `nest run` on a project restored
the image's root section and then **loaded every module from source anyway**. There is no error
and no wrong answer — the observable behaviour of an imaged start was identical to a cold one,
because it *was* a cold one. Only the benefit was missing.

**Two independent defects, either one sufficient to disable it.**

1. **The installer wrote a module-qualified global.** `project-install-image` ran
   `(def *image-sections* …)` from inside module `project`, binding
   `project/*image-sections*`, while `require-force` — root code — read the empty root global.
   The prelude documents this exact rule four lines above the global's own `def`, and ships
   `set-load-path!` as the root setter for the same reason; `set-image-source!` is now its
   sibling and the installer goes through it.
2. **`require-force` tested the branches in the wrong order.** The image branch sat after the
   ADR-070 package-module branch, and a project roots its OWN modules — so
   `*package-module-files*` holds `demo/shapes` exactly as it holds a dependency's `foo/b`, and
   the package branch matched first for every module of every named project. The image branch
   now comes first: an image is only installed after its fingerprint matched the sources that
   branch would re-evaluate, and a module with no section still falls through to it.

**Why the existing tests missed it.** `startup_image_test.blsp` and `image_test.blsp` drive the
`%image-*` primitives directly — they hand in a section list and read it back, never routing
through `require`. The defect lived entirely in the seam between the primitives and the loader,
which no unit of either side can see.

**How to tell it is working** (the only reliable signal): a `println` at a module's top level is
evaluated by a source load and absent from an imaged one. A second run of an unchanged project
must not print it, and `BROOD_IMAGE_TRACE=1` should show one `[image-section]` line per module
the entry point reaches, not just the root section.

**Guard:** `crates/nest/tests/startup_image.rs` — `a_second_run_loads_modules_from_the_image_not_from_source`
(plus `an_edited_source_file_invalidates_the_image`, so the fix cannot be "serve a stale image").

**Follow-up shipped the same day:** a dependency's modules are imaged too, with their files
added to the staleness key (they live outside `:source-paths`, so nothing else could invalidate
them) — see the ADR and `a_path_dependency_is_imaged_and_its_edits_invalidate`.

**Re-measured 2026-08-07, and the two halves of ADR-218 fared very differently.** Same fixture
(4 002 generated modules), same box, release binaries from `90099993` (pre-fix) and `34770be4`:

| row | pre-fix | post-fix | |
|---|---|---|---|
| lazy `nest run` (entry reaches 2 of N) | 0.37 s / 112 MB | 0.36 s / 102 MB | unchanged |
| eager materialise-all | 8.55 s / 674 MB | **1.34 s / 453 MB** | **6.4×**, −33% RSS |
| ⤷ materialise time alone | 7 012 ms | **959 ms** | **7.3×** |

So the **lazy row was never broken in a way a stopwatch could see**: an entry point reaching two
modules pays about the same to source-load two files as to materialise two sections, which is why
ADR-218's 1.30 s was a real number measuring the wrong mechanism (a 16 302-module re-run of that
row lands at 1.57 s / 243 MB against its 1.30 s / 219 MB, on a denser fixture). The row that was
genuinely broken is the **eager** one — `project-materialize-all` → `require-one` → the source
branch — so `nest test`, `nest check` and the LSP re-evaluated the whole project on every start.

**A trap worth keeping:** the eager path *reported* "materialised 4002 of 4003 sections" in both
arms. The count is sections walked, not sections served from the image, so the instrument looked
healthy throughout. Only the wall clock and a top-level `println` in a module told the truth.

---

## KI-33 — fully consuming a stream leaked its producer process · **fixed 2026-08-07**

**Found while fixing KI-32, 2026-08-07.** A `std/stream.blsp` stream is a process; an
exhausted one used to answer `:stream-done` *idempotently* by looping in `stream-done-loop`
— so once a stream was fully consumed, its producer process stayed **parked forever** instead
of exiting. `stream-empty`, `stream-singleton`, `stream-drop` (when it drops past the end),
`stream-chunk` (partial last chunk) and `stream-lines` (final buffer flush) all ended there.
Every *other* source (`stream-list-loop`, `stream-fn-loop`) and every transformer already
exited on exhaustion, so this was an inconsistency: some pipelines self-cleaned, some leaked
one (or more) processes per stream. Harmless for a one-shot, but a long-lived program that
builds many streams leaked processes without bound — surfaced as `memory limit exceeded`
under a stream-hammering repro.

**Fixed** by making `stream-done-loop` (and `stream-err-loop`) answer once and then **exit**,
matching every other loop. Safe because a well-behaved consumer stops at the first `:done`
(the idempotence was never relied on) and no transformer re-polls a done upstream — each
forwards `:done`/`:stream-err` and exits. Terminal consumers therefore need no `stream-close`
call, and a fully-consumed multi-stage pipeline tears itself down completely.

Regression test: `tests/stream_test.blsp`, "streams do not leak producer processes" — consumes
150 batches of the four leaking shapes and asserts no producers remain parked under this
process (a `process-info :parent`/`:status` count, cross-checked to report 5 for 5
deliberately-parked children). **Verified by sabotage** — restoring the recursion fails the
test 3/3.

---

## KI-32 — a selective `receive` corrupted a skipped local message to `nil` · **fixed 2026-08-06**

**Found by the green-tree flake battery, 2026-08-06** — the in-language suite's
`stream-drop › drop more than available` timed out at 600s once (masked by a nextest
retry, so the battery's own fail-count read 0). Not a stream bug: an amplified repro
(`stream-drop` pipelines hammered concurrently) reproduced an intermittent **hang** —
~20% at 200 pipelines, ~80% at 800, and **100% single-worker** (`BROOD_J=1`), which ruled
out a multi-core data race and pointed at a cooperative-scheduling logic bug. A gdb
snapshot of the wedge showed the root process blocked in `receive` while the worker slept
with an empty run queue; a watchdog dump named the culprit: one process parked over a
single queued message whose value had become **`nil`**.

The bug was in the selective-receive scan (`scan_mailbox`, `process/mailbox.rs`). A message
delivered by the **L1 fast path** — the sender copies it straight into a *parked* receiver's
heap, a `Payload::Local` whose value lives in a `msg_roots` slot — was read during the
optimistic single-lock scan with **`msg_root_take`**, which *tombstones the slot for reuse*.
On a **non-match**, the same envelope (still holding that slot index) was re-inserted into
the queue via `reinsert_candidate`; the next scan `msg_root_peek`-ed the now-empty slot and
read **`nil`** (or, worse, a later message that reused the slot). A `[:next pid]` request
sitting in a stream stage's mailbox while it waited for its upstream reply was thus corrupted
to `nil` and could never match again — the stage parked forever, and the whole pull-stream
pipeline deadlocked. Intermittent because it needed the message to arrive via L1 (receiver
parked) *and* be the optimistically-popped first candidate of a non-matching scan; worse
single-threaded because that interleaving is more likely; engine-independent because the
mailbox scan is shared by the VM and the tree-walker; unhelped by every scheduler/message
opt-out lever because none of them touch the take/reinsert.

**Fixed** by **peeking, not taking**, in the optimistic scan (so a re-queued candidate keeps
its slot intact) and freeing the slot (`msg_root_take`) only on the **consume** path, where
the message actually leaves the queue. This also closes a latent slot leak on the
non-optimistic match path, which peeked but never freed. Wire-format messages were always
immune (their value lives in the envelope, not a slot).

Regression test: `tests/concurrency_test.blsp`, "selective receive: a skipped local message
keeps its value" — a receiver is driven to `:waiting` (via `process-info`) so the send takes
the L1 path, then a non-matching `[:data 42]` is skipped and later matched; a companion
hammers the shape across 40 processes and checks the value round-trips. **Verified by
sabotage** — on the pre-fix binary the test hangs 5/5 (600s → timeout); on the fix it passes
0/10, and the amplified repro drops from 80%/100% hang to 0.

---

## KI-31 — a foreign-ecosystem version range silently compiled to its first term · **fixed 2026-08-06**

**Found by a bug hunt against `std/version.blsp`, 2026-08-06** — not by a failing test, and not
by the fuzzers: no generator covers the version module.

`version-satisfies?`/`version-compile` split a constraint into terms on **commas**. A range
written in npm/cargo style, with terms separated by *spaces*, therefore arrived as one term:
`version-split-constraint` peeled the leading `>=` and handed `"1.0.0 <2.0.0"` to
`version-core`, which is **deliberately lenient** ("a missing or non-numeric segment reads as
0" — the property that makes `"1.x"` and the short `">= 0.3"` work). It parsed that as
`(1 0 0 0 0)`, i.e. `>=1.0.0.0.0`. The upper bound vanished without a word.

The consequence is the dangerous direction for a package manager:

```
(version-satisfies? "3.0.0" ">=1.0.0 <2.0.0")   ; => true, before the fix
```

A user pinning a dependency to `>=1.0.0 <2.0.0` got `>=1.0.0`, so the resolver could select a
breaking major. It also failed the other way — `"^1.0.0 || ^2.0.0"` silently dropped the second
alternative, so `2.5.0` did *not* satisfy it — and `"1.0.0 - 2.0.0"` (npm hyphen) became
`=1.0.0` with a garbage prerelease of `(" 2" "0" "0")`.

**Fixed** by rejecting a version part containing whitespace or a constraint operator
(`< > = | ^ ~`) — the same rule `version-split-constraint` already applied to the *operator*
("an operator with no meaning has to say so"), extended to the version beside it. `-` and `+`
are deliberately not rejected: they are legal prerelease/build punctuation.

**The leniency it does not touch, on purpose.** `">= 1.0.0"` (space after the operator),
`">= 1.2"` (short), `"1.x"` and `"1.0.0+build"` all still compile — that leniency is why
`version-core` exists in its current form. One case stays lenient and is *not* fixed:
`">=1.0.0extra"` still reads as `>=1.0.0`, because a segment that begins with digits and
continues with letters is indistinguishable from the `"1.x"` form the module intends to
accept. Tightening that means deciding whether `"1.x"` should survive at all — a design
question, not a bug fix.

Regression test: `tests/version_test.blsp`, "a range from another ecosystem raises instead of
silently widening", plus a companion asserting the intended leniency survives. **Verified by
sabotage** — with the check disabled the first fails and the second does not, which is the only
version of that test worth having.

---

## KI-30 — seven `temp-dir` prefixes were never purged · **fixed 2026-08-05**

**Filed as its own entry deliberately.** Found while fixing KI-29, and KI-29's lesson is that an
adjacent finding recorded inside another entry is invisible. It is a *different* bug: litter on
disk, not a process burning CPU and holding a port.

**Measured.** 4622 `/tmp/brood-*` directories, **168 MB**, on an ordinary dev box.

**The mechanism already existed — the coverage did not.** My filed fix direction (a
`with-temp-dir` macro, and a decision about whether a failing test keeps its directory) was
wrong, because `purge-stale-temp` already implements the convention and documents it: name
fixtures with a unique prefix, and at file **load** drop the previous run's leftovers, which
bounds `/tmp` to one run and recovers from a crashed run. Nine prefixes did this. Seven did not,
and the census is a clean 1:1 with no inference required:

| prefix | dirs | purged? |
|---|---|---|
| `brood-feat-` | 1392 | ❌ |
| `brood-reload-` | 926 | ❌ |
| `brood-walk-` · `brood-skip-` · `brood-p2-` | 464 each | ❌ |
| `brood-ambient-` · `brood-ambient2-` | 387 each | ❌ |
| `brood-pkg-` | 42 | ✅ |
| `brood-file-`, `brood-slurpb-`, `brood-dup-`, `brood-manifest-`, `brood-fmt-*`, `brood-buf-` | ≤20 each | ✅ |

Every unpurged prefix is in the hundreds; every purged one is one run's worth. 4484 of the 4622
directories (97%) come from the seven missing lines.

**Fix.** The seven missing `(purge-stale-temp …)` calls, in `project_test.blsp` (4),
`reload_watch_test.blsp` (1) and `syntax_finalization_test.blsp` (2). Verified by running the
whole suite three times: **128 directories after each run** (110 of them the suite's, 3.7 MB),
flat, where before each run added ~110 permanently.

**Covered by** `tests/temp_purge_coverage_test.blsp`, which scans each test file's own source and
fails if a prefix passed to `temp-dir` is never passed to `purge-stale-temp` — reporting
`[file prefix]` pairs, so a failure names the file to edit and the line to add. A purge covers a
prefix it is a `starts-with?` prefix of, since that is how the purge itself matches
(`brood-file-` legitimately covers `brood-file-swap-`). **Verified by sabotage**: deleting one
purge line fails it with `(["reload_watch_test.blsp" "brood-reload-"] …)`. It carries a second
test asserting the scanner still finds ≥20 uses, so a renamed primitive or a reformat that breaks
the `(temp-dir "` spelling cannot make the first test pass vacuously forever.

**Why a source scan rather than a `/tmp` check:** a `/tmp` check could only fail *after* litter
exists, would depend on what earlier runs left behind, and would not reproduce on a clean machine.
The property is static and local — a file's own text either has the line or it does not.

**Not a correctness risk** — nothing reads a stale dir (each name is freshly randomised), and the
purge runs at load, before any fixture exists. It was disk, and noise when reading `/tmp`.

## KI-29 — the node/observe tests orphan `brood` children · **fixed 2026-08-05**

**Why this became its own entry.** It was written down as an "adjacent finding" inside KI-14, which
is *fixed* — so it read as closed, was counted as closed, and sat for nine days. A live bug filed
under a fixed one is invisible. (KI-30 is filed separately for exactly this reason.)

**Evidence, 2026-08-05.** Three stray `brood` nodes were alive on this box at once:

| child | age | CPU |
|---|---|---|
| `/tmp/brood-observe-*/target.blsp` | **9 d 14 h** | 4.0% |
| `/tmp/brood-dist-reconnect-*/b1.blsp` | 1 h 13 m | 15.3% |
| `/tmp/brood-dist-nodedown-*/quitter.blsp` | 32 m | 15.6% |

Together ~35% of a core, indefinitely, on a machine whose benchmark numbers are measured pinned to
one core. Each is also a **live node**, listening, with the same `secret-test-cookie-16+` and the
same `:a`/`:b` names every dist test uses — which is why it was KI-28's leading suspect.

**The cause: a test binary that is killed rather than allowed to finish** — nextest fail-fast, a
`timeout`, a `^C`. `std::process::Child` has no drop-kill, so every `brood` child the binary had
spawned is reparented and runs forever. Nothing fails when this happens, which is why it went
unnoticed: a leak leaves no red test behind.

**Fixed** in `crates/cli/tests/support/mod.rs` with `BroodChild`, a guard returned by
`spawn_brood`/`spawn_brood_env` in place of a bare `Child`. Two *independent* nets, because neither
covers the other's case:

1. **`Drop`** kills and reaps whatever is still running — for the test that panics between the
   spawn and its `kill`, or returns early past it.
2. **`PR_SET_PDEATHSIG(SIGKILL)`** in `pre_exec`, so the *kernel* kills the child when the thread
   that spawned it terminates, however it terminates. This is the net that fixes the filed bug: a
   SIGKILLed test binary runs no destructors, so net 1 is worth nothing there. It closes its own
   race too — if the parent died in the window between the fork and the `prctl`, the signal was
   already missed, so the child compares `getppid()` against the pid captured before the fork and
   `_exit`s if it changed.

Net 2 is exposed on its own as `dies_with_parent(&mut Command)` and applied to the four test files
that merely `.output()` a one-shot `brood`/bundled-app run as well (`error_format_parity`,
`checker_cross_module_ability`, `std_attribution`, `release_bundle`). Those cannot orphan on the
happy path — `output()` blocks until the child exits — but a program that *hangs* while the binary
is killed is the same leak, and this bug's lesson is that nothing reports it. The invariant is now
uniform: every child a `cli` test starts dies with the test.

**Two things the original entry got wrong**, both worth keeping:

- **Cause 2 — "the observe/attach path leaks even when its test completes normally" — has no
  mechanism, and is withdrawn.** It was inferred from the 9-day-old child being "from an ordinary
  run", which was itself inferred from its age; age is not provenance. Read the test: all three
  harnesses kill the target *before* asserting, so a run that completes cannot leak, and the only
  non-killing exits are panics — i.e. cause 1 again. Verified: `cargo nextest run -p cli` is 46/46
  with zero strays. (The fix covers this either way — PDEATHSIG does not care why.)
- **The filed fix direction, a process group, is the wrong lever for cause 1** and was not taken.
  Moving the child into its *own* group **removes** it from any group an outer tool might kill, and
  nothing runs our group-kill when we are the one being SIGKILLed. A group buys only grandchildren,
  which no test here creates (these programs are green processes, no `run-process`), and it costs a
  pid-recycling hazard: `killpg` on a reaped leader's recycled pgid can signal an unrelated group.
  `Child::kill` cannot — std caches the exit status, so a kill after a `wait` is a no-op rather
  than a signal to a stranger's pid.

**Covered by** `crates/cli/tests/child_cleanup.rs`. "Nothing leaked" is not observable from inside
a passing test, so each test drives **one** net with the other defeated and asserts the child
actually died: the `Drop` test spawns on the test's own thread (where the parent-death signal
cannot have fired yet), and the PDEATHSIG test `mem::forget`s the guard so no destructor runs.
Both were **verified by sabotage** — breaking each mechanism fails its own test at the 10 s
deadline and leaves the other passing, and the PDEATHSIG sabotage reproduced the original leak
live (a child outliving its dead test binary).

**Also verified against the filed scenario itself**, since the unit tests stand in for it rather
than perform it: `cargo nextest run -p cli --test distribution` was started, a test binary holding a
live `brood` child was **SIGKILLed** mid-run, and three seconds later `pgrep -af 'brood /tmp/brood-'`
was empty. Do this with a **bracketed** pattern (`'deps/[d]istribution-'`) — the unbracketed form
matches the shell running it, which is the trap `CLAUDE.md` records, and it cost a run here by
SIGKILLing the very shell doing the killing.

**Check for a recurrence with** `pgrep -af 'brood /tmp/brood-'` — still the right instrument, since
the class of bug is one that no test failure reports.

## KI-28 — a single unexplained `nodedown` flake · **superseded by [KI-38](#ki-38) 2026-08-07**

> **Recurred twice (2026-08-06, 2026-08-07) and is now part of KI-38.** The recurrence this entry
> asked for arrived, and it answered this entry's own question: both sightings panic in
> `wait_until_listening` — **B never bound its port** — so this is a *boot* failure, and the
> framing below ("a dist flake") is the wrong altitude. Both times it failed alongside two
> `cli::child_cleanup` tests that are not dist tests at all. Read KI-38 first; everything below is
> kept because its 0/260 hunt and its ruling-out of KI-27/KI-29 remain valid and are load-bearing
> for KI-38.


**Not a diagnosis — a record, so a second occurrence is recognised as the second.** In the full
`make test` that verified the KI-27 fix, `cli::distribution clean_peer_exit_fires_nodedown_promptly`
failed try 1 and passed try 2 (nextest reported `FLAKY 2/2 [0.631s]`).

**How long the failure took is unknown.** That 0.631 s is the duration of the *passing retry* —
nextest reports the final attempt. An earlier version of this entry read it as the failure's
duration and concluded node A had exited early rather than hitting its 5 s `[:nodedown]` guard;
that inference had nothing under it and is withdrawn. Both shapes are still open: a failed
`connect`, or a nodedown that genuinely did not arrive within 5 s under load.

**What the failure text said: nothing, and that is my fault.** I had piped that run through
`tail -25`, which discarded the assertion output. The test now prints **B's stderr** alongside A's
on failure, so the next occurrence explains itself.

**What is known.** 0/40 solo; 33/33 × 3 for the whole dist file under nextest (the config that
failed); and it did **not** recur in the next full `make test` (956/956, no flaky). It is *not* the
KI-27 mechanism — under nextest this test's process asks for two ports and cannot wrap its slice,
and concurrent test processes have near-consecutive pids and therefore disjoint slices.

**Its one live suspect is now gone, which matters for reading a recurrence.** Orphaned test children
(KI-29) were real and confirmed 2026-08-05: three stray `brood` nodes alive at once, one for **9
days**, together burning ~35% of a core. A leaked node still *listening* on a port, with the same
`secret-test-cookie-16+` and the same `:a`/`:b` names every dist test uses, is exactly the kind of
thing that makes a node test fail once in a hundred runs. **KI-29 is fixed as of 2026-08-05**, so a
run since then starts from a box with no stray nodes on it — meaning a recurrence *after* that date
can no longer be explained away by the leak, and is a stronger signal than the original sighting.
Still worth confirming with `pgrep -af 'brood /tmp/brood-'` before chasing anything subtler.

**If it recurs**, the printed B stderr should immediately separate "B never bound its port" from
"A could not connect to a listening B"; a `connect` failure would then be a real runtime question
rather than a harness one.

**Dedicated hunt 2026-08-07: 0 failures in 260 runs, retries OFF.** With no stray `brood` nodes
on the box (confirmed `pgrep -af 'brood /tmp/brood-'` clean), the whole dist file ran 60× under
nextest (`--retries 0 --no-fail-fast`, the exact config that flaked) and the named test ran 200×
alone — zero flakes, zero aborts. That is now the standing record: KI-28 **genuinely does not
reproduce on demand**, so it stays a watch item rather than an open bug, with the B-stderr
diagnostic armed. A recurrence after this date — on a box with the KI-29 leak fixed — is a real
signal worth chasing; until then there is nothing under it to fix.

## KI-27 — node tests drew their port from the OS ephemeral range · **fixed 2026-08-05**

**Symptom.** In a full `make test`, `cli::distribution reconnect_watcher_heals_a_fallen_link`
failed twice (nextest retried once) after **20.06 s** — its `(after 20000 …)` guard on
`[:nodeup]`, so the watcher never saw node B come back. Fast and reliable alone: **7/7 solo** at
~1.3 s, **16/16** as 16 concurrent copies of the same binary, and 3/3 for the whole dist binary
even under 12 CPU hogs. Only a full suite reproduced it.

**Cause.** `free_port()` in the three node-test harnesses picked a port with
`TcpListener::bind("127.0.0.1:0")` and dropped the listener. That asks the kernel for a port from
the **ephemeral range** — 32768–60999 here (`/proc/sys/net/ipv4/ip_local_port_range`) — which is
the same range every outbound connection on the box is assigned from, and then releases it. So a
port a test node is about to bind can be handed to an unrelated process's *client* socket in the
gap, and the node's bind fails with `EADDRINUSE`.

Measured, not inferred: of 4000 client sockets opened by an unrelated process, **4000 (100%)**
landed on a port the old `free_port()` could have handed a node, and **0** in the band the fix
uses. `reconnect_watcher_heals_a_fallen_link` is the most exposed test in the file because it needs
one port to stay free across B1's whole life, B1's exit, the 400 ms gap *and* B2's rebind; every
other test needs a single bind. And the pre-existing mitigation — a `PORTS` mutex around
bind→spawn — is per-process, so under `cargo-nextest` (one process per test, which is what
`make test` uses) it does not exist between the tests that actually run concurrently.

**Fix.** `crates/cli/tests/support/mod.rs` (new — the three harnesses had their own copies of these
helpers, which is how a fix in one file could miss two). `free_port()` now allocates from a fixed
band **below** the ephemeral floor (12000..32768), where the kernel never auto-assigns, sliced by
pid so concurrent test processes start in different places, and probes bindability before
returning. Additionally `reconnect_watcher_heals_a_fallen_link` now calls `wait_until_listening`
on B2 instead of assuming it came up, and prints B2's stderr on failure — the test previously
could not distinguish "the watcher failed to heal" (the thing under test) from "B never came
back", which is why this presented as a mysterious 20 s timeout. Its `[:nodeup]`/pong deadlines
were raised (60 s / 30 s): they are liveness ceilings on a saturated machine, not latency
assertions, and cost nothing when healthy (the test still runs in 1.3 s).

**Ruled out along the way.** `SO_REUSEADDR` is absent from all three `TcpListener::bind` sites
(`net.rs:1208`, `net.rs:1528`, `dist.rs:859`) — the obvious suspect for a failed rebind, and
**tested directly: not the cause** (immediate rebind after a linked peer exits succeeds 3/3).
That is consistent with the real cause: the port was lost to another process, not to `TIME_WAIT`.

## KI-25 — five JIT/VM suites cannot be re-run in one image · **fixed 2026-08-04**

**Symptom.** Five suites pass on the first run and fail on the second *within the same
image* — that is, under `--repeat-until-failure`, at every seed tried (0 and 5), which rules
out ordering:

| suite | shape |
|---|---|
| `jit_self_rebind_test` | 1 of 2 fails on iteration 2 |
| `jit_shared_spawn_test` | 1 of 4 |
| `pid_identity_test` | its single test — and it is ALREADY `:isolated` |
| `vm_call_head_order_test` | 2 of 3 |
| `vm_selfcall_reload_test` | 1 of 3 |

Found by a sweep of every suite in both repos (3 iterations × 2 seeds). `bedit` came
back completely clean; these five were the only hits, and they reproduce on a working tree
with the day's changes stashed, so they predate them.

**Why it matters more than "who re-runs a suite twice".** `--repeat-until-failure` is *the*
tool for finding flakes, and `nest test --failed` re-runs in one image too. While these
five fail on a second iteration, the tool cannot be used across brood's suite — a real flake
somewhere else is invisible behind them. (That is how it was found: hunting flakes elsewhere.)

**What is known.** These are hot-reload / self-call / tiering tests: they redefine globals
and assert on how the VM or JIT re-links afterwards, so the natural theory is "iteration 2
starts from iteration 1's redefined state". That explains four of them, and `:isolated`
(which runs a unit alone against the clean post-load baseline and rolls back its `def`s) is
the ordinary fix.

**`pid_identity_test` is explained too, and NOT by a JIT cache** (correction 2026-08-04 — the
original entry guessed "a JIT tier election, an IC block, a pid counter, or an arm-keyed cache"
and reasoned from there that the case was informative about JIT internals; it is not). Running
the recipe below prints the cause outright:

```
uncaught error: … :message node-start: this runtime is already a node (node-start called twice)
```

The test calls `node-start`, which is **deliberately one-shot per runtime** (`dist.rs`: a second
listener would need a second port, so the second call is an error by design). `:isolated` rolls
back `def`s; it cannot roll back the runtime's node state, and no test-level isolation can. So
this is not "something survives an isolated re-run" in the mysterious sense — it is a test whose
precondition (`(node-name)` is `:nonode`) is consumed by running it once.

**Fixed 2026-08-04, and it was small once the premise was right.** Two different fixes, as the
diagnosis implies:

- **The four rebinding suites** (`jit_self_rebind`, `jit_shared_spawn`, `vm_call_head_order`,
  `vm_selfcall_reload`) are marked **`:isolated`**, so `%isolate` rolls their `def`s back to the
  post-load baseline and iteration 2 starts where iteration 1 did. `jit_shared_spawn` moved from
  `:serial` to `:isolated` — it already ran alone; what it lacked was the rollback.
- **`pid_identity_test`** now calls `node-start` only when `(node-name)` is `:nonode`. Nothing at
  the test level can undo a started node, so the re-run keeps the equality / hashing /
  receive-pattern assertions and skips re-taking the one-shot transition. Every ordinary
  `nest test` is a fresh image, so the real nonode→node case still runs there.

**Verified**: each of the five clean over 3 iterations at seeds 0 and 5, and — the point of the
issue — the **whole in-language suite now survives a re-run in one image**: `nest test
--repeat-until-failure 2` gives 4390/4390 twice. The failure detector was checked against the
pre-fix file first, so a green result means something. Reproduce in one command:

```bash
nest test tests/pid_identity_test.blsp --repeat-until-failure 3 --seed 0
```

## KI-24 — an `eval`'d definition cannot forward-reference another `eval`'d name · **fixed 2026-08-01**

**Symptom.** Two definitions made by `eval` from inside a namespace, where the first
references the second:

```
(defmodule m)
(eval '(defn r (n) (s n)))
(eval '(defn s (n) (+ n 1)))
(r 41)          ; → unbound symbol: s
```

The hint said it all: ``s` is defined as `m/s``. The *definition* side qualified
correctly; only the bare *reference* failed to.

**The REPL had it too** — it is simply typing a `defmodule` and then two mutually
recursive `defn`s, since each REPL input is its own compile unit:

```
(defmodule rtest)
(defn ra (n) (rb n))
(defn rb (n) (+ n 1))
(ra 41)          ; → unbound symbol: rb
```

**Order-dependent; ordinary code unaffected** — `scripts/fuzz/stress/eval_forward_ref.blsp`:

| shape | before | after |
|---|---|---|
| normal mutual recursion (forward ref) | ✅ `:ok` | ✅ |
| eval'd, target defined **first** | ✅ 42 | ✅ |
| eval'd, target defined **second** | ❌ `unbound symbol: s` | ✅ 42 |

**Confirmed a regression** rather than long-standing: rebuilding `builtins/system.rs` at
`97d63eda^` returned 42 for the failing row.

**Cause.** `97d63eda` (a good change — it took `eval` off the ~14× tree-walker) swapped
`macroexpand_all` for `macros::compile`, which adds the **resolve** pass. Resolve qualifies
a bare name only on *positive evidence* that the namespace owns it: the name is already a
`ns/name` global, or it is in `ns_known_names` — the def heads a **file loader pre-scans
before compiling any form**. That pre-scan is what makes a forward reference work inside a
file. `eval` and the inheriting `eval-string` (REPL, inline) compile **one form at a time**
and cannot scan the future, so the reference was left bare. A survey of every
`macros::compile` call site confirmed those two were the only ones without a pre-scan.

Note the mirror image: *before* the resolve pass, an eval'd `defn` inside a module defined
a bare **root** global — module code leaking into the global namespace. The old behaviour
"worked" only because reference and definition were consistently wrong.

**Fix.** `Heap::set_ns_assume_own` — a compile-context flag, alongside `compile_ns` /
`ns_known_names`, set by the two call sites that have no pre-scan behind them. With it on,
the resolver's *last resort* flips: a bare name bound at root/prelude still falls through
(so `+`/`map`/`count` keep working) and a `(:use …)` import still wins (checked first), but
a name bound **nowhere** is taken to be this namespace's — the conclusion the file pre-scan
would have reached. Deliberately not the resolver's default: over-qualifying a name that is
really a local is a *silent* miscompile, so the general rule stays evidence-only.

**The one trap, caught by the regression test.** Special forms and core-macro keywords —
`if`, `let`, `do`, `fn`, `cond` — are bound in **no** environment, so "unbound ⇒ ours"
rewrote `if` to `m/if` and every eval'd conditional died. `is_syntax_keyword` excludes
`builtins::SPECIAL_FORMS`. (Macro *calls* are already gone by resolve time —
`macroexpand_all` runs first — but a special-form head survives expansion by definition.)

**Covered by** `tests/eval_vm_test.blsp` ("eval resolves names against the enclosing
namespace"): the forward reference, mutual recursion across two evals in O(1) stack, the
definition landing in the namespace rather than root, and one guard test each for syntax
keywords, root/prelude names, and imports. Those evals run at **load** time on purpose —
`compile_ns` is only set while a file loads, so an `eval` inside a `test` body is at root
and never exercises the resolver at all.

**Two fixes were written for this, independently and on the same day.** The other one
(`13706580`) reverts `eval_builtin` to `macroexpand_all`, dropping the resolve pass rather
than feeding it the missing evidence. It fixes the reproducer, and its reasoning about the
pre-scan is the same as above. It was measured against this one and not kept, because
dropping resolve drops everything else resolve does for an eval'd form:

| | revert to `macroexpand_all` | `compile` + `ns_assume_own` |
|---|---|---|
| `eval_forward_ref.blsp` | ✅ `:ok` / 42 / 42 | ✅ `:ok` / 42 / 42 |
| REPL: `defmodule` + two mutually recursive `defn`s | ❌ `unbound symbol: rb` | ✅ 42 |
| eval'd `defn` in a module defines `mod/name` | ❌ leaks a bare **root** global | ✅ |
| `(:use …)` imports, `(:alias …)`, privacy, static-QQ in eval'd code | ❌ lost | ✅ |

The REPL row is the load-bearing one, and it is why that commit's "`eval-string` was never
affected" does not hold: each REPL input is its own compile unit, so `eval_string_inner`'s
*inheriting* path (`reset_ns = false`) has no pre-scan either — its own comment says it
"does neither" — and touching only `eval_builtin` leaves the REPL broken. Verified by
building both and running the same two inputs.

## KI-26 — a fast-link deopt guard re-read `inline_installed`, and its fallthrough re-ran · **fixed 2026-08-04**

**Severity: unknown — structurally reachable, never observed.** Recorded because the root cause
is the same anti-pattern behind two real ADR-210 bugs, and because the failure mode would be a
silently *repeated effect* rather than a crash.

**The shape.** In `jit_run_fast_link` (`eval/compile/jit_runtime.rs`), a deopt takes the
resume path only if `arm.active_nslots() == nslots`, where `nslots` is the frame size recorded
in the fast-link table and `active_nslots()` **re-reads `inline_installed`**. When the guard
declines, control falls through to `vm_apply` — a re-run from ip 0, which repeats any effect the
native had already journaled (a completed non-tail call, or a `table-put`).

**Why it looks reachable.** The inline swap in `jit_tier` deliberately does **not** bump the
global epoch — its own comment explains that a bump cascaded under `pfib` and cost ~2× — and it
invalidates only the *installing* process's fast links. Arms with a `share_key` are shared
across processes via `shared_closure_lookup`, so a peer can hold a link whose recorded `nslots`
predates the swap while `inline_installed` now reads true. Guard declines → re-run → duplicate.

**Why it is not fixed.** It could not be demonstrated. A purpose-built detector — fire whenever
the guard declines *while `jit_ckpt_resume` reports a live journal* — stayed silent across:

- the 4350-test in-language suite,
- `tests/jit_effect_once_test.blsp` (all six cases),
- `pfib` ×3, the parallel shared-arm workload the no-epoch-bump comment is about,
- a targeted 24-process race: one shared arm with a `table-put`, a spliceable leaf (so it gets
  an inlined upgrade), and an i64 overflow forcing a deopt on *every* call. All 24 effect
  counts came out exactly 4000, five runs in a row.

Most likely the per-call epoch check on the fast link re-validates the entry before the mismatch
can be observed. Not proven either way.

**Fixed** by extracting the check as a flag-free predicate, `jit_frame_shape_matches(arm,
frame_nslots)` = `frame_nslots == arm.nslots || frame_nslots == arm.inline_nslots`. That is a
strict superset of the flag form (`active_nslots()` returns exactly one of those two), so it only
*admits* more resumes — the effect-preserving direction — and every admitted resume is still
validated by `jit_ckpt_resume` (positive journal, in-bounds slot). A genuinely foreign arm is
still refused, which is the out-of-bounds protection the check exists for.

**How it was verified, given the runtime detector never fired.** The race could not be won, so
the behaviour difference was pinned at the unit level instead — which turned out to be the better
tool anyway, because it is deterministic. `ki26_frame_shape_check_is_independent_of_inline_installed`
asserts that for an arm with layouts (6, 9), *both* frames stay resumable in *both* flag states,
and that a foreign frame is refused in both; it **fails against the old flag form** (checked by
temporarily restoring it: "the small frame must stay resumable with inline_installed=true") and
passes with the fix. `ki26_shape_check_admits_everything_the_flag_form_did` sweeps four layout
pairs — including the degenerate `(4, 4)` an unjournalled derivation produces — × both flag
states × 40 frame sizes, asserting the new form never refuses what the flag form accepted, so the
fix cannot have traded one silent wrong-resume for another.

Gate re-run: 40/40 `tests/jit.rs` under GC_STRESS+GC_VERIFY, 946/946 Rust, 4350/4350 in-language
with `BROOD_NO_PARTIAL_LEAF` both ways, 4 fuzz generators × 4 engine configs, `--no-default-features`
clean.

**The lesson worth keeping.** The runtime detector was the right instinct and the wrong
instrument: it can only fire if you win a race. When a hazard is a *predicate* rather than a
timing window, extract the predicate and test it directly — a fix for a bug you cannot make fire
is a fix you cannot verify, but "make it fire" does not have to mean "reproduce it end to end."

## KI-23 — the KI-22 lost update also exists in std-module registries · **fixed 2026-08-02**

KI-22 fixed the **prelude's** registries by routing them through `%registry-update!`. The
same `(def *X* (assoc *X* …))` read-modify-write survived in registries that live in `std/`
modules. Measured directly, at 200 concurrent writers into one map: the plain `def` rebind
keeps **~125 of 402** entries (three runs: 128 / 122 / 125). The rest are silently gone.

It was already biting: `tests/repl_test.blsp`'s "re-registering a name replaces rather than
duplicates" failed **2 runs in 5** — two concurrent registrations each filtered the old list
and each appended, so the duplicate the API forbids survived. It is **32/32** now.

**The mechanism: compare-and-swap, not more ops.** `%registry-update!` names each shape it
supports as a kernel op (`:assoc`, `:cons-new`, …), which fits a registry whose update is one
map/list operation. Several of these are not like that — `face-set` merges into the *existing*
entry, `attach` strips an id from every bucket before consing onto one,
`register-repl-command` filters by name overlap and appends, `register-system-layer` is
append-if-absent. A Rust op per shape would have pushed policy into the kernel. Instead:

- `%registry-cas!` (`Heap::registry_cas`) — rebinds the global only if its current value still
  equals the expected one, under the same registry lock.
- `registry-swap!` / the `swap-registry!` macro (**prelude, Brood**) — retry around it, so the
  transform is an ordinary Brood function and only the read-decide-write is indivisible.

**The `defdyn` caveat dissolved.** The original note worried that `%registry-update!` writes at
`env_root(env)` and would bypass an active `binding`. Reading the evaluator settles it: `def`
computes `let root = heap.env_root(env)` and writes *there* too, and both spellings read
through the chain. The lock matches `def` exactly, so converting a `defdyn` registry (`*faces*`)
is semantics-preserving — an active `binding` shadows the root write identically either way.

**Converted** (13 sites): `debug/*traced-fns*`, `protocol/*protocols*`,
`telemetry/*telemetry-handlers*` (attach, detach, and the auto-detach of handlers that threw),
`telemetry/*telemetry-events*`, `*faces*` (`def-face` + `face-set`),
`editor/layers/*system-layers*`, `editor/layers/*type-layers*`,
`editor/layers/*auto-type-by-file*`, `repl/*repl-commands*`.

**Deliberately not converted: `std/tool/test.blsp`'s `*units*` / `*collected*` /
`*collecting*` — they are not racy.** Registration happens at *load* time and test files load
**sequentially**, each inside its own `%isolate` (`test.blsp`: "Files run sequentially (each
`%isolate` blocks to …)"). There is no concurrent writer to lose an update to. `*collecting*`
/`*collected*` are also not registries but a sequential accumulator protocol — `describe` sets
a flag, the body conses, the flag clears — which a CAS would not make correct anyway.

**Covered by** `tests/registry_test.blsp`: the mechanism with its own **control** (a plain
`def` rebind in the same test, asserted to lose entries, so a regression cannot hide behind a
test that would pass with the bug), a concurrent counter (proving each retry recomputes
against the value that won), and one concurrent-registration test per converted registry —
including the repl duplicate-name shape and `register-system-layer`'s idempotence.

## KI-22 — concurrent registration silently lost registrations · **found + fixed 2026-08-01**

**Symptom.** Intermittent failure of the whole in-language suite, ~1 run in 5, in three
different tests: `ability_test`'s "open extension" (`(esize [1 2 3])` returning `-1`, the
`:default` impl, right after its `:vector` impl was registered on the line above),
`modules_test`'s "provide records a feature idempotently" (its own feature missing from
`*features*` on the next line), and `private_test` (a module registered by a sibling test not
found). Because nextest retries, `make test` printed `929 passed` with only a `FLAKY` marker.

**Root cause: a lost update.** Every load-time registry is one global holding a whole map or
list, written as `(def *X* (assoc *X* …))` — read, compute, write, three separate steps. Two
processes registering at once each read the old value and each write their own successor; the
later write silently drops the earlier one. `scripts/fuzz/stress/registry_race.blsp` (N
processes, one **private** ability each, so nothing is a legitimate precedence contest)
measured **24/50, 88/200, 218/500 lost — about 40%.** Not a dispatch-cache bug; the cache was
never involved. **Fifteen registries** share the shape, so multimethod registration had the
identical bug to ability registration, unhit until now.

**Fix: `%registry-update!`** — one kernel primitive that performs the whole read-modify-write
inside a single call, under a per-runtime `registry_lock`. Atomic by construction: no CAS (so
no ABA question), no retry loop, no spinning, and no callback into Brood while a lock is held.
Four ops cover all fifteen sites — `:assoc`, `:assoc-new` (presence test *inside* the lock, so
a derived method mirror cannot clobber an authored one registered in between), `:dissoc`,
`:cons-new` (the `member?` test inside the lock, for `provide`). Policy stays in Brood; only
the atomicity is kernel. Reads are completely untouched, which matters: dispatch reads
`*impls*` on every call, and the registries cannot become `Table`s because a table deep-clones
values in and out, putting a closure copy on the hot path.

**Two earlier attempts, both reverted** — see the devlog. Optimistic retry cut the loss 44% →
20% but cannot close the read-write window. An in-Brood ticket lock on `table-incr` reached
`LOST=0` and then made `make test` *worse* than the bug: a bounded busy-wait burnt 157 s of CPU
and still lost one under load, and adding `sleep` exposed that a timed-out waiter never bumps
`:served`, desynchronising the sequence permanently.

**One trap worth recording**, because it cost a debugging cycle: `def` binds at
`env_root(env)`, which is **not** always `EnvId::GLOBAL`. During prelude load the root is a
bootstrap env whose bindings later *seed* the shared runtime, so the first version of the
primitive — which wrote straight to the globals table — had its writes silently discarded at
seed time, and the prelude lost its own `Display`/`Inspect` impls (`to-str` then failed with
"no impl for :string"). A kernel primitive that stands in for `def` must read and write the
same place `def` does.

**Verified:** the probe goes 218/500 lost → **0 lost** at 50/200/500/1000, in 0.1 s; prelude
registries are back to their pre-change counts; regression tests for concurrent `impl` and
concurrent `provide`; suite green.

## KI-20 — a JIT fast link ran the callee against the **caller's** IC block · **fixed 2026-07-30**

`jit_run_fast_link` set `jit_call_env`, `jit_dbg_fn`, `jit_native_depth` and the stack limit
before entering the callee's native code — but not `ic_bases`, which the cloning native-link
path in `jit_dispatch_call` *does* set. `FastLink` (the IR-readable fast-path mirror) carried
no callee block, unlike `CallIcEntry`. So callee B ran with arm A's `cur_ic_base`/`cur_gic_base`,
and B's `vm_call_ic_put` / `vm_global_ic_put` / `vm_fast_link_publish_native` wrote into A's
slots, and vice versa.

**Never a wrong answer.** Every read path re-validates `(sym, argc, epoch)`, so a crossed
entry simply misses. The cost was that both arms ran with a permanently cold cache; it also
made `dbg_site_loc` and `[jit-staged-stale]` report the wrong source position.

**The fix (the one the earlier reverted attempt spelled out).** The callee's block bases now
**ride in the `FastLink` slot** — `_pad` became `callee_ic_base`, and a `callee_gic_base` was
added (the struct grows one 8-byte slot). `vm_call_ic_fast_link` stamps them from the entry's
already-resolved `CallIcEntry::callee_bases` (no `vm_arm_block` call on the publish path, no
lookup on the memoised hot path), the IR loads them alongside `code`/`nslots`/`env` (two extra
`u32` loads from the same cache line) and passes them as two more args to `brood_rt_fast_frame`,
and `jit_run_fast_link` does `set_ic_bases(callee_bases)` around the native call and restores —
exactly as the cloning path already did. So the runtime callback **never re-reads the table**:
the whole install is two `Cell` writes off values it was handed. A native flat cell carries
`(0, 0)` (a builtin runs no IC-using arm).

**Why the first attempt regressed and this one doesn't.** The reverted version read the bases
inside `jit_dispatch_fast_frame` via a `RefCell` borrow + a bounds-checked index **per call** —
on the very path whose purpose is to skip the IC probe, and dominated by self-recursion where
the install is a no-op — costing `bintree` +5.5% for no gain. Riding the bases in the slot adds
no lookup: a pinned best-of-9 A/B (`fib`, `bintree`) measured **+0.0%** against a +0.0%
base-vs-base floor.

**Guard.** A debug cross-check in `jit_dispatch_fast_frame` asserts the IR-passed bases equal
what the authoritative IC resolves (`b == callee_bases`), so a future mirror desync trips in
debug across the whole suite. The correctness net is the full differential + jit suites (the
change can only make a callee read its *own* caches, never produce a wrong value).

---

## KI-19 — the VM resolved a call's free-global head **after** its arguments · **FIXED 2026-07-30**

**Fixed.** The tree-walker evaluates the operator first; the VM elided a free-global head
and resolved it through the call-site IC *after* the arguments ran, so an argument that
rebound the head made the engines disagree — `(f (bump))` gave `:new` on the VM and `:old`
on the tree-walker.

`Inst::Call` now carries a `staged` flag. The compiler stages the head (as a `GlobalIc`,
resolved at IC speed) ahead of the args for exactly the calls that can be affected: a
**rebindable** global head with at least one argument that can run user code. `head` and
`site` stay populated, so the call-site IC still caches the resolved **arm** — validated by
closure identity against the staged callee, one compare on the common path. The JIT treats
a staged head as a computed callee (`head: None`, `site: NO_SITE`) so it calls the staged
value rather than re-resolving.

Three things had to be got right, and each was learned by measuring a wrong version:

* **Don't demote to the plain computed-callee path.** What the elided head really buys is
  the IC's cached *arm*, not the callee lookup: dropping it cost `json` 168 → 1159 ms (6×).
* **`head: None` with a live `site` aborts the JIT.** `emit` decides elided-vs-staged from
  the callee node while `jit_lower` decides it from `(head, site)`; a mismatch made the JIT
  resolve a head that wasn't there — `json`/`bintree`/`nqueens`/`wordcount` all died in
  `brood_rt_call_slow` → `unbound_error` → non-unwinding panic.
* **Exempt reserved names.** `def` refuses a name the language ships (ADR-166), so its
  resolution cannot change mid-call and the elided head stays correct. Staging every call
  regressed `regex` 31%, `wordcount` 11% and `sieve` 9% — almost entirely `first`/`rest`/
  `str`-class calls that were never rebindable.

Rows flat on an idle machine: `json` −3.6%, `bintree` −2.0%, `fib` +0.4%, `nbody` +0.1%,
and the three largest movers at baseline on a solo re-run (`regex` 95.7 ms, `wordcount`
50.6, `sieve` 49.7). `Inst::Call`'s stale doc comment claiming eval order was unchanged is
gone with the change that made it false.

Regression test: `tests/vm_call_head_order_test.blsp`.

---

## KI-18 — a deopt could re-run a `table-put` · **FIXED 2026-07-30**

**Fixed.** Four distinct paths let a JIT deopt re-execute an effect; the first three were
closed earlier the same day (no journal for a call-free effectful arm, a multi-arity
self-call exemption that ignored argc, and the leaf inliner splicing an effectful callee
into an engine that cannot journal). Those made the corruption unbounded and proportional
to the workload — 200 000 iterations put 402 047 times.

The fourth was the residual recorded here, and it is now fixed too: **`jit_run_fast_link`
treated an IC re-probe miss as "give up and let the caller redo the call"**. By then the
callee's native code had already run, so the caller's `brood_rt_call_slow` executed it a
second time — effect included. It showed up as a bounded over-count of exactly 16 (the
`DEOPT_BAIL_CONSECUTIVE` threshold, after which the arm bails and the duplication stops),
independent of iteration count. The IC probe is only an *optimisation* for locating the
arm, so a miss must not change behaviour: it now resolves the arm by name
(`env_get` → `compiled_arm_for`) and takes the same checkpoint-resume path as a hit.

Found by elimination, after instrumenting every re-run path: the deopt paths all resumed
correctly at their journal, `[fl-fallthrough]` was the one that fired 16 times, and
counting entries to *each arm separately* showed the caller was re-entered too — which is
what pointed above `f` rather than inside it.

Also hardened while here: the checkpoint resume now covers **preempt** (outcome 2), not
just deopt. A preempt normally lands on a back edge where the journal is 0, so this is a
no-op there; but if one ever landed after a completed call or a `table-put`, the ip-0
re-run would have repeated it.

Regression tests: `tests/jit_effect_once_test.blsp`, four shapes including the multi-arity
one, each asserting the effect count equals the iteration count exactly.

---

## KI-17 — `nest check` validated qualified names against ITS load set, not per-file reachability · **FIXED 2026-07-30**

**Fixed (ADR-189).** `check-file` now takes a per-file **reachability set** — the
file's *transitive* require-closure — and flags a **user-written** qualified
reference `mod/name` whose `mod` is outside it (bound in the image only by load-order
luck). `std/tool/project.blsp` builds the module→direct-requires graph once
(`%module-direct-requires`, one native parse per file), closes it transitively per
file, and threads each file's set through the fresh / cached / structured check paths
(carried as data in the parallel chunks). Soundness — the checker's cardinal rule —
held zero false positives across the whole `std/` + `tests/` sweep: the transitive
closure clears legitimately-transitive references (a test that `(:use editor/treesit)`
naming `face/…`, since treesit requires `editor/face`), the guard restricts the lint
to references the user *literally wrote* (never a macro-injected one), and a genuine
circular case (`coverage` naming `project/…`, which can't `(require 'project)` without
a cycle) is resolved by a *lazy* runtime `(require 'project)` in the one function that
uses it. Regression tests: `ki17_flags_a_qualified_reference_to_an_unrequired_module`
(Rust) and the end-to-end `nest check` on a project with a bad reference. Suppress a
deliberate exception with `(check-allow :unrequired form…)`. Original report:

**Symptom.** A qualified call to a module the running program never loads —
`(path/basename f)` in a file whose module neither `(:use path)` nor
`(require 'path)` — passed `nest check` clean, then raised
`unbound symbol: path/basename` at runtime. Hit twice in one day downstream
(myedit: `path/basename`, then a cross-module form value with the same shape).

**Cause.** `check-project` loads the whole project image (sources + test files)
before checking; any file's `require` binds the qualified name image-wide, so the
checker resolved it for EVERY file — including files whose own load graph never
pulls that module in. The check answered "bound in the check image", not "bound when
this module is reachable".

**Workaround (pre-fix).** Discipline: every qualified `mod/…` call needs a
`(require 'mod)` in that file (a bare require avoids `(:use …)` import shadowing,
e.g. std path's `join` vs the prelude's) — which the lint now enforces.

## KI-15 — `impl` silently misregisters a **bare** record id · **FIXED 2026-07-27**

**Fixed:** `impl` and `:sealed` now share one helper, `ability--id-kw`, which qualifies a
bare symbol against `(current-ns)` (`circle` → `:<ns>/circle`), keeps a keyword id
untouched (`:int`, `:default`), and never double-qualifies an already-`/`-qualified
symbol. So `(impl S circle …)` registers under the same `:<ns>/circle` a value presents.
Regression tests: `tests/ability_test.blsp` "impl id qualification (KI-15)" (a bare id
dispatches) and `sealed_ability_bare_impl_id_qualifies_ki15` (a bare impl satisfies a
sealed member). Original report:


`ability`'s two macros disagree about whether a bare record symbol gets namespace-
qualified, and the mismatch fails *silently* at registration:

```lisp
(defmodule geometry (:use ability))
(defrecord* circle (r))
(defability Shape :sealed [circle rect] (area [self] :-> float))  ; :sealed QUALIFIES → :geometry/circle
(impl Shape circle (area [c] …))                                  ; impl does NOT     → :circle
(area (circle 2))
;; error: ability Shape/area: no impl for :geometry/circle — have (:circle)
```

`defability--sealed-vec`'s member mapping qualifies a bare symbol against
`(current-ns)`; `impl`'s `id-kw` is a plain `(keyword (name key))`. Since
`identity-of` produces the *qualified* `:module/name` keyword, the bare form
registers under a key no value ever presents — so the impl never matches, the
sealed-exhaustiveness check reports the member as unimplemented, and the failure
only surfaces at the first call.

**Workaround:** always write the id qualified — `(impl Shape geometry/circle …)`,
which is what `tests/ability_test.blsp` and the documentation do.

**Fix direction:** qualify a bare symbol in `impl` the same way `:sealed` does (a
symbol with no `/` gets `(current-ns)` prepended), so both macros agree. A keyword
id (`:int`, `:default`) must keep passing through untouched, and an already-
qualified symbol must not be double-qualified. Alternatively make a bare,
unqualifiable symbol an expansion-time *error* rather than a silent
misregistration — but qualifying is the ergonomic answer and matches `:sealed`.
Either way, add a test that a bare id and a qualified id reach the same impl.

**Found by:** writing the ADR-168 documentation and following the `impl` docstring
verbatim on a first attempt (docs run, 2026-07-27).

---

## KI-16 — the LSP still matches the retired `defprotocol`/`defimpl` · **FIXED 2026-07-27**

**Fixed:** `introspect::protocol_ops` now scans both `*abilities*` and `*protocols*`, so
`impl` op-completion works and `:implements` hovers still resolve; `completion.rs`'s
`enclosing_impl` matches `(impl …)` (was `(defimpl …)`); `definition.rs`/`module_ref.rs`
match `"defbehaviour" | "defability"` (was `| "defprotocol"`). Regression:
`offers_ability_ops_inside_impl`. Original report:


ADR-168 removed `defprotocol`/`defimpl`, but the language server was not migrated
with them:

- `definition.rs` (`enclosing`/interface goto) and `module_ref.rs` match
  `"defbehaviour" | "defprotocol"`. The `defprotocol` arm is dead code — harmless,
  but it will never fire again.
- `completion.rs` offers an interface's ops inside a `(defimpl …)` form
  (`enclosing_defimpl`). Since `defimpl` no longer exists, **op completion inside
  an `(impl …)` form is simply missing** — the one user-visible loss.
- `completion.rs`'s test seeds `*protocols*` directly and is named
  `offers_protocol_ops_inside_defimpl`.

Nothing is broken for `defbehaviour` (goto + hover over `(:implements …)` still
work); abilities are never claimed in a module header, so they were never part of
that path. The gap is purely the `impl` completion affordance.

**Fix direction:** rename `enclosing_defimpl` → `enclosing_impl` and match `impl`,
reading ops from `ability/*abilities*` (via `introspect::protocol_ops`' ability
sibling) as well as `*protocols*`; drop the `defprotocol` arms from
`definition.rs`/`module_ref.rs`. Update the test name and seed.

**Found by:** the documentation validation pass for ADR-168 (docs run, 2026-07-27).

---

## KI-14 — the RUNTIME collector re-walked a deep process's whole root stack at every safepoint · **found 2026-07-27, fixed 2026-07-27**

**Symptom.** `make test` could not go green: `brood::suite brood_suite_passes` was
SIGKILLed at its 600 s nextest cap, deterministically. One-line repro, which hung
indefinitely (observed >10 min):

```sh
nest test --only 'test:every n_ document'
```

**What made it confusing.** The same work outside the test framework was fast and correct:
the identical scan over the identical 318 corpus files returned `[0 188 nil]` in seconds,
each of the 188 `n_` documents parsed fine one-per-process, and the two deep-nesting
documents parsed cleanly inside a spawned green process. It needed the framework context —
and, it turned out, nothing about the framework except **how much code it had loaded**.

**Root cause.** The ADR-091 RUNTIME two-generation collector's cooperative drain report
(`Heap::report_gen_liveness` → `runtime_gen_referenced_private`) probes in two phases:
Phase 1 over the process's private roots and live arms, Phase 2 over its whole LOCAL heap.
Phase 2 already had a stale-dirty throttle (`P2_REVALIDATE_STRIDE`) added for exactly this
class of problem. Phase 1 was deliberately unthrottled, on the premise that it is the
*cheap* probe.

That premise has an unbounded term: `roots` is the VM operand/env stack, so **Phase 1's
cost is O(recursion depth)**. Parsing a 100 000-level JSON document makes it enormous, and
instrumenting the hung run showed the scale — inside a **single** drain epoch:

```
drains: 1   phase1 walks: 78409
[rt-dbg] phase1 roots=1727686 env=2 dyn=0 arms=66448
```

78 409 walks over a 1.7-million-entry root stack. Three compounding faults:

1. **The Phase-2 stale-dirty short-circuit sat *between* the phases.** A process dirty via
   Phase 2 therefore paid a full Phase-1 walk on every safepoint and then discarded the
   result — the verdict is `true` regardless of what Phase 1 found. Pure waste, invisible
   while Phase 1 really was cheap. This was the dominant cost.
2. **Phase 1 had no throttle of its own,** so a process dirty *via Phase 1* (deep inside
   draining-generation code) re-walked O(depth) every reporting safepoint.
3. **`live_vm_arms` was walked per active frame, not per distinct arm.** It is a per-frame
   stack, so a recursive function appears once per frame — 66 448 entries that are a
   handful of distinct `Arc`s, each arm's whole IR tree re-walked every time.

Why loaded-code volume was the trigger: the drain only arms once the shared RUNTIME region
crosses `BROOD_RT_GC_FLOOR`. Few files loaded → no drain → no probe → 520 ms. Whole suite
loaded → drain armed → the above. `BROOD_RT_GC_FLOOR=100000000` (collector off) took the
hung repro to 2.2 s, which is what confirmed the subsystem.

**Fix.** Hoist the stale-dirty short-circuit above Phase 1 (changes no verdict — it only
skips work whose result was already discarded); give Phase 1 its own `P1_REVALIDATE_STRIDE`
throttle, gated on `P1_LARGE_SEED` so a shallow process keeps reporting on its very next
safepoint (the promptness the drain-completion tests rely on); and dedup `live_vm_arms` by
`Arc` identity. Soundness is the same argument Phase 2 already made: a stale-dirty verdict
only *delays* drain completion, it can never fabricate a clean ack.

**Result.** The filed repro: hang → **9.8 s**. Whole in-language suite including
conformance: **3591 tests, all passing, 88 s**.

**Guarded by** `tests/jit_deep_recursion_test.blsp` (deep recursion under the collector)
and the existing `crates/lisp/tests/runtime_collector.rs` drain-completion tests, which pin
the promptness the seed-size gate preserves.

**Residual — the canary's timeouts were too tight for the grown suite (2026-07-29).** The
hang is fixed, but the guard tests began hard-killing at the framework's 120 s per-test
ceiling under the *grown* suite (3591 → 3800+ tests). Not a regression — a false timeout: the
guard-page fix now makes deep recursion DEOPT to the VM and raise the catchable
`MAX_BC_FRAMES` error (→ the parse returns `:rejected`), so the worker is *slow*, never dead.
With the drain armed (the whole suite's loaded code arms it — the very condition under test)
one parse costs ~36 s of CPU (measured at `BROOD_RT_GC_FLOOR=1`), and `:serial` serialises
only *within* the group, so the warm passes + spawned parse compete with the rest of the
suite for cores; the whole-test wall-clock overran 120 s (the suite still *completed* — no
guard-page abort, the real KI-14 symptom). Fixed by tagging the group `:slow` (the
`*test-slow-timeout-ms*` = 900 s batch budget the conformance corpora already use for work
that is long, not stuck) and widening the in-test `after` liveness nets to 850 s (a stuck
worker is still caught, cleanly, under the ceiling). The warm passes were kept — they are
load-bearing (the spawned parse must hit the WARM native path, the condition the canary
guards). The *underlying* residual — a deep-recursing process still pays a
throttled-but-O(depth) Phase-1 drain-report cost — is bounded and left for a separate,
ADR-gated collector-perf pass if it ever bites outside this canary.

**Found alongside** two genuine stack-overflow aborts on the same corpus, both fixed and
both distinct from the hang (each aborts rather than hangs, and neither is the reason
`make test` was red):

- **A JIT'd arm could run the native stack into its guard page.** The pre-existing guards
  (`jit_native_depth` + the `stacker` headroom probe) sit on the *dispatch* paths, so they
  only bound recursion that goes through a fast link; recursion via `brood_rt_call_slow`
  re-enters Rust every level while the depth counter stays near zero. Fixed with a
  three-instruction prologue guard in every lowered arm, checking the frame's address
  against a `Heap::jit_stack_limit` stamped from the live remaining stack at each native
  entry. On a trip it sets `jit_force_vm` and deopts, so the subtree drains through the
  VM's bounded heap frames and raises the clean, catchable `MAX_BC_FRAMES` error. (Deopt
  alone livelocks: the VM re-runs the arm, the callee re-tiers, the prologue trips again.)
- **`gc_runtime::flush_rt_value` ⇄ `flush_rt_pair` recursed unguarded.** The cdr spine was
  already iterative, but *car* nesting recursed one native frame per level, so promoting a
  deeply nested value into the RUNTIME region aborted the thread. The LOCAL twin
  (`gc::flush_value`) already had the `stacker::maybe_grow` guard; the RUNTIME one was
  simply missed.

**Adjacent finding, now its own entry and fixed:** `nest observe`/attach tests leaked a `brood`
child — one found alive 2h22m after its run (`/tmp/brood-observe-<pid>/target.blsp`, ~2.7% CPU).
Independent of this bug. Recording it *here*, inside a fixed entry, is what made it read as closed
for nine days; it is **KI-29** now, and fixed 2026-08-05.

## KI-13 — cross-module return-type inference blows up exponentially in branch count · **FIXED 2026-07-27**

**Fixed — it had TWO independent causes, both now addressed.**

*Cause 1 — type size.* Inferring an undeclared recursive value-builder's return unions
branch results into a `Ty` with a deep, `Arc`-shared refinement DAG; `==`/`Hash`/
`is_subtype` (recursive, no sharing dedup) walk it as a tree → superlinear per op.
`Ty::bounded` now widens any `Ty` whose refinement tree exceeds `MAX_TY_NODES` (64) to its
flat tag set — a sound over-approximation — applied at `union`, `intersect`, and every
structural constructor (`seq_of`/`map_of`/`record_of`/`tuple_of`), so no `Ty` ever gets
large. Fixed the moderate case (4-branch `deriv`: 25 s → 0.3 s). Regression:
`inferred_type_size_is_bounded_ki13`.

*Cause 2 — walk count.* With size bounded, the full 5-branch `deriv` was still dominated by
exponential **re-inference**: `sig_of`/`infer_sig` had no result cache, so a callee's body
was re-walked on every request (~400k walks), and because the cycle guard is slipped by a
self-call stored as a bare name vs the qualified name a caller resolved, the re-walks
compounded. `infer_sig` now **memoizes** completed inferences per check pass (`SIG_MEMO`,
cleared per `check_file`), capping it at one walk per distinct name. Full 5-branch `deriv`:
>900 s → 1.1 s.

Both are sound (widening / a deterministic memo for a read-only pass): 255 checker tests
plus the full app ecosystem (hatch 750, brood-chat 102, hatch-demo 89, …) unchanged. The
`(sig deriv (any -> any))` in the corpus is no longer load-bearing (kept as API hygiene).
Original report:


**Symptom.** `nest check` never finishes. No diagnostic, no progress output, one core
pegged. The LSP is the same code path, so an editor hovering the call site hangs with it.

**Trigger.** A recursive function with several `cond`/`if` branches that build *nested
list structure*, **called from another module**. The same call inside the defining file
checks instantly — this is specifically the cross-module path, where the checker infers an
undeclared callee's signature (`sig_of` → `infer_sig` → `expr_ty` over the whole body).

Minimal reproduction — this is the `deriv` benchmark from the Gabriel corpus
(`tests/support/gabriel/deriv.blsp`), which is how it was found:

```lisp
;; a.blsp
(defmodule zzm)
(defn d (a)
  (cond
    (not (pair? a)) (if (= a 'x) 1 0)
    (= (first a) '+) (cons '+ (map d (rest a)))
    (= (first a) '-) (cons '- (map d (rest a)))
    (= (first a) '*) (list '* a (cons '+ (map (fn (b) (list '/ (d b) b)) (rest a))))
    (= (first a) '/) (list '- (list '/ (d (second a)) (third a))
                             (list '/ (second a) (list '* (third a) (third a) (d (third a)))))
    else (error "no")))

;; b.blsp — checking THIS file is what hangs
(defmodule zz-probe (:use zzm))
(def r (d '(+ x 1)))
```

**Measured scaling** (release `nest check`, one recursive branch added at a time):

| recursive branches | `nest check` |
|---|---|
| 2 | instant (105 ms, the floor) |
| 3 | 105 ms |
| 4 | **8.7 s** |
| 5 | did not finish (killed at 900 s) |

So each additional branch multiplies the work by roughly an order of magnitude —
consistent with the inferred `Ty` growing multiplicatively as `expr_ty` unions branch
results whose element types are themselves nested unions, with the lattice operations
(`union`/`is_subtype`) then superlinear in that size. Not diagnosed further than the
scaling; nobody has profiled it yet.

**Not a hang in the sense of a loop.** `InferGuard` (`types/check/sigs.rs`) already breaks
recursion *cycles* correctly, and `expr_ty` has a depth cap. What is missing is a bound on
the *size* of an inferred type.

**Workaround, and why the corpus port uses it.** A **declared** signature is consulted
before body inference (`declared_heap_sig`), so it short-circuits the blowup entirely:
adding `(sig deriv (any -> any))` takes the case above from >900 s to 105 ms. That is what
`tests/support/gabriel/deriv.blsp` does, with a comment saying so — the sig is honest and
declaring a public API's type is right anyway (ADR-153), but it is load-bearing there and
must not be removed as decoration.

**Likely fix.** Cap the size of an inferred type and widen past the cap. Widening an
over-approximation is always sound, so this cannot introduce a false positive; it can only
lose precision on exactly the pathological shapes. The existing `MAX_INFER_DEPTH` is the
precedent for the shape of the fix.

**Severity.** `nest check` is a CI gate and the checker also backs the LSP, so a hang is
worse than a wrong warning — and the trigger is ordinary code, not something exotic. No
regression test yet; the reproduction above should become one with the fix.

---

## KI-12 — a frozen prelude global's inner handle resolved to the wrong object · **found + fixed 2026-07-26**

**Symptom.** The prelude's only non-trivial global value is corrupt in every build:

```lisp
(println (pr-str *load-path*))
;; expected: (".")
;; actual:   ("A list of the given arguments.")   ← `list`'s DOCSTRING
```

The *list* is right (one element, `rest` is nil) — its **car** is a different object
entirely. Which object depends on heap layout, which is what makes it a handle bug
rather than a logic bug:

| build / flag | `*load-path*` |
|---|---|
| debug, default | `("A list of the given arguments.")` (a docstring) |
| debug, `BROOD_GC_VERIFY=1` | `("ret")` |
| prelude written `'(".")` instead of `(list ".")` | `(xs)` — a **symbol** |
| **`BROOD_VM=0`** (tree-walker) | `(".")` — **correct** |

So the car slot holds whatever value happens to live where the handle points, and
the kind bits change with it — i.e. the stored `Value` is wrong, not merely
mis-read. Correct under the tree-walker, wrong under the VM.

**Not a regression.** A `brood` binary from 2026-07-25 18:49 (predating that day's
syntax work — it still rejects `^x` pins) shows the same class of failure, with
`cond`'s docstring in the slot instead. The 2026-07-26 prelude edits (alias trims,
`sig` adoption) only changed the *layout*, hence which wrong object appears.

**Why it hid for so long.** The prelude has essentially one global holding a heap
value, and nothing reads it on the happy path: `require` finds every std module via
`%builtin-module`, and a project run replaces the path wholesale
(`project-setup` → `set-load-path!`). So filesystem module lookup from the
*default* path has been broken without a single test noticing — except
`brood-lsp`'s `completion::tests::completes_module_names_in_require_and_use`,
which puts a temp dir on the default path and asks for completions. That test is
the canary and is **currently failing**.

**Root cause.** `to_prelude` (`crates/lisp/src/core/heap.rs`) re-tagged a handle by
keeping its slab **index** and changing its **region** bits. That is valid only for a
LOCAL handle, because the builder's slabs *become* the prelude region — and it was
applied unconditionally. Instrumenting the freeze showed the offending global was:

```
[freeze] *load-path*: Pair region=0 idx=55990 car=Str region=2 idx=60
```

a LOCAL pair (region 0) whose car was a **RUNTIME** string (region 2). The VM interns
its constant-pool literals into the shared RUNTIME region so compiled code can be
shared between processes, so `(def *load-path* (list "."))` — evaluated by the VM
during the prelude build — bound a pair holding a RUNTIME `"."`. Re-tagging turned
that into PRELUDE `Str@60`: a different slab, an unrelated string. The tree-walker
was correct because it passes the LOCAL string straight from the read form.

**Fix.** Two halves, both in `heap.rs`:

1. `Heap::localize_for_freeze` — before anything is re-tagged, deep-copy any part of
   a global's reachable graph that lives outside LOCAL into the builder's own slabs
   (a forwarding table collapses shared structure and terminates cycles). Reachable
   prelude state is then all-LOCAL by construction, which is what the re-tag assumes.
2. `to_prelude` re-tags **LOCAL only** and returns anything else untouched. The slab
   sweep visits every cell including unreachable boot garbage, which may legitimately
   hold RUNTIME handles; flipping those is what did the damage. A `debug_assert` on
   the root bindings catches a future `localize` gap at freeze time instead of
   silently corrupting a global.

Cost: one walk of the reachable global graph per *source* boot (freeze 5.0 → 8.7 ms,
source-boot peak 3.7 → 10.6 MB, both once per build — the cached boot is unchanged at
~49 ms). All three boot paths now agree: `*load-path*` is `(".")` under the source
boot, the cached boot, and `BROOD_VM=0`.

**What it had been costing.** Filesystem module lookup from the *default* load path
never worked — `require` found std modules via `%builtin-module` and a project run
replaced the path wholesale, so nothing noticed. `brood-lsp`'s
`completion::tests::completes_module_names_in_require_and_use` was the only test that
read the default path; it passes again.

## KI-11 — JIT tail-chain recursion escaped the native-depth cap · **found 2026-07-25, fixed 2026-07-26**

**Symptom (before the fix).** Deep non-tail recursion on the **JIT** path overflowed the
*native* stack and killed the process — `thread '<unknown>' has overflowed its stack /
fatal runtime error: stack overflow, aborting`. An abort, not a panic, so the crash-dump
hook (`install_crash_dump`) never fired and there was no `.brood_crash_dump` to read.

The other two engines were correct on the identical input, which is what identified it as
a JIT bug rather than a missing guard in general:

| engine | 20,000-deep nested JSON |
|---|---|
| default (JIT) | **process aborted** — native stack overflow |
| `BROOD_NO_JIT=1` (bytecode VM) | parsed fine — frames are heap-backed |
| `BROOD_VM=0` (tree-walker) | clean, catchable `recursion too deep: used 12585200 bytes of stack, over the 12582912-byte budget` |

Threshold was between 10,000 and 20,000 nesting levels. `gdb --batch -ex run -ex bt` put
the recursion in `jit_runtime::jit_run_fast_link` ← `brood_rt_fast_frame`.

**Impact (before the fix).** Any Brood service parsing untrusted nested input was
killable with a few kilobytes — `std/json` the obvious one, but this was a property of
the JIT call path, not of the parser. Unrecoverable: `try`/`catch` could not see it, and
a supervisor could not restart from it because the whole OS process died, not the green
process.

**Repro** (the minimal shape — a three-function cycle with a tail-call delegator,
entered from a non-tail position; see the root cause for why a plain `(defn deep (n) (+ 1
(deep (- n 1))))` runs to 200,000 without tripping it):

```lisp
(defn tv (n) (if (= n 0) [0 n] (ta (- n 1))))   ; enters the cycle
(defn ta (n) (tacc n))                           ; the tail-call delegator
(defn tacc (n) (let ([v j] (tv n)) [(+ v 1) j])) ; non-tail: result destructured
(tv 20000)                                       ; aborted pre-fix; fine now
```

**Found by** the JSONTestSuite corpus (`n_structure_100000_opening_arrays.json`,
`n_structure_open_array_object.json`) — see ROADMAP "External conformance corpora".

**Root cause.** Not a missing guard — a guard that stopped applying. The cap
(`JIT_NATIVE_DEPTH_LIMIT`, 1500) and its counter (`Heap::jit_native_depth`) already
existed and *were* checked by both dispatch entry points. But `jit_run_fast_link`
restored the counter to the caller's level as soon as the native callee returned, and
*then* handled the outcome — and three outcome arms re-enter the evaluator while that
frame is still on the native stack: the outcome-4 tail-chain follow-through
(`apply_value`) and the two deopt/preempt re-runs (`vm_resume_deopt` / `vm_apply`). So a
chain of tail-calling delegators oscillated between `depth` and `depth+1` forever while
the native stack grew without bound. The cap never tripped because the depth never
climbed.

That is why no existing depth test caught it: the trigger needs a **cycle** of functions
in which at least one link is a plain tail-call delegator (so its native returns outcome
4) and the cycle is entered from a non-tail position. Plain deep self-recursion — even
with a 20-local frame, even mutual, even building a 20,000-deep nested value — all stay
under the cap and run to 200,000 fine. `std/json`'s
`json--value` → `json--array` → `json--array--acc` → `json--value` is exactly the shape,
with `json--array` as the delegator.

**Fix, part 1 — make the counter count.** `jit_native_reenter` re-raises the depth for the
duration of each re-entrant call, so the cap sees the true native nesting and chains past
1500 levels drain on the VM's heap-backed frames as intended. The hot outcome-0 return path
is untouched, and the cap is only reached by depths that previously crashed.

**Fix, part 2 — make the cap a measurement.** With part 1 in place the release build was
fine but the *debug* build still aborted, because `JIT_NATIVE_DEPTH_LIMIT` is a frame
**count**, and a count is only ever right for one frame size: 1500 levels is a few MB of
the 16 MB worker stack in release and several times that in debug. Same root flaw as the
bug itself, one level up. `jit_native_headroom_ok` now probes
`stacker::remaining_stack()` and refuses a new native link below a 512 KB margin; the
count cap stays as the cheap first test, and the probe is skipped below 64 levels so the
hot shallow path (`fib`, `primes`) pays nothing beyond the existing integer compare.
Measured flat: primes-to-200k 57→58 ms, fib-30 5→6 ms (3-sample, within noise).

This also covers a case neither the count nor this bug report anticipated: a host embedding
Brood on a smaller thread stack. Returning `false` is never a correctness problem — the
caller falls through to the VM, which is where deep recursion belongs.

**Regression test.** `tests/jit_tail_chain_depth_test.blsp` — the minimal three-function
cycle at 30,000 levels, the deeply-nested-JSON rejection, and the plain-self-recursion
shapes that always worked (so a future depth change has to keep them working). Verified by
A/B against a pre-fix binary: pre-fix aborts at 20,000, post-fix runs to 50,000 — in
**both** profiles, debug included, which is what part 2 bought.

The two JSONTestSuite documents that found this are still skipped in the sweep, but on
cost grounds rather than correctness: rejecting a 100,000-level document now means actually
draining it on the VM, which is ~400 ms standalone and >120 s inside the full parallel
suite. That >250× gap is steeper than contention explains and is logged in ROADMAP as a
possible GC-depth pathology worth its own investigation; the property is covered by a
synthetic 5,000-level case.

---

## KI-10 — `receive` compile cliff at the 13th arm · **no longer reproduces 2026-07-25**

Adding a 13th arm to a hot `receive` loop degrades it badly: on `buffer--serve`
(std/editor/buffer.blsp), a single TRIVIAL extra arm (`([:sync-probe] (recur …))`)
took the buffer suite from 4.9 s / 139 MB peak to 8.0 s / 248 MB — +65% wall,
+80% peak — and pushed the full parallel suite over the 1 GB test soft limit.
Bisected 2026-07-11 while adding a `[:sync …]` arm for the resync primitive;
12 arms are fine, 13 fall off the cliff, so the dispatch likely drops from an
indexed strategy to an allocating linear one at that width. Worked around at the time by
merging `buffer--serve`'s two `[:edit …]` arms (buffer-edit always sends the
3-element form) to stay at 12.

**Re-measured 2026-07-25 on `b0b4fd1`: gone.** Adding the same trivial
`([:sync-probe] …)` arm to `buffer--serve` and rebuilding costs nothing, and neither
does adding eight:

| serve arms | buffer suite (`brood --test tests/buffer_test.blsp`) | full suite (`nest test`) |
| --- | --- | --- |
| 12 (committed) | 3.33 s / 55 MB | 22.49 s / 610 MB |
| 13 | 3.35 s / 54 MB | — |
| 20 | 3.36 s / 55 MB | 22.79 s / 636 MB |

Measured on a `cargo build --release` binary with the module **baked in** (the
configuration the cliff was originally seen in — a hot-loaded copy was checked too and
is equally flat). Against the original +65% wall / +80% peak for a single arm, +1.3%
wall / +4% peak for *eight* is flat.

**The mechanism was never identified, so this is an incidental fix**, from some part of
the VM/JIT/pattern work between 2026-07-11 and now — not a change aimed at it. The
arm-count budget is therefore lifted (the note in `buffer--serve` is updated), but if a
width-dependent cliff ever reappears it needs bisecting from scratch: the width alone
was never the trigger. A 13-arm receive of *uniform* trivial arms never showed it
either, which is why the original report is specifically about `buffer--serve`'s arm
shapes.

## KI-9 — arity error from a closure shipped in a `spawn` body · **likely transient; not present in committed code**

Surfaced once while parallelising `nest check`: a driver spawned
`(spawn (send me (list :chunk (self) (chunk-fn chunk))))` where `chunk-fn` was a
*closure passed in as an argument* (captured in the spawned body's env); ~1 worker in
3 died with a bogus `arity error: fn: expected 0 arguments, got 1` at the
`(chunk-fn chunk)` call, silently skipping its files.

**Most likely a transient inconsistent-build artifact, NOT a standing kernel bug.**
The sighting happened while a *concurrent session* was mid-edit on the Rust tree
(uncommitted type-checker changes to `check.rs` et al., committed later as
`be0f8cc`); building from a half-applied edit can yield a binary with a real-but-
transient fault. The decisive evidence is the **frequency collapse**: it died ~66 %
of runs then, and **0 of 50+** now under identical repro conditions (a minimal
captured-closure spawn; the *exact* old shipped-closure `pfold` + `check-file`
chunk-fn at 24 and ~470 workers; the full multi-level `pfold-files→groups→spawn`
capture; all under `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` on a debug-assertions
build — all clean, no tripwire/verifier hit; deterministic arity-through-`spawn`+
`promote` passes). A stable race doesn't go 66 %→0 %; a changed binary does. A code
audit of the promote path (`heap.rs::promote_closure`/`promote_env`) found no defect
— arms/arity copied faithfully, reserve-then-fill with cycle-breaking, lock-free
concurrent append, and RUNTIME compaction can't fire mid-fan-out (uniquely-owned-Arc
gate). **No fix applied** — deliberately not blind-patching the moving-GC/shared-
RUNTIME path to chase a fault not present in the committed code. If it ever recurs on
a *clean committed build*, reopen with that repro.

**Best-practice (independent of this):** prefer shipping **data** to a spawned worker
and resolving the operation through a **global** (as the test runner and
`std/tool/project.blsp`'s `project--pfold-run` do) over capturing a closure value in a
`spawn` body — it avoids the heavier per-spawn `promote` deep-copy of a captured-env
closure regardless.

## KI-8 — RUNTIME form-position table (`positions`) stranded by compaction · **fixed 2026-07-03**

The bug-KI-3 class again, in a side table the KI-3 fix didn't cover. `RuntimeCode::positions`
(source positions of RUNTIME list forms) is keyed by the pair's RUNTIME **slab index**, with a doc
comment asserting "a RUNTIME pair never moves" — a pre-ADR-091 premise the compactor invalidated. A
compaction relocated the pairs but not the keys, so `(form-pos …)` / source-location returned a
stranger's position (or none) afterward. Diagnostics-only. **Fix:** `runtime_collect_with` remaps
the keys through the same `fwd.pairs` forwarding after the evacuation walks, dropping entries whose
pair didn't survive (mirroring the LOCAL `form_pos` remap). **Guarded by:** the `declared_sigs`
regression test exercises the same rewrite pass; the LOCAL analog `form_pos` is remapped in `collect`.

## KI-7 — declared `(sig …)` type-expressions corrupted by RUNTIME compaction · **fixed 2026-07-03**

The bug-KI-3 class in another off-graph holder. `RuntimeCode::declared_sigs` stores each `(sig name
type)` form's type-expression as a **promoted RUNTIME `Value`**, held in a `SymbolMap<Value>` beside
`globals` but NOT walked by `runtime_collect_with`. A compaction relocated the type-expr out from
under the stored handle, so the checker's `sig_of` later read a garbage form (confirmed: `(int ->
int)` read back as `(i 1)` after churn + compact). Silent wrong-data (no tripwire — the handle is a
valid RUNTIME index, just the wrong cell). **Fix:** `runtime_collect_with` now `flush_rt_value`s
`declared_sigs` alongside `globals`. **Guarded by:** `tests/runtime_collector.rs::declared_sigs_survive_a_runtime_compaction`.

## KI-6 — `%isolate` snapshot/restore not RUNTIME-compaction-safe · **fixed 2026-07-03**

A sibling of KI-2 (that fix handled orphan-process reaping; this one the compaction-relocation race).
`%isolate` snapshots the global table — an off-graph `SymbolMap<Value>` of raw RUNTIME handles — runs
a thunk, then restores it. A RUNTIME compaction *during* the thunk (its `def`s crossing
`BROOD_RT_GC_FLOOR`, trivially met in a large image) relocated those handles; the stale snapshot then
reinstalled handles aliasing *other* closures → an unrelated pre-isolate global silently misdispatched
(`foo` → a 1-arg `z-*` defined inside the rolled-back isolate). Latent for every `:isolated` test.
**Fix:** a re-entrant `Heap::rt_collect_block` counter (a `Cell<u32>`) suppresses RUNTIME compaction
while a globals snapshot is outstanding — `snapshot_globals` increments it and `restore_globals`
decrements it, so the invariant holds *structurally* (every caller of the protocol is covered, not
just `%isolate`). Checked at the `runtime_collect_with` choke point, so BOTH the auto safepoint path
(via `rt_gc_due`) and a manual `(runtime-collect)` are covered — an explicit collect inside an
`%isolate` is a no-op, not a corruption. The isolate's `def`s become garbage at restore and are
reclaimed by the next safepoint. **Guarded by:**
`tests/runtime_collector.rs::{isolate_is_safe_against_a_runtime_compaction_inside_the_thunk,
manual_runtime_collect_inside_isolate_is_a_noop}`.

## KI-5 — `nest test` OOMs: shared RUNTIME region accumulates every test file's code · **fixed 2026-07-03**

`run-project-tests` loaded every test file into one long-lived driver image before running any, so
each file's top-level `def`s promoted their compiled closures/chunks into the shared `RuntimeCode`
region + global table — globally rooted, live, unbounded, unreclaimable (only same-name redefinition
frees the old version). A 725-test suite crossed the 1 GB soft cap → `memory limit exceeded` on
whichever workers were allocating (bedit: 9 spurious failures, all passing in isolation).
**Fix:** `test/run-tests-scoped` (+ `-structured`) runs the suite file-by-file, each file inside its
own `%isolate` (reset → load one file → drain → rollback), so the file's `def`s roll back and the next
safepoint reclaims the promoted code — bounding memory to ~one file (relies on the KI-6 fix so the
mid-run rollbacks are compaction-safe). `BROOD_TEST_NO_SCOPE` reverts to the legacy load-all path.
bedit: OOM → 725/725 at 199 MB. **Guarded by:**
`tests/runtime_collector.rs::per_isolate_scoping_bounds_runtime_region_growth`.

## KI-4 — bitset stored as a non-UTF-8 `Value::Str` corrupts the GC on promote · **fixed 2026-06-15**

A bitset was a blob-backed `Value::Str` holding raw, non-UTF-8 bytes, but
`Value::Str`/`SharedBlob` carry a valid-UTF-8 invariant; promoting a closure that
captured one (`spawn`/`def`) read the bytes through the UTF-8 string accessor →
panic (armed) or UB/`flush_oob`/SIGSEGV (release). Surfaced ~1-in-3 in the
brood-life `--fair` demo. **Fix:** bitsets are a distinct `Value::Bitset` kind with
their own raw-byte slab (LOCAL `Vec` + RUNTIME `boxcar`), byte-clean accessor /
`promote_in` / equality / `Message::Bitset`, mirroring the `bigint` leaf slab — a
bitset can no longer reach a string accessor. **Guarded by:** the spawn-promote-a-
bitset path under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

> **Superseded 2026-06-28:** the whole `bitset` feature (the `Value::Bitset` kind +
> 13 `bitset-*` prims) was removed — it had no in-repo or external consumer left.
> The KI-4 fix is moot now that the type is gone; kept here as the historical record.

## KI-3 — RUNTIME compaction strands live VM / tree-walker constants · **fixed 2026-06-01**

Once the ADR-076 RUNTIME compactor made promoted code-region handles movable, two
sites held them as immovable: the tree-walker elided the operand-stack slot for a
RUNTIME root (so `runtime_collect` never rewrote it), and the VM held promoted
handles inline in `Node::Const`/`MakeClosure.fn_rest` (off the GC root graph). A
compaction at a nested safepoint left them dangling → `flush_oob` or a constant
read back as a different value. **Fix:** `needs_root_slot` (LOCAL **or** RUNTIME)
gives a RUNTIME handle an operand slot; the VM carries movable consts as
`ConstVal::Handle` and registers its live arm in `Heap::live_vm_arms`, which
`runtime_collect` rewrites in place. **Guarded by:** `compile::tests::{const_handle_round_trips,
rewrite_arm_handles_rewrites_every_embedded_handle}` and
`tests/runtime_collector.rs::auto_safepoint_collect_bounds_runtime_region`.

## KI-1 — multi-thread scheduler race: green processes can't resolve globals · **fixed 2026-05-29**

Spawning green processes that touched globals crashed workers with bogus `unbound
symbol` errors (a data race on shared global/scope state via the kernel
supervisor's RESUME_SLOT machinery, worsened by free-list slot reuse). **Fix
(in series):** strip the kernel supervisor (ADR-039, reverted → ADR-044); switch
to a bump-only allocator (slots never recycle, so a stale handle can't observe a
wrong-type value); per-worker pinned queues. **Durable invariant:** no recycled
slots / no stale handles across a safepoint. (The per-worker *pinning* stopgap was
later superseded by ADR-100's heap-captured continuations, which make cross-thread
migration safe and routine.) **Guarded by:**
`tests/concurrency_race.rs::fanout_with_concurrent_global_rebind_matches_serial`
(the fan-out-matches-serial bar) and the self-diagnosing `flush_oob`/`flush_bound!`
OOB check.

## KI-2 — `nest test` flaky / hangs when parallel tests share heavy global lookups · **fixed 2026-05-29**

Two bugs: (1) the KI-1 lookup race could kill a worker; (2) the runner didn't reap
a dead worker, so the run hung in `receive` forever. A 2026-06-07 recurrence under
maximal load was root-caused **not** to a core race but to test isolation:
`%isolate` (test-only) wholesale-restored the globals table, so a test that left an
orphan process running saw the orphan's next lookup die `unbound`. **Fix:** the
runner `monitor`s every worker and accounts for each exactly once (death → a failing
result, not a hang); `%isolate` reaps the processes its thunk spawned (via the
green-friendly `scheduler::yield_now`, never a thread sleep) **before** restoring
globals. Production never wholesale-restores globals, so the language itself was
never implicated. **Guarded by:** `tests/runner_failfast_test.blsp`.

## Platform gaps — GUI display seam · **all resolved 2026-05-31 (ADR-079)**

The GUI frontend had one font size for everything. Resolved: a `Face` carries an
integer `:scale` (per-op/region larger text in a scale×scale cell block — also
covers per-pane font); `gui-font!` takes an optional window id for per-window fonts;
`std/editor/pane.blsp` (ADR-077/078) provides pane layout + clip-rects. Per-pixel
`:height` sizing stays deferred (would break the uniform grid).

## Minor (all fixed)

- **Type-checker noise around `(require 'proc/hatch)`** — `check_file` pre-evaluates
  top-level `(require …)` so the required module's macros resolve.
- **`nest format` collapsed multi-line forms** — fixed (`5b19787`); respects author
  newlines. Still normalizes intra-line multi-space alignment (a standard trade-off).
- **Plain-release segfault on tail-recursive workers** — fixed by per-worker pinned
  queues, then made moot by ADR-100 (heap-captured continuations).
- **`cargo test --test suite` debug segfault** — coroutine stack overflow, not a
  memory bug; `WORKER_STACK_BYTES` raised (pages mmap'd lazily, ~0 cost until needed).

## KI-21 — `nest run --for` / `--watch` emit a pre-ADR-150 `~p` pin ✅ FIXED 2026-07-30

**Status:** ✅ **fixed** 2026-07-30 (`~p` → `^p`). Found while smoke-testing a
TUI/GUI app end to end.

**Symptom.** Any `nest run --for <duration>` or `nest run --watch <path>` fails
immediately, whatever the file:

```
$ printf '(println "hi")\n' > /tmp/t.blsp
$ nest run /tmp/t.blsp --for 1s
error: match: `~p` is not a pattern — a pin is written `^p` (`~` belongs to
quasiquote alone). To match the literal 2-element list (unquote x), quote the
head: ('unquote x).
    at error / match-compile-velems ×3 / match-compile-vector /
       match-build-clause / receive
```

Plain `nest run <file>` is unaffected — only the wrapped path.

**Cause.** `crates/nest/src/main.rs:1203` builds the wrapper source as a string:

```rust
"(let (p (%spawn (fn () {}))) \
      (monitor p) \
      (receive ([:down _ ~p reason] (println \"[exit]\" reason)) {}))",
```

`~p` was the pin syntax before **ADR-150** moved pins to `^p` and made `~`
quasiquote-only. The generated `receive` therefore fails to compile at runtime.
Because the wrapper is only emitted when `timed.is_some()` or `--watch` is set,
nothing in the normal `nest run` path exercises it — and since the code is a Rust
string literal, neither `nest check` nor the Brood test suite can see it.

**Fix.** `~p` → `^p` in that format string. Verified: `nest run --for 800ms` over a
trivial file now exits 0 and prints `[stopped after 800ms]`.

**Guard — still owed.** A test that actually *runs* `nest run --for 200ms` and asserts
a zero exit. The general lesson is the durable part: **Brood source generated from Rust
string literals is invisible to `nest check` and to the in-language suite**, so every
such snippet needs an execution test rather than a reading. Worth grepping for the
others.

---

## KI-49 — the tagged-tuple `receive` matcher was latched onto the interpreter ✅ FIXED

**Fixed 2026-08-21.** An operand crossing a block boundary now carries a *representation*
(`ParamRepr::Int | Bool | Slot(k)`), agreed across predecessors by the existing
`record_block_flags`. A boxed `Op::Handle` with no slot of its own is **spilled to a frame
slot derived from its operand-stack POSITION** — position, not `spill_next`'s allocation
order, because every predecessor must name the same slot or the edge is rejected. A slot the
tier-time profile saw an `Int` in keeps the unboxed carry, so `fib`/`collatz` are untouched.

| | before | after |
|---|---|---|
| the matcher | latched to the VM | **native** (`jit_link_done` 0 → 187 328, `hof_decline_bailed` 98 981 → 0) |
| `ns_match_run` | 457 ns/msg | **179 ns/msg** (2.6x) |
| `vm_apply` | 100 166 | 6 406 |

`make ab` against the groundwork commit: **`pingpong` −12.9%** (217 → 189 ms, reproduced at
−12.1/−12.3/−12.9% over N=7/11/15), `ring` −1.4%, and — the risk that made this
measure-before-ship — **`spawn` −1.6%, `fib` +1.2%, `collatz` 0.0%**, i.e. the frame growth
did *not* reproduce the 1.9x `spawn` regression blanket-reserving once caused. The reserve
stays behind the `chunk_in_jit_subset` gate, so only lowerable arms pay it.

That takes `pingpong` from 3.7x to ~3.26x of Elixir. It is not parity and was never going to
be: the bare-atom path is 739 ns against Elixir's ~290 ns per message, so `receive` +
`deliver` remain the structural remainder.

### (the diagnosis that led here follows)

> Everything below is the diagnosis **as it was recorded before the fix above**, kept because
> the elimination order is what made the root cause findable. Its status line is historical
> and superseded — KI-49 is fixed; nothing here is open.

**Status (as recorded then; superseded by the fix above):** ⚠️ **open (2026-08-21) —
root-caused and localised; not fixed.**

**What it costs.** `pingpong` is Brood **211 ms** vs Elixir **58 ms** (3.7x); `ring` 2.9x;
`supervisor` 3.3x. Those three rows are exactly the ones whose protocols are **tagged
tuples**, and this is why.

    receive pattern     ns_match_run   hof_native_deopt   ends up
    `:ping`                  59 ns            0           native, stays native
    `[:ping x]`             454 ns           16           BAILED, VM for the process life

The matcher **is** JIT'd. It then type-deopts on its first 16 native activations and the
sixteen-deopt rule latches it to `BAILED` permanently. Same shape as **KI-44**'s `nbody`
bug — "silent interpretation, no error, no failing test" — now its second confirmed
instance, which suggests the deopt-latch deserves a standing check rather than being
rediscovered each time.

**Where it deopts.** All 16 land on the same instruction:

    [jit-deopt] arm=<closure> resume_ip=7 op=Const (journalled)   x16

    ip: 0 Local  1 Call(vector?)  2 SetLocal  3 Local  4 JumpIfFalse
        5 Local  6 Call(vector-length)  7 Const  8 Prim2  …

i.e. immediately after the `Call` to `vector-length`, at the length comparison.

⚠ **CORRECTION (same day).** An earlier version of this entry said the deopt was specific
to the HOF native fast-frame. **That is wrong**, and the check that disproves it is one flag:
with `BROOD_NO_HOF_JIT=1` — which disables that protocol entirely — the arm still deopts
exactly 16 times, merely counted on the other path (`jit_deopt` 16 / `hof_native_deopt` 0,
against 0 / 16 by default). The arm's native code deopts **regardless of how it is invoked**.

**What IS established**, each by measurement:

- exactly **16** deopts, then the sixteen-deopt rule latches `BAILED` for the process life
- all resume at **ip 7** — the checkpoint written after the `Call` to `vector-length`, so the
  failing guard is at or after ip 7, *not necessarily at it* (the journal records the last
  checkpoint, not the deopt site — a distinction that cost an hour)
- **payload type is irrelevant**: `1`, `:x`, `"s"` and `[9]` all thrash identically, so it is
  structural to the pattern, not a value-type guess
- **not an optimizer pass**: `BROOD_NO_INLINE`, `BROOD_NO_LEAF_INLINE`, `BROOD_NO_PARTIAL_LEAF`,
  `BROOD_LINMAP=0` and `BROOD_NO_HOF_JIT` each still thrash
- **not message-vector indexing**: a hot arm doing `(nth m 1)` on a *message-delivered* vector
  does not deopt, and neither does a hot loop doing `(= (vector-length v) 2)`

### ROOT CAUSE (2026-08-21): a non-Int value carried across a block boundary

Per-deopt-site reason codes were built to answer this (all 43 branches into the shared deopt
block now pass a distinct id, recorded via `brood_rt_note_deopt`; the `as_int` guard also
encodes the *observed* tag in the low byte). The verdict:

    [jit-deopt] arm=<closure> resume_ip=7 op=Const reason#5386   x16

`5386 = (21 << 8) | 10` — guard **21** is `as_int`'s `Op::Handle` case, and tag **10** is
`TAG_VECTOR`.

Guard 21 is reached from **`as_block_arg`**, which materialises an operand that crosses a
block boundary. Cranelift block params here are declared `I64`, so *every* block-carried
operand is forced through `as_int` — and a non-Int value deopts. A tagged-tuple matcher
cannot avoid this: it tests `vector?`, then `vector-length`, then `nth` on the **same**
message, so the vector crosses the `if`/`and` merges.

Confirmed by discriminator: a tuple pattern that **binds nothing** (`[:ping]` against
`[:ping]`) deopts identically, so it is not the bindings vector — it is the message itself.
The deopt then re-runs on the VM, which is why results stay correct and only speed is wrong.

**Two fixes are possible, and they are not the same fix.**

1. *Cheap and honest:* teach the profitability gate to refuse an arm that would carry a
   non-Int operand across a block boundary. It ends up on the VM either way, but without the
   compile, the 16 deopts and the latch. **No faster — just stops the waste.**
2. *The real one:* carry a boxed operand across block boundaries as its three words instead
   of forcing `as_int`. This is what would actually make tagged-tuple matchers native, and it
   is a genuine change to the block-argument protocol in `jit_lower`, not a tweak.

The prize is unchanged and still bounded: ~390 ns of a 2144 ns round trip, `pingpong` roughly
211 ms → 175 ms (3.7x → 3.0x vs Elixir). It does not reach parity on its own.

**Ruled out, by measurement rather than argument:**

- *a lowering refusal* — with every `BAILED` route traced, the arm is refused by none of them
- *shared-code adoption across mismatched profiles* — `BROOD_NO_SHARED_ARMS=1` still thrashes
- *inline-cache adoption / the inline swap* — traced, never fires for this arm
- *`codegen_poisoned`* — traced, never fires
- *"it never lowers"* — it does: `arm: 29 (<closure>) ckpt_slot: 6`

**A measurement trap worth keeping.** The thrash is **timing-sensitive**: with
`BROOD_JIT_DUMP_IR=1` the background compiler is slow enough that at N=20000 the arm never
compiles, so it neither dumps nor thrashes and the bug looks absent. It needs N=300000 to
show up *with* the dump on. A green run under the dump flag proves nothing here.

**Next step.** Read the deopt guard at ip 7-8 in the arm's CLIF (dumped at N=300000) and
determine why the fast-frame protocol invalidates it where the ordinary call path does not.
Worth ~390 ns of a 2144 ns round trip — `pingpong` roughly 211 ms -> 175 ms — which is
real but does not reach parity on its own: the bare-atom path is already 739 ns against
Elixir's ~290 ns per message, so `receive` + `deliver` (61% of even the cheapest path) is
the structural remainder.

## KI-48 — JIT tail dispatch read past the roots stack ✅ FIXED 2026-08-21

**Status:** ✅ **root cause found and fixed 2026-08-21.** Two crashes, 4 s apart, captured in
`.brood_crash_dump` on 2026-08-20; never reproduced on demand, so the causal link to the fix
below is strong (right fault shape, right function, right stack) but **not proven**.

### The bug

`jit_dispatch_tail` computed where the native staged its `[callee, args…]` as:

```rust
let top = base + arm.active_nslots();
```

`active_nslots()` re-reads `inline_installed` — which the deopt code *in this same file*
already calls "the anti-pattern behind two ADR-210 bugs" (KI-26). The background inline
upgrade can flip that flag **between** the native entering (built to the small frame) and
this callback running, after which the staged area is written at one offset and read at
another.

**The hazard was live, not theoretical.** Instrumenting the path found **123 arms** reaching
tail dispatch whose two frame sizes differ, `fold` among them:

```
[ki48] tail-dispatch arm=fold nslots=13 inline_nslots=25 delta=+12
```

A mid-flight flip there sends `top` twelve slots past the staged area and off the roots
stack — precisely `root_at(9)` on a len-8 stack.

### The fix

The rule was already written down and obeyed everywhere else: the caller captures the size
once ("the two must agree on the same frame boundary") and, in `vm_run_bc`'s own words, "the
deopt-resume helpers must be **told** it rather than re-deriving it". This path was the one
that was not. `jit_dispatch_tail` now takes `frame_nslots` from the trampoline that built the
frame.

### What sabotage showed, and why the guard is not the fix

Forcing a desync (+8 slots at the call site) made the program **hang**, and the `top >= n`
tripwire did **not** fire: an overshoot that stays in bounds silently dispatches the *wrong
callee*. So the guard only catches the extreme case. It is kept as a backstop — it also
prevents `argc = n - top - 1` from underflowing, which would otherwise turn a clean bounds
panic into a wild `root_at(top + 1 + k)` loop — but passing the correct size is the repair.

### The audit: a SECOND live instance, and a false invariant

Fixing one call site does not fix a class, so every reader of the racy size was audited
(2026-08-21). Four production call sites; two were builders doing it right, two were not:

| site | verdict |
|---|---|
| `jit_runtime.rs` (native entry) | ✅ builds the frame, captures once — the model to copy |
| `mod.rs` `hof_apply_native` | ✅ builds the HOF fast frame, captures once |
| **`dispatch.rs` outcome-4 tail** | ❌ **same bug, and worse** — see below |
| **`vm_cache.rs` FastLink memo** | ⚠️ **safety argument was factually false** |

**`dispatch.rs` was the same defect with the noise removed.** Its outcome-4 path read the
staged callee at `base + active_nslots()`, in a branch *already gated on
`!inline_installed`*, over a frame `push_frame` always sizes to `arm.nslots`. Because it
guards with `if n > frame_top` it never panics — it silently dispatches the **wrong callee**,
which is exactly what a forced desync reproduced as a hang. Now `base + arm.nslots`.

**`vm_cache.rs` claimed protection it does not have.** Its comment read "the small→inlined
swap bumps the epoch, so a stale memo here is invalidated". `jit_tier`'s swap deliberately
does **not** bump `global_epoch` (a bump cascaded under `pfib`); it stores the flags and calls
`invalidate_fast_links_for(arm.inline_name)`. The memo is protected — by a different
mechanism than the one written down. Comment corrected, and the asymmetry it exposes recorded:
the `inline_installed.store(true)` is unconditional while the invalidation is
`if let Some(sym) = arm.inline_name`, so the guarantee is held by construction elsewhere
rather than enforced there.

### The redesign: the anti-pattern now has to be justified

This was the **third** appearance of one anti-pattern — KI-26 and two ADR-210 bugs before it —
so the point fix is not the deliverable:

* `active_nslots()` is renamed **`frame_size_for_new_entry()`**, so the contract is visible at
  every call site rather than buried in a doc comment three files away, and its doc now states
  the rule: only a frame BUILDER may read it, and it must capture and pass on the result.
* `crates/lisp/tests/frame_size_callsites.rs` pins the permitted callers with a reason each.
  A new call site now fails the suite with a message explaining that a consumer must be TOLD
  the size. Sabotage-verified: reintroducing the read in `dispatch.rs` fails it by name.
  Limits stated in the test — it is file-granular (a new bad call inside an allowlisted file
  passes) and it cannot prove a caller correct, only force the question.

### No deterministic regression test exists

`tests/jit_tail_frame_test.blsp` covers the path (hot mutual tail recursion through the
computed dispatcher, concurrent so swaps land while peers are mid-dispatch) but **does not
guard the fix** — verified by sabotage: reverting to `active_nslots()` leaves it green,
because the fault needs the swap inside a few-instruction window and nothing forces that. It
is labelled as coverage in its own header so a green run is not misread as proof. Forcing the
desync artificially produced a **hang**, not an error, so there is no clean failure to assert
on. A real guard would have to drive the flip mid-dispatch; not attempted.

(Method note: a first sabotage attempt silently no-op'd because `cargo fmt` had reformatted
the call site and the patch carried no assertion. It "passed" while testing nothing — the
same shape as the gates repaired the day before. Assert that a sabotage actually landed.)

### (original entry follows)


```
panicked at crates/lisp/src/core/heap/gc.rs:1543:19:
index out of bounds: the len is 8 but the index is 9
  brood::eval::compile::jit_runtime::jit_dispatch_tail
  brood::eval::compile::vm_run_bc::vm_run_bc
  brood::process::scheduler::pool::run_one
```

`gc.rs:1543` is `root_at(i) -> self.roots[i]`. The read comes from `jit_dispatch_tail`:

```rust
let n = heap.roots_len();
let argc = n - top - 1;
let callee = heap.root_at(top);   // panicked here: top = 9, n = 8
```

So JIT'd native code entered tail dispatch with an operand-stack `top` **beyond the roots
stack**: either a stale `top` baked at lowering time, or the roots stack shrank between the
native code staging its args and the callback running (a collection, a frame restore, or a
deopt resume that rewound `roots`).

⚠ **`argc = n - top - 1` underflows on exactly this input.** With `top >= n` that is a
usize wrap, so the panic above is the *lucky* outcome — `root_at` happened to be evaluated
and bounds-checked. A build where the subtraction is reached first takes a huge `argc` and
loops `root_at(top + 1 + k)` over it. Worth a defensive check regardless of the root cause.

**What is known.** Both entries are 2026-08-20 11:10:13 and 11:10:17 UTC, on a scheduler
worker. **Not** caused by that session's changes: the region was last touched by an unrelated
refactor and the session's only `gc.rs` edit (the `form_pos` capacity shrink) is elsewhere in
the file and does not touch `roots`.

**Not reproduced:** the message-shape probes, `pingpong` x3 and `supervisor` x2 all ran clean
afterwards, and the full suite has passed 996/996 repeatedly since.

**How it was found, and the reason it is worth chasing beyond the crash itself.** It surfaced
while investigating why a tagged-tuple `receive` matcher is permanently `BAILED` (it runs on
the VM at 454 ns where the keyword matcher runs natively at 59 ns — the `pingpong`/`ring`/
`supervisor` latency gap). With every lowering refusal now traced (`BROOD_JIT_BAIL_TRACE`,
commits 5ab3c122 + 60c0958b), that arm is refused by *none* of them — which leaves
`codegen_poisoned`, i.e. a swallowed codegen panic marking every later arm BAILED with no
attempt and no trace. The bailed matcher's frame is **nslots=8** and the bad index is **9**.
That is suggestive, not established — but if the two are the same fault, fixing it also
recovers the latency row.

**Next step.** Reproduce with the tripwires armed (`RUSTFLAGS="-C debug-assertions=on"`,
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`) on spawn+tagged-tuple workloads; and add the
`top < n` guard so the underflow cannot turn a bounds panic into a wild loop.

## KI-47 — the tree-walker suite crossed the 1 GiB memory backstop ✅ FIXED 2026-08-19

**Not the three tests it named.** The `differential (tree-walker)` job failed on every run from the
ADR-230/231 stdlib-namespacing merge onward, always reporting the same three `adversarial_test.blsp`
cases (1 MB string / 30 000-element cons list / 10 000-entry CHAMP map) dying on a catchable `E0043`.
They are simply the cases that happen to be running when the **suite's** process-wide total crosses
the cap — a threshold failure blames whoever is holding the parcel.

**Regression window, pinned:** green through run **32141480911**, red from **32221892676** — the
first run after the namespacing merge. The mechanism is module count: the refactor split the stdlib
into new `string` and `file` modules, and module loading costs memory super-linearly here (see the
module-load-scaling note: RSS ≈ 45× source bytes).

**Measured** — in-language suite, debug build, `BROOD_VM=0`:

| | bytes | vs cap |
|---|---|---|
| local | 1 145 412 425 | 6.7% over |
| CI (same job) | 1 149 317 883 | 7.0% over |
| cap | 1 073 741 824 | — |

The VM arm stays under and was never affected; the tree-walker allocates far more by design, so it
crosses first. Runner-reported peak was 996.6 MB.

**Fix:** `TEST_DEFAULT_SOFT` 1 → 2 GiB, `TEST_DEFAULT_HARD` 2 → 3 GiB. Justified by the cap's own
documentation — a *"host-survival backstop, not a working-set budget"*, sized to *"never trip on
legitimate parallel load"*. 1.145 GB is the working set; a real runaway, that same comment notes,
*"heads to many GB"*. Verified: `brood_suite_passes` under `BROOD_VM=0` passes clean (963 s, exit 0),
and CI run 32247618122 is green on all five jobs.

**Tried and rejected, recorded so it is not re-attempted:** `hibernate` after each isolated test in
`run-unit-fresh`. It shrinks only the *calling* process's slabs — a genuine **148×** on a
51-iteration microbenchmark (25.3 MB → 171 KB retained, peak 30.4 MB → 6.9 MB) — while this cap
counts process-wide allocation across every green process and the shared regions. Real effect, wrong
scope. An earlier attempt at `gc-collect` was even further off: it rested on a stale comment in
`std/tool/test.blsp` claiming the runner only collects at "the depth-1 eval safepoint", which
**ADR-061 made false** — `eval/mod.rs` collects at *any* eval depth, so the collector was already
running there.

### ✅ The open question below is ANSWERED (2026-08-20): legitimate growth, not a regression

**KI-47's stated mechanism is wrong, and the 4.8× was an apples-to-oranges comparison.**

**Module count is not the driver.** The entry blamed the stdlib split ("module loading costs memory
super-linearly here"). Tested directly — total source held constant at ~40 KB, module count varied
1 → 12 → 48 → 120 — peak went 11.23 → 12.59 → 15.13 → 18.59 MB: **120× the modules costs 1.65×**, a
marginal ~60 KB per module. All 89 stdlib modules therefore carry ~5.5 MB of per-module fixed cost,
against the +905 MB that needed explaining. The 2026-08-06 module-load entry was misread: its
*quadratic* was in **time** (the `*features*` `member?` walk, fixed by ADR-216), and it explicitly
found memory **not** to be per-module ("10 big functions vs 100 small ones at equal line count costs
the same or more"). `RSS ≈ 45× source bytes` is linear in *source*, and splitting a file into more
modules barely changes source bytes.

**The namespacing merge did not cause it.** `098a3316` (the commit before ADR-230) built in a
worktree and run through the identical harness, release, both arms:

| arm | pre-merge (098a3316) | HEAD (305d85d4) | change |
|---|---|---|---|
| VM | 424.6 MB | 443.1 MB | +4.4% |
| tree-walker | 685.6 MB | 612.7 MB | **−11%** |

On the tree-walker — *the arm that actually went red* — HEAD uses **less** memory than before the
merge. The merge was the commit that crossed the threshold, exactly as this entry suspected, and
none of the growth is attributable to it.

**Where the 4.8× actually came from: three confounds, not growth.** The ~240 MB baseline dates from
**2026-05-30** (`4e801546`); the 1145 MB from 2026-08-19. Different *engine* (measured today: the
tree-walker costs **1.38×** the VM on the same suite and build), different *build* (debug vs
release), and three months of suite growth in between. Compounded, they cover the multiplier without
a regression.

**And it has since receded.** Same harness this entry used — debug, `BROOD_VM=0`, `brood_suite_passes`
— measured at HEAD, two samples: **726.7 MB and 757.9 MB** runner peak (mean ~742 MB, 4.3% spread)
against this entry's **996.6 MB** on 2026-08-19. That is **−24% to −27%**, i.e. back under the
*original* 1 GiB soft cap.

**Recommendation: keep 2 GiB soft / 3 GiB hard.** At ~742 MB the current margin is ~2.6×, near the 4×
the original sizing intended; reverting to 1 GiB would leave ~1.2× and re-trip on the next growth.
The number was right — only its *rationale* was wrong, and that is what this note corrects.

⚠ **Open question this fix deliberately does not answer.** The rationale it replaced claimed the
suite *"peaks ~240 MB under collection"*. The measured peak is ~4.8× that, so the raise restores a
~2× margin rather than the intended 4×. **Whether that growth is legitimate (the suite and the
module count both grew) or a regression is unmeasured** — the namespacing merge is the commit that
pushed it over the line, not necessarily the one that caused the growth. Do not read KI-47's closure
as evidence the growth is fine. The tool for the next person: `BROOD_TRACE_PROMOTE=1` ranks what
enters the append-only shared RUNTIME region, which the local collector cannot reclaim.

**Two diagnostic traps this one set, both of which cost real time:**

1. The failure list and the *slow-test* list sit next to each other in the runner's output. Reading
   a slow-test line as a failure sent the first hours of triage at `parse-number-fxx`, which was
   never failing.
2. `nest test` (release) and `brood_suite_passes` (debug, via the Rust suite binary) are **different
   harnesses with different allocation profiles**. The former passed 4652/4652 while the latter
   failed. Reproduce CI with CI's harness, not the convenient one.

## KI-65 — `take-while` / `drop-while` silently ignored a vector ✅ FIXED 2026-08-27

**Symptom.** `(take-while pred [1 2 3 1])` answered `nil` and `(drop-while pred [1 2 3 1])`
answered `[1 2 3 1]` — for every vector and every string, whatever the predicate said. A
wrong VALUE, not an error, so nothing downstream could notice.

**Cause.** Both loops guarded on `(pair? coll)`, which is false for a vector and a string, so
they walked zero items and returned the accumulator (`nil`) or the input unchanged. Every
neighbouring function — `take`, `drop`, `%take-acc` — guards on `empty?`, which is
collection-generic, so the inconsistency sat inside one file between adjacent definitions.

**Why it survived.** `tests/prelude_seq_test.blsp` and `tests/sequence_test.blsp` both cover
these, on LISTS. The prelude's own two callers (`prelude/tools.blsp`, splitting a parameter
list on `&`) also pass lists, so nothing in the tree exercised the broken path.

**Found by writing a documented example.** The probe that produced the value for the
docstring returned `nil` where `(1 2)` was obvious, which is the whole argument for
`tests/doc_examples_test.blsp`: an example is evaluated, so a wrong one cannot be committed,
and writing one puts a second pair of eyes on behaviour nothing else was checking.

**Fix.** `pair?` → `empty?` in both guards. A string now raises a clear `first: expected
list, vector, set, map or bytes` rather than answering wrongly. `drop-while` returns `coll`
itself when nothing is dropped, matching `(drop 0 [1 2 3])` → `[1 2 3]`.

## KI-69 — two `jit_plan` guards failed on every `main` push ✅ FIXED 2026-08-27

**Symptom.** The `differential (tree-walker)` CI job had been red on every completed run since
KI-64's fix landed, and the run list did not show it: each push cancelled the previous run, so
the page was a wall of `cancelled` with one `in_progress` on top and no visible red.

**Cause.** KI-64's fix added two guards —
`block_argument_spills_never_reach_the_deopt_journal` and
`the_block_argument_want_is_clamped_to_the_reserve` — and both assert over **VM-compiled**
arms via `dbg_compiled_arms()`. That job runs `BROOD_VM=0`, the tree-walker, where nothing is
compiled at all: the first guard inspected 0 chunks and the second found no arm to clamp.

**Why they failed rather than passing hollowly.** By design, and it is the reason this was
findable. Both refuse a vacuous green — `only {checked} lowerable chunks inspected — a green
result would mean nothing` — instead of reporting success over an empty set. A guard that
passes when it examined nothing is the failure mode [[KI-68]] is entirely about; these two
took the opposite choice and were loud.

**Fix.** Pin the tier: `set_forced_ceiling(Some(Tier::Native))`, which is what
`compile/tests.rs` has carried for its two native tests since ADR-222 made the ceiling
coherent (`BROOD_VM=0` and `BROOD_NO_JIT=1` are aliases for ceilings 0 and 1). The guards are
new — 2026-08-26 — and simply missed the pin every other tier-sensitive test already had.
Reproduced locally under `BROOD_VM=0` before and after.

**Guard.** The two tests themselves, now that they run at a ceiling where there is something
to inspect; their own anti-vacuity assertions are what make that meaningful.

## KI-68 — the fuzz-differential gate was comparing dead programs ✅ FIXED 2026-08-27

**Symptom.** None, which is the point. `python3 stress/fuzz_programs.py --seeds 20` printed
`seed N ok (exit=1)` twenty times and then `---- fuzz: 20 seeds, all configs agree`, exit 0.
The gate CLAUDE.md prescribes as part of proving the tree green had not compared a working
program in weeks.

**Cause.** The generator emits Brood source from Python string literals. The v0.9–v0.13
namespacing waves retired essentially every name it wrote — `(table)`, `rem`, `quot`,
`min`/`max`, `bit-and`/`bit-or`/`bit-xor`, `table-get`/`table-put`/`table-incr`/`table-count`,
`println`, and the linear-map whitelist `map-int-add`/`map-get`/`map-count`/`map-dissoc`.
The first form of every generated program is `(def t (table))`, so every program died on
line 1 — **identically in all four configs**. The differential compares config against
config, so identical death reads as agreement.

**Why nothing caught it.** Three independent gaps, each sufficient on its own:

1. The generator is **Python**. `nest check`, the `.blsp` suite, `make check-stress` and
   `scripts/stale-names.sh` all look at `.blsp` files; none of them reads a `.py` that
   *emits* Brood. This is the same class as the Rust-embedded Brood in
   `crates/lisp/tests/*.rs` — see [[rename-wave-checklist]].
2. `run_one` captured **stdout only**. `unbound symbol` goes to stderr, so the diagnostic
   that named the problem was discarded before the comparison ever saw it.
3. The `ok` line prints the last line of the compared value, which for a dead program is
   the string `exit=1`. It was on screen, twenty times per run, and reads as success.

**Fix.** Two parts, and the second is the durable one:

- The names are updated to the current spellings, verified by running the generated corpus:
  60 seeds produce 0 unbound diagnostics and every config agrees on real digests.
- **Liveness is now asserted.** `run_one_full` returns stderr beside the comparison string.
  An `unbound symbol` in a *generated* program is a hard failure — the generator writes the
  calls itself, so an unbound name there is never legitimate — and it reports the dead names
  rather than the seed alone. Beneath that, a run in which **not one seed** reached a clean
  exit fails as `NOT ONE ran to a clean exit — the corpus is dead, not the engines agreeing`.

**Sabotage-verified in the original shape.** Reverting `(table/new)` to `(table)`:

```
DEAD PROGRAM seed=1 (stress/fuzz_out/fuzz_1.blsp kept): the generator emits names this
build does not bind: table
---- fuzz: 3/3 seeds DIVERGED
```

versus `---- fuzz: 3 seeds, all configs agree (3 ran clean)` on the fix. Reverting
`"math/rem"` to `"rem"` is caught too, by the *other* arm — a program whose dead name sits
in an untaken branch still exits 0, and `check_soundness` flags it as a checker false
positive. The two checks are complementary, not redundant.

**The reusable lesson.** A differential gate proves that N engines *agree*; it proves nothing
about whether they agreed on anything. Any harness whose pass condition is "the sides match"
needs a separate assertion that the sides did real work — the same shape as KI-39's silent
annotate step and KI-62's image that installed nothing. When a corpus is generated rather
than checked in, that assertion has to live in the generator.

## KI-70 — the checker never looked inside a vector or map literal ✅ FIXED 2026-08-27

**Symptom.** `nest check` silent on code that raises on its first execution:

```lisp
(defn runnable (code)
  [:textarea {:rows (str (max 2 (count (string/split code "\n"))))} code])
```

`max` moved to `math` in the ADR-227 wave, so this is `unbound symbol: max` the moment the
page renders. hive shipped it, `bin/ci` passed (`nest check`, `nest format --check`,
`nest test`, and the `nest run --for` boot check all green — the boot check does not render
`/docs`), and the only way to see it was to load the page.

**Cause.** One line. `check_into_inner` began:

```rust
let Value::Pair(_) = form else { return };
```

A vector or map **literal** is not a `Pair`, so the walk ended there. Everything nested
inside `[…]` or `{…}` — arbitrarily deep — was invisible to *every* lint, not just the
unbound one. That is the entire Hiccup style: hive's web layer, `std/editor/*`, every
render-op and every UI spec in the language.

**Why it outlived KI-67.** KI-67 was the same *shape* one level in: `try` bodies were
reached by the walk and then discarded. Its sweep found four dead `project-*` call sites in
`std/tool/mcp.blsp` and fixed them. The fifth, `project-all-files` in the `callers` tool, sat
inside `{:references …}` — a map literal — so the walk never arrived, and no amount of
fixing the suppression rules could have surfaced it.

**Fix.** Descend, before the `Pair` bail:

- `Value::Vector` → walk every element.
- `Value::Map` → walk every **key and** value (a computed key is evaluated too).

**Why this cannot false-positive**, which is the only reason it is safe to turn on for a
checker whose CI gate rejects any warning:

1. The checker runs on **macroexpanded** forms. A `match` pattern vector (`([a b] …)`) has
   already been lowered to `let`/`if` binders by the pattern compiler, so no binder vector
   survives in value position for the walk to misread as references.
2. `quote` / `quasiquote` / `comment` return at `SpecialHead::SkipBody`, which is reached at
   the *head symbol* of the enclosing `Pair` — so quoted data never reaches this code.
3. The generic operand recursion below is already gated on `head_is_macro`, so a literal
   passed to an unexpanded macro is still left alone.

Confirmed empirically, not just argued: the CI gate `nest check std/**/*.blsp
tests/**/*.blsp` (held at zero warnings since 2026-07-31) reported **one** warning on the
first run with the fix, and it was a real dead call site.

**The real find it produced immediately.** `std/tool/mcp.blsp`'s `callers` tool called
`project-all-files`, a `defn-` in `std/tool/project.blsp`. The tool raised `unbound symbol`
on every invocation. `project-all-files` is now the public, de-stuttered `project/all-files`
beside `project/source-files`, the same remedy KI-67 applied to its four.

**The reusable lesson.** A lint that is *suppressed* somewhere leaves a trace you can grep
for; a lint that is never *reached* leaves nothing at all. When a checker has a
"return early if this is not the shape I expect" line, that line is a silent coverage
boundary — and the shapes on the far side of it are exactly where nobody is looking.

---

## KI-72 — a module materialised from the stdlib image was callable before it was complete ✅ FIXED 2026-08-28

*Filed as "the stdlib image cannot be default-ON yet: a require-stall storm" — that framing was the symptom, and the two sections below record it as it was understood. The cause was neither a stall nor the require protocol; see "the children DIED", further down.*

**Symptom.** With the stdlib image installed at boot, `autoload_race::racing_the_first_call_into_string_is_sound`
exceeds nextest's 120 s cap during `make test`. It passes **12/12 in 0.5 s** when run alone, and
passes under `cargo nextest run --test autoload_race` alone — only the full parallel suite trips it.

**Repro, and it is a clean one.** 12 parallel copies of the test binary at `--test-threads=4`,
90 s timeout:

```
image ON  : 12 of 12 timed out
image OFF :  0 of 12 timed out
```

**Cause (understood; the image is the amplifier, not the fault).** ADR-256 moved the image
branch's `(provide key)` to run AFTER the module's require-edges are replayed. That ordering is
required: providing first publishes a module whose dependencies are still missing, and a racing
process then dies on `unbound symbol: rand/token` — 112 of the 157 failures ADR-256 fixed were
exactly that. But it also means a module stays **unprovided** while it recursively requires its
whole edge set, and any other process that wants it during that window takes this path:

```lisp
(contains? *features-loading* key) (%require-await key 1000)
...
(defn- %require-await (key n)
  (cond (contains? *features* key) key
        (> n 0) (do (sleep 5) (%require-await key (dec n)))
        else    (%require-force key)))
```

— a **5 ms x 1000 poll**, i.e. up to 5 s, before giving up and force-loading. Nothing hangs
permanently. But the image makes many more modules materialise in a burst, so more processes
land in that window at once and the 5 s stalls compound past the cap.

**Not fixed by reverting the ordering.** That trades this for the 112 unbound failures, which are
wrong ANSWERS rather than slow ones. The fix is the poll: a waiter should block on the loader
finishing (a completion signal) rather than sleep-and-recheck, so the wait costs the load's real
duration instead of a 5 ms quantum.

**Measure both arms.** The synthetic 12x4 load stresses something that pre-dates the image: at
that parallelism the default (image off) still showed **4 of 12** over 90 s in one run. A fix
should move both numbers, and a claim that the image is safe to default-on needs the image arm at
parity with the no-image arm, not merely under the cap.

### Re-characterised 2026-08-28: the wait is UNBOUNDED, and the repro is far cheaper

Two corrections to the account above, both measured.

**1. "Nothing hangs permanently" is wrong.** The 5 ms x 1000 poll bounds `%require-await` at
~5 s, so the entry above reasoned that the suite could only be *slow*. It is not. With the
image installed, a root blocked in `reduce`+`receive` over 24 spawned children:

- outlasted a **30 s** `after` clause and still lost a reply (the whole 30 s elapsed — it did
  not recover at 5 s);
- was caught by a watchdog green process reporting **`backlog=0` continuously from t=20 s to
  t=30 s** — the root's mailbox is EMPTY, so the reply was never delivered. This is not a
  message delivered-and-unmatched, and not a slow poll;
- under gdb, sat in `receive_match` -> `wait_for_message` with **all 12 scheduler workers idle**
  in `wait_timeout` on the run queue. Nothing was runnable — so no child was merely slow.

The watchdog kept ticking throughout, so the scheduler itself is alive. A `send` (or a child)
is genuinely lost.

**2. The repro no longer needs 12 parallel copies over 90 s.** One process is enough:

```
BROOD_STDIMAGE=1 ./target/debug/deps/autoload_race-* --test-threads=4     # 5-6 of 9 hang
```

against a 25 s timeout, where each test alone passes in ~105 ms. Rates measured over 8 runs
each: `--test-threads=1` 3/8, `=2` 4/8, `=4` 5/8. **Concurrency between runtimes is not
required** — sequential tests in one process hang too — but more than one `Interp` in the
process is: a single test, run alone, has never hung.

**Ruled out** (each with a measurement, none of them the cause):

- the mailbox park/notify in general — a `pure` variant (children send a constant, no
  `require`, no image) never lost a reply;
- `%registry-member?` vs `contains?` staleness in the poll — fixed separately (that WAS a real
  defect: a waiter polling the cached global read could miss a racing `provide`, sleep its
  whole budget and then force-load a module that was ready), but it does not fix this;
- pid collision across runtimes — concurrent `Interp`s get distinct pids;
- `ensure_ctx` thread-local reuse — six sequential `Interp`s on one thread all report
  `#<pid nonode/1>`, because the root `Ctx` is cached per THREAD rather than per runtime, and
  the second runtime's root is therefore the first's mailbox. **This is a real design smell
  worth fixing on its own**, but it is not this bug: four sequential same-thread `Interp`s
  each running the full fan completed 6/6.
- `BROOD_NO_RECV_MARK` / `BROOD_NO_HANDOFF` / `BROOD_NO_MSGTAG` — no arm separated from
  baseline at n=12 (5, 4, 2, 6 losses respectively; the spread is noise at that sample size).

**A warning for whoever picks this up.** The bug is timing-fragile and *every* in-language
observer moved it. Collecting the child pids (`spawn-many`) instead of discarding them
(`dotimes`) suppressed it twice; adding a `%list-processes` watchdog suppressed it (0/48);
an `after` body larger than `(cons :LOST acc)` suppressed it. An earlier A/B here concluded
that a `receive` nested inside the native `reduce` builtin was the trigger (3/10 vs 0/10
for a Brood-level loop) — **that conclusion did not hold**: under parallel load the plain
loop lost a reply and the nested one did not. Do not trust a shape comparison at n=10 on
this bug. Use the unmodified `autoload_race` binary as the repro and observe from gdb.

### 2026-08-28, later: three fixes landed, NONE of them closes this

Fixed and pushed, all found while chasing this bug, none of them its cause:

- **the wake was an either/or** — `deliver` re-queued a parked green process *or else*
  notified the condvar, and a green process in a native-nested `receive` is reachable by
  both, so a present `waiter` suppressed the notify. `wake_for_timeout` had no notify at
  all. Now `mailbox::wake_both` signals both, always.
- **`%require-await` polled** — replaced with the `code_server` model described below.
- **the root ctx outlived its `Interp`** — `ensure_ctx` cached it per THREAD, so a second
  `Interp` on one thread inherited the first's pid *and mailbox*, and a queued
  `Payload::Local { slot }` then indexed the new runtime's heap at the old one's index.

**Measured, interleaved, with the image verified live in both arms: no change.** Baseline
and fixed are indistinguishable, and the baseline itself swung between 9/20 and 13/20 across
runs of *identical* code. Do not read the fixes as having narrowed this.

**A much cheaper repro than the one above.** The hang is one test, not the suite:

```
BROOD_STDIMAGE=1 ./target/debug/deps/autoload_race-* \
    --exact racing_the_first_call_into_string_is_sound --test-threads=1     # 8 of 12 hang
```

One test, one thread, one `Interp`, a 12 s cap, no parallelism at all. Across whole-binary
runs it is nearly always `racing_the_first_call_into_string_is_sound` that hangs (sometimes
`seq` as well), never `boot_loads_no_library_feature`.

**Two traps that cost hours here; read these before measuring anything.**

1. **`stdlib-id` embeds the git sha, so COMMITTING invalidates the image.** Commit while
   investigating and the amplifier silently switches off: the baseline then measures 0/12
   and any fix looks perfect. Every measurement in this session that read 0/N was this. Check
   for `[image] install: N sections` — and note the line is prefixed by libtest's `test … `
   output, so a `grep -E '^\[image\]'` anchored at line start finds nothing and looks like
   a clean run.
2. **Every in-language observer moves or suppresses it.** Collecting the child pids
   (`spawn-many`) instead of discarding them (`dotimes`) suppressed it; a `%list-processes`
   watchdog suppressed it (0/48); enlarging the `after` body suppressed it; and so did a
   *Rust* watchdog thread that does nothing for its first 3 s. Reproduce with the unmodified
   binary and observe from gdb.

**What the state looks like when it is stuck** (gdb, single test, single thread): the root
is in `receive_match` → `wait_for_message` on the mailbox condvar, inside `range_reduce_slow`;
**all 12 scheduler workers are idle** in `wait_timeout` on the run queue; the JIT thread is
idle. Nothing is runnable, and the root's mailbox is empty. So the 24 children have all
finished or never ran, and no reply is in flight — consistent with a lost `send`, a child that
died silently, or a child that never got scheduled. Distinguishing those is where this stands.

**Ruled out since**, each with a measurement: the mailbox park/notify in general (a `pure`
variant with no `require` never loses a reply); pid collision across runtimes; the
`ensure_ctx` inheritance above; and `BROOD_NO_RECV_MARK` / `BROOD_NO_HANDOFF` /
`BROOD_NO_MSGTAG`, none of which separated from baseline. An earlier A/B concluding that a
`receive` nested in the native `reduce` was the trigger (3/10 vs 0/10 for a Brood-level loop)
**did not hold** — under parallel load the plain loop lost a reply and the nested one did not.

### How Erlang does it — the `code_server` model

Brood's `%require-await` polls; OTP does not, and the difference is the fix. From
`lib/kernel/src/code_server.erl`, in its own words:

> we queue loaders for a given module and either reply to them or run them if a previous
> loader succeeded.

`schedule_or_run_loader/4` is the whole mechanism: if the module is already in `loading`, the
requester is appended to that module's waiting list and the server answers `{noreply, ...}` —
the caller simply stays blocked in its `gen_server:call` receive. When the in-flight load
finishes, `run_loader_next/2` pops the next waiter and replies to it. Three consequences
Brood does not currently get:

1. **A waiter's cost is the load's real duration**, not a 5 ms quantum times however many
   rounds — it wakes on a message, O(1).
2. **There is no force-load fallback.** The claim is authoritative and only the claim holder
   can release a waiter. Brood's `%require-force` after 1000 rounds is a *second* loader for
   the same module — precisely what turns a slow load into a duplicate one.
3. **Deadlock is detected, not timed out.** If the module's `on_load` is being run by the
   requesting process itself, the server replies `{error, deadlock}` immediately.

The claim itself is not the gap: Brood's `:assoc-new` test-and-set inside the registry lock is
equivalent to the code server's serialisation. It is the *wait* that differs.

**The deeper divergence, and it is the one this bug lives in.** In BEAM everything is a
process and there is exactly ONE park/wake path — `erts_queue_message` enqueues under the
message lock, then:

```c
erts_proc_notify_new_message(Process *p, ErtsProcLocks locks)
{
    erts_aint32_t state = erts_atomic32_read_nob(&p->state);
    if (!(state & ERTS_PSFLG_ACTIVE))
        erts_schedule_process(p, state, locks);
}
```

The wakeup is recorded as a **persistent bit in the process state** (`ERTS_PSFLG_ACTIVE`). A
state flag latches; whatever the interleaving, a process made ACTIVE will be run.

Brood has two paths, chosen by whether a green process is parked:

```rust
st.push(env);
if let Some(proc) = wake_parked(&mut st) { drop(st); wake_enqueue(proc); }
else { mb.cv.notify_one(); }   // wake the root thread, if it's blocked in receive
```

A condvar notify **does not latch** — delivered with no waiter, it is silently discarded. The
root/file-runner thread is a hybrid BEAM has no equivalent of: a mailbox owner that blocks an
OS thread on a condvar instead of being a schedulable green process. `wait_for_message` does
hold the state lock across its check-then-wait, which closes the obvious window, so this is
not yet a proven mechanism for the loss — but it is the structural difference, and unifying
the two wake paths (or latching the wakeup in the mailbox state the way `ERTS_PSFLG_ACTIVE`
does) would remove the whole class.

### FIXED 2026-08-28 — it was never a stall. A section published its entry points too early

**It is not a hang, a lost wakeup, or a lost message.** Every prior account here — the
5 ms poll, the condvar that does not latch, the `code_server` model, the two wake paths —
was chasing a symptom. The root is an **unbound symbol in a child**, which presents as the
root hanging because the test's `reduce` waits for 24 replies and one child died before
sending its own.

**What found it: three counters and a signal handler.** Every in-language observer moved or
suppressed this bug, and `ptrace_scope` forbids attaching gdb to a process the shell did not
fork. What worked was relaxed atomics on paths that run once per process (`spawn`, exit,
`send`) plus a **SIGTERM handler** that writes them with `libc::write(2, …)` — `timeout`
already sends SIGTERM at the cap, so the readout happens only once the process is *already*
stuck and cannot perturb the race. Three readings, in order:

- `spawned=24 exited=24` — no child is parked, none is missing;
- `delivered == sent` exactly — **nothing is lost in transit**; and
- `sent-to-root` 16–23, never 24, with `done + err = 24` and `sent-to-root == done`.

So children were *crashing*, and the shortfall was exactly `err`.

**Why "no child prints died" was believed.** It does print — `pool.rs` has an `eprintln!`
for exactly this. **libtest captures a test's stderr and discards it for a test that never
completes**, so the message was written and thrown away. With `--nocapture`:

```
process 9 died: unbound error: unbound symbol: string/whitespace?
```

**The mechanism** (ADR-279). `blank?` is public and carries an ADR-246 autoload stub;
`whitespace?` is `defn-` and is called from `blank?`'s body. An image section defines its
globals one at a time and each define publishes immediately, so installing the real
`blank?` **removed the stub** — the one door that routes a racing caller into `require-one`
and makes it wait — while `whitespace?` was still unbound. The source path cannot produce
this: `load` evaluates in file order, where a helper precedes its caller.

**The fix:** a section defines names with no current binding first and names that already
have one (i.e. the stubs) last. Sabotage-verified: 9 of 12 hang with the deferral disabled,
**0 of 24** with it enabled.

**Acceptance load** (the one this entry opened with — 12 parallel copies at
`--test-threads=4`, 90 s): **image ON 0/12, image OFF 0/12**, against 12/12 vs 0/12 before.

### A third trap, worse than the two above: the amplifier switches off *by itself*

Trap 1 said "committing invalidates the image". It is broader and it invalidated real work:
`BROOD_STDLIB_HASH` is a content hash over **every `std/**/*.blsp`**, so *any* edit to std
— including one made by somebody else while your measurement is running — changes the id,
no image matches, and the image arm silently becomes the no-image arm. During this session
the tree was being edited in parallel and a 12-run measurement read **0 of 12** purely
because of it; the same runs read 9 of 12 once the image was rebuilt.

This very likely explains the entry's own "baseline and fixed are indistinguishable, and the
baseline itself swung between 9/20 and 13/20 across runs of *identical* code".

**So: verify the image inside the same command that measures.** Not once at the start —
every run. `BROOD_IMAGE_TRACE=1` and count `install: N sections`; `N` must be a number and
not `nil`, and the loop should report how many runs saw it:

```
hangs: 0 of 24   (image live 24/24)
```

**Was open, now decided:** the image went **default-ON in v0.15.0** (`f114d01e`), opt out with `BROOD_NO_STDIMAGE=1` — verified empirically: with no flag set a `require` of `json` reports `install: 103 sections` and materialises from the image, and `BROOD_NO_STDIMAGE=1` emits no `[image]` line at all. The paragraph below is the reasoning as it stood before that call.

Parity is met on this load, but
that is one workload; flipping the default is a shipping decision and ADR-256's flip was
reverted once already.

**And the bar for that decision moved, in both directions, the same day.** Asked whether the
image should default on "except when running scripts", the measurement says the opposite: a
script requiring three std modules runs **46.5 ms → 36.2 ms**, a 22% saving it pays on
*every* invocation, where a long-lived process pays it once and amortises it away. Scripts
are the beneficiary, not the exception — and the exceptions that do exist are already
handled by construction (coverage stands aside; editing std invalidates the id).

What was missing was not a workload but a *proof*. Every divergence in this feature's
history was found by consequence, so ADR-280 added the differential — load every module
from source, load it from the image, require the resulting state to match. It found a
**sixth** on its first run: materialising dropped **privacy**, so every `defn-` in an imaged
module came back public. 1448 names diverged; 0 after the fix. Default-on is a decision
about one gate now rather than about five anecdotes.



### The guard was accidental until 2026-08-28, and it is probabilistic

`autoload_race` is what stands between this bug and a silent return, and until now whether it
ran on the **imaged** path — the only path the bug exists on — was an accident. It never built or
required an image, and nothing in `ci.yml` builds one. What *does* build one is
`image_matches_source.rs` (ADR-280), which writes it to `~/.cache/brood` — so coverage depended
on whether that case happened to run before this one, and nextest gives each case its own process
in no guaranteed order. In CI, with no image on disk, these races ran on the source path and
asserted nothing about the imaged one.

The differential does not cover the gap. It compares final **state** — name, kind, privacy,
declared signature — and proves the two load paths agree *once loaded*. This bug was not a state
divergence but an **ordering** one during install. A differential over end state cannot see it.

So `race_first_call_from_the_stdlib_image` was added, which builds the image in a **throwaway**
interpreter (`stdimage/build` `require`s every module into the calling heap, which would load the
very module whose *first* call is being raced), then installs it into a fresh interpreter and
asserts three things before racing: that the install returned non-nil, that `*std-image-file*` is
set, and that the module is **not yet loaded**. Any of those failing means the test would have
exercised the source path while reporting that KI-72 is still fixed.

**Sabotage-verified, and the first attempt was vacuous — which is the part worth recording.**
Reverting the deferral (`if false && kind != KIND_SIG && heap.env_get(global, sym).is_some()`)
and re-running gave, per 12 standalone invocations of a single case:

| arm | caught the sabotage |
|---|---|
| pre-existing (`…_string_is_sound`) | **6 of 12** |
| new (`…_from_the_stdlib_image`) | **4 of 12** |
| `cargo test` running all 5 cases in one process | **0 of 3 runs** |
| **`cargo nextest run` (what `make test` and CI use)** | **4 of 4 runs RED** |

Three things follow, and none of them is "the guard is fine, move on":

1. **The guard is probabilistic, not deterministic.** The race is intermittent by nature, so a
   single case catches a reintroduced bug roughly half the time. What makes it a usable gate is
   nextest running five cases in five processes: 4 of 4 whole runs went red. Do not reason about
   this guard as if one green case proves anything.
2. **Plain `cargo test` suppresses it entirely** — all five cases share one process, and the
   earlier cases warm the allocator, interner and JIT enough to close the window. CLAUDE.md
   already warns that `cargo test` has no per-test timeout; add that it cannot see this class of
   bug at all. **Reproduce with nextest, or with `--exact` on a single case.**
3. **The new arm is weaker than the old one** (4/12 vs 6/12) for a knowable reason: building the
   image in the throwaway interpreter warms the same process. That is the documented hazard of
   this bug — *every* in-language observer moves it — appearing inside the test written to catch
   it. It is kept because it is the only arm that runs on the imaged path *deterministically*,
   and the two together are stronger than either.

### Two accounts, one bug — and a merge that dropped one of them

This was root-caused twice in parallel on 2026-08-28, from opposite ends, reaching the same
mechanism. The merge (`cba50894`) then hit a conflict in this file and **kept only the weaker
account**, silently dropping the 83 lines above; they were restored by hand afterwards. Worth
recording because nothing failed — the code from both sides merged cleanly and every gate stayed
green, so the loss was invisible to CI. **A doc conflict resolved by "keep one side" loses
findings no test can miss.**

| | the loader account (authoritative, ADR-279) | the writer account (superseded) |
|---|---|---|
| **what is deferred** | names that already have a binding — the ADR-246 **stubs** | module-**privates**, emitted first by `%image-write` |
| **why the race is reachable at all** | overwriting the stub removes the door that routes a caller into `require-one` and makes it wait | not identified — framed as ordering alone |
| **coverage** | total: a public->public window is only reachable *through* a stub | partial: public->private only |
| **where it lives** | `startup_image.rs` (the loader) | `std/tool/stdimage.blsp` (the writer) |

The writer change was **reverted** once the loader fix landed: with it removed the repro is
**0 of 24, image verified live 24/24**, so it was redundant — and two half-mechanisms for one
bug is worse than one whole one.

The writer account also produced a claim that is now **withdrawn**: that privates-first was
"necessary and not sufficient", and that an **atomic** section install was the prerequisite for
default-ON, on the evidence of a static scan finding ~257 public->public calls whose caller
sorts first (`(global-names)` order is alphabetical). The arithmetic was right and the
conclusion was wrong: those windows are **not reachable**, because a racing process can only
enter a partially-installed module through a stub, and stubs now install last. Deferring is
enough; atomicity is not needed.

That scan is still a cautionary tale about static analysis over a Lisp. Its first run said
**757**, inflated twice over: `:year` matched the public `year` (the keyword colon was not
excluded), and `(defn- foo` parsed as a public named `-` (`defn-?` matched `defn`, then captured
the hyphen). Both errors pointed the same, scarier way, and only hand-checking one case caught
them — `epoch-ms->` "calling" `year` is really `(get ymd :year)` beside three `let`-locals named
`hour`/`minute`/`second`.

One measurement caveat, in fairness to the third trap above: the writer account's figures were
taken with the image verified **once before the loop**, not per run. Per-run verification is now
known to be required, so those figures should not be leaned on. The numbers in this section, and
the 0-of-24 above, were taken with it.

## KI-71 — `seq/remove-nth`'s argument swap was invisible to every gate ☑️ NOT A BUG 2026-08-27

Not a defect in brood — a note about the *class*, because this one cost the most time to
find of anything in the downstream migration.

`seq/remove-nth` moved to index-first, correctly: it was the one function in `seq` that
broke the collection-last convention, and its docstring says so. But a **reversed-args
change is invisible to every gate we have**:

- the arity is unchanged, so no arity error;
- no symbol is unbound, so `nest check` is clean;
- the type mismatch is advisory, and the call sites are polymorphic enough not to warn.

In bedit it surfaced as **seven** failures in `buffers_eval`, `hosted` and `tutor` that
looked like unrelated buffer-lifecycle bugs — "killing the last editable buffer leaves a
fresh `*scratch*`" returning the *old* buffer's text. The actual raise
(`<=: expected number, got vector` in `%take-acc`) happened inside `ed-kill-at`, where the
caller's error handling absorbed it.

**What made it findable** was reading the failing function's source and testing the
primitive directly, not any tool. **What fixed it** was `nest rename --swap`, which already
exists for this and rewrites `(f a b)` to `(f b a)`.

**The lesson for the next wave:** a reversed-args rename needs to be announced differently
from a moved name. A moved name produces an unbound symbol the checker will point at; a
swapped one produces a plausible wrong answer somewhere else entirely. Worth listing them
explicitly in the release notes under their own heading, since no gate will.

<!-- KI-70 addendum, 2026-08-27: a second call site of the same reversed-args change was
     found in brood-terminal (`seq/remove-nth tabs i`), presenting as one unrelated-looking
     test failure — "ctrl-d with two tabs closes one". Two repos, two different symptoms,
     one rename. `grep -rn 'remove-nth'` across the ecosystem is what found it; nothing in
     the toolchain would have. -->

## KI-73 — a prelude macro template is captured by a user module defining the same name ✅ FIXED 2026-08-28

**Symptom.** A module that defines an ordinary domain function silently gets wrong values out of
an unrelated language feature:

```lisp
(defmodule inventory)
(defn get (bag k) :CAPTURED)     ; a perfectly reasonable function to write
(defrecord point (x y))
(point-x (point 1 2))            ; => :CAPTURED, not 1
```

No error, correct arity, nothing unbound, and `nest check` says nothing.

**Cause.** A macro's quasiquote template is spliced into the *caller's* file, and a bare symbol
in it resolves against the caller's namespace before root. ADR-065's α clause auto-qualifies a
template's free references to the *defining* namespace, which fixes this for module macros — but
the prelude's defining namespace is root, which has no prefix, so prelude templates emit bare
names and are capturable. It is the general form of the `name` collision ADR-258 fixed for one
symbol: renaming the operation moved the hazard, it did not remove it.

**Fix (partial).** The `/name` root escape (ADR-236) pins a reference to root, and `defrecord`,
`for`, `defonce` and `with-err-str` now use it. That escape had to be *completed* first: it was a
resolve-time rewrite and `resolve` returns early at root, so an emitted `/get` reached the
evaluator literally and was unbound in any root script. `macros::strip_root_escapes` now handles
the root case, skipping `quote`/`quasiquote` subtrees — without that skip the prelude's own
templates get stripped at definition time and the capture returns (it did, for one build).

**`receive` needed two more moves, and both were structural rather than workarounds.** Its
expansion calls `but-last`, which lived in `std/prelude/seq.blsp` — concatenated *after*
`process.blsp` — so `receive` could not expand at prelude compile time; and `sleep`, whose body
*is* a `receive`, was defined 200 lines *before* `receive` existed, so its expansion was deferred
to first call. `but-last` moved to `core.blsp` (it needs only `reverse` and the kernel `rest`)
and `sleep` moved below `receive`. Neither is a shim; both put a definition where its dependency
order already required it to be.

**And the escape had to work at runtime too.** A macro defined *at the REPL or after its use
site* — `(defmacro await (t) \`(receive ([:reply ^~t v] v) (after 1000 :timeout)))` — expands in
the evaluator, which no resolve pass ever touches. `eval/mod.rs`'s macro-application site now
strips escapes on the way out, which is the runtime half of `macros::resolve`'s root case. Found
by `tests/syntax_finalization_test.blsp`, not by reasoning.

**A second emission shape, found a day later and live the whole time.** A macro need not
use a quasiquote at all — it can build its output with `(list 'head …)`, and the template
walker skips `quote` subtrees by design, so those heads were never scanned. Five prelude
macros did it, emitting `map`, `apply`, `mapv`, `current-ns` and `sig`. `defmulti`'s
`(list 'mapv '%identity-of 'args)` was the live bug: a module defining `mapv` broke every
multimethod it declared, dispatching on `:CAPTURED` instead of the record id. All escaped
except `sig` — see below. The gate now walks both shapes.

**`sig` looked unescapable, and the reason was the ordering, not the name.** `compile`
**expands before it resolves**, and `macro_head_id`'s root fallback did not understand the
escape — so `(/sig …)` was not recognised as a macro head, stayed unexpanded, and never
produced the `%register-sig` the checker collects. Every record's constructor and accessor
signature silently stopped being checked. Four checker tests caught it; without them it
would have shipped as a quiet loss of type checking, which is the worst shape a bug can take
in a checker.

The fix is at the lookup site: `macro_head_id` strips a leading `/` in its root fallback, so
a macro head is recognised at *expansion* time rather than merely rewritten afterwards. It is
general — every macro a template emits, not just `sig`. (The escape appeared to work on
macros already; that was the *evaluator* expanding them at runtime. The checker never
evaluates, so it never got that second chance — which is why only a checker test could find
this.)

**No name is reserved and no warning is added.** Reserving `sig`/`get`/`map`/`mapv` inside
modules would break ADR-166's one-sentence rule — reserved ⇔ it shipped with Brood, and in a
module it is yours — which is the escape hatch that makes namespacing worth having. With the
escape total there is nothing left to warn about; the gate asserts **zero** offenders.

**Guarded by** `tests/prelude_capture_test.blsp` — a static scan of every prelude `defmacro`
template plus three behavioural probes that define `get`/`reverse`/`bound?` and assert the real
value comes back. It pins `receive` as the known exception, so a *new* offender fails the build.
Sabotage-verified: removing `defrecord`'s escape fails both the scan and the probe.

## KI-74 — one `cargo test -p brood --lib` run reported a failure it would not name ✅ FIXED 2026-08-29

**What was seen.** A single run of the lib suite ended `test result: FAILED. 607 passed; 1
failed; 1 ignored` — and the run's output carried **no `---- <name> stdout ----` block and no
`failures:` list**, so the failing case named itself nowhere. Every subsequent run has been
green: **20 consecutive** clean runs (8 + 12) of the identical binary, `608 passed; 0 failed`.

**Why it is a watch and not an open bug.** Per the rule above, `⚠️ watching` requires that it
genuinely cannot be reproduced on demand, with the diagnostic armed and named. It cannot: 20
runs, no recurrence, and the one sighting is uninformative by itself. It is *recorded* rather
than dismissed because this repo's own history says a flake seen once is real until proven
otherwise (KI-36 took three sightings and twelve days; KI-39 took four).

**The one lead.** The suite has a test that deliberately touches the filesystem outside any
project — `introspect::tests::load_tooling_image_is_best_effort_outside_a_project` — and its
normal output in every run is a `Permission denied (os error 13)` note for
`/nonexistent/path/xyzzy/.brood`. Tests run on many threads and several build an `Interp`
that consults `~/.cache/brood`, so a shared-cache race is the most plausible shape. This is a
**hypothesis, not a diagnosis**: nothing has tied the sighting to that test, and the run did
not name a case.

**Diagnostic armed.** `make test` runs the suite through cargo-nextest, which runs each test
in its own process and **names the failing case** — the exact gap this sighting had. Re-run
under nextest (`cargo nextest run -p brood --lib`) if it recurs, and capture the full output
rather than the summary line; libtest's summary alone cannot answer it.

**REPRODUCED, NAMED, AND FIXED (2026-08-29).** Forty runs of the same command under a 4-core
spin load reproduced it at **1-in-40**, and full-output capture named it:
`eval::compile::tests::jit_tier_compiles_a_hot_arm_then_runs_native` panicking `the hot arm
should tier up to native code`. The cache-race lead above was wrong; the mechanism is a
**deadline**: the test polled 400 × 2 ms (~0.8 s) for the background compiler to land the
native code, which a loaded box misses — and libtest's single shared process queues every
*other* test's compiles ahead of this one on the same compiler thread, which is why nextest
(process per test, its own queue) ran 30-for-30 clean under the identical load, and why the
original sighting was libtest-only. The same hazard existed in
`vm_run_bc_runs_a_tiered_arm_via_the_hook`, one screen down.

**Fix.** Both polling loops now run against a 60-second wall-clock bound instead of an
iteration count — a compiler that never lands code still fails, on a bound a loaded box can
meet (KI-43's stopwatch lesson, applied to a compile instead of a kill).

**Guard.** The reproduction loop against the fixed tree: the same 40-run 4-core-spin loop
that produced 1-in-40 before the fix. (Re-shrinking the bound fails immediately under load —
the sabotage direction.) The "would not name" half was my capture cutting libtest's failures
list from the tail; full output names it fine.

**Also worth recording:** this entry had no index row — the file's own rule says both halves
are required, and nothing enforces index completeness. Row added with this close.

**Next step if it recurs:** get the case name, then decide. Until then this entry exists so a
second sighting is recognised as a second sighting rather than a first.

## KI-83 — the mono differential failed over a slow-test timing line ✅ FIXED 2026-08-29

**Symptom.** One `make test` run: `cli::mono_differential monomorphization_computes_what_the_
dynamic_path_computes` failed with `the two arms disagree — monomorphization changed an ANSWER`.
Read side by side, both arms end `92 tests, 92 passed, 0 failed (0 failed assertions, 4
isolated)`; the entire diff is one line present in one arm only:

```
  concurrent impl registration (KI-22) › no impl is lost when many processes register at once 13.9s
```

**Cause.** The test framework prints a per-test annotation for any test slower than
`*test-slow-ms*` (default 1 s) — informational, load-dependent. The differential runs the
ability suite twice (dynamic vs `BROOD_MONO=1`) and compares **raw stdout** after a
`without_timings` filter that stripped only lines containing `ms wall` or `Slow tests` — not
this per-test line. Under full-suite nextest parallelism, one arm's nested KI-22 stress case
crossed the 1 s threshold and the other's did not. Standalone, the same test passes in ~0.4 s;
the 13.9 s was contention, which is exactly when the annotation appears.

**Why it survived.** The filter was written against the outputs the author had seen — quiet
runs, where no test crosses 1 s and the annotation never prints. The line only exists under
load, and the differential had only ever run on a lightly loaded machine.

**Fix.** `without_timings` also drops any line whose last whitespace-separated token is a
duration (`13.9s`, `2ms`). A real divergence that manifests *only* in such a line is
theoretically maskable, but a failing arm already fails the `plain_ok`/`mono_ok` asserts
before the comparison runs, so the comparison only ever sees two passing runs' chatter.

**Guard, sabotage-verified** (offline, on the exact captured outputs from the failure): the
old filter leaves the two arms unequal (reproduces the false alarm); the new filter makes them
equal; and mutating one arm's summary `92 passed` → `91 passed` still diverges under the new
filter — so it cannot mask a real answer change in the summary. `cargo test -p cli --release
--test mono_differential` green after the fix.

**Numbering note:** filed as KI-82 in the fixing commit's message (`4d6c8ceb`); renumbered here the same day — a parallel session's KI-82 (the wasm playground recursion) was already cited upstream, and the process renumbers the newer entry. **Same species as [KI-80](#ki-80)** (a nested suite emitting load-dependent output that an
outer gate reads as signal), one gate over — and the general rule both point at: **a
differential must compare answers, not transcripts.** Anything a harness prints conditionally
on timing, load, or environment must be normalised out before an equality check is allowed to
mean anything.
## KI-82 — the hosted playground cannot run its own front-page example ✅ FIXED 2026-08-29

**What happens.** On `https://brood.fly.dev`, the snippet the front page ships as its example

```lisp
(->> (range 1 11) (map (fn (n) (* n n))) (filter math/odd?) (reduce +))
```

answers `recursion too deep: used 14021552 bytes of stack, over the 12582912-byte budget`, with
`{:fn %require-force-in} {:fn %require-force}` in the trace at `std/math.blsp:19`. So the
**auto-require of `math`** — inferred from the qualified `math/odd?` — recurses in the wasm
build.

**Wasm-only.** The identical snippet on the native 0.16.0 binary answers `165`.

**Not yet attributed.** The playground wasm is built during the hive deploy from `BROOD_REF`,
which moved `d3a2cdfa` → `d12dc5d0` (v0.16.0) that day, so the v0.16.0 bump is the prime
suspect — and ADR-290/291 is the part of it that touched `require`: it put qualified
`reflect/…` references into the **prelude**, and a qualified reference is what infers a
require.

**Ruled out:** the playground's own preload line (`crates/playground/src/lib.rs`), renamed in
the same wave. That is the *autocomplete* `Interp`, not the eval path.

**Fastest mitigation, if the site matters more than the diagnosis:** repin `BROOD_REF` to
`d3a2cdfa` in `hive/Dockerfile` and redeploy. The reference page reverts to documenting 0.15.0
with it.

**First thing to try:** reproduce under `wasm32` locally (`crates/playground`) rather than
through a deploy.

**FIXED (`b6706120`, same day) — and the attribution above was wrong in an instructive way.**
Not `require`, not ADR-290/291: `WORKER_STACK_BYTES` was a hard-coded 16 MiB (a native
worker's stack) while wasm runs on a ~1 MiB shadow stack in linear memory. The budget
(stack − 4 MiB margin) could never be reached — the host traps first — and the
"implausibly large ⇒ stale base" backstop only fires above a whole stack, so a bogus
13.4 MiB reading landed exactly in the gap: over the budget (raised) and under the backstop
(never recognised as stale). Both are now target-aware. Reproduced deterministically through
the page's own call sequence (`completions()` then `run()` — the second `Interp` on the same
thread is what leaves the stale `STACK_BASE`), verified to fail before and pass after on
native, lean-native and wasm32 + node. The commit also records a masking hazard: a first
attribution (the ADR-295 arms) made the symptom disappear by *shifting the prelude* enough to
nudge the bogus reading under the threshold — a sabotage run coming back green is what
exposed it.

**Residual, outside this repo:** the deployed site keeps the old wasm until the hive deploy
repins `BROOD_REF` ≥ `b6706120` and redeploys. This entry's runtime half is closed; whoever
owns the deploy closes the visible symptom.

## KI-81 — `BROOD_CONTRACTS=1` was unusable on a cold boot cache ✅ FIXED 2026-08-29

**Filed as an unreproducible one-shot panic. It was never a flake.** The trigger is
`touch target/release/brood`: the boot cache is keyed on the executable's **mtime**, so the
first run after any rebuild expands the prelude from source, and every run after that replays
the cache and never executes the macro bodies involved. Twelve "clean" runs were twelve warm
ones. Reproduces 100% cold.

**Three independent defects, all cold-only** (full reasoning in ADR-293):

1. `sig!`'s expansion-time code called `take`/`nth`/`map`/`range`/`count`, none of which exist
   yet at that point: the prelude concatenates `core → predicates → map → control → match →
   process → seq → string → tools`, and `sig!` lives in `core.blsp` while `take` is defined in
   `seq.blsp`. Now `%sig-take` / `%sig-nth` / `%sig-gensyms`, siblings of the `%sig-pos` that
   already existed for exactly this reason.

   **Corrected 2026-08-29:** this entry first blamed ADR-290/291 for moving `take` out of the
   bare namespace, and named `std/prelude/tools.blsp`'s `defmulti` as a second latent call
   site. Both were wrong. `take` is bound at root (`(bound? 'take)` → true) and never moved;
   the fault is purely load order. And `tools.blsp` is concatenated *after* `seq.blsp`, so
   `defmulti`'s bare `take` resolves and needs no fix. The fix above was right; the reason
   given for it was not.
2. The shim was `(let (orig name) (fn …))` — a closure over a **let-bound local** — which the
   prelude's freeze step rejects outright (`shared closures must capture the global env`). The
   original now lives in a gensym'd global, so the shim captures only globals.
3. `defrecord` emitted its constructor `sig` **before** the `defn` it rebinds, making every
   record in the language fatal under contracts; `std/io.blsp`'s `standard-port` took the boot
   down as soon as anything required `io`. Its accessor sigs already came after theirs.

**Why it rotted:** the mode had **no end-to-end test at all**. `crates/cli/tests/contracts_mode.rs`
is now that gate, and it cold-caches deliberately (`XDG_CACHE_HOME` at a fresh temp dir) —
without that it passes on a broken build, which is exactly what every other check did for the
entire period this was unusable.

### Recurrence 2026-08-30 — defect 3's shape came back 211 times, and the gate could not see it

The signature-adoption sweep put a `(sig …)` **above** its `defn` in 211 places across `std/`,
plus 7 in the prelude — the same forward reference as defect 3, and above-the-defn is the
natural place to write a signature, so this will keep happening. Every one of those modules
was unloadable under `BROOD_CONTRACTS=1`; `std/string.blsp` took the boot down through an
auto-derived import as soon as a program used `str`.

**`contracts_mode.rs` was green the whole time.** It proves the *prelude* boots and declares
its own sigs correctly — it never asserted anything about the other 400 files. A gate that
covers the place a defect was found is not a gate on the defect's *class*.

So the rule is now asserted where it lives: **`crates/lisp/tests/sig_placement.rs`** scans
every `.blsp` and fails on any `(sig NAME …)` that precedes a same-file definition of `NAME`,
naming the line to move. Textual, no interpreter, sabotage-verified. All 218 sites moved below
their definitions.

Two things fell out of fixing it:

- **`sig!` never handled `&optional`.** Read as fixed parameters, the marker itself counts as
  one, so `(sig pad-left (string int &optional string -> string))` armed a **4-arity** shim
  over a 2-3-arity function and every `(string/pad-left s 10)` became an arity error. It was
  unreachable before only because the module died earlier. The shim is now variadic and
  `apply`s just the arguments it received, so the callee's own defaults survive — passing an
  explicit `nil` for an absent optional would have silently changed answers. Covered in
  `contracts_mode.rs` (default kept, supplied value passed, contract still fires).
- **Two multi-line sigs were mangled by the bulk move** (`observe-minibuffer`,
  `lineedit-render-single`): the head line moved and its continuation stayed behind, leaving
  both files unparseable — and the *symptom* was three `unbound symbol` warnings in
  `tests/debug_test.blsp`, a file with nothing wrong with it, because `nest check` could no
  longer load what it imported. `nest format --check` names the unparseable file directly and
  is the faster read.

## KI-80 — `brood_suite_passes` flaked once under load, and the output was thrown away ✅ FIXED 2026-08-29

**What was seen.** One `cargo nextest run -p brood --test-threads 4` ended
`888 tests run: 888 passed (1 slow, 1 flaky)`, the flaky row being `brood::suite
brood_suite_passes` — failed on try 1, passed on try 2. It was the first run that included a
new CPU-heavy test (`arrow_subtyping_is_sound`, ADR-292).

**Why there is no diagnosis.** The failure output was discarded **at the terminal, not by the
tooling**: nextest names a flaky case and prints its `---- … stdout ----` block, and the
command piped the run through `tail -5`. So the summary line survived and the one thing worth
having — *which in-language case failed* — did not. That is exactly the trap already recorded
as `never-truncate-test-output`, and it is the substance of this entry: the tooling was not
the gap.

**Not reproduced.** Ten runs since, all `888 passed`: six loaded 4-thread runs with
`--retries 0`, three solo runs of `--test suite`, and one loaded run before the fix below.

**The one contributing factor found.** The new test rebuilt a `Ty` and recomputed a
denotation inside its inner loop — 1596 redundant rebuilds of each, ~2.5M times over.
Precomputing both took it from 3.4s to 2.0s. That is real contention removed from a
`--test-threads 4` window, though nothing ties it to the sighting.

**Why a watch and not an open bug.** It matches, verbatim, the class this binary's
`retries = 1` was added for — `.config/nextest.toml` records it: the in-language suite
contains timing-sensitive cases that talk to a local node, they run inside this single
wrapper case, and one blown deadline under a loaded runner reddens all ~1200 of them
(observed 2026-07-25, passing 3/3 standalone immediately after). The mitigation is already in
place and a pass-on-retry is deliberately reported as FLAKY rather than absorbed, which is why
this sighting was visible at all.

**Next step if it recurs:** capture the entire run to a file and read the failing case's
stdout block. Which `.blsp` case failed is the whole question; if it is a node round-trip
(`tests/remote_spawn_test.blsp` and friends) this is the known class and can be closed, and if
it is anything else it is a new bug that this sighting cannot distinguish.

**Second sighting, 2026-08-29 (same day), output kept this time.** During a deliberate
4-core-spin load test (the KI-74 hunt), `brood_suite_passes` hit its 300 s nextest cap
(`TRY 1 TMT [300.016s]`) and passed on retry (`TRY 2 SLOW [>60s]`) — reported FLAKY, not
absorbed. So the sighting shape is now confirmed: a **timeout under load**, not a wrong
answer, and the 300 s budget (5× the quiet-box 66 s) was exceeded only under full-core
adversarial contention no CI runner should see. The retry + FLAKY reporting is the designed
handling working. Stays ⚠️ only because the *original* sighting's output is gone; if a third
sighting is also a TMT line, close this as understood.

**DIAGNOSED AND FIXED (2026-08-29, third pass).** The second sighting's TMT try-1 stdout was
not thrown away this time, and it rewrote the entry: the dots stream contains **62 F's in
runs (46, 12, and singles) before the timeout** — the "timeout under load" was mass test
failures under load, with the cap merely hiding the names. Its stderr names the class:
dozens of spawned processes dying `unbound symbol: editor/serve/serve-manager` (and bare
`ui-run`) **after their file's `%isolate` rolled the globals back**. Three defects, each
fixed:

1. **74 hardcoded 1–3 s positive-wait deadlines** across 41 test files (`(after 2000
   :none)` on receives that *expect* a message) — the exact class `*test-wait-ms*` was
   created for, never swept. Under saturation a serve round-trip legitimately exceeds 2 s,
   and the serve/editor cluster fails in blocks. All 74 now use `*test-wait-ms*` (20 s);
   the collect-until-lull terminators and sub-second negative asserts were checked
   individually and left alone.
2. **`%isolate`'s reap "join" was a spin, not a wait** (`for _ in 0..10_000 { yield_now }`
   — a thread-yield hint the OS may ignore, burnable in microseconds while a parked
   victim's kill still needs a scheduler worker). On the ROOT thread — where `brood
   --test` runs `:isolated` units — this was **deterministic**: `tests/remote_spawn_test.blsp`
   failed 4/4 standalone (also at v0.16.0 — latent, not new), because the reaped
   `:remote-spawn` server was still registered when the next test's `serve-spawns` checked,
   so it declined to restart and every spawn request went to the corpse, silently. The join
   is now wall-clock-bounded (5 s) with yield-then-micro-sleep backoff. 6/6 green after.
3. **Retirement swept NAMES after removing the pid from REGISTRY**, so any join on
   REGISTRY-absence could return while the dead pid was still name-registered.
   `deregister` now sweeps NAMES first — the invariant is *REGISTRY-absent ⇒
   NAMES-absent* (the sweep in `retire_pid_tail` stays, idempotent, for the root-ctx
   path).

**Verified:** the full suite under the second sighting's own load (4-core spin + a
concurrent nextest loop) runs **green in 99 s** where it produced the 62-F TMT before;
`remote_spawn_test` standalone 6/6 (was 0/4); suite 1250/1250; gcstress green. The
original sighting's discarded output is permanently unknowable, but every mechanism the
kept output exposed is closed.

**Fourth sighting, 2026-08-31 — and the terminal trap repeated verbatim.** Under a
`make test-both` (KI-95 session), `brood_suite_passes` failed **both tries** on the VM
half (`Summary [511s] 1319 run: 1318 passed, 1 failed` per try, `Brood test suite
failed: error: 1 test(s) failed` — a named in-language failure, NOT a TMT) — and the run
was piped through `grep -E "Summary|FAIL|failed|passed"`, so the one line worth having,
the failing case's name, was discarded *again*, this time by the model driving the
session. Not reproduced with output kept: solo green, a full captured VM run green
(1319/1319, 163 s), a full captured `make test-both` green on both halves (161 s/244 s,
zero retries). Both-tries-failed distinguishes it from the retry-absorbed class above;
with the name lost it cannot be told apart from KI-98's full-suite-context family
either. The standing instruction survives another demonstration: **never pipe a suite
run through a filter — capture the whole run to a file, grep the file.**

## KI-75 — `compare` called unequal values equal, and `sort` inherited it ✅ FIXED 2026-08-28

**Symptom.** Sorting float data containing a `NaN` silently returned it *unsorted*:

```lisp
(sort [3.0 nan 1.0 2.0])   ; => (3.0 nan 1.0 2.0)   — input order, no error
```

And two demonstrably different integers compared equal:

```lisp
(compare 9007199254740993 9007199254740992.0)   ; => 0
(= 9007199254740993 9007199254740992.0)         ; => false
(> 9007199254740993 9007199254740992.0)         ; => false   — neither equal nor greater
```

**Cause, both silent-wrong rather than errors.**

1. **NaN.** `value_cmp` ended every float arm with `partial_cmp(...).unwrap_or(Equal)`, and the
   exact `bigdecimal_cmp_float` helper had an explicit `if f.is_nan() { return Equal }`. So NaN
   compared *equal to everything*. `sort` is built on `compare`, so one NaN made every element
   look equal to it and the merge kept input order. The doc comments recorded this as a
   deliberate choice ("a `NaN` float is `Equal`"), which is why it survived — it was consistent,
   and consistently wrong.
2. **Precision.** `(Int(x), Float(y)) => (x as f64).partial_cmp(&y)`. Every *other* cross-type
   numeric arm — BigInt, Decimal, Ratio against Float — already went through an exact base-10
   `BigDecimal` comparison, with a comment explaining why the lossy path was wrong. The `Int`
   arm was the one that never got the treatment.

**Fix.** `float_total_cmp` places NaN last and equal only to itself — Rust's `f64::total_cmp`
and Java's `Double.compare`. `Int`/`Float` now uses `bigdecimal_cmp_float`, the same exact path
as its neighbours.

**What deliberately did NOT change.** `<`/`<=`/`>`/`>=` stay IEEE: every comparison against NaN
is false. So `compare` and `>` now disagree about NaN, on purpose — `compare` promises a **total
order** because `sort` needs one, `<` promises IEEE because arithmetic needs that. And `=` stays
strict: `(= 1 1.0)` is false while `(compare 1 1.0)` is 0, because 1 and 1.0 are the same point
on the line and different values.

**How it was found.** A review of whether Brood should have cross-type comparison at all. The
answer turned out to be that the design was defensible and the *implementation* had two holes.
A proposed `==` (numeric equality) was written, then withdrawn on this evidence: built on
`compare`, it inherited both bugs, claiming `(== nan nan)` is true and that 2^53+1 equals 2^53.

**Guarded by** `tests/comparison_test.blsp` — 25 cases covering the strictness of `=`, the
tower coercion of `compare`, exactness past 2^53, NaN's placement, the sort that broke, and the
deliberate `<`-vs-`compare` disagreement — plus `float_total_cmp_is_a_total_order`,
`int_vs_float_is_exact_past_2_53` and `nan_sorts_last_and_infinities_order_by_sign` in Rust.
One test in that file has to compare via `pr-str` rather than `assert=`, because a result
containing NaN is never `=` to itself — the contract biting the test that checks it.

## KI-76 — `make green` gated on a binary no command refreshes ✅ FIXED 2026-08-28 (siblings 2026-08-29)

**Symptom.** `./scripts/green.sh` reported the tree NOT green with 8 warnings from the one
gate that is supposed to be held at zero:

```
  FAIL nest check std/ + tests/
       tests/support/gabriel/deriv.blsp:46:39: warning: unbound symbol: third
       …
       tests/gen_test.blsp:8:13: warning: unbound symbol: defserver
```

Every one was phantom. `third` is defined at `std/prelude/predicates.blsp:118` and
`defserver` at `std/proc/gen.blsp:225`. Run against the current binary, the same command
returns `format: 412 files checked, all clean` and **zero** checker warnings.

**Cause.** Two independent mistakes about *which* binary to run.

1. `green.sh` gated on `$root/target/release/nest`. `make release` builds
   `RELEASE_DIR := target/$(TARGET_SUBDIR)release-fast` — a **different path**. Only a plain
   `cargo build --release` writes `target/release`. So the script's own remedy
   ("run `make release` first") could never refresh the binary it was about to run, and that
   binary drifted freely. On this tree it was 9 commits old (`464b6c57` against HEAD
   `6790e1b6`).
2. `std/` is `include_str!`'d into the binary, so a 9-commit-old `nest` carries a 9-commit-old
   `std/` — including `gen/defprocess`, which `e38e9a0b` renamed to `defserver`. The gate was
   faithfully reporting that *its own baked-in stdlib* did not have the name the current
   `tests/gen_test.blsp` calls. It was reading a rename backwards.

**Why it survived.** The guard written for exactly this hazard could not fire:

```sh
if [ -n "$(git status --porcelain -- std crates)" ] && [ "$nest" -ot "$(git diff --name-only …)" ]
```

It is conditioned on `std/` or `crates/` having **uncommitted** changes. A clean tree — which
is the state you are in immediately before a push, and the state in which "is this tree
green?" is actually asked — takes the `[ -n … ]` test to false and skips the check entirely.
The guard was live only in the situation where you already knew your binary was behind.
It was also a `note` (advisory) rather than a failure, so even when it did fire the gate still
printed a verdict beside it.

`make doctor` had the finding all along — *"target/release/brood is built from 464b6c57, HEAD
is 6790e1b6 — it will silently ignore anything newer"* — but nothing makes `green.sh` consult
it, and `make green` is documented as the answer to "is this tree green?".

**Fix.** `green.sh` now resolves the binary by identity rather than by path: it prefers
whichever of `target/release-fast/nest`, `target/release/nest` reports HEAD's short sha from
`nest --version` (the `binary_sha` mechanism `doctor` already used). A binary that is stale, or
merely older than any `std/`/`crates/` source, is **`red`** — not a note — and the `.blsp`
gates are skipped with `the .blsp gates DID NOT RUN`. That direction is deliberate: a stale
binary's verdict is meaningless in *both* directions, so the failure mode must not be a
believable green **or** a believable red. This one cost a real investigation on 2026-08-28 —
two names were chased as rename rot before the binary was suspected.

**Guard, sabotage-verified.** There is no unit test; the gate guards itself, which is checked
by breaking it on purpose. With `std/string.blsp` edited and uncommitted (binary older than
the source), it prints:

```
  FAIL the .blsp gates DID NOT RUN — std/string.blsp is newer than …/target/release-fast/nest (uncommitted work)
       rebuild with `make release`, then re-run.
```

and after `make release`, both `.blsp` gates run and pass. Confirmed in both states.

**The general lesson, which this file already states once (KI-68/69/70) and now again:** a
gate that names the wrong artifact is not a weaker gate, it is a *different* gate, and it
reports confidently about something you did not ask. Whenever a script runs a built binary,
assert the binary's identity — not its existence.

**Addendum 2026-08-29 — the same defect was in three sibling gates, and it hid a second one.**
`green.sh` was fixed inline, so `check-examples.sh`, `check-stress.sh` and `check-corpora.sh`
kept the original bug verbatim: all three defaulted to `target/release/…` while their error
told you to run `make release-brood`, which writes `target/release-fast`. Locally none of the
three could run at all, and the message named a command that could not fix that — the reason
this went unnoticed is that CI *does* build `target/release`, so the gate only misbehaved
where a person would run it.

Pointing them at whichever binary exists then surfaced a **second** wrong answer, and it is
the more interesting one. `make release` builds `brood` with `RUN_FEATURES`, which is LEAN:
`--no-default-features` compiles `DEV_MODULES` (`test` `docs` `grammar` `observer` `reload`
`mcp` `perf` `repl`) out entirely. So `examples/hot-reload/main.blsp` died on
`unbound symbol: reload/on-change` — reported as an example failure, i.e. **as rename rot**,
which is exactly the class this gate exists to detect. That is a false positive that costs a
name hunt through a rename wave; it nearly bought one.

**Fix.** `scripts/lib/gate-binary.sh`, now sourced by all three: `gate_pick` prefers the
candidate whose `--version` reports HEAD's sha (existence is only a tiebreak),
`gate_require_fresh` exits 2 with `the gate DID NOT RUN` — carrying over `green.sh`'s
exemption for a binary whose baked-in `std/`+`crates/` is unchanged, so a docs-only commit
does not refuse the gate — and `gate_classify` separates the two verdicts. A run whose
unbound names *all* belong to a module this **tree has and this binary lacks** is reported as
`skip … (needs reload, absent from this lean build)`, never as a failure. The absent-module
set is derived from the tree (`std/tool/<ns>.blsp` exists, `(builtin-modules)` does not list
it) rather than restating the Rust `DEV_MODULES` list, so it cannot drift from it. The
`stress/*_test.blsp` block skips wholesale on a lean brood for the same reason — `--test`
needs the `test` module.

**Addendum 2026-08-29 (same day, second pass) — the freshness rule itself cried wolf, and
`scripts/*.blsp` was a fifth ungated corpus.**

*The rule.* `gate_require_fresh` asked the **sha first** and the mtime second, so it refused a
binary that was in fact current: build from a dirty tree (the binary records the sha of the
HEAD it was built **at**), then commit, and the sha now names the parent while the contents
are exactly what is on disk. It refused `check-corpora` minutes after being written. **mtime
is now the primary test and the sha is only the rescue** — a binary newer than every
`std/`+`crates/` source baked what is on disk, which is a *stronger* property than any sha
match; the sha exemption is kept for the one case mtime cannot judge (a `git checkout` or
fresh clone rewrites mtimes without changing content). The same reordering was ported to
`green.sh`, which had the defect inline. Candidate *selection* had the same bug in miniature:
with no sha matching HEAD both pickers fell back to "first that exists", which chose a
`release-fast/nest` from 15 commits back over a `release/nest` built minutes earlier — the
fallback is now the **newest** candidate.

*The corpus.* `scripts/*.blsp` sat outside `check-corpora` (which covered `examples/`,
`stress/`, `scripts/fuzz/stress/`, `breakage/`), and all three non-trivial ones were dead:
`scripts/stdlib-audit.blsp` — the standing audit of the library's own surface — plus
`release-ecosystem.blsp` and `suggest-renames.blsp`, on ADR-258's `name` → `->string` and
`os/getenv` → `os/env`. The audit had been unrunnable for long enough that nobody noticed the
number it produces was unobtainable. `scripts` is now its own tree in `check-corpora` (top
level only — `scripts/fuzz/stress` stays a separate corpus, since a name resolves against the
files loaded *with* it). Sabotage-verified: restoring `(name s)` in the audit turns the
`scripts` row red.

**Guard, sabotage-verified.** Four deliberate breaks, all against the LEAN binary:

| sabotage | verdict |
|---|---|
| `(bogus/thing 1)` appended to `examples/life.blsp` (module exists nowhere) | `FAIL life` ✅ |
| `(no-such-function 1)` appended (bare name) | `FAIL life` ✅ |
| unmodified `hot-reload` on the lean build | `skip … (needs reload, …)`, exit 0 ✅ |
| `touch crates/lisp/src/lib.rs` | `the gate DID NOT RUN — … is newer than …`, **exit 2** ✅ |

The first two are the ones that matter: the skip path must not be able to swallow real rot,
and a name whose module exists nowhere is still a failure. With the full-featured binary
(`cargo build --release -p cli -p nest`) all three gates run clean end to end — examples 9/9,
stress 28/28, corpora 68 files across four trees.

## KI-77 — `loop` was ~3% slower than v0.14.1 ☑️ NO LONGER REPRODUCES 2026-08-28 (fixed by v0.15.0)

**Symptom.** The `loop` benchmark row (a pure integer self-tail loop — the simplest shape the
JIT covers) reads consistently slower than `v0.14.1`:

| measurement | base | new | delta | floor |
|---|---|---|---|---|
| `make ab` sweep, best-of-7, pinned | 93 ms | 96 ms | +3.2% | 1.1% |
| solo re-run, best-of-15, pinned | 91 ms | 95 ms | +4.4% | 0.0% |
| fixed baseline + base-vs-base control | 90 ms | 93 ms | +3.3% | **0.0%** (90/90) |
| interleaved, min-of-15, pinned | 89 ms | 92 ms | +3.4% | — |
| interleaved, min-of-15, **unpinned** | 72 ms | 74 ms | +2.8% | — |
| interleaved vs `464b6c57` | 92 ms | 94 ms | +2.2% | — |

**Why it is a watch and not dismissed as noise.** It survives the two checks that normally kill
a few-percent signal on this suite:

- **Unpinned.** `make ab` pins compute rows to one core, which charges the benchmark for
  background JIT compilation, and 70 commits of `std/` growth means more arms tier up — exactly
  the ADR-175 artifact that showed `collatz` +8% pinned and zero unpinned. This row keeps +2.8%
  unpinned, so that is not it.
- **Interleaved.** Alternating the two binaries within one session controls thermal/cache drift.
  The gap holds.

`make ab` still prints `noise`, because its verdict rule is `max(5%, 2 x floor)` and a ~0% floor
collapses that to the 5% absolute threshold. For a row whose base-vs-base spread is genuinely
0.0%, 5% is a lenient bar — worth revisiting in `ab-bench.sh` rather than treating the verdict as
the answer.

**Localization.** `464b6c57..HEAD` only. v0.14.1, `6b172c1d` and `464b6c57` are all within 1.1%
of each other interleaved, and HEAD is +2.2% against both. Not bisected further.

**The measurement trap, which is the more useful half of this entry.** A bisect stalled because
this row's **absolute** numbers drift ~3% between measurement *sessions*: the same `6b172c1d`
binary read **90 ms** in one interleaved pair and **93 ms** in the next. So a per-step bisect that
compares a freshly built candidate against a number taken earlier is reading drift, not signal —
which is how the first three readings here disagreed (+3.2%, +4.4%, −1.1%). If this is picked up:
**compare only same-session interleaved pairs**, budget ~15 reps a side, and expect ~1% of
irreducible spread even then.

**Next step if it is worth chasing:** interleaved-pair bisect over the 24 commits in
`464b6c57..HEAD` (the type-system wave, `e38e9a0b`, `092ba281`, and the KI-72/KI-76 fixes). The
plausible suspects are the ones that touch runtime rather than the checker — `092ba281`'s root-ctx
tagging and `e38e9a0b`'s `core/heap/equality.rs` changes — but nothing has been measured to
implicate either.

### Resolved 2026-08-28: gone at v0.15.0, and the original reading was right

Re-measured at `e9c54606` (v0.15.0) against the same `v0.14.1` worktree binary, `--floor`,
best-of-15, pinned:

| row | v0.14.1 | HEAD | delta | verdict |
|---|---|---|---|---|
| `loop` | 93 ms | 89 ms | **-4.3%** | noise |
| `startup` | 35 ms | 29 ms | **-17.1%** | improved |
| `sieve` | 83 ms | 80 ms | -3.6% | noise |
| `collatz` | 219 ms | 217 ms | -0.9% | noise |
| `fib` | 111 ms | 111 ms | +0.0% | noise |

**The original reading was not a measurement error**, which was worth settling rather than
assuming. Building the exact tree the regression was filed against and measuring it in one
session against HEAD gives `dfcddc4f` **94 ms** vs HEAD **89 ms** — so `dfcddc4f` really did sit
~4 ms above v0.14.1's ~90 ms, as filed, and v0.15.0 moved past both.

**What actually changed is a fixed per-run saving, not a `loop` fix.** `dfcddc4f` -> HEAD, same
session:

| row | dfcddc4f | HEAD | delta | absolute |
|---|---|---|---|---|
| `startup` | 36 ms | 30 ms | **-16.7%** | -6 ms |
| `loop` | 94 ms | 88 ms | -6.4% | -6 ms |
| `sieve` | 84 ms | 78 ms | -7.1% | -6 ms |
| `fib` | 116 ms | 111 ms | -4.3% | -5 ms |
| `collatz` | 223 ms | 218 ms | -2.2% | -5 ms |

Every row gained the *same ~5-6 ms*, and the percentage simply tracks how cheap the row is.
That is the signature of a boot/load win, not a compute change — and it is the mirror image of
what filed this entry, where almost every row read *slightly positive* for the same reason in
reverse.

**Not attributed.** The saving arrived somewhere in `dfcddc4f..e9c54606`; nothing in the devlog
records it, and the prelude changes in that range (ADR-278/281's multimethod return types) are not
an obvious cause. `cli_support.rs`'s addition there is the `.brood_crash_dump` process-death hook,
which is diagnostics, not boot. **Worth attributing before anyone claims it**, because a 17%
startup win is the kind of thing that should have a commit next to it.

**One arithmetic that must NOT be done here.** It is tempting to read "boot fell 6 ms but `loop`'s
wall fell only 6 ms, so compute is unchanged" — or worse, to subtract the `startup` row from
another row to isolate compute. CLAUDE.md and FRONTIER both record that as an **under-subtraction
trap**: the `startup` row is `(io/puts 0)`, which loads `io` and through it `string`, while most
rows load neither, so a row keeps a lazy-load saving that the subtraction only partly removes.
If anyone wants to know whether a residual *compute* delta survives underneath this boot win, the
measurement is an in-process one with a fixed iteration count (`%now-ns` around N iterations
inside one process), not a difference of two whole-invocation rows.

### RETRACTED 2026-08-28 (same day): there was no v0.15.0 boot win — it was image state

The section above attributes KI-77's closure to a "~5-6 ms fixed per-run saving" in v0.15.0 and
calls it an unattributed 17% startup win. **That is wrong, and the error is instructive: it is
this feature's own documented trap, taken while writing about it.**

`make ab BASE=dfcddc4f ROWS=startup` compares a worktree binary against the working-tree binary.
The working-tree binary had a **current stdlib image** (the harness and `make install` both build
one); the worktree binary had none, because the image id carries the git sha and no image was ever
built for that ref. So the arm I read as "v0.15.0 is faster" was *imaged vs unimaged*, which is
KI-72's trap 3 — "the arm you believe is imaged is reading source" — inverted.

**The trustworthy measurement is a single-session interleaved sweep with every binary in the same
image state** (all unimaged, core-pinned, best-of-9):

| row | 0.13.0 | v0.14.0 | v0.14.1 | dfcddc4f | e9c54606 | HEAD |
|---|---|---|---|---|---|---|
| `startup` | **27 ms** | 35 | 34 | 36 | 36 | 36 |
| `fib` | 106 | 108 | 112 | 115 | 114 | 114 |
| `loop` | 84 | 91 | 89 | 93 | 92 | 95 |

Which says something quite different, and more useful:

- **`startup` is flat from v0.14.0 to HEAD.** There is no v0.15.0 win to attribute. What there *is*
  is a **step of ~30% between 0.13.0 and v0.14.0** (27 → 35 ms) that no entry records — masked in
  the published numbers because the image flip landed later in the same window and gives some of
  it back. That is the real open question, and it is a step, so it is bisectable.
- **`fib` and `loop` are ramps**, not steps (+7.5% and +13% across 0.13.0 → HEAD, no single jump).
  brood-benchmarks' CLAUDE.md describes exactly this shape and says there is nothing to bisect —
  `git bisect` will still return a commit, and on `primes` it once returned a `.blsp` *test file*.

**Three measurements of the same thing disagreed before this was settled** (−16.7%, −10.9%, +0.0%),
and every disagreement was image state rather than code. The rule that resolves it is already in
this file and was not followed: **verify the image per arm, not once per session.**
`(stdimage/status)` reports `:live`/`:stale`/`:absent` with the id it wants beside the ids on disk.

**And a papercut that makes this easy to walk into.** `make release-brood` rebuilds only `brood`
(`-p cli`), leaving `target/release-fast/nest` at whatever commit it was built at. `nest stdimage`
then writes an image keyed to *nest's* `stdlib-id`, which `brood` cannot use — so `brood` reports
`:stale` no matter how many times you build one. Observed here with `brood` at `70fbdb32` and
`nest` at `7cd92eed`, and note the content hash was **identical in both** (`f81c5e8bfacc125`): the
baked stdlib was byte-for-byte the same and only the git sha differed. The sha is a deliberate
conservative proxy for "the kernel that interprets this stdlib may have changed", so it is not
simply removable — but it does mean **any** commit invalidates every image, not just a `std/` edit,
which is broader than trap 3 states. If you are measuring, build the image with the *sibling* nest
(`make release`, not `make release-brood`) and check `:state` is `:live`.

**Kept as an entry rather than deleted** because the *method* is the reusable part: this row's
absolute numbers drift ~3% between measurement sessions (the same `6b172c1d` binary read 90 ms in
one interleaved pair and 93 ms in the next), so a per-step bisect on a 3% signal reads drift. What
worked was building every candidate first and measuring them all in **one** session — and, when a
signal disappears, building the exact tree it was filed against rather than assuming the original
was wrong.

## KI-78 — CI never built a stdlib image, so the suite tested the path users do not get ✅ FIXED 2026-08-28

**Symptom.** There isn't one, which is the point. Every CI job is green and every one of them
exercises the **source** load path, while the shipped default since v0.15.0 (`f114d01e`) is the
**imaged** path.

**Cause.** Default-ON is safe by construction: `%std-image-install` answers `nil` when no image is
on disk and `require` falls back to source in ~30 µs. The runtime deliberately never *builds* one
(~1 s, which would land on the short-lived runs the image exists to help) — `nest` writes it. And
nothing in `ci.yml` runs `nest stdimage` or sets `BROOD_STDIMAGE`. So CI has no image, and the
whole suite runs source.

**Why it is worse than "uniformly untested".** `image_matches_source.rs` (ADR-280) builds an image
itself and writes it to `~/.cache/brood`. nextest gives each case its own process in no guaranteed
order, so whether any *other* case runs imaged depends on whether that one happened to go first.
The coverage is not absent, it is **nondeterministic** — which is the harder failure to reason
about, and the same shape as the KI-72 guard hole one level down (`autoload_race` never built an
image either; fixed in `6e52528a`).

**Not a suspected failure.** The suite is known green on the imaged path: 1218/1218 and later
1222/1222 locally with an image verified live. This is a gap in what CI *proves*, not evidence of
a bug hiding behind it.

**Fix, and the shape it has to take.** Build the image in nextest's setup script — the repo
already runs one (`warm-boot-cache`, for KI-38), so this is an addition to an existing mechanism
rather than a new one, and it fixes `make test` locally at the same time. But it must not *trade*
one path for the other: with the image built, `autoload_race`'s two default-path arms become
imaged too, and source-path race coverage would vanish. So one job has to stay on the source path
deliberately, with `BROOD_NO_STDIMAGE=1`.

**The general lesson, which this file has now recorded four times** (KI-68 dead corpora, KI-70 a
walk that returned early, KI-76 the wrong binary, this): a gate must assert *what it is gating
on*. Every one of those four was green while measuring nothing, and in three of them the thing
being measured was not even present.

### Fixed 2026-08-28

`scripts/build-std-image.sh`, registered as a second nextest setup script beside
`warm-boot-cache` (`setup = ['warm-boot-cache', 'build-std-image']`). It follows the same
contract as its neighbour, because it is the same kind of thing — infrastructure that must never
redden a run:

- every failure path exits 0. No `nest` built yet means no image, and the suite behaves exactly as
  before, which is a **correct** configuration rather than a broken one;
- its off switch is `BROOD_NO_STDIMAGE=1` — the variable that already disables the image at
  runtime — so a source-path job gets a source-path *setup* without inventing a second flag;
- `nest stdimage` reports `:present` rather than rebuilding when a current image exists, so the
  cost is ~30 ms on a warm tree and ~4.9 s after a commit (the id carries the git sha plus a
  content hash of every baked-in `.blsp`).

**The trap this had to avoid, and the reason one job opts out.** Building the image makes
`autoload_race`'s two *default-path* arms imaged as well — so simply turning it on everywhere
would have deleted source-path race coverage while appearing to add coverage. ci.yml's
tree-walker job now carries `BROOD_NO_STDIMAGE: 1`, which is the right job for it: it already
exists to run the suite in the non-default configuration, so the pairing is engine × load-path
rather than either alone.

**Verified in both configurations, whole suite, image deleted first each time:**

| configuration | setup script | result |
|---|---|---|
| default | `4816 bindings -> …std-image-0.15.0+…bin` (4.9 s) | **1222/1222** |
| `BROOD_NO_STDIMAGE=1` | `skipped … the suite will run the source path` | **1222/1222** |

One detail worth knowing: the two *explicitly* imaged arms build and install their own image via
`%std-image-install`, which is a plain function and does not consult the env var. So they stay
imaged even in the source-path job — which is what you want, since it means that job covers
**both** paths rather than losing the imaged one.

## KI-79 — one `live_migration` failure on the commit that moved the JIT preempt handler ⚠️ WATCHING 2026-08-28

**Symptom.** CI on `12b31fc2` (`perf(jit): outline the fast link's cold outcomes`):

```
FAIL [3.841s] (847/1227) brood::live_migration deep_receive_continuations_resume_correctly_across_workers
  panicked at crates/lisp/tests/live_migration.rs:118:5
```

Every other job passed. Line 118 is **not** the correctness assertion — it is
`assert!(migrated || gc_stress, …)`, which fails unless `process::migrate_count() > 0` after up to
400 bursts. So the test asserts that a scheduler event **was observed**, and in this run the
per-burst `assert_eq!` on the computed result passed **400 times out of 400**.

**Why it is suspicious despite that.** `12b31fc2` outlined `jit_run_fast_link`'s cold outcome arms
into a `#[cold]` helper, and those arms include **outcome 2 — preempt** — which is precisely the
mechanism live migration rides on. A change that stopped preemption firing would produce exactly
this signature: right answers, no migration observed.

**Why it is probably not that, with the evidence:**

- **The move is verbatim.** A line-by-line diff of the 117 moved lines against the pre-change file
  shows **zero semantic differences**. The only new code is `if outcome == 0 { … return … }` ahead
  of the delegation, which is the same condition the `match` arm had.
- **It cannot change *when* a preempt happens.** The native arm's tick poll decides that
  (`emit_self_call` resets the journal immediately before it); only the *handling* of the returned
  outcome moved.
- **Unreproduced in 18 runs** — 10 unpinned, and 8 pinned to two cores to match the runner CI
  describes as sharing 2 cores with the workspace build.
- **The test's own failure message anticipates this case**: "If this is the only failure and the
  machine was loaded, suspect scheduler starvation rather than the capture machinery: the per-burst
  correctness assertion above passed every time."

**The structural gap, which is the actionable part.** This was the last test in the suite asserting
an *observation of concurrency* with **no retry** — the same gap `.config/nextest.toml`'s
`distribution` override already documents for real-TCP deadlines. Observing a cross-worker migration
on two shared cores is exactly the kind of thing that can legitimately not happen. It now carries
`retries = 1`: a starvation blip stops reddening CI, a deterministic regression still fails both
attempts, and a pass-on-retry is reported as FLAKY so it cannot be absorbed silently.

**If it recurs, the first thing to establish is whether the per-burst `assert_eq!` also failed.**
That is the line between scheduler starvation (this entry) and a real capture-machinery bug (a much
more serious thing, and the class KI-1 lives in). The two look identical in a summary line and
completely different in the log.

**Not filed as a bug against `12b31fc2`** because nothing in it can produce a wrong answer, and
nothing did. Recorded because this repo's own rule is that a failure seen once is real until proven
otherwise — and because a single sighting on the one commit that touched the preempt path is
precisely the coincidence that deserves writing down rather than explaining away.

## KI-89 — a test file's ability impls leak into `std/`'s checker view ✅/⚠️ 2026-08-31: the resurrection race FIXED + guarded; a residual orphan mechanism WATCHING (one binary, since destroyed — see the residual block)

> **Resolution 2026-08-31, two findings.**
>
> **1. The recorded minimal repro never touched `%isolate`.** `nest test FILE...` with
> explicit files takes the **single-file path** in `crates/nest/src/main.rs`: it
> `eval_file`s every named file into ONE image and runs `run-loaded-tests` — no
> `drain-files-scoped`, no per-file isolate anywhere. So
> `nest test tests/record_test.blsp tests/std_check_test.blsp` fails **by design of that
> path**: `usd` is simply still bound (a probe file run the same way sees the
> constructor bound and zero orphans — nothing was ever rolled back, because nothing
> ever restores). Every hypothesis this entry ruled out was tested against `%isolate`,
> which the repro does not exercise; the image/order/`-j1` invariance that seemed
> mysterious is just this. (Whether the explicit-files path *should* scope per file is
> a separate design question, deliberately not changed here.)
>
> **2. The full-suite orphans were a lost-update race between `registry_update` and
> `restore_globals` — found by code reading, reproduced deterministically, fixed.**
> The registry RMW (KI-22's fix) holds `registry_lock` from its read of the registry
> global to its `env_define`; `restore_globals`' wholesale table swap did **not** take
> that lock. A bystander process (a straggler the per-file reaper's documented
> ancestry gap misses) that read the registry *before* a restore's swap and wrote its
> successor *after* it writes back a map computed from the PRE-restore table —
> resurrecting **every accumulated registration wholesale** (`*record-ids*`,
> `*impls*`, `*features*`, …) while the ordinary bindings beside them stay rolled
> back. That is exactly the observed asymmetry: std record ids registered, their
> constructors unbound. And one hit is **sticky**: the resurrected entries sit inside
> every later snapshot, so they never roll back again — which is why orphans from
> five different modules appeared at once.
>
> Reproduced deterministically (`tests/registry_isolate_race_test.blsp`: a spawned
> registering bystander + 2000 isolate cycles): **1994 of 2000 cycles resurrected**
> the rolled-back registration on the pre-fix build, **0 of 2000** with the fix. At
> suite scale: a pre-fix worktree (`e569ca4f`) failed **3 of 3** full `nest test`
> runs on this entry's own sightings (`ability_test.blsp:471` orphans ×2,
> `stdimage_test.blsp:60` ×1); the fixed tree ran the same suite with **zero** orphan
> failures. Fix: `restore_globals` acquires `registry_lock` around the swap (lock
> order registry → globals, matching every RMW; see the comment at the swap). A
> racing RMW now either completes before the swap and is wiped — the isolation
> contract — or starts after and reads the restored table.
>
> Guard: `tests/registry_isolate_race_test.blsp`, with a liveness floor on the
> bystander (its first draft died silently on a renamed builtin and "passed" — the
> gate-that-cannot-fail lesson, again).
>
> **Residual, 2026-08-31 (later): a suite-scale orphan mechanism SURVIVES the fix on
> one binary — and that binary is gone.** The first post-fix build of the merged tree
> failed 3 of 3 full `nest test` runs orphan-shaped (`stdimage_test:60`,
> `ability_test:471` ×2, same seven ids) — so the resurrection race was not the only
> path; the suspected residual is the interleaving the lock deliberately permits (a
> straggler's constructor `def` wiped by a restore while its locked
> `%record-register`, starting after the swap, lands in the restored table: id kept,
> ctor gone — sticky via the next file's snapshot). Then the next incremental build
> (adding the `BROOD_REG_TRACE` instrumentation, runtime-gated OFF) went **15/15
> green** — traced and untraced — and no binary that exhibits the residual exists any
> more. That is KI-88's lesson repeated verbatim, including the mistake: the failing
> binary was overwritten instead of preserved. What the hunt left in-tree:
> `BROOD_REG_TRACE=1` (every `*record-ids*` write with the writer's 4-hop ancestry
> chain + every restore), and two traced observations — the heavy all-registry trace
> *suppresses* the race (stderr-lock serialization), so trace lean; and the
> module-sweep writers seen mid-run are `doc_examples_test`'s legitimate
> load-everything unit, within its own file window. **If orphans are next seen:
> PRESERVE THE BINARY, re-run it with `BROOD_REG_TRACE=1`, and read the chain= of
> the seven writes against the RESTORE lines.** Watching, not open: 15 consecutive
> green full runs and no reproducing artifact.
>
> **Follow-up, same day: a REPRO LEVER exists, the class is wider than registries, and
> this needs a design session.** The residual fired again on the next combined-tree
> build (first run after the build: `stdimage_test:60`, `*lineedit-keymap*` pre-bound),
> that binary WAS preserved this time (sha `8bd15795…` — since cleaned up: the
> delete-the-images lever below supersedes it, reproducing the class on any binary),
> and it then ran 6/6 green warm and 3/3 green cold-booted (`touch` lever) — so the
> boot cache is not the key either. The lever that works: **delete the stdlib images**
> (`rm ~/.cache/brood/std-image-*.bin`) so every module load takes the slower SOURCE
> path. Under that timing the class fires readily and in new shapes: run 1 failed
> `ui_test.blsp:284` (records/vtables mixing — overlay ability records), and run 2
> caught the mechanism LIVE mid-suite — a process died
> `ability Temporal/->iso: no impl for :tempo/tempo — have (:datetime/…)`, i.e.
> **tempo's `*impls*` entries were ripped out from under a RUNNING dispatch** (the id
> resolved; the impl map only held datetime's) — and the run then degraded past a
> 10-minute bound. So the full class is: **a per-file scope restore races processes
> still RUNNING against the pre-restore globals — readers as well as writers** — the
> `%isolate` soundness condition ("no other process mutating globals concurrently")
> violated routinely by the scoped suite under source-path timing. The registry lock
> fixed the one corruption that compounded (wholesale resurrection); the remaining
> design question is structural: process quiescence at file boundaries, or the
> spawn-time ownership generation the `%isolate` comment already names as the missing
> primitive. A scheduler/runner design session with fresh eyes — do not patch it
> piecemeal from here.
>
> **Sighting 2026-09-01 (late).** Fired on a plain `nest test -j1` at `3bcfff10` + the
> strict-gate fix, WITH the stdlib images live (so the source-path lever is sufficient
> but not necessary): `ability_test.blsp:471` — "every registered record id names a bound
> constructor" — i.e. the orphan seen from the *registry* side rather than the checker's.
> 94/94 solo immediately after, on the same binary. No new information beyond confirming
> the class is still live on the current tree; the binary was not preserved because the
> delete-the-images lever supersedes it.

**Symptom.** In a scoped `nest test` run, `std_check_test` ("the standard library carries no
checker warnings") fails with ~15 warnings about a record defined in **another test file**:

```
std/stats.blsp:26:33: warning: *: no `num/mul` method for [:record-test/usd :record-test/usd]
std/json.blsp:138:23: warning: +: no `num/add` method for [:record-test/usd :int]
std/tool/perf.blsp:38:20: warning: /: no `num/div` method for [:record-test/usd :float]
```

`record-test/usd` is `tests/record_test.blsp`'s money record. It is not `require`d by any
`std/` module and cannot be: the checker is seeing it because the registration outlived the
file's scope. `record_test`, `sig_adoption_test`, `docs_test` and `doc_examples_test` fail in
the same run for the same reason; **every one of them passes when run alone.**

**Minimal repro — two files, order-dependent:**

```bash
nest test tests/record_test.blsp tests/std_check_test.blsp   # fails
nest test tests/std_check_test.blsp                          # passes
```

**Pre-existing, and proven so.** The same two-file invocation reproduces identically on a
clean HEAD worktree, so this is not ADR-308's argument-order migration (which is what
surfaced it — adding tests shifted discovery order so `record_test` now loads first). Also
not the `nest check` result cache: `BROOD_NO_CHECK_CACHE=1` behaves the same.

**Mechanism: UNKNOWN. Five hypotheses tested and ruled out** — recorded so the next attempt
does not repeat them:

1. *`%isolate` fails to roll back `*record-ids*`.* It rolls back. `(%isolate (fn ()
   (require-one 'tempo) …))` leaves the count unchanged.
2. *`%isolate` fails to roll back `*impls*`.* It rolls back. Loading the whole of
   `tests/record_test.blsp` inside an isolate leaves `*impls*` at 8 entries, no `usd` key,
   and `record-test/usd` unbound afterwards.
3. *A process spawned inside an isolate registers after the restore.* No orphans.
4. *Another process observes the shared RUNTIME-region entry after the parent rolled back.*
   The child sees the same clean state.
5. *The checker accumulates knowledge across files in one process.* No: `check-file` on
   `tests/record_test.blsp` then on `std/stats.blsp` produces zero `usd` warnings.

So the registries are correctly scoped and the checker is not accumulating — yet the two-file
`nest test` invocation reproduces every time. What remains unexamined is the path between
them: `drain-files-scoped` folds files serially, but a file's *units* run as concurrent
spawned workers, and `std_check_test`'s `check-file` sweep is itself one of those units. The
next probe should be whether a registration made by a worker of file A is visible to a worker
of file B despite A's isolate having restored — i.e. whether the restore covers writes made
by processes the isolate did not create.

**Not fixed** — filed rather than patched, because a fix aimed at the wrong mechanism would
look like a fix. `mono_devirtualize`'s soundness hole is the narrow part worth closing
independently: it resolves a global and then trusts `*record-ids*` by name, so a stale id
plus a later same-named non-record would devirtualize wrongly.

**Two more sightings, 2026-08-31 — and the concurrency hypothesis is now DEAD.** Three
consecutive full `nest test` runs on `da038914` failed, each on a different case, one failure
per run:

| run | failing case |
|-----|--------------|
| 2 | `stdimage_test.blsp:60` — "a root global is attributed to the module that defines it, not to a dependent" |
| 3 | `ability_test.blsp:471` — "every registered record id names a bound constructor" |
| 4 (**`-j1`**) | `ability_test.blsp:471`, identically |

Run 4 used `nest test -j1`, so a file's units did **not** run as concurrent workers — which
retires the "next probe" this entry proposes above (whether a worker of file A is visible to
a worker of file B). The leak survives a fully serial run, so it is not cross-worker
visibility; it is the per-file scope restore itself, or file-to-file state in the folding
process. Both cases pass alone (`ability_test` 94/94, `stdimage_test` 12/12).

`ability_test.blsp:471`'s own diagnostic paid off — it names the orphans, so this sighting
cost no extra run:

```
(:tempo/iset :multimap/multimap :tempo/tempo :queue/queue :pq/pq :tempo/span :log/line-backend)
```

Every one is module-qualified, and every one is a `defrecord` in a std module (`tempo`,
`multimap`, `queue`, `pq`, `log`) that some *earlier* test file `require`d. So the shape is
narrower than "a test file's records leak": the ids of **std** records outlive the scope that
`require`d their module, while the constructors those ids name are correctly unbound again.
That is a registry write escaping a restore that did roll back the bindings beside it —
which contradicts hypothesis 1 above as tested (an isolate around a single `require-one`
leaves the count unchanged), so the escape needs more than one `require` depth, or the
`nest test` scope is not the same mechanism as a bare `%isolate`.

`stdimage_test.blsp:60` fails by the same asymmetry read the other way: it compares the count
of globals a module introduces against the count a dependent introduces, and a registry entry
left behind by an earlier file makes the definer's set no smaller than the dependent's.

## KI-90 — `mono_devirtualize` trusted `*record-ids*` by NAME after resolving a global ✅ FIXED 2026-08-30

**The hole.** `inline::mono_arg_identity` proves a direct-constructor call's identity by
taking the constructor's resolved global name, keywording it (`(circle 2)` → `:mod/circle`)
and confirming that keyword is registered in `*record-ids*`. The registry is consulted by
NAME only — nothing checks that the global still *is* that record's constructor.

So a stale id plus a later, same-named non-record function devirtualizes wrongly: under
`BROOD_MONO=1` the guard compares the identity and then calls an impl directly, which is a
silently **wrong impl**, not a slow path (`docs/dispatch-speculation.md` names this exact
risk). The registry does go stale — see [KI-89](#ki-89), where ids outlived the modules that
registered them.

**Why it has not bitten.** `BROOD_MONO` is off by default (ADR-182 keeps 100% dynamic
semantics precisely because a captured impl fn can go stale), and a name being re-bound to a
non-record between a `defrecord` and a call is rare. It is still the narrow, fixable half of
KI-89's problem, and it is fixable **without** reproducing KI-89.

**Fixed** in `eval/compile/inline.rs`: the name is now necessary but not sufficient. The
constructor must still be **bound** — an unbound id is exactly KI-89's orphan shape, and it
is refused rather than devirtualized — and the registry's recorded name must still be this
constructor. Either check failing leaves the call on the dynamic path, which is the correct
conservative answer: the rewrite declines on any uncertainty (ADR-182).

**Guarded by** `crates/cli/tests/mono_differential.rs`
`a_stale_record_id_is_not_devirtualized_to_whatever_the_name_means_now` — a `defrecord`
named `shape` is rebound to a plain fn returning a bare map while its id stays registered,
and `BROOD_MONO=1` must answer byte-identically to the dynamic path. A differential rather
than a pinned string, so it cannot pass by agreeing with a wrong expectation.

**Residual.** This closes the reachable path (a stale id, or a name rebound to something
else). It does not make the registry itself self-cleaning — that is KI-89, still open.

**Found by** reading the readers while diagnosing KI-89, not by a failure.
