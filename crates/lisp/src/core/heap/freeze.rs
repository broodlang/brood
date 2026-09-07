//! The prelude freeze — turning a builder heap into a `SharedCode` region (child of heap).
//!
//! One operation and its helper, run once per runtime at the end of the prelude build:
//! [`Heap::freeze_as_shared_code`] consumes the builder heap, re-tags every handle
//! local→prelude in place, and hands back the frozen PRELUDE region plus the global
//! bindings that seed each runtime's global table. The re-tag is only valid for LOCAL
//! handles, so `localize_for_freeze` deep-copies anything reachable that is not — a
//! prelude global can reach a RUNTIME string through the VM's constant pool, and
//! re-tagging that in place silently aliased an unrelated prelude string (KI-12).
//! Split out of `heap.rs` on 2026-09-07 (handoff item 1, move b).

use super::*;

impl Heap {
    /// Deep-copy `v` into the builder's **LOCAL** slabs if any part of it lives in
    /// another region, returning an all-LOCAL value; already-LOCAL values (and
    /// atoms, symbols, natives) are returned unchanged.
    ///
    /// The freeze turns the builder's slabs into the prelude region by re-tagging
    /// handles in place, which is only valid for LOCAL ones. A prelude global can
    /// nonetheless reach a **RUNTIME** object: the VM interns its constant-pool
    /// literals there so compiled code is shareable, so `(def *load-path* (list "."))`
    /// bound a LOCAL pair whose car was a RUNTIME string. Re-tagging that car kept
    /// its index and changed its region — silently aliasing an unrelated prelude
    /// string (KI-12). Copying first makes the re-tag total.
    ///
    /// `fwd` collapses shared structure to one copy and terminates cycles: a
    /// closure/env reserves nothing here, but a DAG (the same string reached twice)
    /// must not be duplicated per edge. Keyed on the raw handle bits.
    fn localize_for_freeze(&mut self, v: Value, fwd: &mut HashMap<(u8, u32, u8), Value>) -> Value {
        // Bail out fast on the overwhelmingly common case: an atom or an
        // already-LOCAL handle whose children are LOCAL too. Checking "children
        // are LOCAL too" needs the walk, so only the region test is cheap here;
        // the walk below is O(reachable) once per freeze, on the prelude only.
        let key = match handle_key(v) {
            Some(k) => k,
            None => return v, // an atom: nothing to copy
        };
        if let Some(&done) = fwd.get(&key) {
            return done;
        }
        let out = match v.unpack() {
            ValueRef::Str(id) => {
                if id.region() == LOCAL {
                    return v;
                }
                let s = self.string(id).to_string();
                self.alloc_string(&s)
            }
            ValueRef::BigInt(id) => {
                if id.region() == LOCAL {
                    return v;
                }
                let n = self.bigint(id).clone();
                self.alloc_bigint(n)
            }
            ValueRef::Decimal(id) => {
                if id.region() == LOCAL {
                    return v;
                }
                let n = self.decimal(id).clone();
                self.alloc_decimal(n)
            }
            ValueRef::Ratio(id) => {
                if id.region() == LOCAL {
                    return v;
                }
                let n = self.ratio(id).clone();
                self.alloc_ratio(n)
            }
            ValueRef::Bytes(id) => {
                if id.region() == LOCAL {
                    return v;
                }
                let blob = self.bytes(id).clone();
                self.alloc_bytes(blob)
            }
            ValueRef::Pair(id) => {
                let (a, b) = self.pair(id);
                let a2 = self.localize_for_freeze(a, fwd);
                let b2 = self.localize_for_freeze(b, fwd);
                if id.region() == LOCAL
                    && handle_key(a2) == handle_key(a)
                    && handle_key(b2) == handle_key(b)
                {
                    return v;
                }
                self.alloc_pair(a2, b2)
            }
            ValueRef::Vector(id) => {
                let items = self.vector(id).to_vec();
                let mut out = Vec::with_capacity(items.len());
                let mut same = id.region() == LOCAL;
                for it in items {
                    let c = self.localize_for_freeze(it, fwd);
                    same &= handle_key(c) == handle_key(it);
                    out.push(c);
                }
                if same {
                    return v;
                }
                self.alloc_vector(out)
            }
            // A range and a seq-view are both a wrapper around a backing vector (`[lo hi
            // step]` / `[source xform]`), which lives in the *same* slab as an ordinary
            // vector — so they localize identically and keep their wrapper, exactly as
            // `flush_rt_value` treats them. Without these arms a non-LOCAL range or view
            // reaching a prelude global fell through to the `_ => return v` catch-all: the
            // freeze `debug_assert` below catches that in a debug build, but a release
            // build would seed a RUNTIME handle into a PRELUDE binding whose backing
            // runtime is discarded (the KI-12 shape). No prelude form produces one today
            // — as with `Bytes` in `to_prelude`, silence was the wrong default for a
            // region re-tag: every kind is either handled or explicitly guarded.
            ValueRef::Range(id) | ValueRef::SeqView(id) => {
                let items = self.vector(id).to_vec();
                let mut out = Vec::with_capacity(items.len());
                let mut same = id.region() == LOCAL;
                for it in items {
                    let c = self.localize_for_freeze(it, fwd);
                    same &= handle_key(c) == handle_key(it);
                    out.push(c);
                }
                if same {
                    return v;
                }
                let new_id = match self.alloc_vector(out).unpack() {
                    ValueRef::Vector(nid) => nid,
                    _ => unreachable!("alloc_vector returns a vector"),
                };
                if matches!(v.unpack(), ValueRef::Range(_)) {
                    Value::range(new_id)
                } else {
                    Value::seqview(new_id)
                }
            }
            ValueRef::Map(id) => {
                let entries = self.map_entries(id);
                let mut out = Vec::with_capacity(entries.len());
                let mut same = id.region() == LOCAL;
                for (k, val) in entries {
                    let k2 = self.localize_for_freeze(k, fwd);
                    let v2 = self.localize_for_freeze(val, fwd);
                    same &= handle_key(k2) == handle_key(k) && handle_key(v2) == handle_key(val);
                    out.push((k2, v2));
                }
                if same {
                    return v;
                }
                self.map_from_pairs(out)
            }
            ValueRef::Set(id) => {
                let elems = self.set_elems(id);
                let mut out = Vec::with_capacity(elems.len());
                let mut same = id.region() == LOCAL;
                for e in elems {
                    let c = self.localize_for_freeze(e, fwd);
                    same &= handle_key(c) == handle_key(e);
                    out.push(c);
                }
                if same {
                    return v;
                }
                self.set_from_elems(out)
            }
            // A closure reached from a global is the normal case (`defn`), and its
            // arms' body forms can hold VM constants. Copy only when something
            // inside is non-LOCAL, so the usual all-LOCAL closure is untouched.
            ValueRef::Fn(id) | ValueRef::Macro(id) => {
                let mut c = self.closure(id).clone();
                let mut same = id.region() == LOCAL;
                for arm in std::sync::Arc::make_mut(&mut c.arms).iter_mut() {
                    for f in arm.body.iter_mut() {
                        let c2 = self.localize_for_freeze(*f, fwd);
                        same &= handle_key(c2) == handle_key(*f);
                        *f = c2;
                    }
                    for (_, d) in arm.optionals.iter_mut() {
                        let c2 = self.localize_for_freeze(*d, fwd);
                        same &= handle_key(c2) == handle_key(*d);
                        *d = c2;
                    }
                }
                if same {
                    return v;
                }
                let new_id = self.alloc_closure(c);
                if matches!(v.unpack(), ValueRef::Macro(_)) {
                    Value::macro_(new_id)
                } else {
                    Value::func(new_id)
                }
            }
            // Atoms, symbols, natives, and the opaque handles a prelude cannot hold.
            _ => return v,
        };
        fwd.insert(key, out);
        out
    }

    /// Consume this (builder) heap: move everything it allocated into a frozen
    /// [`SharedCode`] (PRELUDE) region — re-tagging every handle local→prelude —
    /// and return that region plus the global env's bindings
    /// (`symbol -> prelude value`) used to seed each runtime's global table.
    ///
    /// Env frames are dropped: shared (top-level) closures capture the global
    /// env symbolically (`env == None`), so nothing references a frame.
    /// GC is disabled in a builder heap (`Heap::new` sets `gc_enabled = false`),
    /// so the slabs have no holes here — indices are dense and stable across
    /// the local→prelude re-tag.
    pub fn freeze_as_shared_code(mut self, root: EnvId) -> (SharedCode, Vec<(Symbol, Value)>) {
        // Pull anything a global reaches into the LOCAL slabs first, so the
        // re-tag below is valid for every handle it touches (KI-12).
        // The build's declared sigs live in its RUNTIME region, which the freeze discards:
        // localize them like the globals so they can ride into the prelude region.
        let mut declared_sigs: Vec<(Symbol, Value)> = self
            .runtime
            .declared_sigs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        // Same fate, same fix: the build's registry-name set is in its runtime too.
        let registry_names: Vec<Symbol> = self
            .runtime
            .registry_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect();
        {
            let mut fwd: HashMap<(u8, u32, u8), Value> = HashMap::new();
            let vars: Vec<(Symbol, Value)> = self.local.envs[root.index()].vars.to_vec();
            for (i, (_, v)) in vars.iter().enumerate() {
                let lv = self.localize_for_freeze(*v, &mut fwd);
                if handle_key(lv) != handle_key(*v) {
                    self.local.envs[root.index()].vars[i].1 = lv;
                }
            }
            for (_, v) in declared_sigs.iter_mut() {
                *v = self.localize_for_freeze(*v, &mut fwd);
            }
        }
        let bindings: Vec<(Symbol, Value)> = self.local.envs[root.index()]
            .vars
            .iter()
            .map(|&(s, v)| {
                // HARD, not `debug_assert`. What it guards is a silent region re-tag: in
                // release, a non-LOCAL handle reaching here is seeded into a PRELUDE
                // binding whose backing runtime is later discarded — reads then resolve
                // in a wiped or unrelated slab (KI-12), with no crash at the store. That
                // is strictly worse than aborting the prelude build, and this runs once
                // per prelude freeze over a few thousand bindings, so it is free.
                // `to_prelude`'s `Rope` arm already takes the same position.
                assert!(
                    matches!(handle_key(v), None | Some((_, _, LOCAL))),
                    "prelude global {} still points outside LOCAL at freeze — \
                     `localize_for_freeze` missed a case (KI-12)",
                    crate::core::value::symbol_name(s),
                );
                (s, to_prelude(v))
            })
            .collect();

        // Mark which closures are REACHABLE from the global bindings. The
        // builder heap never collects (gc disabled — dense, stable indices are
        // what make the local→prelude re-tag a pure bit-flip), so the slabs
        // also hold boot *garbage*: intermediates from macroexpansion and
        // top-level eval. Expander code legitimately creates closures that
        // capture a local frame while it runs (the receive matcher expansion
        // was the first to do so in the prelude — devlog 2026-07-22); dead by
        // freeze time, they must not trip the dangling-env assert below. The
        // assert stays HARD for reachable closures — a live captured frame
        // really would dangle once the env slab is wiped — and dead ones get
        // their env scrubbed instead, which is unobservable (nothing can
        // reach them) and keeps the wiped-env invariant exact.
        let reachable_clo: Vec<bool> = {
            let slabs = &self.local;
            let mut seen_pair = vec![false; slabs.pairs.len()];
            let mut seen_vec = vec![false; slabs.vectors.len()];
            let mut seen_map = vec![false; slabs.maps.len()];
            let mut seen_clo = vec![false; slabs.closures.len()];
            let mut seen_env = vec![false; slabs.envs.len()];
            enum W {
                V(Value),
                E(EnvId),
                M(MapId),
            }
            let mut work: Vec<W> = slabs.envs[root.index()]
                .vars
                .iter()
                .map(|&(_, v)| W::V(v))
                .collect();
            while let Some(w) = work.pop() {
                match w {
                    W::V(v) => match v.unpack() {
                        ValueRef::Pair(id) if id.region() == LOCAL => {
                            if !std::mem::replace(&mut seen_pair[id.index()], true) {
                                let (a, b) = slabs.pairs[id.index()];
                                work.push(W::V(a));
                                work.push(W::V(b));
                            }
                        }
                        // A range's `[lo hi step]` and a seq-view's `[source xform]` live
                        // in the vectors slab like any other vector, and a view's
                        // `source`/`xform` can be — indeed usually is — a closure. Not
                        // descending them left a reachable closure marked dead, and a dead
                        // closure gets its captured env scrubbed at freeze.
                        ValueRef::Vector(id) | ValueRef::Range(id) | ValueRef::SeqView(id)
                            if id.region() == LOCAL =>
                        {
                            if !std::mem::replace(&mut seen_vec[id.index()], true) {
                                for &x in slabs.vectors[id.index()].iter() {
                                    work.push(W::V(x));
                                }
                            }
                        }
                        ValueRef::Map(id) | ValueRef::Set(id) if id.region() == LOCAL => {
                            work.push(W::M(id))
                        }
                        ValueRef::Fn(id) | ValueRef::Macro(id) if id.region() == LOCAL => {
                            if !std::mem::replace(&mut seen_clo[id.index()], true) {
                                let c = &slabs.closures[id.index()];
                                for arm in c.arms.iter() {
                                    for &f in &arm.body {
                                        work.push(W::V(f));
                                    }
                                    for &(_, d) in &arm.optionals {
                                        work.push(W::V(d));
                                    }
                                }
                                if let Some(e) = c.env {
                                    work.push(W::E(e));
                                }
                            }
                        }
                        _ => {}
                    },
                    W::M(id) => {
                        if !std::mem::replace(&mut seen_map[id.index()], true) {
                            let node = &slabs.maps[id.index()];
                            for &(k, v) in node.data.iter() {
                                work.push(W::V(k));
                                work.push(W::V(v));
                            }
                            for &child in node.children.iter() {
                                work.push(W::M(child));
                            }
                        }
                    }
                    W::E(e) => {
                        if !std::mem::replace(&mut seen_env[e.index()], true) {
                            let frame = &slabs.envs[e.index()];
                            for &(_, v) in frame.vars.iter() {
                                work.push(W::V(v));
                            }
                            if let Some(p) = frame.parent {
                                work.push(W::E(p));
                            }
                        }
                    }
                }
            }
            seen_clo
        };

        let mut slabs = self.local;
        debug_assert!(
            slabs.ropes.is_empty(),
            "a Rope cannot appear in the prelude — it is pure Brood with no rope literals",
        );
        // Inline-extract any `Shared` string entries the builder created
        // (~9 prelude docstrings exceed `SHARED_BLOB_THRESHOLD` at the time
        // of writing). PRELUDE is shared `Arc<SharedCode>` across runtimes;
        // `Arc<SharedBlob>` is per-runtime, so leaving them as `Shared` here
        // would entangle their lifetimes. The blob's `Arc` drops as the old
        // `LocalString::Shared` is overwritten — freeing the blob if no other
        // handle remains (none does, at freeze time).
        for entry in slabs.strings.iter_mut() {
            if let StrData::Shared(arc) = &entry.data {
                let bytes: Vec<u8> = arc.as_bytes().to_vec();
                *entry = LocalString::inline(
                    String::from_utf8(bytes).expect("prelude blob is valid UTF-8"),
                );
            }
        }
        for p in &mut slabs.pairs {
            p.0 = to_prelude(p.0);
            p.1 = to_prelude(p.1);
        }
        for vec in &mut slabs.vectors {
            for x in vec.iter_mut() {
                *x = to_prelude(*x);
            }
        }
        for map_node in &mut slabs.maps {
            // Re-tag every (k, v) inside the trie node — child `MapId`s
            // need their region bits flipped to PRELUDE too.
            for (k, v) in map_node.data.iter_mut() {
                *k = to_prelude(*k);
                *v = to_prelude(*v);
            }
            for child in map_node.children.iter_mut() {
                *child = MapId::prelude(child.index());
            }
        }
        let mut scrubbed = 0usize;
        for (i, c) in slabs.closures.iter_mut().enumerate() {
            // Prelude closures are built from LOCAL/PRELUDE `fn_rest` (never cached —
            // the template cache is RUNTIME-keyed), so their arms are unique here and
            // `make_mut` never clones; it's used for robustness, not sharing.
            for arm in std::sync::Arc::make_mut(&mut c.arms).iter_mut() {
                for f in arm.body.iter_mut() {
                    *f = to_prelude(*f);
                }
                for (_, d) in arm.optionals.iter_mut() {
                    *d = to_prelude(*d);
                }
            }
            // A dead boot intermediate (unreachable from the globals) may hold
            // a captured local frame — expander code makes such closures while
            // it runs. Scrub the env: unobservable (nothing reaches it), and
            // the wiped-env invariant below stays exact.
            if !reachable_clo[i] && c.env.is_some() {
                c.env = None;
                scrubbed += 1;
            }
            // Hard assert (not debug_assert!) — `slabs.envs` is wiped below,
            // so a REACHABLE closure capturing a non-None env would survive
            // into the frozen prelude with a dangling env handle, and the
            // first call would silently index past the empty slab. We want the
            // same failure in release: a clear panic at freeze time, not
            // corrupt state at runtime. The message names the closure so the
            // prelude line that produced it is easy to find.
            assert!(
                c.env.is_none(),
                "shared closures must capture the global env (closure {:?} \
                 has env={:?}); the prelude tried to freeze a REACHABLE \
                 closure with a captured local frame — most likely a \
                 `defn`/`def` whose body closes over a let-bound name instead \
                 of a global",
                c.name.map(crate::core::value::symbol_name),
                c.env,
            );
        }
        if scrubbed > 0 && std::env::var_os("BROOD_BOOT_TRACE").is_some() {
            eprintln!("[boot] freeze scrubbed {scrubbed} dead boot-intermediate closure env(s)");
        }
        slabs.envs = Vec::new(); // the prelude region has no env frames

        // Move the def-sites the builder recorded (via `note_definition` while
        // loading the prelude) into the immutable region. They describe prelude
        // globals, never change, and shouldn't be re-recorded per runtime.
        let def_sites = std::mem::take(&mut *self.runtime.def_sites_write());
        let declared_sigs: SymbolMap<Value> = declared_sigs
            .into_iter()
            .map(|(k, v)| {
                assert!(
                    matches!(handle_key(v), None | Some((_, _, LOCAL))),
                    "prelude sig {} still points outside LOCAL at freeze (KI-12)",
                    crate::core::value::symbol_name(k),
                );
                (k, to_prelude(v))
            })
            .collect();

        let binding_names: HashSet<Symbol> = bindings.iter().map(|(s, _)| *s).collect();
        (
            SharedCode {
                slabs,
                def_sites,
                declared_sigs,
                registry_names,
                binding_names,
            },
            bindings,
        )
    }
}
