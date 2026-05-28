//! CHAMP — *Compressed Hash-Array Mapped Prefix-tree* — backing for
//! `Value::Map` (ADR-040). Replaces ADR-030's insertion-ordered association
//! vector with a 16-way branching, path-copying, structurally-immutable
//! trie. Lookup / assoc / dissoc are O(log₁₆ N) — effectively O(1) up to
//! billions of entries — and assoc no longer copies the whole entries
//! vector (only the O(log N) nodes on the path from root to the touched
//! leaf are reallocated; the rest is structurally shared).
//!
//! Why CHAMP and not vanilla Clojure HAMT: same big-O, but the *two
//! bitmaps per node* design (`data_map` for inline `(k,v)` entries,
//! `node_map` for child sub-nodes) is smaller, cache-friendlier, and
//! canonical (no two equal maps have different node shapes), so
//! recursive `=` between two maps can bail on the first shape mismatch
//! instead of "iterate one, look every key up in the other" as ADR-030
//! does today. (See Steindorfer & Vinju, OOPSLA 2015.)
//!
//! The node type lives in this module; the *operations* — `map_get`,
//! `map_assoc`, `map_dissoc`, iteration — live as `Heap` methods in
//! `heap.rs` because they need `&Heap` for `hash_value` / `equal` and
//! `&mut Heap` to allocate fresh nodes along the path.

use smallvec::SmallVec;

use crate::core::value::{MapId, Value};

/// One node in the CHAMP trie. Two forms:
///
/// - **Branch** (`is_collision == false`) — `data_map` and `node_map` are
///   16-bit bitmaps over the 16 possible child slots (4-bit hash slice).
///   Slot `i` is **empty** if both bits are 0; an **inline entry** if
///   `data_map` bit `i` is set (its `(k, v)` lives at `data[rank(data_map, i)]`);
///   a **child sub-node** if `node_map` bit `i` is set (the `MapId` lives
///   at `children[rank(node_map, i)]`). The two bitmaps never overlap.
///
/// - **Collision leaf** (`is_collision == true`) — at depth
///   [`MAX_DEPTH`] (full 64-bit hash consumed), `data` holds every
///   entry that shares the full hash; they're distinguished only by
///   `Heap::equal`. `data_map`, `node_map`, `children` are unused.
///
/// `size` is the number of entries **in this subtree** (inclusive), kept
/// in every node so `(count m)` is O(1) at the root and so each
/// assoc/dissoc can update parent sizes on the way back up the path in
/// O(log N).
///
/// `SmallVec<[..; 4]>` lets small nodes (the common case at the leaves)
/// avoid a heap allocation per slab slot — CHAMP nodes are typically
/// half-full or less. Large nodes spill to the heap.
pub struct MapNode {
    /// Entries in this subtree (inclusive). The root node's `size` is
    /// the map's count.
    pub size: u32,
    /// Bit `i` set ⇔ slot `i` holds an inline entry in `data`. Unused on
    /// a collision leaf.
    pub data_map: u16,
    /// Bit `i` set ⇔ slot `i` holds a child sub-node in `children`.
    /// Unused on a collision leaf. `data_map & node_map == 0` always.
    pub node_map: u16,
    /// `true` ⇔ this is a max-depth collision leaf. The `data` array
    /// then holds every entry that hashes identically; `equal` distinguishes.
    pub is_collision: bool,
    /// Inline entries, ordered by slot index (or by insertion on a
    /// collision leaf).
    pub data: SmallVec<[(Value, Value); 4]>,
    /// Child sub-node handles, ordered by slot index. Empty on a
    /// collision leaf.
    pub children: SmallVec<[MapId; 4]>,
}

impl Default for MapNode {
    /// The empty branch — no entries, no children. Used as the freshly
    /// allocated root and as the sweep-cleared slot value.
    fn default() -> Self {
        MapNode {
            size: 0,
            data_map: 0,
            node_map: 0,
            is_collision: false,
            data: SmallVec::new(),
            children: SmallVec::new(),
        }
    }
}

impl MapNode {
    /// Number of *direct* entries (inline + collision); excludes child
    /// subtrees. Equal to `data.len()` regardless of which form.
    #[inline]
    pub fn direct_len(&self) -> usize {
        self.data.len()
    }

    /// Number of child slots (always 0 for a collision leaf).
    #[inline]
    pub fn child_len(&self) -> usize {
        self.children.len()
    }

    /// True if this node has exactly one direct entry and no children —
    /// the *promotion* trigger: a parent dissoc'ing one of its children
    /// down to this shape inlines the surviving entry into the parent
    /// and frees the child, keeping the trie shallow (a deep chain of
    /// 1-entry nodes would be a pathological waste of indirection).
    /// Includes collision leaves dissoc'd down to their last entry.
    #[inline]
    pub fn is_singleton(&self) -> bool {
        self.data.len() == 1 && self.children.is_empty()
    }

    /// Totally empty — no entries, no children. Result of dissoc'ing the
    /// last entry of an inner node; the parent collapses this child away.
    /// The root node may be empty (a freshly-allocated `(hash-map)`).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty() && self.children.is_empty()
    }
}

/// Bits of hash consumed per trie level. 4 → 16-way branching.
pub const BITS_PER_LEVEL: u32 = 4;

/// Number of levels before the hash is exhausted (64 bits / 4 bits per level).
/// At this depth the trie spawns a [collision leaf](MapNode) instead of
/// recursing further.
pub const MAX_DEPTH: u32 = 64 / BITS_PER_LEVEL;

/// The 4-bit hash slice for level `depth` — picks which of the 16 child
/// slots a key belongs in at this level. `depth == 0` is the root.
#[inline]
pub fn slot_at(hash: u64, depth: u32) -> u8 {
    debug_assert!(depth < MAX_DEPTH, "slot_at past hash exhaustion");
    ((hash >> (depth * BITS_PER_LEVEL)) & 0xF) as u8
}

/// `1 << slot` as a 16-bit mask isolating slot `slot` in a bitmap.
#[inline]
pub const fn slot_mask(slot: u8) -> u16 {
    1u16 << (slot as u16)
}

/// The index into a packed slot array (`data` or `children`) for the
/// entry/child at slot `slot`, given the relevant bitmap. Defined only
/// when `bitmap` bit `slot` is set; the rank is the count of lower bits.
#[inline]
pub fn rank(bitmap: u16, slot: u8) -> usize {
    (bitmap & (slot_mask(slot) - 1)).count_ones() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_at_consumes_4_bits_per_level() {
        let h = 0xFEDC_BA98_7654_3210u64;
        assert_eq!(slot_at(h, 0), 0x0);
        assert_eq!(slot_at(h, 1), 0x1);
        assert_eq!(slot_at(h, 15), 0xF);
    }

    #[test]
    fn rank_counts_lower_bits() {
        // bits 0, 2, 5 set
        let bm = 0b00100101u16;
        assert_eq!(rank(bm, 0), 0); // first
        assert_eq!(rank(bm, 2), 1); // second
        assert_eq!(rank(bm, 5), 2); // third
    }

    #[test]
    fn empty_node_is_branch_with_no_entries() {
        let n = MapNode::default();
        assert!(!n.is_collision);
        assert_eq!(n.size, 0);
        assert_eq!(n.data_map, 0);
        assert_eq!(n.node_map, 0);
        assert!(n.data.is_empty());
        assert!(n.children.is_empty());
    }
}
