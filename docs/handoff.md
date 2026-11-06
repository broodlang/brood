# Handoff — the current state, and where to pick up

**This page is the current picture only, rewritten in place each session — not a log.** The
narrative and the measurements live in [`devlog.md`](devlog.md) (dated entries, newest at the
bottom); decisions in [`decisions.md`](decisions.md); bugs in
[`known-issues.md`](known-issues.md); the perf option book in
[`runtime-frontier.md`](runtime-frontier.md) and [`compute-frontier.md`](compute-frontier.md);
how to measure, the traps, and what needs a quiet benchmark box in
[`benchmarking.md`](benchmarking.md) (§7; the answered cross-session tasks are in
[`archive/perf-handoff.md`](archive/perf-handoff.md)). The dated sections this file
used to accumulate (2026-09-04 … 2026-10-05) are in git history; their substance is in the
devlog entries of the same dates.

## State (2026-10-17)

- **Easy pickings, done 2026-10-16** (devlog "a plain `assert`, the rustfmt 2024 reformat, the
  concurrency teaching note"): `assert` in the prelude, the `style_edition = "2024"` reformat,
  executed examples for 23 bare core names, and the tiling note in `brood-for-claude.md` with
  `examples/life_bands.blsp`. The docstring-example campaign continues module by module — the
  largest gaps are `editor/buffer` (101), `debug` and `test` (~40 each), `project`, `datetime`.

- **Latest work (2026-10-15/16, devlog "the call into native code, 32% cheaper" and "the
  interpreted call, 10% cheaper"):** the interpreter → native call 636 → 434 instructions
  (settled arms skip the tier-up; no shared-arm atomic write per entry), the interpreted call
  771 → 691. Rows: default ceiling `ring` −3.3%, `nbody`/`regex`/`spawn` ~−3%, `json` −1.6%;
  `BROOD_TIER=1` every row ≤ 0 but `sort` (+0.04%), `fib` −3.4%. Code generation itself is unchanged — the one IR
  change tried (`push_room` inlined) moved `json` +2.5% and was reverted. Six call-path
  experiments are recorded as measured-and-dropped; the remaining call costs are structural.
- **Don't build the debug profile while a suite runs:** `cargo build` relinks
  `target/debug/brood` under the running tests (it reddened `cli::stdimage_reporting`
  2026-10-16, reproduced on demand). Release builds (`make release-brood`) are safe.
- **2026-10-15/16, the maintenance review** (devlog entries of 2026-10-15, the last one
  "two bugs (KI-262, KI-263), one formatter walk, the 2024 edition"). Dependencies are on
  latest with MSRV 1.96, and the workspace is on the **2024 edition**, rustfmt's
  `style_edition` too (the reformat landed on its own, `27a24cef`). Two bugs were found
  and fixed:
  - **KI-262:** after `unlink` plus a zero-timeout flush, an `[:EXIT]` could still arrive
    later. Guard: `crates/lisp/tests/unlink_race.rs`.
  - **KI-263:** `nest format` deleted the blank line below a trailing comment.

  Large files were split by job, and the ADR log now lives in `docs/decisions/` by number
  range. `process.rs` documents the lock order in one table. All of this was verified with
  targeted tests on tiers 0–2, GC stress and fuzz. **Run on the other machine 2026-10-16:**
  `make test-both` found four reds, all stale tests from those moves (devlog "the merged
  tree's first full `test-both`"), fixed — now 1897/1897 on both engines. CI's red on
  `fec73f3f` was a test race, KI-267, also fixed.
- **Released:** v0.38.0 (`CHANGELOG.md`). On `main` since, merged 2026-10-14 from a branch
  developed apart from the release: preemption reaches every shape of running code (ADR-400..405,
  KI-230..238), one native entry and one quantum module, folds walked in spans, the gate sweep,
  and handoff items B (the fold span result stops allocating) and A (ADR-406, the computed-call
  memo). That branch's KI and ADR numbers were moved at the merge; commit messages from before
  it carry the old ones — the map is in the devlog entry "merged origin/main".
- `known-issues.md` lists **one watch entry, KI-281** (a JIT fast-link test that never published, once in CI; diagnostic armed) and nothing open (KI-279/280 fixed 2026-10-18). KI-247 (a bystander raising after a
  holder died) and KI-248 (a large allocation passing the hard limit when the live snapshot
  had not seen an earlier large block) were found and fixed 2026-10-14 (night); KI-264 (a native
  tail call by name never hit its head's cache slot) and KI-265 (a memory-pressure test that
  required the selector's stderr line when the culprit had raised in itself) and KI-266 (a stale
  leaf derivation refused for good) 2026-10-15. Those three were KI-250..252 on the other
  machine and were renumbered at the 2026-10-16 merge (this side's KI-250..255 came first).
- **Gates on the merged tree** (2026-10-14): `make test-both` 1859/1859 on both engines,
  `make gcstress`, fuzz (`tier_transition`, `metamorphic`, `pipeline`: 1 350 programs, clean),
  clippy `--all-features`, the `--no-default-features` checks, the JIT/frame/preemption/
  `process_floor`/`doc_refs` Rust tests, and the pre-push steps (`nest check` and `--strict` at
  zero, the 45 test files the merge touches). `nest format --check` no longer reads
  git-ignored files (`make prepush` used to stop on `stress/fuzz_out/fuzz_6.blsp`). On
  `0ba6c797` (2026-10-16): `test-both` 1882/1882 both engines, fuzz (four generators × 300,
  `stale=0`), `gcstress`, `check-stress`, `check-examples`, `check-imaged`, `breakagetests`
  (37/37) and `latency-gate` (worst p99 79 µs) all clean.
- **Memory ceiling:** decided (ADR-407). Inside a memory-capped container `brood`/`nest run`
  default their limits from the cap (hard = 3/4 of it), re-read every few seconds so a
  resized container moves them; elsewhere they stay unlimited. A table holding the memory is
  named in the `E0043`. A table now dies with the process that created it unless a global
  holds it (ADR-418), and a dropped table's slot is reused (KI-283).
- **Probes worth keeping:** every value walker survives data nested a million deep; a
  program in `unshare -U` with `ulimit -u N` gets an exact thread budget
  (`crates/cli/tests/thread_exhaustion.rs`); `systemd-run --user --scope -p MemoryMax=…`
  gives a real container cap locally.
- **The cleanup is finished** (devlog entries 2026-10-09 and both of 2026-10-12): the dead
  `BROOD_*` flags and aliases are gone — `BROOD_TIER=0|1|2` is the only tier knob — `nest
  mcp` is Brood, every Python script is gone, closed known-issue bodies live in
  [`archive/known-issues-closed.md`](archive/known-issues-closed.md). Its last four items
  landed too: `tcp/close` silences the socket at once, the two message-copy walkers are one,
  ability devirtualization is always on (`BROOD_MONO` deleted), and the doc catalogue
  derives its categories. Nothing from it is deferred.
- **Before pushing:** `make prepush`, and `git merge --ff-only origin/main` first if it moved.
- **Is the tree green?** Answer with `make green`, never by reading the run list. On this
  machine run targeted tests only, capped (`( ulimit -v 16000000; … -j1 )`).
- **bedit at CI's pin (`d50442f`) passes 2464/2464** with a grammar-enabled `nest`
  (`make smoke-bedit NEST=…`; without `NEST=` the smoke picks a grammar-less one and the mode
  tests fail on "no grammar", which is not a regression). `make wasm-test` needs
  wasm-bindgen-cli at the playground's pin, 0.2.129. On this machine `~/.cargo/bin` holds
  0.2.100 (2026-10-17); `cargo install wasm-bindgen-cli --version 0.2.129 --locked` fixes it.

## Open threads, in value order

### DONE (2026-11-06): the language design pass

A–D of ROADMAP's language design pass are done. The last two decisions: `()` stays `nil` with a
`:list-truthiness` lint (ADR-620), and one grammar for every `fn` head with in-order clauses
and a compile error for a shadowed clause (ADR-619). Also landed: fields on `failure`
(ADR-617), macro errors at the call or the named sub-form (ADR-618), and folds over vectors
and ranges stopping on a failure (ADR-315 finished).

### DONE (2026-10-21): a wait for another process's module load parks, at every tier

The last weakness the `bytes_literal_compiles` diagnosis turned up. `bbd6194f` hands a VM
global miss on a module another live process is loading (`derive::module_loading_elsewhere`)
to the prelude's `%lazy-global`/`%lazy-call` as ordinary frames, so the wait parks. The same
day this was carried to the rest: the tier-0 machine makes the same hand-off (and does not
redirect a thin wrapper whose inner head is mid-load), and the JIT's tier-up pre-load declines
rather than waits. Guards: `lazy_load_wait_parks.rs` (VM, stdlib image) and
`autoload_parks.rs` (tiers 0/1/2, and the tier-up); devlog 2026-10-21. **Later the same
day the claim race closed too:** a waiter whose claim is taken between the loading-elsewhere
check and `require-one`'s CAS hands off rather than waiting (`derive::parkable_miss`,
`%load-may-hand-off?`; guard: the six `…loses_the_claim_race…` cases, held open by
`BROOD_FAULT_CLAIM_RACE`). Still blocking, by the general dirty-block carve-out (ADR-100
§7.4) rather than anything load-specific: a load inferred at COMPILE time (a qualified macro
head) and a miss in a nested (non-capturing) run — devlog 2026-10-21 (later) says why.

### DONE (2026-10-19): the stepping tree-walker, ADR-416 (the user chose option B)

All five stages are in (devlog 2026-10-19, stage 1 through "late night"). At `BROOD_TIER=0`
every process body and top-level form runs on the tree-walker's capturing machine
(`eval/machine.rs`): its `receive` parks and its budget preempts, so tier 0 is the reference
semantics for whole programs and `make test-both`'s tree-walker half finally runs test
bodies on the tree-walker. The escape audit (`BROOD_TIER0_AUDIT=<file>`) over the whole
workspace at tier 0 records only a Rust unit test that drives `vm_run_bc` itself.

Running test bodies on the tree-walker found a real, older `let` divergence (a `fn` lost its
own name after a list-valued binding; fixed) and re-sized three budgets (the tree-walker's
frame cap `1 << 19`; `quantum_tail_panic.rs`'s spinners; the suite wrapper's nextest cap,
1800 s, for 370 s measured). **Measured 2026-10-20** (instruction counts, devlog "what ADR-416 costs at tier 0"): +3–10% at
tier 0 on single-process rows, `pfib` 11× (its bodies now really tree-walk), the default ceiling
flat. Fixed 2026-10-20: a global read no
longer scans the dynamic stack for names not on it, and a tree-walked level costs ~600 B
(was ~1 KB; the VM's ~220 B — the rest is an env frame per call and the root stack).

**Done 2026-10-20, the user's item 1:** the slow gates on this code — `make gcstress` (both
ceilings), the fuzzers, the dist tests looped, `make runtime-soak` — all clean (devlog
2026-10-20).

### DONE (2026-10-17): the design promises without a guard now have one

An audit mapped the design's promises to tests and closed the eight that had none (devlog
2026-10-17 "every design promise has a guard"). Two were real bugs, both fixed: KI-272 (a
`receive` on a decimal, ratio or bignum literal raised on every message) and KI-273 (a crash
reason dropped what the process threw). The standard suite (`make test`) stays inside its
5-minute budget: the new per-push tests cost a few seconds. The slow ones are separate gates:

- **`make test-contracts`** — the suite with every `sig` armed, as `nest test` users get it.
  CI job `contracts-armed`; `green-all`. Not in `make test`: armed costs twice the CPU.
- **`make gcstress-all`** — every test file under GC stress (~80 s at -P 6). Nightly and
  `green-all`; `make gcstress` (the twelve process files) stays per push.
- **`tier-audit`** — nightly now too (checks out `brood-benchmarks`), with the checkout and a
  native tier asserted first so it cannot pass vacuously. **`latency-gate`** joins `green-all`
  only: it is a benchmark-box gate, and a shared runner's p99 means nothing (benchmarking.md).

**No KI is open; KI-281 is a watch** (see above). The 2026-10-17 stability review found and fixed KI-271 (the run-check cache sweep
held every boot), KI-274 (a native self-loop never saw a `def` of itself), KI-275 (an edited
`include-str` file did not stale the project image) and KI-276 (the project image was not keyed
by contract mode). The nightly is
green again: cargo-deny (a yanked crate), and ASAN, whose job now runs through nextest
(`make asan`, profile `asan`) because `cargo test`'s one shared process broke tests written for
process-per-test.

**Found and then done:** `monitor` on a remote `{:name :node}` address refused ("pass the pid"),
so `gen/call` by name across nodes failed at once. ADR-411 adds `Frame::MonitorName`, resolved by
the peer; guard `gen_call_round_trips_across_nodes_by_pid_and_by_name`.

### DONE (2026-10-16): `exec_value`, the VM's second executor, is retired

The bytecode loop is the VM's only executor (devlog 2026-10-16 "exec_value retired"). A
top-level form compiles to a chunk and runs through `vm_apply` (a throwaway arm, `BAILED` so
it never tiers); a real `&optional` default is a prologue in its arm's own bytecode —
`push_frame` sets a hidden flag slot per missing defaulted optional (`optional_flags`), the
prologue tests it by truthiness and sets the slot. `exec_call` went with it; the prim helpers
are `eval/compile/prim_exec.rs`. Guard: `tests/optional_defaults_test.blsp`.

### DONE (2026-10-16): `receive` on the native path (ADR-409, ADR-410)

Two items, taken in order. **ADR-409:** a `receive` whose clauses are all simple shapes hands
the kernel a pattern spec instead of a matcher closure (`process/receive_spec.rs`). **ADR-410:**
an arm that receives at its head, `(defn loop (state) (receive …))`, is compiled to native code.
Its park is captured at ip 0 by the top-level driver, and only an entry that can unwind runs it
natively (`jit_tier_in_frame`). `ring` −19.4% and `pingpong` −7.3% for ADR-410, on top of
ADR-409's −2% and −3.7%. Guards: `tests/receive_spec_test.blsp`, `tests/receive_loop_jit_test.blsp`.
Left on the messaging path: a loop that does work before its `receive` (a `send` and then a
`receive`, the `gen/call` shape) stays on the VM. **Measured and dropped 2026-10-17:** admitting
it by resuming a park at the last deopt checkpoint (`060ed7c0`, undone the next commit) read
`pingpong` +1.9% and `supervisor` +4.8%. Such a loop parks on nearly every iteration, so native
code only adds crossings. Making it pay would need the wake to re-enter native code *at* the
`receive`: a native continuation, not a VM resume.

### Next: performance items (A and B the user's pick, 2026-10-13; C found doing A)

Measure both by **instruction count** first — `perf stat -e instructions:u` with
`BROOD_NO_CHECK=1` (the run pre-flight walk is noise), each binary's stdlib image live (`nest
stdimage`; `make ab` now writes both arms' images itself), the timing binary from
`make release-brood`. Times on this box are noisy; confirm with `make ab --floor` and, for
anything on the scheduler path, `make latency-gate`. `perf record` is capped at 1 kHz here:
aggregate 20-40 runs, and build `make perf-symbols` for names.

**A. DONE (2026-10-14, ADR-406, devlog "native code memoises calls to a closure value").**
`jit_dispatch_call` memoises a computed-head call's arm, env and IC block per closure instance:
~240 instructions off every such call (anonymous closure 943 → 706; named heads 415). What
remains of the gap to a named call is `fill_captures`, `finish_nested`, `native_feedback` and the
unconditional stack probe — an in-IR fast link for computed heads (a site id per computed call,
the handoff's original design) is the next step there; check `staged` (KI-19) and
`mono_devirtualize` when allocating the site.

**C. DONE (2026-10-14, ADR-408).** Native code runs a thin wrapper's compiled arm instead of
the dispatcher's by-name redirect, so the wrapper tiers and links natively: `(fn (acc x) (+ acc
x))` as a callback 1 892 → 714 instructions a call. The interpreters keep the redirect.

**B. DONE (2026-10-13, devlog "(later) — the fold span result stops allocating").** A finished
span returns the accumulator; a stopped one returns `(:%fold-more acc . next)`. `nqueens` −1.3%,
one-span folds ~145 instructions cheaper each, `reduce` flat. Two leftovers, neither urgent:
`Reducer::resolve` per span (`reduce_prim_op` re-derives `+`'s primitive every span), and the
stopped-short path's `first`/`rest` are runtime callbacks (~150 instructions a span, once per
4 096 elements). **Read the devlog's measurement lesson before trusting a few-percent
instruction delta between two builds:** a comment-only prelude edit moved rows by up to 2.3%.

**D. DONE (2026-10-14, night).** `reduce` takes the counted range loop (`nqueens` 113 → 90
ms); a named thin wrapper links from native code (~2 100 → ~420 instructions a call); the
CLIF verifier is off in release (`base64`/`json` −11% pinned); a fold's native callback
finishes inline (746 → 617 instructions an element). Leads left, measured: the fold callback
is still ~617 instructions an element against ~412 for a native→native call by name — the
remaining Rust entry ceremony (`enter_native`, frame nil-fill, captures), whose amortisation
would reshape ADR-403's per-element preemption; and a computed-head call from native code is
652 against ~410 by name (771 before ADR-406's 2026-10-14 amendment). **The in-IR fast link
for computed heads is measured low-value on the corpus:** computed-head calls from native code
per run are `supervisor` 57k, `persistent-map` 50k, `base64` 48k, `json` 25k, `wordcount` 1k,
every other row ~0 — at ~240 instructions saved each, ≤2% of any row. Worth it for user code
built on higher-order loops, not for the benchmark column. `nqueens`' `solve` stays
interpreted (it builds its loop closure; §7.1).

**E. DONE (2026-10-15) — `json`, picked apart by counters.** KI-264 (a native tail call by
name caches its head), KI-266 (a stale leaf derivation is re-validated, not refused for good),
the unlinkable-call slow path reads the call IC instead of walking the environment, `nth`'s
default is a fixed parameter, and a queued callback compiles synchronously after 2 048 declines.
`json` −5.1% instructions on top of KI-264's rows. What is left on `json` (N=20 000, counter
build): `vm_apply` ~470 000 — the functions the native compiler refuses
(`call-mediated-boxed`: `json-value`, `json-slice`, `enc-key`, `enc-string`, `->string`,
`reverse`; `chunk-outside-jit-subset`: `enc-escapes`, `count`), each called from native code
through the slow path. Followed 2026-10-15 (evening): the gate is right (§7.1), so the
boundary was cut instead — a native → interpreted → native round trip −23% (devlog). Left:
the nested `vm_run_bc` trampoline per crossing and `jit_tier_in_frame`'s entry ceremony
(older thread 2), and `fast_link_from_entry` re-checking a callee that is not native on
every call. A spike (devlog 2026-10-15 night) measured that resuming native code through the
driver instead of nesting would be break-even for compute: the lever is the per-call
constants — 768 instructions an interpreted call, 640 into native code, ~410 native to native.
The middle one is now 434 (devlog 2026-10-15 late: settled arms skip the tier-up, the shared
atomic writes are gone); what is left of it is `exec_chunk`'s call-site share (IC probe + its
`Arc` clone, the frame fill), `enter_native`'s save/restore and the stack stamp. Next: the
768 of an interpreted call, then native to native.
The interpreted call is now 691 (devlog 2026-10-16; the quantum TLS merge, a fast-link base
mirror and an in-IR `push_room` were measured and dropped there — the last moved `json` +2.5%).
Native to native is ~255 once the xcall re-lowering lands; a direct i64-convention call that
skips the callee's boxed wrapper was the structural idea, but the wrapper is 0.0% of every
row (an i64 worker is entered once and recurses in registers) — not worth building for the
corpus.

### Older threads

1. **Receive loops run interpreted — DONE for the receive-at-head shape (ADR-410,
   2026-10-16).** A loop that does work before its `receive` (send then receive) still runs
   interpreted; see the DONE entry above.
2. **The rest of the VM call** (~790 instructions after `CallInPlace`): `exec_chunk`'s `Call` +
   callee entry, and `vm_run_bc`'s frame push/pop. The cheap forms were built, measured and
   dropped (devlog 2026-09-25 "(later)"); what remains is structural.
3. **Per-message residue:** `receive_match`, `run_one`, allocation, `Ctx` install — each small;
   count the calls before touching any of them. Profiled 2026-10-16 (`ring`, precise
   sampling): `receive_match` is 9.7% of the row with no source line above 0.6% — no
   hotspot to cut; `run_one` 3.8%, `send` 2.3%, the `Ctx` thread-local + its drop ~2.8%.
   The interpreter running the process bodies (`exec_chunk` 30%) is thread 1's lever.
4. **Refresh the benchmark column** once 1 or 2 lands (protocol in brood-benchmarks'
   `CLAUDE.md`). Not on this machine.
5. **Done 2026-10-18:** the tree-walker no longer reports a contract shim's position
   (devlog "the tree-walker stops reporting contract shims").
6. **Done 2026-10-19 — ADR-416 (see "DONE" above).** `BROOD_TIER=0` did not reach process bodies. `run_process_body` runs
   a VM-eligible body on the VM whatever the ceiling, so `make test-both`'s tree-walker half
   and CI's differential job tree-walk only each test file's top-level forms: every test
   body runs on the bytecode VM. Either tree-walk process bodies at tier 0 (a `receive` then
   blocks its worker; the suite's many-process tests may starve) or document that tier 0 is
   top-level only and say what the differential covers. A tree-walker guard has to pin the
   ceiling itself (`set_forced_ceiling`), as `contract_error_position.rs` does.
7. **Perf lead from the 099abd59 column:** `ef80547d` (KI-274) costs `collatz` +6.3% and
   `primes` +3.0% in instructions — per iteration, not per poll; check whether the entry
   epoch held across the self-loop spills.

## Flake hunting

A one-in-N race passes N-1 runs; repetition is the evidence. The tools:

- `make gcstress` — the concurrency/mailbox files under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`
  on the debug build (nightly runs it).
- `make test-both` — the suite at the default ceiling and on the tree-walker (`BROOD_TIER=0`).
- `scripts/fuzz/run.sh <generator> [count] [seed]` — the engine differential (tree-walker, VM,
  JIT, GC-stress) over generated programs; `make stress` for the program fuzzer, the reader
  fuzzer and the property/race corpus (nightly runs it).
- `make runtime-soak [SOAK_SECONDS=600]` — boots `stress/runtime_soak.blsp` fresh round after
  round (first module loads raced across a 240-process fan, warm spawn bursts, link churn,
  large copies), cycling tiers, one worker and the GC verifier; stops at the first lost
  process, hang or watchdog line. The hunt for KI-88 and KI-127, whose causes were never
  found; the nightly runs it for 30 minutes and keeps every round's record.
- The nightly workflow (`.github/workflows/nightly.yml`) runs the suite three times, the stress
  suite, ASAN, TSAN + loom, and a `perf-stats` build.
- A flake hunt keeps its logs on success too: a loop that deletes them when green cannot be
  believed afterwards.
- CI and the nightly run the suite through `scripts/nextest-no-flakes.sh`, so a test that
  passes only on nextest's retry fails the job and is named. Before it, `FLAKY 2/2` was green
  and unread (`dns_test` failed its first try in four of six runs before anyone looked).
- Several runtimes in one process is a real configuration (embedders, `cargo test` threads,
  the nightly sanitizer jobs): state that belongs to a runtime lives on `RuntimeCode`, never
  in a `static` (KI-224, KI-226). `crates/lisp/tests/process_wide_statics.rs` makes every
  lock-holding `static` in `crates/lisp/src` state why it is process-wide.
- A test that times out names what it was stuck on: the runner reads the hung worker and the
  processes it spawned before the kill (running / waiting with N unmatched messages / waiting
  with nothing queued).
