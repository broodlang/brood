//! Lock helpers that recover from poisoning rather than propagating the panic.
//!
//! Standard `Mutex::lock()` / `RwLock::read()` / `RwLock::write()` return a
//! `Result` whose `Err` carries a guard for a *poisoned* lock — meaning a
//! panic occurred while a guard was held. Calling `.unwrap()` then turns a
//! single bad `Drop` (e.g. a `panic!` inside any code holding a global lock)
//! into a cascade: every subsequent take of that lock panics too, and the
//! runtime's `MONITORS` / `NODES` / `REGISTRY` / etc. tables become
//! permanently unusable.
//!
//! All this module's helpers return the inner guard regardless. Brood's
//! globals are all append-only or replace-only tables (no half-mutated
//! invariants a poisoned guard could observe partial state of), so trading
//! the panic for "carry on, possibly with the last write missing" matches
//! the runtime's "keep the system up" philosophy. Mirrors the `ids()`
//! pattern already used by the symbol interner (`core/value.rs`).

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Take a `Mutex` lock, recovering from poisoning. Equivalent to
/// `m.lock().unwrap_or_else(|e| e.into_inner())` — that is, never panics.
#[inline]
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Take a `RwLock` read lock, recovering from poisoning.
#[inline]
pub fn read<T>(m: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

/// Take a `RwLock` write lock, recovering from poisoning.
#[inline]
pub fn write<T>(m: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}
