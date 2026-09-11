//! Compiling a closure's arms and caching the result: `compile_arm` lowers one arm
//! (with `compile_closure` over all of them), the body cache is keyed by `cache_key`
//! and shared across a runtime's processes (ADR-175/215), `precompile` warms it, and
//! the `hof_*` fast paths let a native higher-order primitive step a compiled callback
//! without the full call ceremony.

use super::*;

// ============ linear map-accumulator → Table rewrite (docs/linear-map-accumulator.md) ============
//
// A self-tail-recursive fold that threads an immutable-map accumulator one update
// at a time pays an O(depth) path-copy per update (~2.25M node allocations for the
// `wordcount` benchmark). When the accumulator is provably *linear* — never
// aliased, never escapes except as the function's return — we represent it
// internally as a private `Table` (already GC-safe, mutated in place) and snapshot
// it back to an immutable map at the return. Sound because (a) the entry copies the
// input map into a fresh table the function alone owns, so callers' maps are never
// mutated, and (b) the intra-procedural reachability check below proves the slot is
// only ever a whitelisted map op's first arg, the self-call threading arg, or the
// return — exactly the "no alias analysis needed; a value is only reachable through
// references the code creates" property `local_escapes` relies on. The observable
// result is an ordinary immutable map (ADR-026 holds: the only mutable thing is a
// `Table`, never surfaced). On by default; opt out with `BROOD_LINMAP=0`.

/// Compile one arm to a [`CompiledArm`], or `None` (defer this arm to the
/// tree-walker) if its body or any real `&optional` default uses a form outside the
/// VM vocabulary. Binds frame slots in layout order — required params, then each
/// optional (its default compiled *before* the optional's own slot is bound, so a
/// default sees the required params and earlier optionals but never itself), then
/// the `&` rest param — then compiles the body. The default nodes ride along in
/// `optional_defaults` for `push_frame` to evaluate on a missing arg.
pub(crate) fn compile_arm(
    heap: &Heap,
    required: &[Symbol],
    optionals: &[(Symbol, Value)],
    rest: Option<Symbol>,
    body: &[Value],
    enclosing: Vec<Symbol>,
    self_name: Option<Symbol>,
    defn_name: Option<Symbol>,
    trace_name: Option<Symbol>,
) -> Option<CompiledArm> {
    // Grab the first body form before `body` is shadowed by the compiled Node —
    // its recorded reader position carries the defining source file (`src_file`).
    let body_first_form = body.first().copied();
    let nrequired = required.len();
    let noptional = optionals.len();
    let mut scope = Scope::with_params_enclosing(&[], enclosing);
    // The self-call optimization applies only to a plain fixed-arity closure (no
    // `&optional`/`&` rest), where a tail call passing exactly `nrequired` args
    // re-runs this arm verbatim. With optionals/rest the frame-fill differs per
    // call, so such calls fall back to the regular env-resolved path (correct,
    // just unoptimized).
    if let Some(name) = self_name {
        if noptional == 0 && rest.is_none() {
            scope.self_call = Some((name, nrequired));
        }
    }
    // `defn` tail self-calls get the same inline frame-reset via SelfCall. The
    // in-flight call holds an Arc to its own compiled arm, so it correctly runs
    // the current compiled version even if the global is redefined mid-call.
    if let Some(name) = defn_name {
        if noptional == 0 && rest.is_none() {
            scope.self_call = Some((name, nrequired));
        }
    }
    for &p in required {
        scope.bind(p);
    }
    let mut optional_defaults: Vec<Option<Node>> = Vec::with_capacity(noptional);
    for (name, default) in optionals {
        // A nil default needs no eval (push_frame just leaves the slot nil); a real
        // default compiles in the current scope (required + earlier optionals bound).
        let node = match default.unpack() {
            ValueRef::Nil => None,
            _ => Some(compile_node(heap, *default, &mut scope, false)?),
        };
        optional_defaults.push(node);
        scope.bind(*name);
    }
    if let Some(r) = rest {
        scope.bind(r);
    }
    // #3 lexical addressing: bind each captured enclosing lexical to a **capture slot**
    // (right after params/optionals/rest, so `capture_base = nrequired + noptional +
    // rest_count`), so a body reference resolves to a fast `Node::Local(slot)` instead of
    // an `env_get` symbol-scan through the captured env. `push_frame` fills these slots at
    // call setup. A name already bound (a param shadows the enclosing lexical) is skipped —
    // the param wins, and `push_frame`'s by-name fill stays correct for the misaligned rest.
    let mut capture_names: Vec<Symbol> = Vec::new();
    for &name in &scope.enclosing.clone() {
        if scope.lookup(name).is_none() {
            scope.bind(name);
            capture_names.push(name);
        }
    }
    let capture_names = capture_names.into_boxed_slice();
    let mut body = compile_body(heap, body, &mut scope, true)?;
    // Escape-analysis scalar replacement (lever 2): eliminate non-escaping `(let (p […]) …)`
    // vector allocations, binding their elements to fresh slots `[scope.max ..]` and rewriting
    // `(nth p K)` to direct reads. Bumps `scope.max` for the element slots; makes the arm
    // simpler (fewer allocs, no `nth`), so it JITs better. No-op for arms without the pattern.
    ea_scalar_replace(&mut body, &mut scope.max);
    let optional_defaults = optional_defaults.into_boxed_slice();
    let has_runtime_handles =
        node_has_rt_handles(&body) || optional_defaults.iter().flatten().any(node_has_rt_handles);
    // Stage 1: try to compile the body to flat bytecode (a call-free, handle-free
    // subset for now — `compile_chunk` returns `None` otherwise, and the arm runs
    // via `exec_node` exactly as before).
    let chunk = compile_chunk(&body);
    // Line coverage (ADR-148 tier 2): register this arm's instrumented lines as they
    // are compiled. This is the report's DENOMINATOR — see `coverage.rs` for why it
    // cannot be inferred from the source text instead.
    if crate::diagnostics::coverage::enabled() {
        if let (Some(chunk), Some(file)) = (
            chunk.as_ref(),
            body_first_form
                .and_then(|f| heap.form_pos(f))
                .and_then(|(_, file)| file),
        ) {
            crate::diagnostics::coverage::note_instrumented(
                &file,
                chunk.code.iter().filter_map(|inst| match inst {
                    Inst::RecordLine(line) => Some(*line),
                    _ => None,
                }),
            );
            // The branch denominator: each distinct `(line, col)` decision point (both
            // edges of one `if` carry the same site, so the set dedups them).
            crate::diagnostics::coverage::note_branches(
                &file,
                chunk.code.iter().filter_map(|inst| match inst {
                    Inst::RecordBranch(line, col, _) => Some((*line, *col)),
                    _ => None,
                }),
            );
        }
    }
    // Reserve a few extra frame slots (above the compiler's `scope.max`) when the arm
    // has ≥2 non-tail calls, so a JIT-lowered version can spill call-result handles
    // that must survive a later call's safepoint (two-call recursion: `fib`, bintree
    // `check`). The VM never references these slots; `push_frame` nil-inits them like
    // any other. Computed identically here (to size the frame) and in `jit_lower_arm`
    // (to place spills) via `jit_spill_reserve`.
    let spill_reserve = chunk.as_ref().map_or(0, |c| jit_spill_reserve(&c.code));
    // Deopt-resume checkpoint slots (see `CompiledArm::ckpt_slot`): one packed
    // journal slot + room for the deepest post-call operand stack. Reserved above
    // the spill slots; zero cost for call-free arms (`ckpt_depth` is None).
    // `self_arity` is this arm's own fixed arity, and `None` when argc doesn't select an
    // arm 1:1 (optionals / a rest param) — see the pure-self exemption in `jit_ckpt_depth`,
    // which must not treat a call to a *sibling* arm of the same multi-arity `defn` as a
    // call back into this provably effect-free one.
    let self_arity = (noptional == 0 && rest.is_none()).then_some(nrequired);
    let ckpt_depth = chunk
        .as_ref()
        .and_then(|c| jit_ckpt_depth(&c.code, defn_name, self_arity));
    let (ckpt_slot, ckpt_reserve) = match ckpt_depth {
        Some(d) => ((scope.max + spill_reserve) as u32, 1 + d),
        None => (u32::MAX, 0),
    };
    // Deopt-feedback watch (see the field doc): every non-loop arm. The one exclusion
    // that has a rationale is `SelfCall` loops — their deopt follows productive native
    // iterations (an overflow at the end of a long int loop), so counting activations
    // would mis-bail them. The predicate used to ALSO require ≥1 non-tail call (the shape
    // nbody's `advance-body` had), which left a **call-free** arm with no thrash
    // protection at all: mandelbrot's `->float` — one multiply, no calls — deopted
    // **275,007 times in one run** (native entry + guard + deopt + full VM re-run per
    // call, watch=false), the exact pathology this mechanism exists to stop, invisible
    // to every gate because the answers are right. A healthy arm pays one relaxed load
    // per native completion; an arm that deopts 16 times consecutively was not being
    // served by its native code, calls or no calls.
    let deopt_watch = chunk
        .as_ref()
        .is_some_and(|c| !c.code.iter().any(|i| matches!(i, Inst::SelfCall { .. })));
    let nslots_total = scope.max + spill_reserve + ckpt_reserve;
    let uid = next_arm_uid();
    let site_pos = std::mem::take(&mut scope.site_pos).into_boxed_slice();
    let src_file = body_first_form
        .and_then(|f| heap.form_pos(f))
        .and_then(|(_, file)| file);
    // Recursive self-inlining (Phase B, §6b — two-stage tiering, devlog 2026-06-17):
    // PROBE depth-1 inlining of a top-level no-capture recursive `defn`'s body WITHOUT
    // mutating the original. The VM keeps the original small `body`/`chunk`/`nslots`;
    // the inlined body is re-derived fresh in `jit_lower_arm` and compiled as a deferred
    // upgrade. Here we only record whether the arm qualifies + the inlined frame
    // high-water mark (`inline_nslots`), by running the inliner on a CLONE (then
    // discarding it). Gated to a clean fixed-arity layout (no `&optional`/`&` rest —
    // `M = scope.max` must be the whole frame so shifted blocks don't collide), with a
    // `defn_name` (top-level recursive, set only when the closure doesn't capture). The
    // probe enforces the rest of the gate (no `SelfCall`/`MakeClosure`, body-size bound,
    // ≥1 qualifying call). Deterministic: same arm → same shifted IR.
    //
    // Runs HERE, after `nslots_total`, because the leaf splice needs the caller's full
    // small frame size as its base — see `leaf_inline_probe`.
    #[cfg(feature = "jit")]
    let (inline_name, inline_stride, inline_nslots, leaf): (
        Option<Symbol>,
        usize,
        usize,
        Option<Box<ir::LeafInline>>,
    ) = {
        let m = scope.max;
        match defn_name {
            Some(name) if noptional == 0 && rest.is_none() => {
                match self_inline_probe(&body, name, nrequired, m) {
                    Some(inline_max) => (Some(name), m, inline_max, None),
                    // Mutually exclusive with self-inlining: the leaf derivation is
                    // stored (not re-derived), stamped with the current epoch, and
                    // rides the same deferred-upgrade channel (`inline_name` set so
                    // the swap invalidates this caller's fast links; `inline_stride`
                    // unused — the lowerer branches on `leaf` first).
                    None => {
                        match leaf_inline_probe(
                            heap,
                            &body,
                            m,
                            nslots_total,
                            Some(name),
                            self_arity,
                        ) {
                            Some(d) => {
                                // Apply the small-frame floor HERE, before the resume arm
                                // is built, so `resume.nslots` is the value the frame is
                                // actually sized to (`arm.inline_nslots`, floored below).
                                // The lowering reads the frame size off the resume arm and
                                // stages a tail call above `frame_size_for_new_entry()`; if the two
                                // disagreed, the staged area would be written at one offset
                                // and read at another.
                                // **Strictly** larger than the small layout, always. The
                                // deopt-resume path identifies which of an arm's two
                                // layouts a live frame was built to *by its size* (see
                                // `jit_frame_layout`), because the `inline_installed` flag
                                // is flipped by `jit_tier` between the sizing and the
                                // deopt. That test is only sound if the sizes differ, and
                                // `d.nslots.max(nslots_total)` alone does not guarantee it:
                                // a derivation that splices a callee needing no slot beyond
                                // the caller's own (`(dec n)`) comes out exactly equal.
                                //
                                // When they were equal, a leaf-spliced frame was read as
                                // the small layout, so the small body's `ckpt_slot` — a
                                // live local in the spliced layout — was decoded as a deopt
                                // journal: `(defn sum-down (n acc) (if (<= n 0) acc
                                // (sum-down (dec n) (+ acc n))))` resumed at ip `n >> 16`
                                // with operand depth `n & 0xFFFF`, returning 6251217600 for
                                // `(sum-down 200000 0)` instead of 20000100000, and failing
                                // as `type error: -: expected number, got nil` at 400000.
                                // Splicing removes the residual `Call`, which makes the
                                // derivation `pure_self` and therefore *unjournalled*, so
                                // the old "leaf and journalled" test could never separate
                                // this pair. One reserved slot restores the invariant the
                                // rest of the machinery already documents and asserts.
                                let leaf_nslots = d.nslots.max(nslots_total + 1);
                                // The resume arm (see `ir::LeafInline::resume`): the same
                                // function over the spliced body, so a deopt out of the
                                // inlined native can be resumed in the ip space it
                                // journalled against. It shares this arm's `uid` (hence
                                // its inline-cache block) and its identity/diagnostic
                                // fields; only body, chunk, frame and checkpoint differ.
                                let resume = CompiledArm {
                                    nrequired,
                                    noptional: 0,
                                    optional_defaults: Box::new([]),
                                    rest_slot: None,
                                    nslots: leaf_nslots,
                                    nsites: scope.sites,
                                    ngsites: scope.gsites,
                                    uid,
                                    site_pos: site_pos.clone(),
                                    body: d.body,
                                    chunk: Some(d.chunk),
                                    has_runtime_handles,
                                    jit_code: AtomicPtr::new(std::ptr::null_mut()),
                                    jit_calls: AtomicU32::new(0),
                                    deopt_watch: false,
                                    jit_deopts: AtomicU32::new(0),
                                    float_globals: std::sync::OnceLock::new(),
                                    self_global_ok: std::sync::atomic::AtomicBool::new(false),
                                    ckpt_slot: d.ckpt_slot,
                                    compile_epoch: AtomicU64::new(0),
                                    // Never published to the cross-process cache: this arm
                                    // is reachable only through its caller's derivation.
                                    share_key: None,
                                    shared_published: std::sync::atomic::AtomicBool::new(false),
                                    fn_name: trace_name,
                                    src_file: src_file.clone(),
                                    capture_names: capture_names.clone(),
                                    dbg_name: defn_name,
                                    // The resume arm is the spliced body; it must not
                                    // itself splice again.
                                    #[cfg(feature = "jit")]
                                    inline_name: None,
                                    #[cfg(feature = "jit")]
                                    inline_stride: 0,
                                    #[cfg(feature = "jit")]
                                    inline_nslots: leaf_nslots,
                                    #[cfg(feature = "jit")]
                                    inline_code: AtomicPtr::new(std::ptr::null_mut()),
                                    #[cfg(feature = "jit")]
                                    inline_queued: std::sync::atomic::AtomicBool::new(false),
                                    #[cfg(feature = "jit")]
                                    inline_installed: std::sync::atomic::AtomicBool::new(false),
                                    #[cfg(feature = "jit")]
                                    xcall_wanted: std::sync::OnceLock::new(),
                                    #[cfg(feature = "jit")]
                                    leaf: None,
                                };
                                // Load-bearing for the deopt-resume swap in `vm_run_bc`,
                                // which replaces the frame's arm with `resume` mid-run:
                                // the frame's `live_arm_push` registration and its IC-base
                                // window were both established from the ORIGINAL arm, so
                                // the swap is only transparent while (a) the arm needs no
                                // RUNTIME-handle registration and (b) the two share a
                                // `uid`, hence one `vm_arm_block`. Both are guaranteed
                                // above — `leaf_inline_probe` rejects a body with RUNTIME
                                // handles, and `uid` is copied — so assert rather than
                                // leave them as reasoning a later edit could break.
                                debug_assert!(
                                    !has_runtime_handles,
                                    "a leaf derivation must not carry RUNTIME handles: the \
                                     deopt-resume arm swap bypasses live_arm registration"
                                );
                                debug_assert_eq!(
                                    resume.uid, uid,
                                    "the resume arm must share the caller's uid so both \
                                     index the same IC block"
                                );
                                (
                                    Some(name),
                                    0,
                                    leaf_nslots,
                                    Some(Box::new(ir::LeafInline {
                                        resume: Arc::new(resume),
                                        epoch: heap.global_epoch(),
                                    })),
                                )
                            }
                            None => (None, 0, 0, None),
                        }
                    }
                }
            }
            _ => (None, 0, 0, None),
        }
    };
    Some(CompiledArm {
        nrequired,
        noptional,
        optional_defaults,
        rest_slot: rest.map(|_| nrequired + noptional),
        nslots: nslots_total,
        nsites: scope.sites,
        ngsites: scope.gsites,
        uid,
        site_pos,
        body,
        chunk,
        has_runtime_handles,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        deopt_watch,
        jit_deopts: AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot,
        compile_epoch: AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: trace_name,
        // The file the body was read from — trace entries name it as the call
        // site's file (a fn's calls are in its own source). Cold: once per arm
        // compile.
        src_file,
        capture_names,
        #[cfg(feature = "jit")]
        inline_name,
        dbg_name: defn_name,
        #[cfg(feature = "jit")]
        inline_stride,
        // Floored at the SMALL frame size: the VM/small-native frame is already
        // `nslots_total` (locals + spill + ckpt reserves), and the per-engine sizing
        // hook grows a live frame to `inline_nslots` on a post-swap entry — a smaller
        // value would make that "grow" an underflowing shrink (hit by the leaf
        // inliner, whose spliced layout can be smaller than the small layout's
        // reserves). An UNJOURNALLED leaf splice overlaps the small spill/ckpt area by
        // design — each engine owns its layout exclusively per activation. A journalled
        // one (ADR-210) deliberately does not: it splices above `nslots_total` so the two
        // layouts' journals cannot alias while they take turns running one frame.
        // The leaf path has already applied this floor (its resume arm must agree with
        // the frame size), so `max` is idempotent there.
        #[cfg(feature = "jit")]
        // Floored to the frame size even with NO derivation: the xcall re-lowering
        // (same body, same frame) parks its compiled pointer in `inline_code`, and a
        // racing `frame_size_for_code` that matches it must answer `nslots`, not 0.
        inline_nslots: inline_nslots.max(nslots_total),
        #[cfg(feature = "jit")]
        inline_code: AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        xcall_wanted: std::sync::OnceLock::new(),
        #[cfg(feature = "jit")]
        leaf,
    })
}

/// Compile a closure's body to a [`CompiledArm`], or `None` if it isn't
/// VM-eligible (multi-arm with no exact arity, every arm `&optional`/`&` rest, or
/// every arm body uses a non-core form). Single-arm, exact-arity arms compile;
/// **local-capturing closures are eligible** (Stage 2c) — a free var resolves by
/// name through the closure's captured env (`Node::Global` → `env_get(genv, …)`),
/// which `vm_apply` sets to the closure's own env, so the body compiles the same
/// way whether the capture is global or local.
pub(crate) fn compile_closure(heap: &Heap, id: ClosureId) -> Option<CompiledClosure> {
    crate::perf_bump!(n_compile);
    // TEMP diagnostic: which closures compile, and how often.
    if std::env::var_os("BROOD_TRACE_COMPILE").is_some() {
        let c = heap.closure(id);
        let body_region = c
            .arms
            .first()
            .and_then(|a| a.body.first())
            .map(|b| format!("{:?}", b.unpack()))
            .unwrap_or_default();
        let key = match cache_key(heap, id) {
            Some(VmCacheKey::Runtime(x)) => format!("Runtime({x})"),
            Some(VmCacheKey::Body(x)) => format!("Body({x})"),
            None => "none".to_string(),
        };
        eprintln!(
            "[compile] id_region={} key={} body={}",
            id.region(),
            key,
            body_region
        );
    }
    crate::perf_time!(ns_compile, { compile_closure_timed(heap, id) })
}

pub(crate) fn compile_closure_timed(heap: &Heap, id: ClosureId) -> Option<CompiledClosure> {
    let cl = heap.closure(id);
    // The lexical names this closure inherits from outer closures (Stage 2c) —
    // empty for a global-capturing (top-level) closure. A nested `(fn …)` in the
    // body needs these to snapshot the enclosing environment it captures.
    let enclosing: Vec<Symbol> = match cl.env {
        Some(e) if !heap.is_global(e) => heap.env_chain_names(e),
        _ => Vec::new(),
    };
    // Direct `letrec` self-recursion (the self-call optimization): a closure whose
    // captured frame binds a name to *itself* (the `env_define` the `MakeClosure`
    // self-name path installs) is a local recursive helper — `defseq`'s `--loop`,
    // a hand-written named loop. A tail call to that name can re-invoke this very
    // arm without resolving the callee through the env or any dispatch (the binding
    // is an immutable letrec slot — no late-binding/epoch concern, unlike a global
    // `defn`, which is *not* self-bound in a captured frame and so never matches
    // here). `compile_arm` turns such calls into [`Node::SelfCall`].
    let self_name: Option<Symbol> = match cl.env {
        Some(e) if !heap.is_global(e) => heap.env_frame_self_name(e, id),
        _ => None,
    };
    // `defn` tail self-calls use the same `Inst::SelfCall` inline frame-reset path as
    // letrec. The in-flight call's Arc owns the compiled arm, so it runs the current
    // compiled version even if the global is redefined; new callers see the new version.
    let defn_name: Option<Symbol> = if cl.env.is_none() { cl.name } else { None };
    // Any closure's name (top-level or not), for error stack traces (`fn_name`).
    let trace_name: Option<Symbol> = cl.name;
    // Snapshot every arm's shape + body (cloning ends the `cl` borrow), then compile
    // each via [`compile_arm`]. An arm is VM-eligible when its body — and every real
    // `&optional` default form — is core vocabulary; otherwise that arm defers
    // (`compiled: None`). Ineligible arms are still recorded so `arm_for` selection
    // stays faithful to `select_arm` (variadic/exact overlap — see ArmSpec).
    struct Src {
        required: Vec<Symbol>,
        optionals: Vec<(Symbol, Value)>, // name + default form (`Nil` = nil-default)
        rest: Option<Symbol>,
        body: Vec<Value>,
    }
    let arms_src: Vec<Src> = cl
        .arms
        .iter()
        .map(|a| Src {
            required: a.params.clone(),
            optionals: a.optionals.clone(),
            rest: a.rest,
            body: a.body.clone(),
        })
        .collect();
    let mut specs: Vec<ArmSpec> = Vec::with_capacity(arms_src.len());
    for s in arms_src {
        let nrequired = s.required.len();
        let noptional = s.optionals.len();
        let has_rest = s.rest.is_some();
        let compiled = compile_arm(
            heap,
            &s.required,
            &s.optionals,
            s.rest,
            &s.body,
            enclosing.clone(),
            self_name,
            defn_name,
            trace_name,
        )
        .map(|mut arm| {
            // Shared-JIT identity (the spawn lever, ADR-101): a simple fixed-arity
            // RUNTIME/PRELUDE closure arm has a stable, process-independent `(id, argc)`
            // key (the same key `cache_key` uses), so its JIT'd native code can be
            // shared across all of the runtime's processes instead of being recompiled
            // per process. See `CompiledArm::share_key`.
            if noptional == 0 && !has_rest && matches!(id.region(), value::RUNTIME | value::PRELUDE)
            {
                arm.share_key = Some((id.0, nrequired as u16));
            }
            Arc::new(arm)
        });
        specs.push(ArmSpec {
            nrequired,
            noptional,
            has_rest,
            compiled,
        });
    }
    // Nothing to gain if no arm compiled (and a wholly-`None` entry would just mask
    // the tree-walker on every call) — defer the closure.
    if specs.iter().all(|s| s.compiled.is_none()) {
        None
    } else {
        Some(CompiledClosure { arms: specs })
    }
}

/// A stable cache key for closure `id`, or `None` if it can't be safely cached /
/// VM-run (ADR-076 §2c(a)). A **RUNTIME** closure (top-level / promoted) is keyed
/// by its own handle `.0`, which is stable for the closure's life. A **LOCAL**
/// closure's handle index is recycled by the collector, so it's keyed instead by
/// the handle of its first body form — but only when that form lives in the
/// immovable RUNTIME code region. A LOCAL closure whose body was built from movable
/// LOCAL forms (e.g. conased by `eval`/quasiquote) has no stable key *and* would
/// put movable handles in the cached `Node` tree, so it's left to the tree-walker.
/// The already-compiled arm for `id`/`argc`, **without compiling anything** — a pure cache
/// read, unlike [`compiled_arm_for`], which compiles on a miss. Used at the tiering election
/// to answer "does this global still resolve to this same arm?" (see
/// [`CompiledArm::self_global_ok`]), where compiling would be re-entrant and expensive.
/// `None` on a miss, which callers must treat as "don't know" — never as "yes".
pub(crate) fn cached_arm_for(heap: &Heap, id: ClosureId, argc: usize) -> Option<Arc<CompiledArm>> {
    let key = cache_key(heap, id)?;
    heap.vm_cache_arm(key, argc).flatten()
}

pub(crate) fn cache_key(heap: &Heap, id: ClosureId) -> Option<VmCacheKey> {
    // Prefer the closure's **AST identity** — its first arm's first body form — over its
    // instance handle, in every region (ADR-215). Require an allocated non-LOCAL pair so
    // the key is stable and collision-free (immediates and interned symbols are shared, so
    // they would alias unrelated closures; a LOCAL cell's slot is recycled by the
    // collector).
    //
    // Why the AST and not the handle: a closure that captures no locals is *promoted on
    // every creation* (ADR-194), so `(spawn (worker))`'s thunk gets a FRESH RUNTIME handle
    // per spawn while reusing one template (`make_closure_cached` already keys that by the
    // same `fn` form). Keying the compiled body by handle therefore missed on every
    // creation — measured as one full bytecode compile per spawned process.
    if let Some(first) = heap
        .closure(id)
        .arms
        .first()
        .and_then(|a| a.body.first())
        .copied()
    {
        if let ValueRef::Pair(p) = first.unpack() {
            if p.region() != value::LOCAL {
                return Some(VmCacheKey::Body(p.0));
            }
        }
    }
    match id.region() {
        value::RUNTIME | value::PRELUDE => Some(VmCacheKey::Runtime(id.0)),
        _ => None, // LOCAL with an unkeyable body, or any other region — not VM-cached.
    }
}

/// The compiled body for closure `id`, compiling-and-caching on first use. Keyed by
/// [`cache_key`] so a local-capturing closure is found by its RUNTIME body code,
/// not its recycled LOCAL handle. `None` (ineligible) is cached too — but only when
/// the closure *has* a stable key; an unkeyable closure simply defers each call
/// (cheap: a region check + a body-handle peek).
/// The per-call hot path: resolve `id`'s `argc` arm, cloning **only** the
/// `Arc<CompiledArm>` (not the enclosing `CompiledClosure`). On a cache hit
/// (the overwhelmingly common case — a recursive or repeated callee) this is a
/// single `vm_cache_arm` lookup + one arm clone. A miss compiles + caches the
/// closure once, then resolves the arm. `None` = no VM arm for `argc` (defer to
/// the tree-walker), identical to `compiled_for(..).and_then(|c| c.arm_for(argc))`.
/// Resolve `id`/`argc` to a compiled arm for **inspection during a leaf probe**, caching
/// nothing. A cache hit is used as-is; a miss compiles a throwaway copy that is then
/// dropped.
///
/// It must not cache, because a compile reached from inside a probe runs under the
/// [`LEAF_RESOLVING`](inline) reentrancy guard and therefore never gets its OWN leaf
/// probe. Installing that arm would hand it to every later call of the callee, silently
/// denying the callee its derivation for the rest of the process — and the callees reached
/// this way are mid-level functions like `(defn mix (i) (+ (sq (add1 i)) (rec 3 0)))`,
/// exactly the shape partial splicing exists to speed up. The throwaway costs one extra
/// (cold, microsecond-scale) arm compile per caller→callee edge during warm-up; in steady
/// state every callee is already cached and this is a lookup.
#[cfg(feature = "jit")]
pub(crate) fn probe_arm_for(heap: &Heap, id: ClosureId, argc: usize) -> Option<Arc<CompiledArm>> {
    let key = cache_key(heap, id)?;
    if let Some(hit) = heap.vm_cache_arm(key, argc) {
        return hit;
    }
    // Read-only peek at the cross-process cache — an entry there was compiled by a real
    // call, so it carries its own metadata and is safe to use (and not ours to install).
    if !crate::core::heap::Heap::shared_arms_disabled() {
        if let Some(cc) = heap.shared_closure_lookup(key.shared_bits()) {
            return cc.arm_for(argc).cloned();
        }
    }
    compile_closure(heap, id).and_then(|cc| cc.arm_for(argc).cloned())
}

/// The `argc` arm of closure `id`, as this process's [`ArmHandle`] — compiling and caching
/// the closure on a miss.
///
/// Returns the **handle**, not the bare `Arc<CompiledArm>`, because every caller wants a
/// handle (ADR-224) and a computed-head call site has no inline cache to keep one in. The
/// handle is memoized per `(closure, argc)` in the `vm_cache` entry, so the steady state of a
/// per-element closure call is a hash lookup plus a process-local `Arc` clone — no
/// allocation, and no touch of the shared arm's refcount. See `Heap::vm_cache_arm_handle`.
/// Does this arm's chunk call `%receive` (directly)? The tree-walker→VM router must not
/// route such an arm: a `receive` inside the routed `vm_apply` is a NESTED run, so it
/// cannot capture-suspend — it dirty-blocks its worker on the mailbox condvar (§7.4),
/// and KI-88's chaos combo showed a routed reader wedging exactly there (core stack:
/// `receive_match ← %receive ← … ← vm_apply ← tw_vm_route`). Left tree-walked, the same
/// receive uses the path those shapes have always used. This is the JIT's `%receive`
/// fence (`chunk_in_jit_subset`) applied to the router; lifting either is §7.3's
/// receive-as-exit design, not a predicate tweak.
pub(crate) fn arm_calls_receive(arm: &CompiledArm) -> bool {
    let receive_sym = crate::core::value::intern("%receive");
    arm.chunk.as_ref().is_none_or(|c| {
        c.code
            .iter()
            .any(|inst| matches!(inst, Inst::Call { head: Some(h), .. } if *h == receive_sym))
    })
}

pub(crate) fn compiled_arm_for(heap: &Heap, id: ClosureId, argc: usize) -> Option<Arc<ArmHandle>> {
    let key = cache_key(heap, id)?;
    if let Some(hit) = heap.vm_cache_arm_handle(key, argc) {
        return hit;
    }
    // Shared-closure fast path (ADR-175): a closure another process already compiled is
    // installed instead of recompiled — the compiled form lives once per runtime, like
    // the AST it was compiled from. Covers both shared regions:
    //   * PRELUDE — sealed (ADR-166) and never freed, so an entry is valid forever;
    //   * RUNTIME — user code, valid until the region is compacted or a generation is
    //     freed. Both are handled: the compactor calls `shared_closures_clear` (a merely
    //     *cached* arm is on no execution stack, so its handles are never rewritten), and
    //     an entry carries the `free_epoch` its publisher observed BEFORE compiling, so a
    //     generation freed mid-compile leaves the entry stale and uninstallable.
    // Every keyable closure's compiled form is shared across the runtime's processes
    // (ADR-215), including a local-capturing one: [`cache_key`] names the closure by its
    // AST (a non-LOCAL body cell), so the key means the same code in every process.
    // Sharing is sound for the
    // same reason as a top-level one: a compiled arm embeds no LOCAL handle by
    // construction (`const_node` promotes every literal and asserts immovability;
    // `MakeClosure` defers rather than embed an unstable `fn_rest`), the captured
    // *values* are read from the closure's env at call time rather than baked in, and
    // slot layout is a property of the AST. Without this, a process that runs a fresh
    // closure once — every `receive` matcher, every spawned handler — recompiled it:
    // measured one compile per process at 8.1 µs, a third of `spawn-live`'s per-unit time.
    let shareable = !crate::core::heap::Heap::shared_arms_disabled();
    if shareable {
        if let Some(cc) = heap.shared_closure_lookup(key.shared_bits()) {
            heap.vm_cache_put(key, Some(cc.clone()));
            // Re-enter through the cache rather than deriving here, so this arity's handle
            // is memoized for every call after this one — but do NOT trust the cache to
            // still hold what we just put there: `vm_cache_arm_handle` begins with
            // `sync_free_epoch`, and a *peer* process can advance `free_epoch` at any
            // instant (`free_runtime_gen` runs from another worker's ordinary safepoint,
            // no stop-the-world), which clears this cache. Falling through to the arm we
            // already hold keeps that race a lost memo instead of a spurious "no VM arm",
            // which would silently tree-walk a compiled body — or panic a caller that
            // resolved twice.
            return heap
                .vm_cache_arm_handle(key, argc)
                .flatten()
                .or_else(|| cc.arm_for(argc).cloned().map(ArmHandle::new));
        }
    }
    // Read the free-epoch BEFORE compiling: if a generation is freed while we compile,
    // the arm we produce may hold handles into it, and publishing under the pre-compile
    // stamp makes the entry dead on arrival rather than poisonous.
    let fe = heap.free_epoch_now();
    // Cold: compile + cache the closure once, then take the arm.
    let compiled = compile_closure(heap, id).map(Arc::new);
    heap.vm_cache_put(key, compiled.clone());
    if shareable {
        if let Some(cc) = &compiled {
            heap.shared_closure_publish(key.shared_bits(), fe, cc.clone());
        }
    }
    // As above: take the handle back out of the cache so it is memoized, not re-derived —
    // with the same fallback, for the same peer-`free_epoch` race.
    heap.vm_cache_arm_handle(key, argc).flatten().or_else(|| {
        compiled
            .and_then(|cc| cc.arm_for(argc).cloned())
            .map(ArmHandle::new)
    })
}

/// Compile `f`'s body NOW, without calling it, and cache the result. Returns whether
/// anything was compiled.
///
/// Exists for line coverage's denominator (ADR-148 tier 2). Arms compile LAZILY — on
/// first call, via [`compiled_arm_for`] — so the set of instrumented lines otherwise
/// contains only lines that already ran, making the ratio a tautology: a fixture whose
/// every function had run reported 100% while a deliberately-uncalled function's lines
/// were absent from BOTH halves. Forcing the compile registers those lines, so a
/// never-called function correctly reports 0%.
///
/// Only the closure's own arms are reached. A nested `(fn …)` inside a body compiles
/// when the enclosing body runs, so an unexecuted body's inner closure stays
/// unmeasured — a known under-count, and a strictly smaller one than not forcing at all.
pub fn precompile(heap: &mut Heap, f: Value) -> bool {
    let ValueRef::Fn(id) = f.unpack() else {
        return false;
    };
    let compiled = compile_closure(heap, id).map(Arc::new);
    let did = compiled.is_some();
    // Cache it if the closure is keyable, so the forced compile isn't wasted work the
    // first real call redoes. An unkeyable closure just recompiles later, as always.
    if let Some(key) = cache_key(heap, id) {
        heap.vm_cache_put(key, compiled);
    }
    did
}

/// The higher-order-fn closure-call fast path (gated). A `reduce`/`fold`/… driver calls
/// the SAME step closure once per element; the general per-call path (`apply_value` → `dispatch`)
/// re-resolves the closure's arm (`vm_cache_arm`) and re-runs the passthrough/arity matching every
/// element — ~40–50% of `pipeline`/`nqueens` per the profile, and a user-closure fold is ~60× a
/// primitive one for identical work. This resolves the arm ONCE ([`hof_resolve`]); the driver then
/// calls [`hof_apply_step`] per element, which only re-reads the (rooted, GC-current) closure for
/// its captured env and calls the cached arm via `vm_apply` — skipping the re-resolution.
///
/// **Default ON**; `BROOD_NO_HOF` opts out (the A/B lever). A modest, broad win — ~8% on
/// `nqueens`, ~19% on a light-closure `range-reduce` — for any Rust HOF driver folding a user
/// closure. (It removes dispatch's self-overhead, not the per-call `push_frame`/`vm_run_bc`
/// protocol — that's the separate lean-native-call lever.)
#[cfg(feature = "jit")]
pub(crate) fn hof_fast_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_HOF").is_none())
}

#[cfg(not(feature = "jit"))]
pub(crate) fn hof_fast_enabled() -> bool {
    std::env::var_os("BROOD_NO_HOF").is_none()
}

/// A step closure resolved once for the HOF fast path: the closure identity (re-checked per call
/// so a late-rebind falls back) + its compiled arm (GC-stable `Arc`, off the heap graph).
pub(crate) struct HofArm {
    pub(crate) id: ClosureId,
    pub(crate) arm: Arc<ArmHandle>,
    /// The step arm's IC block ([`Heap::vm_arm_block`]), resolved once here so the
    /// per-element native fast-frame installs it without a registry lookup. Only the native
    /// (jit) fast path reads it, so a `--no-default-features` build carries neither the
    /// field nor the lookup that fills it — the mirror of `CompiledArm::dbg_name`, which is
    /// ungated precisely because the bytecode VM does read that one (ADR-199 build note).
    #[cfg(feature = "jit")]
    pub(crate) bases: (u32, u32),
}

/// Resolve `f` to a cached [`HofArm`] if it's a **plain fixed-arity-`argc` VM closure** (not a
/// thin passthrough wrapper, no optional/rest) — else `None` (the driver uses its general path).
/// Returns `None` when the gate is off. Call once, before the per-element loop.
pub(crate) fn hof_resolve(heap: &Heap, f: Value, argc: usize) -> Option<HofArm> {
    if !hof_fast_enabled() {
        return None;
    }
    let id = match f.unpack() {
        ValueRef::Fn(id) => id,
        _ => return None,
    };
    // A thin-wrapper passthrough (`>` → `%lt`, …) redirects; leave those to `dispatch`.
    if crate::eval::passthrough_arm(heap, id, argc).is_some() {
        return None;
    }
    let arm = compiled_arm_for(heap, id, argc)?;
    if arm.nrequired != argc || arm.noptional != 0 || arm.rest_slot.is_some() {
        return None;
    }
    #[cfg(feature = "jit")]
    let bases = heap.vm_arm_block(&arm);
    Some(HofArm {
        id,
        arm,
        #[cfg(feature = "jit")]
        bases,
    })
}

/// Call the cached step closure on `args`. `f` is the *current* (rooted, GC-relocated) closure
/// value — re-read by the caller each element; if it no longer names the cached closure (a
/// late-rebind), returns `None` so the caller falls back to its general per-call path. Otherwise
/// runs the cached arm in the closure's captured env — via the **native fast-frame** when the arm
/// has installed, epoch-current JIT code ([`hof_apply_native`]), else via `vm_apply`.
pub(crate) fn hof_apply_step(
    heap: &mut Heap,
    hof: &HofArm,
    f: Value,
    args: &[Value],
) -> Option<LispResult> {
    let id = match f.unpack() {
        ValueRef::Fn(id) => id,
        _ => return None,
    };
    if id != hof.id {
        return None;
    }
    let cenv = heap.closure(id).env.unwrap_or_else(|| heap.global());
    // Fast-frame straight into the step's native code when installed (`nqueens`/`pipeline`:
    // the per-element step is JIT-eligible, but `vm_apply` re-enters the `vm_run_bc`
    // trampoline + `jit_tier` every element — ~25%+ of both per the profile). Falls back to
    // `vm_apply` when the arm isn't natively callable (not tiered yet / over the native cap /
    // shape) or deopts.
    #[cfg(feature = "jit")]
    if hof_native_enabled() {
        if let Some(r) = hof_apply_native(heap, hof.arm.arc(), args, cenv, hof.bases) {
            return Some(r);
        }
    }
    Some(vm_apply(heap, hof.arm.clone(), args, cenv))
}

/// Run the HOF step arm via the JIT **fast-frame** protocol — stage the args + captures and jump
/// the installed native entry directly, skipping the `vm_apply` → `vm_run_bc` trampoline (frame
/// save/restore, per-loop safepoints) and the per-call `jit_tier` re-entry. Returns `Some(result)`
/// when it ran the native call (following an outcome-4 tail chain and re-running a deopt on the VM
/// itself), or `None` when the arm can't be linked (not yet native / over the native-recursion cap
/// / non-trivial shape) so the caller falls back to `vm_apply`.
///
/// Mirrors the computed-head native-link block in [`jit_dispatch_call`]: same frame setup, the same
/// `capture_value` fill (the fast frame bypasses `push_frame`, so captured lexicals must be filled
/// here), and the same 0/3/4/deopt outcome handling. `hof_resolve` already proved the arm is
/// fixed-arity-`argc` with no optionals/rest, so `capture_base == argc`.
/// Default ON; `BROOD_NO_HOF_JIT` opts out (the A/B / correctness lever for the HOF native
/// fast-frame, independent of `BROOD_NO_HOF` which disables the whole cached-arm path).
#[cfg(feature = "jit")]
pub(crate) fn hof_native_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_HOF_JIT").is_none())
}

#[cfg(feature = "jit")]
pub(crate) fn hof_apply_native(
    heap: &mut Heap,
    arm: &Arc<CompiledArm>,
    args: &[Value],
    cenv: EnvId,
    bases: (u32, u32),
) -> Option<LispResult> {
    use std::sync::atomic::Ordering::Acquire;
    let argc = args.len();
    let code = arm.jit_code.load(Acquire);
    if code.is_null() || code == crate::jit::BAILED || code == crate::jit::QUEUED {
        // Which of the three, under `perf-stats`: `jit_link_done` reading 0 against N calls
        // says the fast frame never engaged but not why, and the three have entirely
        // different fixes (never got hot / deopt-latched off / still compiling).
        //
        // The arms bump three DIFFERENT counters and are identical only in a build without
        // `perf-stats`, where `perf_bump!` expands to nothing — so collapsing them would
        // silently merge the three cases this comment exists to keep apart.
        #[allow(clippy::if_same_then_else)]
        if code == crate::jit::BAILED {
            crate::perf_bump!(hof_decline_bailed);
            // Name the arm under BROOD_JIT_BAIL_TRACE. The counter says "some HOF arm is
            // BAILED N times" and stops there — and the `receive` matcher for a tagged
            // tuple turned out to be exactly such an arm on 2026-08-20, invisible in every
            // other trace because it is refused somewhere that does not report.
            {
                static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                    static SEEN: std::sync::OnceLock<
                        std::sync::Mutex<std::collections::HashSet<usize>>,
                    > = std::sync::OnceLock::new();
                    let seen = SEEN.get_or_init(|| std::sync::Mutex::new(Default::default()));
                    let key = Arc::as_ptr(arm) as usize;
                    if seen.lock().map(|mut g| g.insert(key)).unwrap_or(false) {
                        let name = arm
                            .dbg_name
                            .map(crate::core::value::symbol_name_ref)
                            .unwrap_or("<closure>");
                        let ops: Vec<&str> = arm
                            .chunk
                            .as_ref()
                            .map(|c| c.code.as_slice())
                            .unwrap_or(&[])
                            .iter()
                            .map(crate::eval::compile::jit_plan::codegen::inst_opcode_name)
                            .collect();
                        eprintln!(
                            "[jit-bail] arm={name} reason=hof-arm-already-bailed nslots={} ops=[{}]",
                            arm.nslots,
                            ops.join(" ")
                        );
                    }
                }
            }
        } else if code == crate::jit::QUEUED {
            crate::perf_bump!(hof_decline_queued);
        } else {
            crate::perf_bump!(hof_decline_nocode);
        }
        return None;
    }
    // Over the native-recursion cap → don't link (would overflow the native stack); let the VM
    // drain the recursion. (`hof_resolve` guaranteed nslots>0 / noptional==0 / rest none / the
    // `argc` arm; re-check the epoch here since a `def` can recompile mid-fold.)
    if heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT
        || !crate::eval::compile::jit_runtime::jit_native_headroom_ok(heap.jit_native_depth)
    {
        crate::perf_bump!(hof_decline_depth);
        return None;
    }
    if arm.compile_epoch.load(Acquire) != heap.global_epoch() {
        crate::perf_bump!(hof_decline_epoch);
        return None;
    }
    // Size the fast frame to the version we are about to CALL — keyed on the `code` pointer
    // loaded above, not on a second (independently-racing) read of `inline_installed`. A peer
    // process sharing this `CompiledArm` (ADR-215) can swap the inlined upgrade in between the
    // two reads, and then the small native we hold would run against an `inline_nslots` frame
    // (its outcome-4 staging read back at the wrong top). See `frame_size_for_code`.
    let nslots = crate::eval::compile::jit_runtime::frame_size_for_code(arm, code);
    // Diagnostic label for the debug staged-stale report / BROOD_JIT_VERIFY (the arm's defining
    // name if known, else leave the caller's — cosmetic only).
    let dbg_sym = arm.dbg_name.unwrap_or(heap.jit_dbg_fn);
    let base = heap.roots_len();
    for &a in args {
        heap.push_root(a);
    }
    // Runtime BROOD_JIT_VERIFY: scan the staged args for a stale handle (the fast frame bypasses
    // `jit_dispatch_call`'s scan), matching `jit_run_fast_link`. NO_SITE: no call site (computed).
    if jit_verify_active() {
        jit_verify_staged(heap, base, base + argc, dbg_sym, NO_SITE, argc);
    }
    heap.extend_roots_to_nil(base + nslots);
    // Root the callee's captured env so a tenure inside the arm forwards it (the deopt path
    // below re-reads the live id from this root).
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(cenv);
    // Fill the capture slots from the captured env — the fast frame placed only params + nil.
    // `capture_base == argc` (nrequired == argc, no optionals/rest). No alloc → no GC → the
    // nil-filled body slots stay valid.
    if !arm.capture_names.is_empty() {
        let cenv_live = heap.read_root_env(env_root);
        for (k, &name) in arm.capture_names.iter().enumerate() {
            let v = heap.capture_value(cenv_live, k, name);
            heap.set_root_at(base + argc + k, v);
        }
    }
    let depth = heap.jit_native_depth;
    let saved = std::mem::replace(&mut heap.jit_call_env, env_root);
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, dbg_sym);
    // The step's native reads its OWN IC block through the heap cursors (ADR-175).
    let saved_bases = heap.set_ic_bases(bases);
    heap.jit_native_depth = depth + 1;
    // SAFETY: `code` is a finalized [`crate::jit::JitArmFn`] from `jit_lower_arm`, kept for
    // the process in `GLOBAL_JIT`; the frame is at `roots[base..]`; validated current by the
    // epoch check above.
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(code) };
    // Destination for a Done result: this entry hands one back, so a stack local.
    let mut ret = Value::Nil;
    heap.native_gateway_seq += 1;
    let gw_seq = heap.native_gateway_seq;
    let saved_gw = std::mem::replace(&mut heap.cur_native_gateway, gw_seq);
    let outcome = f(heap as *mut Heap, base as i64, &mut ret as *mut Value);
    heap.cur_native_gateway = saved_gw;
    heap.jit_native_depth = depth;
    heap.set_ic_bases(saved_bases);
    heap.jit_call_env = saved;
    heap.jit_dbg_fn = saved_fn;
    // Suspend-host latch (see `jit_latch_suspend_host` / `Heap::blocked_under_gateway`):
    // a HOF step arm can enclose a parking receive too.
    if heap.blocked_under_gateway == gw_seq {
        heap.blocked_under_gateway = 0;
        jit_runtime::jit_latch_suspend_host(arm);
    }
    // Deopt feedback (see `jit_deopt_feedback`): the HOF step arm is the canonical
    // watched shape (nqueens' reduce closure).
    if outcome == 1 {
        crate::perf_bump!(hof_native_deopt);
        // KI-48 follow-on: name WHERE the native bailed. The deopt journal at the arm's
        // checkpoint slot packs the resume position as `ip = p >> 16`, so this maps a deopt
        // back to the bytecode instruction that produced it — the difference between "this
        // arm thrashes" and "this arm thrashes at instruction N, a Call to `vector-length`".
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
            let name = arm
                .dbg_name
                .map(crate::core::value::symbol_name_ref)
                .unwrap_or("<closure>");
            let slot = arm.ckpt_slot;
            let journal = if slot != u32::MAX {
                match heap.root_at(base + slot as usize) {
                    Value::Int(p) if p > 0 => Some(p),
                    _ => None,
                }
            } else {
                None
            };
            match journal {
                Some(p) => {
                    let ip = (p >> 16) as usize;
                    let op = arm
                        .chunk
                        .as_ref()
                        .and_then(|c| c.code.get(ip))
                        .map(crate::eval::compile::jit_plan::codegen::inst_opcode_name)
                        .unwrap_or("<out-of-range>");
                    eprintln!(
                        "[jit-deopt] arm={name} resume_ip={ip} op={op} reason#{} (journalled)",
                        heap.jit_deopt_reason()
                    );
                }
                None => eprintln!(
                    "[jit-deopt] arm={name} no journal — re-runs from ip 0 (ckpt_slot={slot}) reason#{}",
                    heap.jit_deopt_reason()
                ),
            }
        }
    }
    if arm.deopt_watch {
        use std::sync::atomic::Ordering::Relaxed;
        if outcome == 1 {
            jit_deopt_feedback(arm);
        } else if arm.jit_deopts.load(Relaxed) != 0 {
            arm.jit_deopts.store(0, Relaxed);
        }
    }
    // `f()` may have collected + relocated the captured env; re-read the live id before dropping
    // its root (the deopt path hands it to `vm_apply`).
    let cenv_live = heap.read_root_env(env_root);
    heap.truncate_env_roots(env_base);
    match outcome {
        0 => {
            crate::perf_bump!(jit_link_done);
            heap.truncate_roots(base);
            Some(Ok(ret))
        }
        3 => {
            heap.truncate_roots(base);
            Some(Err(jit_take_error(heap).unwrap_or_else(|| {
                LispError::type_err("jit step deopt without a parked error")
            })))
        }
        // Tail call (4): the callee JIT'd a tail — [callee, arg0..argN] staged above its frame at
        // `[base+nslots, roots_len)`. Follow the chain rather than re-running via `vm_apply`.
        4 => {
            let staged_start = base + nslots;
            let staged_end = heap.roots_len();
            if staged_end > staged_start {
                let staged_callee = heap.root_at(staged_start);
                let staged_argc = staged_end - staged_start - 1;
                let staged_args: SmallVec<[Value; 4]> = (1..=staged_argc)
                    .map(|k| heap.root_at(staged_start + k))
                    .collect();
                heap.truncate_roots(base);
                return Some(apply_value(
                    heap,
                    staged_callee,
                    &staged_args,
                    heap.global(),
                ));
            }
            heap.truncate_roots(base);
            Some(Err(LispError::type_err(
                "jit step tail with no staged call",
            )))
        }
        // deopt (1) / preempt (2): re-run the arm on the VM. The args survive in the param slots
        // `[base, base+argc)` (GC-updated); re-read, drop the frame, and `vm_apply`.
        _ => {
            crate::perf_bump!(jit_link_rerun);
            // Deopt-resume (see `CompiledArm::ckpt_slot`): resume AT the checkpoint,
            // frame intact — never re-running side effects. `1 | 2` (deopt AND preempt),
            // consistent with the other three consumers — see `jit_run_fast_link` (KI-18).
            if matches!(outcome, 1 | 2) {
                if let Some((resume, rip, depth)) = jit_ckpt_resume(heap, arm, base, nslots) {
                    return Some(vm_resume_deopt(heap, resume, base, cenv_live, rip, depth));
                }
            }
            let mut argv2: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
            for k in 0..argc {
                argv2.push(heap.root_at(base + k));
            }
            heap.truncate_roots(base);
            Some(vm_apply(
                heap,
                ArmHandle::new(arm.clone()),
                &argv2,
                cenv_live,
            ))
        }
    }
}
