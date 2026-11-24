//! Promotion — copying code and data from LOCAL into the shared RUNTIME region
//! (child of heap).
//!
//! What makes a value visible to a runtime's other processes. A `def` promotes the code it
//! binds and `spawn` promotes its target function, so both are readable from every inner
//! process; atoms and already-shared (PRELUDE/RUNTIME) values are returned unchanged. The
//! region is **append-only** — a redefinition adds a version while in-flight calls keep
//! running the old one — which is why an unbounded per-operation promote is a leak of
//! shared code rather than garbage (`BROOD_TRACE_PROMOTE` names the sites).
//!
//! [`PromoteForward`] rides along: the per-promotion forwarding tables that collapse
//! shared structure to one copy and terminate cycles, keyed on canonical handle identity.
//! They live here rather than with the freeze because promotion is their only user.

use super::*;
use crate::process::message::{ClosureMsg, Message};

impl Heap {
    // ----- promotion: copy code from LOCAL into the shared RUNTIME region -----

    /// Deep-copy a value's reachable structure from the local heap into the
    /// shared RUNTIME region, returning a handle valid in every inner process.
    /// `def` of a global runs this so the bound code/data is shareable;
    /// `spawn` runs it on the target function. Atoms and already-shared values
    /// (PRELUDE/RUNTIME) are returned unchanged — no copy.
    ///
    /// Appends only (never mutates existing shared code), so a redefinition adds
    /// a new version while in-flight calls keep running the old one.
    pub fn promote(&self, v: Value) -> Value {
        // Hold the promote⇄age read lock for the whole (recursive) promotion so a
        // concurrent `age_runtime` on another process can't flip `current_gen` between
        // this promote's slot reservation and its fill (ADR-091). Uncontended off the
        // multi-generation path. Acquired once at the top — the recursion must not
        // re-acquire (std `RwLock` read isn't reentrant against a queued writer).
        let _promote_guard = self
            .runtime
            .promote_lock
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut fwd = PromoteForward::default();
        self.promote_in(v, &mut fwd)
    }

    /// [`promote`](Self::promote) for a value about to become reachable from a global — a
    /// `def`, a registry write, a staged module binding. A table it meets is **pinned**:
    /// any process may read the global, so the table no longer dies with the process that
    /// created it (`core::table`, "Lifetime"). A plain `promote` (a `spawn`ed closure)
    /// only marks the table shared.
    ///
    /// Values already in a shared region are not walked, so a table inside a container a
    /// `spawn` promoted earlier is not pinned by a later `def` of that same container;
    /// a table bound directly, or inside anything built since, is.
    pub fn promote_global(&self, v: Value) -> Value {
        let _promote_guard = self
            .runtime
            .promote_lock
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut fwd = PromoteForward {
            pin_tables: true,
            ..PromoteForward::default()
        };
        self.promote_in(v, &mut fwd)
    }

    /// [`promote`](Self::promote) + [`rehome_to_current_locked`] + a caller-supplied
    /// **publish** step, all under ONE `promote_lock` read guard — the atomic
    /// "share this value and bind it" used by every shared root (`globals`,
    /// `declared_sigs`).
    ///
    /// **Why the guard must span the publish.** Re-homing validates "this handle is in
    /// the current generation" and the store makes that handle a *shared GC root*. Split
    /// across two lock acquisitions those two steps are a TOCTOU: an aging flip plus
    /// `migrate_live_globals` can complete in the gap, and the store then lands on a
    /// generation that is already draining, after the migration snapshotted the table.
    /// Both outcomes are live bugs:
    ///
    /// - a **new** name's binding pins the aged-out generation through a root the drain
    ///   deliberately stops probing (post-aging the per-process probes skip the shared
    ///   roots, on the invariant this guard restores), so the generation is freed and the
    ///   global is left dangling; and
    /// - a **rebind** landing between migration's snapshot and its reconcile is
    ///   indistinguishable from a stale binding to `value_in_gen(cur, old_gen)`, so the
    ///   reconcile overwrites it with the migrated copy of the *old* value — a `def` that
    ///   silently reverts.
    ///
    /// Holding one read guard across both closes the window: `age_runtime` takes the
    /// write lock, so it cannot flip `current_gen` while any publish is in flight. This
    /// is the same shape as the race [`Heap::free_runtime_gen`] documents closing with
    /// the aging gate. Lock order is `promote_lock` → the published table's lock, and
    /// nothing takes them the other way round.
    ///
    /// Uncontended (one `RwLock` read) off the multi-generation path.
    pub(crate) fn promote_rehome_publish<R>(
        &self,
        v: Value,
        publish: impl FnOnce(&Heap, Value) -> R,
    ) -> R {
        // Acquired ONCE at the top — `promote_in` must not re-acquire (std `RwLock`
        // read isn't reentrant against a queued writer), which is why this inlines
        // `promote`'s body rather than calling it.
        let _promote_guard = self
            .runtime
            .promote_lock
            .read()
            .unwrap_or_else(|e| e.into_inner());
        // Every caller publishes to a global root, so tables are pinned — see
        // `promote_global`.
        let mut fwd = PromoteForward {
            pin_tables: true,
            ..PromoteForward::default()
        };
        let promoted = self.promote_in(v, &mut fwd);
        let shared = self.rehome_to_current_locked(promoted);
        publish(self, shared)
    }

    /// The recursive core of [`promote`](Self::promote), threading the
    /// [`PromoteForward`] tables so a *cyclic* graph (a closure capturing its own
    /// binding scope) terminates — closures and envs reserve their RUNTIME slot
    /// and register it in `fwd` *before* recursing, so the back-edge resolves to
    /// the reserved handle instead of recursing forever — and so shared (DAG)
    /// substructure of every forwarded kind (closures, envs, pairs, vectors,
    /// maps, strings) collapses to ONE RUNTIME copy instead of one per referrer
    /// (KI-95).
    fn promote_in(&self, v: Value, fwd: &mut PromoteForward) -> Value {
        // Deep-car-nesting guard — see `WALKER_RED_ZONE`.
        crate::stack::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
            self.promote_in_grown(v, fwd)
        })
    }

    fn promote_in_grown(&self, v: Value, fwd: &mut PromoteForward) -> Value {
        match v.unpack() {
            ValueRef::Str(id) if id.region() == LOCAL => {
                if let Some(&nid) = fwd.strings.get(&id) {
                    return Value::str_(nid);
                }
                let s = self.string(id).to_string();
                let nid = self.runtime.push_str(s);
                fwd.strings.insert(id, nid);
                Value::str_(nid)
            }
            ValueRef::BigInt(id) if id.region() == LOCAL => {
                // A leaf: clone the value into the shared region (no children).
                let n = self.bigint(id).clone();
                Value::bigint(self.runtime.push_bigint(n))
            }
            ValueRef::Decimal(id) if id.region() == LOCAL => {
                // A leaf: clone the value into the shared region (no children).
                let n = self.decimal(id).clone();
                Value::decimal(self.runtime.push_decimal(n))
            }
            ValueRef::Ratio(id) if id.region() == LOCAL => {
                // A leaf: clone the value into the shared region (no children).
                let n = self.ratio(id).clone();
                Value::ratio(self.runtime.push_ratio(n))
            }
            ValueRef::Bytes(id) if id.region() == LOCAL => {
                // A leaf: share the Arc<SharedBlob> into the shared region byte-clean —
                // never through the UTF-8 string path. Just an Arc bump.
                let b = Arc::clone(self.bytes(id));
                Value::bytes(self.runtime.push_bytes(b))
            }
            ValueRef::Rope(id) if id.region() == LOCAL => {
                // Cheap `Arc`-node clone into the shared region; the rope is
                // immutable, so sibling processes read it concurrently.
                let r = self.rope(id).clone();
                Value::rope(self.runtime.push_rope(r))
            }
            ValueRef::Pair(id) if id.region() == LOCAL => self.promote_list(id, fwd),
            ValueRef::Vector(id) if id.region() == LOCAL => {
                Value::vector(self.promote_vec_store(id, fwd, /*promote_items*/ true))
            }
            // A range's backing `[lo hi step]` vector holds only ints (atoms) —
            // copy it across and keep the `Range` wrapper. (Item promotion is a
            // no-op on atoms, so it shares the vector table harmlessly.)
            ValueRef::Range(id) if id.region() == LOCAL => {
                Value::range(self.promote_vec_store(id, fwd, /*promote_items*/ false))
            }
            // A seq-view's backing `[source xform]` holds heap values (a
            // collection and a transducer closure), so promote each across like a
            // vector and keep the `SeqView` wrapper.
            ValueRef::SeqView(id) if id.region() == LOCAL => {
                Value::seqview(self.promote_vec_store(id, fwd, /*promote_items*/ true))
            }
            // Map, set and failure share one CHAMP store, so one arm promotes all
            // three: recursively promote the trie depth-first — children before their
            // parent, so the parent's `children` array can be wired to the freshly
            // allocated RUNTIME sub-node handles — then re-wrap in the kind that came
            // in. `fwd` is shared, so KI-95's DAG forwarding applies to all three.
            ValueRef::Map(id)
            | ValueRef::Set(id)
            | ValueRef::Failure(id)
            | ValueRef::Record { fields: id, .. }
                if id.region() == LOCAL =>
            {
                crate::core::value::champ_rewrap(v, self.promote_map_node(id, fwd))
            }
            ValueRef::Fn(id) if id.region() == LOCAL => Value::func(self.promote_closure(id, fwd)),
            ValueRef::Macro(id) if id.region() == LOCAL => {
                Value::macro_(self.promote_closure(id, fwd))
            }
            // A table is a scalar handle — nothing to copy — but it is leaving its
            // creating process, and under a global it outlives it.
            ValueRef::Table(id) => {
                crate::core::table::note_shared(id, fwd.pin_tables);
                v
            }
            // Atoms and PRELUDE values need no copy, and neither does a RUNTIME value of the
            // current generation. A RUNTIME value of an OLDER generation does: it is the
            // boundary where the new structure meets existing shared structure (the tail of
            // `(cons x xs)`, a captured value), and left as it is the promoted copy would
            // point into a generation that is draining. A `def` publishes that copy to the
            // globals, which the drain stopped probing once their own values moved, so the
            // generation was freed under it — `make gcstress`: `(def units (cons unit units))`
            // with `units` read before an aging flip and consed after it.
            _ => self.rehome_to_current_locked(v),
        }
    }

    /// Promote a LOCAL vector-backed store (vector / range / seq-view — they
    /// share the `VecId` slab) into RUNTIME, forwarding through `fwd.vectors` so
    /// a store referenced from several places is copied once (KI-95). The three
    /// wrappers re-tag the forwarded handle, which is sound because handle
    /// equality means "same store".
    fn promote_vec_store(&self, id: VecId, fwd: &mut PromoteForward, promote_items: bool) -> VecId {
        if let Some(&nid) = fwd.vectors.get(&id) {
            return nid;
        }
        let mut items = self.vector(id).to_vec();
        if promote_items {
            for x in items.iter_mut() {
                *x = self.promote_in(*x, fwd);
            }
        }
        let nid = self.runtime.push_vec(VecStore::from_vec(items));
        fwd.vectors.insert(id, nid);
        nid
    }

    /// Promote a local cons-chain. Walks the `cdr` spine *iteratively* so a long
    /// list doesn't recurse its length deep (which overflowed the native stack);
    /// recursion is bounded by element nesting via `promote_in` on each `car`.
    /// Stops at the first already-shared cell, already-*copied* cell
    /// (`fwd.pairs` — the KI-95 DAG collapse; a shared tail resolves to its one
    /// RUNTIME copy) or non-pair tail, preserving both improper (dotted) lists
    /// and existing structure sharing.
    fn promote_list(&self, first: PairId, fwd: &mut PromoteForward) -> Value {
        // Keep each source LOCAL pair id alongside its promoted head, so the new
        // RUNTIME pair can inherit the source position (`form_pos`) the reader stamped
        // on it — without this, `(form-pos …)` on a frozen body returns nil and a
        // position is lost across a cross-node send.
        let mut nodes: Vec<(PairId, Value)> = Vec::new();
        let mut cur = Value::pair(first);
        let tail = loop {
            match cur.unpack() {
                ValueRef::Pair(id) if id.region() == LOCAL => {
                    // Already copied on this walk (a shared cell/tail, or the
                    // whole list re-referenced) — reuse its one RUNTIME copy.
                    if let Some(&copied) = fwd.pairs.get(&id) {
                        break Value::pair(copied);
                    }
                    let (head, next) = self.pair(id);
                    let promoted_head = self.promote_in(head, fwd);
                    nodes.push((id, promoted_head));
                    cur = next;
                }
                other => break self.promote_in(other, fwd),
            }
        };
        let mut acc = tail;
        // Register the spine in `fwd.pairs` so a later walk reaching any of these
        // cells (a shared tail, the whole list re-referenced) reuses this copy
        // (KI-95). Registration is per-cell only for SMALL spines: on a bulk
        // `def` of a long list the map insert is the dominant promote cost,
        // so a long spine registers every
        // `SPINE_REG_STRIDE`-th cell instead. A re-entering walk then re-copies
        // at most `stride − 1` cells before hitting a registered one — per
        // *referrer*, so total growth stays O(n) and the exponential class
        // KI-95 closed stays closed. Index 0 (the spine head — the common
        // `(list a a)` sharing shape) always registers, whatever the stride.
        const SPINE_REG_FULL: usize = 64;
        const SPINE_REG_STRIDE: usize = 8;
        let stride = if nodes.len() <= SPINE_REG_FULL {
            1
        } else {
            SPINE_REG_STRIDE
        };
        // One rehash for the whole spine (a 375k-cell def otherwise pays ~20
        // incremental rehash rounds).
        fwd.pairs.reserve(nodes.len() / stride + 1);
        // One read of the current generation for the whole spine: the caller holds the
        // `promote_lock` read guard, so aging cannot flip it underneath us — and the
        // position key must name the same generation as the handle we mint.
        // The generation's slabs are loaded once too, under the same guarantee — not one
        // `arc-swap` guard acquisition per cons cell.
        let cur_gen = self.runtime.cur_gen();
        let code = self.runtime.cur_code();
        for (i, (src, head)) in nodes.into_iter().enumerate().rev() {
            let idx = code.pairs.push((head, acc));
            if let Some(entry) = self
                .cold()
                .and_then(|c| c.form_pos.get(&form_pos_key(src)).cloned())
            {
                self.runtime.set_position(idx, cur_gen, entry);
            }
            let promoted = PairId::runtime_gen(idx, cur_gen);
            acc = Value::pair(promoted);
            if i % stride == 0 {
                fwd.pairs.insert(src, promoted);
            }
        }
        acc
    }

    /// Promote a LOCAL CHAMP trie into the shared RUNTIME region. Walks
    /// depth-first: child sub-nodes are promoted before their parent so
    /// the parent's `children` array references the new RUNTIME handles.
    /// Every `(k, v)` entry is promoted recursively (matches `promote_in`
    /// on vectors / lists). The result is a brand-new trie in RUNTIME;
    /// the original LOCAL trie is left untouched (it'll be GC'd when its
    /// last reference goes).
    fn promote_map_node(&self, id: MapId, fwd: &mut PromoteForward) -> MapId {
        // Already copied on this walk (a shared sub-trie — path copying makes
        // these routinely) — reuse its one RUNTIME copy (KI-95).
        if let Some(&nid) = fwd.maps.get(&id) {
            return nid;
        }
        let node = self.map_node(id);
        // Promote children first (bottom-up) so the new RUNTIME node can
        // be built with the new child handles in one push.
        let new_children: SmallVec<[MapId; 4]> = node
            .children
            .iter()
            .map(|&c| match c.region() {
                LOCAL => self.promote_map_node(c, fwd),
                // Already shared: kept, unless it is in an older generation (see the
                // `promote_in_grown` fallback).
                _ => self.rehome_map_node_to_current_locked(c),
            })
            .collect();
        let new_data: SmallVec<[(Value, Value); 4]> = node
            .data
            .iter()
            .map(|&(k, v)| (self.promote_in(k, fwd), self.promote_in(v, fwd)))
            .collect();
        let promoted = MapNode {
            size: node.size,
            data_map: node.data_map,
            node_map: node.node_map,
            is_collision: node.is_collision,
            data: new_data,
            children: new_children,
        };
        let nid = MapId::runtime_gen(
            self.runtime.cur_code().maps.push(promoted),
            self.runtime.cur_gen(),
        );
        fwd.maps.insert(id, nid);
        nid
    }

    fn promote_closure(&self, id: ClosureId, fwd: &mut PromoteForward) -> ClosureId {
        // Already promoted on this walk? Return the shared handle (cycle break +
        // DAG-sharing collapse). Keyed on the handle's canonical identity.
        if let Some(&existing) = fwd.closures.get(&id) {
            return existing;
        }
        // Reserve the RUNTIME slot *first* and register it, so a reference back to
        // this closure reached while promoting its captured scope resolves here
        // rather than recursing forever (e.g. `(let (g (fn () g)) g)`).
        // `BROOD_TRACE_PROMOTE=1` — name every closure entering the append-only RUNTIME
        // region, with the Rust frames that put it there — the way to find a site that
        // promotes per operation (an unbounded leak of shared code; KI-22). Gated, so it
        // costs a single `var_os` on the promote path when off.
        if std::env::var_os("BROOD_TRACE_PROMOTE").is_some() {
            let nm = self
                .closure(id)
                .name
                .map(crate::core::value::symbol_name)
                .unwrap_or_else(|| "<anon>".to_string());
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let frame = bt
                .lines()
                .filter(|l| l.contains("brood::"))
                .map(|l| l.trim())
                .filter(|l| !l.contains("promote"))
                .take(3)
                .collect::<Vec<_>>()
                .join(" <- ");
            // The capture state is the diagnostic that matters: a closure promoted with
            // `captures-frame` is one the const-closure cache could not dedupe, so it is
            // being appended per activation rather than once.
            let cap = if self.closure(id).env.is_some() {
                "captures-frame"
            } else {
                "capture-free"
            };
            eprintln!("[promote] closure {} [{}] :: {}", nm, cap, frame);
        }
        let new_idx = self.runtime.cur_code().closures.push(OnceLock::new());
        // The RUNTIME closure count just grew — arm the eval safepoint's `rt_gc_due`
        // probe (see `rt_dirty`). This is the one place closures enter the region.
        self.runtime.rt_dirty.store(true, Ordering::Relaxed);
        let runtime_id = ClosureId::runtime_gen(new_idx, self.runtime.cur_gen());
        fwd.closures.insert(id, runtime_id);
        let cl = self.closure(id).clone();
        // Promote every arm's body forms and `&optional` defaults into the shared
        // region (param symbols and `&` rest are interned/copy, so they ride along).
        let arms = cl
            .arms
            .iter()
            .map(|arm| ClosureArm {
                params: arm.params.clone(),
                optionals: arm
                    .optionals
                    .iter()
                    .map(|&(s, d)| (s, self.promote_in(d, fwd)))
                    .collect(),
                rest: arm.rest,
                body: arm.body.iter().map(|&f| self.promote_in(f, fwd)).collect(),
                // The forwarding head is an interned symbol and the map is plain
                // indices, so the analysis is region-independent — copy it verbatim.
                passthrough: arm.passthrough.clone(),
            })
            .collect();
        // A top-level closure captures the global env (`None`) and is fully
        // shareable as-is. A closure that captured a *local* scope has its scope
        // promoted too, so it resolves its free variables in any process.
        let env = cl.env.map(|e| self.promote_env(e, fwd));
        let promoted = Closure {
            name: cl.name,
            arms,
            doc: cl.doc,
            env,
            module: cl.module,
            identity: cl.identity,
        };
        // Fill the reserved slot exactly once. The handle isn't published (bound
        // in a global / shipped to a process) until `promote` returns, so nothing
        // can observe the cell before this set.
        self.runtime
            .cur_code()
            .closures
            .get(new_idx)
            .expect("reserved closure slot")
            .set(promoted)
            .ok()
            .expect("promote: closure slot filled exactly once");
        runtime_id
    }

    /// Deep-copy an environment frame chain from LOCAL into the shared RUNTIME
    /// region, promoting each bound value. Stops at the global scope (the shared
    /// sentinel). Already-shared (RUNTIME) frames are kept, an older generation's re-homed
    /// into the current one (see the `promote_in_grown` fallback). Reserves its slot before
    /// recursing (same cycle break as [`promote_closure`]).
    fn promote_env(&self, env: EnvId, fwd: &mut PromoteForward) -> EnvId {
        if env == EnvId::GLOBAL {
            return env;
        }
        if env.region() == RUNTIME {
            return self.rehome_env_to_current_locked(env);
        }
        if let Some(&existing) = fwd.envs.get(&env) {
            return existing;
        }
        let new_idx = self.runtime.cur_code().envs.push(OnceLock::new());
        let runtime_id = EnvId::runtime_gen(new_idx, self.runtime.cur_gen());
        fwd.envs.insert(env, runtime_id);
        // Snapshot the frame, then promote its parent and values (no borrow held).
        let (parent, bindings): (Option<EnvId>, Vec<(Symbol, Value)>) = {
            let frame = self.env_frame(env);
            (
                frame.parent,
                frame.vars.iter().map(|&(s, v)| (s, v)).collect(),
            )
        };
        let parent = parent.map(|p| self.promote_env(p, fwd));
        let vars = bindings
            .into_iter()
            .map(|(s, v)| (s, self.promote_in(v, fwd)))
            .collect();
        self.runtime
            .cur_code()
            .envs
            .get(new_idx)
            .expect("reserved env slot")
            .set(EnvFrame { vars, parent })
            .ok()
            .expect("promote: env slot filled exactly once");
        runtime_id
    }

    // ----- building a startup-image value straight into RUNTIME (ADR-771) -----

    /// Rebuild an imaged `message` **directly in the shared RUNTIME region**: what
    /// `from_message` and then the promote a `def` runs would produce, without the LOCAL
    /// copy in between. Materialising a section bound every value it rebuilt, so each was
    /// built twice — once in LOCAL, once more by the promote — and the LOCAL one was
    /// garbage the moment it was bound: a third of an imaged application's start.
    ///
    /// **Why this is as safe as `promote`.** Every RUNTIME slot is minted under the
    /// `promote_lock` read guard, as `promote` mints them, so aging cannot flip the
    /// generation between a slot and the handle naming it. The value is published by the
    /// caller afterwards (`env_define`, whose promote passes a RUNTIME value through and
    /// whose re-home moves one a flip left behind), with no safepoint in between: this
    /// process cannot ack a drain while it holds the unbound value, so the generation it
    /// was built in cannot be freed under it. That is the window `env_define` already
    /// leaves when it stages a load's bindings. The guard is NOT held into the publish —
    /// `promote_lock` is not reentrant against a queued writer, and the facts a define
    /// replays take `registry_lock`, which orders before it.
    ///
    /// Lists, vectors, strings, byte blobs and closures are minted in place, every list
    /// position stamped under one lock. A map, set, failure or record has its children
    /// built here and its trie built LOCAL around them, then copied node by node (the
    /// children pass through). The rare rest — a big number, a closure carrying module
    /// guards or shared arms, a handle — is rebuilt LOCAL and promoted, as before.
    pub(crate) fn build_shared(&mut self, message: &Message) -> Value {
        let runtime = Arc::clone(&self.runtime);
        let _promote_guard = runtime
            .promote_lock
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut build = SharedBuild {
            file: self.cold().and_then(|cold| cold.current_file_arc.clone()),
            positions: Vec::new(),
            forward: PromoteForward {
                pin_tables: true,
                ..PromoteForward::default()
            },
        };
        let value = self.build_shared_in(message, &mut build);
        runtime.set_positions(build.positions);
        value
    }

    fn build_shared_in(&mut self, message: &Message, build: &mut SharedBuild) -> Value {
        // Deep-nesting guard — see `WALKER_RED_ZONE`.
        crate::stack::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
            self.build_shared_grown(message, build)
        })
    }

    fn build_shared_grown(&mut self, message: &Message, build: &mut SharedBuild) -> Value {
        match message {
            Message::Nil => Value::nil(),
            Message::Bool(flag) => Value::boolean(*flag),
            Message::Int(number) => Value::int(*number),
            Message::Float(number) => Value::float(*number),
            Message::Sym(symbol) => Value::symbol(*symbol),
            Message::Keyword(symbol) => Value::keyword(*symbol),
            Message::Str(text) => Value::str_(self.runtime.push_str(text.clone())),
            Message::Bytes(blob) => Value::bytes(self.runtime.push_bytes(Arc::clone(blob))),
            // An empty list rebuilds as nil, as `from_message`'s `heap.list` does.
            Message::List(items, _) if items.is_empty() => Value::nil(),
            Message::List(items, position) => {
                let heads = self.build_shared_items(items, build);
                let (list, head) = self.push_shared_spine(heads, Value::nil());
                if let (Some(pos), Some((index, code_gen))) = (position, head) {
                    build.positions.push((
                        rt_pos_key(index, code_gen),
                        FormPos {
                            pos: *pos,
                            file: build.file.clone(),
                            synthetic: false,
                        },
                    ));
                }
                list
            }
            Message::Dotted(items) => {
                let mut heads = self.build_shared_items(items, build);
                let tail = heads.pop().unwrap_or(Value::nil());
                self.push_shared_spine(heads, tail).0
            }
            Message::Vector(items) => {
                let values = self.build_shared_items(items, build);
                Value::vector(self.runtime.push_vec(VecStore::from_vec(values)))
            }
            Message::FloatVector(numbers) => {
                let values = numbers.iter().map(|number| Value::float(*number)).collect();
                Value::vector(self.runtime.push_vec(VecStore::from_vec(values)))
            }
            Message::Map(entries) => {
                let pairs = self.build_shared_entries(entries, build);
                let local = self.map_from_pairs(pairs);
                self.promote_in(local, &mut build.forward)
            }
            Message::Failure(entries) => {
                let pairs = self.build_shared_entries(entries, build);
                let local = match self.map_from_pairs(pairs) {
                    Value::Map(id) => Value::failure(id),
                    other => other,
                };
                self.promote_in(local, &mut build.forward)
            }
            Message::Record(record, entries) => {
                let pairs = self.build_shared_entries(entries, build);
                let local = match self.map_from_pairs(pairs) {
                    Value::Map(id) => Value::record(*record, id),
                    other => other,
                };
                self.promote_in(local, &mut build.forward)
            }
            Message::Set(items) => {
                let elements = self.build_shared_items(items, build);
                let local = self.set_from_elems(elements);
                self.promote_in(local, &mut build.forward)
            }
            Message::Closure(closure)
                if closure.shared_arms.is_none()
                    && closure
                        .modules
                        .iter()
                        .all(|need| self.env_get(EnvId::GLOBAL, need.probe).is_some()) =>
            {
                self.build_shared_closure(closure, build)
            }
            _ => {
                let local = crate::process::message::from_message(self, message);
                self.promote_in(local, &mut build.forward)
            }
        }
    }

    fn build_shared_items(&mut self, items: &[Message], build: &mut SharedBuild) -> Vec<Value> {
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            values.push(self.build_shared_in(item, build));
        }
        values
    }

    fn build_shared_entries(
        &mut self,
        entries: &[(Message, Message)],
        build: &mut SharedBuild,
    ) -> Vec<(Value, Value)> {
        let mut pairs = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let key = self.build_shared_in(key, build);
            let value = self.build_shared_in(value, build);
            pairs.push((key, value));
        }
        pairs
    }

    /// Cons `heads` onto `tail` in the current generation, last first, and answer the list
    /// with its head cell's slab index and generation (none for no heads).
    fn push_shared_spine(&self, heads: Vec<Value>, tail: Value) -> (Value, Option<(usize, usize)>) {
        // One generation read and one slab load for the spine: the caller holds the
        // `promote_lock` read guard, so aging cannot flip it underneath us.
        let code_gen = self.runtime.cur_gen();
        let code = self.runtime.cur_code();
        let mut list = tail;
        let mut head = None;
        for item in heads.into_iter().rev() {
            let index = code.pairs.push((item, list));
            list = Value::pair(PairId::runtime_gen(index, code_gen));
            head = Some((index, code_gen));
        }
        (list, head)
    }

    fn build_shared_closure(&mut self, closure: &ClosureMsg, build: &mut SharedBuild) -> Value {
        let mut arms: Vec<ClosureArm> = Vec::with_capacity(closure.arms.len());
        for arm in &closure.arms {
            let mut optionals = Vec::with_capacity(arm.optionals.len());
            for (name, default) in &arm.optionals {
                optionals.push((*name, self.build_shared_in(default, build)));
            }
            let body = self.build_shared_items(&arm.body, build);
            let mut built = ClosureArm {
                params: arm.params.clone(),
                optionals,
                rest: arm.rest,
                body,
                passthrough: None,
            };
            // What `alloc_closure` derives for every closure it allocates.
            built.passthrough = self.compute_passthrough(&built);
            arms.push(built);
        }
        // The captured free variables, as one frame over the global scope — the frame
        // `from_message` builds, minted in place. A message is a tree, so nothing in it
        // can reach back to this closure: no slot need be reserved before the build.
        let env = if closure.captured.is_empty() {
            None
        } else {
            let mut vars = Vec::with_capacity(closure.captured.len());
            for (name, value) in &closure.captured {
                vars.push((*name, self.build_shared_in(value, build)));
            }
            let code_gen = self.runtime.cur_gen();
            let code = self.runtime.cur_code();
            let index = code.envs.push(OnceLock::new());
            code.envs
                .get(index)
                .expect("reserved env slot")
                .set(EnvFrame {
                    vars: vars.into_iter().collect(),
                    parent: Some(EnvId::GLOBAL),
                })
                .ok()
                .expect("build_shared: env slot filled exactly once");
            Some(EnvId::runtime_gen(index, code_gen))
        };
        let code_gen = self.runtime.cur_gen();
        let code = self.runtime.cur_code();
        let index = code.closures.push(OnceLock::new());
        // The RUNTIME closure count just grew — arm the eval safepoint's `rt_gc_due` probe,
        // as `promote_closure` does.
        self.runtime.rt_dirty.store(true, Ordering::Relaxed);
        code.closures
            .get(index)
            .expect("reserved closure slot")
            .set(Closure {
                name: closure.name,
                arms: arms.into(),
                doc: closure.doc.clone(),
                env,
                module: closure.module,
                // A copy across heaps is a new cell — never `=` to its source.
                identity: crate::core::value::fresh_closure_identity(),
            })
            .ok()
            .expect("build_shared: closure slot filled exactly once");
        Value::func(ClosureId::runtime_gen(index, code_gen))
    }
}

/// What one [`Heap::build_shared`] carries down its walk: the file its lists' positions
/// are stamped with, the positions to install under one lock at the end, and the forwarding
/// tables for the parts that go through `promote_in`.
struct SharedBuild {
    file: Option<Arc<str>>,
    positions: Vec<(u64, FormPos)>,
    forward: PromoteForward,
}

/// Forwarding tables for [`Heap::promote`]: LOCAL handle → the RUNTIME handle it
/// was promoted to. Closures/envs can form a *cycle* (a closure capturing its own
/// binding scope), so they reserve-then-register BEFORE recursing — the back-edge
/// resolves to the reserved handle. Pairs/vectors/maps/strings are acyclic by
/// construction (immutable, built bottom-up) but **not trees**: path-copying code
/// routinely produces DAGs, and without forwarding each shared node was re-copied
/// once per referrer into the append-only RUNTIME region — exponentially with
/// nesting (KI-95; the GC's flush path always forwarded these, `gc.rs`
/// `flush_pair`/`flush_vector`/`flush_map`). They register AFTER copying (no cycle
/// to break), collapsing every DAG to one RUNTIME copy.
///
/// Keys are the handles themselves: their `Eq`/`Hash` use the canonical identity
/// (region + index + the LOCAL nursery/old AGE bit), so an old-gen and a
/// nursery handle at the same slab index — distinct objects — cannot collide the
/// way the previous bare-`index()` keys could. The maps are lazy (an empty
/// `HashMap` doesn't allocate), so the dominant small no-sharing promote pays
/// only the lookups.
#[derive(Default)]
struct PromoteForward {
    closures: HandleMap<ClosureId, ClosureId>,
    envs: HandleMap<EnvId, EnvId>,
    /// Source pair cell → its promoted cell (tail included — `promote_list`
    /// registers every cell of a spine it builds). The id, not a `Value`: a
    /// bulk `def` of a long list registers every cell, so entry size is table
    /// cache-miss rate.
    pairs: HandleMap<PairId, PairId>,
    vectors: HandleMap<VecId, VecId>,
    maps: HandleMap<MapId, MapId>,
    strings: HandleMap<StrId, StrId>,
    /// The promoted value is bound to a global: pin the tables it reaches.
    pin_tables: bool,
}

/// Multiplicative hasher for [`PromoteForward`]'s handle keys. Promote registers
/// every node it copies, so the map hash is a per-node cost on the `def`/`spawn`
/// path — and the key is a single canonical-handle `u64` (one `write_u64`), for
/// which the default SipHash is pure tax. A Fibonacci multiply spreads the low slab-index bits into the
/// high bits hashbrown's control bytes read. Same pattern as `table.rs`'s
/// `IdentityHasher`, which strips the identical round from table ops.
#[derive(Default)]
struct HandleHasher(u64);
impl std::hash::Hasher for HandleHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Handle keys always arrive as one `write_u64`; this keeps the impl total.
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}
type HandleMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<HandleHasher>>;

#[cfg(test)]
mod promote_sharing_tests {
    use super::*;

    /// Level 0 is `(1)`; level k is a pair whose car AND cdr are level k-1:
    /// n+1 distinct cells, but 2^(n+1)-1 if re-copied once per referrer.
    #[test]
    fn a_self_sharing_pair_dag_promotes_linearly() {
        let mut h = Heap::new();
        let n = 16;
        let mut v = h.alloc_pair(Value::int(1), Value::nil());
        for _ in 0..n {
            v = h.alloc_pair(v, v);
        }
        let before = h.runtime.cur_code().pairs.count();
        let promoted = h.promote(v);
        let grown = h.runtime.cur_code().pairs.count() - before;
        assert_eq!(
            grown,
            n + 1,
            "each distinct cell must be promoted exactly once"
        );
        // The promoted graph still reads correctly: car^n reaches the `(1)` leaf.
        let mut cur = promoted;
        for _ in 0..n {
            let ValueRef::Pair(id) = cur.unpack() else {
                panic!("expected a pair");
            };
            cur = h.pair(id).0;
        }
        let ValueRef::Pair(id) = cur.unpack() else {
            panic!("expected the leaf pair");
        };
        assert!(matches!(h.pair(id).0.unpack(), ValueRef::Int(1)));
    }

    /// A shared list *tail* rides the same table: two lists converging on one
    /// spine must promote the shared cells once.
    #[test]
    fn a_shared_list_tail_promotes_once() {
        let mut h = Heap::new();
        let mut tail = Value::nil();
        for i in 0..8 {
            tail = h.alloc_pair(Value::int(i), tail);
        }
        let a = h.alloc_pair(Value::int(100), tail);
        let b = h.alloc_pair(Value::int(200), tail);
        let both = h.alloc_pair(a, b);
        let before = h.runtime.cur_code().pairs.count();
        h.promote(both);
        let grown = h.runtime.cur_code().pairs.count() - before;
        assert_eq!(
            grown,
            8 + 3,
            "the 8 shared tail cells + a + b + the outer pair"
        );
    }

    /// Past `SPINE_REG_FULL` a spine registers only every `SPINE_REG_STRIDE`-th
    /// cell, so a walk re-entering it (the shared tail here) may re-copy up to
    /// stride−1 cells per referrer before hitting a registered one — bounded,
    /// still O(n), never the pre-KI-95 once-per-referrer full re-copy.
    #[test]
    fn a_long_shared_tail_stays_linear_past_the_stride_threshold() {
        let mut h = Heap::new();
        let n = 1000;
        let mut tail = Value::nil();
        for i in 0..n {
            tail = h.alloc_pair(Value::int(i), tail);
        }
        let a = h.alloc_pair(Value::int(-1), tail);
        let b = h.alloc_pair(Value::int(-2), tail);
        let both = h.alloc_pair(a, b);
        let before = h.runtime.cur_code().pairs.count();
        h.promote(both);
        let grown = h.runtime.cur_code().pairs.count() - before;
        assert!(
            grown <= n as usize + 3 + 8,
            "grown={grown} for a {n}-cell tail shared twice — a re-entering walk \
             must join the registered copy within one stride window"
        );
    }

    #[test]
    fn a_self_sharing_vector_dag_promotes_linearly() {
        let mut h = Heap::new();
        let n = 12;
        let mut v = h.alloc_vector(vec![Value::int(1)]);
        for _ in 0..n {
            v = h.alloc_vector(vec![v, v]);
        }
        let before = h.runtime.cur_code().vectors.count();
        h.promote(v);
        let grown = h.runtime.cur_code().vectors.count() - before;
        assert_eq!(
            grown,
            n + 1,
            "each distinct vector must be promoted exactly once"
        );
    }

    /// A map referenced twice must land as one RUNTIME trie, not two. The trie's
    /// per-copy node count is measured by a single-reference promote of the same
    /// map, so the assertion doesn't hardcode CHAMP layout.
    #[test]
    fn a_shared_map_promotes_once() {
        let mut h = Heap::new();
        let s = h.alloc_string("shared-once");
        let m = h.map_from_pairs(vec![(Value::int(1), s), (Value::int(2), s)]);
        let unit = {
            let before = h.runtime.cur_code().maps.count();
            h.promote(m);
            h.runtime.cur_code().maps.count() - before
        };
        let twice = h.alloc_vector(vec![m, m]);
        let before_maps = h.runtime.cur_code().maps.count();
        let before_strs = h.runtime.cur_code().strings.count();
        h.promote(twice);
        assert_eq!(
            h.runtime.cur_code().maps.count() - before_maps,
            unit,
            "the map is one value referenced twice — one trie copy"
        );
        assert_eq!(
            h.runtime.cur_code().strings.count() - before_strs,
            1,
            "the string is referenced twice inside the map — one copy"
        );
    }
}

#[cfg(test)]
mod build_shared_tests {
    use super::*;
    use crate::process::message::to_message_image;

    /// ADR-771: an imaged value of every kind a section carries is built straight into the
    /// RUNTIME region — no LOCAL list on the way — equal to the value it was written from,
    /// its closure calling through its captured frame and its forms keeping their positions.
    #[test]
    fn an_imaged_value_is_built_in_the_runtime_region_equal_to_its_source() {
        let mut interp = crate::Interp::new();
        let original = interp
            .eval_str(
                "(def original
                   [(list 1 \"two\" :three 'four) '(1 . 2) [1 [2 #{3}]] {:a {:b \"c\"}}
                    123456789012345678901234567890 1.5
                    (let (base 40) (fn (step) (+ base step)))])",
            )
            .expect("define the source value");
        let original = interp
            .heap
            .env_get(EnvId::GLOBAL, crate::core::value::intern("original"))
            .unwrap_or(original);
        let message = to_message_image(&interp.heap, original).expect("image the value");
        let local_pairs = interp.heap.local.pairs.len();
        let built = interp.heap.build_shared(&message);
        assert_eq!(
            interp.heap.local.pairs.len(),
            local_pairs,
            "a list was built LOCAL on the way"
        );
        let ValueRef::Vector(vector) = built.unpack() else {
            panic!("a vector comes back a vector");
        };
        assert_eq!(vector.region(), RUNTIME);
        let items = interp.heap.vector(vector).to_vec();
        let ValueRef::Fn(closure) = items[6].unpack() else {
            panic!("the closure comes back a closure");
        };
        assert_eq!(closure.region(), RUNTIME);
        let rebuilt = interp.heap.closure(closure).clone();
        assert_eq!(
            rebuilt.env.map(|env| env.region()),
            Some(RUNTIME),
            "its captured frame is shared too"
        );
        assert!(
            interp.heap.form_pos(rebuilt.arms[0].body[0]).is_some(),
            "a body form keeps the position it was read at"
        );
        interp
            .heap
            .env_define(EnvId::GLOBAL, crate::core::value::intern("built"), built);
        let verdict = interp
            .eval_str("[(= (take built 6) (take original 6)) ((nth built 6) 2)]")
            .expect("compare and call");
        assert_eq!(interp.print(verdict), "[true 42]");
    }
}
