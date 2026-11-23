//! The hasher for the runtime's hot internal tables.
//!
//! `std`'s default `RandomState` is SipHash-1-3: built to resist collision attacks on
//! untrusted keys, and several times slower than it needs to be on the short keys the
//! kernel hashes most — symbol spellings, packed slab indices, heap addresses. On bedit's
//! warm start it was about 3% of the main thread (the symbol interner, the source-position
//! maps, the GC's visited sets), and more on a build from source.
//!
//! `foldhash`'s `fast::RandomState` keeps the property that matters: it is seeded per
//! process, so a symbol name read from untrusted input (`reflect/read-string` interns what
//! it reads) cannot be chosen to collide. It is not a cryptographic hash, and nothing here
//! needs one.
//!
//! Use these for a table on a hot path. A map built once, or one whose iteration order
//! leaks into output, gains nothing from switching.

/// A `HashMap` with the fast seeded hasher. Build with `FastMap::default()`.
pub type FastMap<K, V> = std::collections::HashMap<K, V, foldhash::fast::RandomState>;

/// A `HashSet` with the fast seeded hasher. Build with `FastSet::default()`.
pub type FastSet<T> = std::collections::HashSet<T, foldhash::fast::RandomState>;
