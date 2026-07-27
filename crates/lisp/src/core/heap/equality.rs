//! Value equality, comparison, hashing (child of heap).
use super::*;

/// Order a bignum against a float for `value_cmp`'s heterogeneous numeric
/// fallback, **exactly**. A `NaN` float is `Equal` (matching the `Float`/`Float`
/// arm's `unwrap_or(Equal)`); ±∞ is beyond any finite bignum. Otherwise both
/// sides convert to `BigDecimal` — every finite `f64` is a dyadic rational, so
/// its decimal form is exact, as is the bignum's — and we compare in base 10.
/// This avoids the precision loss of the old `BigInt::to_f64` path, which could
/// misorder a bignum that lies inside f64's range but isn't exactly representable
/// (e.g. between 2^53 and 2^1024) against a near-equal float.
fn bigint_cmp_float(b: &num_bigint::BigInt, f: f64) -> std::cmp::Ordering {
    bigdecimal_cmp_float(&bigdecimal::BigDecimal::from(b.clone()), f)
}

/// Order a `BigDecimal` against a float **exactly** — the shared kernel of the
/// bignum-vs-float and decimal-vs-float `value_cmp` arms. `NaN` is `Equal`
/// (matching `Float`/`Float`'s `unwrap_or(Equal)`); ±∞ is beyond any finite
/// value. Otherwise compare in base 10: every finite f64 is a dyadic rational,
/// so its `BigDecimal` form is exact — no rounding either side.
fn bigdecimal_cmp_float(b: &bigdecimal::BigDecimal, f: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if f.is_nan() {
        return Ordering::Equal;
    }
    if f.is_infinite() {
        return if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    // Finite f64 always converts (only NaN/∞ fail, handled above).
    b.cmp(&bigdecimal::BigDecimal::try_from(f).expect("finite f64 → BigDecimal"))
}

/// Tag ranks for `value_cmp`'s heterogeneous fallback. The order is mostly
/// aesthetic — what matters is that it's *fixed* so a heterogeneous sort is
/// reproducible. Numbers come first (most common), then strings/keywords/
/// symbols (text), then collections, then everything else.
fn tag_rank(v: Value) -> u8 {
    match v.unpack() {
        ValueRef::Nil => 0,
        ValueRef::Bool(_) => 1,
        ValueRef::Int(_) | ValueRef::BigInt(_) | ValueRef::Float(_) | ValueRef::Decimal(_) => 2,
        ValueRef::Str(_) => 3,
        ValueRef::Keyword(_) => 4,
        ValueRef::Sym(_) => 5,
        ValueRef::Pair(_) => 6,
        // A range sorts among lists (it is one, lazily).
        ValueRef::Range(_) => 6,
        // A lazy seq-view sorts among lists too (it is one, lazily).
        ValueRef::SeqView(_) => 6,
        ValueRef::Vector(_) => 7,
        ValueRef::Map(_) => 8,
        // A set sorts among collections, just past maps (its own rank so a
        // heterogeneous set-vs-map fallback never needs a same-rank tiebreak).
        ValueRef::Set(_) => 19,
        ValueRef::Fn(_) => 9,
        ValueRef::Native(_) => 10,
        ValueRef::Macro(_) => 11,
        ValueRef::Ref(_) => 12,
        ValueRef::Pid { .. } => 13,
        ValueRef::Rope(_) => 14,
        ValueRef::Socket(_) => 15,
        ValueRef::Subprocess(_) => 16,
        ValueRef::Table(_) => 18,
        ValueRef::Bytes(_) => 20,
    }
}

impl Heap {
    // ===== Value equality, comparison, and hashing =============================

    /// A `u64` hash of `v` consistent with [`Heap::equal`]: two values that
    /// `equal` agrees on must hash to the same number. Used by the CHAMP map
    /// (ADR-040) to drive trie navigation — top 4 bits pick the root slot,
    /// next 4 the child, …
    ///
    /// Subtle bits the consistency proof rides on:
    /// - `Float(0.0)` and `Float(-0.0)` hash the same (they compare equal).
    /// - `NaN` ≠ `NaN` per IEEE-754, so two `NaN` keys won't be `equal` and
    ///   needn't hash the same — but a single canonical bit pattern still
    ///   keeps the trie well-typed; pick `u64::MAX` so any NaN routes to one
    ///   leaf where it'll fail the `equal` check anyway.
    /// - Maps are insertion-order-independent: the hash XORs each entry's
    ///   `(k, v)` hash so order doesn't matter (XOR is commutative).
    /// - Pair / Vector hashes feed children into a `DefaultHasher` so
    ///   structure matters; lists with the same `equal` shape hash the same
    ///   regardless of which `Cons` cells they're built from.
    /// - Region bits in handles are ignored — `hash_value` works on
    ///   *structure*, so a LOCAL pair and its PRELUDE-retagged twin land at
    ///   the same key.
    pub fn hash_value(&self, v: Value) -> u64 {
        use std::hash::Hasher;
        // Fast path for immediate scalars — the overwhelmingly common table/map key
        // (a `sieve` int, a counter, an id, a `persistent-map` key). Building a fresh
        // SipHash `DefaultHasher` (128-bit init + finalize) to hash one integer is
        // ~40 ns of pure overhead paid on *every* table/map op; a splitmix64 finalize
        // is a handful of cycles. The hash is internal (buckets/trie slots resolve
        // exact equality with `equal`), so any well-distributed deterministic function
        // is correct — this only re-distributes, and equal scalars still hash equal.
        // A distinct per-type salt keeps `Int(0)`/`Bool(false)`/`Nil` from colliding.
        // Compound values fall through to the unchanged `DefaultHasher` path.
        match v.unpack() {
            ValueRef::Int(i) => return Self::hash_int(i),
            ValueRef::Bool(b) => return Self::mix64(b as u64 ^ 0xD1B5_4A32_D192_ED03),
            ValueRef::Nil => return Self::mix64(0xA0761D6478BD642F),
            _ => {}
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash_value_into(v, &mut h);
        h.finish()
    }

    /// The structural hash of an `Int`, heap-free — the exact value
    /// [`hash_value`](Self::hash_value) computes for `Value::Int` (that fast path
    /// delegates here). Public for `table`'s dense→hashed migration, which hashes
    /// int keys without a heap in scope.
    #[inline]
    pub fn hash_int(i: i64) -> u64 {
        Self::mix64(i as u64 ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// splitmix64 finalizer: a fast, well-distributed `u64 -> u64` avalanche mix.
    #[inline]
    fn mix64(mut z: u64) -> u64 {
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn hash_value_into<H: std::hash::Hasher>(&self, v: Value, h: &mut H) {
        // Deep-car-nesting guard — see `WALKER_RED_ZONE`. Scalars never recurse,
        // so skip the check for them (the common map/table key).
        if matches!(
            v.unpack(),
            ValueRef::Nil
                | ValueRef::Bool(_)
                | ValueRef::Int(_)
                | ValueRef::Float(_)
                | ValueRef::Sym(_)
                | ValueRef::Keyword(_)
        ) {
            return self.hash_value_into_grown(v, h);
        }
        stacker::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
            self.hash_value_into_grown(v, h)
        })
    }

    fn hash_value_into_grown<H: std::hash::Hasher>(&self, v: Value, h: &mut H) {
        use std::hash::{Hash, Hasher};
        // A leading byte tags the variant so a `Sym(0)` and an `Int(0)` never
        // collide on the *exact* same hash by accident.
        match v.unpack() {
            ValueRef::Nil => 0u8.hash(h),
            ValueRef::Bool(b) => {
                1u8.hash(h);
                b.hash(h);
            }
            ValueRef::Int(i) => {
                2u8.hash(h);
                i.hash(h);
            }
            ValueRef::BigInt(id) => {
                // A distinct tag byte (17) from Int's (2) so a BigInt never
                // collides with an Int — they're numerically disjoint by the
                // normalize invariant, so equal values still hash equal. Hash
                // the value's own bytes (sign + magnitude) so two equal bignums
                // (different handles) hash the same.
                17u8.hash(h);
                let (sign, bytes) = self.bigint(id).to_bytes_le();
                let sign_byte: u8 = match sign {
                    num_bigint::Sign::Minus => 0,
                    num_bigint::Sign::NoSign => 1,
                    num_bigint::Sign::Plus => 2,
                };
                sign_byte.hash(h);
                bytes.hash(h);
            }
            ValueRef::Bytes(id) => {
                // Distinct fresh tag byte (21) + the raw bytes, so two
                // equal byte values hash the same and never collide with another type.
                21u8.hash(h);
                self.bytes(id).as_bytes().hash(h);
            }
            ValueRef::Decimal(id) => {
                // Distinct fresh tag byte (22). CRITICAL: numerically equal decimals
                // (`1.5M` / `1.50M`) must hash equal, but BigDecimal's representation
                // (and hash) differs by scale. `normalized()` strips trailing zeros to
                // a canonical form, so equal values hash the same — matching the
                // normalized-equality used in `equal` below.
                22u8.hash(h);
                self.decimal(id).normalized().to_string().hash(h);
            }
            ValueRef::Float(f) => {
                3u8.hash(h);
                if f.is_nan() {
                    u64::MAX.hash(h);
                } else if f == 0.0 {
                    // 0.0 and -0.0 compare equal; canonicalise to +0.0 bits.
                    0u64.hash(h);
                } else {
                    f.to_bits().hash(h);
                }
            }
            ValueRef::Sym(s) => {
                4u8.hash(h);
                s.hash(h);
            }
            ValueRef::Keyword(s) => {
                5u8.hash(h);
                s.hash(h);
            }
            ValueRef::Str(id) => {
                6u8.hash(h);
                self.string(id).hash(h);
            }
            ValueRef::Pair(id) => {
                7u8.hash(h);
                // Walk the cdr spine iteratively (matches `equal`'s loop).
                let mut cur = id;
                loop {
                    let (car, cdr) = self.pair(cur);
                    self.hash_value_into(car, h);
                    match cdr.unpack() {
                        ValueRef::Pair(next) => cur = next,
                        other => {
                            // Marker so a 1-pair `(a . b)` doesn't hash the
                            // same as a 2-pair `(a b)` (whose cdr ends Nil).
                            0xFFu8.hash(h);
                            self.hash_value_into(other, h);
                            break;
                        }
                    }
                }
            }
            // A range hashes byte-for-byte like the proper list it stands in for
            // (it must — `(= (range 5) (list 0 1 2 3 4))`, so they share a hash):
            // the same `7u8` list tag, each `Int` element hashed via the same
            // path, then the `0xFF` + `Nil` end-marker a proper list emits.
            ValueRef::Range(id) => {
                7u8.hash(h);
                let (lo, hi, step) = self.range_parts(id);
                let mut i = lo;
                while if step > 0 { i < hi } else { i > hi } {
                    self.hash_value_into(Value::int(i), h);
                    i = match i.checked_add(step) {
                        Some(v) => v,
                        None => break,
                    };
                }
                0xFFu8.hash(h);
                self.hash_value_into(Value::nil(), h);
            }
            // A lazy seq-view cannot be realised here (no evaluator to run its
            // transducer), so it hashes to a single sentinel bucket — consistent
            // with `equal`'s identity fallback below (two views are `equal` only
            // when the same handle, and same handle ⇒ same hash). The prelude
            // realises a view before it can reach a hash-keyed map in normal use;
            // this is the safe, never-panic fallback for an escaped raw view.
            ValueRef::SeqView(_) => {
                0x5E_u8.hash(h);
            }
            ValueRef::Vector(id) => {
                8u8.hash(h);
                let xs = self.vector(id);
                (xs.len() as u64).hash(h);
                for &x in xs.iter() {
                    self.hash_value_into(x, h);
                }
            }
            ValueRef::Map(id) => {
                9u8.hash(h);
                // Order-insensitive: XOR each entry's hash into an
                // accumulator (XOR is commutative — works regardless of
                // CHAMP trie shape). Mix in size so `{}` ≠ `{a a}` even
                // if the per-entry hash ever conspired to 0.
                let mut acc: u64 = 0;
                let size = self.map_size(id);
                self.fold_entries(id, &mut |k, vv| {
                    let mut sub = std::collections::hash_map::DefaultHasher::new();
                    self.hash_value_into(k, &mut sub);
                    self.hash_value_into(vv, &mut sub);
                    acc ^= sub.finish();
                });
                (size as u64).hash(h);
                acc.hash(h);
            }
            ValueRef::Set(id) => {
                // Order-insensitive like Map (XOR the per-element hashes), but a
                // distinct tag byte (23) and **keys only** (a set's backing values are
                // all `true`) — so a set never hashes the same as the map with the same
                // keys, matching that a set is never `equal` to a map (ADR-060).
                23u8.hash(h);
                let mut acc: u64 = 0;
                let size = self.map_size(id);
                self.fold_entries(id, &mut |k, _vv| {
                    let mut sub = std::collections::hash_map::DefaultHasher::new();
                    self.hash_value_into(k, &mut sub);
                    acc ^= sub.finish();
                });
                (size as u64).hash(h);
                acc.hash(h);
            }
            ValueRef::Fn(id) => {
                10u8.hash(h);
                id.0.hash(h);
            }
            ValueRef::Macro(id) => {
                11u8.hash(h);
                id.0.hash(h);
            }
            ValueRef::Native(id) => {
                12u8.hash(h);
                id.0.hash(h);
            }
            ValueRef::Ref(id) => {
                13u8.hash(h);
                id.hash(h);
            }
            ValueRef::Pid { node, id } => {
                14u8.hash(h);
                // Normalize the node stamp for LOCAL pids — `nonode` (pre-node-start)
                // and the current node name are the same runtime — so a pid captured
                // before `node-start` hashes (and compares: see `equal`) the same as
                // the identical process's post-node-start `(self)`. Both stamps map
                // to one sentinel, so a pid-keyed map entry survives the node coming
                // up. Remote pids hash by their real node.
                if crate::dist::is_local(node) {
                    u32::MAX.hash(h);
                } else {
                    node.hash(h);
                }
                id.hash(h);
            }
            ValueRef::Rope(id) => {
                15u8.hash(h);
                // Hash by text content so two ropes with equal text hash equal,
                // consistent with `equal` below. Materialise the whole string:
                // hashing chunk-by-chunk would frame each chunk (str's Hash adds
                // a terminator), so equal text under different chunk boundaries
                // could hash differently — breaking the equal⇒same-hash contract.
                // Only paid when a rope is actually used as a map key (rare).
                self.rope(id).to_string().hash(h);
            }
            ValueRef::Socket(id) => {
                16u8.hash(h);
                id.hash(h);
            }
            ValueRef::Subprocess(id) => {
                19u8.hash(h);
                id.hash(h);
            }
            ValueRef::Table(id) => {
                // Identity-hashed (tag 20; a table is shared mutable state addressed
                // by its registry handle, like a socket — compared by identity).
                20u8.hash(h);
                id.hash(h);
            }
        }
    }

    /// Structural equality (the basis of `=`). Functions/macros/natives compare
    /// by identity (same handle).
    ///
    /// Floats compare by IEEE value, so `-0.0 = 0.0` is true and `nan = nan` is
    /// false — the least-surprising arithmetic semantics (not bitwise equality).
    /// Structural equality of a range against a list spine, element by element,
    /// without materialising the range. Both must run out together.
    fn range_eq_list(&self, rid: VecId, mut lst: Value) -> bool {
        let (lo, hi, step) = self.range_parts(rid);
        let mut i = lo;
        loop {
            let in_range = if step > 0 { i < hi } else { i > hi };
            match (in_range, lst.unpack()) {
                (false, ValueRef::Nil) => return true,
                (true, ValueRef::Pair(p)) => {
                    let (car, cdr) = self.pair(p);
                    if !self.equal(Value::int(i), car) {
                        return false;
                    }
                    i += step;
                    lst = cdr;
                }
                // One ran out before the other (or the list is improper).
                _ => return false,
            }
        }
    }

    /// Structural equality of two ranges. An arithmetic sequence is fixed by its
    /// first element, length, and (for length ≥ 2) its step — so this is O(1).
    fn range_eq_range(&self, x: VecId, y: VecId) -> bool {
        let (lo1, _, s1) = self.range_parts(x);
        let (lo2, _, s2) = self.range_parts(y);
        let n1 = self.range_len(x);
        let n2 = self.range_len(y);
        n1 == n2 && lo1 == lo2 && (n1 < 2 || s1 == s2)
    }

    pub fn equal(&self, a: Value, b: Value) -> bool {
        // Fast path: identical immediates/handles (and the common scalar pairs)
        // never need the guard; only compound shapes recurse. Checking here
        // keeps the per-call cost of the deep-nesting guard off the hot
        // scalar-compare path (map/table lookups hash to scalars mostly).
        match (a.unpack(), b.unpack()) {
            (ValueRef::Int(x), ValueRef::Int(y)) => return x == y,
            (ValueRef::Sym(x), ValueRef::Sym(y)) => return x == y,
            (ValueRef::Keyword(x), ValueRef::Keyword(y)) => return x == y,
            (ValueRef::Nil, ValueRef::Nil) => return true,
            (ValueRef::Bool(x), ValueRef::Bool(y)) => return x == y,
            (ValueRef::Float(x), ValueRef::Float(y)) => return x == y,
            _ => {}
        }
        // Deep-car-nesting guard — see `WALKER_RED_ZONE`.
        stacker::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
            self.equal_grown(a, b)
        })
    }

    fn equal_grown(&self, a: Value, b: Value) -> bool {
        use Value::*; // Stage 1: -> use ValueRef::*; (matched via .unpack())
        match (a.unpack(), b.unpack()) {
            (Nil, Nil) => true,
            // A range equals the list it stands in for (and another range),
            // compared element-wise without materialising either.
            (Range(x), Range(y)) => self.range_eq_range(x, y),
            (Range(x), Pair(_)) => self.range_eq_list(x, b),
            (Pair(_), Range(y)) => self.range_eq_list(y, a),
            // A lazy seq-view can't be realised here (no evaluator for its
            // transducer), so it compares only by handle identity — the safe,
            // never-panic fallback. The prelude `=` realises a view first, so a
            // structural compare against a list/another view goes through the
            // realised lists; this arm only catches an escaped raw view.
            (SeqView(x), SeqView(y)) => x == y,
            (Bool(x), Bool(y)) => x == y,
            (Int(x), Int(y)) => x == y,
            // Two bignums compare by value. An Int vs a BigInt is never equal —
            // the normalize invariant keeps their ranges disjoint — so those
            // mixed pairs fall through to `_ => false`.
            (BigInt(x), BigInt(y)) => self.bigint(x) == self.bigint(y),
            // Two decimals are equal iff their values are numerically equal —
            // `1.5M` == `1.50M`. Compare via `normalized()` (the same canonical form
            // hashed above) so equality and hashing agree. A decimal is its own type,
            // so a Decimal vs an Int/Float falls through to `_ => false`.
            (Decimal(x), Decimal(y)) => {
                self.decimal(x).normalized() == self.decimal(y).normalized()
            }
            (Bytes(x), Bytes(y)) => self.bytes(x).as_bytes() == self.bytes(y).as_bytes(),
            (Float(x), Float(y)) => x == y,
            (Sym(x), Sym(y)) => x == y,
            (Keyword(x), Keyword(y)) => x == y,
            (Str(x), Str(y)) => self.string(x) == self.string(y),
            // Walk the `cdr` spine iteratively so comparing long lists doesn't
            // recurse their length deep; recursion stays bounded by `car` nesting.
            (Pair(x), Pair(y)) => {
                let (mut x, mut y) = (x, y);
                loop {
                    let (a0, a1) = self.pair(x);
                    let (b0, b1) = self.pair(y);
                    if !self.equal(a0, b0) {
                        break false;
                    }
                    match (a1.unpack(), b1.unpack()) {
                        (Pair(nx), Pair(ny)) => {
                            x = nx;
                            y = ny;
                        }
                        _ => break self.equal(a1, b1),
                    }
                }
            }
            (Vector(x), Vector(y)) => {
                let xs = self.vector(x);
                let ys = self.vector(y);
                xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(&p, &q)| self.equal(p, q))
            }
            // Maps: CHAMP is *canonical* under structural equality, so two
            // equal maps have identical trie shapes — same `data_map` /
            // `node_map` / `is_collision` bits at every node. Recurse
            // structurally; collision leaves fall back to set-equality on
            // their entries (their internal order isn't canonical).
            (Map(x), Map(y)) => self.map_equal(x, y),
            // Two sets are equal iff they hold the same elements. Backed by the same
            // canonical CHAMP as maps (element→`true`), so this reduces to map
            // equality on the underlying trie. A set is never equal to a map — that
            // mixed pair falls through to `_ => false` (distinct kinds, ADR-060).
            (Set(x), Set(y)) => self.map_equal(x, y),
            (Fn(x), Fn(y)) => x == y,
            (Macro(x), Macro(y)) => x == y,
            (Native(x), Native(y)) => x == y,
            (Ref(x), Ref(y)) => x == y,
            // Pids are equal by node identity + local id (same process, anywhere).
            // Node identity is *normalized for local pids*: a pid captured BEFORE
            // `node-start` is stamped `nonode`, while the same process's `(self)`
            // afterwards carries the node name — both mean "this runtime" (exactly
            // as `dist::is_local` already treats them for routing), so they must
            // compare equal or every pre-node-start captured pid (a subscriber
            // list, a registry, a pid-keyed marker) silently stops matching the
            // moment the node comes up.
            (Pid { node: n1, id: i1 }, Pid { node: n2, id: i2 }) => {
                i1 == i2 && (n1 == n2 || (crate::dist::is_local(n1) && crate::dist::is_local(n2)))
            }
            // Ropes compare by text content (ropey's PartialEq walks chunks; no
            // full materialisation). Distinct handles to equal text are `=`.
            (Rope(x), Rope(y)) => self.rope(x) == self.rope(y),
            // Sockets are identity values — equal iff the same registry handle.
            (Socket(x), Socket(y)) => x == y,
            // Subprocesses are identity values — equal iff the same registry handle.
            (Subprocess(x), Subprocess(y)) => x == y,
            // Tables are shared mutable state — equal iff the same registry handle
            // (the same store), like a pid. Two distinct tables never compare equal,
            // even with equal contents (identity, not value).
            (Table(x), Table(y)) => x == y,
            _ => false,
        }
    }

    /// Structural equality between two closures — used *only* to dedup a
    /// hot-reload redefinition that didn't actually change the code (a
    /// save-without-change, or `nest format` rewriting the whole file) so it
    /// doesn't append a duplicate into the append-only RUNTIME region
    /// (docs/live-editing.md Stage 5). Deliberately **conservative**: it bails
    /// (returns `false`) on any closure that captured a *local* scope
    /// (`env.is_some()`), handling only the common top-level case where `env`
    /// resolves to the global per-process. Soundness rests on the asymmetry — a
    /// false "not equal" merely keeps today's behaviour (append, i.e. the leak),
    /// while a false "equal" would skip a real redefinition; identical params,
    /// body, optionals, rest, name and doc with no captured scope means the two
    /// closures are behaviourally identical, so "equal" is never false-positive.
    pub(crate) fn closures_structurally_equal(&self, a: ClosureId, b: ClosureId) -> bool {
        let ca = self.closure(a);
        let cb = self.closure(b);
        if ca.env.is_some() || cb.env.is_some() {
            return false;
        }
        ca.name == cb.name
            && ca.doc == cb.doc
            && ca.arms.len() == cb.arms.len()
            && ca.arms.iter().zip(cb.arms.iter()).all(|(aa, ab)| {
                aa.params == ab.params
                    && aa.rest == ab.rest
                    && aa.optionals.len() == ab.optionals.len()
                    && aa.body.len() == ab.body.len()
                    && aa
                        .optionals
                        .iter()
                        .zip(ab.optionals.iter())
                        .all(|((sa, da), (sb, db))| sa == sb && self.equal(*da, *db))
                    && aa
                        .body
                        .iter()
                        .zip(ab.body.iter())
                        .all(|(&x, &y)| self.equal(x, y))
            })
    }

    /// Equality between two CHAMP maps — canonical-form recursion. Two
    /// equal maps have the same node shape (same bitmaps, same children
    /// in slot order), so a structural walk bails on the first mismatch.
    /// Collision leaves fall back to set-equality on their entries (their
    /// internal order isn't canonical — two equally-content collision
    /// leaves can hold their entries in different positions).
    fn map_equal(&self, x: MapId, y: MapId) -> bool {
        let nx = self.map_node(x);
        let ny = self.map_node(y);
        if nx.size != ny.size {
            return false;
        }
        if nx.is_collision != ny.is_collision {
            return false;
        }
        if nx.is_collision {
            // Set-equality on entries. Collision leaves are tiny (entries
            // share the full 64-bit hash — astronomically rare), so O(n²)
            // is fine.
            if nx.data.len() != ny.data.len() {
                return false;
            }
            return nx.data.iter().all(|(k, v)| {
                ny.data
                    .iter()
                    .any(|(k2, v2)| self.equal(*k, *k2) && self.equal(*v, *v2))
            });
        }
        // Branch: same bitmaps → same slot occupancy → same shapes.
        if nx.data_map != ny.data_map || nx.node_map != ny.node_map {
            return false;
        }
        for ((k1, v1), (k2, v2)) in nx.data.iter().zip(ny.data.iter()) {
            if !self.equal(*k1, *k2) || !self.equal(*v1, *v2) {
                return false;
            }
        }
        for (&c1, &c2) in nx.children.iter().zip(ny.children.iter()) {
            if !self.map_equal(c1, c2) {
                return false;
            }
        }
        true
    }

    /// A total structural ordering for `(sort coll)`'s non-numeric fallback.
    /// **Not** Brood-visible as `<`/`compare` — that's a separate decision; this
    /// is just enough to give the sort builtin a defined order on heterogeneous
    /// values without throwing.
    ///
    /// Within a kind, ordering is the natural one: ints by `<`, floats by IEEE,
    /// mixed numerics by promotion (same compromise as `prim_lt`); strings/
    /// symbols/keywords by their text; pairs/vectors lexicographically;
    /// `nil` < `false` < `true`. Across kinds we use a fixed tag order
    /// (`tag_rank`) so a heterogeneous list still has *some* total order — the
    /// alternative is the current "throws on a vector" trap. Maps, fns,
    /// natives, macros, refs, pids fall through to a tag-rank compare (sorting
    /// them by content isn't well-defined here).
    pub fn value_cmp(&self, a: Value, b: Value) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        // Fast path + deep-car-nesting guard, mirroring `equal`/`hash_value_into`:
        // the cons *spine* is walked iteratively below, but a nest built in the
        // CAR (`(fold (fn (acc x) (list acc)) nil (range 200000))`) recurses one
        // native frame per level, and the reader's 256-level cap can't see a value
        // built at runtime. Unguarded this aborted the process on a guard page
        // (2026-07-27); scalars never recurse, so the check stays off the hot
        // sort/compare path.
        match (a.unpack(), b.unpack()) {
            (ValueRef::Int(x), ValueRef::Int(y)) => return x.cmp(&y),
            (ValueRef::Nil, ValueRef::Nil) => return Ordering::Equal,
            (ValueRef::Bool(x), ValueRef::Bool(y)) => return x.cmp(&y),
            (ValueRef::Float(x), ValueRef::Float(y)) => {
                return x.partial_cmp(&y).unwrap_or(Ordering::Equal)
            }
            _ => {}
        }
        stacker::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
            self.value_cmp_grown(a, b)
        })
    }

    fn value_cmp_grown(&self, a: Value, b: Value) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        use Value::*; // Stage 1: -> use ValueRef::*; (matched via .unpack())
        match (a.unpack(), b.unpack()) {
            (Nil, Nil) => Ordering::Equal,
            (Bool(x), Bool(y)) => x.cmp(&y),
            (Int(x), Int(y)) => x.cmp(&y),
            (Float(x), Float(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            (Int(x), Float(y)) => (x as f64).partial_cmp(&y).unwrap_or(Ordering::Equal),
            (Float(x), Int(y)) => x.partial_cmp(&(y as f64)).unwrap_or(Ordering::Equal),
            // Bignums: compare by value, with the other operand promoted. A
            // BigInt is always outside the i64 range, so a BigInt vs an Int is
            // ordered by the bignum's sign (it's strictly larger/smaller in
            // magnitude than any i64) — `BigInt::cmp` after promotion gives that
            // for free.
            (BigInt(_) | Int(_), BigInt(_) | Int(_)) => self.bigint_of(a).cmp(&self.bigint_of(b)),
            (BigInt(x), Float(y)) => bigint_cmp_float(&self.bigint(x), y),
            (Float(x), BigInt(y)) => bigint_cmp_float(&self.bigint(y), x).reverse(),
            // Decimals order by value; against an Int/BigInt promote both to
            // BigDecimal; against a Float compare exactly in base 10 (the f64 is
            // an exact decimal — so ordering is precise, unlike the arithmetic
            // tower's deliberate float contagion).
            (Decimal(x), Decimal(y)) => self.decimal(x).cmp(&self.decimal(y)),
            (Decimal(x), Int(y)) => self.decimal(x).cmp(&bigdecimal::BigDecimal::from(y)),
            (Int(x), Decimal(y)) => bigdecimal::BigDecimal::from(x).cmp(&self.decimal(y)),
            (Decimal(x), BigInt(y)) => self
                .decimal(x)
                .cmp(&bigdecimal::BigDecimal::from(self.bigint(y).clone())),
            (BigInt(x), Decimal(y)) => {
                bigdecimal::BigDecimal::from(self.bigint(x).clone()).cmp(&self.decimal(y))
            }
            (Decimal(x), Float(y)) => bigdecimal_cmp_float(&self.decimal(x), y),
            (Float(x), Decimal(y)) => bigdecimal_cmp_float(&self.decimal(y), x).reverse(),
            (Str(x), Str(y)) => self.string(x).cmp(&self.string(y)),
            // Symbols/keywords sort by spelling so it's stable and human-meaningful.
            (Sym(x), Sym(y)) | (Keyword(x), Keyword(y)) => {
                crate::core::value::symbol_name(x).cmp(&crate::core::value::symbol_name(y))
            }
            (Vector(x), Vector(y)) => {
                let xs: Vec<Value> = self.vector(x).to_vec();
                let ys: Vec<Value> = self.vector(y).to_vec();
                for (xv, yv) in xs.iter().zip(ys.iter()) {
                    match self.value_cmp(*xv, *yv) {
                        Ordering::Equal => continue,
                        o => return o,
                    }
                }
                xs.len().cmp(&ys.len())
            }
            // Lists: walk the cons spine like equal(). Empty list < non-empty.
            (Nil, Pair(_)) => Ordering::Less,
            (Pair(_), Nil) => Ordering::Greater,
            (Pair(x), Pair(y)) => {
                let (mut x, mut y) = (x, y);
                loop {
                    let (a0, a1) = self.pair(x);
                    let (b0, b1) = self.pair(y);
                    match self.value_cmp(a0, b0) {
                        Ordering::Equal => {}
                        o => return o,
                    }
                    match (a1.unpack(), b1.unpack()) {
                        (Pair(nx), Pair(ny)) => {
                            x = nx;
                            y = ny;
                        }
                        _ => return self.value_cmp(a1, b1),
                    }
                }
            }
            _ => tag_rank(a).cmp(&tag_rank(b)),
        }
    }
}

#[cfg(test)]
mod bigint_cmp_float_tests {
    use super::bigint_cmp_float;
    use num_bigint::BigInt;
    use std::cmp::Ordering;

    #[test]
    fn exact_against_a_float_inside_f64_range() {
        // 2^70 is exactly representable as f64; 2^70 ± 1 are NOT (each rounds to
        // 2^70). The old `BigInt::to_f64` path rounded the bignum and so called
        // all three Equal; the exact BigDecimal comparison orders them correctly.
        let p70 = BigInt::from(2u8).pow(70);
        let one = BigInt::from(1);
        let f = 2f64.powi(70);
        assert_eq!(bigint_cmp_float(&p70, f), Ordering::Equal);
        assert_eq!(bigint_cmp_float(&(&p70 + &one), f), Ordering::Greater);
        assert_eq!(bigint_cmp_float(&(&p70 - &one), f), Ordering::Less);
    }

    #[test]
    fn nan_is_equal_and_infinities_order_by_sign() {
        let b = BigInt::from(5);
        assert_eq!(bigint_cmp_float(&b, f64::NAN), Ordering::Equal);
        assert_eq!(bigint_cmp_float(&b, f64::INFINITY), Ordering::Less);
        assert_eq!(bigint_cmp_float(&b, f64::NEG_INFINITY), Ordering::Greater);
    }

    #[test]
    fn beyond_f64_range_orders_by_sign() {
        let huge = BigInt::from(2u8).pow(2000); // > f64::MAX
        assert_eq!(bigint_cmp_float(&huge, 1.0), Ordering::Greater);
        assert_eq!(bigint_cmp_float(&(-huge), 1.0), Ordering::Less);
    }

    #[test]
    fn decimal_vs_float_is_exact() {
        use super::bigdecimal_cmp_float;
        use bigdecimal::BigDecimal;
        use std::str::FromStr;
        // The decimal 0.1 is exactly 1/10; the f64 `0.1` is slightly larger
        // (0.1000000000000000055…). Exact comparison must order them, not call
        // them equal (which the old `to_f64` round-trip did).
        let exact_tenth = BigDecimal::from_str("0.1").unwrap();
        assert_eq!(bigdecimal_cmp_float(&exact_tenth, 0.1_f64), Ordering::Less);
        // 0.5 is exactly representable, so they're equal.
        let half = BigDecimal::from_str("0.5").unwrap();
        assert_eq!(bigdecimal_cmp_float(&half, 0.5_f64), Ordering::Equal);
    }
}
