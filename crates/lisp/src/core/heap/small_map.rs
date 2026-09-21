//! A per-process map that costs what it holds. `SmallMap` keeps its first few entries in
//! an exact-capacity vector and turns into a hash map only past [`SPILL`].
//!
//! Every process carries a handful of these — its body cache, its global-lookup cache, its
//! arm IC-block index — and a process that parks after one call holds ONE entry in each.
//! `hashbrown` cannot hold one entry in less than four buckets, so a parked process paid
//! 276 + 180 + 84 B for three maps of one entry apiece (the 2026-09-21 allocation histogram,
//! `runtime-frontier.md` §B): 540 B of a 4 345 B floor. Here one entry costs one entry —
//! the vector grows 1 → 2 → 4 → 8 by `reserve_exact`, never `Vec`'s four-minimum — and the
//! probe is a linear scan of at most eight `Copy` keys, which is faster than hashing at that
//! size. Past eight the entries move into a `HashMap` and the map behaves exactly as before,
//! so a process running a large program pays nothing new.
//!
//! Only the surface the three maps use is provided. Add to it when a fourth caller needs
//! more; do not reach for `HashMap` again for something a process holds per instance.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// Entries beyond this many live in the hash map.
const SPILL: usize = 8;

pub(crate) enum SmallMap<K, V, S> {
    /// Up to [`SPILL`] entries, unordered, keys unique.
    Inline(Vec<(K, V)>),
    /// Past the spill point.
    Hashed(HashMap<K, V, S>),
}

impl<K, V, S: Default> Default for SmallMap<K, V, S> {
    fn default() -> Self {
        SmallMap::Inline(Vec::new())
    }
}

impl<K: Copy + Eq + Hash, V, S: BuildHasher + Default> SmallMap<K, V, S> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(crate) fn get(&self, k: &K) -> Option<&V> {
        match self {
            SmallMap::Inline(v) => v.iter().find(|(kk, _)| kk == k).map(|(_, vv)| vv),
            SmallMap::Hashed(m) => m.get(k),
        }
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        match self {
            SmallMap::Inline(v) => v.iter_mut().find(|(kk, _)| kk == k).map(|(_, vv)| vv),
            SmallMap::Hashed(m) => m.get_mut(k),
        }
    }

    /// Insert or overwrite, returning the previous value for `k`.
    pub(crate) fn insert(&mut self, k: K, val: V) -> Option<V> {
        match self {
            SmallMap::Inline(v) => {
                if let Some(slot) = v.iter_mut().find(|(kk, _)| *kk == k) {
                    return Some(std::mem::replace(&mut slot.1, val));
                }
                if v.len() < SPILL {
                    if v.len() == v.capacity() {
                        // 0 → 1 → 2 → 4 → 8: exact, never `Vec`'s four-entry minimum.
                        let want = v.capacity().max(1) * 2;
                        let want = if v.capacity() == 0 { 1 } else { want };
                        v.reserve_exact(want.min(SPILL) - v.len());
                    }
                    v.push((k, val));
                    return None;
                }
                let mut m: HashMap<K, V, S> =
                    HashMap::with_capacity_and_hasher(SPILL * 2, S::default());
                for (kk, vv) in v.drain(..) {
                    m.insert(kk, vv);
                }
                m.insert(k, val);
                *self = SmallMap::Hashed(m);
                None
            }
            SmallMap::Hashed(m) => m.insert(k, val),
        }
    }

    /// Drop every entry and release the storage: a cleared map costs nothing again.
    pub(crate) fn clear(&mut self) {
        *self = SmallMap::Inline(Vec::new());
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K, &mut V) -> bool) {
        match self {
            SmallMap::Inline(v) => v.retain_mut(|(k, val)| keep(k, val)),
            SmallMap::Hashed(m) => m.retain(|k, val| keep(k, val)),
        }
    }

    /// Test-only today (`dbg_compiled_arms`); lift the gate when a runtime caller appears.
    #[cfg(test)]
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        let (a, b) = match self {
            SmallMap::Inline(v) => (Some(v.iter().map(|(_, val)| val)), None),
            SmallMap::Hashed(m) => (None, Some(m.values())),
        };
        a.into_iter().flatten().chain(b.into_iter().flatten())
    }

    /// Entry count — the `%vm-ic-stats` accounting (dev-tools) and the tests.
    #[cfg(any(feature = "dev-tools", test))]
    pub(crate) fn len(&self) -> usize {
        match self {
            SmallMap::Inline(v) => v.len(),
            SmallMap::Hashed(m) => m.len(),
        }
    }

    /// Slots allocated (for the capacity accounting `%vm-ic-stats` reports).
    #[cfg(any(feature = "dev-tools", test))]
    pub(crate) fn capacity(&self) -> usize {
        match self {
            SmallMap::Inline(v) => v.capacity(),
            SmallMap::Hashed(m) => m.capacity(),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            SmallMap::Inline(v) => v.shrink_to_fit(),
            SmallMap::Hashed(m) => m.shrink_to_fit(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::RandomState;

    type M = SmallMap<u64, u32, RandomState>;

    #[test]
    fn one_entry_costs_one_entry_and_growth_is_exact() {
        let mut m = M::new();
        assert_eq!(m.capacity(), 0);
        m.insert(1, 10);
        assert_eq!(m.capacity(), 1, "a first entry allocates exactly one slot");
        m.insert(2, 20);
        assert_eq!(m.capacity(), 2);
        m.insert(3, 30);
        assert_eq!(m.capacity(), 4);
        for k in 4..=8 {
            m.insert(k, k as u32 * 10);
        }
        assert!(
            matches!(m, SmallMap::Inline(_)),
            "eight entries stay inline"
        );
        assert_eq!(m.capacity(), 8);
        m.insert(9, 90);
        assert!(matches!(m, SmallMap::Hashed(_)), "the ninth spills");
        for k in 1..=9u64 {
            assert_eq!(m.get(&k), Some(&(k as u32 * 10)));
        }
        assert_eq!(m.len(), 9);
    }

    #[test]
    fn insert_overwrites_and_reports_the_previous_value_in_both_shapes() {
        let mut m = M::new();
        assert_eq!(m.insert(7, 1), None);
        assert_eq!(m.insert(7, 2), Some(1));
        assert_eq!(m.len(), 1);
        for k in 10..30 {
            m.insert(k, 0);
        }
        assert!(matches!(m, SmallMap::Hashed(_)));
        assert_eq!(m.insert(7, 3), Some(2));
        assert_eq!(m.get(&7), Some(&3));
        *m.get_mut(&7).unwrap() = 4;
        assert_eq!(m.get(&7), Some(&4));
    }

    #[test]
    fn retain_clear_and_values_agree_across_the_spill() {
        for n in [3usize, 20] {
            let mut m = M::new();
            for k in 0..n as u64 {
                m.insert(k, k as u32);
            }
            m.retain(|k, _| k % 2 == 0);
            let mut vals: Vec<u32> = m.values().copied().collect();
            vals.sort();
            assert_eq!(
                vals,
                (0..n as u32).filter(|k| k % 2 == 0).collect::<Vec<_>>()
            );
            m.clear();
            assert_eq!(m.len(), 0);
            assert_eq!(m.capacity(), 0, "a cleared map holds no storage");
            assert!(matches!(m, SmallMap::Inline(_)));
        }
    }
}
