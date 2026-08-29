# Plan — the post-JIT single-threaded compute frontier

> **Note on the background docs cited below.** `jit-tier2.md`, `jit-stage1.md`,
> `value-repr.md` and `frame-representation.md` were trimmed in `fdce5400` once the work
> they planned had shipped. Their conclusions are folded into this file, the ADR that
> cites them, or the source; the full text is recoverable from git history
> (`git show fdce5400^:docs/<name>.md`).

> ## ✅ CLOSED (2026-07-27) — the KI-14 guard cost is recovered; the named suspect was wrong
>
> The regression this block was opened for (**`fib` 57 → 75 ms, `pfib` 165 → 218 ms**, bisected to
> `f11f4cb`) is **fixed**: `fib` 88 → 74 ms and `pfib` 252 → 202 ms on a same-session A/B, which is
> parity with a build that has the guard removed entirely (73 / 199 ms). The guard keeps every bit
> of its teeth — `tests/jit_deep_recursion_test.blsp` passes unchanged, at unchanged wall time.
>
> **The lever named here was the wrong one, and the measurement said so immediately.** The
> hypothesis was that `stamp_stack_limit`'s `stacker::remaining_stack()` probe on every
> `jit_run_fast_link` was the cost. Hoisting it to the outermost native entry (plus a quantum-start
> stamp in `Process::drive`, since worker stack bases differ) measured **exactly zero on `fib`** —
> because `fib` runs on the **i64 register worker**, which recurses natively and never touches a
> fast link. A build with both prologue guards deleted recovered the full 16 ms, which located the
> cost precisely: the **per-frame prologue check**, not the stamp.
>
> Attributing *within* the prologue took three more one-line builds, and none of the obvious
> suspects was it — dropping the frame-address probe saved ~0, dropping the `limit` load saved ~2 ms,
> dropping the redundant `limit != 0` test saved 5. What cost the remaining ~11 ms was simply having
> **two** tests per level instead of one: the byte check ran *alongside* the old frame-count cap
> (`I64_DEPTH_LIMIT`), and `over_count | over_bytes` is a second compare on every level of a 30 M-call
> recursion. Deleting the count cap — which the byte check subsumes, and which was itself the thing
> KI-14 proved wrong — recovers all of it. The wrapper now refuses the register worker outright when
> the limit is `0` (probe unavailable), which is the case the count cap had been covering.
>
> **The stamp hoist was kept anyway**, on its own measurement rather than the one it was proposed
> for: it is worth ~5% on `bintree` (130 → 124 ms, best-of-15), the fast-link-heavy row, and nothing
> anywhere else.
>
> Method note worth keeping: every step here was a separately-built binary and a best-of-N A/B, and
> **the first two hypotheses were both wrong**. The cost of a wrong guess was ~16 s of build each,
> which is why guessing was cheap and reasoning was not.
>
> Still unmeasured, inherited from this block: whether the now-unconditional `stack_headroom_ok()`
> in `jit_dispatch_call` matters. It is on the slow call path, so probably not.
>
> ---

> ## ⏯ RESUME HERE (2026-07-24) — allocation frontier re-profiled; premise corrected
>
> Picking up the "allocation / GC frontier" item (`bintree` + `nqueens`, the two worst compute
> rows). **The stale framing was wrong and is now fixed:** the ROADMAP said these "run
> interpreted (~39×/187× behind Elixir)". They do **not** — both are cleanly JIT'd (`jit_deopt=0`),
> and the real gap is ~9.5×. `Cons` and small `MakeVector` are admitted to the JIT subset
> (`chunk_in_jit_subset`, `jit_plan.rs` — it moved out of `jit_lower.rs` in ADR-221), so
> structure-building arms lower and win.
>
> **Verified current numbers** (fresh `--bin brood` build; `make`-installed harness N):
>
> | bench | tree-walker | VM (no JIT) | VM+JIT | vs fastest (last archived run) |
> |---|---|---|---|---|
> | `bintree` N=200 | 8508 ms | 422 ms | **~119 ms** | 9.8× behind Elixir (10.4 ms), 6/7 |
> | `nqueens` N=10 | 3866 ms | 286 ms | **~100 ms** | 9.5× behind Node/Elixir (8.7/9.0 ms), 5/7 |
>
> JIT gives ~3.5× / ~2.9× over the plain VM. **`BROOD_GC_FLOOR` sweep: raising the nursery floor
> makes both *slightly slower* (bintree 118→138 ms), never faster** — so this is NOT
> GC-frequency-bound; it's allocation *volume* + dispatch. Nursery tuning is a dead end here.
>
> **Measured hotspots (`--features perf-stats`, `BROOD_PERF_STATS=1`). The spike's verdict is a
> NEGATIVE result — both rows already run at/near their JIT ceiling; there is no sound quick win:**
> - **`bintree` — 100% native, pure escaping allocation.** `vm_apply=48` (≈zero VM),
>   `jit_link_done=3.24M`, `jit_deopt=0`. `alloc=876,789` — ≈819K are the `[left right]`
>   `MakeVector(2)` per internal node (2^12−1 × 200; ~39 MB churned/run). The nodes are **returned
>   from `make` and walked later by `check`** → they **escape** → JIT escape analysis / scalar
>   replacement is **inapplicable**. Floored by the boxed 24-byte `Value` (48 B/node). The only
>   lever is a **narrower cell representation** — invariant-risky (a new `Value` kind, brushes the
>   NaN-boxing line §2 explicitly rejects) — or accepting the cap.
> - **`nqueens` — the step lambda already runs NATIVE; no stuck slow path.** The `reduce` step
>   lambda (`arm 12 <closure>`, confirmed in the `BROOD_JIT_DUMP_IR` dump at N=10) DOES JIT-compile
>   and runs native via the HOF fast-frame. Proof: `BROOD_NO_HOF=1` (force it off the HOF path) makes
>   `jit_apply_fast` jump **62 → 341,801** (it just moves to the dispatch fast-frame) and is *slower*
>   (119 vs 102 ms) — the fast paths are engaged and helping. The `vm_apply=42,617` is a **minority**
>   (native completions are in the hundreds of thousands: `jit_link_done=681,893`), `jit_deopt=0`.
>   Residual gap is allocation + inherent per-element overhead, not a dispatch bug.
>   **(Correction: an earlier draft of this block claimed nqueens was "capturing-closure dispatch
>   bound, stuck on the slow trampoline" and proposed a `!capture_names` fast-link fix. That was
>   wrong — the lambda is not stuck; it runs native. Disregard that lever for nqueens.)**
>
> **Conclusion for this push — do NOT sink effort here as a quick win.** Both rows are already
> JIT-native on their hot paths (bintree entirely; nqueens' lambda + safe?/solve). The ~9.5× gap is
> the boxed-24-byte-`Value` allocation floor + GC churn — the "foundational, multi-session bet with
> capped payoff, will not reach .NET/Node on raw numeric throughput" §4 always described. The only
> real lever is:
> 1. **`bintree` narrower-repr** — spends a core invariant (new `Value` kind / representation), for a
>    capped payoff. **Defer** — needs an explicit decision to spend an invariant; not a quick win.
> 2. **Escape analysis** — deprioritized: bintree's cells escape; nqueens has no stuck allocation.
>
> Better ROI lives elsewhere (JIT Stage-4 RUNTIME-compaction survival; closure-arm inlining) — see
> `ROADMAP.md` VM & JIT. Reprioritize unless the narrower-repr invariant spend is explicitly wanted.
>
> `perf record` was unavailable when this block was written (`perf_event_paranoid=4`), so the
> `BROOD_PERF_STATS` counters above were the substrate. **That changed on 2026-08-14:**
> `kernel.perf_event_paranoid=1`, and `perf record` works — see the 2026-08-14 profile in the
> `pipeline` entry of the benchmark repo's `FRONTIER.md`. Note `release-fast` sets `strip = true`,
> so a profiling build needs `CARGO_PROFILE_RELEASE_FAST_STRIP=false
> CARGO_PROFILE_RELEASE_FAST_DEBUG=line-tables-only make release-brood` (same codegen, symbols
> kept). Kernel frames stay unresolved (restricted kallsyms); user space is what matters.

> ## ⏯ RESUME HERE (2026-07-02) — unboxed-register JIT + HOF fast path shipped
>
> The big lever since 2026-06-20 (full play-by-play in `docs/devlog.md`, 2026-07-02): an
> **unboxed-register JIT calling convention** for int-/float-only recursive arms. A `fib`-class arm
> lowers to a compact worker `fn(a0..aN: i64|f64, depth, ovf) -> …` (`jit_lower_i64_arm` in
> `eval/compile/jit_lower.rs`) that recurses with args/results in **registers** — no boxing /
> roots-staging / fast-link dispatch at the recursive call boundary (the Increment-0 profile showed
> that protocol is ~55% of `fib`). Overflow-checked int → deopt to BigInt; float has no overflow
> (IEEE inf/NaN valid); covers multi-arg, `let`/`do`, rem/quot/bitwise (int) and `+ - * /` (float); a
> native-depth cap deopts deep recursion to the boxed path. Default-on (`BROOD_NO_I64` opts out).
> Fuzz-hardened: ~1600 chaotic int+float differential programs + boundary torture + concurrency/
> GC_STRESS, **0 bugs**; regression guard `tests/unbox_torture_test.blsp` + `scripts/fuzz_unbox.py`.
> - **`fib` 227→~52 ms (5th→2nd, beats Elixir & Node); `pfib` N=31 847→~152 ms (5th→2nd, 1.3× off .NET).**
> Also shipped: a **HOF closure-call fast path** (`range_reduce` caches the step closure's arm once →
> `nqueens` ~9%; `BROOD_NO_HOF`), and **`let*` removed** (Brood's `let` is already sequential).
>
> - **Standings** (single-thread aggregate compute vs fastest): .NET 1.0× · Node 2.6× ·
>   **Brood 3.0× (3rd of 7, ahead of Elixir 3.7×)** · Clojure ~8× · Ruby 11.7× · Python 26.9×.
>   Up from **6.0× / 4th** at 2026-06-20. Also fixed the `pfib` parallel-scaling cascade (per-process
>   fast-link invalidation + shared inlined native) — green scheduling now ~93% of the machine ceiling.
>
> - **Next levers** (profiled; details in `todo.md`):
>   1. ~~**Capturing-closure fast-link / lean HOF call**~~ — **KILLED for `pipeline` by profiling,
>      2026-07-03. Do not re-attempt on this evidence.** The lever as written ("`eduction`'s
>      transducer step closures capture → `vm_call_ic_fast_link` bails on
>      `!capture_names.is_empty()` → dispatch fallthrough; drop the bail") named the wrong path.
>      That bail is on the **elided free-global in-IR fast-link**, and `perf` measured that path at
>      **0% of pipeline**: a transducer step is a **computed head** (a captured `rf`/`f`), so it
>      never reaches the elided fast-link at all. The "Increment-0 confirmed GO" note was a
>      *ceiling* measurement (how much the row could theoretically give), not a confirmation that
>      the proposed mechanism was on the row's hot path — it was wrong about the mechanism.
>      Separately, both JIT fast frames now **fill** capture slots from the captured env rather
>      than refusing a capturing arm (`jit_runtime.rs`'s native→native link and `hof_apply_native`
>      in `compile/mod.rs` — verified present 2026-08-14), so there is no bail left to drop.
>      **What shipped instead** was `hof_apply_native` (jump the step arm's installed native
>      directly, skipping `vm_apply`→`vm_run_bc`): **`nqueens` ~18%**, and **`pipeline` flat** —
>      which is the datum that redirected this lever. Pure computed-head arm-caching was killed in
>      the same pass (~1–6%: `vm_cache` is already FxHash-keyed, so the residue is the `arm_for`
>      scan + an `Arc` clone, and `push_frame` does the same slot+capture work a fast frame would).
>      **The open lever for `pipeline`** is therefore the **VM `dispatch` computed-head branch**
>      (19.7% self at N=10M) + **`push_frame` (11.5%)** — give that path its own fast frame, as
>      `jit_dispatch_call` already has. Riskier core-dispatch change; profile before building.
>   2. **Unboxed arrays** (`matmul` — boxed 24-byte `Value` array reads; profile: `vector_ref` ~14%).
>      The "monomorphic → unboxed" theme applied to *storage*.
>   3. Interpreter/dispatch cost still bounds every un-JIT'd row.

> ## ⏯ RESUME HERE (2026-06-20, historical) — current perf state + 5-item work queue
>
> The newest work is the **JIT call-dispatch + loop-overhead** round (full play-by-play in
> `docs/devlog.md`, entries 2026-06-18/19). Status:
>
> - **SHIPPED + default-on:** the **in-IR call fast-link** (Technique A increment 1 — a JIT'd
>   non-tail free-global call epoch-guards a flat `#[repr(C)]` mirror of the call IC in IR and
>   calls `brood_rt_fast_frame`, skipping the IC probe + `RefCell` borrow; **fib ~20%**), gated by
>   `BROOD_JIT_ICALL` (now **default-on**, `BROOD_NO_JIT_ICALL=1` opts out). Plus two back-edge FFI
>   eliminations on self-tail loops: raw-load the global epoch (`brood_rt_global_epoch_ptr`) and
>   skip the `brood_rt_tick` preemption poll in non-capture mode (`brood_rt_in_capture`, read once at
>   entry — capture-mode path unchanged). **`loop` 0.14→0.09 s (~36%).** All gated: jit.rs 28/28,
>   differential, nest 2161, preemption/reductions/work-stealing, GC-stress+verify.
> - **NO-GO — do NOT re-attempt:** Technique A **increment 2** (full in-IR frame setup —
>   `#[repr(C)] RootStack` + in-IR `len`/nil-fill/depth/`call_indirect`). Implemented + correct but
>   ~5% SLOWER than the `brood_rt_fast_frame` FFI. **The FFI boundary is not the bottleneck** — LLVM
>   compiles the frame work better than hand-emitted Cranelift IR. Reverted. The dispatch lever is
>   mined out at increment 1.
> - **Standings (full 7-language `brood-benchmarks` run, single-thread aggregate compute vs the
>   fastest):** .NET 1.0× · Node 2.7× · Elixir 3.5× · **Brood 6.0× (4th of 7)** · Ruby 11.9× ·
>   Clojure 18.2× · Python 27.3×. Brood wins `strings` + `http`; ~18 MB base RSS; ~26 ms startup.
> - **SHIPPED 2026-06-19 — `%map-int-add` + JIT GC safepoint:** `wordcount` 810→**470 ms** (~42%).
>   `(%map-int-add m k delta)` fuses `(assoc m k (+ (get m k 0) delta))` into one CHAMP trie walk.
>   Added GC safepoint in `jit_dispatch_call`'s slow-path `Ok(v)` arm — roots `v` before
>   `heap.collect`, fixing the 1770 MB RSS regression that plagued the JIT path for native callees.
>   wordcount gap: ~31× → ~13× off the fastest.
> - **SHIPPED 2026-06-19 — `nil?`/`pair?`/`empty?` as native builtins + PrimOp1::IsNil/IsPair:**
>   bintree 383→230 ms (−40%), nqueens 504→320 ms (−36%). These predicates were Brood closures;
>   every call pushed a BcFrame (~100–150 ns). As native builtins `dispatch()` returns `Step::Done`
>   inline — no BcFrame. `nil?`/`pair?` also compile to `Prim1::IsNil`/`IsPair` (single tag-check),
>   eliminating all dispatch overhead for compiled arms. `chunk_walks_structure` updated to only gate
>   on `First`/`Rest` (heap deref), not `IsNil`/`IsPair` (tag-only). **7.7× → 7.0×** overall.
> - **SHIPPED 2026-06-19 — lift `chunk_walks_structure` gate + fix Prim2SlotInt VectorRef:**
>   bintree 241→**116 ms** (−52%, 2.3× speedup). The gate was correct pre-fast-link (JIT `check`'s
>   recursive calls cost same as VM's BcFrame then), but now fast-link makes JIT→JIT calls ~35–40 ns
>   vs ~150 ns BcFrame — so two-call structure-walking arms gain. Also fixed: `Prim2SlotInt { VectorRef }`
>   (constant-index `nth`) was bailing with `return None`; now materialises the integer index as a
>   Value word-triple and calls `vector_ref`. Deleted `chunk_walks_structure` (dead code). **7.0× → 6.1×**
>   (sum aggregate). nqueens flat (safe? was already JIT-compiled, no VectorRef).
> - **SHIPPED 2026-06-19 — `PrimOp1::IsEmpty`:** nqueens 321→**166 ms** (−48%, 12.5× → 7.4× behind
>   .NET). `empty?` was a native builtin (no BcFrame), but JIT arms still emitted `brood_rt_call_slow`
>   (~150 ns/call). `safe?` calls `empty?` once per list iteration → O(n²) FFI calls in the inner
>   loop. `IsEmpty` emits: read tag byte; `is_nil = (tag == 0)`; `is_pair = (tag == TAG_PAIR)`;
>   `brif(is_nil|is_pair, cont, deopt)` — deopt for Vec/Str/Map (need heap length); push
>   `Op::Int(is_nil)`. Also VM inline paths (single-eval + bytecode-compiled). **6.1× → 6.0×**
>   aggregate (nqueens is 1 of 15 compute benchmarks; geomean barely moves).
> - **SHIPPED 2026-06-19 — register-carry for loop-carried Int params:** loop 60→**38 ms** (−37%),
>   collatz 359→**320 ms** (−11%). Pure-arithmetic self-tail loops carried all loop state through
>   `roots` slots — every read of a loop-carried integer emitted a tag-check + 2 memory loads.
>   Fix: declare Cranelift `Variable`s for slots `0..carry_argc` (phi-node SSA), `def_var` once at
>   entry and at each SelfCall back-edge, `use_var` in `load_slot_int`. Zero memory ops, zero
>   branches per carry slot. Eligibility: `int_carry_eligible` (SelfCall, no non-tail Calls, no
>   Cons/MakeVector/First/Rest) + all carry slots profiled as `TAG_INT` (critical — `!= TAG_FLOAT`
>   was a latent bug that would deopt vector-param functions on every call). Aggregate: **6.0×
>   (unchanged)** — dominated by wordcount/fib; per-benchmark improvements are real.
> - **SHIPPED 2026-06-20 — float register-carry + F64 SSA value cache:** mandelbrot 224→**204 ms**
>   (−9%), 3rd of 7. Float carry extends `carry_vars` to `Vec<(Variable, is_float)>` — slots
>   profiled TAG_FLOAT get an F64 Cranelift Variable (entry: tag-check + bitcast i64→f64; back-edge:
>   `def_var` with new F64; reads: `use_var` — no memory ops). F64 SSA cache
>   (`slot_f64_cache: RefCell<Vec<Option<Value>>>`) covers let-bound floats not in carry params:
>   `store_op(Op::Float(v))` stashes `v`; `as_f64(Op::Slot(k))` returns it directly. Eliminated
>   4 full tag-check+load+bitcast sequences per inner iteration for `nx²`/`ny²`. Key safety note:
>   `slot_float[k]` is NOT safe to skip tag-checks (single-pass, cross-branch contamination caused
>   a real test failure); only the cache (populated on the actual store path) is safe.
>   Aggregate: **6.0× (unchanged)** — mandelbrot is one of 15 compute rows.
> - **SHIPPED 2026-06-20 — 2-level Brood-level self-recursive body inlining:** fib 532ms (no-inline)
>   → **277 ms** (−48% vs baseline, best of 3). `inline_self_calls` is run twice in both
>   `self_inline_probe` and `rederive_inlined_body` (probe and rederive must be identical). Level-1
>   inlines the 2 non-tail self-calls from the original fib body; level-2 inlines the 4 external
>   calls left over — each using the original body as template and a continuing `next_block` counter
>   so slot ranges are disjoint. Guard: after level-1 `node_count(body) > SELF_INLINE_MAX_BODY`
>   prevents level-2 for already-large bodies. `node_touches_heap` keeps bintree/sort-walk at
>   level-1 (no regression). Debug: `BROOD_INLINE_DBG=1` → `new_max=7 inline_nslots=14` for fib(35).
> - **SHIPPED 2026-06-20 — `bit/and`/`bit/or`/`bit/xor` as PrimOp2:** sort 238→**209 ms** (−12%).
>   sort's `gen` function used `bit/and` in a self-tail loop; that emitted `brood_rt_call_slow`
>   (~150 ns/call) AND blocked int register-carry for gen's loop variables. Making them PrimOps
>   eliminates the Call, re-enables carry, and removes ~56 ms per sort run (N=375K). Same 7-location
>   PrimOp pipeline: enum, `from_native_name`, `prim2_int_fast`, `prim_apply`, `prim_apply_float`
>   (explicit float defer), `chunk_in_jit_subset`, `emit_arith` (CLIF `band`/`bor`/`bxor`).
> - **SHIPPED 2026-06-20 — max/min as PrimOp2 native + cranelift `select`:** collatz 323→**111 ms**
>   (−66%), 4th→4th of 7. Replaced the prelude's `(defn max (x & xs) (fold (fn …) x xs))` with a
>   native builtin (`prim_max`/`prim_min`: Int fast-path → BigInt exact → float coerce,
>   `Arity::at_least(1)`) and a JIT-inlined `icmp(SGE/SLE)` + `select` pair. The old definition
>   allocated ~2 heap cells per 2-arg call (one cons for the `xs` rest arg, one closure for the
>   fold lambda); collatz's inner `(math/max best (steps k 0))` ran 250K times = ~500K allocs/run.
>   `PrimOp::Max`/`Min` added to the full PrimOp pipeline: `from_native_name`, `prim2_int_fast`,
>   `prim_apply`, `prim_apply_float`, `chunk_in_jit_subset`, `emit_arith`. No overflow guard needed
>   (max/min are branchless). Aggregate: **5.9× → ~5.8×** (collatz now comparable to Node's 182ms).
>
> **Work queue (5 items, see §3e–§3i for details):**
>
> 1. ~~**car/cdr inline in JIT** (§3e)~~ **SHIPPED 2026-06-20** — nqueens 163→**137 ms** (−16%).
>    3-file change: `heap.rs` exposes `local_pair_nursery_base`/`local_pair_old_base`; `jit/rt.rs`
>    exports them via `builder.symbol()` (critical: without this the JIT linker can't resolve the
>    symbols even though they're `#[no_mangle]`); `compile/jit_lower.rs` hoists both pointers at arm entry
>    (`pair_bases: Option<(nursery, old)>`) when the arm has First/Rest AND no Cons, then emits
>    `ushr(w1,62)==0` region guard → `ushr(w1,61)!=0` age select → `base+idx*48+{0,24}` loads.
>    bintree flat (uses vectors not pairs). sort: ~215ms now (pair reads were not the bottleneck).
> 2. ~~**range-fold JIT bypass** (§3f)~~ **SHIPPED 2026-06-20** — reduce 139→**28 ms** (−80%).
>    The previous fast path (already in place) used `prim_apply_step(op, Value::int(acc), Value::int(i))`
>    per element — but `Value` is 24 bytes (> 16), so SysV ABI passes it by pointer: 72 bytes of
>    stack traffic + a non-inlined call per iteration = ~24ns/elem despite the prim detection.
>    Fix: `prim_apply_int_step(op: PrimOp, a: i64, b: i64) -> Option<i64>` takes/returns raw i64
>    with `#[inline]`, so the compiler emits a tight 2-instruction loop (add + overflow branch
>    never-taken). When init is `Int` and prim resolves, the tight path runs; overflow or
>    non-Int init falls through to `range_reduce_slow` (the old root_scope path). Result:
>    5M iters in ~3ms (~0.6ns/iter, 2 CPU cycles), down from ~112ms. Startup dominates (25ms).
> 3. ~~**sort list-walk** (§3g)~~ **PARTIALLY ADDRESSED 2026-06-20** — `bit/and` PrimOp removes
>    ~12% overhead from gen's loop; residual cost is `list_with_tail` O(n) pair allocs (structural,
>    needs mutable-sort or `sort-vec` variant). **209 ms** on N=375K.
> 4. ~~**fib call inlining** (§3h)~~ **SHIPPED 2026-06-20** via Brood-level 2-level inline —
>    **277 ms** (best of 3), down from 532ms no-inline / ~320ms level-1. True CLIF inlining would
>    eliminate the remaining 8 `brood_rt_fast_frame` calls per arm activation; marked long-horizon.
>
> - **Build/bench discipline:** perf bins via `cargo build --release --features jit --bin brood`
>   (NEVER `-p brood` — stale-lib trap); `make install` before benchmarking (`cp target/release/brood
>   ~/.local/bin/brood` — the harness runs the *installed* `brood`, not `target/`); GC-debug build
>   = `RUSTFLAGS="-C debug-assertions=on"`.
>
> ---
>
> **Status: in progress (2026-06-15). Lever 1 (matmul LICM) shipped — local AND global
> hoist** — see the devlog entries "JIT matmul LICM" + "the global lever". Both invariant
> `nth`s inlined (the local `rowa`; the global `b` via a back-edge `global_epoch` guard):
> matmul **~241 → ~171 ms compute / ~30× gap** (was ~45×), now beating both interpreters;
> isolated invariant read ~7.8→~1.2 ns. Lever 2 (zero-copy messages) is next. The JIT and
> easy codegen-shaped wins are landed (geomean **19.5× → 13.5×** off the fastest runtime
> across the single-threaded suite). This note scopes what's *left* and — importantly —
> records that the remaining gaps are **data-structure-specific**, not the `Value`-width
> question (which `value-repr.md` already settled).

See also: `value-repr.md` (the `Value`-enum-width decision — **keep the 24-byte enum**,
§5), `jit-tier2.md` / `jit-float.md` / `jit-stage1.md` (the JIT as built),
`benchmarking.md`, the `brood-benchmarks` repo.

## 1. Where we are (what shipped)

Codegen-shaped JIT wins this round (all landed + benchmarked):

| fix | benchmark effect |
|---|---|
| bool literals into the JIT subset + `Value::Bool` truthiness mask | `primes` 351→56 ms, `nqueens` 933→523 ms |
| left-fold n-ary `+`/`*` into native 2-ary ops | `bintree` 1123→452 ms |
| top-level-lambda promotion (freeze an inline `(fn …)` body into RUNTIME) | `pipeline` 552→122 ms (~4.5×), `matmul` 542→241 ms (~2.2×) |
| lower `and`/`or` (zero-extend an `i8` comparison crossing a block boundary) | `mandelbrot` 1326→250 ms (**~5.3×**, the biggest single win) |

These were all "the JIT couldn't *express* this shape" gaps. **That well is now dry.**
A profiling sweep of every remaining weak row (2026-06-14) confirms the rest are
**not** codegen-expressibility gaps.

## 2. The `Value`-width question is NOT the lever (already settled)

`value-repr.md` §4 measured it directly: padding the operand slot 16→32 bytes made **zero**
difference on the compute loops (they're CPU/dispatch-bound and stay L1-resident). So a
single-word/NaN-boxed `Value` buys ~zero at tier-1; its only upside is tier-2
register-passing, deferred. **Nothing this round changes that** — do not reach for
NaN-boxing to close the rows below. (Tracked there; not re-opened here.)

## 2b. A variadic call in a loop cost ~36% — FIXED 2026-08-28 (`collatz` −38.2%)

Found by publishing a benchmark run. `collatz` read 95 → 185 ms against the previous published
run with no runtime regression: the port had to migrate off the bare `rem`/`quot`/`max` the
namespacing waves retired, and one of the three names it moved to is **variadic**.

Isolated on one binary, same program shape:

| variant | time |
|---|---|
| `math/rem`, `math/quot`, `math/max` all qualified | 223 ms |
| the same, but `%max` called directly | **142 ms** |
| all three primitives directly | **142 ms** |

The middle row equals the bottom row, so `math/rem` and `math/quot` are **free** and the whole
delta is `math/max`, called once per `sweep` iteration.

**Why, from the JIT dump.** `steps` lowers to native with **no `Call` instruction at all**:

```
arm: 17 (steps)  insts: Prim2SlotInt JumpIfFalse Local Jump Prim2SlotInt Const Prim2
                        JumpIfFalse Prim2SlotInt Prim2SlotInt SelfCall Jump …
```

A fixed-arity body that is one primitive call is **inlined into its caller and disappears** — so
`math/rem`/`math/quot` never appear as lowered arms *and* never appear in `BROOD_JIT_BAIL_TRACE`.
That absence is easy to misread as "they never lower, so the loop pays a VM round-trip per op";
it means the opposite, and the isolation table is what settles which. `math/max` by contrast
lowers to `GlobalIc Local Call` — it is `(apply %max xs)` over `& xs`, so it allocates an argument
list per call and cannot be inlined.

**So the lever is variadic dispatch, not wrappers.** This is the case CLAUDE.md's dogfooding
section already uses as its worked example ("variadic `+`/`-`/`=` … ~40× a direct call") with the
prescribed fix: efficient **multi-arity dispatch in the evaluator**, which keeps the functions in
Brood and makes *every* multi-arity call faster rather than two. `math/max` and `math/min` are the
`math` entries in that shape; `collatz` and `latency` are the rows that call them in a loop.

**Fixed, in Brood, using capability the language already had.** `max` and `min` were
single-clause `(& xs)` over `(apply %max xs)`. They now carry a **two-argument arm**, which is
exactly how the prelude keeps its own variadic arithmetic fast — `<=` spells its 2-arg body
`(%le a b)` and its comment says why: "so the ADR-069 thin-wrapper elision reaches it". Nothing
in the kernel changed; no multi-arity dispatch had to be built, because
[`docs/language.md`](language.md) already guarantees an arity arm "binds its params *directly*
(no rest-list), so it's as cheap as a single-clause fn".

```lisp
(defn max "…"
  ((a b) (%max a b))
  ((& xs) (apply %max xs)))
```

Measured, `make ab` against the parent commit, best-of-11 with `--floor`:

| row | base | new | delta | floor | verdict |
|---|---|---|---|---|---|
| `collatz` | 228 ms | 141 ms | **−38.2%** | 0.4% | improved |
| `latency` | 4743 ms | 4589 ms | −3.2% | 2.4% | noise (same direction; the other `math/min` row) |
| `loop`, `fib`, `primes`, `sort`, `json` | | | ±1.4% | | flat — no regression |

The qualified call is now indistinguishable from calling the primitive: 139 ms against the
`%max`-direct control at 141 ms and the all-primitives lower bound at 140 ms, where it had been
223 ms. `math/max` no longer appears as a lowered arm at all — it is elided into its caller like
`math/rem`.

**Guarded** by `tests/math_test.blsp`'s "the two-argument arm agrees with the variadic path" —
equal args, negatives, mixed int/float, and 2-arg-vs-variadic agreement. Sabotage-verified: the
suite goes red naming the assertion (5151 in-language tests, 1 failed). The point of pinning the
2-arg case separately is that a wrong arm would leave every existing many-arg test green while
silently changing the answer at the arity everyone uses.

**Not done, deliberately:** `bytes/concat` and `hash-map` are the only other single-clause
`(& rest)` wrappers over an `apply` in `std/`. Neither is shown to be hot, and adding arms on
speculation is how a codebase accumulates changes nobody can point at a measurement for. They are
candidates if a row ever implicates them.

**Two side results worth keeping.** Qualification is free — `math/rem` referred bare via
`(:use math)` measured 204 ms against 205 ms qualified — so the module system costs nothing at a
call site. And the leaf/one-prim inlining is already doing its job, which is a better starting
position than the earlier framing implied: there is no missing inliner to build here, only the
variadic shape it cannot reach.

## 2c. Re-profiled 2026-08-28, before starting the call-convention work

`perf` is unusable on this box (`perf_event_paranoid: 4`), so this is brood's own counters via
`(perf/measure …)` — **scoped**, because process-global totals are dominated by boot. That is not
a theoretical caveat: at whole-process scope `pipeline` reported `alloc` and both call-IC counters
*identical* between a 1× and a 10× run while `env-get` scaled 8×, i.e. the interesting counters
were all boot's.

The method that works: run the same program at two sizes and keep only the counters whose ratio
tracks the work. Everything else is fixed cost and cannot be what you are chasing.

### `pipeline` — the "allocation churn" claim does not hold

| counter | N=100k | N=1M | ratio | per element |
|---|---|---|---|---|
| `env-get` | 140,182 | 1,400,182 | **10.0×** | 1.40 |
| `jit-link-done` | 101,623 | 1,482,098 | **14.6×** | 1.48 |
| `jit-fast-tail4` | 30,043 | 450,869 | **15.0×** | 0.45 |
| `alloc` | 15 | 15 | 1.0× | **~0** |
| `vm-apply`, `prim2-inline`, `tail-call`, `call-ic-*`, `hof-decline-queued`, `n-compile` | | | 1.0× | fixed |

**`alloc` is 15 and flat.** `alloc_slot!` is the single macro behind `alloc_pair`/`alloc_vector`/
`alloc_map`/`alloc_closure` and the rest, so that is all LOCAL heap allocation — the lazy
`lfilter`/`lmap` form genuinely streams without allocating per element, which is what it is for.
So FRONTIER's "allocation churn dominates" for this row is **wrong on this tree**; the per-element
cost is ~1.5 fast-linked calls plus ~1.4 environment lookups. The "~50% call plumbing" half of
that entry stands; the allocation half does not.

Also worth recording: `hof-decline-queued` is ~33k and **fixed**, not per-element. Read at
process scope it looks like "the HOF fast path is declined on 96% of activations"; scoped and
size-swept it is a one-off warm-up cost.

### `bintree` — FRONTIER's characterisation is confirmed

| counter | n=100 | n=200 | ratio | per iteration |
|---|---|---|---|---|
| `alloc` | 409,501 | 819,001 | **2.0×** | **4,095** |
| `jit-link-done` | 1,519,372 | 3,154,494 | **2.1×** | **15,772** |
| `jit-native` | 196 | 397 | 2.0× | 2 |
| `vm-apply`, `env-get`, `prim2-inline`, `call-ic-hit` | | | 1.0× | fixed (the arm is native, so its iterations stop being counted — see the `:alloc-bound` caveat) |

4,095 allocations per iteration is exactly 2¹²−1, one per node, and 15,772 links per iteration is
**3.85 per node** — an independent confirmation of the entry's "~77 ns per node over four non-tail
calls" from a counter rather than a stopwatch. So `bintree` really is one allocation and ~four
calls per node, and the call-convention work is aimed at the right row.

## 2d. `perf` profiles, 2026-08-28 — the call protocol confirmed, and the boot trap that nearly hid it

First real per-symbol profiles (see [benchmarking.md](benchmarking.md) for the two blockers:
`perf_event_paranoid` and `strip = true`). Frame-pointer call graphs — `--call-graph dwarf`
produced no usable chains; `-C force-frame-pointers=yes` does.

**Read the size caveat first.** Boot is **~47 ms** on this tree. `bintree` at its benchmark size
(n=200) runs 160 ms, so **29% of that profile is boot** — and boot is macro-expansion-heavy, so it
shows up as `Heap::env_get` (14.4%) and `eval_tail_loop` (4.25%), which reads exactly like "the row
is partly interpreted". It is not: at n=2000 both symbols **vanish entirely**. The tree-walked form
count is identical at n=3 and n=6 (13,253 both), i.e. fixed startup work, zero per-node. Profile
below ~1 s and you are largely profiling boot.

### `bintree`, n=2000 (boot ~4%)

| % | symbol | group |
|---|---|---|
| **24.03** | `jit_runtime::jit_run_fast_link` | call |
| 12.73 + 12.37 | `brood_jit_arm_53` / `_54` | **the real work — 25.1%** |
| 11.38 | `__memmove_evex_unaligned_erms` | copying |
| **10.67** | `brood_rt_fast_frame` | call |
| 5.66 | `brood_rt_make_vector2` | alloc |
| 3.95 / 2.78 / 2.15 | `brood_rt_push_n` / `fastlink_base` / `roots_base` | call |
| 2.15 / 1.97 | `drop_glue::<Slabs>` / `__memset` | alloc |

**Call protocol ≈ 44%, real work 25%, allocation ≈ 21%.** So the entry's "~77 ns per node over
four non-tail calls" is right, and the counters agreed independently (3.85 links/node).

### `pipeline`, N=10M (boot <2%)

`dispatch` 13.48 · `jit_dispatch_call` 7.90 · `vm_cache_arm_handle` 5.74 · `passthrough_arm` 5.35 ·
**SmallVec staging 5.22 + 2.80 + 1.92 = 9.94** · `Heap::closure` 4.89 · `push_frame` 4.29 ·
`apply_value` 3.02 · `code_gen_pinned` 2.87 · `capture_value` 2.67 · `compiled_arm_for` 2.60 ·
`hof_apply_step` 2.44 · `select_arm`'s `max_by_key` fold 2.01.

**~50% call plumbing**, matching the older profile, with argument staging ~10%.

### Ranked, and two of my own wrong answers on the way

1. **`jit_run_fast_link` 24% + `brood_rt_fast_frame` 10.7% of `bintree`** — 35% between them, and
   `jit_run_fast_link` alone costs as much as all the native compute. Plus 11% `memmove` that is
   very likely the same frame/roots copying. **Start here.**
2. `dispatch` 13.5% of `pipeline` — the interpreted-side call path.
3. Argument staging ~10% of `pipeline`. Real, but a third the size of (1).

**Withdrawn on the way here, both from reasoning instead of measuring:** that argument staging
must be cheap because `SmallVec<[Value; 4]>` keeps ≤4 args inline (true, and irrelevant — the cost
is the *copy*, 9.9%); and that `env_get` was the top cost and the row was partly interpreted (a
boot artifact, gone at n=2000). Neither survived a profile. The rule that keeps surviving:
**measure at two sizes and keep only what scales.**

## 3. The remaining gaps are data-structure-specific (measured 2026-06-14)

Profiled with `--features perf-stats` (`BROOD_PERF_STATS=1`) + `BROOD_JIT_DUMP_IR`.

### 3a. `matmul` (~30× — the largest gap; LICM local+global shipped 2026-06-15) — **inline VectorRef via hoisted immutable base**

- The hot `dot` loop **already runs native** (it lowers, `define_function` succeeds, it's
  dispatched native; the high `prim2_fallback` is the one-time matrix *construction*, not
  the loop). Confirmed: **not a deopt, not a missing-codegen path** — the old devlog note
  ("data-dependent deopt") was wrong.
- The cost is the **per-element `VectorRef`**. Microbenchmark: a 30 M-iteration loop with
  one `nth` per step costs 0.33 s vs 0.11 s without — **~7.3 ns per `VectorRef`**. For
  `matmul` N=175 (~16 M inner reads × the calls in `dot`) that's roughly **half** the
  241 ms.
- Why it's a call, not an inline read: `nth` lowers to `brood_rt_vector_ref` (a real call:
  marshal 6 words in, return a 24-byte `Value` via the out-pointer ABI, plus a
  **`boxcar::Vec` segment lookup** + bounds check). RUNTIME vectors are `boxcar` (segmented,
  append-only) so the index→(segment,offset) math can't be cheaply reproduced in CLIF.
- **The lever (revised 2026-06-14 — immutability makes this tractable, not a flat-storage
  rewrite):** a vector built with `(into [] …)` is **immutable**, so its `(data-ptr, len)`
  never change. The JIT can therefore **hoist a loop-invariant vector's base out of the loop
  once** (one `brood_rt_vector_base`-style helper call returning the inner `&Vec<Value>`'s
  ptr+len) and **inline `ptr + idx*stride` reads** for the rest of the loop — turning the
  per-element call into a ~1 ns load. The usual blocker for hoisting a load (alias analysis:
  proving no write invalidates it) **does not apply** — immutability guarantees no write
  exists — so even this *template* JIT can do the LICM **soundly**. In `dot`, `rowa` is
  loop-invariant (hoistable → inline); `(nth b k)` / `(nth (nth b k) j)` vary with `k` (still
  a per-`k` base fetch, since `boxcar`'s segmentation resists a pure-CLIF arbitrary-index
  read). What it still can't beat without unboxing: .NET reads a register `long`, Brood a
  24-byte boxed `Value` — so this **narrows substantially** but doesn't fully close matmul.
  See §6 for why this is sound.
- Entry points: `eval/compile/` — `let vector_ref =` (the JIT helper in `jit_lower.rs`, currently
  emits the call), `chunk_in_jit_subset`/`resolve_prim` (`nth` → `PrimOp::VectorRef`),
  `jit/rt.rs::brood_rt_vector_ref` (the runtime helper); `core/heap.rs` `vector()` + the
  `CodeSlabs.vectors` boxcar (the storage to flatten).

### 3b. `bintree` (~?×) — **car/cdr FFI per tree step (§3e is the lever)**

After `chunk_walks_structure` removal (2026-06-19), bintree ~116→~127ms (local noise). It now
JIT-compiles `check` correctly but every `first`/`rest` call still emits a `brood_rt_car`/`cdr`
FFI (marshal 3 words in, write 3 words to an out-ptr stack slot, region-dispatch inside). The
`check` walk touches every node twice per tree — 200 trees × 8190 nodes × 2 = ~3.3M FFI calls
per run. See §3e for the inline approach.

### 3c. `strings` / `pipeline` — **lazy sequence combinators** — *shipped (pipeline)*

`strings` (~19×) and `pipeline` (the eager part) materialize a full cons list per stage
(`(map str (range n))`) which the copying GC then relocates — `strings` is also
the memory outlier (~180 MB). The lever is a **lazy/streaming, fusing pipeline** so a
chain folds instead of building intermediate lists.

**Shipped** (ADR lazy-seq-view): a `Value::SeqView(VecId)` kernel kind mirroring
`Value::Range` (a distinct tag over the vector slab, backing `[source xform]`, `tag = Pair`).
`fold` fuses over it — `(fold (xform rf) init source)` — so the pipeline walks the source
once with no intermediate lists; `seq`/`first`/`count`/… realise on demand.

**Design choice — fusion is opt-in, `map`/`filter` stay eager.** Making `map`/`filter`
lazy *by default* breaks Brood's entrenched "iterate for side effects" idiom — the module
loader (`(map require-one …)`) and the test runner (`(map run-test …)`) rely on eager
evaluation, and a lazy view silently drops those effects (immutability covers *data*, not
*I/O*). So the eager combinators are unchanged and the fusing views are explicit:
`lmap`/`lfilter`/`lkeep`/`lremove`, threaded with `->>` (the transducer plumbing that backs
them is internal — `%x*` — not public surface). Measured `pipeline` (n = 1e6): eager
`(->> … (filter …) (map …) (reduce +))` ≈ 2.0 s / 173 MB → fused
`(->> (range n) (lfilter …) (lmap …) (reduce + 0))` ≈ 0.63 s / 13 MB
(~3.3× faster, ~13× less memory).

**`strings` — partly fused, residual deferred (measured 2026-07-09).** `join` over a
view realises the *fused* view to a single strings list (`(seq view)`), then the native
`%string-join` walks it. This already **beats eager** because the stages fuse — no
per-stage intermediate list: measured n = 1e6, `(join "," (lmap str (range n)))`
≈ 0.51 s vs eager `(join "," (map str (range n)))` ≈ 0.81 s (~1.6×); the
two-stage `->>` view ≈ 0.37 s vs eager ≈ 0.61 s. The *only* residue is the final strings
list (the transformed elements, which the Brood transform closures must produce anyway) plus
its list→Vec pass. Eliminating that would need a **string-builder reducer folding straight
into one buffer** — i.e. a mutable-buffer accumulator driven per-element from inside
`%string-join` (via the `apply` callback). That is a single-call-site win of ~1.3× at best
(the per-element closure call dominates — cf. the closure-free range fast path at ~25 ms),
it fights ADR-026 (the acc would be observably mutable unless hidden behind the `%map-into`
GC-quiet discipline), and driving the transducer's `rf` protocol from Rust is reentrancy-
fragile under the green scheduler. Verdict: **deferred as low-ROI** — it doesn't clear the
"optimize only to build a *broad* primitive" bar (`~/CLAUDE.md`); the view path already fuses
the stages, which was the real win. Entry if revisited: `%string-join` in
`builtins/sequences.rs` + the `apply` callback helper in `builtins/mod.rs`.

A second, immutability-enabled lever for the memory side (and for `spawn`/`pfib`'s
message cost): **zero-copy message passing.** Today `to_message` *deep-copies* a value
across a process boundary because LOCAL heaps are isolated. But an **immutable** value can
be **shared by handle** (an `Arc` bump, no copy) once it lives in a shared region — exactly
what `Message::StrShared` already does for large strings. Extending that to whole immutable
structures (lists/vectors/maps) would cut both the copy cost and the peak RSS. Sound *only*
because the value can't be mutated out from under a sharer. Entry: `process/message.rs`
(`to_message`/`StrShared`), `core/heap.rs` (the shared RUNTIME region + `promote`). See §6.

### 3d. `wordcount` (~13×) — **persistent map build; `%map-int-add` shipped**

**SHIPPED 2026-06-19:** `%map-int-add` (single-pass CHAMP fused get+add+assoc) + JIT GC
safepoint in `jit_dispatch_call`. wordcount 810 → **422 ms** compute; gap vs fastest
(Node ~33ms) **~31× → ~13×**; gap vs Elixir **4.5× → 2.5×**.

Residual gap is algorithmic: CHAMP path-copy allocates O(log₁₆ N) nodes per update vs a
mutable `Dictionary`. A transient-map build path exists (`5a7b8bb`); wiring
`into`/`reduce`-into-a-map through it is the only realistic remaining lever short of
abandoning persistence. Lowest priority — most inherent to Brood's identity.

### 3e. `bintree`/`nqueens`/`sort` — **inline `first`/`rest` slab reads in the JIT**

**Root cause.** `PrimOp1::First` and `PrimOp1::Rest` in the JIT currently tag-check for
`Pair` (deopt otherwise), then call `brood_rt_car`/`brood_rt_cdr`. Each call marshals 5 args
(heap ptr, out ptr, w0/w1/w2) and inside: reconstructs the `Value` from 3 words, matches on
`Pair(id)`, dispatches on `id.region()`, indexes into the right slab, and writes 3 words to
`*out`. Cost: ~20–30 ns per `first`/`rest`. bintree has ~3.3M such calls/run; nqueens has
similar list-walk density; `sort`'s `seq_items` and `hash--acc` also pay it.

**Layout facts (load-bearing for the inline).** A `Value::Pair(PairId)` under `#[repr(C, u8)]`
is exactly 3 i64 words:
- `w0` — tag byte (low 8 bits = `TAG_PAIR = 9`), upper bits 0
- `w1` — `PairId` u64: bits 0..31 = index; bits 32..60 = gen epoch; bit 61 = age
  (0=nursery, 1=old); bits 62..63 = region (LOCAL=0, PRELUDE=1, RUNTIME=2)
- `w2` — 0 (padding)

The pair slab entry is `(Value, Value)` = 48 bytes. Car at offset 0, cdr at offset 24. For
LOCAL pairs (region=0), the slabs are plain `Vec<(Value, Value)>` — flat arrays, so
`base_ptr + index * 48 + {0,24}` is a valid inline load. PRELUDE pairs never move (stable
pointer for the process lifetime). RUNTIME pairs are `boxcar::Vec` (segmented) — complex to
inline; fall back to FFI.

**Proposed approach.**
1. Add `brood_rt_pair_bases(heap, out_nursery: *mut *const u8, out_old: *mut *const u8)` to
   `jit/rt.rs` — writes the nursery `pairs.as_ptr()` and old-gen `pairs.as_ptr()` as raw
   byte pointers. Call once at JIT function entry (like `brood_rt_roots_base`).
2. In `jit_lower_arm`'s `Inst::Prim1 { First | Rest }` arm: after the existing `TAG_PAIR`
   tag-check, extract `region = (w1 >> 62) & 3` and `age = (w1 >> 61) & 1` and `idx = w1 &
   0xFFFF_FFFF`. Emit a branch: LOCAL (region==0) → use nursery or old base (via age bit) +
   `idx * 48 + {0,24}`; non-LOCAL → fall back to `call_handle(car_ref/cdr_ref, [w0,w1,w2])`.
3. Safety: LOCAL pair slabs can grow only via `cons` (nursery push). For arms that don't
   call `cons` (`bintree`/`nqueens` structure-walks, `sort`'s `seq_items`/`hash--acc`), the
   base pointer is stable for the arm's duration. Arms that DO call `cons` must use the FFI
   path (can gate: if the arm contains `Cons`, skip the inline). The epoch guard covers GC
   relocations. PRELUDE is immutable; can add `brood_rt_prelude_pair_base` later for that
   region.
4. `chunk_in_jit_subset` already admits `First`/`Rest`; no gate change needed.

**Expected gain.** Eliminating the FFI boundary + 3-word marshal + out-ptr copy per
`first`/`rest`: ~20–30 ns → ~2–3 ns (3 loads + arithmetic). bintree: ~127ms → ~90ms;
nqueens: ~163ms → ~120ms. `sort`'s `hash--acc` walk gains proportionally.

**Entry points:** `jit/rt.rs` (add `brood_rt_pair_bases`), `eval/compile/jit_lower.rs`
`jit_lower_arm` (the `Inst::Prim1` arm), `jit_lower_arm` function entry (add the
one-shot `brood_rt_pair_bases` call + store base SSA values for later use by First/Rest arms).

---

### 3f. `reduce` — **range-fold JIT bypass**

**Root cause.** `(reduce + 0 (range n))` routes through the prelude's `fold`, which detects a
range and calls `%range-reduce` (the Rust native in `builtins/`). Inside `%range-reduce`, the
accumulator function `f` is called per element via `heap.eval_apply(f, &[acc, elem])` — the
full function-dispatch path: IC probe, `RefCell` borrow, dispatch match. Even though `+` is a
native (`prim_add`), `eval_apply` still goes through `dispatch()`. Cost: ~22 ns/element × 5M
elements = ~109ms. The JIT never sees this loop — `%range-reduce` is Rust.

**Proposed approach.** In `%range-reduce` (or a fast-path variant), detect whether `f` resolves
to a single PrimOp-eligible function at call time:
1. Resolve `f`'s native name via the interpreter's IC cache (or check `f.native_fn()`).
2. If it matches a PrimOp (`+`/`-`/`max`/`min`/etc.), run a tight Rust loop:
   ```rust
   let mut acc = init;
   for elem in range_iter {
       acc = prim_apply(op, acc, elem)?.unwrap_or_else(|| eval_apply(f, acc, elem));
   }
   ```
   where `prim_apply` is the same inline function already used in `prim2_inline_exec` —
   `(Int, Int)` case returns directly, overflow/float defers to `eval_apply`.
3. A global-epoch guard around the loop deopts to the fallback if `+` gets rebound.

This keeps `%range-reduce` Rust-native (no JIT compile of the loop), but replaces the per-step
`eval_apply` (~22 ns) with `prim_apply` (~2–3 ns) for the common `(reduce + 0 (range n))` shape.

**Expected gain.** reduce: 109ms → ~20ms (matching `loop`'s profile at ~44ms for 30M iters, or
~1.4× worse due to the `prim_apply` overhead vs pure SSA arithmetic).

**Entry points:** `builtins/` (`range_reduce` function), `eval/compile/` (`prim_apply`
export or inline copy), `core/value.rs` (`PrimOp` — may need to be accessible from builtins).

---

### 3g. `sort` — **list-walk and rebuild cost**

**Root cause.** `(sort lst)` (N=375k integers) has three phases:
1. `seq_items` — O(n) cons-spine walk to collect into `Vec<Value>`: each step calls `h.pair(id)`
   (a region-dispatch slab read). ~375K pair reads = ~7.5ms at 20 ns each.
2. `Vec::sort_by` — pure Rust timsort with no function dispatch; fast (~10ms for 375K ints).
3. `list_with_tail` — O(n) `alloc_pair` calls to rebuild the list: ~375K nursery allocations.
   Each `alloc_pair` bumps the nursery; this also triggers periodic minor GCs.
4. `hash--acc` — O(n) JIT-compiled list walk via `first`/`rest` FFI calls: ~375K car/cdr pairs.

Phase 2 is already fast. Phases 1 and 4 are directly fixed by §3e (car/cdr inline). Phase 3
(alloc_pair) is structural: rebuilding an immutable sorted list requires allocating n new pairs.

**Residual after §3e.** seq_items drops ~7ms; hash--acc drops ~7ms; the alloc cost (phase 3)
remains. Estimate: ~172ms → ~130ms after §3e; further narrowing requires either a mutable sort
(in-place pair update — unsafe, only valid for nursery pairs not aliased elsewhere) or returning
a sorted vector instead of a list (`sort-vec` variant).

**Entry points:** `builtins/` (`sort_asc`, `seq_items`, `list_with_tail`). Phase 3 is in
`core/heap.rs` (`alloc_pair`/`list_with_tail`). The `hash--acc` gain is from §3e.

---

### 3h. `fib` — **function call inlining (long horizon)**

**Root cause.** `fib(35)` makes ~18M recursive calls. With fast-link (`brood_rt_fast_frame`),
each non-tail JIT→JIT call costs ~15 ns. 18M × 15 ns = 270ms — matches the observed ~280ms.
This is the floor of the fast-link approach. The call overhead IS the benchmark.

**What would help.** True CLIF inlining: detect that the callee's body is small and pure,
emit it at the call site, eliminating the frame entirely. For `fib`, the two recursive calls
become inline additions — no frame setup, no `brood_rt_fast_frame`, no result ABI. Estimated
gain: 280ms → ~80ms (pure arithmetic loop over the call tree).

**Why it's deferred.** Requires: (a) detecting that `(fib (- m 1))` and `(fib (- m 2))` are
calls to the same function being compiled, (b) emitting the callee body inline (two levels deep
for fib), (c) handling the base case (`if (< m 2) m`) as a CLIF conditional inside the inlined
body. Cranelift supports this — it's a normal CLIF subgraph — but the compiler machinery to
detect, bound, and emit self-recursive inlines doesn't exist yet. Likely a 2–3 day change.

**Entry points:** `eval/compile/jit_lower.rs` `jit_lower_arm` — would detect `Node::Call` to the
function being compiled and recurse into `emit_body` with a depth limit.

## 4. Recommendation & priority

These are **foundational, multi-session bets with capped payoff** (Brood's boxed/immutable/
lightweight design means none will reach .NET/Node on raw numeric throughput). Brood's
actual standouts — **memory** (~14 MB base, lightest in `pfib` at ~16 MB), **concurrency**
(`http` 2nd of six, `pfib` ahead of Ruby/Python), **startup** (~28 ms) — are already strong
and are where the language's identity lives.

Priority if/when this is picked up:

1. **`matmul` — hoist the immutable vector base + inline `VectorRef`** (§3a, §6) —
   **SHIPPED 2026-06-15, local AND global** (see the "JIT matmul LICM" + "the global lever"
   devlog entries). Inlined the invariant-local read (`(nth rowa k)`) *and* the global
   (`(nth b k)`) — the global is hoisted with a back-edge `global_epoch` guard that deopts
   on a concurrent rebind, so it stays bit-identical to the VM's late binding (the earlier
   "parity-unsound" worry is solved by the guard). Isolated read ~7.8 → ~1.2 ns; `matmul`
   compute ~241 → ~171 ms, now beating both interpreters. The one residual read is the
   **per-`k` row** (varies — not hoistable), so the gap stays the suite's largest (~30×,
   noise-sensitive denominator) — bounded ultimately by the boxed 24-byte `Value`.
2. **zero-copy message passing** (§3c, §6) — share immutable structures by handle instead of
   deep-copying across processes; attacks the `strings` ~180 MB outlier and `spawn`/`pfib`
   message cost. Also opens **lazy combinators** as the eager-list fix.
3. **`bintree` allocation** — GC/nursery tuning; diffuse.
4. **`wordcount`** — **SHIPPED 2026-06-19** (`%map-int-add` + JIT GC safepoint, 810→422ms,
   gap ~31×→~13×). Residual: transient-map `into` for the final 13× → ~4× if wanted.
5. **inline `first`/`rest`** (§3e) — `brood_rt_pair_bases` + CLIF inline loads for LOCAL
   pairs; bintree/nqueens/sort-walk. Medium effort, 20–30% on affected benchmarks.
6. **range-fold JIT bypass** (§3f) — `%range-reduce` PrimOp fast-path; reduce 109ms → ~20ms.
   Medium effort; requires `prim_apply` accessible from builtins.
7. **fib call inlining** (§3h) — long horizon; requires self-recursive CLIF inlining.

NaN-boxing / `Value`-width is **not** on this list (§2).

## 5. How to pick it up

1. Re-baseline: `cd brood-benchmarks && python3 bench/harness.py --runs 5 --startup-runs 15`
   (needs `make install` of a `--features jit` binary first) — confirm the numbers above
   haven't drifted.
2. Profile the target with a `--features jit,perf-stats` debug build:
   `BROOD_PERF_STATS=1 BENCH_N=<small> ./target/debug/brood bench/brood/<t>.blsp` — read
   `jit_native` / `jit_deopt` / `prim2_fallback` / `alloc`. Dump lowered arms with
   `BROOD_JIT_DUMP_IR=1`.
3. For matmul specifically: the microbenchmark in §3a (a tight `nth` loop) isolates the
   `VectorRef` cost cleanly — use it to validate any helper/storage change before touching
   the benchmark.
4. **Guardrails (this area bites — three regressions this round came from JIT/compile
   changes):** run the full in-language suite under `--features jit` (the
   `format`-tiering-corruption canary lives there), keep per-benchmark JIT==tree-walker
   checksum parity, and run `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` for any
   allocation/storage change. The `value.rs` accessor discipline (ADR-002) is what keeps a
   storage change containable.

## 6. Immutability shortcuts (why these gaps are more tractable than they look)

Brood is immutable (ADR-026), and that's not just a semantics choice — it **removes the
analysis that makes these optimizations hard in a mutable language**, so several "capped /
foundational" rows above have a sound, contained path:

- **Loop-invariant hoisting is sound with no alias analysis.** Hoisting a load out of a
  loop normally requires proving no write through any aliasing pointer can invalidate it —
  the expensive part of an optimizing compiler. In Brood *no such write can exist*, so the
  JIT can hoist an immutable vector's `(ptr, len)` out of `dot` and inline the element reads
  with zero alias analysis. This is the `matmul` lever (§3a, priority 1) and generalizes to
  every indexed-array loop over an immutable vector.
- **Zero-copy sharing across processes.** An immutable value can be shared by `Arc` handle
  instead of deep-copied (already done for big strings via `StrShared`); the copy is *only*
  needed because LOCAL heaps are isolated, not because the value could change. Extending it
  to whole immutable structures cuts message-copy cost and peak RSS (§3c, priority 2).
- **Hash-consing + O(1) equality.** Immutable values can be interned/deduplicated, making
  `=` on shared structure a pointer compare instead of a structural walk.
- **CSE / memoization / free reordering.** Referential transparency lets the compiler
  common-subexpression-eliminate repeated pure reads (`(nth v i)` with immutable `v`/`i`),
  memoize pure functions, and reorder without a happens-before worry.
- **No write barriers.** The frozen RUNTIME region needs none (already banked); more
  generally, immutability is why the tracing collector and cross-process sharing stay simple.

The throughline: where I earlier called a row "representation-bound" or "foundational with
capped payoff," immutability often supplies a *contained* path (hoist-and-inline, share-by-
handle) a mutable language would need a full optimizing pass to justify. The hard residual
is the boxed 24-byte `Value` itself (§2) — which immutability does *not* fix.

## 2e. `jit_run_fast_link` outlined — `bintree` −4.0% (2026-08-28)

The first change taken off §2d's ranking. `perf` put `jit_run_fast_link` at **24% of `bintree`**,
as much as all of that row's native compute, and instruction-level annotation showed why: the cost
was **spread thin across the prologue and epilogue** — register saves (`pushq %r15`/`%rbx`), spills
at `-0x158`/`-0x160(%rbp)` — with no single hot operation. That is the signature of a large
function on a hot path, not of expensive work.

It was **201 lines**. Outcome 0 — return the value — is **6** of them; the deopt, preempt,
tail-chain and error arms are the other ~120, and they need several `SmallVec`s and many live
values. So the compiler sized a ~350-byte frame and saved the registers for arms that almost never
run, on **every** call.

Outcome 0 is now handled inline and the rest moved to a `#[cold] #[inline(never)]`
`jit_fast_link_cold_outcome`. **Pure code layout — the cold code is the same code, same order,
same comments.**

| measurement | base | new | delta | floor |
|---|---|---|---|---|
| `make ab`, best-of-11 | 157 ms | 150 ms | −4.5% | 0.0% |
| interleaved, min-of-13, n=200 | 155 ms | 150 ms | **−3.2%** | 1.3% |
| interleaved, min-of-7, **n=2000** | 1021 ms | 980 ms | **−4.0%** | **0.2%** |

Measured at two sizes deliberately (§2d's rule): the win is *larger* at n=2000, because boot is 29%
of the n=200 run and dilutes it. 20× the floor at the long size.

`fib` −0.9%, `nqueens` −0.7%, `ackermann` −0.5%, `collatz` −1.4%, `pipeline`/`loop`/`sort` flat —
no row regressed.

**Verified beyond the suite**, because this function owns the deopt/preempt/tail paths where a
mistake is a *silently repeated side effect* rather than a crash: `--test jit` 40/40, the
deopt/tier/fast_link/inline/preempt selection 33/33, `tests/jit_effect_once_test.blsp` 6/6 (the
KI-18 duplicated-effect guard), and `--test jit` again under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` 40/40 — the frame this function builds is GC-visible.

**What is left on this row**, from the same profile: `brood_rt_fast_frame` 10.7% (the next call-path
item) and ~21% allocation, of which the 11.4% `__memmove` is **not** the call protocol —
frame-pointer chains put it at `Vec<VecStore>::push` → `RawVecInner::finish_grow`, i.e. the value
slab reallocating as the row allocates 8.19M node vectors. That is the allocation frontier and a
separate lever: a segmented slab would never move existing elements. Handles are indices, so
nothing in the language observes the difference.

## 2f. The tenure reservation, re-measured — 1.3%, and its justifying numbers are stale

§2d's `bintree` profile put **11.4% in `__memmove`**, and frame-pointer chains put that at
`brood_rt_make_vector2` → `Vec<VecStore>::push` → `RawVecInner::finish_grow` — the value slab
reallocating. The obvious suspect was the tenure path in `minor_collect`, which installs
`Slabs::default()` (zero capacity) for the new nursery while the flip path beside it uses
`Slabs::with_capacity_like`. That asymmetry is **deliberate and documented**: reserving there
"holds a peak-sized allocation the next cycle may never touch", and the comment cites `sort`
peaking at 191 MB against .NET's 30 MB as the reason. `BROOD_GC_TENURE_RESERVE=1` exists to A/B it.

**Measured, both arms, this tree:**

| row | reserve OFF (default) | reserve ON | |
|---|---|---|---|
| `bintree` n=200 | 149 ms · 115.8 MB | 149 ms · **107.9 MB** | time equal, memory *better* with reserve |
| `bintree` n=2000 | 987 ms | **974 ms** | **−1.3%** |
| `sort` | 134 ms · 252.4 MB | 134 ms · 252.1 MB | indistinguishable |

Three things follow, and the first two are corrections to the comment rather than to the code:

1. **`sort` is 252 MB on this tree, not 191 MB**, and it measures the same with the reservation
   on or off. Whatever made the reservation expensive for `sort` no longer does — so the memory
   argument that decided this trade-off no longer reproduces as written. **Do not re-cite the
   191 MB figure without re-measuring it.**
2. **The prize is ~1.3%, not 11.4%.** The tenure ladder is a small part of that memmove; most of
   it is elsewhere (the `major_collect` path still uses `Slabs::default()`, and first-time growth
   is genuine). My hypothesis was mostly wrong and the measurement is the only reason it is not
   recorded as a win.
3. **The default was NOT flipped.** On these two rows `ON` is equal-or-better on both time and
   memory, which looks like a free win — but that is two rows, the original decision rested on
   evidence this measurement cannot reconstruct, and a GC memory policy is the wrong thing to
   change on a narrow sample. Worth revisiting with the full row set and a peak-RSS sweep;
   `BROOD_GC_TENURE_RESERVE=1` makes that a one-flag experiment.

The remaining call-path item from §2d is `brood_rt_fast_frame` (10.7%) — same family as §2e's
win, and a better next target than this at 1.3%.

## 2g. Re-profiled after §2e, and the call convention now has a price tag on one instruction

Re-profiled deliberately rather than continuing against §2d's numbers — §2e changed the very
function that was 24%, so the ranking had moved.

**`bintree` n=2000, after the outlining:**

| % | symbol | vs §2d |
|---|---|---|
| **22.17** | `jit_run_fast_link` | 24.03 → 22.17 |
| 12.56 + 12.47 | `brood_jit_arm_43`/`_44` (the real work) | ~unchanged at 25% |
| 10.16 | `__memmove` | 11.38 → 10.16 |
| **9.12** | `brood_rt_fast_frame` | 10.67 → 9.12 |
| 5.02 / 4.75 | `make_vector2` / `push_n` | |
| 2.73 | `env_get` | boot residue at this size |

The ~2-point drop in `jit_run_fast_link` matches §2e's −4% wall-clock. It is **still the top item**,
so the next question is what remains inside it.

### One instruction is 23.5% of the function

Instruction-level annotation is unambiguous this time — where §2d found cost *spread thin* across
the prologue (which is what outlining fixed), what is left is concentrated:

```
23.50 :  movups (%rax,%rcx,8), %xmm0
 3.28 :  movq   0x170(%r15), %rax        # the roots pointer
```

A 16-byte load from the roots stack (scale 8 with the index pre-multiplied by 3 = 24-byte
`Value` stride). 23.5% of a function that is 22.17% of the row is **~5.2% of `bintree` on a single
load**, and it sits shortly after the `callq *%rdx` that runs the callee's native code.

**The mechanism, stated with the confidence the evidence supports:** the callee writes its result
into `roots[base]` and the caller loads it straight back out — a store-to-load round trip through
memory across the native call boundary. A load that close behind a call, at that share, is a
store-forwarding/latency stall rather than expensive work. (Perf skid means the exact source line
is not certain; the *instruction* and its cost are.)

**So the X-register call convention now has a measured price tag and a first concrete step**:
return the callee's result **in a register** instead of through the roots slot. That is worth ~5%
of `bintree` on its own, it is the narrowest possible slice of the convention change, and it is
testable in isolation.

> **Done — see §2h (2026-08-29).** It landed as an out-pointer rather than a register (a
> `Value` is 24 bytes and cannot come back in the two SysV return registers), and it was worth
> **more** than the ~5% estimated here, because the load was only one of *three* copies on the
> path. The stall mechanism guessed below — store forwarding — was measured and is **wrong**.

**Not attempted here, deliberately.** That is an ABI change spanning `jit_lower.rs`'s Cranelift
lowering, the `brood_rt_*` callback contract and the VM's expectations of a frame — the
multi-session redesign FRONTIER describes. Starting it at the end of a long session risks leaving
a half-migrated ABI, which is the one state worse than not starting. The finding is the valuable
part: it turns "redesign the call convention" into "make the return value come back in a register,
and expect ~5% on `bintree`."

**Also still open on this row:** `brood_rt_fast_frame` 9.12% (whose inlined work is
`jit_dispatch_fast_frame`'s per-call cap and `jit_native_headroom_ok` checks — worth reading before
assuming they are free), and ~19% allocation of which §2f showed the tenure reservation is only
1.3%.

## 2h. The result no longer returns through memory — `bintree` −7.5%, `collatz` −5.8% (2026-08-29)

§2g's first step, taken. The arm ABI gained a third parameter — `out: *mut Value` — and the
Done exits write the result **through it** instead of into `roots[base]`.

**What §2g got right and what it got wrong.** Right: that `movups` really is the cost. Under
`cycles:pp` (precise events, so *not* skid from the `callq` 115 bytes earlier) it was **16.4%
of `jit_run_fast_link`**. Wrong: the mechanism. The obvious explanation — a 16-byte load
straddling `store_int`'s 1-byte tag + 8-byte payload cannot store-forward — predicts exactly
this symptom, and it was tested: widening the callee's Done store to a single 16-byte vector
store while still going through `roots[base]` left the instruction at **16.4%** and bought
1.3% of the row against a 1.0% floor. (Note `iconcat` + `store.i128` is *not* that test —
Cranelift's x64 backend keeps an `i128` in a GPR pair, so it legalizes back into two 8-byte
`mov`s. That version measured no change, correctly.) The cost is the memory round trip, not
the width mismatch.

**Why the win is bigger than §2g's ~5% estimate.** The load was only the visible third of it.
The value was copied **three times** between the callee producing it and the JIT'd caller
consuming it:

1. the callee stored it into `roots[base]`;
2. `jit_run_fast_link` loaded it back (the `movups`) and returned `FastLinkOutcome::Done(Value)`
   — a 32-byte enum, so returned through a hidden pointer (`sret`), i.e. stored again;
3. `brood_rt_fast_frame` matched on it and did `*out = v` into the caller's slot — stored again.

Handing `out` down to the callee collapses all three: it is written once, into the slot the
consumer already owns. `FastLinkOutcome::Done` is now payload-free, which is what removes the
`sret`.

**Measured** (`ab-bench --floor`, base `7acd6a09`):

| row | delta | floor | |
|---|---|---|---|
| `bintree` | **−6.5%** | 1.3% | improved |
| `collatz` | **−5.8%** | 2.2% | improved |
| the other 28 rows | — | — | noise |

Solo, interleaved base/new/base/new: `bintree` **−8.5%** at n=200 (floor 0.0%) and **−7.5%**
at n=2000 (floor 0.7%); flat at n=20, where the row is boot. Tier ceiling 1 (`ab-bench --tier 1`,
the VM's own call path, which this also touches): six rows, all noise. `latency` read +5.2% in
the pinned sweep and is **not** a regression — unpinned interleaved runs give 2.56 s on both
binaries with p50 20 vs 20 µs, p99 90 vs 89 µs and identical sustained rps; it is a fixed-schedule
open-loop row, so its wall time under core pinning is queueing, not throughput.

**The profile after**, same command as §2g (`cycles:pp`, n=800): `jit_run_fast_link` 14.9% →
**13.1%**, and inside it **no instruction above 6.4%** — the 16.4% load is gone rather than
cheaper. The native arms are now the top entries.

**The trap this change is built around.** Every caller reaches an arm through
`mem::transmute` of a raw code pointer, so **an ABI change here is not type-checked**: a caller
left at the old arity reads `out` from a register nobody set and stores a `Value` through it.
Two mitigations, both load-bearing:

- `crate::jit::JitArmFn` is now a **named type** used at every transmute, so the arity is a
  single-site fact rather than nine independent restatements.
- `out_ptr` lives on `emit::Frame`, not as a parameter to the exit helpers — because there are
  **two** Done exits (`exit_done` in `jit_lower.rs` and the `t == len` arm of
  `control::emit_jump`), and the first migration missed the second. Every `if`/loop arm then
  returned `nil` while the straight-line arms were fine: five unit tests caught it, but a
  parameter that must be threaded to a site you have not found is exactly the shape that gets
  missed. On `Frame` the type system asks the question.

**GC:** `out` is not a root, so nothing may allocate between the callee's store and the
consumer taking the value. The outcome-0 path does none (`set_ic_bases` and the two
`truncate_*` are `Cell` writes and `Vec::truncate`), and the cold outcomes — which *do* re-enter
the evaluator — write `out` only after all their allocation. This is the discipline the
`brood_rt_{cons,car,cdr}` out-pointer ABI has always run under. Verified: full suite 1231/1231
under both engines (`make test-both`), `make gcstress`, `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`
on the effect-once torture cases, `BROOD_JIT_VERIFY=1`, all 21 `tests/jit_*_test.blsp`, and the
fuzz differential across **all 11 generators × 4 engine configs** (tree-walker / VM-no-JIT /
VM+JIT / GC-stress) — 0 divergences, 0 crashes. Lowering is unchanged (86 vs 85 `[jit-ir]` arms,
46 vs 46 bails).

**Still open on this row**, and now the ranking to work from: `__memmove` **10.4%** (the frame
and staging copies — the next-largest single item and the one this change did *not* touch),
`brood_rt_fast_frame` **8.2%**, and ~19% allocation.
