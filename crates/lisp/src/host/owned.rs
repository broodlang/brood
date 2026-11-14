//! A handle table whose entries belong to a green process, indexed by owner.
//!
//! Sockets, child OS processes, wasm instances and tree-sitter trees all die with the
//! process that owns them, so every process exit asks each of their tables "what did this
//! pid own?". A plain id → entry map answers that by scanning every entry in the runtime;
//! this keeps an owner → ids index beside it, so the answer costs O(entries that pid owns).

use std::collections::{HashMap, HashSet};

struct Slot<T> {
    owner: u64,
    value: T,
}

/// An id → entry table with an owner → ids index kept in step. The owner of an entry
/// changes only through [`OwnedTable::set_owner`].
pub(crate) struct OwnedTable<T> {
    entries: HashMap<u64, Slot<T>>,
    by_owner: HashMap<u64, HashSet<u64>>,
}

impl<T> Default for OwnedTable<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            by_owner: HashMap::new(),
        }
    }
}

// On wasm32 the socket and subprocess hosts are stubs, so the one remaining user (the wasm
// host) never reaches some of these.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl<T> OwnedTable<T> {
    /// Insert `value` under `id`, owned by `owner`. Returns any entry it replaced.
    pub(crate) fn insert(&mut self, id: u64, owner: u64, value: T) -> Option<T> {
        let previous = self.remove(&id);
        self.by_owner.entry(owner).or_default().insert(id);
        self.entries.insert(id, Slot { owner, value });
        previous
    }

    pub(crate) fn remove(&mut self, id: &u64) -> Option<T> {
        let slot = self.entries.remove(id)?;
        self.unindex(slot.owner, *id);
        Some(slot.value)
    }

    pub(crate) fn get(&self, id: &u64) -> Option<&T> {
        self.entries.get(id).map(|slot| &slot.value)
    }

    // Used by the TCP/TLS host (`net`), which a wasm32 build does not carry.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn get_mut(&mut self, id: &u64) -> Option<&mut T> {
        self.entries.get_mut(id).map(|slot| &mut slot.value)
    }

    // Used by the TCP/TLS host (`net`), which a wasm32 build does not carry.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn contains_key(&self, id: &u64) -> bool {
        self.entries.contains_key(id)
    }

    /// The ids `owner` holds, in no order — O(what it owns).
    // Used by the tree-sitter host (`treesit`), which a build without it does not carry.
    #[cfg_attr(not(feature = "treesit"), allow(dead_code))]
    pub(crate) fn owned_by(&self, owner: u64) -> Vec<u64> {
        self.by_owner
            .get(&owner)
            .map_or_default(|ids| ids.iter().copied().collect())
    }

    /// The process that owns entry `id`, or `None` for an unknown id.
    // Used by the tree-sitter host (`treesit`), which a build without it does not carry.
    #[cfg_attr(not(feature = "treesit"), allow(dead_code))]
    pub(crate) fn owner_of(&self, id: &u64) -> Option<u64> {
        self.entries.get(id).map(|slot| slot.owner)
    }

    /// Hand entry `id` to `owner`. A no-op for an unknown id.
    // Used by the TCP/TLS host (`net`), which a wasm32 build does not carry.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn set_owner(&mut self, id: u64, owner: u64) {
        let Some(slot) = self.entries.get_mut(&id) else {
            return;
        };
        let previous = std::mem::replace(&mut slot.owner, owner);
        if previous != owner {
            self.unindex(previous, id);
            self.by_owner.entry(owner).or_default().insert(id);
        }
    }

    /// Remove and return every entry `owner` holds — O(what it owns).
    pub(crate) fn remove_owned_by(&mut self, owner: u64) -> Vec<(u64, T)> {
        let Some(ids) = self.by_owner.remove(&owner) else {
            return Vec::new();
        };
        ids.into_iter()
            .filter_map(|id| self.entries.remove(&id).map(|slot| (id, slot.value)))
            .collect()
    }

    /// Remove and return every entry.
    // Used by the TCP/TLS host (`net`), which a wasm32 build does not carry.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn drain(&mut self) -> Vec<(u64, T)> {
        self.by_owner.clear();
        self.entries
            .drain()
            .map(|(id, slot)| (id, slot.value))
            .collect()
    }

    fn unindex(&mut self, owner: u64, id: u64) {
        if let Some(ids) = self.by_owner.get_mut(&owner) {
            ids.remove(&id);
            if ids.is_empty() {
                self.by_owner.remove(&owner);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_index_follows_insert_remove_and_hand_off() {
        let mut table: OwnedTable<&str> = OwnedTable::default();
        table.insert(1, 10, "a");
        table.insert(2, 10, "b");
        table.insert(3, 20, "c");
        assert_eq!(table.remove(&1), Some("a"));
        table.set_owner(3, 10);
        let mut owned = table.remove_owned_by(10);
        owned.sort();
        assert_eq!(owned, vec![(2, "b"), (3, "c")]);
        assert!(table.remove_owned_by(20).is_empty(), "3 moved away from 20");
        assert!(table.entries.is_empty() && table.by_owner.is_empty());
    }

    #[test]
    fn replacing_an_entry_moves_it_to_the_new_owner() {
        let mut table: OwnedTable<u8> = OwnedTable::default();
        table.insert(1, 10, 0);
        assert_eq!(table.insert(1, 20, 1), Some(0));
        assert!(table.remove_owned_by(10).is_empty());
        assert_eq!(table.remove_owned_by(20), vec![(1, 1)]);
    }
}
