//! Compiling a closure's arms and caching the result: `compile_arm` lowers one arm
//! (with `compile_closure` over all of them), the body cache is keyed by `cache_key`
//! and shared across a runtime's processes (ADR-175/215), `precompile` warms it, and
//! the `hof_*` fast paths let a native higher-order primitive step a compiled callback
//! without the full call ceremony.

use super::*;

// ============ linear map-accumulator → Table rewrite (ADR-112, ADR-360 in docs/decisions.md) ============
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
/// the `&` rest param — then compiles the body. A real default becomes part of the arm's
/// own bytecode: a prologue ahead of the body sets each defaulted optional whose argument
/// was missing, which `push_frame` marks in a hidden flag slot (`optional_flags`).
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
    module: Option<Symbol>,
) -> Option<CompiledArm> {
    // Grab the first body form before `body` is shadowed by the compiled Node —
    // its recorded reader position carries the defining source file (`src_file`).
    let body_first_form = body.first().copied();
    let nrequired = required.len();
    let noptional = optionals.len();
    let mut scope = Scope::with_params_enclosing(&[], enclosing);
    scope.module = module;
    // The self-call optimization applies only to a plain fixed-arity closure (no
    // `&optional`/`&` rest), where a tail call passing exactly `nrequired` args
    // re-runs this arm verbatim. With optionals/rest the frame-fill differs per
    // call, so such calls fall back to the regular env-resolved path (correct,
    // just unoptimized).
    if let Some(name) = self_name
        && noptional == 0
        && rest.is_none()
    {
        scope.self_call = Some((name, nrequired));
    }
    // `defn` tail self-calls get the same inline frame-reset via SelfCall. The
    // in-flight call holds an Arc to its own compiled arm, so it correctly runs
    // the current compiled version even if the global is redefined mid-call.
    if let Some(name) = defn_name
        && noptional == 0
        && rest.is_none()
    {
        scope.self_call = Some((name, nrequired));
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
    // The self-name rides in as a capture too (the frame binds it to the closure); mark
    // its slot so a tail self-call through that name is a `SelfCall`, not a shadowed call.
    if let Some(name) = self_name
        && capture_names.contains(&name)
    {
        scope.self_slot = scope.lookup(name);
    }
    let mut body = compile_body(heap, body, &mut scope, true)?;
    // Escape-analysis scalar replacement (lever 2): eliminate non-escaping `(let (p […]) …)`
    // vector allocations, binding their elements to fresh slots `[scope.max ..]` and rewriting
    // `(nth p K)` to direct reads. Bumps `scope.max` for the element slots; makes the arm
    // simpler (fewer allocs, no `nth`), so it JITs better. No-op for arms without the pattern.
    ea_scalar_replace(&mut body, &mut scope.max);
    // ADR-368: the 3-arity map read and the single-pair `assoc` become instructions LAST,
    // once the linear-map machinery has seen them as the calls it matches.
    inline::lower_map_prim3(heap, &mut body);
    // Real `&optional` defaults run in the arm's own bytecode, ahead of the body: for each,
    // `(if <missing> (set <slot> <default>) nil)`, left to right, so a later default sees an
    // earlier one. "Missing" is a hidden flag slot `push_frame` sets to `true`, above every
    // slot the body uses; a truthiness test, so no operator a program could rebind is
    // involved. The body stays last, so its tail position — and every tail call in it — is
    // unchanged.
    let mut optional_flags: Vec<u32> = Vec::with_capacity(noptional);
    let mut prologue: Vec<Node> = Vec::new();
    for (j, default) in optional_defaults.into_iter().enumerate() {
        match default {
            None => optional_flags.push(u32::MAX),
            Some(node) => {
                let flag = scope.max;
                scope.max += 1;
                optional_flags.push(flag as u32);
                // The flag is cleared as the default lands, so a re-run of the arm from its
                // first instruction (a deopt that resumes at ip 0) does not evaluate it twice.
                prologue.push(Node::If(
                    Box::new(Node::Local(flag)),
                    Box::new(Node::LetBind {
                        binds: Box::new([
                            (nrequired + j, node),
                            (flag, Node::Const(ConstVal::new(Value::nil()))),
                        ]),
                        body: Box::new(Node::Const(ConstVal::new(Value::nil()))),
                    }),
                    Box::new(Node::Const(ConstVal::new(Value::nil()))),
                ));
            }
        }
    }
    if !prologue.is_empty() {
        prologue.push(body);
        body = Node::Do(prologue.into_boxed_slice());
    }
    let optional_flags = optional_flags.into_boxed_slice();
    let has_runtime_handles = node_has_rt_handles(&body);
    // The body as flat bytecode — what the driver runs.
    let chunk = compile_chunk(&body);
    // Line coverage (ADR-148 tier 2): register this arm's instrumented lines as they
    // are compiled. This is the report's DENOMINATOR — see `coverage.rs` for why it
    // cannot be inferred from the source text instead.
    if crate::diagnostics::coverage::enabled()
        && let (Some(chunk), Some(file)) = (
            chunk.as_ref(),
            body_first_form
                .and_then(|f| heap.form_pos(f))
                .and_then(|(_, file)| file),
        )
    {
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
    // would mis-bail them. Do not also require a non-tail call: a **call-free** arm (one
    // multiply, like mandelbrot's `->float`) can thrash deopts just as hard, invisibly,
    // since the answers stay right. A healthy arm pays one relaxed load per native
    // completion; an arm that deopts 16 times consecutively is not being served by its
    // native code, calls or no calls.
    let deopt_watch = chunk
        .as_ref()
        .is_some_and(|c| !c.code.iter().any(|i| matches!(i, Inst::SelfCall { .. })));
    let nslots_total = scope.max + spill_reserve + ckpt_reserve;
    let uid = next_arm_uid();
    let site_pos = std::mem::take(&mut scope.site_pos).into_boxed_slice();
    let staged_head_pos = std::mem::take(&mut scope.staged_head_pos).into_boxed_slice();
    let local_call_heads = scope.take_local_call_heads();
    let receive_at_head = chunk
        .as_ref()
        .is_some_and(|c| super::jit_plan::receive_at_head(&c.code));
    let src_file = body_first_form
        .and_then(|f| heap.form_pos(f))
        .and_then(|(_, file)| file);
    // Recursive self-inlining (Phase B, §6b — two-stage tiering):
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
                                // Equal sizes would read a leaf-spliced frame as the small
                                // layout, decoding the small body's `ckpt_slot` — a live
                                // local in the spliced layout — as a deopt journal (wrong
                                // answers on e.g. a `(sum-down (dec n) (+ acc n))` loop).
                                // Such a derivation is `pure_self` and *unjournalled*, so
                                // only the size can separate the pair. One reserved slot
                                // restores the invariant the rest of the machinery asserts.
                                let leaf_nslots = d.nslots.max(nslots_total + 1);
                                // The resume arm (see `ir::LeafInline::resume`): the same
                                // function over the spliced body, so a deopt out of the
                                // inlined native can be resumed in the ip space it
                                // journalled against. It shares this arm's `uid` (hence
                                // its inline-cache block) and its identity/diagnostic
                                // fields; only body, chunk, frame and checkpoint differ.
                                // Never published to the cross-process cache (`share_key`
                                // stays None): this arm is reachable only through its
                                // caller's derivation. It is the spliced body, so it must
                                // not itself splice again (no `inline_name`, no `leaf`).
                                let resume = CompiledArm {
                                    nrequired,
                                    nslots: leaf_nslots,
                                    nsites: scope.sites,
                                    ngsites: ir::gsite_reservation(scope.gsites, scope.sites),
                                    uid,
                                    site_pos: site_pos.clone(),
                                    staged_head_pos: staged_head_pos.clone(),
                                    local_call_heads: local_call_heads.clone(),
                                    body: d.body,
                                    chunk: Some(d.chunk),
                                    has_runtime_handles,
                                    ckpt_slot: d.ckpt_slot,
                                    // `share_key` stays None (the default): never published
                                    // to the cross-process cache, as this arm is reachable
                                    // only through its caller's derivation.
                                    fn_name: trace_name,
                                    src_file: src_file.clone(),
                                    module: scope.module,
                                    capture_names: capture_names.clone(),
                                    dbg_name: defn_name,
                                    #[cfg(feature = "jit")]
                                    inline_nslots: leaf_nslots,
                                    ..CompiledArm::default()
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
                                        epoch: AtomicU64::new(heap.global_epoch()),
                                        callees: d.callees,
                                        relocations: heap.code_relocations(),
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
        optional_flags,
        rest_slot: rest.map(|_| nrequired + noptional),
        nslots: nslots_total,
        nsites: scope.sites,
        ngsites: ir::gsite_reservation(scope.gsites, scope.sites),
        uid,
        site_pos,
        staged_head_pos,
        local_call_heads,
        receive_at_head,
        body,
        chunk,
        has_runtime_handles,
        deopt_watch,
        ckpt_slot,
        fn_name: trace_name,
        // The file the body was read from — trace entries name it as the call
        // site's file (a fn's calls are in its own source). Cold: once per arm
        // compile.
        src_file,
        module: scope.module,
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
        leaf,
        ..CompiledArm::default()
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
    // `BROOD_TRACE_COMPILE`: which closures compile, and how often.
    if crate::diagnostics::debug_flags::trace_compile() {
        let c = heap.closure(id);
        let body_region = c
            .arms
            .first()
            .and_then(|a| a.body.first())
            .map_or_default(|body| format!("{:?}", body.unpack()));
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
    // The contract boundary (ADR-383): what the closure records of its authorship, else
    // — for a `def`'d closure whose construction recorded nothing, the tree-walked
    // `(def M/f (fn …))` at a module's top level — the module of its own qualified name.
    let module: Option<Symbol> = cl.module.or_else(|| {
        cl.name
            .and_then(crate::builtins::contracts::module_of_qualified)
    });
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
            module,
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
    // same `fn` form). Keying the compiled body by handle would miss on every creation —
    // one full bytecode compile per spawned process.
    if let Some(first) = heap
        .closure(id)
        .arms
        .first()
        .and_then(|a| a.body.first())
        .copied()
        && let ValueRef::Pair(p) = first.unpack()
        && p.region() != value::LOCAL
    {
        return Some(VmCacheKey::Body(p.0));
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
    // A hit whose bindings a module load has since staled (ADR-366) is not what a caller
    // would run next — fall through to a fresh throwaway, exactly as the call path will
    // recompile it; the eviction itself is the call path's, this probe installs nothing.
    if let Some(hit) = heap.vm_cache_arm(key, argc)
        && !hit.as_ref().is_some_and(|arm| {
            arm.stale_bindings
                .load(std::sync::atomic::Ordering::Acquire)
        })
    {
        return hit;
    }
    // Read-only peek at the cross-process cache — an entry there was compiled by a real
    // call, so it carries its own metadata and is safe to use (and not ours to install).
    if let Some(cc) = heap.shared_closure_lookup(key.shared_bits())
        && !cc.stale_bindings()
    {
        return cc.arm_for(argc).cloned();
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
    *arm.calls_receive.get_or_init(|| {
        let receive_sym = crate::core::value::intern("%receive");
        arm.chunk.as_ref().is_none_or(|c| {
            c.code
                .iter()
                .any(|inst| matches!(inst, Inst::Call { head: Some(h), .. } if *h == receive_sym))
        })
    })
}

/// Does a NON-tail call site of this arm reach `%receive` through NAMED callees — is this
/// the arm that would HOST a parking receive if it went native? A native arm's non-tail
/// callee runs nested under its gateway, and everything the callee reaches (its own calls,
/// tail or not) runs there too, where a `receive` cannot be state-captured and dirty-blocks
/// the OS worker; `jit_latch_suspend_host` then latches the arm BAILED on the first such
/// park. This is that latch asked BEFORE the lowering, so the arm is refused with a name
/// (`hosts-receive`) instead of compiled, parked dirty once, and latched — the state
/// `make tier-audit` reported for `bench-supervisor/fill` (`fill` → `start-child` →
/// `gen/call` → `%receive`, three named hops). The direct `%receive` fence
/// (`chunk_in_jit_subset`) and [`arm_calls_receive`] see only the arm's own chunk.
///
/// A tail site is NOT followed: a native arm's staged tail call is dispatched by the
/// driver at frame level (`jit_dispatch_tail`), where a receive down that chain suspends
/// cleanly — `vm_direct_call.rs`'s `via` is that shape and must keep lowering. Only callees
/// this process has already compiled are read (`cached_arm_for`, no compile here); a
/// computed head, an unbound name or an uncompiled callee reads as "no receive", and the
/// latch stays the safety net for what this cannot see. Bounded by a visited set of 64.
#[cfg(feature = "jit")]
pub(crate) fn arm_hosts_receive(heap: &Heap, arm: &CompiledArm, env: EnvId) -> bool {
    let receive_sym = crate::core::value::intern("%receive");
    let Some(chunk) = arm.chunk.as_ref() else {
        return false;
    };
    let mut seen: Vec<u64> = vec![arm.uid];
    // An arm that receives at its head (ADR-410) hosts its OWN `%receive` by design — the park
    // leaves native code as a re-run from ip 0. Its callees are still checked: one that
    // reaches a `receive` would park under this native frame.
    let mut work: Vec<(crate::core::value::Symbol, usize)> = chunk
        .code
        .iter()
        .filter_map(|inst| match inst {
            Inst::Call {
                head: Some(h),
                argc,
                tail: false,
                ..
            } if !(arm.receive_at_head && *h == receive_sym) => Some((*h, *argc)),
            _ => None,
        })
        .collect();
    while let Some((sym, argc)) = work.pop() {
        if sym == receive_sym {
            return true;
        }
        let Some(Value::Fn(id)) = heap.env_get(env, sym) else {
            continue;
        };
        let Some(callee) = cached_arm_for(heap, id, argc) else {
            continue;
        };
        if seen.contains(&callee.uid) {
            continue;
        }
        if seen.len() >= 64 {
            return false;
        }
        seen.push(callee.uid);
        if let Some(c) = callee.chunk.as_ref() {
            for inst in &c.code {
                if let Inst::Call {
                    head: Some(h),
                    argc,
                    ..
                } = inst
                {
                    work.push((*h, *argc));
                }
            }
        }
    }
    false
}

/// Does this process's cached `argc` arm of `id` carry the ADR-366 stale mark? `None` when
/// nothing is cached — which after a stale mark means "dropped by the lookup sync and not
/// yet recompiled", a state a guard must tell apart from "recompiled" (`Some(false)`). A pure
/// read of the per-process cache — nothing is compiled, evicted, synced or installed — for
/// the `%vm-arm-stale?` probe.
#[cfg(feature = "dev-tools")]
pub(crate) fn cached_arm_stale(heap: &Heap, id: ClosureId, argc: usize) -> Option<bool> {
    let key = cache_key(heap, id)?;
    heap.vm_cache_arm_raw(key, argc).map(|arm| {
        arm.stale_bindings
            .load(std::sync::atomic::Ordering::Acquire)
    })
}

pub(crate) fn compiled_arm_for(heap: &Heap, id: ClosureId, argc: usize) -> Option<Arc<ArmHandle>> {
    let key = cache_key(heap, id)?;
    // (ADR-366: a body marked stale by a module load was dropped by the lookup's own sync,
    // so a hit here is never stale, and a miss recompiles with the bindings present.)
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
    // closure once — every `receive` matcher, every spawned handler — recompiled it.
    //
    // A peer's stale entry (ADR-366) is dropped, not installed — whoever recompiles first
    // republishes.
    if let Some(cc) = heap
        .shared_closure_lookup(key.shared_bits())
        .filter(|cc| !cc.stale_bindings())
    {
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
    // Read the free-epoch BEFORE compiling: if a generation is freed while we compile,
    // the arm we produce may hold handles into it, and publishing under the pre-compile
    // stamp makes the entry dead on arrival rather than poisonous.
    let fe = heap.free_epoch_now();
    // Cold: compile + cache the closure once, then take the arm.
    let compiled = compile_closure(heap, id).map(Arc::new);
    heap.vm_cache_put(key, compiled.clone());
    if let Some(cc) = &compiled {
        heap.shared_closure_publish(key.shared_bits(), fe, cc.clone());
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
/// contains only lines that already ran, making the ratio a tautology (an uncalled
/// function's lines would be absent from BOTH halves). Forcing the compile registers those lines, so a
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

/// The higher-order-fn closure-call fast path. A `reduce`/`fold`/… driver calls the SAME step
/// closure once per element; the general per-call path (`apply_value` → `dispatch`) re-resolves
/// the closure's arm and re-runs the passthrough/arity matching every element. This resolves the
/// arm ONCE; the driver then calls [`hof_apply_step`] per element.
///
/// Resolve `f` to a cached [`HofArm`] if it's a **plain fixed-arity-`argc` VM closure** (no
/// optional/rest) — else `None` (the driver uses its general path). Returns `None` when the gate
/// is off (`BROOD_NO_HOF=1`). Call once, before the per-element loop.
pub(crate) fn hof_resolve(heap: &Heap, f: Value, argc: usize) -> Option<HofArm> {
    if !crate::diagnostics::debug_flags::higher_order_fast_path() {
        return None;
    }
    let id = match f.unpack() {
        ValueRef::Fn(id) => id,
        _ => return None,
    };
    // A thin-wrapper passthrough takes this path too: its compiled arm IS the cheap form —
    // its body inlines to a `Prim2` or a direct native call.
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
    // `BROOD_NO_HOF_JIT=1` opts out of the native fast frame alone.
    #[cfg(feature = "jit")]
    if crate::diagnostics::debug_flags::higher_order_native()
        && let Some(r) = hof_apply_native(heap, hof.arm.arc(), args, cenv, hof.bases)
    {
        return Some(r);
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
/// The entry and the outcome are every nested native entry's ([`enter_native`],
/// [`finish_nested`]); this only builds the frame — the fast frame bypasses `push_frame`, so
/// it fills the captures itself. `hof_resolve` already proved the arm is fixed-arity-`argc`
/// with no optionals/rest, so `capture_base == argc`.
#[cfg(feature = "jit")]
pub(crate) fn hof_apply_native(
    heap: &mut Heap,
    arm: &Arc<CompiledArm>,
    args: &[Value],
    cenv: EnvId,
    bases: (u32, u32),
) -> Option<LispResult> {
    use std::sync::atomic::Ordering::{Acquire, Relaxed};
    let argc = args.len();
    // A receive loop's native code is the driver's alone (ADR-410): a callback runs it on the
    // VM, where a `receive` under the builtin blocks rather than parks.
    if arm.receive_at_head {
        return None;
    }
    let mut code = arm.jit_code.load(Acquire);
    // The spinning loop's escape hatch (`JIT_QUEUED_SYNC_EDGES`), for a callback: it has no
    // back-edge to count, so count the calls it ran interpreted while its compile waited in
    // the queue. Behind a backed-up queue a whole fold ran that way — `json`'s closing
    // `reduce` over 20 000 records, every element on the VM. `jit_calls` is not counted
    // while an arm is queued, so it is free to count here.
    if code == crate::jit::QUEUED
        && (arm.jit_calls.fetch_add(1, Relaxed) + 1)
            .is_multiple_of(crate::eval::compile::jit_runtime::JIT_QUEUED_SYNC_EDGES)
    {
        crate::eval::compile::jit_runtime::jit_compile_now_with_args(heap, arm, args);
        code = arm.jit_code.load(Acquire);
    }
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
            // BAILED N times" and stops there; such an arm (e.g. a `receive` matcher) can be
            // invisible in every other trace because it is refused somewhere that does not
            // report.
            {
                if crate::diagnostics::debug_flags::jit_bail_trace() {
                    static SEEN: std::sync::OnceLock<
                        std::sync::Mutex<std::collections::HashSet<usize>>,
                    > = std::sync::OnceLock::new();
                    let seen = SEEN.get_or_init(|| std::sync::Mutex::new(Default::default()));
                    let key = Arc::as_ptr(arm) as usize;
                    if seen.lock().map(|mut g| g.insert(key)).unwrap_or(false) {
                        eprintln!(
                            "[jit-bail] arm={} reason=hof-arm-already-bailed nslots={} ops=[{}]",
                            arm.dbg_label(),
                            arm.nslots,
                            arm.dbg_opcodes()
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
    // Tier work only an entry does (an upgrade to queue or swap in): run this call through
    // the driver, which does it and settles the arm.
    if crate::eval::compile::needs_tier_entry(arm, code) {
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
    fill_captures(heap, arm, base, cenv);
    let native_depth = heap.jit_native_depth;
    // Destination for a Done result: this entry hands one back, so a stack local.
    let mut ret = Value::Nil;
    // A builtin calling back: never an unwindable entry (ADR-401) — the builtin's own Rust
    // frame is between this and any driver.
    let exit = enter_native(
        heap,
        NativeEntry {
            code,
            base,
            env: EntryEnv::Bare(cenv),
            dbg_fn: dbg_sym,
            ic_bases: Some(bases),
            from: EntryFrom::Native,
            unwind: false,
        },
        &mut ret,
    );
    native_feedback(heap, arm, base, nslots, &exit);
    // A value — nearly every element — is finished here: `finish_nested`'s DONE is
    // exactly this truncate, and calling it per element cost more than the step itself.
    if exit.outcome == crate::jit::outcome::DONE {
        crate::perf_bump!(jit_link_done);
        heap.truncate_roots(base);
        return Some(Ok(ret));
    }
    if exit.outcome == crate::jit::outcome::DEOPT {
        crate::perf_bump!(hof_native_deopt);
    }
    let frame = NestedFrame {
        stage_base: base,
        base,
        nslots,
        argc,
        native_depth,
        unwind: false,
    };
    let env = exit.env;
    Some(
        match finish_nested(heap, exit.outcome, ret, &frame, false, |_| {
            Some((ArmHandle::new(arm.clone()), env, true))
        }) {
            Nested::Done(v) => Ok(v),
            Nested::Raised | Nested::Unresolved => Err(jit_take_error(heap)
                .unwrap_or_else(|| LispError::type_err("a native step raised no error"))),
        },
    )
}
