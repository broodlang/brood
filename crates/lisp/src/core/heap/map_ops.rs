//! CHAMP map operations (child of heap).
use super::*;

impl Heap {
    // ===== map operations (ADR-040: CHAMP — see `core/map_champ.rs`) =====
    //
    // Every op returns a fresh `Value::Map` handle; the trie is path-copied
    // from root to the touched leaf, with the rest structurally shared.
    // None of these mutate any existing `MapNode` — the slab is append-only
    // from the language's point of view, which is what makes RUNTIME/PRELUDE
    // maps safely shareable across processes.

    /// Allocate a fresh empty map — a single root `MapNode` with no
    /// entries. Used by `(hash-map)` with no args and as the starting
    /// point for `map_from_pairs`.
    pub fn alloc_empty_map(&mut self) -> Value {
        let idx = alloc_slot!(self, maps, MapNode::default());
        Value::map(MapId::local_gen(idx, self.local_epoch))
    }

    /// The value `key` maps to, by structural equality, or `None` if absent.
    /// O(log₁₆ N) — one 4-bit hash slice + one bitmap test per trie level.
    pub fn map_get(&self, id: MapId, key: Value) -> Option<Value> {
        let hash = self.hash_value(key);
        self.champ_get(id, key, hash, 0)
    }

    fn champ_get(&self, id: MapId, key: Value, hash: u64, depth: u32) -> Option<Value> {
        let node = self.map_node(id);
        if node.is_collision {
            return node
                .data
                .iter()
                .find(|(k, _)| self.equal(*k, key))
                .map(|(_, v)| *v);
        }
        let slot = map_champ::slot_at(hash, depth);
        let bit = map_champ::slot_mask(slot);
        if node.data_map & bit != 0 {
            let i = map_champ::rank(node.data_map, slot);
            let (k, v) = node.data[i];
            if self.equal(k, key) {
                Some(v)
            } else {
                None
            }
        } else if node.node_map & bit != 0 {
            let j = map_champ::rank(node.node_map, slot);
            self.champ_get(node.children[j], key, hash, depth + 1)
        } else {
            None
        }
    }

    /// A fresh map with `key` bound to `val` — replaces or inserts in
    /// O(log₁₆ N). Path-copies only the nodes from root to the touched
    /// leaf; every other node is structurally shared with the input map.
    pub fn map_assoc(&mut self, id: MapId, key: Value, val: Value) -> Value {
        let hash = self.hash_value(key);
        // A lone assoc has no build-local node to reuse — copy-on-write (`None`).
        let new_root = self.champ_assoc(id, key, val, hash, 0, None);
        Value::map(new_root)
    }

    /// A fresh map with `key`'s integer value incremented by `delta`, or
    /// `delta` itself when `key` is absent. Semantically equivalent to
    /// `(assoc m key (+ (get m key 0) delta))` but in a **single trie walk**:
    /// the read and write are fused into one path-copy traversal instead of two.
    pub fn map_int_add(&mut self, id: MapId, key: Value, delta: i64) -> Value {
        let hash = self.hash_value(key);
        let new_root = self.champ_int_add(id, key, delta, hash, 0);
        Value::map(new_root)
    }

    fn champ_int_add(&mut self, id: MapId, key: Value, delta: i64, hash: u64, depth: u32) -> MapId {
        let node = self.map_node(id);
        let is_collision = node.is_collision;
        let data_map = node.data_map;
        let node_map = node.node_map;

        if is_collision {
            let pos = self
                .map_node(id)
                .data
                .iter()
                .position(|(k, _)| self.equal(*k, key));
            match pos {
                Some(i) => {
                    let old_v = self.map_node(id).data[i].1;
                    let new_val = Value::int(old_v.as_int().unwrap_or(0) + delta);
                    let mut new_data = self.map_node(id).data.clone();
                    let size = self.map_node(id).size;
                    new_data[i].1 = new_val;
                    return self.alloc_map_node(MapNode {
                        size,
                        data_map: 0,
                        node_map: 0,
                        is_collision: true,
                        data: new_data,
                        children: SmallVec::new(),
                    });
                }
                None => {
                    let new_val = Value::int(delta);
                    let node = self.map_node(id);
                    let mut new_data = node.data.clone();
                    let size = node.size + 1;
                    new_data.push((key, new_val));
                    return self.alloc_map_node(MapNode {
                        size,
                        data_map: 0,
                        node_map: 0,
                        is_collision: true,
                        data: new_data,
                        children: SmallVec::new(),
                    });
                }
            }
        }

        let slot = map_champ::slot_at(hash, depth);
        let bit = map_champ::slot_mask(slot);

        if data_map & bit != 0 {
            let i = map_champ::rank(data_map, slot);
            let (existing_k, existing_v) = self.map_node(id).data[i];
            if self.equal(existing_k, key) {
                let new_val = Value::int(existing_v.as_int().unwrap_or(0) + delta);
                let node = self.map_node(id);
                let mut new_data = node.data.clone();
                new_data[i].1 = new_val;
                return self.alloc_map_node(MapNode {
                    size: node.size,
                    data_map,
                    node_map,
                    is_collision: false,
                    data: new_data,
                    children: node.children.clone(),
                });
            }
            // Key absent — insert delta as new entry, splitting the slot.
            let other_hash = self.hash_value(existing_k);
            let new_val = Value::int(delta);
            let child_id = self.champ_split(
                existing_k,
                existing_v,
                other_hash,
                key,
                new_val,
                hash,
                depth + 1,
            );
            let new_data_map = data_map ^ bit;
            let new_node_map = node_map | bit;
            let child_pos = map_champ::rank(new_node_map, slot);
            let node = self.map_node(id);
            let mut new_data = node.data.clone();
            new_data.remove(i);
            let mut new_children = node.children.clone();
            new_children.insert(child_pos, child_id);
            return self.alloc_map_node(MapNode {
                size: node.size + 1,
                data_map: new_data_map,
                node_map: new_node_map,
                is_collision: false,
                data: new_data,
                children: new_children,
            });
        }

        if node_map & bit != 0 {
            let j = map_champ::rank(node_map, slot);
            let old_child = self.map_node(id).children[j];
            let old_child_size = self.map_node(old_child).size;
            let new_child = self.champ_int_add(old_child, key, delta, hash, depth + 1);
            let new_child_size = self.map_node(new_child).size;
            let node = self.map_node(id);
            let mut new_children = node.children.clone();
            new_children[j] = new_child;
            return self.alloc_map_node(MapNode {
                size: node.size + new_child_size - old_child_size,
                data_map,
                node_map,
                is_collision: false,
                data: node.data.clone(),
                children: new_children,
            });
        }

        // Empty slot — insert key → delta.
        let new_val = Value::int(delta);
        let new_data_map = data_map | bit;
        let new_data_pos = map_champ::rank(new_data_map, slot);
        let node = self.map_node(id);
        let mut new_data = node.data.clone();
        new_data.insert(new_data_pos, (key, new_val));
        self.alloc_map_node(MapNode {
            size: node.size + 1,
            data_map: new_data_map,
            node_map,
            is_collision: false,
            data: new_data,
            children: node.children.clone(),
        })
    }

    /// True iff `id` names a node *this transient build allocated*, so it is
    /// safe to mutate in place (see `docs/transients.md`). The rule is a single
    /// integer compare because `alloc_slot!` only ever appends to the nursery
    /// and GC cannot fire mid-build: a node whose nursery index is `>= watermark`
    /// (the slab length captured at build entry) was created by this build; any
    /// input node — older nursery slot, tenured `old` generation, or a shared
    /// PRELUDE/RUNTIME region — fails the test and is copied instead. `None`
    /// watermark is copy-on-write mode: nothing is owned.
    #[inline]
    fn is_owned(id: MapId, watermark: Option<usize>) -> bool {
        match watermark {
            Some(w) => id.region() == LOCAL && !id.is_old() && id.index() >= w,
            None => false,
        }
    }

    /// Insert/replace `key → val`, in O(log₁₆ N). `watermark` selects the write
    /// strategy: `None` path-copies every touched node (the immutable
    /// contract — used by `map_assoc` and all single-op callers); `Some(w)`
    /// runs the **transient build** — every node this build owns
    /// ([`is_owned`]) is rewritten in place instead of re-allocated, collapsing
    /// the per-level `SmallVec` clone + slab push. The *structural* decisions
    /// (slot, bitmap, rank, position, size) are identical to the copy-on-write
    /// arm, so both produce the byte-identical canonical CHAMP shape — only the
    /// write differs (mutate the owned slot vs. allocate a fresh one).
    fn champ_assoc(
        &mut self,
        id: MapId,
        key: Value,
        val: Value,
        hash: u64,
        depth: u32,
        watermark: Option<usize>,
    ) -> MapId {
        let owned = Self::is_owned(id, watermark);
        // Snapshot the node fields we need — releases the immutable borrow
        // on `self` before we go allocating new slots.
        let node = self.map_node(id);
        let is_collision = node.is_collision;
        let data_map = node.data_map;
        let node_map = node.node_map;

        if is_collision {
            // At max depth — all entries share the full hash. Linear scan
            // by `equal`.
            let pos = self
                .map_node(id)
                .data
                .iter()
                .position(|(k, _)| self.equal(*k, key));
            match pos {
                Some(i) => {
                    if owned {
                        self.local.maps[id.index()].data[i].1 = val;
                        return id;
                    }
                    let mut new_data = self.map_node(id).data.clone();
                    let size = self.map_node(id).size;
                    new_data[i].1 = val;
                    return self.alloc_map_node(MapNode {
                        size,
                        data_map: 0,
                        node_map: 0,
                        is_collision: true,
                        data: new_data,
                        children: SmallVec::new(),
                    });
                }
                None => {
                    if owned {
                        let n = &mut self.local.maps[id.index()];
                        n.data.push((key, val));
                        n.size += 1;
                        return id;
                    }
                    let node = self.map_node(id);
                    let mut new_data = node.data.clone();
                    let size = node.size + 1;
                    new_data.push((key, val));
                    return self.alloc_map_node(MapNode {
                        size,
                        data_map: 0,
                        node_map: 0,
                        is_collision: true,
                        data: new_data,
                        children: SmallVec::new(),
                    });
                }
            }
        }

        let slot = map_champ::slot_at(hash, depth);
        let bit = map_champ::slot_mask(slot);

        // Case 1: slot already holds an inline (k, v) entry.
        if data_map & bit != 0 {
            let i = map_champ::rank(data_map, slot);
            let (existing_k, existing_v) = self.map_node(id).data[i];
            if self.equal(existing_k, key) {
                // Overwrite. If the value is identical by `equal`, we could
                // return id unchanged — but assoc's contract is "returns a
                // fresh map", and callers can dedup themselves if they care.
                if owned {
                    self.local.maps[id.index()].data[i].1 = val;
                    return id;
                }
                let node = self.map_node(id);
                let mut new_data = node.data.clone();
                new_data[i].1 = val;
                let new_node = MapNode {
                    size: node.size,
                    data_map,
                    node_map,
                    is_collision: false,
                    data: new_data,
                    children: node.children.clone(),
                };
                return self.alloc_map_node(new_node);
            }
            // Different key hashed to same slot. Split: turn this inline
            // entry into a child sub-node holding both pairs. (The child is
            // freshly allocated, so it is auto-owned for the rest of the build.)
            let other_hash = self.hash_value(existing_k);
            let child_id = self.champ_split(
                existing_k,
                existing_v,
                other_hash,
                key,
                val,
                hash,
                depth + 1,
            );
            let new_data_map = data_map ^ bit;
            let new_node_map = node_map | bit;
            let child_pos = map_champ::rank(new_node_map, slot);
            if owned {
                let n = &mut self.local.maps[id.index()];
                n.data.remove(i);
                n.children.insert(child_pos, child_id);
                n.data_map = new_data_map;
                n.node_map = new_node_map;
                n.size += 1;
                return id;
            }
            let node = self.map_node(id); // re-borrow after the recursive alloc
            let mut new_data = node.data.clone();
            new_data.remove(i);
            let mut new_children = node.children.clone();
            new_children.insert(child_pos, child_id);
            let new_node = MapNode {
                size: node.size + 1,
                data_map: new_data_map,
                node_map: new_node_map,
                is_collision: false,
                data: new_data,
                children: new_children,
            };
            return self.alloc_map_node(new_node);
        }

        // Case 2: slot holds a child sub-node — recurse, then patch the
        // child handle.
        if node_map & bit != 0 {
            let j = map_champ::rank(node_map, slot);
            let old_child = self.map_node(id).children[j];
            let old_child_size = self.map_node(old_child).size;
            let new_child = self.champ_assoc(old_child, key, val, hash, depth + 1, watermark);
            let new_child_size = self.map_node(new_child).size;
            if owned {
                let n = &mut self.local.maps[id.index()];
                n.children[j] = new_child;
                n.size = n.size + new_child_size - old_child_size;
                return id;
            }
            let node = self.map_node(id);
            let mut new_children = node.children.clone();
            new_children[j] = new_child;
            let new_node = MapNode {
                size: node.size + new_child_size - old_child_size,
                data_map,
                node_map,
                is_collision: false,
                data: node.data.clone(),
                children: new_children,
            };
            return self.alloc_map_node(new_node);
        }

        // Case 3: empty slot — insert a fresh inline entry.
        let new_data_map = data_map | bit;
        let new_data_pos = map_champ::rank(new_data_map, slot);
        if owned {
            let n = &mut self.local.maps[id.index()];
            n.data.insert(new_data_pos, (key, val));
            n.data_map = new_data_map;
            n.size += 1;
            return id;
        }
        let node = self.map_node(id);
        let mut new_data = node.data.clone();
        new_data.insert(new_data_pos, (key, val));
        let new_node = MapNode {
            size: node.size + 1,
            data_map: new_data_map,
            node_map,
            is_collision: false,
            data: new_data,
            children: node.children.clone(),
        };
        self.alloc_map_node(new_node)
    }

    /// Build a sub-node holding two entries with different keys but
    /// possibly the same slot at `depth`. Recursively descends until
    /// the two keys' hash slices diverge (or until [`MAX_DEPTH`], where
    /// it spawns a collision leaf). Used by `champ_assoc`'s split case.
    //
    // 8 args: two (k, v, h) triples + depth + &mut self. Bundling the
    // triples into a struct adds noise for an internal-only helper called
    // from one site.
    #[allow(clippy::too_many_arguments)]
    fn champ_split(
        &mut self,
        k1: Value,
        v1: Value,
        h1: u64,
        k2: Value,
        v2: Value,
        h2: u64,
        depth: u32,
    ) -> MapId {
        if depth >= MAX_DEPTH {
            // Hash exhausted — both keys hash identically. Collision leaf.
            let mut data = SmallVec::<[(Value, Value); 4]>::new();
            data.push((k1, v1));
            data.push((k2, v2));
            return self.alloc_map_node(MapNode {
                size: 2,
                data_map: 0,
                node_map: 0,
                is_collision: true,
                data,
                children: SmallVec::new(),
            });
        }
        let s1 = map_champ::slot_at(h1, depth);
        let s2 = map_champ::slot_at(h2, depth);
        if s1 == s2 {
            // Still colliding at this level — recurse.
            let child = self.champ_split(k1, v1, h1, k2, v2, h2, depth + 1);
            let bit = map_champ::slot_mask(s1);
            let mut children = SmallVec::<[MapId; 4]>::new();
            children.push(child);
            return self.alloc_map_node(MapNode {
                size: 2,
                data_map: 0,
                node_map: bit,
                is_collision: false,
                data: SmallVec::new(),
                children,
            });
        }
        // Diverged: two inline entries in the new node, ordered by slot.
        let (lo_slot, lo_kv, hi_slot, hi_kv) = if s1 < s2 {
            (s1, (k1, v1), s2, (k2, v2))
        } else {
            (s2, (k2, v2), s1, (k1, v1))
        };
        let data_map = map_champ::slot_mask(lo_slot) | map_champ::slot_mask(hi_slot);
        let mut data = SmallVec::<[(Value, Value); 4]>::new();
        data.push(lo_kv);
        data.push(hi_kv);
        self.alloc_map_node(MapNode {
            size: 2,
            data_map,
            node_map: 0,
            is_collision: false,
            data,
            children: SmallVec::new(),
        })
    }

    /// A fresh map with `key` removed; a clone of the same shape if
    /// `key` was absent. Path-copies the affected branch; collapses
    /// singleton sub-trees into the parent's inline data (the CHAMP
    /// canonicalisation rule that keeps the tree shallow).
    pub fn map_dissoc(&mut self, id: MapId, key: Value) -> Value {
        let hash = self.hash_value(key);
        let new_root = self.champ_dissoc(id, key, hash, 0);
        Value::map(new_root)
    }

    fn champ_dissoc(&mut self, id: MapId, key: Value, hash: u64, depth: u32) -> MapId {
        let node = self.map_node(id);
        let is_collision = node.is_collision;

        if is_collision {
            let pos = node.data.iter().position(|(k, _)| self.equal(*k, key));
            let Some(i) = pos else {
                return self.clone_map_node(id);
            };
            let mut new_data = node.data.clone();
            new_data.remove(i);
            return self.alloc_map_node(MapNode {
                size: node.size - 1,
                data_map: 0,
                node_map: 0,
                is_collision: true,
                data: new_data,
                children: SmallVec::new(),
            });
        }

        let slot = map_champ::slot_at(hash, depth);
        let bit = map_champ::slot_mask(slot);
        let data_map = node.data_map;
        let node_map = node.node_map;

        // Case 1: inline entry at this slot.
        if data_map & bit != 0 {
            let i = map_champ::rank(data_map, slot);
            if !self.equal(node.data[i].0, key) {
                return self.clone_map_node(id); // key absent
            }
            let new_data_map = data_map ^ bit;
            let mut new_data = node.data.clone();
            new_data.remove(i);
            return self.alloc_map_node(MapNode {
                size: node.size - 1,
                data_map: new_data_map,
                node_map,
                is_collision: false,
                data: new_data,
                children: node.children.clone(),
            });
        }

        // Case 2: child sub-node at this slot — recurse and patch.
        if node_map & bit != 0 {
            let j = map_champ::rank(node_map, slot);
            let old_child = node.children[j];
            let old_child_size = self.map_node(old_child).size;
            let new_child = self.champ_dissoc(old_child, key, hash, depth + 1);
            let new_child_node = self.map_node(new_child);
            let new_child_size = new_child_node.size;
            if new_child_size == old_child_size {
                // No change (key was absent below).
                return self.clone_map_node(id);
            }
            // Promote: if the child shrunk to a singleton (one entry, no
            // children — branch *or* collision leaf), inline it here.
            // Collision leaves are legitimate singletons: the surviving
            // entry's hash still routes through this slot at this depth,
            // so inlining is safe and keeps the trie shallow.
            if new_child_node.is_singleton() {
                let (kk, vv) = new_child_node.data[0];
                let node = self.map_node(id);
                let new_node_map = node_map ^ bit;
                let new_data_map = data_map | bit;
                let mut new_children = node.children.clone();
                new_children.remove(j);
                let new_data_pos = map_champ::rank(new_data_map, slot);
                let mut new_data = node.data.clone();
                new_data.insert(new_data_pos, (kk, vv));
                return self.alloc_map_node(MapNode {
                    size: node.size - 1,
                    data_map: new_data_map,
                    node_map: new_node_map,
                    is_collision: false,
                    data: new_data,
                    children: new_children,
                });
            }
            // If the child is now empty entirely, drop the reference.
            if new_child_node.is_empty() {
                let node = self.map_node(id);
                let new_node_map = node_map ^ bit;
                let mut new_children = node.children.clone();
                new_children.remove(j);
                return self.alloc_map_node(MapNode {
                    size: node.size - 1,
                    data_map,
                    node_map: new_node_map,
                    is_collision: false,
                    data: node.data.clone(),
                    children: new_children,
                });
            }
            // Otherwise just swap the child handle.
            let node = self.map_node(id);
            let mut new_children = node.children.clone();
            new_children[j] = new_child;
            return self.alloc_map_node(MapNode {
                size: node.size - old_child_size + new_child_size,
                data_map,
                node_map,
                is_collision: false,
                data: node.data.clone(),
                children: new_children,
            });
        }

        // Case 3: empty slot — key absent.
        self.clone_map_node(id)
    }

    /// Build a canonical map from raw `(key, value)` pairs, applying
    /// last-wins de-dup by structural equality. Used by the `{ }` literal
    /// reader path and `(hash-map …)`. Folds `assoc` over a fresh empty
    /// root — O(N log N) overall, in line with CHAMP's per-op cost.
    pub fn map_from_pairs(&mut self, pairs: Vec<(Value, Value)>) -> Value {
        // GC-quiet in-place build: the fresh root is allocated at `watermark`, so
        // it and every node below it is build-owned and rewritten in place — no
        // per-`assoc` path-copy. This is purely an implementation detail of
        // *constructing* a fresh immutable map (no `Value` is ever mutable, and GC
        // can't fire mid-builtin); the result is the byte-identical canonical CHAMP
        // shape a copy-on-write fold would yield.
        let watermark = Some(self.local.maps.len());
        let mut current = match self.alloc_empty_map().unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!("alloc_empty_map returns Value::Map"),
        };
        for (k, v) in pairs {
            let hash = self.hash_value(k);
            current = self.champ_assoc(current, k, v, hash, 0, watermark);
        }
        Value::map(current)
    }

    /// Pour `pairs` into the existing map `into` via a transient build. The
    /// input `into` is *not* build-owned (it was allocated before the watermark),
    /// so the first `assoc` path-copies its touched nodes once; every node
    /// allocated thereafter is owned and mutated in place. Backs `(into m …)` /
    /// `zipmap` / `select-keys` in the prelude. The result equals the
    /// copy-on-write `(reduce assoc into pairs)`.
    pub fn map_from_pairs_into(&mut self, into: MapId, pairs: Vec<(Value, Value)>) -> Value {
        let watermark = Some(self.local.maps.len());
        let mut current = into;
        for (k, v) in pairs {
            let hash = self.hash_value(k);
            current = self.champ_assoc(current, k, v, hash, 0, watermark);
        }
        Value::map(current)
    }

    /// Build a **set** (`Value::Set`) from `elems`: a CHAMP of `elem → true`,
    /// deduped by structural equality (the trie collapses duplicate keys), wrapped
    /// as a set. Same GC-quiet in-place build as `map_from_pairs`; the result is a
    /// fresh immutable value. Backs the `#{…}` reader literal, `set` construction,
    /// and `from_message` reconstruction.
    pub fn set_from_elems(&mut self, elems: Vec<Value>) -> Value {
        let watermark = Some(self.local.maps.len());
        let mut current = match self.alloc_empty_map().unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!("alloc_empty_map returns Value::Map"),
        };
        for e in elems {
            let hash = self.hash_value(e);
            current = self.champ_assoc(current, e, Value::Bool(true), hash, 0, watermark);
        }
        Value::set(current)
    }

    /// The elements of a set, in the CHAMP's deterministic-per-shape order (the
    /// keys of the backing trie — the values are all `true` and dropped).
    pub fn set_elems(&self, id: MapId) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.map_size(id));
        self.fold_entries(id, &mut |k, _v| out.push(k));
        out
    }

    /// All entries in the map, walked depth-first through the trie.
    /// Order is deterministic per shape (slot-index ascending at each
    /// level, then collision-leaf order) but is **not** insertion order
    /// — ADR-040's one contract change vs ADR-030. Callers that need an
    /// ordered set should sort the result.
    pub fn map_entries(&self, id: MapId) -> Vec<(Value, Value)> {
        let mut out = Vec::with_capacity(self.map_size(id));
        self.collect_entries_into(id, &mut out);
        out
    }

    fn collect_entries_into(&self, id: MapId, out: &mut Vec<(Value, Value)>) {
        let node = self.map_node(id);
        for &kv in &node.data {
            out.push(kv);
        }
        if !node.is_collision {
            // children are in slot-ascending order — that's our traversal.
            for &child in &node.children {
                self.collect_entries_into(child, out);
            }
        }
    }

    /// Walk every entry in the map, calling `f(k, v)` on each. Borrow-friendly
    /// alternative to `map_entries` when the caller doesn't need a Vec — used by
    /// `hash_value_into` where allocating per call would be wasteful.
    pub fn fold_entries(&self, id: MapId, f: &mut dyn FnMut(Value, Value)) {
        let node = self.map_node(id);
        for &(k, v) in &node.data {
            f(k, v);
        }
        if !node.is_collision {
            for &child in &node.children {
                self.fold_entries(child, f);
            }
        }
    }

    /// Number of entries in the map. O(1) — every node tracks the size
    /// of its own subtree, so the root's `size` is the answer.
    pub fn map_size(&self, id: MapId) -> usize {
        self.map_node(id).size as usize
    }

    /// Allocate a new map node — the path-copy primitive every assoc /
    /// dissoc step ends with. Returns the `MapId` (not a `Value`) so
    /// internal callers can stitch children together before wrapping the
    /// root in `Value::Map`.
    fn alloc_map_node(&mut self, node: MapNode) -> MapId {
        let idx = alloc_slot!(self, maps, node);
        MapId::local_gen(idx, self.local_epoch)
    }

    /// A fresh root `MapNode` slot holding the same shape as `id`. The
    /// child handles are reused (structural sharing extends one level
    /// out from the root), so this is `O(branching)`, not deep. Used by
    /// `dissoc` when the key was absent — the surface contract is
    /// "every op returns a fresh map handle", and an unconditional
    /// root clone keeps that honest without touching the unchanged
    /// subtree.
    fn clone_map_node(&mut self, id: MapId) -> MapId {
        let node = self.map_node(id);
        let cloned = MapNode {
            size: node.size,
            data_map: node.data_map,
            node_map: node.node_map,
            is_collision: node.is_collision,
            data: node.data.clone(),
            children: node.children.clone(),
        };
        self.alloc_map_node(cloned)
    }

    /// The single chokepoint for materialising a `Value::Str` into LOCAL. Routes
    /// by size: strings of [`SHARED_BLOB_THRESHOLD`] bytes or more allocate an
    /// `Arc<SharedBlob>` so a later cross-process send can ship a handle
    /// instead of copying the bytes; smaller strings stay inline because
    /// atomic-refcount traffic dominates the per-byte memcpy at small sizes.
    /// Every `String -> Value::Str` path must come through here — don't add a
    /// second allocator that bypasses the threshold.
    pub fn alloc_string(&mut self, s: &str) -> Value {
        let entry = if s.len() >= SHARED_BLOB_THRESHOLD {
            LocalString::Shared(SharedBlob::new(s.as_bytes()))
        } else {
            LocalString::Inline(s.to_string())
        };
        let idx = self.local.strings.len();
        self.local.strings.push(entry);
        Value::str_(StrId::local_gen(idx, self.local_epoch))
    }

    /// Materialise a `Value::Rope` into LOCAL from an owned `ropey::Rope`
    /// (ADR-045). Bump-only like the other allocators; the rope's internal
    /// `Arc` nodes mean this stores one cheap handle, not a byte copy.
    pub fn alloc_rope(&mut self, r: ropey::Rope) -> Value {
        let idx = self.local.ropes.len();
        self.local.ropes.push(r);
        Value::rope(RopeId::local_gen(idx, self.local_epoch))
    }

    /// Resolve a rope handle to its `&ropey::Rope`. LOCAL slots are the common
    /// case; RUNTIME holds a rope `def`'d to a global (shared read-only across
    /// the runtime's processes). There is no PRELUDE rope (see `to_prelude`).
    pub fn rope(&self, id: RopeId) -> SlabRef<'_, ropey::Rope> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "rope");
                SlabRef::direct(&self.old.ropes[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "rope");
                SlabRef::direct(&self.local.ropes[id.index()])
            }
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.ropes.get(id.index()).expect("runtime rope handle")
            }),
            _ => unreachable!("Rope handles live only in LOCAL or RUNTIME"),
        }
    }

    /// The **single chokepoint** for turning a `num_bigint::BigInt` back into a
    /// Brood integer, enforcing the normalize invariant: if `n` fits in `i64`
    /// it returns `Value::Int` (demotion); otherwise it allocates a LOCAL
    /// `Value::BigInt`. Every arithmetic/bitwise path that computes in BigInt
    /// space funnels its result through here, so an `Int` and a `BigInt` are
    /// always numerically disjoint — which is what lets equality/hashing/
    /// comparison treat them as never-equal. Use this, never `alloc_bigint`,
    /// for the *result* of a computation.
    pub fn int_from_bigint(&mut self, n: num_bigint::BigInt) -> Value {
        use num_traits::ToPrimitive;
        match n.to_i64() {
            Some(i) => Value::int(i),
            None => self.alloc_bigint(n),
        }
    }

    /// Materialise a `Value::BigInt` into LOCAL from an owned `num_bigint::BigInt`
    /// (mirrors [`alloc_string`](Self::alloc_string)). **Does not normalize** —
    /// the caller must already know `n` is outside the i64 range (the reader for
    /// an over-range literal, or the demotion-checked [`int_from_bigint`]). A
    /// `debug_assert!` guards the invariant.
    pub fn alloc_bigint(&mut self, n: num_bigint::BigInt) -> Value {
        debug_assert!(
            {
                use num_traits::ToPrimitive;
                n.to_i64().is_none()
            },
            "alloc_bigint given an i64-range value (breaks the normalize invariant); \
             use int_from_bigint for computed results"
        );
        let idx = self.local.bigints.len();
        self.local.bigints.push(n);
        Value::bigint(BigIntId::local_gen(idx, self.local_epoch))
    }

    /// Resolve a bignum handle to its `&num_bigint::BigInt`. LOCAL is the common
    /// case; RUNTIME holds a bignum `def`'d to a global or baked into shared code;
    /// PRELUDE a bignum literal frozen into the prelude (none today, but the path
    /// mirrors `string`). Honours the GC epoch tripwire like every leaf.
    pub fn bigint(&self, id: BigIntId) -> SlabRef<'_, num_bigint::BigInt> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "bigint");
                SlabRef::direct(&self.old.bigints[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "bigint");
                SlabRef::direct(&self.local.bigints[id.index()])
            }
            PRELUDE => SlabRef::direct(&self.prelude.slabs.bigints[id.index()]),
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.bigints.get(id.index()).expect("runtime bigint handle")
            }),
            _ => unreachable!("invalid handle region"),
        }
    }

    /// Materialise a `Value::Decimal` into LOCAL from an owned `bigdecimal::BigDecimal`
    /// (mirrors [`alloc_bigint`](Self::alloc_bigint)). Unlike a bignum there is no
    /// normalize-into-`Int` invariant — a decimal is stored as-is.
    pub fn alloc_decimal(&mut self, n: bigdecimal::BigDecimal) -> Value {
        let idx = self.local.decimals.len();
        self.local.decimals.push(n);
        Value::decimal(DecimalId::local_gen(idx, self.local_epoch))
    }

    /// Resolve a decimal handle to its `&bigdecimal::BigDecimal` (mirrors
    /// [`bigint`](Self::bigint)). Honours the GC epoch tripwire like every leaf.
    pub fn decimal(&self, id: DecimalId) -> SlabRef<'_, bigdecimal::BigDecimal> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "decimal");
                SlabRef::direct(&self.old.decimals[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "decimal");
                SlabRef::direct(&self.local.decimals[id.index()])
            }
            PRELUDE => SlabRef::direct(&self.prelude.slabs.decimals[id.index()]),
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.decimals.get(id.index()).expect("runtime decimal handle")
            }),
            _ => unreachable!("invalid handle region"),
        }
    }

    /// Materialise a `Value::Bytes` into LOCAL from an `Arc<SharedBlob>` of raw bytes
    /// (mirrors [`alloc_bigint`](Self::alloc_bigint)). Byte-clean — the bytes
    /// are arbitrary, never assumed UTF-8.
    pub fn alloc_bytes(&mut self, blob: Arc<SharedBlob>) -> Value {
        let idx = self.local.bytes.len();
        self.local.bytes.push(blob);
        Value::bytes(BytesId::local_gen(idx, self.local_epoch))
    }

    /// Resolve a bytes handle to its `&Arc<SharedBlob>` (mirrors
    /// [`bigint`](Self::bigint)). Honours the GC epoch tripwire. The
    /// caller reads `.as_bytes()` — raw bytes, never decoded as UTF-8 text.
    pub fn bytes(&self, id: BytesId) -> SlabRef<'_, Arc<SharedBlob>> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "bytes");
                SlabRef::direct(&self.old.bytes[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "bytes");
                SlabRef::direct(&self.local.bytes[id.index()])
            }
            PRELUDE => SlabRef::direct(&self.prelude.slabs.bytes[id.index()]),
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.bytes.get(id.index()).expect("runtime bytes handle")
            }),
            _ => unreachable!("invalid handle region"),
        }
    }

    /// Read any integer `Value` (`Int` or `BigInt`) as an owned
    /// `num_bigint::BigInt`, promoting an `Int`. The bridge the arithmetic
    /// primitives use to compute in a single (big) domain. Returns `None` for a
    /// non-integer value.
    pub fn as_bigint(&self, v: Value) -> Option<num_bigint::BigInt> {
        match v.unpack() {
            ValueRef::Int(i) => Some(num_bigint::BigInt::from(i)),
            ValueRef::BigInt(id) => Some(self.bigint(id).clone()),
            _ => None,
        }
    }

    /// Like [`as_bigint`](Self::as_bigint) but for a context that already knows
    /// `v` is an integer (`Int`/`BigInt`) — panics otherwise. Used by
    /// `value_cmp`'s integer arms, which match-guard on integer-ness first.
    pub(crate) fn bigint_of(&self, v: Value) -> num_bigint::BigInt {
        self.as_bigint(v).expect("bigint_of on a non-integer value")
    }

    /// Read any *exact* number (`Int`, `BigInt`, or `Decimal`) as an owned
    /// `bigdecimal::BigDecimal`, for the decimal arithmetic path. Returns `None`
    /// for a `Float` (inexact — the float-contagion path handles that) or a
    /// non-number.
    pub fn as_bigdecimal(&self, v: Value) -> Option<bigdecimal::BigDecimal> {
        match v.unpack() {
            ValueRef::Int(i) => Some(bigdecimal::BigDecimal::from(i)),
            ValueRef::BigInt(id) => Some(bigdecimal::BigDecimal::from(self.bigint(id).clone())),
            ValueRef::Decimal(id) => Some(self.decimal(id).clone()),
            _ => None,
        }
    }

    /// Install a pre-existing `Arc<SharedBlob>` as a new LOCAL string slot.
    /// Used by the receive path ([`crate::process::message::from_message`]):
    /// the sender already bumped the refcount via `Arc::clone` for the
    /// `Message`, so installing it here is just slot bookkeeping — no copy.
    pub(crate) fn alloc_string_from_shared(&mut self, blob: Arc<SharedBlob>) -> Value {
        let idx = self.local.strings.len();
        self.local.strings.push(LocalString::Shared(blob));
        Value::str_(StrId::local_gen(idx, self.local_epoch))
    }

    /// A LOCAL string slot, routed to the nursery or old generation by the
    /// handle's age bit. Caller must have checked `id.region() == LOCAL`. Not
    /// debug-gated — the production `local_shared_blob` path uses it too.
    fn string_slot(&self, id: StrId) -> &LocalString {
        if id.is_old() {
            &self.old.strings[id.index()]
        } else {
            &self.local.strings[id.index()]
        }
    }

    /// Debug-only: the underlying `SharedBlob` address for a LOCAL Shared
    /// string, used by the `%blob-ptr` primitive for identity assertions in
    /// cross-process tests. `None` for an inline string or a non-LOCAL handle.
    /// Does **not** clone the `Arc`, so the read leaves the refcount
    /// untouched. Honours the GC epoch tripwire — a use-after-GC trips an
    /// assertion at the call site, the same as every other LOCAL accessor.
    #[cfg(debug_assertions)]
    pub(crate) fn local_shared_blob_ptr(&self, id: StrId) -> Option<*const SharedBlob> {
        if id.region() != LOCAL {
            return None;
        }
        self.check_epoch_aged(
            id.is_old(),
            id.generation(),
            id.index(),
            "local_shared_blob_ptr",
            id.0,
        );
        match self.string_slot(id) {
            LocalString::Shared(arc) => Some(Arc::as_ptr(arc)),
            LocalString::Inline(_) => None,
        }
    }

    /// Debug-only: the current `Arc::strong_count` for a LOCAL Shared string.
    /// Used by `%blob-strong-count` for leak-check assertions; like
    /// [`Self::local_shared_blob_ptr`] this does not bump the count, so the
    /// reading caller doesn't itself perturb the value it's checking.
    /// Honours the GC epoch tripwire.
    #[cfg(debug_assertions)]
    pub(crate) fn local_shared_blob_strong_count(&self, id: StrId) -> Option<usize> {
        if id.region() != LOCAL {
            return None;
        }
        self.check_epoch_aged(
            id.is_old(),
            id.generation(),
            id.index(),
            "local_shared_blob_strong_count",
            id.0,
        );
        match self.string_slot(id) {
            LocalString::Shared(arc) => Some(Arc::strong_count(arc)),
            LocalString::Inline(_) => None,
        }
    }

    /// If `id` is a LOCAL `Shared` string, return a cloned `Arc<SharedBlob>`
    /// (atomic incr, no byte copy). Otherwise return `None` so the caller
    /// falls back to the byte-copying [`Self::string`] path. Used by
    /// [`crate::process::message::to_message`] to ship big strings between
    /// processes without copying.
    pub(crate) fn local_shared_blob(&self, id: StrId) -> Option<Arc<SharedBlob>> {
        if id.region() != LOCAL {
            return None;
        }
        #[cfg(debug_assertions)]
        self.check_epoch_aged(
            id.is_old(),
            id.generation(),
            id.index(),
            "local_shared_blob",
            id.0,
        );
        match self.string_slot(id) {
            LocalString::Shared(arc) => Some(Arc::clone(arc)),
            LocalString::Inline(_) => None,
        }
    }

    pub fn alloc_closure(&mut self, mut c: Closure) -> ClosureId {
        // Precompute each arm's thin-wrapper redirect once, here at the single
        // closure-construction choke point — every LOCAL closure (`fn`/`defn`,
        // and a message-rebuilt one) flows through here. promote/freeze copy the
        // result verbatim, so it never has to be re-derived per call (see
        // `eval::passthrough_arm` and `ClosureArm::passthrough`).
        // Unique arms (the case here — a freshly-built or message-rebuilt closure):
        // fill each arm's pass-through in place. A *shared* arms (from the template
        // cache) already had its pass-through computed once at parse time, so there is
        // nothing to do — and `get_mut` correctly declines to mutate the shared alloc.
        if let Some(arms) = Arc::get_mut(&mut c.arms) {
            for arm in arms.iter_mut() {
                if arm.passthrough.is_none() {
                    arm.passthrough = self.compute_passthrough(arm);
                }
            }
        }
        let idx = alloc_slot!(self, closures, c);
        ClosureId::local_gen(idx, self.local_epoch)
    }

    /// Like [`alloc_closure`](Self::alloc_closure) but for a closure whose arms already
    /// carry their computed [`Passthrough`] — as a [`ClosureTemplate`]-built one does —
    /// so it skips the per-creation pass-through re-analysis (a RUNTIME-body walk). The
    /// hot closure-creation path (`make_closure_cached`).
    pub fn alloc_closure_pre(&mut self, c: Closure) -> ClosureId {
        let idx = alloc_slot!(self, closures, c);
        ClosureId::local_gen(idx, self.local_epoch)
    }

    /// Analyse whether `arm` is a pure pass-through wrapper — a single body form
    /// `(head p_i p_j …)` with no `&optional`/`&` rest, `head` an ordinary
    /// function reference (not a special form, not one of the arm's own params),
    /// and every argument one of the arm's parameters used directly. Returns the
    /// forwarding `(head, map)` if so. A pure function of the immutable arm, run
    /// once at allocation; mirrors the predicate `eval::passthrough_arm` used to
    /// recompute on every call. (A *self-recursive* redirect — `head` resolving back
    /// to this closure, as in `(defn hog () (hog))` — is detected and broken at the
    /// redirect site, since the closure's own global name isn't known here; see the
    /// redirect loops in `eval::eval` and `compile::dispatch`.)
    pub(crate) fn compute_passthrough(&self, arm: &ClosureArm) -> Option<Passthrough> {
        if !arm.optionals.is_empty() || arm.rest.is_some() || arm.body.len() != 1 {
            return None;
        }
        let (head, mut rest) = match arm.body[0].unpack() {
            ValueRef::Pair(p) => self.pair(p),
            _ => return None,
        };
        let head_sym = match head.unpack() {
            ValueRef::Sym(s) => s,
            _ => return None,
        };
        if crate::eval::is_special_form(head_sym) || arm.params.contains(&head_sym) {
            return None;
        }
        let mut map: SmallVec<[usize; 4]> = SmallVec::new();
        loop {
            match rest.unpack() {
                ValueRef::Nil => break,
                ValueRef::Pair(p) => {
                    let (a, next) = self.pair(p);
                    let asym = match a.unpack() {
                        ValueRef::Sym(s) => s,
                        _ => return None, // a literal / nested call — not a pure forward
                    };
                    map.push(arm.params.iter().position(|&p| p == asym)?);
                    rest = next;
                }
                _ => return None, // improper arg list
            }
        }
        Some(Passthrough { head, map })
    }

    pub fn alloc_native(&mut self, f: NativeFn) -> Value {
        // Natives are only allocated during the prelude build (then frozen into
        // PRELUDE); the LOCAL natives slab stays empty at runtime and is never
        // collected.
        let idx = self.local.natives.len();
        self.local.natives.push(f);
        Value::native(NativeId::local_gen(idx, self.local_epoch))
    }

    /// Build a proper list from a vector of items.
    pub fn list(&mut self, items: Vec<Value>) -> Value {
        self.list_with_tail(items, Value::nil())
    }

    /// Build a list of `items` ending in `tail`. A `Nil` tail gives a proper
    /// list; any other tail gives an improper (dotted) list, e.g. `(1 2 . 3)`.
    pub fn list_with_tail(&mut self, items: Vec<Value>, tail: Value) -> Value {
        let mut acc = tail;
        for item in items.into_iter().rev() {
            acc = self.alloc_pair(item, acc);
        }
        acc
    }

    /// Build a proper list from a slice — no intermediate `Vec`. For the hot path
    /// where the items already live in a buffer, notably a `& rest` parameter's
    /// trailing args (every variadic call, which includes all the arithmetic and
    /// comparison operators).
    pub fn list_from_slice(&mut self, items: &[Value]) -> Value {
        let mut acc = Value::nil();
        for &item in items.iter().rev() {
            acc = self.alloc_pair(item, acc);
        }
        acc
    }

}
