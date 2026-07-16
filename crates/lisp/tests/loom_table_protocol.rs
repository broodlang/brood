//! Loom model-check of the dense-table **migration protocol** (`table.rs`,
//! devlog 2026-07-16): lock-free per-slot ops racing a dense→hashed migration.
//!
//! The REAL slots live in an mmap'd region of `std` atomics that loom cannot
//! instrument (loom only tracks its own types), so this is a **faithful
//! miniature** of the protocol — the same op sequences as `table::put` /
//! `incr` / `get` / `migrate_to_hashed`, transliterated onto loom atomics —
//! not the production code itself. What it proves: under EVERY interleaving
//! loom can generate, the protocol's linearizability claims hold (no lost
//! puts, exact increments, gets never observe torn state). Keep this file in
//! sync with the protocol comment on `table::Store` when the logic changes.
//!
//! Run: `make loom` (cargo test -p brood --release --features brood/loom-model
//! --test loom_table_protocol).
#![cfg(feature = "loom-model")]

use loom::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering::SeqCst};
use loom::sync::Mutex;
use loom::thread;
use std::collections::HashMap;
use std::sync::Arc;

const EMPTY: u64 = 0;
const MOVED: u64 = 5;
const INT_TAG: u64 = 0b100;

fn enc(n: u64) -> u64 {
    (n << 3) | INT_TAG
}
fn dec(s: u64) -> u64 {
    s >> 3
}

struct Model {
    slots: Vec<AtomicU64>,
    dense: AtomicBool,
    hashed: Mutex<Option<HashMap<usize, u64>>>,
}

impl Model {
    fn new(n: usize) -> Self {
        Model {
            slots: (0..n).map(|_| AtomicU64::new(EMPTY)).collect(),
            dense: AtomicBool::new(true),
            hashed: Mutex::new(None),
        }
    }

    /// `table::Store::migrate_to_hashed`, verbatim in miniature.
    fn migrate_locked(&self, g: &mut Option<HashMap<usize, u64>>) {
        self.dense.store(false, SeqCst);
        // Store→load ordering fence: the real code's SeqCst ops carry the C11
        // SC total order, which loom 0.7 does NOT model for plain store/load
        // pairs (see `sc_litmus_store_buffering` — loom explores executions SC
        // forbids). Explicit SeqCst FENCES express the same ordering in the
        // fragment loom models faithfully; on hardware they are subsumed by
        // the locked RMWs / SC accesses the real code already performs.
        fence(SeqCst);
        let mut map = HashMap::new();
        for (k, slot) in self.slots.iter().enumerate() {
            if slot.load(SeqCst) == EMPTY {
                continue;
            }
            let s = slot.swap(MOVED, SeqCst);
            if s != EMPTY && s != MOVED {
                map.insert(k, dec(s));
            }
        }
        *g = Some(map);
    }

    /// `table::put`'s dense fast path + hashed fallback.
    fn put(&self, k: usize, v: u64) {
        if self.dense.load(SeqCst) {
            let old = self.slots[k].swap(enc(v), SeqCst);
            fence(SeqCst); // swap→flag-load order (see migrate_locked's note)
            if old != MOVED && self.dense.load(SeqCst) {
                return;
            }
        }
        let mut g = self.hashed.lock().unwrap();
        if g.is_none() {
            self.migrate_locked(&mut g);
        }
        g.as_mut().unwrap().insert(k, v);
    }

    /// `table::incr`'s dense CAS loop + the ambiguous-case resolution.
    fn incr(&self, k: usize) -> u64 {
        if self.dense.load(SeqCst) {
            let slot = &self.slots[k];
            let mut cur = slot.load(SeqCst);
            loop {
                if cur == MOVED {
                    break;
                }
                let cur_int = if cur == EMPTY { 0 } else { dec(cur) };
                let next = cur_int + 1;
                match slot.compare_exchange(cur, enc(next), SeqCst, SeqCst) {
                    Ok(_) => {
                        fence(SeqCst); // CAS→flag-load order
                        if self.dense.load(SeqCst) {
                            return next;
                        }
                        // CAS landed but a migration started: resolve under its lock.
                        let mut g = self.hashed.lock().unwrap();
                        if slot.load(SeqCst) == MOVED {
                            return next; // the migrator captured our word
                        }
                        // skipped as EMPTY before our CAS — re-execute on the map
                        let map = g.as_mut().expect("flag false ⇒ map published");
                        let n = map.get(&k).copied().unwrap_or(0) + 1;
                        map.insert(k, n);
                        return n;
                    }
                    Err(actual) => cur = actual,
                }
            }
        }
        let mut g = self.hashed.lock().unwrap();
        if g.is_none() {
            self.migrate_locked(&mut g);
        }
        let map = g.as_mut().unwrap();
        let n = map.get(&k).copied().unwrap_or(0) + 1;
        map.insert(k, n);
        n
    }

    /// `table::get`'s dense fast path + hashed fallback (post-run, single-threaded
    /// callers also use it — the protocol must serve both).
    fn get(&self, k: usize) -> Option<u64> {
        if self.dense.load(SeqCst) {
            let s = self.slots[k].load(SeqCst);
            fence(SeqCst); // slot-load→flag-load order
            if s != MOVED && self.dense.load(SeqCst) {
                return if s == EMPTY { None } else { Some(dec(s)) };
            }
        }
        let mut g = self.hashed.lock().unwrap();
        if g.is_none() {
            self.migrate_locked(&mut g);
        }
        g.as_ref().unwrap().get(&k).copied()
    }

    fn force_migrate(&self) {
        let mut g = self.hashed.lock().unwrap();
        if g.is_none() {
            self.migrate_locked(&mut g);
        }
    }
}

/// Two writers to DISJOINT keys race a migration: both writes must survive.
#[test]
fn puts_survive_migration() {
    loom::model(|| {
        let m = Arc::new(Model::new(2));
        let m1 = m.clone();
        let t1 = thread::spawn(move || m1.put(0, 7));
        let m2 = m.clone();
        let t2 = thread::spawn(move || m2.force_migrate());
        m.put(1, 9);
        t1.join().unwrap();
        t2.join().unwrap();
        let s0 = m.slots[0].load(SeqCst);
        let s1 = m.slots[1].load(SeqCst);
        let dense = m.dense.load(SeqCst);
        let map = m.hashed.lock().unwrap().clone();
        assert_eq!(
            m.get(0),
            Some(7),
            "writer 0 lost: slots=[{s0},{s1}] dense={dense} map={map:?}"
        );
        assert_eq!(m.get(1), Some(9), "writer 1 lost across migration");
    });
}

/// Two writers to the SAME key race a migration: last-writer-wins — the final
/// value is one of the two, never lost, never torn.
#[test]
fn same_key_put_race_linearizes() {
    loom::model(|| {
        let m = Arc::new(Model::new(1));
        let m1 = m.clone();
        let t1 = thread::spawn(move || m1.put(0, 1));
        let m2 = m.clone();
        let t2 = thread::spawn(move || m2.force_migrate());
        m.put(0, 2);
        t1.join().unwrap();
        t2.join().unwrap();
        let v = m.get(0);
        assert!(
            v == Some(1) || v == Some(2),
            "torn/lost same-key put: {v:?}"
        );
    });
}

/// Two increments race a migration: the count is EXACTLY 2 (the deopt-rerun
/// class of bug — a lost or doubled increment — is what this hunts).
#[test]
fn incr_exact_across_migration() {
    loom::model(|| {
        let m = Arc::new(Model::new(1));
        let m1 = m.clone();
        let t1 = thread::spawn(move || {
            m1.incr(0);
        });
        let m2 = m.clone();
        let t2 = thread::spawn(move || m2.force_migrate());
        m.incr(0);
        t1.join().unwrap();
        t2.join().unwrap();
        assert_eq!(m.get(0), Some(2), "increment lost or doubled");
    });
}

/// A read racing a put and a migration returns a coherent value: absent or one
/// of the written values — never MOVED leaking out, never a torn word.
#[test]
fn get_never_observes_protocol_internals() {
    loom::model(|| {
        let m = Arc::new(Model::new(1));
        let m1 = m.clone();
        let t1 = thread::spawn(move || m1.put(0, 3));
        let m2 = m.clone();
        let t2 = thread::spawn(move || m2.force_migrate());
        let v = m.get(0);
        assert!(v.is_none() || v == Some(3), "incoherent read: {v:?}");
        t1.join().unwrap();
        t2.join().unwrap();
        assert_eq!(m.get(0), Some(3));
    });
}

/// SC litmus (store buffering) WITH FENCES: both threads reading 0 is
/// impossible. NOTE: without the fences this FAILS under loom 0.7 — loom does
/// not model the C11 SC total order for plain SeqCst store/load pairs, which
/// is exactly why the model above expresses its orderings as fences.
#[test]
fn sc_litmus_store_buffering() {
    loom::model(|| {
        let x = Arc::new(AtomicU64::new(0));
        let y = Arc::new(AtomicU64::new(0));
        let x1 = x.clone();
        let y1 = y.clone();
        let t = thread::spawn(move || {
            x1.store(1, SeqCst);
            fence(SeqCst);
            y1.load(SeqCst)
        });
        y.store(1, SeqCst);
        fence(SeqCst);
        let rx = x.load(SeqCst);
        let ry = t.join().unwrap();
        assert!(
            !(rx == 0 && ry == 0),
            "SC violated: both loads saw 0 (store buffering observed)"
        );
    });
}
