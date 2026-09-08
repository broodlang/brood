//! The LOCAL string representation — how a Brood string is stored in a slab, and the
//! side tables that keep indexing it linear.
//!
//! A string slab entry is a [`LocalString`]: the bytes ([`StrData`] — inline, or an
//! `Arc<SharedBlob>` past [`SHARED_BLOB_THRESHOLD`] so a cross-process send bumps a
//! refcount) plus the char count cached at construction, plus a lazily-built [`StrAux`]
//! holding the sparse char→byte [`CharIndex`] and one opaque slot a higher layer can
//! use for its own table. Everything here is about *representing* a string; allocation
//! and the region-dispatching accessors stay in the parent.

use super::*;

/// A LOCAL (and transitively PRELUDE-builder) string slab entry. Small strings
/// stay inline; strings of [`SHARED_BLOB_THRESHOLD`] bytes or more route through
/// an `Arc<SharedBlob>` so cross-process sends bump a refcount instead of
/// deep-copying the bytes (see `core/blob.rs`).
///
/// PRELUDE itself contains no `Shared` entries — `freeze_as_shared_code`
/// inline-extracts any builder-time Shared blobs into `Inline(String)` before
/// freezing, keeping the cross-runtime PRELUDE region independent of any
/// runtime-scoped `Arc<SharedBlob>`.
/// A stored string plus its cached **char** length.
///
/// Brood indexes strings by Unicode scalar, but they are stored as UTF-8, so every
/// char index has to be converted to a byte offset. Done on demand that is O(index)
/// — `chars().count()` for the length, `char_indices().nth(k)` for the offset — which
/// silently makes any per-character or incremental-search loop quadratic. It has cost
/// real time here more than once: `url.blsp` and `csv.blsp` both had to switch to
/// code-point vectors, `ansi.blsp` was 1.58 s on 90 KB, and `index-of`'s `from` offset
/// stayed quadratic even after its suffix copy was removed.
///
/// Caching the count fixes the class rather than the instances. It is free to compute
/// (construction already walks the bytes to copy them) and it buys two things:
///   * `string-length` becomes O(1) instead of a full scan;
///   * `chars == as_str().len()` **is** the pure-ASCII test, and for an ASCII string a
///     char index *is* its byte offset — so the conversion in both directions is O(1).
///
/// The count alone cannot help a **non-ASCII** string, because its mechanism *is* the
/// pure-ASCII test: off that path every conversion still walked from the start, so a
/// scan carrying a rising index stayed quadratic (`inc-scan` in `scale_sweep.blsp` read
/// 16.85× per 4× of input under `UTF8=1`, against an unmeasurable ASCII row). Hence
/// [`StrAux`]: lazily-built side tables keyed to this exact string value — the sparse
/// char→byte index, and an opaque slot a higher layer can use for its own table.
#[derive(Clone)]
pub(super) struct LocalString {
    pub(super) data: StrData,
    pub(super) chars: usize,
    /// Side tables for this string value, built on first use and never otherwise — see
    /// [`StrAux`]. One cell, because this struct is every string slab entry and its size
    /// is per-string memory in every heap (40 → 56 bytes, pinned by a test); a second
    /// cell for the second table would have cost every string another 16.
    pub(super) aux: OnceLock<Box<StrAux>>,
}

/// Lazily-built, immutable side tables for one string value. Both are pure functions of
/// the string's (immutable) bytes, which is what makes `OnceLock` — rather than a
/// `Cell`/`RefCell` — the right cell: a slot can live in the **RUNTIME** region, which
/// every process of a runtime reads concurrently, so a lazily-populated cache there has
/// to be synchronised, and two racing builders produce identical tables of which
/// `get_or_init` publishes one.
#[derive(Clone)]
pub(super) struct StrAux {
    /// The sparse char→byte index (see [`CharIndex`]), built on the first char↔byte
    /// conversion of a long non-ASCII string.
    pub(super) index: OnceLock<CharIndex>,
    /// A table belonging to some **other layer**, attached to this exact string value.
    /// The heap owns the cell and never interprets the contents: it is `dyn Any` so that
    /// a per-string cache can exist for a higher layer's own type without the core
    /// depending on it (today the Lisp lexical scanners in `builtins/syntax_scan.rs`
    /// keep their form-start safepoint table here). `Arc` so a slot clone — what the GC
    /// does when it tenures a survivor — shares the table instead of rebuilding it.
    pub(super) scan: OnceLock<Arc<dyn std::any::Any + Send + Sync>>,
}

/// One [`CharIndex`] mark per `STRIDE` chars, so an index costs `4 * chars / STRIDE`
/// bytes (~1.5% of a 2-bytes-per-char string) and bounds a conversion's walk by `STRIDE`
/// chars. 32 trades table size against that walk; it is not tuned, and the measured win
/// is orders of magnitude larger than any nearby power of two would move it.
pub(super) const CHAR_INDEX_STRIDE: usize = 32;

/// Below this many chars a conversion just walks: the walk is already bounded by a
/// small number, and building an index would cost an allocation per string for it.
/// Above it the quadratic term is what dominates, which is what the index removes.
pub(super) const CHAR_INDEX_MIN_CHARS: usize = 256;

/// A sparse char→byte index for one non-ASCII string: `marks[k]` is the byte offset of
/// char `(k + 1) * CHAR_INDEX_STRIDE`. Char 0 is byte 0 and needs no entry, and the
/// last char is the last one that can have one, so `marks` has `(chars - 1) / STRIDE`
/// entries — nothing maps the end of the string, which the conversions handle directly.
///
/// Byte offsets are `u32`: a string of 4 GiB or more is left on the walking path rather
/// than given a 64-bit table (see [`LocalString::char_index`]).
#[derive(Clone)]
pub(super) struct CharIndex {
    pub(super) marks: Vec<u32>,
}

impl CharIndex {
    /// One pass over the bytes, recording every `STRIDE`-th char boundary.
    fn build(s: &str, chars: usize) -> CharIndex {
        let mut marks = Vec::with_capacity(chars / CHAR_INDEX_STRIDE);
        for (k, (b, _)) in s.char_indices().enumerate() {
            if k > 0 && k % CHAR_INDEX_STRIDE == 0 {
                marks.push(b as u32);
            }
        }
        CharIndex { marks }
    }

    /// The nearest indexed point at or before char `ci`: `(char index, byte offset)`.
    fn floor_char(&self, ci: usize) -> (usize, usize) {
        let k = (ci / CHAR_INDEX_STRIDE).min(self.marks.len());
        if k == 0 {
            (0, 0)
        } else {
            (k * CHAR_INDEX_STRIDE, self.marks[k - 1] as usize)
        }
    }

    /// The nearest indexed point at or before byte offset `b`, found by binary search
    /// over the (sorted) marks: `(char index, byte offset)`.
    fn floor_byte(&self, b: usize) -> (usize, usize) {
        let k = self.marks.partition_point(|&m| (m as usize) <= b);
        if k == 0 {
            (0, 0)
        } else {
            (k * CHAR_INDEX_STRIDE, self.marks[k - 1] as usize)
        }
    }
}

#[derive(Clone)]
pub(super) enum StrData {
    Inline(String),
    Shared(Arc<SharedBlob>),
}

impl Default for LocalString {
    fn default() -> Self {
        LocalString::inline(String::new())
    }
}

impl LocalString {
    pub(super) fn inline(s: String) -> Self {
        let chars = s.chars().count();
        LocalString {
            data: StrData::Inline(s),
            chars,
            aux: OnceLock::new(),
        }
    }

    pub(super) fn shared(b: Arc<SharedBlob>) -> Self {
        // Build first, then measure through `as_str` so the UTF-8 handling (and its
        // debug-only validation) lives in exactly one place.
        let mut me = LocalString {
            data: StrData::Shared(b),
            chars: 0,
            aux: OnceLock::new(),
        };
        me.chars = me.as_str().chars().count();
        me
    }

    /// The cached number of Unicode scalars — O(1).
    #[inline]
    pub(super) fn char_len(&self) -> usize {
        self.chars
    }

    /// Is this string pure ASCII, i.e. is a char index also a byte offset? O(1).
    #[inline]
    pub(super) fn is_ascii(&self) -> bool {
        self.chars == self.as_str().len()
    }

    /// This string's side-table block, allocated on the first table that needs it.
    #[inline]
    pub(super) fn aux(&self) -> &StrAux {
        self.aux.get_or_init(|| {
            Box::new(StrAux {
                index: OnceLock::new(),
                scan: OnceLock::new(),
            })
        })
    }

    /// This string's sparse char→byte index, built on first use; `None` for a string
    /// that walks instead (ASCII — where conversion is arithmetic — short, or larger
    /// than a `u32` offset can address).
    pub(super) fn char_index(&self) -> Option<&CharIndex> {
        if self.chars < CHAR_INDEX_MIN_CHARS {
            return None;
        }
        let s = self.as_str();
        // ASCII converts by arithmetic and needs no table; a string past a `u32` offset is
        // left on the walking path rather than given a 64-bit one.
        if self.chars == s.len() || s.len() > u32::MAX as usize {
            return None;
        }
        // Two threads racing here both build; `get_or_init` publishes one and drops the
        // other. Identical tables, so which one wins does not matter.
        Some(
            self.aux()
                .index
                .get_or_init(|| CharIndex::build(s, self.chars)),
        )
    }

    /// Byte offset of char `ci`, clamped to the end of the string (so an out-of-range
    /// index reads as "past the last char", which is what the string builtins want).
    /// O(1) on ASCII, O(1) + a walk bounded by [`CHAR_INDEX_STRIDE`] with an index,
    /// O(ci) without one.
    pub(super) fn char_to_byte(&self, ci: usize) -> usize {
        // `chars == bytes` IS the pure-ASCII test; taken here against the `&str` already in
        // hand, so the fast path resolves the slot's bytes once.
        let s = self.as_str();
        if self.chars == s.len() {
            return ci.min(s.len());
        }
        if ci >= self.chars {
            return s.len();
        }
        let (base_char, base_byte) = match self.char_index() {
            Some(ix) => ix.floor_char(ci),
            None => (0, 0),
        };
        match s[base_byte..].char_indices().nth(ci - base_char) {
            Some((b, _)) => base_byte + b,
            None => s.len(),
        }
    }

    /// Char index of byte offset `b`, which must be a char boundary (every caller has
    /// one from a byte-level match or a boundary snap). The inverse of
    /// [`char_to_byte`](Self::char_to_byte), with the same three complexities.
    pub(super) fn byte_to_char(&self, b: usize) -> usize {
        let s = self.as_str();
        debug_assert!(
            b <= s.len() && s.is_char_boundary(b),
            "byte {} is not a char boundary of a {}-byte string",
            b,
            s.len()
        );
        if self.chars == s.len() {
            return b.min(s.len());
        }
        let (base_char, base_byte) = match self.char_index() {
            Some(ix) => ix.floor_byte(b),
            None => (0, 0),
        };
        base_char + s[base_byte..b].chars().count()
    }

    pub(super) fn as_str(&self) -> &str {
        match &self.data {
            StrData::Inline(s) => s.as_str(),
            // SAFETY: `SharedBlob::new` is the only constructor and takes
            // `&[u8]` from a `&str`'s `as_bytes()` (see [`Heap::alloc_string`]).
            // Blobs are immutable after construction. The wire decoder
            // (`get_str` in `dist::wire`) validates UTF-8 on entry before
            // allocating, so a cross-node payload satisfies the invariant
            // too. In debug builds an extra `from_utf8` round-trip catches
            // a missed entry-point — the unchecked read only ships in
            // release.
            #[cfg(not(debug_assertions))]
            StrData::Shared(b) => unsafe { std::str::from_utf8_unchecked(b.as_bytes()) },
            #[cfg(debug_assertions)]
            StrData::Shared(b) => {
                std::str::from_utf8(b.as_bytes()).expect("shared blob bytes are valid UTF-8")
            }
        }
    }
}

#[cfg(test)]
mod char_index_tests {
    use super::*;

    /// The naive conversion the index replaces — the definition both directions are
    /// checked against.
    fn walk_char_to_byte(s: &str, ci: usize) -> usize {
        s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
    }

    /// Every char index of `s` (and one past the end) must convert to the same byte
    /// offset the walk gives, and back again — with the index built and, by running a
    /// second slot below the threshold, without it. A wrong offset here is a silent
    /// wrong substring, not a crash, so the check is exhaustive rather than sampled.
    fn agrees_at_every_index(s: &str) {
        let e = LocalString::inline(s.to_string());
        assert_eq!(
            e.char_len(),
            s.chars().count(),
            "cached char count: {:?}",
            s
        );
        for ci in 0..=e.char_len() {
            let want = walk_char_to_byte(s, ci);
            assert_eq!(
                e.char_to_byte(ci),
                want,
                "char {} of {:?} (len {} chars)",
                ci,
                s,
                e.char_len()
            );
            assert_eq!(
                e.byte_to_char(want),
                ci.min(e.char_len()),
                "byte {} of {:?} back to a char index",
                want,
                s
            );
        }
    }

    /// Shapes that put the multi-byte chars in different places relative to the index
    /// stride: leading (the sweep's fixture), trailing, one per stride boundary, and
    /// all-wide. Each is built both long enough to be indexed and short enough not to
    /// be, so the two paths are checked against the same expectations.
    #[test]
    fn conversions_agree_with_the_walk() {
        for reps in [1, 3, CHAR_INDEX_MIN_CHARS] {
            agrees_at_every_index(&"a".repeat(reps));
            agrees_at_every_index(&"é".repeat(reps));
            agrees_at_every_index(&"🙂".repeat(reps));
            agrees_at_every_index(&"aé漢🙂x".repeat(reps));
            agrees_at_every_index(&format!("café — {}", "x".repeat(reps)));
            agrees_at_every_index(&format!("{}é", "x".repeat(reps)));
            // A wide char exactly on every stride boundary, ASCII in between.
            let mut s = String::new();
            for k in 0..reps {
                s.push_str(&"a".repeat(CHAR_INDEX_STRIDE - 1));
                s.push(if k % 2 == 0 { '漢' } else { '🙂' });
            }
            agrees_at_every_index(&s);
        }
    }

    /// The index is built for a long multi-byte string and declined otherwise — the
    /// ASCII case needs no table (a char index *is* the byte offset) and a short one is
    /// cheaper to walk than to index.
    #[test]
    fn the_index_is_built_only_where_it_pays() {
        let long_ascii = LocalString::inline("a".repeat(CHAR_INDEX_MIN_CHARS * 2));
        assert!(long_ascii.char_index().is_none(), "ASCII needs no index");

        let short = LocalString::inline("é".repeat(CHAR_INDEX_MIN_CHARS - 1));
        assert!(short.char_index().is_none(), "a short string walks");

        let long = LocalString::inline("é".repeat(CHAR_INDEX_MIN_CHARS * 2));
        let ix = long
            .char_index()
            .expect("long multi-byte string is indexed");
        assert_eq!(
            ix.marks.len(),
            (CHAR_INDEX_MIN_CHARS * 2 - 1) / CHAR_INDEX_STRIDE,
            "one mark per whole stride, none for the end"
        );
        // Marks are byte offsets of the stride-th chars, in order.
        assert_eq!(ix.marks[0] as usize, CHAR_INDEX_STRIDE * 2);
        assert!(ix.marks.windows(2).all(|w| w[0] < w[1]), "marks ascend");
    }

    /// The index cell costs a pointer plus its `Once` state, not an inline table: this
    /// struct is every string slab entry, so its size is per-string memory in every
    /// process heap (and feeds the GC's byte accounting via `slab_bytes`).
    #[test]
    fn the_slot_stays_small() {
        let n = std::mem::size_of::<LocalString>();
        assert!(n <= 56, "LocalString grew to {} bytes", n);
        // Both side tables share the one cell — a second cell would cost every string
        // another word plus its `Once` state.
        assert_eq!(
            std::mem::size_of::<OnceLock<Box<StrAux>>>(),
            16,
            "the aux cell is one boxed pointer"
        );
    }

    /// A `Shared` slot (a string past `SHARED_BLOB_THRESHOLD`, so exactly the long ones
    /// worth indexing) reads its bytes through the blob; the index must work there too.
    #[test]
    fn a_shared_blob_slot_is_indexed_too() {
        let s = "aé漢🙂x".repeat(CHAR_INDEX_MIN_CHARS);
        assert!(s.len() > SHARED_BLOB_THRESHOLD);
        let e = LocalString::shared(SharedBlob::new(s.as_bytes()));
        assert!(e.char_index().is_some());
        for ci in [0, 1, 31, 32, 33, 500, e.char_len() - 1, e.char_len()] {
            assert_eq!(e.char_to_byte(ci), walk_char_to_byte(&s, ci), "char {}", ci);
        }
    }
}
