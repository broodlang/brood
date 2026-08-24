# Known issues

KI-9 is a one-off arity sighting judged a transient inconsistent-build artifact, not
present in committed code; KI-10 no longer reproduces, incidentally fixed — both kept as
records, not open bugs. **KI-17** (the checker reachability gap) is now **FIXED** (ADR-189).
**KI-18** (effect duplication on a deopt) and **KI-19** (call-head evaluation order) are
both now **FIXED**, as is **KI-20** (a fast link ran the callee against the caller's IC
block — a cold cache, never a wrong answer), as is **KI-21** (`nest run --for` /
`--watch` emitted a pre-ADR-150 `~p` pin and failed on every file). **KI-25** (five JIT/VM suites could not be re-run in one image, blocking
`--repeat-until-failure`) is **FIXED** — see its entry for the two fixes and why its original
diagnosis was wrong.
This file is the condensed record — what each was, how it was fixed, and the regression
test that guards it — so a recurrence is recognizable. For the narrative discovery
writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md); deeper rationale is in the cited
ADRs / topic docs.

## Index — status per issue (⌘F the `KI-N` to jump)

| # | What | Status |
|---|---|---|
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

**No other open items.** KI-49 (the tagged-tuple receive matcher latched onto the interpreter) was root-caused and **fixed 2026-08-21** — `pingpong` −12.9%. KI-48 (JIT tail dispatch read past the roots stack) was root-caused and fixed 2026-08-21 — though never reproduced on demand, so watch for a recurrence. Before that, no open items — KI-36 was reproduced and fixed 2026-08-19, KI-47 the same day. `main` is green on all five CI jobs at `c8dbf0ea` (run 32247618122) — the first fully green run since the ADR-230/231 namespacing merge. KI-44 (the `sqrt` call-site inline, worth ~1.8× on `nbody`) and KI-45 (the stale `examples/editor`) were both fixed 2026-08-17. KI-43 (a fixed-sleep race in the remote-attach test) was found and fixed 2026-08-14. KI-28 is **no longer a watch item — it recurred twice
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
all verified against the other ports' checksums (`nbody` −169063618 = node = python, `json`
364568836 = node) rather than merely "it runs now".

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

**Blocked on:** `gh`'s stored token is invalid (`gh auth status`: "The token in
~/.config/gh/hosts.yml is invalid"), so API calls fall back to the unauthenticated 60/hour
limit and artifact/log downloads return 401/403. Re-authenticating (`gh auth login -h
github.com`) is what makes the next sighting readable.

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

## KI-21 — `nest run --for` / `--watch` emit a pre-ADR-150 `~p` pin

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

**Status:** ⚠️ **open (2026-08-21) — root-caused and localised; not fixed.**

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

## KI-48 — JIT tail dispatch read past the roots stack (open)

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
