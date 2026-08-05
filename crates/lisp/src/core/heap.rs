//! The per-process data heap, plus the two shared regions: the immutable
//! **prelude** and a runtime's mutable, shared **code** region.
//!
//! A `Value`'s heap variants are integer handles whose two high bits (the
//! *region*, see `value.rs`) say where they live:
//!
//! - **LOCAL** — the per-process [`Heap`]: everything a process allocates at
//!   runtime (cons cells, vectors, strings, call-frame env scopes). Plain
//!   `Vec`s, mutated through `&mut Heap`, so the whole `Heap` is `Send`.
//!   Bump-allocated into a **nursery**; survivors are relocated by the copying
//!   collector (see below), never freed in place, so handle slots are never
//!   reused.
//! - **PRELUDE** — a [`SharedCode`] region (behind `Arc`) holding the prelude +
//!   builtins. Built once, frozen, shared read-only by every runtime.
//! - **RUNTIME** — a [`RuntimeCode`] region (behind `Arc`) holding a runtime's
//!   `def`'d code and its global bindings. **Mutable and shared** by all of a
//!   runtime's inner (spawned) processes, so a redefinition is visible to a
//!   running process on its next global lookup (Erlang-style hot reload). The
//!   code slabs are append-only (old code is never moved or freed, so in-flight
//!   calls keep running it); the global bindings are a `RwLock<HashMap>`.
//!
//! GC is **per-process, single-threaded, generational semi-space copying**
//! (ADR-055/061/072, see `docs/memory-model.md` and `docs/memory-review.md`). The
//! LOCAL heap is a **nursery** + a tenured **old** generation; a *minor*
//! collection ([`collect`](Self::collect) → [`minor_collect`](Self::minor_collect))
//! copies the nursery's survivors (tenuring or flipping) and drops the rest, a
//! rare *major* compacts old. Because survivors **move**, a handle held across a
//! collection without being re-rooted goes stale — so the evaluator keeps its
//! in-flight LOCAL handles on an explicit operand stack ([`roots`](Self::roots) +
//! [`env_roots`](Self::env_roots)) that the collector relocates in place, letting
//! it collect at **any** eval depth; a generation epoch on every handle (ADR-054)
//! trips a precise debug tripwire on a stale deref. PRELUDE and RUNTIME are never
//! traced (they hold no LOCAL refs, by the promotion invariant — see
//! [`promote`](Self::promote)); the collector only touches LOCAL.

use arc_swap::{ArcSwap, Guard};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use smallvec::SmallVec;

use crate::core::blob::{SharedBlob, SHARED_BLOB_THRESHOLD};
use crate::core::keywords as kw;
use crate::core::map_champ::{self, MapNode, MAX_DEPTH};
use crate::core::value::{
    BigIntId, BytesId, Closure, ClosureArm, ClosureId, ClosureTemplate, DecimalId, EnvId, MapId,
    NativeFn, NativeId, PairId, Passthrough, RatioId, RopeId, StrId, Symbol, Value, ValueRef,
    VecId, LOCAL, PRELUDE, RUNTIME,
};
use crate::error::LispError;

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
struct LocalString {
    data: StrData,
    chars: usize,
    /// Side tables for this string value, built on first use and never otherwise — see
    /// [`StrAux`]. One cell, because this struct is every string slab entry and its size
    /// is per-string memory in every heap (40 → 56 bytes, pinned by a test); a second
    /// cell for the second table would have cost every string another 16.
    aux: OnceLock<Box<StrAux>>,
}

/// Lazily-built, immutable side tables for one string value. Both are pure functions of
/// the string's (immutable) bytes, which is what makes `OnceLock` — rather than a
/// `Cell`/`RefCell` — the right cell: a slot can live in the **RUNTIME** region, which
/// every process of a runtime reads concurrently, so a lazily-populated cache there has
/// to be synchronised, and two racing builders produce identical tables of which
/// `get_or_init` publishes one.
#[derive(Clone)]
struct StrAux {
    /// The sparse char→byte index (see [`CharIndex`]), built on the first char↔byte
    /// conversion of a long non-ASCII string.
    index: OnceLock<CharIndex>,
    /// A table belonging to some **other layer**, attached to this exact string value.
    /// The heap owns the cell and never interprets the contents: it is `dyn Any` so that
    /// a per-string cache can exist for a higher layer's own type without the core
    /// depending on it (today the Lisp lexical scanners in `builtins/syntax_scan.rs`
    /// keep their form-start safepoint table here). `Arc` so a slot clone — what the GC
    /// does when it tenures a survivor — shares the table instead of rebuilding it.
    scan: OnceLock<Arc<dyn std::any::Any + Send + Sync>>,
}

/// One [`CharIndex`] mark per `STRIDE` chars, so an index costs `4 * chars / STRIDE`
/// bytes (~1.5% of a 2-bytes-per-char string) and bounds a conversion's walk by `STRIDE`
/// chars. 32 trades table size against that walk; it is not tuned, and the measured win
/// is orders of magnitude larger than any nearby power of two would move it.
const CHAR_INDEX_STRIDE: usize = 32;

/// Below this many chars a conversion just walks: the walk is already bounded by a
/// small number, and building an index would cost an allocation per string for it.
/// Above it the quadratic term is what dominates, which is what the index removes.
const CHAR_INDEX_MIN_CHARS: usize = 256;

/// A sparse char→byte index for one non-ASCII string: `marks[k]` is the byte offset of
/// char `(k + 1) * CHAR_INDEX_STRIDE`. Char 0 is byte 0 and needs no entry, and the
/// last char is the last one that can have one, so `marks` has `(chars - 1) / STRIDE`
/// entries — nothing maps the end of the string, which the conversions handle directly.
///
/// Byte offsets are `u32`: a string of 4 GiB or more is left on the walking path rather
/// than given a 64-bit table (see [`LocalString::char_index`]).
#[derive(Clone)]
struct CharIndex {
    marks: Vec<u32>,
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
enum StrData {
    Inline(String),
    Shared(Arc<SharedBlob>),
}

impl Default for LocalString {
    fn default() -> Self {
        LocalString::inline(String::new())
    }
}

impl LocalString {
    fn inline(s: String) -> Self {
        let chars = s.chars().count();
        LocalString {
            data: StrData::Inline(s),
            chars,
            aux: OnceLock::new(),
        }
    }

    fn shared(b: Arc<SharedBlob>) -> Self {
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
    fn char_len(&self) -> usize {
        self.chars
    }

    /// Is this string pure ASCII, i.e. is a char index also a byte offset? O(1).
    #[inline]
    fn is_ascii(&self) -> bool {
        self.chars == self.as_str().len()
    }

    /// This string's side-table block, allocated on the first table that needs it.
    #[inline]
    fn aux(&self) -> &StrAux {
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
    fn char_index(&self) -> Option<&CharIndex> {
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
    fn char_to_byte(&self, ci: usize) -> usize {
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
    fn byte_to_char(&self, b: usize) -> usize {
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

    fn as_str(&self) -> &str {
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

/// Generate a `&self` accessor that resolves a handle to a shared reference by
/// region: the LOCAL/PRELUDE slab is indexed directly; the append-only RUNTIME
/// slab via `boxcar::Vec::get` (stable refs, lock-free). The three uniform
/// all-three-region reference accessors share this; `pair` (returns by value)
/// and the region-restricted `native`/`env_frame` stay hand-written.
macro_rules! region_ref {
    ($name:ident, $id:ty, $field:ident, $t:ty, $what:literal) => {
        pub fn $name(&self, id: $id) -> SlabRef<'_, $t> {
            match id.region() {
                LOCAL if id.is_old() => {
                    #[cfg(debug_assertions)]
                    self.check_epoch_aged(
                        true,
                        id.generation(),
                        id.index(),
                        stringify!($name),
                        id.0,
                    );
                    SlabRef::direct(&self.old().$field[id.index()])
                }
                LOCAL => {
                    #[cfg(debug_assertions)]
                    self.check_epoch_aged(
                        false,
                        id.generation(),
                        id.index(),
                        stringify!($name),
                        id.0,
                    );
                    SlabRef::direct(&self.local.$field[id.index()])
                }
                PRELUDE => SlabRef::direct(&self.prelude.slabs.$field[id.index()]),
                RUNTIME => {
                    let pin = self.code_gen_pinned(id.code_gen());
                    let r: &$t = pin.$field.get(id.index()).expect($what);
                    let ptr = r as *const $t;
                    // SAFETY: `ptr` points into `pin`'s CodeSlabs (stable `boxcar`
                    // address), kept alive by the `Arc` moved into the `SlabRef`.
                    unsafe { SlabRef::pinned(pin, ptr) }
                }
                _ => unreachable!("invalid handle region"),
            }
        }
    };
}

/// Emit the use-after-GC tripwire for **one LOCAL match arm** of a hand-written
/// accessor — the generational `check_epoch_aged`. Factors the byte-for-byte-identical
/// preamble the `pair`/`string`/`closure`/`rope`/`bigint` accessors each copy-pasted;
/// `region_ref!` already inlines the same check for the uniform reference accessors.
/// `$name` is the accessor name (for the epoch "what" string); `$h` the handle
/// expression (`id.index()`/`id.0`/`id.generation()`).
///
/// Two forms select the aged flag: `old` → aged, `nursery` → nursery. (env_frame stays
/// hand-written — its message carries extra docs prose and binds `env`, not `id`.)
macro_rules! local_gc_check {
    (old, $self:ident, $h:expr, $name:literal) => {
        #[cfg(debug_assertions)]
        $self.check_epoch_aged(true, $h.generation(), $h.index(), $name, $h.0);
    };
    (nursery, $self:ident, $h:expr, $name:literal) => {
        #[cfg(debug_assertions)]
        $self.check_epoch_aged(false, $h.generation(), $h.index(), $name, $h.0);
    };
}

/// Inline storage for an env frame's bindings. A frame holds a handful (function
/// params, a `let`'s names), so keeping them inline avoids a heap allocation per
/// call / `let` — which the byte-counting global allocator would otherwise tax
/// with atomics on the hot path. Spills to the heap past the inline capacity.
type EnvVars = SmallVec<[(Symbol, Value); 4]>;

struct EnvFrame {
    // A small association list, not a `HashMap`: frames hold a handful of
    // bindings (function params, a `let`'s names), and they're immutable after
    // their bind phase (ADR-026 — no `set!`), so a build-once / scan-to-read
    // vector is lighter than hashing and wins at these sizes. Lookups scan from
    // the end so a later binding shadows an earlier one of the same name
    // (sequential `let`).
    vars: EnvVars,
    parent: Option<EnvId>,
}

/// Parse a GC threshold override (an *object count*, with an optional `K`/`M`
/// suffix — `64K` = 65536, `1M` = 1048576) from env var `key`. `None` if unset;
/// a malformed value warns and is ignored (so the caller's default stands).
/// Mirrors the `BROOD_MEM_LIMIT` size-parse style in `core/alloc.rs`, but counts
/// objects rather than bytes.
fn gc_count_env(key: &str) -> Option<usize> {
    let v = std::env::var(key).ok()?;
    let s = v.trim();
    let (num, mult) = match s.chars().last() {
        Some(c @ ('K' | 'k')) => (&s[..s.len() - c.len_utf8()], 1024usize),
        Some(c @ ('M' | 'm')) => (&s[..s.len() - c.len_utf8()], 1024 * 1024),
        _ => (s, 1usize),
    };
    match num.trim().parse::<usize>() {
        Ok(n) => n.checked_mul(mult),
        Err(_) => {
            eprintln!("[gc] ignoring malformed {key}={v:?} (try e.g. 65536 or 64K)");
            None
        }
    }
}

/// Live **green-process** gauge: incremented when a process is spawned,
/// decremented when it is torn down (the scheduler calls [`live_process_inc`] /
/// [`live_process_dec`]). The root thread is *not* counted — `gc_floor`'s
/// `.max(1)` covers it. Kept here in `core` so the GC can read it without a
/// `core → process` dependency; `process` is the only writer.
static LIVE_PROCESSES: AtomicUsize = AtomicUsize::new(0);

/// Note a newly spawned green process (scheduler `spawn`).
pub fn live_process_inc() {
    LIVE_PROCESSES.fetch_add(1, Ordering::Relaxed);
}

/// Note a torn-down green process (scheduler `deregister`).
pub fn live_process_dec() {
    LIVE_PROCESSES.fetch_sub(1, Ordering::Relaxed);
}

/// Current count of live green processes (excludes the root).
pub fn live_process_count() -> usize {
    LIVE_PROCESSES.load(Ordering::Relaxed)
}

/// Object-count budget a process may accumulate before its **first** GC (the
/// initial/minimum value of the adaptive `gc_threshold`, which after each GC
/// becomes `max(gc_floor, live*2)` — so a genuinely large live set keeps its own
/// `live*2` threshold and the floor is irrelevant to it). The floor therefore
/// only bites churny processes whose working set stays *below* it.
///
/// **Process-count-aware** (the fix for the `pfib` 1-GB blowup): a fixed object
/// budget is divided among the live processes, so fanning out N short-lived
/// churny processes doesn't have each one climb to the single-process ceiling
/// before collecting. A lone process is unchanged at `FLOOR_MAX`; the
/// per-process floor scales down toward `FLOOR_MIN` as concurrency rises
/// (e.g. 100-way `pfib`: ~64K each → ~4K each, ~990 MB → ~90 MB peak, no
/// throughput cost — the churn GCs were happening regardless, just later).
///
/// Read only at process creation and after each GC (never on the allocation hot
/// path), so the relaxed atomic load is free. `BROOD_GC_FLOOR` / `BROOD_GC_STRESS`
/// still pin a fixed value and opt out of the adaptive policy.
fn gc_floor() -> usize {
    // An explicit override pins a fixed floor and bypasses the adaptive policy —
    // used by the GC stress tests and honoured by the "non-default GC config"
    // benchmark guard. Cached: the env is read once.
    static OVERRIDE: OnceLock<Option<usize>> = OnceLock::new();
    let fixed = *OVERRIDE.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            Some(0)
        } else {
            // Overridable for tuning via `BROOD_GC_FLOOR` (object count, K/M ok).
            gc_count_env("BROOD_GC_FLOOR")
        }
    });
    if let Some(n) = fixed {
        return n;
    }
    // ~64K objects for a lone process (well above per-call working sets, trivial
    // vs the GBs a long-running process leaks); ~4K is the floor under heavy
    // fan-out (below this, GC churn starts to cost more than the memory saved).
    const FLOOR_MAX: usize = 64 * 1024;
    const FLOOR_MIN: usize = 4 * 1024;
    let live = live_process_count().max(1);
    (FLOOR_MAX / live).clamp(FLOOR_MIN, FLOOR_MAX)
}

/// RUNTIME-closure count at or above which the eval safepoint auto-runs a
/// **RUNTIME** compaction ([`Heap::maybe_runtime_collect`]) — the shared-code
/// analog of [`gc_floor`]. The region only grows on `def`/hot-reload (≈1 KB per
/// superseded closure), so a default of 4096 (~4 MB of churn) keeps a normal
/// program — which defines each global once, far below this — from ever
/// auto-collecting, while a sustained redefinition session is bounded. Like
/// [`major_floor`] (and unlike [`gc_floor`]) this stays **nonzero under
/// `BROOD_GC_STRESS`**: recompacting the whole RUNTIME region at *every*
/// safepoint would be O(region) per step, so stress keeps it periodic (still
/// exercised) at a small floor rather than literally every safepoint.
/// Overridable via `BROOD_RT_GC_FLOOR` (object count, K/M ok).
fn rt_gc_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            gc_count_env("BROOD_RT_GC_FLOOR").unwrap_or(256)
        } else {
            gc_count_env("BROOD_RT_GC_FLOOR").unwrap_or(4096)
        }
    })
}

/// How often a process runs the multigen drain free-attempt (the O·live-process
/// `report_parked_liveness` registry scan) at its RUNTIME safepoint while a drain is
/// armed: once every `RT_DRAIN_SCAN_STRIDE` safepoints, not every frame. See the
/// `rt_drain_tick` field for why (the pinned-generation scan storm).
const RT_DRAIN_SCAN_STRIDE: u32 = 64;

/// How often a process runs its **per-safepoint drain self-report** while a generation drain
/// is armed: once every `DRAIN_REPORT_STRIDE` safepoints, not every frame. During a `spawn`
/// fan-out the drain lingers (workers pin it until they exit), so every worker reaches the
/// report on nearly every safepoint — ~9 M calls for 10 k workers, 99.9 % of them no-op
/// re-confirmations by an already-acked process. Each call re-reads the shared `drain_epoch`
/// (periodically written as drains re-arm → its cache line bounces → a coherence miss), so at
/// that volume it dominated the residual `spawn` collector overhead. The throttle is a
/// per-heap `Cell` tick (no shared read), so a skipped frame costs nothing. Sound: a process
/// that turns clean acks within a stride (drain completes ≤ stride safepoints later) and an
/// exiting one is accounted at once by `drain_note_exit`. To keep completion prompt, the
/// process that *arms* a drain resets its own tick (see [`Heap::begin_gen_drain`]) so it
/// reports on its very next frame. Throttles ONLY the safepoint path
/// ([`crate::process::report_drain_liveness`]); the parked-process inspector and the
/// drain-completion tests call `report_gen_liveness` directly and stay unthrottled.
const DRAIN_REPORT_STRIDE: u32 = 64;

/// How often a process already found **dirty via Phase 2** (a RUNTIME handle embedded in
/// its LOCAL heap data) re-runs that O(heap) walk in the drain self-report, vs. reporting
/// its cached stale-dirty verdict: once every `P2_REVALIDATE_STRIDE` safepoints of the
/// current drain epoch. Bounds a data-pinned process's per-safepoint report to 1/stride of
/// the full-heap walk — without it a big-heap pinning process (e.g. the root over a growing
/// message backlog) re-walks its whole heap every safepoint, quadratic (the ~300× `spawn`
/// fan-out regression). Sound: a stale-dirty verdict only delays completion, and a process
/// that turns clean re-validates within a stride. See `runtime_gen_referenced_private`.
const P2_REVALIDATE_STRIDE: u32 = 64;

/// The Phase-1 counterpart of [`P2_REVALIDATE_STRIDE`], and the seed size above which it
/// applies.
///
/// Phase 1 (private roots + live arms) is the *cheap* probe and is deliberately
/// unthrottled, so a process that stops running draining-generation code acks on its very
/// next safepoint — the promptness the drain-completion tests rely on. "Cheap" holds only
/// while the seed is small, and one term in it is not bounded: `roots` is the VM operand /
/// env stack, so it grows with **recursion depth**. A process 100 000 frames deep seeds
/// hundreds of thousands of values per probe, and while it stays dirty it pays that on
/// every reporting safepoint — quadratic in depth, and in practice a run that stops making
/// progress (KI-14: one worker pinned at 100% CPU, the suite never finishing).
///
/// So throttle Phase 1 the same way Phase 2 already is, but **only for a large seed**: a
/// shallow process (every drain-completion test, and the overwhelming majority of real
/// ones) is below the threshold and keeps reporting on every safepoint, unchanged. Sound
/// for the same reason as Phase 2 — a stale-dirty verdict only delays drain completion, it
/// can never fabricate a clean ack, and a process that turns clean re-validates within a
/// stride. See `runtime_gen_referenced_private`.
const P1_REVALIDATE_STRIDE: u32 = 64;

/// Seed size (roots + env roots + dynamics + live arms) above which a dirty Phase-1 verdict
/// starts being cached between re-validations. See [`P1_REVALIDATE_STRIDE`].
const P1_LARGE_SEED: usize = 4096;

/// Live old-gen object count below which a **major** collection never fires —
/// the old-gen counterpart of [`gc_floor`]. Crucially this is **not** zeroed by
/// `BROOD_GC_STRESS`: stress makes *minor* collection fire at every safepoint
/// (its purpose), but a major every safepoint would recompact the whole old
/// generation on an incremental large-structure build — O(n²). Keeping a nonzero
/// floor makes majors periodic under stress (still exercised) and rare in normal
/// operation (the old gen grows to a few MB before a compaction reclaims tenured
/// garbage, so live tenured data isn't recopied often).
/// Growth factor for the major-collection threshold: after a major, the next one
/// fires when the old gen has grown this many× (was 2×). A larger factor makes
/// majors geometrically rarer during a large-structure build — where the old gen
/// is nearly all-live and a compaction copies everything for almost no reclaim —
/// at the cost of retaining more tenured garbage between majors (memory for speed).
/// Ceiling for the adaptive nursery threshold (see `collect`): the young gen may
/// grow to at most this many objects before a minor GC, regardless of old-gen size.
/// Bounds the transient young-garbage buffer for a large-heap churny process while
/// sitting far above real build working sets (~8M objects ≈ a few hundred MB).
const NURSERY_MAX: usize = 8 * 1024 * 1024;

/// Deep-value guard for the recursive heap walkers (`promote_in`, the GC
/// `flush_value`, `equal`, `hash_value_into`): each recurses per **car**-nesting
/// level (their cdr spines are already iterative), so a deep-but-legal immutable
/// value — a 60k-deep nested list is just data — overflowed the native stack
/// (found 2026-07-19/20 by the iolist deep-nesting test; CI SIGABRT). Each
/// recursion entry checks the remaining stack and, inside the red zone, grows in
/// a heap-backed segment (`stacker::maybe_grow`, rustc's own approach) instead
/// of overflowing. Cost when not growing: one thread-local read + compare per
/// level. The alternative — rewriting four bottom-up builders as explicit
/// two-phase stack machines — was rejected as far more complexity for the same
/// guarantee.
const WALKER_RED_ZONE: usize = 64 * 1024;
/// Segment size for a deep-walker stack grow — large enough that even a
/// million-deep value grows a handful of times, small enough to stay cheap.
const WALKER_STACK_CHUNK: usize = 1024 * 1024;

fn major_growth() -> usize {
    static G: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *G.get_or_init(|| {
        std::env::var("BROOD_MAJOR_GROWTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n >= 2)
            .unwrap_or(4)
    })
}

fn major_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            8192
        } else {
            // Overridable for tuning via `BROOD_GC_MAJOR` (object count, K/M ok).
            gc_count_env("BROOD_GC_MAJOR").unwrap_or(256 * 1024)
        }
    })
}

/// Nursery-pressure threshold (live object count) at or above which a minor
/// collection **tenures** survivors into the old generation; below it the minor
/// does a young **semi-space flip** (survivors stay in a fresh nursery) instead.
/// This is the *aging* policy: an object tenures only when it survives a
/// collection that followed real allocation pressure — never a premature one.
/// Stress-independent (unlike [`gc_floor`]) so that `BROOD_GC_STRESS=1`, which
/// fires a minor at *every* safepoint with a tiny nursery, always flips and so
/// never tenures transient garbage (which would otherwise bloat the old gen and
/// make majors recopy it — the adversarial-under-stress regression). A
/// long-lived structure still tenures once the nursery genuinely grows past this.
fn min_tenure() -> usize {
    static T: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    // Overridable for tuning via `BROOD_GC_TENURE` (object count, K/M ok).
    *T.get_or_init(|| gc_count_env("BROOD_GC_TENURE").unwrap_or(16 * 1024))
}

/// Default for the per-process GC **trace** flag, from the `BROOD_GC_TRACE` env
/// var (set it to trace the whole run — including the root process, which the
/// `(gc-trace …)` builtin can't reach before user code runs). Read once and
/// cached; `(gc-trace on/off)` overrides it per process at runtime.
fn gc_trace_default() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_GC_TRACE").is_some())
}

/// Re-tag a value's handle from the local region to the immutable **prelude**
/// region (same slab index, region bits set). Atoms are unchanged.
/// A movable handle's identity — `(kind, index, region)`. `None` for an atom, which
/// has no heap identity and never needs copying. Used by
/// [`Heap::localize_for_freeze`] to collapse shared structure and to tell "already
/// LOCAL, unchanged" from "copied".
fn handle_key(v: Value) -> Option<(u8, u32, u8)> {
    let (kind, idx, reg) = match v.unpack() {
        ValueRef::Pair(id) => (0u8, id.index() as u32, id.region()),
        ValueRef::Vector(id) => (1, id.index() as u32, id.region()),
        ValueRef::Range(id) => (2, id.index() as u32, id.region()),
        ValueRef::SeqView(id) => (3, id.index() as u32, id.region()),
        ValueRef::Map(id) => (4, id.index() as u32, id.region()),
        ValueRef::Set(id) => (5, id.index() as u32, id.region()),
        ValueRef::Str(id) => (6, id.index() as u32, id.region()),
        ValueRef::BigInt(id) => (7, id.index() as u32, id.region()),
        ValueRef::Decimal(id) => (8, id.index() as u32, id.region()),
        ValueRef::Ratio(id) => (13, id.index() as u32, id.region()),
        ValueRef::Bytes(id) => (9, id.index() as u32, id.region()),
        ValueRef::Fn(id) => (10, id.index() as u32, id.region()),
        ValueRef::Macro(id) => (11, id.index() as u32, id.region()),
        ValueRef::Rope(id) => (12, id.index() as u32, id.region()),
        _ => return None,
    };
    Some((kind, idx, reg))
}

/// Re-tag a **LOCAL** handle as PRELUDE, preserving its slab index.
///
/// Only LOCAL: a re-tag is an index-preserving bit flip, which is valid exactly
/// because the builder's slabs *become* the prelude region. Applying it to a
/// RUNTIME (or already-PRELUDE) handle would keep the index and change the region,
/// pointing at an unrelated object in a different slab — that was KI-12. The VM
/// promotes its constant-pool literals into RUNTIME, so a prelude global built by
/// compiled code (`(def *load-path* (list "."))`) held a LOCAL pair whose car was a
/// RUNTIME string; re-tagging it yielded PRELUDE `Str@60`, some unrelated
/// docstring. Non-LOCAL values are copied into the builder's slabs *before* this
/// runs — see [`Heap::localize_for_freeze`].
fn to_prelude(v: Value) -> Value {
    // **LOCAL only.** Reachable structure is copied LOCAL beforehand
    // ([`Heap::localize_for_freeze`]), so a non-LOCAL handle reaching here belongs to
    // unreachable boot garbage — the slab sweep visits every cell, dead ones
    // included. Leave those alone: nothing can read them, and flipping them is
    // precisely what corrupted a live global (KI-12).
    if !matches!(handle_key(v), None | Some((_, _, LOCAL))) {
        return v;
    }
    match v.unpack() {
        ValueRef::Pair(id) => Value::pair(PairId::prelude(id.index())),
        ValueRef::Vector(id) => Value::vector(VecId::prelude(id.index())),
        ValueRef::Range(id) => Value::range(VecId::prelude(id.index())),
        ValueRef::SeqView(id) => Value::seqview(VecId::prelude(id.index())),
        ValueRef::Map(id) => Value::map(MapId::prelude(id.index())),
        ValueRef::Set(id) => Value::set(MapId::prelude(id.index())),
        ValueRef::Str(id) => Value::str_(StrId::prelude(id.index())),
        ValueRef::BigInt(id) => Value::bigint(BigIntId::prelude(id.index())),
        ValueRef::Decimal(id) => Value::decimal(DecimalId::prelude(id.index())),
        ValueRef::Ratio(id) => Value::ratio(RatioId::prelude(id.index())),
        // A `bytes` handle used to fall through the `other` arm below, keeping its
        // LOCAL tag — so a `#b"…"` literal reaching a prelude global would resolve
        // in the wiped builder heap after the freeze. No prelude form produces one
        // today (the bit-syntax matcher only mentions them in comments), so this is
        // latent, but silence was the wrong default for a region re-tag: every kind
        // is either flipped or explicitly guarded (see `Rope`). Noticed while
        // investigating KI-12.
        ValueRef::Bytes(id) => Value::bytes(BytesId::prelude(id.index())),
        ValueRef::Fn(id) => Value::func(ClosureId::prelude(id.index())),
        ValueRef::Macro(id) => Value::macro_(ClosureId::prelude(id.index())),
        ValueRef::Native(id) => Value::native(NativeId::prelude(id.index())),
        // The prelude is pure Brood (no rope literals), so a rope can never
        // exist at freeze time. Guard the invariant rather than silently
        // re-tagging a LOCAL handle into PRELUDE.
        ValueRef::Rope(_) => unreachable!("a Rope cannot appear in the prelude region"),
        other => other,
    }
}

/// How many elements a vector stores **inline in its slab slot** before it
/// spills to a heap `Vec` (see [`VecStore`]). Set to 2 — the hot small-vector
/// case (2-element tuples like `bintree` nodes, 2-element `SeqView` backings) —
/// kept small so an inline slot (~64 B) stays *below* the old `Vec<Value>`
/// handle-plus-`malloc` footprint (a 24 B slot + a ≥48 B heap block). A larger
/// cap would inline 3-element `Range` backings too, but at a bigger slot for
/// *every* vector, which added GC-copy traffic on the copy-bound `bintree`
/// (whose whole tree is live at once) — net CPU-negative in measurement.
pub(crate) const INLINE_VEC_CAP: usize = 2;

/// Element storage for one heap vector: **inline** in the slab slot for the
/// common small case, or **spilled** to a heap `Vec` for larger vectors. This
/// replaced a bare `Vec<Value>` per slot (`vectors: Vec<Vec<Value>>`), which
/// paid a `malloc` on *every* vector allocation and forced element reads through
/// a double indirection the JIT couldn't inline. A small vector is now a plain
/// bump-push (like a pair), and its elements sit at a fixed offset in the slot
/// so the JIT can inline the read the way it inlines a pair car/cdr.
///
/// An **enum** (not a struct with an always-present spill field), so the inline
/// and spill forms share storage and a small vector costs no more than its two
/// `Value`s plus a length. Both present as `&[Value]` through [`Deref`]/
/// [`DerefMut`], so every reader (accessors, GC, message copy, builtins) is
/// oblivious to which form backs a given vector. `#[repr(u8)]` pins the layout
/// (tag byte at 0; the `Inline` variant's `len` at 8, `items` at 16) for the
/// JIT's inline element read — see `jit_lower.rs`.
#[repr(u8)]
pub(crate) enum VecStore {
    Inline {
        len: u8,
        items: [Value; INLINE_VEC_CAP],
    },
    // Rust never reads `ptr`/`len` by name — JIT-lowered native code loads them
    // through the `#[repr(u8)]`-pinned byte offsets (see jit_lower.rs), which
    // dead_code analysis can't see.
    #[allow(dead_code)]
    Spill {
        /// Cached `vec.as_ptr()`, so the JIT reads spilled elements through one
        /// raw load instead of an FFI slab call (the ~20 ns/element that gated
        /// nbody's field reads and the json/regex code-vector scans). Sound
        /// because a spilled buffer never moves: vector contents are immutable
        /// (never pushed/resized after construction; `DerefMut` element writes
        /// don't reallocate), and moving the `VecStore` struct itself (slab
        /// growth, GC copy) moves three words — not the heap buffer they point
        /// to. A GC relocation builds a NEW store via [`VecStore::spill`], which
        /// re-derives the pointer.
        ptr: *const Value,
        /// Cached element count for the JIT's bounds check.
        len: u64,
        vec: Vec<Value>,
    },
}

// SAFETY: `Spill::ptr` always points into `Spill::vec`'s own buffer (established
// by the one constructor and re-derived on clone), so it is exactly as sendable/
// sharable as the `Vec` it caches.
unsafe impl Send for VecStore {}
unsafe impl Sync for VecStore {}

impl Clone for VecStore {
    fn clone(&self) -> Self {
        match self {
            VecStore::Inline { len, items } => VecStore::Inline {
                len: *len,
                items: *items,
            },
            // NOT derived: a derived clone would copy `ptr` — pointing the clone
            // at the ORIGINAL buffer. Re-derive from the cloned Vec.
            VecStore::Spill { vec, .. } => VecStore::spill(vec.clone()),
        }
    }
}

impl VecStore {
    /// The one `Spill` constructor: caches the buffer pointer + length.
    #[inline]
    fn spill(vec: Vec<Value>) -> Self {
        VecStore::Spill {
            ptr: vec.as_ptr(),
            len: vec.len() as u64,
            vec,
        }
    }

    /// Wrap owned elements, inlining when they fit (no heap allocation) and
    /// spilling otherwise. Consumes `items` so the spill path is a move, not a copy.
    #[inline]
    fn from_vec(items: Vec<Value>) -> Self {
        if items.len() <= INLINE_VEC_CAP {
            let mut inline = [Value::nil(); INLINE_VEC_CAP];
            inline[..items.len()].copy_from_slice(&items);
            VecStore::Inline {
                len: items.len() as u8,
                items: inline,
            }
        } else {
            VecStore::spill(items)
        }
    }

    /// Build from a known element count + a per-index producer, inlining without
    /// a temporary `Vec` when it fits. The GC copy path ([`flush_vector`]) uses
    /// this so relocating a small survivor allocates nothing.
    #[inline]
    fn from_flushed(len: usize, mut producer: impl FnMut(usize) -> Value) -> Self {
        if len <= INLINE_VEC_CAP {
            let mut inline = [Value::nil(); INLINE_VEC_CAP];
            for (i, slot) in inline[..len].iter_mut().enumerate() {
                *slot = producer(i);
            }
            VecStore::Inline {
                len: len as u8,
                items: inline,
            }
        } else {
            VecStore::spill((0..len).map(producer).collect())
        }
    }

    #[inline]
    fn as_slice(&self) -> &[Value] {
        match self {
            VecStore::Inline { len, items } => &items[..*len as usize],
            VecStore::Spill { vec, .. } => vec,
        }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [Value] {
        match self {
            VecStore::Inline { len, items } => &mut items[..*len as usize],
            VecStore::Spill { vec, .. } => vec,
        }
    }

    // ---- Byte layout for the JIT's inline element read (jit_lower.rs) ----
    // A `#[repr(u8)]` enum is laid out per RFC 2195 as a union of repr(C)
    // variant-structs, each prefixed by the u8 discriminant. So for the `Inline`
    // variant: discriminant @0, `len` @1, `items` @8 (8-aligned). The
    // discriminant of the first variant (`Inline`) is 0. `JIT_STRIDE` is the
    // slab stride. These are asserted against reality by `vecstore_jit_layout`.

    /// Slab stride: bytes per `VecStore` slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_STRIDE: i64 = std::mem::size_of::<VecStore>() as i64;
    /// Discriminant byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_TAG_OFF: i32 = 0;
    /// Discriminant value that means `Inline` (inline-readable).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_INLINE_TAG: i64 = 0;
    /// `Inline.len` (u8) byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_LEN_OFF: i32 = 1;
    /// `Inline.items[0]` byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_ITEMS_OFF: i32 = 8;
    /// Discriminant value that means `Spill` (pointer-readable).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_TAG: i64 = 1;
    /// `Spill.ptr` byte offset within a slot (u8 tag, padded to the pointer's align).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_PTR_OFF: i32 = 8;
    /// `Spill.len` byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_LEN_OFF: i32 = 16;
}

impl std::ops::Deref for VecStore {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        self.as_slice()
    }
}

impl std::ops::DerefMut for VecStore {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Value] {
        self.as_mut_slice()
    }
}

/// The slabs holding heap objects in the LOCAL data heap and the PRELUDE region.
#[derive(Default)]
struct Slabs {
    pairs: Vec<(Value, Value)>,
    vectors: Vec<VecStore>,
    /// Maps as a flat slab of CHAMP nodes (ADR-040). Each [`MapNode`] is
    /// either a branch (two bitmaps + packed data/children arrays) or a
    /// max-depth collision leaf. The handle in `Value::Map(MapId)` points
    /// at the trie's *root* node; child sub-nodes live in the same slab,
    /// referenced by `MapId`. The root is the only entry-point — internal
    /// nodes are reachable only through the trie itself.
    maps: Vec<MapNode>,
    strings: Vec<LocalString>,
    /// Arbitrary-precision integers (the bignum leaf, mirrors `strings`). One
    /// `num_bigint::BigInt` per live value that overflowed i64; immutable, holds
    /// no `Value` children. Every entry satisfies the normalize invariant
    /// (strictly outside the i64 range) — `Heap::int_from_bigint` enforces it.
    bigints: Vec<num_bigint::BigInt>,
    /// Arbitrary-precision base-10 decimals (mirrors `bigints` exactly). One
    /// `bigdecimal::BigDecimal` per live `Value::Decimal`; immutable, holds no
    /// `Value` children. Unlike `bigints` there is no normalize-into-`Int`
    /// invariant — a decimal is its own type and any value is stored as-is.
    decimals: Vec<bigdecimal::BigDecimal>,
    /// Exact rationals (mirrors `decimals`). One `num_rational::BigRational` per live
    /// `Value::Ratio`; immutable, holds no `Value` children. Always reduced with a
    /// positive denominator; a denominator of 1 is demoted to `Int` at construction
    /// (`Heap::alloc_ratio`), so no entry here is ever integer-valued.
    ratios: Vec<num_rational::BigRational>,
    /// **Raw bytes** — byte-clean immutable leaves, one `Arc<SharedBlob>` per live
    /// value (arbitrary bytes, never UTF-8, own slab + handle). The `Arc` is the unit
    /// of cross-process sharing (a refcount bump, not a byte copy).
    bytes: Vec<Arc<SharedBlob>>,
    /// Text ropes (ADR-045). A `ropey::Rope` is itself `Arc`-shared internally,
    /// so this slab owns one cheap handle per live rope; cloning for an edit
    /// bumps refcounts, not bytes. Always inline (no SharedBlob split — ropes
    /// don't cross processes, so there's no cross-heap aliasing to optimise).
    ropes: Vec<ropey::Rope>,
    closures: Vec<Closure>,
    natives: Vec<NativeFn>,
    envs: Vec<EnvFrame>,
}

/// Live object count of a [`Slabs`] (`Σ slab.len()`). The collector is a moving
/// copy collector that never reuses a slot in place — survivors are relocated
/// into fresh slabs and the dead dropped wholesale — so there is no free list to
/// subtract and the slab lengths *are* the live count. Shared by both
/// [`Heap::local_live_count`] (the nursery) and [`Heap::old_live_count`] (the old
/// gen), which were identical sums. `natives` is excluded (it's never GC'd — see
/// the byte-weighted [`slab_bytes`], which does count it for footprint).
impl Slabs {
    /// A fresh, empty `Slabs` whose per-slab `Vec`s carry the **capacity of
    /// `like`'s lengths** — the flip-side nursery allocator. A minor collection
    /// used to install `Slabs::default()` (zero capacity), so every cycle
    /// re-paid the full Vec-doubling ladder up to the nursery threshold — each
    /// doubling memmoves everything allocated so far, ~12 % of an
    /// allocation-bound run (bintree) went to those copies. The previous
    /// nursery's *lengths* are the steady-state high-water mark (each cycle
    /// allocates about as much as the last), so reserving them up front makes
    /// the next cycle's pushes copy-free while releasing the memory of any
    /// one-off spike (capacity follows the last cycle's actual use, not max).
    fn with_capacity_like(like: &Slabs) -> Slabs {
        Slabs {
            pairs: Vec::with_capacity(like.pairs.len()),
            vectors: Vec::with_capacity(like.vectors.len()),
            maps: Vec::with_capacity(like.maps.len()),
            strings: Vec::with_capacity(like.strings.len()),
            bigints: Vec::with_capacity(like.bigints.len()),
            decimals: Vec::with_capacity(like.decimals.len()),
            ratios: Vec::with_capacity(like.ratios.len()),
            bytes: Vec::with_capacity(like.bytes.len()),
            ropes: Vec::with_capacity(like.ropes.len()),
            closures: Vec::with_capacity(like.closures.len()),
            natives: Vec::new(), // never GC'd, never grows here
            envs: Vec::with_capacity(like.envs.len()),
        }
    }
}

fn slab_live_count(s: &Slabs) -> usize {
    s.pairs.len()
        + s.vectors.len()
        + s.maps.len()
        + s.strings.len()
        + s.bigints.len()
        + s.decimals.len()
        + s.ratios.len()
        + s.bytes.len()
        + s.ropes.len()
        + s.closures.len()
        + s.envs.len()
}

/// Byte-weighted footprint of a [`Slabs`] (`Σ slab.len() * size_of::<elem>`) —
/// the slab arrays themselves, not nested/shared content (inner spilled vectors,
/// string bytes, `Arc`-shared ropes/blobs). A comparative figure, not exact RSS.
/// Counts `natives` too (unlike [`slab_live_count`]). Backs [`Heap::local_bytes`].
///
/// **O(1)** — every term is `slab.len() * size_of::<elem>`, no per-element walk.
/// This matters: [`Heap::local_bytes`] is republished on every `receive` park, and
/// an earlier `Σ vectors.spilled_bytes()` term (walking every VecStore) made it
/// O(heap) — ~50% of a tight message-passing loop. Excluding spilled buffers also
/// squares the code with this doc's "not nested/shared content" contract; a
/// process with large spilled vectors under-reports `:memory` by their buffer
/// bytes, acceptable for a comparative observability figure (the hard memory cap
/// uses the global allocator counter, not this).
/// How much retained capacity must have accumulated **since the last trim** before
/// [`Heap::trim_parked`] does anything.
///
/// Note the "since the last trim": an absolute size threshold is the obvious design and it
/// is wrong in both directions, which the 2026-07-28 measurements showed plainly.
///
/// * A **high** absolute gate (32 KiB) is latency-safe but makes memory non-monotonic in
///   allocation: a process that consed 1,000 pairs crossed it and got trimmed to 8.5 KB,
///   while one that consed *100* stayed under and kept **14.5 KB** — allocating more used
///   less, which is indefensible to explain to a user.
/// * A **low** absolute gate (4 KiB) fixes that (5.4 → 8.5 → 8.5 KB, monotonic) and costs
///   `pingpong` **+193%** (213 → 624 ms), because that row parks 200k times and each park
///   now pays a collection.
///
/// Growth-since-last-trim gets both. A responder that parks constantly reaches a steady
/// working set, so after one trim its capacity stops growing and it never trims again — it
/// pays one subtraction per park. A process that actually accumulated something trims once
/// per accumulation, regardless of how large its heap is in absolute terms.
///
/// Measured in [`park_trim_probe`] slots rather than bytes so the gate stays a few loads:
/// 64 slots is roughly a few KiB of pairs, i.e. the same intent as the 4 KiB it replaces.
const PARK_TRIM_GROWTH_SLOTS: usize = 64;

/// Retained *capacity* of a slab set, in bytes — what the process is holding from the
/// allocator, as opposed to [`slab_bytes`]'s live contents. The two diverge sharply for a
/// process that allocated and then dropped: the `Vec`s keep their high-water capacity, and a
/// nursery flip deliberately preserves it (`Slabs::with_capacity_like`) so the next cycle
/// does not re-pay the doubling ladder. That is right for a *running* process and wrong for
/// a parked one, which may hold it for the rest of the program.
fn slab_capacity_bytes(s: &Slabs) -> usize {
    use std::mem::size_of;
    s.pairs.capacity() * size_of::<(Value, Value)>()
        + s.vectors.capacity() * size_of::<VecStore>()
        + s.maps.capacity() * size_of::<MapNode>()
        + s.strings.capacity() * size_of::<LocalString>()
        + s.bigints.capacity() * size_of::<num_bigint::BigInt>()
        + s.decimals.capacity() * size_of::<bigdecimal::BigDecimal>()
        + s.ratios.capacity() * size_of::<num_rational::BigRational>()
        + s.bytes.capacity() * size_of::<Arc<SharedBlob>>()
        + s.ropes.capacity() * size_of::<ropey::Rope>()
        + s.closures.capacity() * size_of::<Closure>()
        + s.natives.capacity() * size_of::<NativeFn>()
        + s.envs.capacity() * size_of::<EnvFrame>()
}

/// A **cheap** stand-in for retained capacity, in slab elements, for the park-time gate.
///
/// [`slab_capacity_bytes`] sums eleven `capacity()` fields per generation and multiplies each
/// by a size — ~25 ns, which is nothing once, and everything on a path that runs on every
/// park. `ring` parks a million times: the full sum cost it **+4.6%** while the trims it
/// gated cost nothing measurable. Three element counts from the nursery track growth just as
/// well for a heuristic (pairs, vectors and env frames are what a working set is made of),
/// and the trim itself still measures real bytes.
#[inline]
fn park_trim_probe(s: &Slabs) -> usize {
    s.pairs.capacity() + s.vectors.capacity() + s.envs.capacity()
}

/// Hand every slab's unused capacity back to the allocator.
fn shrink_slabs(s: &mut Slabs) {
    s.pairs.shrink_to_fit();
    s.vectors.shrink_to_fit();
    s.maps.shrink_to_fit();
    s.strings.shrink_to_fit();
    s.bigints.shrink_to_fit();
    s.decimals.shrink_to_fit();
    s.ratios.shrink_to_fit();
    s.bytes.shrink_to_fit();
    s.ropes.shrink_to_fit();
    s.closures.shrink_to_fit();
    s.natives.shrink_to_fit();
    s.envs.shrink_to_fit();
}

fn slab_bytes(s: &Slabs) -> usize {
    use std::mem::size_of;
    s.pairs.len() * size_of::<(Value, Value)>()
        + s.vectors.len() * size_of::<VecStore>()
        + s.maps.len() * size_of::<MapNode>()
        + s.strings.len() * size_of::<LocalString>()
        + s.bigints.len() * size_of::<num_bigint::BigInt>()
        + s.decimals.len() * size_of::<bigdecimal::BigDecimal>()
        + s.ratios.len() * size_of::<num_rational::BigRational>()
        + s.bytes.len() * size_of::<Arc<SharedBlob>>()
        + s.ropes.len() * size_of::<ropey::Rope>()
        + s.closures.len() * size_of::<Closure>()
        + s.natives.len() * size_of::<NativeFn>()
        + s.envs.len() * size_of::<EnvFrame>()
}

/// The immutable, read-only prelude region (closures, code values, the
/// builtins). Built once, then shared by `Arc` into every runtime.
#[derive(Default)]
pub struct SharedCode {
    slabs: Slabs,
    /// Where each prelude global was defined — `name → (cache-file, pos)`,
    /// recorded once during the prelude build (the file is the materialized
    /// `prelude.blsp` copy; see `lib.rs`). Immutable like the rest of this
    /// region, and consulted by [`Heap::def_site`] *after* the runtime table so
    /// a user redefinition of a prelude name still wins. Powers cross-file
    /// goto-definition into the standard library (ADR-031, docs/lsp.md).
    def_sites: HashMap<Symbol, SourceLoc>,
}

/// A snapshot of the LOCAL heap's sizes, taken at a top-level boundary. Passing
/// it back to [`Heap::reset_local_to`] reclaims everything allocated since (see
/// there for the safety contract). This is the arena-reset reclamation strategy
/// (`docs/memory-model.md`): at a quiescent point the LOCAL heap holds nothing
/// live but the form's result, because globals live in PRELUDE/RUNTIME and never
/// point into LOCAL.
#[derive(Clone, Copy)]
pub struct LocalCheckpoint {
    pairs: usize,
    vectors: usize,
    maps: usize,
    strings: usize,
    bigints: usize,
    decimals: usize,
    ratios: usize,
    bytes: usize,
    ropes: usize,
    closures: usize,
    envs: usize,
    // The `local_epoch` the checkpoint was taken in. A collection between the
    // checkpoint and its `reset_local_to` bumps the epoch and rewrites the
    // nursery (a flip compacts survivors into fresh slabs; a tenure empties it),
    // so the slab lengths above no longer describe the live nursery — truncating
    // to them would strand the survivors the collector just kept. `reset_local_to`
    // compares this against the current epoch and skips the truncation on a
    // mismatch (the collection already reclaimed the dead). See its body.
    epoch: u32,
    // No `natives` field: a live runtime never allocates a native into its LOCAL
    // heap (they're registered once during the prelude build, then frozen into
    // PRELUDE). If that ever changes, add a field here and truncate it below.
}

/// Append-only code slabs for the shared RUNTIME region. `boxcar::Vec` gives
/// lock-free reads that return stable references (existing elements never move
/// or free as the vector grows), so process threads read closure bodies without
/// locking while another process `def`s new code.
#[derive(Default)]
struct CodeSlabs {
    pairs: boxcar::Vec<(Value, Value)>,
    vectors: boxcar::Vec<VecStore>,
    maps: boxcar::Vec<MapNode>,
    strings: boxcar::Vec<LocalString>,
    /// Bignums `def`'d into a global / baked as a literal into shared RUNTIME
    /// code (mirrors `strings`). Immutable, holds no handles; append-only.
    bigints: boxcar::Vec<num_bigint::BigInt>,
    /// Decimals `def`'d into a global / baked as a literal into shared RUNTIME
    /// code (mirrors `bigints`). Immutable, holds no handles; append-only.
    decimals: boxcar::Vec<bigdecimal::BigDecimal>,
    /// Rationals `def`'d into a global / baked as a literal into shared RUNTIME
    /// code (mirrors `decimals`). Immutable, holds no handles; append-only.
    ratios: boxcar::Vec<num_rational::BigRational>,
    /// Raw bytes `def`'d into a global / captured by a promoted closure (mirrors
    /// `bigints`). Byte-clean `Arc<SharedBlob>`, never read as UTF-8.
    /// Append-only; the Arc is shared, not copied.
    bytes: boxcar::Vec<Arc<SharedBlob>>,
    /// Ropes `def`'d into a global (shared read-only across this runtime's
    /// processes). A `ropey::Rope` is `Send + Sync` and immutable-by-construction
    /// here (every edit makes a fresh LOCAL rope), so sharing one by handle is
    /// sound. Append-only like the rest of this region.
    ropes: boxcar::Vec<ropey::Rope>,
    /// `OnceLock`-wrapped so `promote` can **reserve a slot, then fill it** — the
    /// append-only `boxcar` can't write-back the way the GC's mutable slabs do, so
    /// a *cyclic* promote (a closure whose captured scope binds the closure itself,
    /// e.g. `(let (g (fn () g)) g)` or mutually-recursive `letrec` closures) would
    /// otherwise recurse forever → SIGSEGV. Reserve-then-fill lets the recursion
    /// resolve the back-edge to the reserved handle. Each cell is set exactly once
    /// before the handle is ever published, so reads (`get().unwrap()`) never race.
    closures: boxcar::Vec<OnceLock<Closure>>,
    /// Captured environments of promoted closures. A closure defined *inside a
    /// function call* (not at top level) closes over a local scope; promoting it
    /// for sharing copies that scope here so it resolves in any process. Frozen
    /// once promoted (read-only), so append-only is sound. `OnceLock`-wrapped for
    /// the same reserve-then-fill cycle break as `closures` above.
    envs: boxcar::Vec<OnceLock<EnvFrame>>,
}

/// A borrow into a slab, valid for as long as the wrapper is held. It is either a
/// **direct** `&self`-borrow (LOCAL / PRELUDE, or a compaction-time RUNTIME read) or
/// a **pinned** borrow into an ArcSwap-managed RUNTIME generation, where the held
/// `Arc<CodeSlabs>` keeps that generation's slab alive so a concurrent Stage-4 free
/// ([`Heap::free_runtime_gen`], ADR-091) can swap the slab out without invalidating
/// an in-flight read. `Deref`s to `T`, so call sites use it exactly like `&T`.
///
/// The RUNTIME pin is a plain `Arc` clone obtained from a per-process **version-gated
/// cache** ([`Heap::code_gen_pinned`]) rather than a fresh `ArcSwap::load` guard per
/// deref: the latter's hybrid-strategy load dominated global-data-heavy hot loops (a
/// read of a `def`'d matrix element in `matmul` derefs a RUNTIME handle, ~16 M times).
pub struct SlabRef<'a, T: ?Sized> {
    /// Keeps the RUNTIME generation's `Arc<CodeSlabs>` alive while borrowed; `None`
    /// for a direct borrow. Never read directly — held purely so its `Drop` (the
    /// Arc release) runs no earlier than the pointer's last use.
    _pin: Option<Arc<CodeSlabs>>,
    /// Points into the borrowed slot — a direct `&'a T`, or into the slab the pin
    /// keeps alive. Valid for the wrapper's whole lifetime either way.
    ptr: *const T,
    _life: std::marker::PhantomData<&'a T>,
}

// SAFETY: `SlabRef` is a plain shared borrow (a `&T` plus, optionally, the `Arc`
// that keeps `T` alive). It is `Send`/`Sync` exactly when `&T` is — the pin is an
// `Arc` clone (already `Send`+`Sync` for our `CodeSlabs`), and the raw pointer only
// ever yields shared `&T` access.
unsafe impl<T: ?Sized + Sync> Sync for SlabRef<'_, T> {}
unsafe impl<T: ?Sized + Sync> Send for SlabRef<'_, T> {}

impl<'a, T: ?Sized> SlabRef<'a, T> {
    /// A direct `&self`-borrow (LOCAL / PRELUDE, or a compaction-time RUNTIME read).
    #[inline]
    fn direct(r: &'a T) -> Self {
        SlabRef {
            _pin: None,
            ptr: r as *const T,
            _life: std::marker::PhantomData,
        }
    }
    /// A pinned borrow into a RUNTIME generation the `pin` `Arc` keeps alive.
    ///
    /// SAFETY: `ptr` must point into the `CodeSlabs` held alive by `pin` (obtained
    /// from `&*pin`), so it stays valid for the wrapper's whole lifetime.
    #[inline]
    unsafe fn pinned(pin: Arc<CodeSlabs>, ptr: *const T) -> Self {
        SlabRef {
            _pin: Some(pin),
            ptr,
            _life: std::marker::PhantomData,
        }
    }
    /// Re-project the borrow to a part of `T` (e.g. a field), carrying the same pin
    /// so the underlying slab stays alive. Like `Ref::map`.
    #[inline]
    pub(crate) fn map<U: ?Sized>(self, f: impl FnOnce(&T) -> &U) -> SlabRef<'a, U> {
        // SAFETY: `self.ptr` is valid (invariant of `SlabRef`); the projected `&U`
        // points within the same slab the pin (moved below) keeps alive.
        let ptr = f(unsafe { &*self.ptr }) as *const U;
        SlabRef {
            _pin: self._pin,
            ptr,
            _life: std::marker::PhantomData,
        }
    }
}

impl<T: ?Sized> std::ops::Deref for SlabRef<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: `ptr` is either a direct `&'a T` or points into the slab the held
        // `_guard` keeps alive; both outlive `&self`.
        unsafe { &*self.ptr }
    }
}

// `&T`-like ergonomics so a `SlabRef` drops into most call sites unchanged.
impl<T: ?Sized> AsRef<T> for SlabRef<'_, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}
impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SlabRef<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}
impl<T: ?Sized + std::fmt::Display> std::fmt::Display for SlabRef<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}
impl<T: ?Sized + PartialEq> PartialEq for SlabRef<'_, T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}
impl<T: ?Sized + PartialEq> PartialEq<T> for SlabRef<'_, T> {
    #[inline]
    fn eq(&self, other: &T) -> bool {
        **self == *other
    }
}
// Comparing a `SlabRef<str>` against a string literal / `&str` (`sr == "foo"`).
impl PartialEq<&str> for SlabRef<'_, str> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        &**self == *other
    }
}

impl CodeSlabs {
    /// True if this generation holds no code — every slab empty. Aging may only
    /// start a new generation in a slot that is empty (its previous generation
    /// fully reclaimed), so a fresh gen's handle indices can't collide with a
    /// stale generation's still-live handles (the 2-versions-max rule, ADR-091).
    fn is_empty(&self) -> bool {
        self.pairs.count() == 0
            && self.vectors.count() == 0
            && self.maps.count() == 0
            && self.strings.count() == 0
            && self.bigints.count() == 0
            && self.decimals.count() == 0
            && self.ratios.count() == 0
            && self.bytes.count() == 0
            && self.ropes.count() == 0
            && self.closures.count() == 0
            && self.envs.count() == 0
    }
}

/// A runtime's mutable, shared code region: the code `def`'d at runtime plus the
/// global bindings table. All of a runtime's inner processes share one of these
/// (via `Arc::clone`), which is what makes a `def` propagate to them — and what
/// keeps separate runtimes (nodes) independent (each has its own).
/// A fast hasher for `Symbol` (`u32`) keys. The globals table is consulted on
/// every global reference (every operator / prelude call), and the default
/// SipHash is overkill — and notably slow to finalize — for a single `u32`.
/// FxHash-style: one wrapping multiply per key. `write_u32` is the only path that
/// runs for a `Symbol`, and multiplying by an odd constant is a bijection, so
/// distinct symbols never collide.
#[derive(Default)]
pub struct SymbolHasher(u64);

impl std::hash::Hasher for SymbolHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.0 = (self.0 ^ i as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        // The hot path for a `VmCacheKey` (its handle `.0`): same odd-multiply
        // bijection as `write_u32`, so distinct handles never collide.
        self.0 = (self.0 ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write(&mut self, bytes: &[u8]) {
        // Fallback for any non-`u32` key (none on the hot path); kept correct.
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// A `HashMap` keyed by interned `Symbol`s, using the fast [`SymbolHasher`].
pub type SymbolMap<V> = HashMap<Symbol, V, std::hash::BuildHasherDefault<SymbolHasher>>;

/// A `HashMap` keyed by [`VmCacheKey`], using the fast [`SymbolHasher`] (its
/// manual `Hash` writes a single `u64`, so it takes the `write_u64` fast path).
/// The compiling VM hits this on **every closure call** (`compiled_for`), so the
/// stock `SipHash` was pure per-call overhead (perf #2).
pub type VmCacheMap<V> = HashMap<VmCacheKey, V, std::hash::BuildHasherDefault<SymbolHasher>>;

/// The [`Heap::lookup_closure_template`] cache map: `fn_rest` [`PairId`] → parsed
/// [`ClosureTemplate`], on the fast [`SymbolHasher`] (a `PairId` writes one `u64`).
type ClosureTemplateMap =
    HashMap<PairId, Arc<ClosureTemplate>, std::hash::BuildHasherDefault<SymbolHasher>>;

/// The [`Heap::lookup_const_closure`] cache map: a capture-free `(fn …)` literal's
/// `fn_rest` [`PairId`] → the **promoted RUNTIME closure handle** built for it once.
type ConstClosureMap = HashMap<PairId, Value, std::hash::BuildHasherDefault<SymbolHasher>>;

/// Which update [`Heap::registry_update`] performs. See that method for why the whole
/// read-modify-write has to happen inside one kernel call (KI-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryOp {
    /// Set `path` to the value, creating the intermediate map if needed.
    Assoc,
    /// Set `path` only if it is currently absent.
    AssocNew,
    /// Remove a one-key `path`.
    Dissoc,
    /// Prepend to a list-valued global unless already a member.
    ConsNew,
}

pub struct RuntimeCode {
    /// The **two** code generations (ADR-091 Erlang-style 2-generation collector).
    /// New code (`def`/`promote`) lands in `gens[current_gen]`; the *other* slot
    /// holds the previous generation's still-referenced code during a migration
    /// (freed once no live process references it). A RUNTIME handle self-describes
    /// its generation ([`code_gen`](crate::core::value::PairId::code_gen)), so a
    /// read resolves `gens[handle.code_gen()]` — no shared read on the hot path.
    /// Until aging is wired, `current_gen` stays `0` and `gens[1]` is empty, so this
    /// behaves exactly like the former single `code: CodeSlabs`.
    ///
    /// Each slot is an [`ArcSwap`] so a drained generation can be **freed while the
    /// runtime is shared** (ADR-091 Stage 4): [`Heap::free_runtime_gen`] stores a
    /// fresh empty `CodeSlabs`, and the old `Arc` drops once the last reader releases
    /// its [`Guard`]. Reads stay lock-free (`gens[g].load()`); appends push into the
    /// loaded slab's `boxcar` in place (visible to every holder of that `Arc`), so a
    /// store only ever happens on a free — never on the `def`/`promote` hot path.
    gens: [ArcSwap<CodeSlabs>; 2],
    /// Index (0 or 1) of the current code generation — read at `promote`/collect,
    /// never on the hot read path (the handle carries its own generation).
    current_gen: AtomicUsize,
    /// A process-wide-unique tag for this runtime instance — a plain `u64` the
    /// background JIT compiler keys its per-runtime publish cache by. Carried
    /// inside compile work items INSTEAD of the runtime `Arc` (or a `Weak`):
    /// either would park a reference in the queue and break the single-process
    /// RUNTIME compactor's `Arc::get_mut` uniqueness gate. Distinct per runtime,
    /// so shared native code never leaks across independent runtimes. (Read only
    /// by the JIT publish path — dead in a no-`jit` build, but kept unconditional
    /// so the struct layout and construction don't fork on the feature.)
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    runtime_tag: u64,
    /// Per-generation count of **in-flight shared-closure messages** — a queued
    /// `Message::FnShared` holding a RUNTIME handle into that generation.
    ///
    /// Why a counter rather than the reachability probe. A shared handle that has *landed*
    /// in a receiver's LOCAL heap is already sound: the drain's Phase 2 walks the whole
    /// local heap, so `runtime_gen_referenced` sees it (this is ADR-194's argument for the
    /// L1 path). A handle **still queued** is in no heap and no process's roots, so nothing
    /// walks it — and it cannot be found by extending the probe either, because
    /// `report_gen_liveness` caches a process's clean ack for the whole epoch on the
    /// explicit grounds that *"an old-gen handle can never arrive by message (messages
    /// deep-copy)"*. This counter restores that guarantee from the other side: while a
    /// generation has messages in flight against it, `free_runtime_gen` refuses. The pin is
    /// released by `GenPin`'s `Drop`, so every path that discards a message — a dead target,
    /// a dropped mailbox, a routing failure — releases it without a manual decrement.
    gen_inflight: [AtomicUsize; 2],
    /// Monotonic version of the `gens` **`Arc` identities**, bumped only when a slot's
    /// `Arc<CodeSlabs>` is *replaced* — a Stage-4 free or a compaction store, both rare
    /// (never on the `def`/`promote`/append hot path, which mutates a loaded slab's
    /// `boxcar` in place without swapping the `Arc`). It gates the per-process pinned
    /// read cache ([`Heap::code_gen_pinned`]): a RUNTIME deref clones the *cached* `Arc`
    /// when this version is unchanged, avoiding the `ArcSwap::load` hybrid-strategy cost
    /// that dominated global-data-heavy hot loops. An aging flip changes `current_gen`
    /// but not either slot's `Arc`, so it deliberately does **not** bump this — a cached
    /// pin stays valid across it (a handle carries its own generation index). `Relaxed`
    /// suffices: the cache re-`load_full`s on any change, which republishes the `Arc`.
    gen_version: AtomicU64,
    /// The global bindings (prelude + user `def`s). Read on every global lookup,
    /// written on `def` (the only mutation). The values point into PRELUDE or RUNTIME.
    globals: RwLock<SymbolMap<Value>>,
    /// Serialises a **registry update** — the read-modify-write of a global that holds a
    /// whole registry map (`*impls*`, `*features*`, `*abilities*`, … — see
    /// [`Heap::registry_update`]). `def` itself is atomic, but `(def *X* (assoc *X* …))` is
    /// three steps in the language, and two processes registering at once each read the old
    /// map and each write their own successor, so the later write silently drops the
    /// earlier one (KI-22: ~40% of concurrent registrations lost). This lock lets the whole
    /// sequence happen inside ONE kernel call.
    ///
    /// Separate from `globals` on purpose: the update needs `&mut Heap` for the map ops
    /// between the read and the write, which it could not do while holding a guard borrowed
    /// from `self`. Nothing acquires this while holding the `globals` lock, so there is no
    /// ordering hazard. Registration is a load-time/hot-reload event, so the contention is
    /// nil and holding it briefly on the worker thread is free.
    registry_lock: Mutex<()>,
    /// **Reserved** names — everything the language itself ships, which a user `def`
    /// may not rebind (ADR-166). Seeded with every symbol bound at runtime-seed time
    /// (the prelude's 443 definitions plus every Rust builtin), and extended with each
    /// name an *embedded* std module defines as it loads. The rule the boundary
    /// encodes: **if it shipped inside the `brood` binary it is reserved; if you or a
    /// package author wrote it, it is yours** — so hot-reloading your own code, and a
    /// dependency's, is untouched, which is all the live-editing story ever needed.
    ///
    /// Read only when a global `def` runs (rare), so a `HashSet` probe costs nothing
    /// on any hot path. Shared through the runtime `Arc`, so every inner process sees
    /// one reserved set.
    sealed: RwLock<std::collections::HashSet<Symbol>>,
    /// Module-private globals (ADR-146): the qualified [`Symbol`] of every global
    /// whose bare tail carries the `--` privacy marker. This makes privacy a
    /// **recorded fact** rather than a name re-parsed at each consultation site:
    /// the `--` marker is read once, where the binding is made (`env_define`, and
    /// derived from bindings in `seeded` for the prelude, which is inserted rather
    /// than re-`eval`ed), and every semantic privacy check reads this set via
    /// [`Heap::is_private`] instead of scanning the name string. The marker stays
    /// the *populator*; changing it later is a change to the populate sites only.
    /// Shared through the runtime `Arc`, so every inner process sees one set. Only
    /// `--` names are ever inserted, so a name without `--` needs no lock to answer.
    private: RwLock<std::collections::HashSet<Symbol>>,
    /// Monotonic version of `globals`, bumped on every binding change (`def`
    /// rebind, `restore_globals`). Per-process global **inline caches**
    /// (`Heap::global_ic`) stamp the version they resolved at and re-resolve only
    /// when it has moved — so a steady-state global read is an atomic load + a
    /// local hash hit instead of taking the shared `RwLock`. Late-binding stays
    /// exact: any `def` makes every stamped cache entry stale at once. `Relaxed`
    /// is sufficient — a global value is an immovable PRELUDE/RUNTIME handle, so
    /// there's no data it gates publication of; the counter only has to *change*.
    version: AtomicU64,
    /// Where each global was *defined* — file + form position, recorded at load
    /// time before macroexpansion (ADR-031). Lives here, beside `globals`, so it
    /// is shared across a runtime's processes and updated by a redefinition, the
    /// same as the bindings it describes. Read by `(source-location 'name)`; the
    /// image-query foundation for cross-file goto-definition.
    def_sites: RwLock<HashMap<Symbol, SourceLoc>>,
    /// Source positions of RUNTIME *list forms*, keyed by the pair's RUNTIME slab
    /// index — the RUNTIME counterpart of the per-heap LOCAL [`Heap::form_pos`] map.
    /// The reader stamps positions on LOCAL pairs; `promote` carries them here when a
    /// form is frozen into RUNTIME (a `defn` body, or a top-level inline lambda baked
    /// for VM-compilation), so `(form-pos …)` still resolves and a position survives a
    /// cross-node send (`Message::List`). Append-only in practice (a RUNTIME pair never
    /// moves), so entries stay valid; shared across the runtime's processes via `Arc`.
    positions: RwLock<HashMap<usize, (crate::error::Pos, Option<Arc<str>>)>>,
    /// Shared JIT native-code cache (ADR-101, the spawn lever): maps a simple
    /// fixed-arity RUNTIME/PRELUDE closure arm's `(closure_id, argc)` key (see
    /// `CompiledArm::share_key`) to its compiled native code as
    /// `(code_ptr_as_usize, compile_epoch)`. The first process to JIT such an arm
    /// publishes here; every other process of this runtime installs the pointer
    /// directly (epoch-checked) instead of re-tiering + recompiling its own copy — so
    /// a hot shared function (`fib` under `spawn`) compiles to native ONCE, not once
    /// per process (the spawn-14× cause). The code lives in the process-lifetime
    /// GLOBAL_JIT module (never freed or moved), so the raw pointer is valid across
    /// processes/threads; the `compile_epoch` is checked against `version` (this
    /// struct's `global_epoch`) on install, so a `def` or RUNTIME compaction — both
    /// bump `version` — invalidates every entry without a sweep. Stored as `usize`
    /// because a raw code pointer isn't `Send`/`Sync`; reconstituted on read. Empty
    /// unless the JIT runs.
    jit_code_cache: RwLock<HashMap<(u64, u16), (usize, u64)>>,
    /// **Shared compiled-closure cache** (ADR-175 Phase B — the BEAM module-area move):
    /// PRELUDE closure handle bits → the compiled closure, shared by every process of
    /// this runtime. Before this, each green process compiled its own copy of every
    /// prelude function it called (~18 KB per distinct callee per process — the
    /// spawn-live 4.5 GB cause). Eligibility is strict (see `compiled_arm_for`):
    /// PRELUDE-region key (never freed/recycled, so no ADR-091 free-epoch discipline
    /// needed here) and **immortal** arms (no RUNTIME-region handle anywhere, so
    /// `runtime_collect`'s per-process rewrite never touches them — a shared arm
    /// rewritten by two processes would double-forward its handles). Publish is
    /// idempotent: every process compiles the identical closure from the same shared
    /// AST, so last-writer-wins is safe. `BROOD_NO_SHARED_ARMS=1` bypasses (ADR-175's
    /// off-switch). Arm site ids are arm-relative (Phase A), so a shared arm's ICs
    /// work in every process, each against its own block.
    /// Value is `(free_epoch_at_compile, closure)`. The stamp is read **before** the
    /// publisher compiles and validated against the live `free_epoch` on lookup, so a
    /// closure compiled against a generation that was freed mid-compile can never be
    /// installed by anyone (ADR-091: a freed slot is reused with bit-identical
    /// `(gen, index)` handles, which is exactly what the per-process `vm_cache` guards
    /// with `sync_free_epoch`).
    shared_closures: RwLock<HashMap<u64, (u64, Arc<crate::eval::compile::CompiledClosure>)>>,
    /// Companion to `jit_code_cache` for the two-stage-tiering **inlined** upgrade
    /// (the deferred, self-inlined body). Same `(closure_id, argc)` key and
    /// `(code_ptr, compile_epoch)` value, but a separate map because a slot holds
    /// either the small native (that cache) or the inlined native (this one), never
    /// both. Sharing the inlined native across a runtime's processes — exactly as the
    /// small native already is — means ONE inlined compile serves every process instead
    /// of each of N spawned workers compiling (and, for a short fan-out like `pfib`,
    /// finishing before) its own copy; the inlined win then lands for short parallel
    /// bursts too. Safe because `inline_nslots` is deterministic for a given bytecode
    /// (so a peer sizes its own frame correctly on install) and the epoch guard flushes
    /// it on `def`/compaction just like the small-native cache. See
    /// [`Heap::jit_inline_lookup`] / [`Heap::jit_inline_publish`].
    jit_inline_cache: RwLock<HashMap<(u64, u16), (usize, u64)>>,
    /// User-declared `(sig name type)` signatures, keyed by the **module-qualified**
    /// global `Symbol` (the same key `def` produces for `name`) and holding the raw
    /// type-expression as a promoted RUNTIME `Value` (e.g. the `(int -> int)` form).
    /// Registered by the `%register-sig` primitive when a `(sig …)` form evaluates,
    /// so a declared sig is visible to the checker's `sig_of` *first* — ahead of
    /// primitive/curated/inferred — both intra-module (the call resolves to the
    /// qualified name the file-local ctx misses) and cross-module (`nest check`
    /// loads the whole project image, so b's sig is present when a's caller is
    /// checked). The stored value is a `Value`, not a `types::Sig`: the `core`
    /// layer must not depend on `types` (the checker parses it on read). Shared
    /// across the runtime's processes via `Arc`, like `globals`.
    declared_sigs: RwLock<SymbolMap<Value>>,
    /// **RUNTIME collector — Stage 3b (cooperative drain coordination, ADR-091).**
    /// When an aged-out generation is being reclaimed, each of the runtime's
    /// processes cooperatively reports — at its safepoint / before parking —
    /// whether it still references the draining generation
    /// ([`Heap::runtime_gen_referenced`]). The old generation is dead (Stage 4 may
    /// free it) only once *every* live process has reported clean for the current
    /// drain epoch. Shared across the runtime's processes via `Arc`, like `globals`.
    ///
    /// `drain_active` is `false` when no drain is in progress (the always-case until
    /// Stage 4 arms one, so the whole mechanism is inert by default). `drain_gen` is
    /// the generation being reclaimed. `drain_epoch` is **strictly monotonic** (a new
    /// drain bumps it and clears `drain_acks`), so a stale ack from a previous drain
    /// can never be mistaken for a current-epoch one. `drain_acks` maps a process's
    /// pid → the epoch it last reported *clean* for; a process still referencing the
    /// draining generation has no current-epoch entry, so it pins the generation.
    drain_active: AtomicBool,
    drain_gen: AtomicUsize,
    drain_epoch: AtomicU64,
    /// **O(1) drain-completion gate (ADR-091).** A running count of *distinct*
    /// processes that have reported clean for the current drain epoch (reset to 0
    /// by `begin_gen_drain`, bumped once per new ack in `report_gen_liveness`). The
    /// process layer's `old_gen_drained` compares it to the live-process count as a
    /// cheap gate: while `drain_acked < live` some process still pins the generation,
    /// so it skips the O(live-process) parked-liveness registry scan + mailbox-lock
    /// sweep entirely — the whole cost of a lingering drain (a `spawn` fan-out where
    /// every child's body pins the draining gen made this ~300× at scale). It only
    /// grows within an epoch (an acked process that later exits is not decremented),
    /// which is sound: the count can only *over*-report completion, and the actual
    /// free is still gated by the authoritative `gen_drained` scan below the gate —
    /// so a stale count can at worst run the scan a bit early (never free early).
    drain_acked: AtomicU64,
    /// RUNTIME-churn dirty bit: set true whenever a closure is minted into the
    /// current code generation (`promote_closure` — i.e. every `def`/`spawn`/
    /// hot-reload `promote`). The eval safepoint reads it to decide whether the
    /// (relatively costly) `rt_gc_due` probe — an `ArcSwap` load + a closure count
    /// — is worth running: the RUNTIME region only grows on a mint, which never
    /// happens inside a hot compute loop, so a def-free loop (`fib`, `reduce`,
    /// `apply`) skips the probe entirely. Cleared once the safepoint has run the
    /// probe. A plain relaxed `bool`: a read keeps the cache line Shared across
    /// worker cores (no invalidation), a mint writes it once.
    rt_dirty: AtomicBool,
    drain_acks: RwLock<HashMap<u64, u64>>,
    /// **RUNTIME collector — Stage 4 (free-generation epoch, ADR-091).** Bumped each
    /// time a generation is freed ([`Heap::free_runtime_gen`]). A freed slot is later
    /// reused by aging, minting handles with bit-identical `(gen, index)` to the freed
    /// ones — so a per-process `vm_cache` entry (keyed on the closure handle bits, not
    /// version-stamped) could otherwise alias *old* compiled code onto *new* code. Each
    /// process compares this against its own [`Heap::seen_free_epoch`] on the
    /// `vm_cache` read path and clears its `vm_cache` once when it advances. The
    /// version-stamped caches (`global_ic`, the call/global ICs, the shared JIT caches)
    /// self-invalidate on the `version` bump a free also does, so only `vm_cache` needs
    /// this. Relaxed: it only has to *change* (a lazy one-shot cache clear, no data
    /// publication gated on it — the freed slab is already unreachable by the drain).
    free_epoch: AtomicU64,
    /// **RUNTIME collector — Stage 4 (single-flight aging, ADR-091).** Held for the
    /// duration of an `age + migrate_live_globals + begin_gen_drain` sequence so at
    /// most one process ages at a time. Two processes racing the safepoint could both
    /// observe the other slot empty and both run the migration, double-copying the
    /// live image into the new generation (wasteful, and the second's reconcile would
    /// mostly no-op). A plain CAS gate ([`Heap::begin_aging`]/[`Heap::end_aging`]) —
    /// the loser skips this safepoint and retries at the next one.
    aging: AtomicBool,
    /// **RUNTIME collector — Stage 4 (aging counter, ADR-091).** Bumped by
    /// [`Heap::age_runtime`]; surfaced via [`Heap::runtime_aged_count`] so a test can
    /// confirm the multi-generation collector aged, even when a full free is timing-
    /// dependent.
    aged_count: AtomicU64,
    /// **RUNTIME collector — Stage 4 (promote⇄age mutual exclusion, ADR-091).** A
    /// generation flip ([`Heap::age_runtime`]) must not interleave with an in-flight
    /// [`Heap::promote`] on another process: promote reserves a slot in the current
    /// generation and then fills it, re-reading `cur_code()` — if aging flipped
    /// `current_gen` in between, the fill would target the *wrong* generation's slab
    /// (a panic or cross-generation-split closure). Promotion holds this **read** lock
    /// (many concurrent promotes are fine — they append to a lock-free `boxcar`);
    /// aging holds the **write** lock, so the flip waits for every in-flight promote to
    /// finish and no promote ever spans it. Uncontended on the default single-generation
    /// path (nothing ever ages), so it's a bare read-lock acquire per `def`/`spawn`.
    promote_lock: RwLock<()>,
}

/// Where a global was defined: the file, and the start position of its
/// `def`/`defn`/`defmacro` form. Captured pre-macroexpansion so `defn`/`defmacro`
/// definitions are located accurately (ADR-031).
#[derive(Clone)]
pub struct SourceLoc {
    pub file: String,
    pub pos: crate::error::Pos,
}

/// A rolled-back-on-restore snapshot of the runtime globals, plus the RUNTIME-compaction
/// suppression it holds (KI-6). Constructed **only** by [`Heap::snapshot_globals`] — and
/// the sole argument type [`Heap::restore_globals`] accepts — so the snapshot↔restore
/// protocol can't be misused: a restore can't run without a paired snapshot (no way to
/// forge one), and `restore_globals` takes it *by value* so the same snapshot can't be
/// restored twice. `#[must_use]`: dropping a snapshot without restoring it leaves the
/// globals mutated AND compaction suppressed, so the compiler flags an ignored one.
#[must_use = "a globals snapshot must be handed to heap.restore_globals — dropping it \
              leaves the globals table mutated and RUNTIME compaction suppressed (KI-6)"]
pub struct GlobalsSnapshot {
    saved: SymbolMap<Value>,
    /// The `rt_collect_block` depth this snapshot established (post-increment). Restore
    /// asserts the live depth still matches — catching an out-of-order (non-LIFO) restore,
    /// which would release the wrong scope's suppression.
    block_depth: u32,
}

/// An RAII pin on one RUNTIME generation, held by an in-flight `Message::FnShared` so the
/// generation cannot be freed while a shared handle into it is queued but not yet landed in
/// any heap (see [`RuntimeCode::gen_inflight`]).
///
/// Deliberately RAII rather than a manual increment/decrement pair. A message is dropped on
/// several paths that are easy to miss — an unknown or dead target, a mailbox torn down, a
/// routing failure — and a *leaked* pin is the worst possible failure here: the generation is
/// never reclaimed, so the region grows without bound, silently, which is the very class of
/// bug this whole change exists to fix. Making the release structural means the compiler
/// enforces it instead of a reviewer.
pub struct GenPin {
    runtime: Arc<RuntimeCode>,
    gen: usize,
}

impl GenPin {
    fn new(runtime: Arc<RuntimeCode>, gen: usize) -> Self {
        runtime.gen_inflight[gen].fetch_add(1, Ordering::AcqRel);
        GenPin { runtime, gen }
    }
}

impl Clone for GenPin {
    fn clone(&self) -> Self {
        GenPin::new(Arc::clone(&self.runtime), self.gen)
    }
}

impl Drop for GenPin {
    fn drop(&mut self) {
        self.runtime.gen_inflight[self.gen].fetch_sub(1, Ordering::AcqRel);
    }
}

impl std::fmt::Debug for GenPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GenPin(gen={})", self.gen)
    }
}

impl Heap {
    /// Pin the generation `id` lives in, for as long as the returned guard is held. Returns
    /// `None` for a non-RUNTIME handle (PRELUDE is never freed; LOCAL is not shareable).
    pub fn pin_gen_of(&self, id: crate::core::value::ClosureId) -> Option<GenPin> {
        if id.region() != RUNTIME {
            return None;
        }
        Some(GenPin::new(Arc::clone(&self.runtime), id.code_gen()))
    }

    /// Are there shared-closure messages in flight against generation `gen`?
    pub fn gen_has_inflight(&self, gen: usize) -> bool {
        self.runtime.gen_inflight[gen].load(Ordering::Acquire) != 0
    }

    /// Forget this process's cached "clean" drain ack, forcing it to re-walk on its next
    /// safepoint.
    ///
    /// `report_gen_liveness` caches the ack for a whole epoch, justified by "an old-gen
    /// handle can never arrive by message (messages deep-copy)". Materialising a
    /// `Message::FnShared` breaks exactly that: this heap may now hold a handle into the
    /// draining generation, and a stale clean ack would let the collector free it. Called
    /// only on that path, so the fan-out drain cost the caching was introduced to fix is
    /// unchanged for every other message.
    pub fn rearm_drain_ack(&self) {
        // `0` is the "never acked" sentinel the constructors use; a real epoch is >= 1.
        self.acked_drain_epoch.set(0);
    }
}

/// The next [`RuntimeCode::runtime_tag`] — a process-wide monotonic counter.
fn next_runtime_tag() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Default for RuntimeCode {
    fn default() -> Self {
        RuntimeCode {
            gens: [
                ArcSwap::from_pointee(CodeSlabs::default()),
                ArcSwap::from_pointee(CodeSlabs::default()),
            ],
            current_gen: AtomicUsize::new(0),
            runtime_tag: next_runtime_tag(),
            gen_inflight: [AtomicUsize::new(0), AtomicUsize::new(0)],
            gen_version: AtomicU64::new(0),
            globals: RwLock::new(SymbolMap::default()),
            registry_lock: Mutex::new(()),
            // A default (un-seeded) runtime reserves nothing — the prelude hasn't run.
            sealed: RwLock::new(std::collections::HashSet::new()),
            // Likewise no private names until the prelude has been seeded.
            private: RwLock::new(std::collections::HashSet::new()),
            version: AtomicU64::new(0),
            def_sites: RwLock::new(HashMap::new()),
            positions: RwLock::new(HashMap::new()),
            jit_code_cache: RwLock::new(HashMap::new()),
            shared_closures: RwLock::new(HashMap::new()),
            jit_inline_cache: RwLock::new(HashMap::new()),
            declared_sigs: RwLock::new(SymbolMap::default()),
            drain_active: AtomicBool::new(false),
            rt_dirty: AtomicBool::new(true),
            drain_gen: AtomicUsize::new(0),
            drain_epoch: AtomicU64::new(0),
            drain_acked: AtomicU64::new(0),
            drain_acks: RwLock::new(HashMap::new()),
            free_epoch: AtomicU64::new(0),
            aging: AtomicBool::new(false),
            aged_count: AtomicU64::new(0),
            promote_lock: RwLock::new(()),
        }
    }
}

impl RuntimeCode {
    /// The current code generation's index (0 or 1). Where new code lands.
    #[inline]
    fn cur_gen(&self) -> usize {
        self.current_gen.load(Ordering::Relaxed)
    }
    /// A guard on the current code generation's slabs — the target of `promote`/`def`
    /// and the region the single-process compactor operates on. Derefs to
    /// `&CodeSlabs`; hold it (don't re-call) across a multi-step read so the slab
    /// can't be freed mid-use.
    #[inline]
    fn cur_code(&self) -> Guard<Arc<CodeSlabs>> {
        self.gens[self.cur_gen()].load()
    }
    // Append a value into the *current* code generation and mint a handle tagged
    // with that generation, so a read later resolves the right slab (2-generation
    // collector, ADR-091). Centralised so every RUNTIME mint is gen-tagged the same
    // way — the push slab and the handle's `code_gen` can never disagree.
    #[inline]
    fn push_str(&self, v: String) -> StrId {
        let g = self.cur_gen();
        StrId::runtime_gen(self.gens[g].load().strings.push(LocalString::inline(v)), g)
    }
    #[inline]
    fn push_bigint(&self, v: num_bigint::BigInt) -> BigIntId {
        let g = self.cur_gen();
        BigIntId::runtime_gen(self.gens[g].load().bigints.push(v), g)
    }
    #[inline]
    fn push_decimal(&self, v: bigdecimal::BigDecimal) -> DecimalId {
        let g = self.cur_gen();
        DecimalId::runtime_gen(self.gens[g].load().decimals.push(v), g)
    }
    #[inline]
    fn push_ratio(&self, v: num_rational::BigRational) -> RatioId {
        let g = self.cur_gen();
        RatioId::runtime_gen(self.gens[g].load().ratios.push(v), g)
    }
    #[inline]
    fn push_bytes(&self, v: Arc<SharedBlob>) -> BytesId {
        let g = self.cur_gen();
        BytesId::runtime_gen(self.gens[g].load().bytes.push(v), g)
    }
    #[inline]
    fn push_rope(&self, v: ropey::Rope) -> RopeId {
        let g = self.cur_gen();
        RopeId::runtime_gen(self.gens[g].load().ropes.push(v), g)
    }
    #[inline]
    fn push_vec(&self, v: VecStore) -> VecId {
        let g = self.cur_gen();
        VecId::runtime_gen(self.gens[g].load().vectors.push(v), g)
    }
    /// A fresh runtime whose global table is seeded with the prelude bindings
    /// (`symbol -> prelude value`). The code slabs start empty — user `def`s
    /// append to them. Inner processes share this whole thing via `Arc`.
    pub fn seeded(bindings: &[(Symbol, Value)], prelude_private: &[Symbol]) -> Self {
        let mut globals = SymbolMap::with_capacity_and_hasher(bindings.len(), Default::default());
        for &(s, v) in bindings {
            globals.insert(s, v);
        }
        RuntimeCode {
            gens: [
                ArcSwap::from_pointee(CodeSlabs::default()),
                ArcSwap::from_pointee(CodeSlabs::default()),
            ],
            current_gen: AtomicUsize::new(0),
            runtime_tag: next_runtime_tag(),
            gen_inflight: [AtomicUsize::new(0), AtomicUsize::new(0)],
            gen_version: AtomicU64::new(0),
            // Reserved at seed time: every shipped **function**, macro and builtin.
            // Deliberately NOT the prelude's data globals — `*features*`,
            // `*load-path*`, `*module-docs*`, `*reload-diagnostics*` are registries
            // that prelude functions rebind with `def` at runtime (Brood's one
            // mutation), so `require`/`defmodule`/`provide` would break if they were
            // reserved. The rule is exactly "a shipped FUNCTION can't be redefined";
            // shipped mutable state stays rebindable, which is how it works at all.
            sealed: RwLock::new(
                bindings
                    .iter()
                    .filter(|(_, v)| {
                        matches!(
                            v.unpack(),
                            ValueRef::Fn(_) | ValueRef::Macro(_) | ValueRef::Native(_)
                        )
                    })
                    .map(|&(s, _)| s)
                    .collect(),
            ),
            // The prelude's own module-private names (ADR-146). `seeded` *inserts*
            // the bindings (it does not re-`eval` them, so `%mark-private` never
            // fires for a prelude name in the live runtime), and privacy is no
            // longer derivable from the clean name — so the set is collected when
            // the prelude is built (every `defn-`/`def-` head recorded in the
            // builder heap's runtime) and threaded in here, the same way the
            // bindings themselves are.
            private: RwLock::new(prelude_private.iter().copied().collect()),
            globals: RwLock::new(globals),
            registry_lock: Mutex::new(()),
            version: AtomicU64::new(0),
            def_sites: RwLock::new(HashMap::new()),
            positions: RwLock::new(HashMap::new()),
            jit_code_cache: RwLock::new(HashMap::new()),
            shared_closures: RwLock::new(HashMap::new()),
            jit_inline_cache: RwLock::new(HashMap::new()),
            declared_sigs: RwLock::new(SymbolMap::default()),
            drain_active: AtomicBool::new(false),
            rt_dirty: AtomicBool::new(true),
            drain_gen: AtomicUsize::new(0),
            drain_epoch: AtomicU64::new(0),
            drain_acked: AtomicU64::new(0),
            drain_acks: RwLock::new(HashMap::new()),
            free_epoch: AtomicU64::new(0),
            aging: AtomicBool::new(false),
            aged_count: AtomicU64::new(0),
            promote_lock: RwLock::new(()),
        }
    }

    /// Read/write the global table, recovering from a poisoned lock instead of
    /// propagating the panic. The values are `Copy` handles and writers only
    /// `insert`/replace, so a writer that panicked left the map structurally
    /// sound — recovering keeps one bad process from wedging every other one
    /// that later looks up or defines a global.
    fn globals_read(&self) -> RwLockReadGuard<'_, SymbolMap<Value>> {
        self.globals.read().unwrap_or_else(|e| e.into_inner())
    }
    fn globals_write(&self) -> RwLockWriteGuard<'_, SymbolMap<Value>> {
        self.globals.write().unwrap_or_else(|e| e.into_inner())
    }
    /// Is `sym` a reserved (language-shipped) name? See [`RuntimeCode::sealed`].
    ///
    /// A **dynamic variable is never reserved**, whatever it holds. `defdyn` (or
    /// `%declare-dynamic`) *declares a name rebindable* — that is the entire meaning
    /// of the declaration — so reserving one would contradict it. This matters
    /// concretely for `*out*`/`*err*`: an output port IS a function
    /// (`(fn (s) …)`), so the function-valued test would otherwise reserve them and
    /// make a permanent output redirect impossible, leaving only the scoped
    /// `binding` form. The check lives here rather than in the seed filter so it also
    /// covers a `defdyn` inside an embedded module, and so a name declared dynamic
    /// *after* the seed is exempt too.
    fn is_sealed(&self, sym: Symbol) -> bool {
        !crate::core::value::is_dynamic(sym)
            && self
                .sealed
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&sym)
    }
    /// Reserve `sym` — called for each name an embedded std module defines as it
    /// loads, so the module's own surface becomes reserved once it exists.
    fn seal(&self, sym: Symbol) {
        self.sealed
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym);
    }

    /// Record `sym` (a qualified global name) as module-private — called from
    /// `env_define` when the name carries the `--` marker. Idempotent insert. See
    /// [`RuntimeCode::private`].
    fn mark_private(&self, sym: Symbol) {
        self.private
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym);
    }
    /// Is `sym` recorded module-private? The authoritative (and, since ADR-146 step 2,
    /// the *only*) half of [`Heap::is_private`].
    fn is_private_recorded(&self, sym: Symbol) -> bool {
        self.private
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&sym)
    }

    /// As `globals_read`/`globals_write`, for the def-site table (same
    /// poison-recovery rationale — entries are owned data, never structurally
    /// corrupting on a panicked writer).
    fn def_sites_read(&self) -> RwLockReadGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.read().unwrap_or_else(|e| e.into_inner())
    }
    /// RUNTIME-form source position + file by slab index, or `None`. See [`Self::positions`].
    fn position_of(&self, idx: usize) -> Option<(crate::error::Pos, Option<Arc<str>>)> {
        self.positions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&idx)
            .cloned()
    }
    /// Record a RUNTIME-form source position + file (called by `promote`). See [`Self::positions`].
    fn set_position(&self, idx: usize, pos: crate::error::Pos, file: Option<Arc<str>>) {
        self.positions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(idx, (pos, file));
    }

    fn def_sites_write(&self) -> RwLockWriteGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// The set of global observations one `check-file-deps` made (ADR-119 Phase 2),
/// accumulated in [`Heap::check_dep_rec`] while the check runs. The checker's
/// `obs_*` wrappers push into it; `types::check::deps` turns it into the file's
/// serializable dep-keys and fingerprint. Plain data — no layering dependency on
/// the checker.
#[derive(Default)]
pub(crate) struct CheckDepRec {
    /// Global symbols whose binding/arity/sig the check observed.
    pub(crate) syms: std::collections::HashSet<Symbol>,
    /// `mod/` prefixes whose known-ness the check queried.
    pub(crate) known_ns: std::collections::HashSet<String>,
    /// `mod/` prefixes whose export set the check read (`:use` resolution).
    pub(crate) exports: std::collections::HashSet<String>,
    /// Globals DEFINED in this file (its own def-names). Excluded from the dep-keys:
    /// a file's dependency on its own globals is already covered by its own mtime, so
    /// storing them would just bloat the manifest (a 1000-def file otherwise records
    /// ~1000 self-names — the 64MB-manifest bug).
    pub(crate) own: std::collections::HashSet<Symbol>,
    /// Whether the check consulted the `*protocols*` table.
    pub(crate) protocols: bool,
}

/// **Loader / checker / namespace state — cold for a worker process** (ADR-175 follow-up,
/// 2026-07-29). Every field here is used by the process that *loads modules or runs the
/// checker*; a spawned green process that only runs Brood code never touches any of them.
/// Held behind `Option<Box<ColdHeap>>` on the `Heap` so a worker pays 8 bytes instead of
/// ~320, which matters because `Box<Process>` sits near a mimalloc size-class boundary:
/// measured, +320 B of `Process` costs **+640 B of RSS per process**, so the same move
/// downward is worth a class.
///
/// Allocated lazily by [`Heap::cold_mut`] on first write; readers go through
/// [`Heap::cold`] and treat `None` as empty, which is exactly right — an absent
/// `ColdHeap` means "this process has loaded nothing and checked nothing".
#[derive(Default)]

pub(crate) struct ColdHeap {
    /// Nesting depth of an embedded-module load (ADR-166). See `Heap::in_module_load`.
    pub(crate) module_load_depth: u32,
    /// Source position of LOCAL list forms, keyed by [`form_pos_key`].
    pub(crate) form_pos: HashMap<u64, (crate::error::Pos, Option<Arc<str>>)>,
    /// The file currently being `load`ed, exposed via `(current-file)`.
    pub(crate) current_file: Option<String>,
    /// The namespace being compiled (`defmodule`).
    pub(crate) compile_ns: Option<Symbol>,
    /// Names the current namespace defines.
    pub(crate) ns_known_names: HashSet<Symbol>,
    /// Compiling a form that has **no** whole-file pre-scan behind it (a runtime
    /// `eval`), so `ns_known_names` cannot answer "will this namespace define it?".
    pub(crate) ns_assume_own: bool,
    /// `(:use …)` import map for the namespace being compiled.
    pub(crate) imports: HashMap<Symbol, Symbol>,
    /// Package-rooted namespaces (ADR-070): while loading a *dependency* `foo`, its
    /// local name is the active prefix, so a module the file declares `(defmodule b)`
    /// roots to `foo/b` and its intra-package `(:use b)`/`(:alias b …)` targets root
    /// too. `None` outside a dep load (the root project / std stay unrooted, short).
    pub(crate) package_prefix: Option<Symbol>,
    /// The short module names the active package provides — the set that decides
    /// whether a referenced module name is *intra-package* (root it) or external
    /// (a std/other-dep name, leave it bare). Empty when `package_prefix` is `None`.
    pub(crate) package_modules: HashSet<Symbol>,
}

/// Checker-only heap state, lazily boxed off [`Heap`] (see [`Heap::check`]). None of it
/// is touched by running Brood code — only by `nest check` / `check-file-deps`.
#[derive(Default)]
pub(crate) struct CheckHeap {
    /// `mod/` prefix → its public `(bare, qualified)` export pairs, keyed by the global
    /// count so a `def` invalidates it. Lets a whole-project check build the index in ONE
    /// pass instead of rescanning every global per file (O(files²)).
    exports: Option<(
        usize,
        std::sync::Arc<std::collections::HashMap<String, Vec<(Symbol, Symbol)>>>,
    )>,
    /// The set of `mod/` namespace prefixes in the loaded image, count-keyed and shared.
    known_ns: Option<(usize, std::sync::Arc<std::collections::HashSet<String>>)>,
    /// The in-flight incremental-check dependency record (ADR-119).
    dep_rec: Option<CheckDepRec>,
}

impl Heap {
    /// The old-generation slabs. Callers must already hold an OLD handle, whose existence
    /// implies a promotion allocated this — see [`Heap::old`](Self::old)'s field docs.
    #[inline]
    fn old(&self) -> &Slabs {
        self.old
            .as_deref()
            .expect("an OLD handle implies the old generation was allocated")
    }
    /// The old-generation slabs for mutation, allocating on first promotion.
    #[inline]
    fn old_mut(&mut self) -> &mut Slabs {
        self.old.get_or_insert_with(Box::default)
    }
    /// The old generation if this process ever promoted — for aggregate walks (capacity
    /// sums, GC scans) that must tolerate its absence rather than assume it.
    #[inline]
    fn old_opt(&self) -> Option<&Slabs> {
        self.old.as_deref()
    }
}

impl Heap {
    /// The checker state, allocating it on first use. Callers hold `&self` (see
    /// [`Heap::check`]), so this returns a guard rather than a reference.
    fn check_mut(&self) -> std::cell::RefMut<'_, Box<CheckHeap>> {
        let mut b = self.check.borrow_mut();
        if b.is_none() {
            *b = Some(Box::default());
        }
        std::cell::RefMut::map(b, |o| o.as_mut().expect("just filled"))
    }
}

pub struct Heap {
    /// The **nursery** (young generation): every `alloc_*` bumps into here, so it
    /// holds the freshly-allocated, mostly-short-lived objects. A *minor*
    /// collection ([`minor_collect`](Self::minor_collect)) copies its survivors
    /// into [`old`](Self::old) and drops the rest whole. Kept named `local` because
    /// it's the allocation hot path and the common case for an accessor.
    local: Slabs,
    /// The **old (tenured) generation**: objects that survived a minor collection,
    /// addressed by LOCAL handles with the [`AGE_OLD`](crate::core::value::AGE_OLD)
    /// bit set. Grows by append on each minor collection (cheap — old objects are
    /// never recopied); reclaimed only by a *major* collection
    /// ([`major_collect`](Self::major_collect)), which compacts it. Because Brood
    /// data is immutable, an old object can never come to point at a young one, so
    /// the old generation is **not a root set for a minor collection** — no write
    /// barrier, no remembered set.
    /// The old generation, lazily boxed. Empty for almost every process — measured at
    /// **7 of 300,000** on `spawn-live`, because a process only populates it by surviving a
    /// minor collection, and a short-lived worker never collects at all. Inline it is 264 B
    /// (eleven `Vec` headers) on every `Heap`, which is inline in `Box<Process>`; boxed it
    /// is 8. Reads go through [`Heap::old`], which may only be called when an OLD handle
    /// exists — and an OLD handle can only exist if a promotion allocated this.
    old: Option<Box<Slabs>>,
    prelude: Arc<SharedCode>,
    runtime: Arc<RuntimeCode>,
    /// Nesting depth of an **embedded-module load** in this process (ADR-166). While
    /// non-zero, a global `def` of a reserved name is permitted *and* reserves the
    /// name — that is how a std module's own surface (`set/union`, `path/join`)
    /// becomes reserved once it exists, and how re-loading one stays idempotent
    /// (`require--await` deliberately re-evaluates a module whose loader died).
    /// Per-process, so two processes loading different modules never see each
    /// other's exemption; incremented and decremented by `%load-module-source`,
    /// which restores it even when the load throws.

    /// **Per-process pinned-read cache for the RUNTIME generations.** A RUNTIME handle
    /// deref must pin `gens[g]`'s `Arc<CodeSlabs>` (so a concurrent Stage-4 free can't drop
    /// it mid-read), but taking a fresh `ArcSwap::load` guard per deref dominated
    /// global-data-heavy hot loops. Instead each slot caches the last-loaded `Arc` plus the
    /// [`RuntimeCode::gen_version`] it was loaded at; [`code_gen_pinned`](Self::code_gen_pinned)
    /// clones the cached `Arc` (a single refcount bump) when the version is unchanged and
    /// only `load_full`s on a real generation replacement (Stage-4 free / compaction store —
    /// rare). `RefCell`/`Cell`: the `Heap` is single-threaded (one worker owns a process at a
    /// time). `gen_cache_ver` starts at `u64::MAX` so the first read always populates.
    gen_cache: [RefCell<Option<Arc<CodeSlabs>>>; 2],
    gen_cache_ver: [Cell<u64>; 2],
    /// **Per-process parse cache for `(fn …)` literals**, keyed by the `MakeClosure`
    /// site's `fn_rest` AST handle. Building a closure re-parses its param
    /// lists/optionals/doc and walks the (RUNTIME) body cons list on every creation —
    /// pure waste in a closure-in-a-loop (a `receive` matcher, a per-frame callback),
    /// since the parse is a function of the fixed AST. This memoises the parsed
    /// [`ClosureTemplate`], so creation drops to cloning the arm `Vec` out of it + env
    /// attach — the re-parse, RUNTIME-AST walk, and pass-through analysis are gone. (The
    /// per-instance arm-`Vec` clone/drop is the remaining cost; sharing the arms via an
    /// `Arc<[ClosureArm]>` in `Closure` would remove it too, but touches the GC's in-place
    /// arm rewrite — deferred.) Invalidated exactly like [`gen_cache`](Self::gen_cache): the arms hold RUNTIME
    /// AST handles, which move only on a `gen_version` bump (Stage-4 free / compaction),
    /// so a version change clears the whole map (`closure_tpl_ver` starts at `u64::MAX`
    /// so the first use populates). `RefCell`: the `Heap` is single-threaded (one worker
    /// owns a process at a time); `Arc` (not `Rc`) so the `Heap` stays `Send` across the
    /// worker migration a process undergoes. Uses [`SymbolHasher`] — a `PairId` hashes as a
    /// single `u64`, so the lookup (once per closure creation) takes its bijective
    /// `write_u64` fast path instead of stock `SipHash`.
    closure_tpl_cache: RefCell<ClosureTemplateMap>,
    closure_tpl_ver: Cell<u64>,
    /// **Capture-free closure constant cache.** A `(fn …)` literal with no lexical captures
    /// and no self-name is a *constant* — its `env` is [`EnvId::GLOBAL`], so it late-binds
    /// globals but captures nothing, and every evaluation would otherwise rebuild an
    /// identical closure and (for a `spawn` thunk) re-`promote` it into the RUNTIME region,
    /// piling up garbage the collector must reclaim. This memoises the closure built **once**
    /// and promoted to a stable RUNTIME handle, so re-evaluating the literal returns the same
    /// handle — no alloc, no re-promote (`(spawn (worker))` in a fan-out drops ~7×). Keyed and
    /// invalidated exactly like [`closure_tpl_cache`](Self::closure_tpl_cache): the handle is a
    /// RUNTIME value that moves only on a `gen_version` bump, so a version change clears the
    /// map (`closure_const_ver` starts at `u64::MAX` so the first use populates).
    closure_const_cache: RefCell<ConstClosureMap>,
    closure_const_ver: Cell<u64>,
    /// This process's global scope. For a real runtime this is [`EnvId::GLOBAL`]
    /// (routing to `runtime.globals`); for the prelude *builder* it's a real
    /// local root frame (so the prelude can be evaluated, then frozen).
    global: EnvId,
    /// Source position of LOCAL list forms, keyed by pair slab index, recorded
    /// by the reader. Queried via `(form-pos …)` (e.g. by the test macros, which
    /// look up a form's line *before* it expands). LOCAL-only and dropped on
    /// reset, since it is read-time metadata for the source being loaded.
    /// Keyed by [`form_pos_key`] — the pair's slab index packed with its
    /// generation age bit, so a nursery pair and an old pair at the same slab
    /// index don't collide (the two LOCAL spaces share an index range).

    /// The file currently being `load`ed, exposed via `(current-file)`. Saved and
    /// restored around each load so nested loads don't clobber the outer file.

    /// The namespace currently being compiled into (ADR-065). `None` = root (the
    /// prelude, plain code, and the REPL until an `(ns …)` form runs). Set by the
    /// `(ns foo)` form via the `%in-ns` primitive; read by the resolver pass
    /// (`eval::macros::resolve`) to qualify definition heads and free references to
    /// `foo/name`. Per-process compile state — NOT a shared global, which would race
    /// across green processes (`RuntimeCode` is shared). File/module loaders save +
    /// reset this to root per file (so a `require`d file starts at root); the REPL
    /// driver leaves it sticky across entries.

    /// Names the current-namespace file will define (its top-level `def`/`defmacro`
    /// heads), pre-scanned when an `(ns …)` form runs so the resolver can qualify a
    /// *forward* reference (`bar` used before `foo/bar` is defined) — without it,
    /// such a reference would silently stay bare (order-dependent miscompile). Bare
    /// symbols only; consulted alongside the live global table. Cleared/repopulated
    /// per file by the loader.

    /// Names the current file `(:use …)`-imported: bare name → qualified global
    /// (`describe` → `test/describe`). Populated by `%refer` when the `(ns …)`
    /// header runs; consulted by the resolver after the current namespace and
    /// before root fall-through. Per-file like `ns_known_names` — reset/restored
    /// by the loaders so imports never leak across files (ADR-065 inc-2).

    /// This process's dynamic-variable binding stack (the `binding` form). Each
    /// `binding` pushes its `(symbol, value)` pairs and pops them when its body
    /// returns (even on error); a read of a dynamic var consults this — latest
    /// binding wins — before the shared global table (see [`Heap::env_get`]).
    /// Per-process and not shared: a `spawn`ed child starts with an empty stack,
    /// so dynamic bindings never cross to another process (data isn't shared).
    /// Empty whenever no `binding` is active — so it's free on the common path
    /// and holds no LOCAL handles across a top-level arena reset.
    dynamics: Vec<(Symbol, Value)>,
    /// The debugger's durable per-process causal context (ADR-174 send-level slice):
    /// a settable slot that, unlike a `binding` on [`dynamics`], survives across
    /// `receive` and migration (so a long-lived server adopts the sender's context
    /// per message). GC-traced exactly where `dynamics` is. `#[cfg(dev-tools)]` — a
    /// lean release has no such field, so the whole send-level path compiles out.
    #[cfg(feature = "dev-tools")]
    trace_context: Option<Value>,
    /// Whether [`trace_context`] is the process's OWN context (set by `with-debugger`
    /// / `span`) — which `spawn` propagates to children — versus one merely ADOPTED
    /// from a received message, which is used to handle that message but must NOT
    /// propagate onward (else an adopted context leaks transitively through unrelated
    /// spawns). Meaningful only when `trace_context` is `Some`.
    #[cfg(feature = "dev-tools")]
    trace_context_own: bool,
    /// Per-process **global inline cache** (perf): `symbol -> (runtime version,
    /// resolved value)`. Consulted by [`env_get`](Self::env_get) only after the
    /// local env chain misses *and* no dynamic binding shadows the name — i.e.
    /// exactly where a lookup would otherwise take the shared `RwLock` on
    /// `runtime.globals`. On a version match it returns the cached handle with no
    /// lock; a stale entry (a `def` bumped `runtime.version`) falls through to the
    /// locked table and re-stamps. Cached values are always immovable
    /// PRELUDE/RUNTIME handles (globals are `promote`d before binding), so an entry
    /// survives a local GC untouched and needs no rooting. `RefCell` because
    /// `env_get` is `&self`; per-process, so never shared across threads.
    global_ic: RefCell<SymbolMap<(u64, Value)>>,
    /// Memoized `mod/name` → `prefix/mod/name` rooting for intra-package *qualified
    /// references* (ADR-070). A miss in the global table falls back to the rooted name
    /// (see [`root_qualified_ref`](Self::root_qualified_ref)); this caches the symbol→symbol answer —
    /// including the negative one (`None` = not intra-package, don't retry) — so the
    /// fallback costs one hash lookup rather than a `format!` + intern per reference.
    /// Keyed only by symbol, which is safe because [`set_package_context`] clears it:
    /// the mapping is a property of the *active* package context, and that's the one
    /// place the context changes. `RefCell` because `env_get` is `&self`; per-process.
    rooted_ref_ic: RefCell<SymbolMap<Option<Symbol>>>,
    /// Cached `mod/` prefix → that module's public exports (`(bare, qualified)` pairs).
    /// Used ONLY by the advisory whole-project checker's direct import setup
    /// (`types::check::setup_check_imports`): a whole-project check resolves every file's
    /// `(:use …)` clauses, and enumerating a module's exports by scanning all globals per
    /// file was O(files²). Keyed by the global symbol **count**, which is safe here because
    /// the checker's per-file loop performs NO `def`s (it sets up imports without evaling
    /// the header) and runs no `%isolate` rollback mid-loop, so the global set — hence the
    /// count — is stable across the loop. (NOT used by runtime `%refer`, where `%isolate`
    /// rollback could otherwise collide counts.) Per-process; built once per check.
    /// Cached set of `mod/` namespace prefixes the loaded image knows — the checker's
    /// `known_ns` (decides whether an unresolved *qualified* name is a real unbound ref or
    /// one in an unloaded module). Same rationale + count-keying + checker-only soundness as
    /// [`module_exports_cache`](Self::module_exports_cache): rebuilding it by scanning all
    /// globals per file was the residual O(files²) after the header-eval redesign.

    /// Phase-2 incremental-check dependency recorder (ADR-119). `Some` only while a
    /// `check-file-deps` runs *on this process*; the advisory checker's `obs_*`
    /// wrappers record every global observation into it. Living on the **heap** (not
    /// a thread-local) makes it per-process — a green process owns its heap and it
    /// migrates *with* the process — so dep-capture can run in parallel across the
    /// worker pool without two concurrent checks (or a mid-check migration/preempt)
    /// clobbering each other's record. Off (`None`) for all normal eval; the record
    /// borrow on the hot per-symbol observation path is a single `RefCell` check.

    /// Explicit GC root stack — the evaluator's **operand stack** (ADR-061).
    /// Every LOCAL [`Value`] an eval frame still needs *after* a nested `eval`
    /// (its accumulated `argv`, literal accumulators, `callee`, the `call_form`,
    /// the cons-spine cursor) is pushed here for the duration of that call, then
    /// re-read via [`root_at`](Self::root_at) afterwards (the copying collector
    /// relocates these in place). This is what lets the safepoint collect at
    /// **any** eval depth, not just the outermost — see `docs/memory-model.md`.
    /// Also used by `eval_str`/`eval_source` for the unevaluated forms vector.
    /// Empty between top-level forms.
    /// **Delivered-message slots (ADR-177 / L1).** When a `send` finds this process
    /// *parked*, it copies the value straight into this heap — skipping the wire-format
    /// `Message` round trip entirely — and parks the result here; the envelope in the
    /// mailbox carries the slot index. A traced root set, flushed in place by `collect`
    /// exactly like [`Self::roots`], because a queued message can sit through any number
    /// of the receiver's collections before a selective `receive` gets to it.
    ///
    /// It is a **slot table, not a stack**: `roots` is the operand stack and is truncated
    /// from ~109 sites (every frame pop), so a long-lived value cannot live there. A
    /// consumed slot is tombstoned to `nil` and reused, so the table stays as small as the
    /// process's peak *undelivered* Local message count — normally 0 or 1.
    ///
    /// Boxed and lazily allocated: inline it is 24 bytes on every `Heap`, and a `Heap`
    /// is inline in `Box<Process>`, where bytes cost about 2:1 in RSS via mimalloc's size
    /// classes — measured at `spawn` +5.9% for the inline `Vec`. `None` until this process
    /// is actually handed a fast-path message, which most processes never are.
    // `Box<Vec>` is deliberate, not the redundant box clippy assumes: boxing keeps this
    // 8 bytes inline on every `Heap` instead of the `Vec`'s 24 (the +5.9%-spawn cost above).
    #[allow(clippy::box_collection)]
    msg_roots: Option<Box<Vec<Value>>>,
    /// Loader/checker/namespace state — see [`ColdHeap`]. `None` until a module load,
    /// namespace compile or checker run needs it, so a plain worker process never
    /// allocates it (worth a mimalloc size class on `Box<Process>`).
    cold: Option<Box<ColdHeap>>,
    /// Checker caches. These stay on `Heap` rather than moving into [`ColdHeap`]
    /// because they are filled through `&self` (the checker's read paths), which
    /// cannot lazily allocate the boxed cold state. ~96 bytes; the six fields that
    /// *are* in `ColdHeap` were enough to drop `Box<Process>` a size class.
    /// Checker-only state — see [`CheckHeap`]. Lazily boxed and `None` until a check
    /// actually runs, because it is **288 bytes** inline (`check_dep_rec` alone is 208,
    /// four `HashSet`s) on every `Heap`, and a `Heap` is inline in `Box<Process>`. A
    /// spawned worker process never checks anything, so it never pays for this. Behind a
    /// `RefCell` rather than a plain `Option<Box<_>>` (the shape [`ColdHeap`] uses)
    /// because every one of these is filled through `&self` — which is exactly why M1
    /// left them inline.
    check: RefCell<Option<Box<CheckHeap>>>,
    roots: Vec<Value>,
    /// The env half of the operand stack (ADR-061): LOCAL [`EnvId`]s an eval
    /// frame still needs across a nested `eval` (its `scope`/`env`). Relocated in
    /// place by [`arena_flip`](Self::arena_flip) alongside `roots`; re-read via
    /// [`env_root_at`](Self::env_root_at). Separate stack because an `EnvId`
    /// isn't a `Value`. Empty between top-level forms.
    env_roots: Vec<EnvId>,
    /// Adaptive GC trigger: collect when the LOCAL live-object count crosses
    /// this. Recomputed after each [`collect`](Self::collect) as
    /// `max(GC_FLOOR, 2 * live)`. `usize::MAX` while [`gc_enabled`] is false
    /// (prelude build) so the safepoint check is a single compare with no GC.
    ///
    /// [`gc_enabled`]: Self::gc_enabled
    gc_threshold: usize,
    /// [`park_trim_probe`] as of this process's last park-time trim — the baseline
    /// [`Heap::trim_parked`] measures growth against, in slab elements.
    park_trim_mark: usize,
    /// Adaptive **RUNTIME**-collection trigger: the eval safepoint reclaims the shared
    /// code region once the RUNTIME closure count crosses this — in-place compaction when
    /// this heap uniquely owns the runtime, else a step of the 2-generation collector.
    /// Recomputed after each reclaim as `max(RT_GC_FLOOR, 2 * live)`, so the collector
    /// re-enters only as the region grows rather than bailing every safepoint.
    /// `usize::MAX` while [`gc_enabled`] is false. See [`rt_gc_floor`] and
    /// [`maybe_runtime_collect`](Self::maybe_runtime_collect).
    rt_gc_threshold: usize,
    /// GC switch. `false` during the prelude *build* (`Heap::new`), `true` for
    /// real process heaps (`Heap::with_regions`); also forced `false` when the
    /// prelude `SharedCode` `Arc` is the default (empty) one, since a missing
    /// prelude means a freshly-built builder heap that's about to freeze.
    gc_enabled: bool,
    /// Re-entrant suppression of RUNTIME-region compaction while a **globals snapshot
    /// is outstanding**. [`snapshot_globals`] clones the global table — a
    /// `SymbolMap<Value>` of raw RUNTIME handles — off the graph and hands it back for a
    /// later [`restore_globals`] (the `%isolate` protocol). A compaction between the two
    /// would relocate those handles, leaving the snapshot pointing at recycled slots — so
    /// the restore reinstalls stale handles and unrelated globals silently misdispatch
    /// (KI-6). `snapshot_globals` increments this and `restore_globals` decrements it, so
    /// the invariant "no compaction while a snapshot is live" holds *structurally* — any
    /// caller of the snapshot/restore protocol is covered, not just `%isolate`.
    /// [`runtime_collect_with`] bails while it's >0 (the choke point for both the auto
    /// safepoint path — via [`rt_gc_due`] — and a manual `(runtime-collect)`); the
    /// isolate's `def`s become garbage at restore and are reclaimed by the next safepoint.
    /// `Cell` so the `&self` snapshot/restore can bump it; a counter (not a bool) so nested
    /// snapshots compose. [`rt_gc_due`]: Self::rt_gc_due
    rt_collect_block: std::cell::Cell<u32>,
    /// The LOCAL **generation epoch** — stamped into every LOCAL handle minted
    /// (the `local_gen` in `alloc_*`), and bumped on every arena flip
    /// ([`arena_flip`](Self::arena_flip), shared by `flush`/`collect`) so the
    /// survivors are re-minted with the new value and any handle held across the
    /// flip without being re-rooted keeps the old one. A debug-only deref check
    /// in the LOCAL accessors compares `handle.generation()` against this and
    /// panics at the bad deref. Per-heap (not per-slot): the bump allocator never
    /// reuses a slot, so a whole-arena flip is the only LOCAL-invalidating event.
    /// See `docs/memory-review.md`.
    local_epoch: u32,
    /// **Write-barrier remembered set.** Old-generation env frames mutated by
    /// [`env_define`](Self::env_define) since the last minor collection — the only
    /// way an old object can come to reference a young one (a frame promoted while
    /// still mid-bind, e.g. a collection during a `let` rhs eval, then bound
    /// further). A minor collection scans these as extra roots and rewrites their
    /// bindings to the promoted handles, then clears the set. Empty on the common
    /// path (binds finish in the nursery). Env-frame binding (late binding / `def`
    /// rebinding, ADR-013) is the **only** data mutation the collector must track;
    /// every Lisp value is immutable, so the minor flip can safely rely on the
    /// invariant that old never points to young everywhere else.
    remembered: Vec<EnvId>,
    /// The **old-generation** epoch — stamped into tenured handles
    /// (`local_old_gen`) and bumped only by a *major* collection (which moves old
    /// objects). A minor collection leaves old objects in place, so it does **not**
    /// bump this — old handles stay valid across minor GCs. Routed to by the
    /// LOCAL accessors when `handle.is_old()`. See [`local_epoch`](Self::local_epoch)
    /// for the nursery counterpart.
    old_epoch: u32,
    /// Live old-generation object count after the last collection; a *major*
    /// collection is triggered when `old` grows past `2×` this (recomputed each
    /// major), so major GCs stay rare while minors keep the nursery bounded.
    major_threshold: usize,
    /// GC observability counters (Tier-1; `docs/memory-review.md` §7). Bumped by
    /// every [`arena_flip`](Self::arena_flip) — so they count both the automatic
    /// Stage-B safepoint collections and any bare [`flush`](Self::flush) (the
    /// tested arena-flip helper), which share that path. Read out via `(gc-stats)`.
    /// Per-heap (per Brood process), reset
    /// to zero only at process start; survive arena flips (the flip writes them,
    /// it doesn't clear them). `u64` so a long-lived server loop can't wrap them.
    /// `gc_runs` = collections performed; `gc_copied` = cumulative survivors
    /// relocated; `gc_reclaimed` = cumulative objects dropped (live-before minus
    /// survivors). These are *counts of LOCAL objects*, not bytes — the cheap,
    /// traversal-free figure (cf. [`local_bytes`](Self::local_bytes) for a byte
    /// estimate).
    gc_runs: u64,
    gc_copied: u64,
    gc_reclaimed: u64,
    /// GC **pause durations** (the observability timing tier, ROADMAP survey
    /// gap #4 — counts alone can't answer "is GC why this frame stuttered").
    /// Cumulative / max / most-recent collection wall time in nanoseconds,
    /// measured around [`collect`](Self::collect)'s body (covers both the
    /// legacy flip and the generational path). Timing cost is two `Instant`
    /// reads per *collection* — noise against the µs–ms the collection itself
    /// takes. Surfaced by `(gc-stats)` as `:pause-total-us` / `:pause-max-us` /
    /// `:pause-last-us`.
    gc_ns_total: u64,
    gc_ns_max: u64,
    gc_ns_last: u64,
    /// Per-process heap limit (bytes), the BEAM `max_heap_size` analogue — set by
    /// this process on itself via `(process-flag :max-heap n)`, `None` = unlimited
    /// (the default; the ADR-043 global soft/hard cap is separate). Checked
    /// **after** each collection against the *surviving* footprint (nursery +
    /// old gen), so transient garbage a collection reclaims never trips it.
    proc_mem_limit: Option<usize>,
    /// Sticky post-collection flag: `Some(live_bytes)` when the last collection
    /// left the heap over `proc_mem_limit`. Probed (and cleared) at the eval/VM
    /// safepoints, which raise a catchable error **in this process only** — the
    /// per-process isolation the global hard cap (whole-OS-process abort) lacks.
    proc_limit_hit: Option<usize>,
    /// `(process-flag :send-errors on)` — when set, a `send` whose target *node*
    /// is unknown/disconnected raises a catchable `:noconnection` error instead
    /// of silently dropping the message (the dist self-healing seam). Default
    /// off: Erlang's silent-send semantics.
    proc_send_errors: bool,
    /// Per-process GC **trace** switch (`(gc-trace on/off)`, defaulted from
    /// `BROOD_GC_TRACE`). When set, each minor/major collection prints a one-line
    /// summary to stderr — a Tier-1 observability aid for tests/benchmarks (the
    /// numbers `(gc-stats)` reports as cumulative totals, but per collection as
    /// they happen). Per-process like every other heap field: a spawned child
    /// starts from the `BROOD_GC_TRACE` default, not the parent's setting.
    gc_trace: bool,
    /// Compiling-VM body cache (ADR-076, `BROOD_VM`). Maps a closure handle's raw
    /// bits to its compiled single-arm body, or `None` if the closure isn't
    /// VM-eligible (so we don't re-attempt). Per-process (a `RefCell`, like
    /// `global_ic`). The key is **namespaced** (`VmCacheKey`) because two stable
    /// handle spaces are mixed: a top-level RUNTIME closure is keyed by its own
    /// closure-handle `.0`, while a local-capturing closure (Stage 2c) is keyed by
    /// its **body-code handle** — the closure's `ClosureId` is a LOCAL handle whose
    /// index is recycled after GC, so it can't be a stable key, but the body forms
    /// it points at live in the immovable RUNTIME code region (ADR-076 §2c(a)). The
    /// two spaces share the same numeric range, so the `u8` tag keeps them apart. A
    /// `def` rebind promotes a *new* closure (new handle → new key), so a stale
    /// entry is simply never looked up again. Empty unless `BROOD_VM` is on. `Arc`
    /// so the trampoline can hold the compiled body across a call without borrowing
    /// the cache.
    vm_cache: RefCell<VmCacheMap<Option<Arc<crate::eval::compile::CompiledClosure>>>>,
    /// The [`RuntimeCode::free_epoch`] this process last synced its [`Self::vm_cache`]
    /// to (ADR-091 Stage 4). When the shared free-epoch advances (a generation was
    /// freed and its slot may be reused with bit-identical handles), the `vm_cache`
    /// read path clears the cache once and updates this — so a stale compiled body
    /// can't alias a reused handle. Cheap: one relaxed atomic load + compare per
    /// closure-call cache lookup, a full clear only on the rare free.
    seen_free_epoch: Cell<u64>,
    /// **RUNTIME collector — Stage 4 (drain free-attempt throttle, ADR-091).** A
    /// per-process tick rate-limiting how often this process runs the multigen drain
    /// **free-attempt** ([`crate::process::free_drained_gen`] → the O·live-process
    /// `report_parked_liveness` whole-registry scan) at its safepoint. While a drain is
    /// armed the threshold is held low so every safepoint re-enters the collector; when
    /// the drain can't yet complete — a long-lived process still runs old-generation
    /// code, so the generation stays pinned — that means the O(live-process) registry
    /// scan runs on *every* safepoint of *every* worker purely to re-discover "still not
    /// drained" (measured: 800 k scans / 20 M mailbox locks on a 30-round repro, ~6×
    /// the default runtime). Throttling the free-attempt to 1/[`RT_DRAIN_SCAN_STRIDE`]
    /// cuts that ~stride-fold; the free is still attempted regularly (no lost wakeup) as
    /// long as any process reaches a safepoint, and every process's O(1) drain
    /// self-report still runs every frame so acks stay current. A plain `Cell` (the
    /// `Heap` is single-threaded), so the throttle adds no atomic or cross-core traffic.
    rt_drain_tick: Cell<u32>,
    /// **RUNTIME collector — Stage 3c (local clean-ack cache, ADR-091).** The drain
    /// epoch this process last reported *clean* for (0 = none). While a drain stays
    /// armed — which it does for the whole run whenever a long-lived process pins the
    /// draining generation (a top-level test-runner loop still executing old-gen code;
    /// Erlang has the same local-call limitation) — every process's safepoint calls
    /// `report_gen_liveness`, and a process that already acked clean would re-take the
    /// shared `drain_acks` *read* lock every frame just to re-confirm its ack. That
    /// per-frame lock, across every worker for the whole run, is the residual cost once
    /// the scan and the dirty write are handled (the `rounds`-shape ~6× overhead). This
    /// `Cell` short-circuits it: once clean for epoch E, the process is clean for E by
    /// the clean-stays-clean invariant, so a `Cell` read + compare replaces the lock. A
    /// fresh drain bumps the epoch (≠ the cached value) so the process re-reports. Plain
    /// `Cell`: the `Heap` is single-threaded.
    acked_drain_epoch: Cell<u64>,
    /// Per-heap safepoint tick throttling the drain self-report to 1/[`DRAIN_REPORT_STRIDE`]
    /// (see the const). Plain `Cell` (the `Heap` is single-threaded), read/written with no
    /// shared atomic so a throttled frame is nearly free; a miscount only shifts *when* a
    /// report fires, never its correctness. Reset by [`begin_gen_drain`](Self::begin_gen_drain)
    /// on the arming process so its first report is prompt.
    drain_report_tick: Cell<u32>,
    /// **Phase-2 dirty re-validation throttle** for the drain self-report. When the private
    /// probe finds this process dirty via Phase 2 (a RUNTIME handle embedded in its LOCAL
    /// heap data — see `runtime_gen_referenced_private`), it records the drain epoch here and
    /// re-runs that O(heap) walk only every [`P2_REVALIDATE_STRIDE`] safepoints, reporting a
    /// cheap stale-dirty verdict in between. Reset to `u64::MAX` (an epoch that never matches)
    /// when the probe next finds it clean. `p2_dirty_tick` counts safepoints within the epoch.
    /// Plain `Cell`s: the `Heap` is single-threaded.
    p2_dirty_epoch: Cell<u64>,
    p2_dirty_tick: Cell<u32>,
    /// **Phase-1 dirty re-validation throttle**, the deep-recursion counterpart of the
    /// Phase-2 pair above. Armed only while this process's Phase-1 seed exceeds
    /// [`P1_LARGE_SEED`] — a `roots` stack that has grown with recursion depth — so a
    /// shallow process is never throttled and keeps acking on its very next safepoint.
    /// See [`P1_REVALIDATE_STRIDE`].
    p1_dirty_epoch: Cell<u64>,
    p1_dirty_tick: Cell<u32>,
    /// **Receive-mark** (ADR-195): the most recent `(ref)` this process minted, paired with
    /// its mailbox's arrival sequence at that instant. A `receive` whose every clause pins
    /// that ref can start its scan at the first message with `seq >= mark`, because a
    /// message enqueued *before* the ref existed cannot possibly carry it — turning a
    /// backlogged selective receive from O(backlog) into a binary search.
    ///
    /// One entry, deliberately: it covers `(let (r (ref)) (send …) (receive ([:reply ^r v]
    /// …)))`, which is every synchronous call in the language. A nested call evicts it and
    /// the outer receive simply scans from the front — slower, never wrong.
    recv_mark: Cell<(u64, u64)>,
    /// The compiled arms **currently executing** on this process's stack — a stack
    /// pushed by `compile::vm_apply` (and the top-level `run`) on entry, the top
    /// updated on a tail-call into a different arm, popped on return. `runtime_collect`
    /// walks these after evacuating the RUNTIME region and rewrites the movable
    /// handles their node trees embed (`Const`/`MakeClosure` literals): they're the
    /// one RUNTIME-handle holder the root walk can't reach — the `Arc`'d node tree is
    /// off the GC root graph, and `exec_node` holds it by `&Node`, so the `Arc` can't
    /// be swapped for a relocated copy. (The non-live arms in `vm_cache` are just
    /// cleared and rebuilt lazily; only the live ones need fixup.) Empty unless the VM
    /// is running a body. See ADR-076 / `docs/known-issues.md`.
    live_vm_arms: Vec<Arc<crate::eval::compile::CompiledArm>>,
    /// Call-site inline caches (ADR-096). Indexed by the `site` id a compiled
    /// `Node::Call` with a global-symbol callee carries; each entry caches that
    /// site's most recent resolution — the callee value, and (for a VM-eligible
    /// non-passthrough closure callee) its compiled arm + captured env — stamped
    /// with the global epoch it was resolved at. A probe validates
    /// `(sym, argc, epoch)`, so a `def` rebind, a `restore_globals` swap, or a
    /// RUNTIME compaction (all bump `runtime.version`) invalidates every entry
    /// without a sweep; sym+argc are re-checked so a recycled site id after
    /// [`Heap::runtime_collect`] clears the table can never alias a different
    /// call site into a wrong hit. Per-process (`RefCell`, like `vm_cache`); a
    /// site is allocated at compile time ([`Heap::vm_site_alloc`]), so ids are
    /// only as dense as the code this process actually compiled.
    vm_call_ics: RefCell<Vec<Option<CallIcEntry>>>,
    /// **IR-readable mirror** of the fast-link memo (Track B / Technique A): a flat,
    /// `#[repr(C)]` side table indexed by the same call-site id as [`Self::vm_call_ics`],
    /// so JIT'd code can read a site's `(epoch, code, nslots, env)` with a raw load + an
    /// epoch compare — no `RefCell` borrow, no `Vec<Option<…>>` niche, no `Cell` (none of
    /// which are safe to touch from Cranelift IR). It is the same data as a
    /// [`CallIcEntry::fast`] memo, written in lockstep by [`Self::vm_call_ic_fast_link`].
    /// A slot is **valid** only when `epoch == global_epoch()`; a `def`/compaction bumps
    /// the epoch (so a stale or recycled slot misses the IR guard and falls to the slow
    /// path), and the table is cleared in lockstep with `vm_call_ics` on a
    /// [`Self::runtime_collect`]. Grown by [`Self::vm_site_alloc`] so it stays the same
    /// length as `vm_call_ics` (the IR bounds-checks `site < len` for a live arm whose
    /// site ids outran a post-collect re-grow).
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    vm_fast_links: RefCell<Vec<FastLink>>,
    /// DEBUG ONLY: per-call-site source position, recorded at compile time and indexed
    /// by the same site id as [`Self::vm_call_ics`]. Lets a crash map a runtime call site
    /// back to its `.blsp` file:line (call-site ids are positional + reset on
    /// `runtime_collect`, so this is grown/cleared in lockstep). For diagnosing the JIT
    /// stale-operand bug — see `dbg_site_pos` / `dbg_set_site_pos`.
    #[cfg(debug_assertions)]
    #[cfg(debug_assertions)]
    dbg_site_pos: RefCell<Vec<Option<(crate::error::Pos, Option<Arc<str>>)>>>,
    /// Global-read inline caches (ADR-096) — the value-position counterpart of
    /// [`Self::vm_call_ics`], indexed by a compiled `Node::GlobalIc`'s site id.
    /// Same lifecycle: allocated at compile time, validated by (sym, epoch),
    /// cleared wholesale on a RUNTIME compaction.
    vm_global_ics: RefCell<Vec<Option<GlobalIcEntry>>>,
    /// Per-arm IC block registry (ADR-175 Phase A): [`CompiledArm::uid`] → the
    /// `(call-site base, global-site base)` this process allocated for that arm in
    /// the tables above. Blocks are contiguous, lazily allocated on first activation
    /// ([`Heap::vm_arm_block`]), and never individually freed — a `runtime_collect`
    /// table clear drops the whole map in lockstep with the tables.
    arm_ic_blocks: RefCell<std::collections::HashMap<u64, (u32, u32)>>,
    /// The **currently executing arm's** IC block bases (call sites / global sites).
    /// Set by the VM/JIT drivers at every arm transition; every site-indexed IC
    /// method resolves `base + arm-relative site` through these. Plain `Cell`s: the
    /// Heap is single-threaded (one worker owns a process at a time).
    cur_ic_base: Cell<u32>,
    cur_gic_base: Cell<u32>,
    /// Ability-dispatch inline cache (ADR-172 §7), keyed by an op's `[ability op]` symbol
    /// pair packed into a `u64`, on the fast [`SymbolHasher`]. Per process, like the other
    /// ICs; validated by (`id`, `global_epoch`) so it self-heals on any `def *impls*` /
    /// compaction. See [`Self::vm_dispatch`].
    dispatch_ics:
        RefCell<HashMap<u64, DispatchIcEntry, std::hash::BuildHasherDefault<SymbolHasher>>>,
    /// JIT execution state, per process. These were thread-locals; moved onto the heap so
    /// (a) they travel with a process that migrates worker threads — notably `jit_force_vm`,
    /// which must stay set across a yield during an over-deep VM drain — and (b) each access
    /// is a plain field load rather than a TLS lookup (the linked-call hot path touches them
    /// ~4× per call). Only meaningful while a JIT'd arm is on the stack.
    ///
    /// The executing JIT'd arm's env (its compiled `fn(heap, base)` carries none, but a
    /// Brood→Brood call needs it to resolve a free-global callee). Save/restored around each
    /// native-arm entry ([`jit_tier`]) so re-entry nests correctly.
    // These four are read only from JIT-gated code paths, so a non-jit build
    // (e.g. `brood-lsp`) sees them as dead. Keep them (they're written by the
    // shared initializers) and silence the lint only when jit is off.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_call_env: EnvRoot,
    /// Native-to-native call recursion depth — bounds the native stack (which `MAX_BC_FRAMES`
    /// doesn't), draining deeper recursion onto the VM instead of overflowing.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_native_depth: u32,
    /// **Absolute stack address below which a JIT'd arm must not run** (KI-14). Every
    /// compiled arm's prologue loads this and deopts to the VM if its own frame sits
    /// below it, so deep recursion drains into the bounded heap-frame loop instead of
    /// running the native stack into its guard page — an abort `try`/`catch` cannot see
    /// and no supervisor can restart.
    ///
    /// The pre-existing guards (`jit_native_depth` + the `stacker` headroom probe) sit on
    /// the *dispatch* paths, so they only bound recursion that goes through a fast link.
    /// A JSON parse 100 000 levels deep proved a path that reaches none of them: on the
    /// root thread the depth cap fired at 1500, while in a spawned green process the probe
    /// was never even called and the worker died. Checking in the prologue is the one place
    /// every native frame must pass, whatever route created it.
    ///
    /// Written by the three native entry points (`jit_tier`, `jit_run_fast_link`, the
    /// i64-worker wrapper) from the *live* remaining stack, so it is correct on the root
    /// thread and on any worker regardless of their differing stack bases. `0` disables
    /// the check (the probe couldn't read the stack — fail open, as the old code did).
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_stack_limit: usize,
    /// Set while draining an over-deep native-recursion subtree on the VM ([`jit_tier`]
    /// reads it and declines to run native, keeping the recursion in the bounded heap-frame
    /// loop).
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_force_vm: bool,
    /// Diagnostic only (`BROOD_JIT_VERIFY`/staged-stale): the symbol name of the JIT'd arm
    /// currently executing native code (`u32::MAX` = none/unknown). Set on each native entry
    /// and restored after, so when that arm stages a stale handle for a sub-call the report
    /// can name the *caller* arm (the one holding the stale handle), not just the callee.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_dbg_fn: u32,
    /// An error parked by a JIT runtime callback (the C ABI can't return a `Value` *and* an
    /// error); the arm returns the error outcome and [`vm_run_bc`] takes this to propagate.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_pending_error: Option<crate::error::LispError>,
    /// Overflow sentinel for the unboxed-`i64` fast path (the register calling convention for
    /// int-only recursive arms). That path carries args/results as raw `i64` in registers and
    /// uses overflow-checked arithmetic; on an overflow (or a non-`Int` at the boxed entry) it
    /// sets this and unwinds, and the boxed wrapper deopts to the VM — which recomputes with
    /// BigInt, keeping the JIT bit-identical to the VM. A plain `bool` (per-process heap, only
    /// this process's native code touches it); the JIT loads/stores it through a stable pointer
    /// fetched once at arm entry (`brood_rt_i64_overflow_ptr`).
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_i64_overflow: bool,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

/// Bump-only allocation: append to the slab, return the new index. The shared
/// shape behind `alloc_pair`, `alloc_vector`, `alloc_map`, `alloc_closure`
/// (and the rest). Indices grow monotonically per process — **no slot is ever
/// reused in place**, which is what makes a stale handle detectable (the
/// epoch tripwire) instead of silently aliasing fresh data. Slab `len()` is
/// bounded not by a free list but by collections relocating survivors into
/// fresh slabs and dropping the old slabs wholesale.
macro_rules! alloc_slot {
    ($self:expr, $field:ident, $value:expr) => {{
        $crate::perf_bump!(alloc);
        let idx = $self.local.$field.len();
        $self.local.$field.push($value);
        idx
    }};
}

/// The `form_pos` map key for a LOCAL pair: its slab index packed with the
/// generation age bit (bit 32). Nursery and old pairs share one slab-index range,
/// so the age bit keeps their source-position entries from colliding.
#[inline]
fn form_pos_key(id: PairId) -> u64 {
    (id.index() as u64) | ((id.is_old() as u64) << 32)
}

/// True iff `v` is a LOCAL heap object the copying collector relocates — the set
/// `push_value`/`flush_value` move in place during a LOCAL (nursery/major)
/// collection. Atoms (`Int`, `Sym`, `Pid`, …) and shared-region
/// (`PRELUDE`/`RUNTIME`) handles are never touched by the LOCAL collector.
///
/// **Not** the rooting predicate: a RUNTIME handle is immovable under the LOCAL
/// collector but *is* evacuated by the runtime compactor, so it still needs an
/// operand-stack slot — see [`needs_root_slot`], which [`Heap::root`] uses.
#[inline]
pub fn is_movable(v: Value) -> bool {
    match v.unpack() {
        ValueRef::Pair(id) => id.region() == LOCAL,
        ValueRef::Vector(id) => id.region() == LOCAL,
        ValueRef::Range(id) => id.region() == LOCAL,
        ValueRef::SeqView(id) => id.region() == LOCAL,
        ValueRef::Map(id) => id.region() == LOCAL,
        ValueRef::Set(id) => id.region() == LOCAL,
        ValueRef::Str(id) => id.region() == LOCAL,
        ValueRef::BigInt(id) => id.region() == LOCAL,
        ValueRef::Decimal(id) => id.region() == LOCAL,
        ValueRef::Ratio(id) => id.region() == LOCAL,
        ValueRef::Bytes(id) => id.region() == LOCAL,
        ValueRef::Rope(id) => id.region() == LOCAL,
        ValueRef::Fn(id) | ValueRef::Macro(id) => id.region() == LOCAL,
        _ => false,
    }
}

/// True iff a handle to `v` held across a collection safepoint must take an
/// operand-stack slot to be rewritten — because **some** collector relocates it:
/// a LOCAL object (the copying collector moves it) **or** a RUNTIME object (the
/// runtime compactor [`Heap::runtime_collect`] evacuates the shared code region,
/// ADR-076). Only atoms and the immutable PRELUDE region are truly fixed and may
/// stay inline as a [`Root::Stable`].
///
/// This is the superset [`Heap::root`] gates on; [`is_movable`] is the narrower
/// LOCAL-only set. The distinction matters because a RUNTIME constant held inline
/// (e.g. a `let` body or a `do` spine cursor in hot-reloaded/REPL code) would be
/// invisible to `runtime_collect`'s root rewrite and go stale across a
/// compaction — the slab-OOB / silent-corruption class in `docs/known-issues.md`.
#[inline]
pub fn needs_root_slot(v: Value) -> bool {
    let shared = |r| r == LOCAL || r == RUNTIME;
    match v.unpack() {
        ValueRef::Pair(id) => shared(id.region()),
        ValueRef::Vector(id) => shared(id.region()),
        ValueRef::Range(id) => shared(id.region()),
        ValueRef::SeqView(id) => shared(id.region()),
        ValueRef::Map(id) => shared(id.region()),
        ValueRef::Set(id) => shared(id.region()),
        ValueRef::Str(id) => shared(id.region()),
        ValueRef::BigInt(id) => shared(id.region()),
        ValueRef::Decimal(id) => shared(id.region()),
        ValueRef::Ratio(id) => shared(id.region()),
        ValueRef::Bytes(id) => shared(id.region()),
        ValueRef::Rope(id) => shared(id.region()),
        ValueRef::Fn(id) | ValueRef::Macro(id) => shared(id.region()),
        _ => false,
    }
}

/// A rooted value handle from [`Heap::root`]: either a truly-fixed value kept
/// inline (no operand-stack slot) or the index of a slot a collector rewrites.
/// Read back with [`Heap::read_root`] after any potential collection. Running
/// prelude code or handling atoms pays no `Vec` churn; LOCAL handles **and**
/// RUNTIME handles (which `runtime_collect` evacuates) take a slot — see
/// [`needs_root_slot`].
#[derive(Clone, Copy)]
pub enum Root {
    /// A truly-fixed value (atom or `PRELUDE` handle); the inline copy stays
    /// valid across any collection. RUNTIME handles do **not** use this — they
    /// take a `Slot`, since the runtime compactor relocates them.
    Stable(Value),
    /// A relocatable value (LOCAL or RUNTIME) parked at this operand-root-stack
    /// index; rewritten in place by whichever collector moves it.
    Slot(usize),
}

/// The [`EnvId`] counterpart of [`Root`] — see [`Heap::root_env`]. The
/// [`EnvId::GLOBAL`] sentinel and immutable PRELUDE frames stay inline; a LOCAL
/// **or** RUNTIME frame takes a slot (the latter is evacuated by the runtime
/// compactor, so it must be rewritten there).
#[derive(Clone, Copy)]
pub enum EnvRoot {
    Stable(EnvId),
    Slot(usize),
}

// ===== Construction and shared-region management ================================

mod equality;
mod gc;
mod gc_runtime;
mod map_ops;
mod vm_cache;
// `stall_guard` is used by the RUNTIME compactor (`gc_runtime`) and the GUI paint
// path, so it's re-exported unconditionally; `stall_guard_pid` by the scheduler.
pub(crate) use self::gc::{stall_guard, stall_guard_pid, stall_threshold_ms};
pub(crate) use self::vm_cache::{
    CallIcEntry, DispatchIcEntry, FastLink, GlobalIcEntry, VmCacheKey,
};

impl Heap {
    /// Park a message value copied into this heap, returning its slot index. Reuses a
    /// tombstoned slot when one is free so a steady request/response process never grows
    /// the table past one entry.
    pub fn msg_root_add(&mut self, v: Value) -> u32 {
        let table = self.msg_roots.get_or_insert_with(Box::default);
        if let Some(i) = table.iter().position(|s| matches!(s, Value::Nil)) {
            table[i] = v;
            return i as u32;
        }
        table.push(v);
        (table.len() - 1) as u32
    }

    /// Take the value out of slot `i`, tombstoning it for reuse. Returns `nil` for an
    /// out-of-range index, which cannot happen for an envelope this heap produced.
    pub fn msg_root_take(&mut self, i: u32) -> Value {
        match self.msg_roots.as_mut().and_then(|t| t.get_mut(i as usize)) {
            Some(slot) => std::mem::replace(slot, Value::nil()),
            None => Value::nil(),
        }
    }

    /// Read slot `i` without clearing it — the peek-in-place scan path, where a
    /// candidate that fails to match must stay queued with its slot intact.
    pub fn msg_root_peek(&self, i: u32) -> Value {
        self.msg_roots
            .as_ref()
            .and_then(|t| t.get(i as usize))
            .copied()
            .unwrap_or(Value::nil())
    }
}

impl Heap {
    /// The cold loader/checker state, if this process has ever needed it. `None` is the
    /// normal case for a worker and means "empty" for every reader.
    #[inline]
    fn cold(&self) -> Option<&ColdHeap> {
        self.cold.as_deref()
    }

    /// The cold state, allocating it on first use. Only write paths call this — a module
    /// load, a `defmodule` compile, or a checker run.
    #[inline]
    fn cold_mut(&mut self) -> &mut ColdHeap {
        self.cold.get_or_insert_with(Default::default)
    }
}

impl Heap {
    /// A bare heap with empty shared regions — used to *build* the prelude
    /// before freezing it. Real runtimes use [`Heap::with_regions`]. GC is
    /// disabled here (the prelude is built once, then frozen — collection would
    /// be wasted work and could complicate `freeze_as_shared_code` if it left
    /// holes mid-build).
    pub fn new() -> Self {
        Heap {
            local: Slabs::default(),
            old: None,
            gen_cache: [RefCell::new(None), RefCell::new(None)],
            gen_cache_ver: [Cell::new(u64::MAX), Cell::new(u64::MAX)],
            closure_tpl_cache: RefCell::new(ClosureTemplateMap::default()),
            closure_tpl_ver: Cell::new(u64::MAX),
            closure_const_cache: RefCell::new(ConstClosureMap::default()),
            closure_const_ver: Cell::new(u64::MAX),
            prelude: Arc::default(),
            runtime: Arc::default(),
            global: EnvId::local(0),
            dynamics: Vec::new(),
            #[cfg(feature = "dev-tools")]
            trace_context: None,
            #[cfg(feature = "dev-tools")]
            trace_context_own: false,
            global_ic: RefCell::new(SymbolMap::default()),
            rooted_ref_ic: RefCell::new(SymbolMap::default()),
            msg_roots: None,
            cold: None,
            check: RefCell::new(None),
            roots: Vec::new(),
            env_roots: Vec::new(),
            gc_threshold: usize::MAX,
            park_trim_mark: 0,
            rt_gc_threshold: usize::MAX,
            gc_enabled: false,
            rt_collect_block: std::cell::Cell::new(0),
            local_epoch: 0,
            remembered: Vec::new(),
            old_epoch: 0,
            major_threshold: usize::MAX,
            gc_runs: 0,
            gc_copied: 0,
            gc_reclaimed: 0,
            gc_ns_total: 0,
            gc_ns_max: 0,
            gc_ns_last: 0,
            proc_mem_limit: None,
            proc_limit_hit: None,
            proc_send_errors: false,
            gc_trace: gc_trace_default(),
            vm_cache: RefCell::new(VmCacheMap::default()),
            seen_free_epoch: Cell::new(0),
            rt_drain_tick: Cell::new(0),
            acked_drain_epoch: Cell::new(0),
            drain_report_tick: Cell::new(0),
            p2_dirty_epoch: Cell::new(u64::MAX),
            p2_dirty_tick: Cell::new(0),
            p1_dirty_epoch: Cell::new(u64::MAX),
            p1_dirty_tick: Cell::new(0),
            recv_mark: Cell::new((0, 0)),
            live_vm_arms: Vec::new(),
            vm_call_ics: RefCell::new(Vec::new()),
            vm_fast_links: RefCell::new(Vec::new()),
            #[cfg(debug_assertions)]
            #[cfg(debug_assertions)]
            dbg_site_pos: RefCell::new(Vec::new()),
            vm_global_ics: RefCell::new(Vec::new()),
            arm_ic_blocks: RefCell::new(std::collections::HashMap::new()),
            cur_ic_base: Cell::new(0),
            cur_gic_base: Cell::new(0),
            dispatch_ics: RefCell::new(HashMap::default()),
            jit_call_env: EnvRoot::Stable(EnvId::GLOBAL),
            jit_native_depth: 0,
            jit_stack_limit: 0,
            jit_force_vm: false,
            jit_dbg_fn: u32::MAX,
            jit_pending_error: None,
            jit_i64_overflow: false,
        }
    }

    /// A fresh process heap sharing the given prelude + runtime regions (empty
    /// local slabs). Spawned inner processes pass the *same* `runtime` Arc as
    /// their parent, so they see its global bindings and its later `def`s.
    pub fn with_regions(prelude: Arc<SharedCode>, runtime: Arc<RuntimeCode>) -> Self {
        Heap {
            local: Slabs::default(),
            old: None,
            gen_cache: [RefCell::new(None), RefCell::new(None)],
            gen_cache_ver: [Cell::new(u64::MAX), Cell::new(u64::MAX)],
            closure_tpl_cache: RefCell::new(ClosureTemplateMap::default()),
            closure_tpl_ver: Cell::new(u64::MAX),
            closure_const_cache: RefCell::new(ConstClosureMap::default()),
            closure_const_ver: Cell::new(u64::MAX),
            prelude,
            runtime,
            global: EnvId::local(0),
            dynamics: Vec::new(),
            #[cfg(feature = "dev-tools")]
            trace_context: None,
            #[cfg(feature = "dev-tools")]
            trace_context_own: false,
            global_ic: RefCell::new(SymbolMap::default()),
            rooted_ref_ic: RefCell::new(SymbolMap::default()),
            msg_roots: None,
            cold: None,
            check: RefCell::new(None),
            roots: Vec::new(),
            env_roots: Vec::new(),
            gc_threshold: gc_floor(),
            park_trim_mark: 0,
            rt_gc_threshold: rt_gc_floor(),
            gc_enabled: true,
            rt_collect_block: std::cell::Cell::new(0),
            local_epoch: 0,
            remembered: Vec::new(),
            old_epoch: 0,
            major_threshold: major_floor(),
            gc_runs: 0,
            gc_copied: 0,
            gc_reclaimed: 0,
            gc_ns_total: 0,
            gc_ns_max: 0,
            gc_ns_last: 0,
            proc_mem_limit: None,
            proc_limit_hit: None,
            proc_send_errors: false,
            gc_trace: gc_trace_default(),
            vm_cache: RefCell::new(VmCacheMap::default()),
            seen_free_epoch: Cell::new(0),
            rt_drain_tick: Cell::new(0),
            acked_drain_epoch: Cell::new(0),
            drain_report_tick: Cell::new(0),
            p2_dirty_epoch: Cell::new(u64::MAX),
            p2_dirty_tick: Cell::new(0),
            p1_dirty_epoch: Cell::new(u64::MAX),
            p1_dirty_tick: Cell::new(0),
            recv_mark: Cell::new((0, 0)),
            live_vm_arms: Vec::new(),
            vm_call_ics: RefCell::new(Vec::new()),
            vm_fast_links: RefCell::new(Vec::new()),
            #[cfg(debug_assertions)]
            #[cfg(debug_assertions)]
            dbg_site_pos: RefCell::new(Vec::new()),
            vm_global_ics: RefCell::new(Vec::new()),
            arm_ic_blocks: RefCell::new(std::collections::HashMap::new()),
            cur_ic_base: Cell::new(0),
            cur_gic_base: Cell::new(0),
            dispatch_ics: RefCell::new(HashMap::default()),
            jit_call_env: EnvRoot::Stable(EnvId::GLOBAL),
            jit_native_depth: 0,
            jit_stack_limit: 0,
            jit_force_vm: false,
            jit_dbg_fn: u32::MAX,
            jit_pending_error: None,
            jit_i64_overflow: false,
        }
    }

    /// Clone the Arc to this heap's prelude region (for spawning a child).
    pub fn prelude_arc(&self) -> Arc<SharedCode> {
        Arc::clone(&self.prelude)
    }

    /// Clone the Arc to this runtime's shared code region (for spawning a child
    /// that shares this runtime's live globals).
    /// This heap's runtime-instance tag — see [`RuntimeCode::runtime_tag`]. Unconditional
    /// (it was `jit`-only) because the messaging path needs it to decide whether a target
    /// process shares this runtime, and therefore whether a RUNTIME handle may cross to it.
    pub(crate) fn runtime_tag(&self) -> u64 {
        self.runtime.runtime_tag
    }

    pub fn runtime_arc(&self) -> Arc<RuntimeCode> {
        Arc::clone(&self.runtime)
    }

    /// Whether `other` is a heap of the **same runtime** — i.e. the two processes
    /// read one shared RUNTIME code region through the same `Arc`, so a handle into
    /// it is meaningful in both. Pointer comparison, no `Arc` clone.
    ///
    /// This is the precondition for handing a promoted (shared-region) handle from
    /// one process to another instead of deep-copying: `spawn` relies on it
    /// implicitly (parent and child share the region by construction), and the
    /// local-send closure fast path checks it explicitly, because the process
    /// REGISTRY is global — two `Interp`s in one OS process (a test harness, an
    /// embedder) have *different* regions, and a handle must never cross that line.
    /// Record that `ref_id` was minted when this process's mailbox was at arrival
    /// sequence `seq` — see [`Heap::recv_mark`].
    pub fn set_recv_mark(&self, ref_id: u64, seq: u64) {
        self.recv_mark.set((ref_id, seq));
    }

    /// The arrival sequence to start a scan at for a receive pinned on `ref_id`, or
    /// `None` when that ref is not the one we last minted (so we must scan from the front).
    pub fn recv_mark_for(&self, ref_id: u64) -> Option<u64> {
        let (id, seq) = self.recv_mark.get();
        (id == ref_id).then_some(seq)
    }

    pub fn shares_runtime_with(&self, other: &Heap) -> bool {
        Arc::ptr_eq(&self.runtime, &other.runtime)
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

    pub fn freeze_as_shared_code(mut self, root: EnvId) -> (SharedCode, Vec<(Symbol, Value)>) {
        // Pull anything a global reaches into the LOCAL slabs first, so the
        // re-tag below is valid for every handle it touches (KI-12).
        {
            let mut fwd: HashMap<(u8, u32, u8), Value> = HashMap::new();
            let vars: Vec<(Symbol, Value)> = self.local.envs[root.index()].vars.to_vec();
            for (i, (_, v)) in vars.iter().enumerate() {
                let lv = self.localize_for_freeze(*v, &mut fwd);
                if handle_key(lv) != handle_key(*v) {
                    self.local.envs[root.index()].vars[i].1 = lv;
                }
            }
        }
        let bindings: Vec<(Symbol, Value)> = self.local.envs[root.index()]
            .vars
            .iter()
            .map(|&(s, v)| {
                debug_assert!(
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
                        ValueRef::Vector(id) if id.region() == LOCAL => {
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

        (SharedCode { slabs, def_sites }, bindings)
    }

    // ===== Process global scope =================================================

    /// Record this process's global scope (call once, after creating it).
    pub fn set_global(&mut self, env: EnvId) {
        self.global = env;
    }

    /// This process's global scope.
    pub fn global(&self) -> EnvId {
        self.global
    }

    /// True if `env` is this process's global scope.
    pub fn is_global(&self, env: EnvId) -> bool {
        env == self.global
    }

    /// Snapshot the LOCAL heap's current sizes (for arena-reset reclamation).
    pub fn checkpoint(&self) -> LocalCheckpoint {
        LocalCheckpoint {
            pairs: self.local.pairs.len(),
            vectors: self.local.vectors.len(),
            maps: self.local.maps.len(),
            strings: self.local.strings.len(),
            bigints: self.local.bigints.len(),
            decimals: self.local.decimals.len(),
            ratios: self.local.ratios.len(),
            bytes: self.local.bytes.len(),
            ropes: self.local.ropes.len(),
            closures: self.local.closures.len(),
            envs: self.local.envs.len(),
            epoch: self.local_epoch,
        }
    }

    /// Reclaim everything allocated into the LOCAL heap since `cp`, by truncating
    /// the slabs back to it.
    ///
    /// **Safety contract (logical, not `unsafe`):** call this only at a top-level
    /// boundary — when the evaluator has fully returned and no value reachable
    /// from here on holds a LOCAL handle at or past `cp`. Globals live in the
    /// PRELUDE/RUNTIME regions and never point into LOCAL (a top-level `def`
    /// *promotes* its value out), so they're always safe; the only thing that can
    /// still be live is the *result* of the form just evaluated — consume or
    /// promote it before resetting. Resetting mid-evaluation would strand the
    /// in-flight computation's values and corrupt later reads.
    ///
    /// **Collection-safety.** If a collection fired between [`checkpoint`] and
    /// here, it already compacted the nursery (a flip rewrote the slabs; a tenure
    /// emptied them) and bumped [`local_epoch`](Self::local_epoch), so `cp`'s slab
    /// lengths no longer describe the live nursery. Truncating to them would
    /// **strand the survivors the collector just kept** (the demonstrated GC
    /// slab-OOB crash: a wide-bignum eval forced a flip, then the stale-length
    /// truncate cut live objects loose). On an epoch mismatch this is a no-op — the
    /// collection has already reclaimed the dead, and the next `gc_due` reclaims
    /// this form's now-garbage survivors. Only the no-collection fast path (epoch
    /// unchanged: a pure bump-allocated region) actually truncates.
    pub fn reset_local_to(&mut self, cp: LocalCheckpoint) {
        if self.local_epoch != cp.epoch {
            return;
        }
        self.local.pairs.truncate(cp.pairs);
        self.local.vectors.truncate(cp.vectors);
        self.local.maps.truncate(cp.maps);
        self.local.strings.truncate(cp.strings);
        self.local.bigints.truncate(cp.bigints);
        self.local.decimals.truncate(cp.decimals);
        self.local.ratios.truncate(cp.ratios);
        self.local.bytes.truncate(cp.bytes);
        self.local.ropes.truncate(cp.ropes);
        self.local.closures.truncate(cp.closures);
        self.local.envs.truncate(cp.envs);
        // Drop position metadata for the pairs just reclaimed (indices reused).
        // Keys pack the age bit at bit 32; this checkpoint path is nursery-only,
        // so compare the low-32 slab index against the checkpoint length.
        if self.cold.as_ref().is_some_and(|c| !c.form_pos.is_empty()) {
            self.cold_mut()
                .form_pos
                .retain(|&k, _| (k as u32 as usize) < cp.pairs);
        }
        // The threshold is relative to live count; reclamation here is so cheap
        // that we let the next `gc_due` check recompute against the smaller heap.
    }

    // ===== Source positions and compilation context =============================

    /// Record the source position of a LOCAL list form (no-op for atoms and
    /// forms in the shared regions). Called by the reader as it builds lists.
    pub fn set_form_pos(&mut self, v: Value, pos: crate::error::Pos) {
        if let Some(id) = v.as_pair() {
            if id.region() == crate::core::value::LOCAL {
                let file: Option<Arc<str>> = self
                    .cold()
                    .and_then(|c| c.current_file.as_deref())
                    .map(Arc::from);
                self.cold_mut()
                    .form_pos
                    .insert(form_pos_key(id), (pos, file));
            }
        }
    }

    /// The recorded source position (and originating file, if known) of a list form.
    /// LOCAL pairs read the per-heap reader-stamped table; RUNTIME pairs read the
    /// shared table `promote` carried the position into (so `form-pos` works on a
    /// frozen `defn`/lambda body and a position survives a cross-node send). PRELUDE
    /// forms carry none.
    ///
    /// Use [`form_pos_only`](Self::form_pos_only) when only the line/col is needed.
    pub fn form_pos(&self, v: Value) -> Option<(crate::error::Pos, Option<Arc<str>>)> {
        if let Some(id) = v.as_pair() {
            match id.region() {
                crate::core::value::LOCAL => {
                    return self
                        .cold()
                        .and_then(|c| c.form_pos.get(&form_pos_key(id)).cloned())
                }
                crate::core::value::RUNTIME => return self.runtime.position_of(id.index()),
                _ => {}
            }
        }
        None
    }

    /// Convenience: just the `Pos` part of [`form_pos`](Self::form_pos), for
    /// callers that don't need the file.
    pub fn form_pos_only(&self, v: Value) -> Option<crate::error::Pos> {
        self.form_pos(v).map(|(p, _)| p)
    }

    /// Set the file currently being loaded, returning the previous value so the
    /// caller can restore it (loads nest).
    pub fn set_current_file(&mut self, file: Option<String>) -> Option<String> {
        std::mem::replace(&mut self.cold_mut().current_file, file)
    }

    /// The file currently being loaded, exposed to Brood via `(current-file)`.
    pub fn current_file(&self) -> Option<&str> {
        self.cold().and_then(|c| c.current_file.as_deref())
    }

    // ----- current namespace (ADR-065) -----

    /// Set the namespace being compiled into (`None` = root), returning the prior
    /// value so the caller can restore it. File/module loaders save + reset to
    /// `None` per file; the `%in-ns` primitive sets it from an `(ns …)` form.
    pub fn set_compile_ns(&mut self, ns: Option<Symbol>) -> Option<Symbol> {
        std::mem::replace(&mut self.cold_mut().compile_ns, ns)
    }

    /// The namespace currently being compiled into, or `None` at root.
    pub fn compile_ns(&self) -> Option<Symbol> {
        self.cold().and_then(|c| c.compile_ns)
    }

    // ----- package-rooted namespaces (ADR-070) -----

    /// Enter a dependency's load: `prefix` is the dep's local name, `modules` the
    /// short module names it provides. While set, `root_module_name` roots an
    /// intra-package module reference to `prefix/name`. Returns the prior
    /// `(prefix, modules)` so the caller restores it (dep loads nest — a dep may
    /// `require` another dep). Passing `None` clears the context (root project / std).
    pub fn set_package_context(
        &mut self,
        prefix: Option<Symbol>,
        modules: HashSet<Symbol>,
    ) -> (Option<Symbol>, HashSet<Symbol>) {
        let cold = self.cold_mut();
        let prev_prefix = std::mem::replace(&mut cold.package_prefix, prefix);
        let prev_modules = std::mem::replace(&mut cold.package_modules, modules);
        // Both memos are properties of the context we just replaced — drop them so a
        // nested dep load doesn't inherit the outer package's answers: `rooted_ref_ic`
        // holds `mod/name` → rooted spellings, and `global_ic` may hold a value cached
        // under an unrooted key that resolved through one (see `global_lookup_cached`).
        // Context switches are load-time and rare, so clearing costs nothing at run time.
        self.rooted_ref_ic.borrow_mut().clear();
        self.global_ic.borrow_mut().clear();
        (prev_prefix, prev_modules)
    }

    /// The active package prefix (a dep's local name), or `None` outside a dep load.
    pub fn package_prefix(&self) -> Option<Symbol> {
        self.cold().and_then(|c| c.package_prefix)
    }

    /// The active package context as `(prefix, modules)` — the pair
    /// [`set_package_context`](Self::set_package_context) takes. Read to *propagate* the
    /// context to a spawned process: "which package is this code from?" is a property of
    /// the code, not of the process running it (see `spawn_impl`).
    pub fn package_context(&self) -> (Option<Symbol>, HashSet<Symbol>) {
        match self.cold() {
            Some(c) => (c.package_prefix, c.package_modules.clone()),
            None => (None, HashSet::new()),
        }
    }

    /// Root a referenced module name to the active package: if a dep load is active
    /// and `module` is one of that dep's provided modules, return `prefix/module`;
    /// otherwise return `module` unchanged. This is the one place the `foo/` prefix
    /// is applied — used by `%in-ns` (rooting a declared `(defmodule b)`) and the
    /// loader's `%root-module-name` (rooting `(:use b)`/`(:alias b …)`/`require`
    /// targets). An external name (std, another dep, already `foo/…`) is left alone.
    pub fn root_module_name(&mut self, module: Symbol) -> Symbol {
        let Some(prefix) = self.package_prefix() else {
            return module;
        };
        let is_intra = self
            .cold()
            .is_some_and(|c| c.package_modules.contains(&module));
        if !is_intra {
            return module;
        }
        let rooted = format!(
            "{}/{}",
            crate::core::value::symbol_name(prefix),
            crate::core::value::symbol_name(module)
        );
        crate::core::value::intern(&rooted)
    }

    /// Root an intra-package **qualified reference** — the counterpart of
    /// [`root_module_name`](Self::root_module_name) for a `mod/name` symbol appearing in
    /// *code*, rather than a module name in a `(:use …)`/`(:alias …)` clause. Inside
    /// project `bedit`, `commands/cmd-open` roots to `bedit/commands/cmd-open`.
    ///
    /// Without this the rooted model is asymmetric: `(:use commands)` roots its target, so
    /// the bare names import fine, but every explicit `commands/cmd-open` — and every
    /// `(eval 'commands/cmd-open)` behind a late-bound keymap — goes unbound. Rooting is
    /// meant to be *implied* (ADR-070), which has to include the qualified spelling.
    ///
    /// The module part is everything before the **last** `/` (a global's own name never
    /// contains one), so a nested module splits correctly: `editor/treesit/point-forward`
    /// asks about module `editor/treesit`, which no project provides, and is left bare.
    /// An already-rooted `bedit/commands/cmd-open` asks about `bedit/commands` — also not
    /// in the short-name set — so rooting is idempotent. `None` = nothing to root.
    pub(crate) fn root_qualified_ref(&self, sym: Symbol) -> Option<Symbol> {
        if let Some(&cached) = self.rooted_ref_ic.borrow().get(&sym) {
            return cached;
        }
        let answer = self.root_qualified_ref_uncached(sym);
        self.rooted_ref_ic.borrow_mut().insert(sym, answer);
        answer
    }

    /// [`root_qualified_ref`](Self::root_qualified_ref) without the memo — the rule itself.
    fn root_qualified_ref_uncached(&self, sym: Symbol) -> Option<Symbol> {
        let prefix = self.package_prefix()?;
        let name = crate::core::value::symbol_name_ref(sym);
        let split = name.rfind('/')?;
        let module = crate::core::value::intern(&name[..split]);
        if !self
            .cold()
            .is_some_and(|c| c.package_modules.contains(&module))
        {
            return None;
        }
        Some(crate::core::value::intern(&format!(
            "{}/{}",
            crate::core::value::symbol_name_ref(prefix),
            name
        )))
    }

    /// Record the bare names the current-namespace file will define, so the
    /// resolver can qualify forward references. Returns the prior set so the
    /// caller can restore it (loads nest).
    pub fn set_ns_known_names(&mut self, names: HashSet<Symbol>) -> HashSet<Symbol> {
        std::mem::replace(&mut self.cold_mut().ns_known_names, names)
    }

    /// Is `sym` (a bare name) known to be defined in the current namespace's file?
    pub fn ns_knows_name(&self, sym: Symbol) -> bool {
        self.cold().is_some_and(|c| c.ns_known_names.contains(&sym))
    }

    /// Compile the next form(s) as a namespace's **own** code with no whole-file
    /// pre-scan available — a runtime `eval`. A file loader scans every form's def
    /// head up front, so a forward reference inside a file has positive evidence
    /// (`ns_known_names`) and qualifies; `eval` sees one form at a time and cannot,
    /// so a reference to a name a *later* `eval` will define is left bare and then
    /// misses the module-qualified global (KI-24). With this set, the resolver's
    /// last resort flips: a bare name that is bound at root/prelude still falls
    /// through (so `+`/`map` keep working), but one bound *nowhere* is taken to be
    /// this namespace's, matching what the file pre-scan would have concluded.
    /// Returns the prior value so the caller can restore it (evals nest).
    pub fn set_ns_assume_own(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.cold_mut().ns_assume_own, on)
    }

    /// Should an otherwise-unresolvable bare name be taken as this namespace's own?
    pub fn ns_assume_own(&self) -> bool {
        self.cold().is_some_and(|c| c.ns_assume_own)
    }

    /// Record one more bare name as defined in the current namespace's file. Used
    /// by the resolver when it qualifies a `def` head whose name the up-front
    /// forward-ref scan missed — a name produced by a *macro* expansion (e.g.
    /// `defprocess` → `(def counter …)`), which `scan_def_names` can't see in the
    /// raw form. Registering it before the def's body is resolved lets self-
    /// references (the recursion in `counter`'s loop) qualify to the same name.
    pub fn add_ns_known_name(&mut self, sym: Symbol) {
        self.cold_mut().ns_known_names.insert(sym);
    }

    /// Replace the current file's `(:use …)` import table, returning the prior one
    /// so the caller can restore it (loads nest). Maps bare → qualified.
    pub fn set_imports(&mut self, imports: HashMap<Symbol, Symbol>) -> HashMap<Symbol, Symbol> {
        std::mem::replace(&mut self.cold_mut().imports, imports)
    }

    /// Add one imported binding (bare name → qualified global). Used by `%refer`.
    pub fn add_import(&mut self, bare: Symbol, qualified: Symbol) {
        self.cold_mut().imports.insert(bare, qualified);
    }

    /// The qualified global a bare name was `(:use …)`-imported to, if any.
    pub fn import_of(&self, bare: Symbol) -> Option<Symbol> {
        self.cold().and_then(|c| c.imports.get(&bare).copied())
    }

    /// Every `(bare, qualified)` import pair in the current file's table — for the
    /// LSP to offer imported names as bare completion candidates (ADR-065 §6).
    pub fn imported_pairs(&self) -> Vec<(Symbol, Symbol)> {
        self.cold()
            .map(|c| c.imports.iter().map(|(&b, &q)| (b, q)).collect())
            .unwrap_or_default()
    }

    // ===== Definition sites (cross-file xref; ADR-031) =========================

    /// If `form` is a top-level `def`/`defn`/`defmacro`, record its name's source
    /// location (the [`current_file`] + `pos`). Called by the file loaders on each
    /// *un-expanded* top-level form — before macroexpansion, so `defn`/`defmacro`
    /// (which lower to `def`) are still recognisable by their head and their span
    /// is intact. A no-op when no file is set (e.g. the REPL) or the form isn't a
    /// definition.
    ///
    /// A `(do …)` is descended into: a definer macro like `defrecord`/`defability`
    /// expands to a `do` wrapping several inner `def`/`defn`s (the constructor, its
    /// accessors, the ability's op dispatchers). The loaders call this on the
    /// *expanded* form too, so recording each inner def at the same call-site `pos`
    /// gives those macro-synthesized globals a def-site — otherwise cross-file
    /// goto-definition on a record constructor or ability op finds nothing (ADR-031).
    ///
    /// [`current_file`]: Self::current_file
    pub fn note_definition(&mut self, form: Value, pos: crate::error::Pos) {
        let Some(file) = self.cold().and_then(|c| c.current_file.clone()) else {
            return;
        };
        self.note_definition_with_file(form, &file, pos);
    }

    /// Record `form`'s def-site under `file`, descending into a `(do …)`. Split from
    /// [`note_definition`] so the `do` recursion resolves `current_file` just once.
    fn note_definition_with_file(&mut self, form: Value, file: &str, pos: crate::error::Pos) {
        if let Some(name) = self.def_form_name(form) {
            self.runtime.def_sites_write().insert(
                name,
                SourceLoc {
                    file: file.to_string(),
                    pos,
                },
            );
            return;
        }
        // Not a definer itself — if it's a `(do child…)`, record each child.
        let ValueRef::Pair(p) = form.unpack() else {
            return;
        };
        let ValueRef::Sym(head) = self.car(p).unpack() else {
            return;
        };
        if !crate::core::value::symbol_is(head, kw::DO) {
            return;
        }
        let mut rest = self.cdr(p);
        while let ValueRef::Pair(cell) = rest.unpack() {
            self.note_definition_with_file(self.car(cell), file, pos);
            rest = self.cdr(cell);
        }
    }

    /// The name a top-level `def`/`defn`/`defmacro` form binds, reading the head
    /// and first argument from the *un-expanded* form. `None` for anything else
    /// (including `(def (pattern) …)`, which has no plain name — deferred).
    fn def_form_name(&self, form: Value) -> Option<Symbol> {
        let ValueRef::Pair(p) = form.unpack() else {
            return None;
        };
        let ValueRef::Sym(head) = self.car(p).unpack() else {
            return None;
        };
        if !(crate::core::value::symbol_is(head, kw::DEF)
            || crate::core::value::symbol_is(head, kw::DEF_PRIVATE)
            || crate::core::value::symbol_is(head, kw::DEFN)
            || crate::core::value::symbol_is(head, kw::DEFN_PRIVATE)
            || crate::core::value::symbol_is(head, kw::DEFMACRO))
        {
            return None;
        }
        let ValueRef::Pair(rest) = self.cdr(p).unpack() else {
            return None;
        };
        match self.car(rest).unpack() {
            // Qualify the recorded name to the current namespace (ADR-065) so the
            // def-site key matches the global the resolver will actually define
            // (`foo/name`); a no-op at root or for an already-qualified name.
            ValueRef::Sym(name) => Some(match self.compile_ns() {
                Some(ns) => {
                    crate::eval::macros::qualify_name(&crate::core::value::symbol_name(ns), name)
                }
                None => name,
            }),
            _ => None,
        }
    }

    /// Where `name`'s global definition was loaded from, if recorded. Backs
    /// `(source-location 'name)`. The runtime table (user/project `def`s) takes
    /// precedence over the immutable prelude table, so redefining a prelude name
    /// reports the user's site, not the standard library's.
    pub fn def_site(&self, name: Symbol) -> Option<SourceLoc> {
        self.runtime
            .def_sites_read()
            .get(&name)
            .cloned()
            .or_else(|| self.prelude.def_sites.get(&name).cloned())
    }

    /// Is the global `sym` module-private (ADR-146)? The single predicate every
    /// semantic privacy check consults (see [`RuntimeCode::private`]). Since step 2
    /// moved the marker off the name onto the def form, a private is spelled exactly
    /// like a public, so there is no name-shaped fast-negative to take first: every
    /// query is the recorded-set lookup. That is O(1) regardless of how many privates
    /// exist, and the callers pre-filter (intra-module refs and granted modules never
    /// reach here), which is why dropping the old `--` fast path measured within noise.
    pub fn is_private(&self, sym: Symbol) -> bool {
        self.runtime.is_private_recorded(sym)
    }

    /// Record the qualified global `sym` as module-private (ADR-146). The public
    /// face of [`RuntimeCode::mark_private`], called by the `%mark-private`
    /// primitive that `defn-`/`def-` emit. Privacy is now a property the def form
    /// declares (recorded here), not one derived from the name.
    pub fn mark_private(&self, sym: Symbol) {
        self.runtime.mark_private(sym);
    }

    /// A snapshot of this runtime's recorded module-private names. Used once, at
    /// prelude-build time, to capture the privates `%mark-private` recorded in the
    /// builder heap so they can seed each live runtime (the prelude is inserted, not
    /// re-evaluated — see [`RuntimeCode::seeded`]).
    pub fn private_names_snapshot(&self) -> Vec<Symbol> {
        self.runtime
            .private
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect()
    }

    // ===== Allocation — LOCAL slab =============================================
    //
    // Every allocator bump-appends to its LOCAL slab (the [`alloc_slot!`]
    // macro is that shape in one place; `alloc_string` / `new_env` stay
    // hand-written for their extra bookkeeping). Slots are never reused in
    // place; the slab's `len()` is kept bounded by the copying collector
    // relocating survivors into fresh slabs and dropping the rest.

    pub fn alloc_pair(&mut self, head: Value, tail: Value) -> Value {
        let idx = alloc_slot!(self, pairs, (head, tail));
        Value::pair(PairId::local_gen(idx, self.local_epoch))
    }

    pub fn alloc_vector(&mut self, items: Vec<Value>) -> Value {
        let idx = alloc_slot!(self, vectors, VecStore::from_vec(items));
        Value::vector(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a 2-element vector directly from its elements — a bump-push of an
    /// inline [`VecStore`] with **no temporary `Vec`** (hence no `malloc`). The
    /// JIT's `MakeVector(2)` runtime helper (`brood_rt_make_vector2`) uses this
    /// so the overwhelmingly common 2-tuple allocation (e.g. every `bintree`
    /// node) is as cheap as a `cons`. Larger literals still go through
    /// [`alloc_vector`].
    pub fn alloc_vector2(&mut self, a: Value, b: Value) -> Value {
        let idx = alloc_slot!(
            self,
            vectors,
            VecStore::Inline {
                len: 2,
                items: [a, b],
            }
        );
        Value::vector(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a lazy integer range `lo..hi` by `step`. Returns `Nil` when the
    /// range is empty (so a `Value::Range` always has ≥1 element), otherwise a
    /// `Value::Range` backed by a 3-element `[lo hi step]` vector. `step` must be
    /// non-zero (the caller — `%range` — enforces it).
    pub fn alloc_range(&mut self, lo: i64, hi: i64, step: i64) -> Value {
        let empty = if step > 0 { lo >= hi } else { hi >= lo };
        if empty {
            return Value::nil();
        }
        let idx = alloc_slot!(
            self,
            vectors,
            VecStore::from_vec(vec![Value::int(lo), Value::int(hi), Value::int(step)])
        );
        Value::range(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a lazy **seq-view** backed by a 2-element `[source xform]`
    /// vector. `source` is the underlying collection, `xform` a transducer
    /// composing every pending `map`/`filter`/`keep`/`remove` stage. Rides the
    /// vector slab exactly like [`Heap::alloc_range`], but its backing holds heap
    /// values (not just ints), so GC promote/flush/verify recurse into them.
    pub fn alloc_seqview(&mut self, source: Value, xform: Value) -> Value {
        let idx = alloc_slot!(self, vectors, VecStore::from_vec(vec![source, xform]));
        Value::seqview(VecId::local_gen(idx, self.local_epoch))
    }

    /// The `(source, xform)` of a seq-view handle's backing `[source xform]`
    /// vector.
    pub fn seqview_parts(&self, id: VecId) -> (Value, Value) {
        let v = self.vector(id);
        (v[0], v[1])
    }

    /// The `(lo, hi, step)` of a range handle's backing `[lo hi step]` vector.
    pub fn range_parts(&self, id: VecId) -> (i64, i64, i64) {
        let v = self.vector(id);
        let int = |x: Value| x.as_int().unwrap_or(0);
        (int(v[0]), int(v[1]), int(v[2]))
    }

    /// The number of elements a range yields. O(1).
    pub fn range_len(&self, id: VecId) -> i64 {
        let (lo, hi, step) = self.range_parts(id);
        // step is non-zero and the range is non-empty by construction. Compute in
        // i128: a wide range (e.g. i64::MIN..i64::MAX) overflows an i64 span even
        // though its element count is meaningful. Saturate on the way back — a range
        // longer than i64::MAX can't be materialised anyway.
        let (lo, hi, step) = (lo as i128, hi as i128, step as i128);
        let span = if step > 0 { hi - lo } else { lo - hi };
        let mag = step.abs();
        (((span + mag - 1) / mag).min(i64::MAX as i128)) as i64
    }

    /// Materialise a range's elements into a `Vec<Value>` of `Int`s — the slow
    /// path behind realising a range to a list / vector.
    pub fn range_to_vec(&self, id: VecId) -> Vec<Value> {
        let (lo, hi, step) = self.range_parts(id);
        let mut out = Vec::with_capacity(self.range_len(id).max(0) as usize);
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            out.push(Value::int(i));
            // The final step near i64::MIN/MAX would overflow; the loop is done anyway.
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
        }
        out
    }

    // ----- promotion: copy code from LOCAL into the shared RUNTIME region -----

    /// Deep-copy a value's reachable structure from the local heap into the
    /// shared RUNTIME region, returning a handle valid in every inner process.
    /// `def` of a global runs this so the bound code/data is shareable;
    /// `spawn` runs it on the target function. Atoms and already-shared values
    /// (PRELUDE/RUNTIME) are returned unchanged — no copy.
    ///
    /// Appends only (never mutates existing shared code), so a redefinition adds
    /// a new version while in-flight calls keep running the old one.
    pub fn promote(&self, v: Value) -> Value {
        // Hold the promote⇄age read lock for the whole (recursive) promotion so a
        // concurrent `age_runtime` on another process can't flip `current_gen` between
        // this promote's slot reservation and its fill (ADR-091). Uncontended off the
        // multi-generation path. Acquired once at the top — the recursion must not
        // re-acquire (std `RwLock` read isn't reentrant against a queued writer).
        let _promote_guard = self
            .runtime
            .promote_lock
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut fwd = PromoteForward::default();
        self.promote_in(v, &mut fwd)
    }

    /// The recursive core of [`promote`](Self::promote), threading a forwarding
    /// table so a *cyclic* graph (a closure capturing its own binding scope)
    /// terminates: closures and envs reserve their RUNTIME slot and register it in
    /// `fwd` *before* recursing, so the back-edge resolves to the reserved handle
    /// instead of recursing forever. The table also collapses shared (DAG)
    /// closures/envs to one RUNTIME copy. Pairs/vectors/maps/strings/ropes are
    /// acyclic by construction (immutable, built bottom-up), so they aren't
    /// forwarded — they just recurse through `fwd` to reach any closures inside.
    fn promote_in(&self, v: Value, fwd: &mut PromoteForward) -> Value {
        // Deep-car-nesting guard — see `WALKER_RED_ZONE`.
        stacker::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
            self.promote_in_grown(v, fwd)
        })
    }

    fn promote_in_grown(&self, v: Value, fwd: &mut PromoteForward) -> Value {
        match v.unpack() {
            ValueRef::Str(id) if id.region() == LOCAL => {
                let s = self.string(id).to_string();
                Value::str_(self.runtime.push_str(s))
            }
            ValueRef::BigInt(id) if id.region() == LOCAL => {
                // A leaf: clone the value into the shared region (no children).
                let n = self.bigint(id).clone();
                Value::bigint(self.runtime.push_bigint(n))
            }
            ValueRef::Decimal(id) if id.region() == LOCAL => {
                // A leaf: clone the value into the shared region (no children).
                let n = self.decimal(id).clone();
                Value::decimal(self.runtime.push_decimal(n))
            }
            ValueRef::Ratio(id) if id.region() == LOCAL => {
                // A leaf: clone the value into the shared region (no children).
                let n = self.ratio(id).clone();
                Value::ratio(self.runtime.push_ratio(n))
            }
            ValueRef::Bytes(id) if id.region() == LOCAL => {
                // A leaf: share the Arc<SharedBlob> into the shared region byte-clean —
                // never through the UTF-8 string path. Just an Arc bump.
                let b = Arc::clone(&self.bytes(id));
                Value::bytes(self.runtime.push_bytes(b))
            }
            ValueRef::Rope(id) if id.region() == LOCAL => {
                // Cheap `Arc`-node clone into the shared region; the rope is
                // immutable, so sibling processes read it concurrently.
                let r = self.rope(id).clone();
                Value::rope(self.runtime.push_rope(r))
            }
            ValueRef::Pair(id) if id.region() == LOCAL => self.promote_list(id, fwd),
            ValueRef::Vector(id) if id.region() == LOCAL => {
                let items: Vec<Value> = self
                    .vector(id)
                    .to_vec()
                    .into_iter()
                    .map(|x| self.promote_in(x, fwd))
                    .collect();
                Value::vector(self.runtime.push_vec(VecStore::from_vec(items)))
            }
            // A range's backing `[lo hi step]` vector holds only ints (atoms) —
            // copy it across and keep the `Range` wrapper.
            ValueRef::Range(id) if id.region() == LOCAL => {
                let items = self.vector(id).to_vec();
                Value::range(self.runtime.push_vec(VecStore::from_vec(items)))
            }
            // A seq-view's backing `[source xform]` holds heap values (a
            // collection and a transducer closure), so promote each across like a
            // vector and keep the `SeqView` wrapper.
            ValueRef::SeqView(id) if id.region() == LOCAL => {
                let items: Vec<Value> = self
                    .vector(id)
                    .to_vec()
                    .into_iter()
                    .map(|x| self.promote_in(x, fwd))
                    .collect();
                Value::seqview(self.runtime.push_vec(VecStore::from_vec(items)))
            }
            ValueRef::Map(id) if id.region() == LOCAL => {
                // Recursively promote the trie depth-first. Children are
                // promoted before their parent so the parent's `children`
                // array can be wired to the freshly-allocated RUNTIME
                // sub-node handles.
                Value::map(self.promote_map_node(id, fwd))
            }
            // A set shares the CHAMP storage — promote its trie exactly like a map
            // and keep the `Set` wrapper (mirrors the `SeqView` case above).
            ValueRef::Set(id) if id.region() == LOCAL => Value::set(self.promote_map_node(id, fwd)),
            ValueRef::Fn(id) if id.region() == LOCAL => Value::func(self.promote_closure(id, fwd)),
            ValueRef::Macro(id) if id.region() == LOCAL => {
                Value::macro_(self.promote_closure(id, fwd))
            }
            // Atoms, and values already in PRELUDE/RUNTIME, need no copy.
            _ => v,
        }
    }

    /// Promote a local cons-chain. Walks the `cdr` spine *iteratively* so a long
    /// list doesn't recurse its length deep (which overflowed the native stack);
    /// recursion is bounded by element nesting via `promote_in` on each `car`.
    /// Stops at the first already-shared cell or non-pair tail, preserving both
    /// improper (dotted) lists and existing structure sharing.
    fn promote_list(&self, first: PairId, fwd: &mut PromoteForward) -> Value {
        // Keep each source LOCAL pair id alongside its promoted head, so the new
        // RUNTIME pair can inherit the source position (`form_pos`) the reader stamped
        // on it — without this, `(form-pos …)` on a frozen body returns nil and a
        // position is lost across a cross-node send.
        let mut nodes: Vec<(PairId, Value)> = Vec::new();
        let mut cur = Value::pair(first);
        let tail = loop {
            match cur.unpack() {
                ValueRef::Pair(id) if id.region() == LOCAL => {
                    let (head, next) = self.pair(id);
                    let promoted_head = self.promote_in(head, fwd);
                    nodes.push((id, promoted_head));
                    cur = next;
                }
                other => break self.promote_in(other, fwd),
            }
        };
        let mut acc = tail;
        for (src, head) in nodes.into_iter().rev() {
            let idx = self.runtime.cur_code().pairs.push((head, acc));
            if let Some((pos, file)) = self
                .cold()
                .and_then(|c| c.form_pos.get(&form_pos_key(src)).cloned())
            {
                self.runtime.set_position(idx, pos, file);
            }
            acc = Value::pair(PairId::runtime_gen(idx, self.runtime.cur_gen()));
        }
        acc
    }

    /// Promote a LOCAL CHAMP trie into the shared RUNTIME region. Walks
    /// depth-first: child sub-nodes are promoted before their parent so
    /// the parent's `children` array references the new RUNTIME handles.
    /// Every `(k, v)` entry is promoted recursively (matches `promote_in`
    /// on vectors / lists). The result is a brand-new trie in RUNTIME;
    /// the original LOCAL trie is left untouched (it'll be GC'd when its
    /// last reference goes).
    fn promote_map_node(&self, id: MapId, fwd: &mut PromoteForward) -> MapId {
        let node = self.map_node(id);
        // Promote children first (bottom-up) so the new RUNTIME node can
        // be built with the new child handles in one push.
        let new_children: SmallVec<[MapId; 4]> = node
            .children
            .iter()
            .map(|&c| match c.region() {
                LOCAL => self.promote_map_node(c, fwd),
                _ => c, // already shared
            })
            .collect();
        let new_data: SmallVec<[(Value, Value); 4]> = node
            .data
            .iter()
            .map(|&(k, v)| (self.promote_in(k, fwd), self.promote_in(v, fwd)))
            .collect();
        let promoted = MapNode {
            size: node.size,
            data_map: node.data_map,
            node_map: node.node_map,
            is_collision: node.is_collision,
            data: new_data,
            children: new_children,
        };
        MapId::runtime_gen(
            self.runtime.cur_code().maps.push(promoted),
            self.runtime.cur_gen(),
        )
    }

    fn promote_closure(&self, id: ClosureId, fwd: &mut PromoteForward) -> ClosureId {
        // Already promoted on this walk? Return the shared handle (cycle break +
        // DAG-sharing collapse). Keyed on LOCAL slot index.
        let key = id.index() as u32;
        if let Some(&existing) = fwd.closures.get(&key) {
            return existing;
        }
        // Reserve the RUNTIME slot *first* and register it, so a reference back to
        // this closure reached while promoting its captured scope resolves here
        // rather than recursing forever (e.g. `(let (g (fn () g)) g)`).
        // `BROOD_TRACE_PROMOTE=1` — name every closure entering the append-only RUNTIME
        // region, with the Rust frames that put it there. This is the tool that finally
        // pinned KI-22's sibling (thread 6): 1382 of 1389 promotions in a supervisor
        // workload came from one site, `spawn_impl <- spawn_link`. Elimination bisecting had
        // failed on it for hours; one run of this answered it. Gated, so it costs a single
        // `var_os` on the promote path when off.
        if std::env::var_os("BROOD_TRACE_PROMOTE").is_some() {
            let nm = self
                .closure(id)
                .name
                .map(crate::core::value::symbol_name)
                .unwrap_or_else(|| "<anon>".to_string());
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let frame = bt
                .lines()
                .filter(|l| l.contains("brood::"))
                .map(|l| l.trim())
                .filter(|l| !l.contains("promote"))
                .take(3)
                .collect::<Vec<_>>()
                .join(" <- ");
            // The capture state is the diagnostic that matters: a closure promoted with
            // `captures-frame` is one the const-closure cache could not dedupe, so it is
            // being appended per activation rather than once.
            let cap = if self.closure(id).env.is_some() {
                "captures-frame"
            } else {
                "capture-free"
            };
            eprintln!("[promote] closure {} [{}] :: {}", nm, cap, frame);
        }
        let new_idx = self.runtime.cur_code().closures.push(OnceLock::new());
        // The RUNTIME closure count just grew — arm the eval safepoint's `rt_gc_due`
        // probe (see `rt_dirty`). This is the one place closures enter the region.
        self.runtime.rt_dirty.store(true, Ordering::Relaxed);
        let runtime_id = ClosureId::runtime_gen(new_idx, self.runtime.cur_gen());
        fwd.closures.insert(key, runtime_id);
        let cl = self.closure(id).clone();
        // Promote every arm's body forms and `&optional` defaults into the shared
        // region (param symbols and `&` rest are interned/copy, so they ride along).
        let arms = cl
            .arms
            .iter()
            .map(|arm| ClosureArm {
                params: arm.params.clone(),
                optionals: arm
                    .optionals
                    .iter()
                    .map(|&(s, d)| (s, self.promote_in(d, fwd)))
                    .collect(),
                rest: arm.rest,
                body: arm.body.iter().map(|&f| self.promote_in(f, fwd)).collect(),
                // The forwarding head is an interned symbol and the map is plain
                // indices, so the analysis is region-independent — copy it verbatim.
                passthrough: arm.passthrough.clone(),
            })
            .collect();
        // A top-level closure captures the global env (`None`) and is fully
        // shareable as-is. A closure that captured a *local* scope has its scope
        // promoted too, so it resolves its free variables in any process.
        let env = cl.env.map(|e| self.promote_env(e, fwd));
        let promoted = Closure {
            name: cl.name,
            arms,
            doc: cl.doc,
            env,
        };
        // Fill the reserved slot exactly once. The handle isn't published (bound
        // in a global / shipped to a process) until `promote` returns, so nothing
        // can observe the cell before this set.
        self.runtime
            .cur_code()
            .closures
            .get(new_idx)
            .expect("reserved closure slot")
            .set(promoted)
            .ok()
            .expect("promote: closure slot filled exactly once");
        runtime_id
    }

    /// Deep-copy an environment frame chain from LOCAL into the shared RUNTIME
    /// region, promoting each bound value. Stops at the global scope (the shared
    /// sentinel). Already-shared (RUNTIME) frames are returned unchanged. Reserves
    /// its slot before recursing (same cycle break as [`promote_closure`]).
    fn promote_env(&self, env: EnvId, fwd: &mut PromoteForward) -> EnvId {
        if env == EnvId::GLOBAL || env.region() == RUNTIME {
            return env;
        }
        let key = env.index() as u32;
        if let Some(&existing) = fwd.envs.get(&key) {
            return existing;
        }
        let new_idx = self.runtime.cur_code().envs.push(OnceLock::new());
        let runtime_id = EnvId::runtime_gen(new_idx, self.runtime.cur_gen());
        fwd.envs.insert(key, runtime_id);
        // Snapshot the frame, then promote its parent and values (no borrow held).
        let (parent, bindings): (Option<EnvId>, Vec<(Symbol, Value)>) = {
            let frame = self.env_frame(env);
            (
                frame.parent,
                frame.vars.iter().map(|&(s, v)| (s, v)).collect(),
            )
        };
        let parent = parent.map(|p| self.promote_env(p, fwd));
        let vars = bindings
            .into_iter()
            .map(|(s, v)| (s, self.promote_in(v, fwd)))
            .collect();
        self.runtime
            .cur_code()
            .envs
            .get(new_idx)
            .expect("reserved env slot")
            .set(EnvFrame { vars, parent })
            .ok()
            .expect("promote: env slot filled exactly once");
        runtime_id
    }

    // ----- access (dispatch on the handle's region) -----

    /// A heap epoch counter truncated to the handle GEN field's width. A
    /// handle's `generation()` is its mint-time epoch masked to `GEN_MASK`,
    /// while the heap's `local_epoch`/`old_epoch` counters are full u32s — so
    /// every stale-handle comparison must truncate the expected side
    /// identically, or after 2^29 collections of one heap every *valid*
    /// handle would "mismatch" (kernel audit; astronomically rare, but the
    /// tripwire must not be the thing that cries wolf). The one definition
    /// shared by [`check_epoch_aged`](Self::check_epoch_aged) and the
    /// `BROOD_GC_VERIFY` walker, so the two detectors can't drift.
    // Available in release too: `dbg_value_stale` (used by the runtime BROOD_JIT_VERIFY
    // staged-stale scan) calls it. Pure arithmetic — zero cost unless called.
    #[allow(dead_code)]
    fn epoch_in_gen_width(epoch: u32) -> u32 {
        epoch & (crate::core::value::GEN_MASK as u32)
    }

    /// Generation-aware epoch tripwire. Young (`is_old == false`) handles are
    /// checked against the nursery epoch (bumped by every collection); old handles
    /// against the old-generation epoch (bumped only by a major collection, since a
    /// minor leaves old objects in place). Both sides compare truncated — see
    /// [`epoch_in_gen_width`]. A mismatch means a handle was held
    /// across a collection that moved its space without being re-rooted. Only the
    /// debug-gated accessors call it, so it's `cfg(debug_assertions)` too (no
    /// release dead-code).
    #[cfg(debug_assertions)]
    fn check_epoch_aged(&self, is_old: bool, gen: u32, index: usize, what: &str, raw: u64) {
        let (expected, space) = if is_old {
            (self.old_epoch, "OLD")
        } else {
            (self.local_epoch, "nursery")
        };
        // Compare in the handle's truncated GEN width — see `epoch_in_gen_width`.
        let expected = Self::epoch_in_gen_width(expected);
        debug_assert!(
            gen == expected,
            "use-after-GC: {} handle ({} slot {}) is from epoch {}, but that generation is \
             now epoch {} — a handle held across a collection without being re-rooted \
             (handle {:#x}). [current JIT arm: '{}']",
            what,
            space,
            index,
            gen,
            expected,
            raw,
            crate::core::value::symbol_name_opt(self.jit_dbg_fn).unwrap_or("<none/computed>"),
        );
    }

    /// Is `v` a LOCAL handle whose generation epoch no longer matches the live epoch
    /// (stale across a collection)? Non-panicking sibling of the per-deref tripwire, for
    /// scanning staged call args. Returns `Some((kind, handle_gen, live_gen))` if stale.
    /// Available in release (gated only by the caller) so the runtime `BROOD_JIT_VERIFY`
    /// scan can run without a debug-assertions build.
    pub fn dbg_value_stale(&self, v: Value) -> Option<(&'static str, u32, u32)> {
        let (name, region, is_old, gen) = match v {
            Value::Pair(id) => ("pair", id.region(), id.is_old(), id.generation()),
            Value::Vector(id) => ("vector", id.region(), id.is_old(), id.generation()),
            Value::Map(id) => ("map", id.region(), id.is_old(), id.generation()),
            Value::Set(id) => ("set", id.region(), id.is_old(), id.generation()),
            Value::Str(id) => ("string", id.region(), id.is_old(), id.generation()),
            Value::Rope(id) => ("rope", id.region(), id.is_old(), id.generation()),
            _ => return None,
        };
        if region != LOCAL {
            return None;
        }
        let expected = Self::epoch_in_gen_width(if is_old {
            self.old_epoch
        } else {
            self.local_epoch
        });
        if gen != expected {
            Some((name, gen, expected))
        } else {
            None
        }
    }

    /// Is `v` a handle whose slab index is **out of bounds** for its region's slab — i.e.
    /// garbage read from a freed/wrong location (a recycled roots buffer, an unspilled
    /// register that went stale across a collection)? Catches bug-#2 garbage that
    /// `dbg_value_stale` misses (the garbage's region/epoch bits don't read as a clean
    /// LOCAL-stale handle). Returns `Some((kind, index, slab_len))` if OOB.
    pub fn dbg_value_oob(&self, v: Value) -> Option<(&'static str, usize, usize)> {
        macro_rules! check {
            ($id:expr, $name:expr, $field:ident) => {{
                let id = $id;
                let idx = id.index();
                // Only LOCAL (nursery/old) — the bug-#2 garbage is young/local; PRELUDE/RUNTIME
                // are stable boxcar slabs (different len API), skip.
                let len = match id.region() {
                    LOCAL if id.is_old() => self.old_opt().map_or(0, |o| o.$field.len()),
                    LOCAL => self.local.$field.len(),
                    _ => return None,
                };
                if idx >= len {
                    return Some(($name, idx, len));
                }
            }};
        }
        match v {
            Value::Pair(id) => check!(id, "pair", pairs),
            Value::Vector(id) | Value::Range(id) => check!(id, "vector", vectors),
            Value::Map(id) | Value::Set(id) => check!(id, "map", maps),
            Value::Str(id) => check!(id, "string", strings),
            _ => {}
        }
        None
    }

    // ===== Accessors — read LOCAL/PRELUDE/RUNTIME values =======================

    /// Pin RUNTIME generation `g`'s `Arc<CodeSlabs>` for a read, via the per-process
    /// version-gated cache ([`gen_cache`](Self::gen_cache)). Returns a cheap `Arc` clone
    /// (one refcount bump) when the generation's identity is unchanged since this process
    /// last read it, and `load_full`s only on a real replacement — a Stage-4 free or a
    /// compaction store, both rare and both bumping [`RuntimeCode::gen_version`]. This
    /// replaces the per-deref `ArcSwap::load` guard whose hybrid-strategy cost dominated
    /// global-data-heavy hot loops (a `def`'d matrix element read in `matmul` derefs a
    /// RUNTIME handle ~16 M times). Soundness: the returned `Arc` pins the slab exactly as
    /// the old guard did, so a concurrent free can't drop it mid-read; and reading a stale
    /// cached `Arc` is impossible to observe wrongly — a generation is freed only once every
    /// process (this one included) has reported clean of it (ADR-091), so this process holds
    /// no live handle into a generation whose `Arc` it might still have cached.
    /// Run `f` against RUNTIME generation `g`'s slabs **without bumping its refcount**.
    ///
    /// [`code_gen_pinned`](Self::code_gen_pinned) returns an owned `Arc` so a caller can hold
    /// the generation alive across a borrow — necessary when handing out a reference, but pure
    /// overhead for a read that copies its value straight out. The clone and its matching drop
    /// are two atomic RMWs on a path that runs once per element of every `def`'d structure,
    /// and they dominated it: measured 2026-07-28, `first` on a RUNTIME pair cost **77 ns**
    /// against **1 ns** for the identical code on a LOCAL one — a 70x cliff that every global
    /// data structure fell off (`sort` walks a 375k-element `def`'d list; `matmul` derefs a
    /// `def`'d matrix ~16 M times).
    ///
    /// Soundness is unchanged: the cache still owns the `Arc`, so the generation cannot be
    /// freed while `f` borrows it. `f` must not itself take a *mutable* borrow of this same
    /// generation's cache slot — every caller is a trivial copy-out read, which cannot.
    #[inline]
    fn with_code_gen<R>(&self, g: usize, f: impl FnOnce(&CodeSlabs) -> R) -> R {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.gen_cache_ver[g].get() != ver {
            *self.gen_cache[g].borrow_mut() = Some(self.runtime.gens[g].load_full());
            self.gen_cache_ver[g].set(ver);
        }
        let cached = self.gen_cache[g].borrow();
        f(cached
            .as_ref()
            .expect("gen cache populated on the version miss above"))
    }

    #[inline]
    fn code_gen_pinned(&self, g: usize) -> Arc<CodeSlabs> {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.gen_cache_ver[g].get() != ver {
            // First read, or generation `g`'s `Arc` was replaced — reload and re-stamp.
            *self.gen_cache[g].borrow_mut() = Some(self.runtime.gens[g].load_full());
            self.gen_cache_ver[g].set(ver);
        }
        Arc::clone(
            self.gen_cache[g]
                .borrow()
                .as_ref()
                .expect("gen cache populated on the version miss above"),
        )
    }

    /// Look up the parsed [`ClosureTemplate`] for a `MakeClosure` site's `fn_rest`
    /// handle, gen-synced exactly like [`code_gen_pinned`](Self::code_gen_pinned): a
    /// `gen_version` bump (the only event that relocates the RUNTIME AST handles the
    /// arms carry) clears the whole cache first, so any hit is current-generation.
    /// `None` on a miss — the caller parses once and calls [`store_closure_template`].
    pub(crate) fn lookup_closure_template(&self, key: PairId) -> Option<Arc<ClosureTemplate>> {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.closure_tpl_ver.get() != ver {
            self.closure_tpl_cache.borrow_mut().clear();
            self.closure_tpl_ver.set(ver);
            return None;
        }
        self.closure_tpl_cache.borrow().get(&key).cloned()
    }

    /// Memoise a freshly-parsed [`ClosureTemplate`] under its `fn_rest` key. Call only
    /// right after a [`lookup_closure_template`] miss (which synced the version this
    /// creation), so the insert lands against the current generation.
    pub(crate) fn store_closure_template(&self, key: PairId, tpl: Arc<ClosureTemplate>) {
        self.closure_tpl_cache.borrow_mut().insert(key, tpl);
    }

    /// Look up the memoised **promoted RUNTIME closure** for a capture-free `(fn …)`
    /// literal's `fn_rest` key (see [`closure_const_cache`](Self::closure_const_cache)),
    /// gen-synced like [`lookup_closure_template`]: a `gen_version` bump clears the map, so
    /// any hit is a current-generation handle. `None` on a miss — the caller builds +
    /// promotes once and calls [`store_const_closure`].
    pub(crate) fn lookup_const_closure(&self, key: PairId) -> Option<Value> {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.closure_const_ver.get() != ver {
            self.closure_const_cache.borrow_mut().clear();
            self.closure_const_ver.set(ver);
            return None;
        }
        self.closure_const_cache.borrow().get(&key).copied()
    }

    /// Memoise a capture-free closure's promoted RUNTIME handle under its `fn_rest` key.
    /// Call only right after a [`lookup_const_closure`] miss (which synced the version), so
    /// the insert lands against the current generation.
    pub(crate) fn store_const_closure(&self, key: PairId, closure: Value) {
        self.closure_const_cache.borrow_mut().insert(key, closure);
    }

    pub fn pair(&self, id: PairId) -> (Value, Value) {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "pair");
                self.old().pairs[id.index()]
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "pair");
                self.local.pairs[id.index()]
            }
            PRELUDE => self.prelude.slabs.pairs[id.index()],
            // Copy-out read: borrow the generation rather than pinning it, so a pair
            // deref costs no atomic refcount traffic (see `with_code_gen`).
            RUNTIME => self.with_code_gen(id.code_gen(), |slabs| {
                *slabs.pairs.get(id.index()).expect("runtime pair handle")
            }),
            _ => unreachable!("invalid handle region"),
        }
    }
    pub fn car(&self, id: PairId) -> Value {
        self.pair(id).0
    }
    pub fn cdr(&self, id: PairId) -> Value {
        self.pair(id).1
    }
    region_ref!(vector, VecId, vectors, [Value], "runtime vector handle");
    region_ref!(map_node, MapId, maps, MapNode, "runtime map node");

    /// Build a guarded [`SlabRef`] into RUNTIME generation `g` by projecting the
    /// generation's [`CodeSlabs`] to the `&T` a hand-written accessor wants. The
    /// [`Guard`] is moved into the `SlabRef`, keeping the generation's slab alive for
    /// the borrow's lifetime — so a concurrent Stage-4 free can't drop it mid-read
    /// (ADR-091). Mirrors the `RUNTIME` arm of [`region_ref!`] for the accessors that
    /// can't use the macro (`OnceLock`/`LocalString`/`Arc` projections).
    #[inline]
    fn rt_slab_ref<T: ?Sized>(
        &self,
        g: usize,
        project: impl FnOnce(&CodeSlabs) -> &T,
    ) -> SlabRef<'_, T> {
        // A generation can be **freed concurrently** by the multi-process collector while
        // other processes run (ADR-091), so the `SlabRef` must pin `gens[g]`'s `Arc<CodeSlabs>`
        // to defer the freed `Arc`'s drop until this borrow ends. The `Arc` comes from the
        // per-process version-gated cache ([`code_gen_pinned`]) — a cheap clone when the
        // generation is unchanged — not a fresh `ArcSwap::load` guard per deref.
        let pin = self.code_gen_pinned(g);
        let ptr = project(&pin) as *const T;
        // SAFETY: `ptr` points into `pin`'s `CodeSlabs` (stable `boxcar` address);
        // the `Arc` moved into the `SlabRef` keeps that slab alive for the borrow.
        unsafe { SlabRef::pinned(pin, ptr) }
    }

    /// Resolve a string handle to a `&str`. Hand-written (not via the
    /// `region_ref!` macro) because LOCAL slots are `LocalString` enum
    /// variants that need a match to extract their bytes, while PRELUDE and
    /// RUNTIME store plain `String` (PRELUDE is inline-extracted at freeze;
    /// RUNTIME is append-only via `boxcar::Vec<String>` for stable refs).
    /// The **char** length of string `id`, and whether it is pure ASCII — both O(1),
    /// read from the count cached at construction (see [`LocalString`]). The pair is
    /// returned together because every caller that converts a char index to a byte
    /// offset needs both, and resolving the slot twice would cost more than the work.
    pub fn str_metrics(&self, id: StrId) -> (usize, bool) {
        self.with_string_slot(id, |e| (e.char_len(), e.is_ascii()))
    }

    /// Byte offset of char `ci` in string `id`, clamped to the string's end — the
    /// conversion every char-indexed string builtin needs before it can touch the UTF-8
    /// bytes. O(1) for ASCII; for non-ASCII a lookup in the slot's sparse char→byte
    /// index plus a walk bounded by one stride (which is what keeps a scan carrying a
    /// rising index linear rather than quadratic — see [`LocalString`]).
    pub fn str_char_to_byte(&self, id: StrId, ci: usize) -> usize {
        self.with_string_slot(id, |e| e.char_to_byte(ci))
    }

    /// Char index of byte offset `b` in string `id` (`b` must be a char boundary) — the
    /// return direction: a byte-level `find`/`match_indices` result converted back to
    /// the char index the language speaks. Same complexities as
    /// [`str_char_to_byte`](Self::str_char_to_byte).
    pub fn str_byte_to_char(&self, id: StrId, b: usize) -> usize {
        self.with_string_slot(id, |e| e.byte_to_char(b))
    }

    /// The higher-layer table cached against string `id`, built by `build` on first use
    /// and shared thereafter (including with the slot's GC copies). The heap does not
    /// interpret it — see [`StrAux::scan`]; the caller downcasts to its own type. Callers
    /// that key a cache by string *value* belong here rather than in a map keyed by
    /// handle: a handle is only unique within a GC epoch, while this cell travels with
    /// the bytes it describes.
    pub fn str_scan_table(
        &self,
        id: StrId,
        build: impl FnOnce(&str) -> Arc<dyn std::any::Any + Send + Sync>,
    ) -> Arc<dyn std::any::Any + Send + Sync> {
        self.with_string_slot(id, |e| {
            Arc::clone(e.aux().scan.get_or_init(|| build(e.as_str())))
        })
    }

    /// Resolve a string handle to its slab entry and hand it to `f`. The
    /// region dispatch the string-metric accessors share; separate from
    /// [`string`](Self::string) because these need the `LocalString` itself (its cached
    /// count and char index), not just its bytes.
    fn with_string_slot<R>(&self, id: StrId, f: impl FnOnce(&LocalString) -> R) -> R {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "string");
                f(&self.old().strings[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "string");
                f(&self.local.strings[id.index()])
            }
            PRELUDE => f(&self.prelude.slabs.strings[id.index()]),
            RUNTIME => {
                let c = self
                    .runtime
                    .gens
                    .get(id.code_gen())
                    .expect("runtime string generation")
                    .load();
                f(c.strings.get(id.index()).expect("runtime string handle"))
            }
            _ => unreachable!("invalid handle region"),
        }
    }

    pub fn string(&self, id: StrId) -> SlabRef<'_, str> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "string");
                SlabRef::direct(self.old().strings[id.index()].as_str())
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "string");
                SlabRef::direct(self.local.strings[id.index()].as_str())
            }
            // PRELUDE's `Slabs::strings` is also `Vec<LocalString>` because
            // it shares the `Slabs` shape, but `freeze_as_shared_code`
            // inline-extracts any `Shared` entries — every prelude slot is
            // `Inline`. `as_str` works either way.
            PRELUDE => SlabRef::direct(self.prelude.slabs.strings[id.index()].as_str()),
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.strings
                    .get(id.index())
                    .expect("runtime string handle")
                    .as_str()
            }),
            _ => unreachable!("invalid handle region"),
        }
    }

    /// Resolve a closure handle to its `&Closure`. Hand-written (not via
    /// `region_ref!`) because the RUNTIME slab wraps each entry in a `OnceLock`
    /// (reserve-then-fill cycle break, see `CodeSlabs::closures`); the cell is
    /// always filled before its handle is published, so `get()` is infallible in
    /// practice.
    pub fn closure(&self, id: ClosureId) -> SlabRef<'_, Closure> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "closure");
                SlabRef::direct(&self.old().closures[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "closure");
                SlabRef::direct(&self.local.closures[id.index()])
            }
            PRELUDE => SlabRef::direct(&self.prelude.slabs.closures[id.index()]),
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.closures
                    .get(id.index())
                    .expect("runtime closure handle")
                    .get()
                    .expect("runtime closure read before promote filled its slot")
            }),
            _ => unreachable!("invalid handle region"),
        }
    }

    pub fn native(&self, id: NativeId) -> &NativeFn {
        match id.region() {
            LOCAL => &self.local.natives[id.index()],
            PRELUDE => &self.prelude.slabs.natives[id.index()],
            _ => unreachable!("natives live only in the local or prelude region"),
        }
    }

    /// Collect a proper list into a `Vec`. Errors on an improper (dotted) list.
    pub fn list_to_vec(&self, v: Value) -> Result<Vec<Value>, LispError> {
        let mut out = Vec::new();
        let mut cur = v;
        loop {
            match cur.unpack() {
                ValueRef::Nil => return Ok(out),
                ValueRef::Pair(p) => {
                    let (head, tail) = self.pair(p);
                    out.push(head);
                    cur = tail;
                }
                _ => return Err(LispError::type_err("improper list")),
            }
        }
    }

    /// Treat a list or vector as a sequence of items.
    pub fn seq_items(&self, v: Value) -> Result<Vec<Value>, LispError> {
        match v.unpack() {
            ValueRef::Nil => Ok(Vec::new()),
            ValueRef::Pair(_) => self.list_to_vec(v),
            ValueRef::Vector(id) => Ok(self.vector(id).to_vec()),
            ValueRef::Range(id) => Ok(self.range_to_vec(id)),
            // A set is a sequence of its elements — so `map`/`reduce`/`count`/`vec`/…
            // work on it (Clojure-like). Order is the CHAMP's deterministic-per-shape
            // order, matching how `#{…}` prints.
            ValueRef::Set(id) => Ok(self.set_elems(id)),
            _ => Err(LispError::type_err("expected a list or vector")),
        }
    }

    // ===== Environment chain ====================================================
    //
    // Real env frames are always LOCAL. The global scope is the sentinel
    // [`EnvId::GLOBAL`], which routes to the shared `runtime.globals` table; a
    // top-level frame's parent chain bottoms out there. (During prelude *build*
    // the global is instead a real local root frame with no parent.)

    fn env_frame(&self, env: EnvId) -> SlabRef<'_, EnvFrame> {
        // `EnvId::GLOBAL` is a sentinel (region bits `0b11`) — there is no
        // frame to return; the global scope routes through
        // `runtime.globals_read()` instead. Callers MUST short-circuit
        // GLOBAL before reaching here (every walker does — see `env_get`).
        // A clear assert when that invariant slips, rather
        // than the `_ => unreachable!()` arm catching it via the
        // undefined-region byte.
        assert!(
            env != EnvId::GLOBAL,
            "env_frame called with EnvId::GLOBAL — global scope has no frame; \
             use env_get / globals_read instead",
        );
        match env.region() {
            LOCAL if env.is_old() => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(true, env.generation(), env.index(), "env_frame", env.0);
                SlabRef::direct(&self.old().envs[env.index()])
            }
            LOCAL => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(false, env.generation(), env.index(), "env_frame", env.0);
                SlabRef::direct(&self.local.envs[env.index()])
            }
            RUNTIME => self.rt_slab_ref(env.code_gen(), |c| {
                c.envs
                    .get(env.index())
                    .expect("runtime env frame")
                    .get()
                    .expect("runtime env read before promote filled its slot")
            }),
            _ => unreachable!("env frames live only in the local or runtime region"),
        }
    }

    /// A captured frame's parent link and a borrow of its bindings — no copy.
    /// Used to *serialize* a closure's captured environment into a `Message`
    /// (cross-process / cross-node), mirroring what [`Self::promote_env`] reads
    /// to share it within a runtime. `EnvId::GLOBAL` has no frame (it routes to
    /// the shared global table), so the walk stops there — globals resolve on
    /// the receiver, never travel. The borrow is tied to `&self` (the LOCAL slab
    /// or the stable-ref RUNTIME boxcar), so callers walk a chain without cloning.
    pub fn env_frame_ref(&self, env: EnvId) -> (Option<EnvId>, SlabRef<'_, [(Symbol, Value)]>) {
        let frame = self.env_frame(env);
        let parent = frame.parent;
        (parent, frame.map(|f| f.vars.as_slice()))
    }

    /// The name `env`'s *immediate* frame binds to `val`, if any — used by the VM's
    /// self-call optimization to recognise a `letrec` self-recursive closure: its
    /// captured frame binds its own name to itself (the `MakeClosure` self-name
    /// `env_define`). Scans the frame only (not parents), newest binding first (so
    /// the self-binding, pushed last, wins). `None` when nothing in the frame is
    /// `val` — e.g. a global-capturing closure, which resolves its name via the
    /// global table rather than a captured binding.
    pub fn env_frame_self_name(&self, env: EnvId, id: ClosureId) -> Option<Symbol> {
        let (_, vars) = self.env_frame_ref(env);
        vars.iter()
            .rev()
            .find(|(_, v)| matches!(v.unpack(), ValueRef::Fn(fid) if fid == id))
            .map(|(s, _)| *s)
    }

    pub fn new_env(&mut self, parent: Option<EnvId>) -> EnvId {
        let idx = self.local.envs.len();
        self.local.envs.push(EnvFrame {
            vars: EnvVars::new(),
            parent,
        });
        EnvId::local_gen(idx, self.local_epoch)
    }

    pub fn env_get(&self, env: EnvId, sym: Symbol) -> Option<Value> {
        crate::perf_bump!(env_get);
        let mut cur = Some(env);
        while let Some(e) = cur {
            crate::perf_bump!(env_hops);
            if e == EnvId::GLOBAL {
                // A dynamic var resolves to its innermost active `binding`, if
                // any, before the shared global default. The stack is empty
                // unless a `binding` is in scope, so this costs nothing on the
                // ordinary path; when active it shadows only at the global level
                // (dynamic vars are never lexically bound).
                if !self.dynamics.is_empty() {
                    if let Some(&(_, v)) = self.dynamics.iter().rev().find(|&&(s, _)| s == sym) {
                        return Some(v);
                    }
                }
                return self.global_lookup_cached(sym);
            }
            let frame = self.env_frame(e);
            // Scan from the end: a later binding shadows an earlier same-named one.
            if let Some(&(_, v)) = frame.vars.iter().rev().find(|&&(s, _)| s == sym) {
                return Some(v);
            }
            cur = frame.parent;
        }
        None
    }

    /// Read the `k`-th captured lexical (`#3` lexical addressing). Fast path: when the
    /// captured env is a **flat frame** whose `vars[k]` is exactly `name` — the VM-built
    /// closure's snapshot, the common case — return it by direct index, no symbol scan or
    /// chain walk. Fallback: a chained / tree-walker env (or a shadowed misalignment)
    /// resolves by name through [`env_get`] (correct, the old cost). `name` makes the fast
    /// path self-verifying, so the two engines stay in lockstep (the differential gate).
    #[inline]
    pub fn capture_value(&self, env: EnvId, k: usize, name: Symbol) -> Value {
        if env != EnvId::GLOBAL {
            let frame = self.env_frame(env);
            if let Some(&(s, v)) = frame.vars.get(k) {
                // Fast path only if `vars[k]` is `name` AND is its **last** binding in
                // this frame — i.e. exactly what `env_get`'s reverse scan returns. A
                // `letrec` env binds a name twice (a nil placeholder + the wired value),
                // so a bare forward index would read the placeholder; the no-later-dup
                // check rejects that and falls through to `env_get` (correct for both).
                if s == name && !frame.vars[k + 1..].iter().any(|&(s2, _)| s2 == name) {
                    return v;
                }
            }
        }
        self.env_get(env, name).unwrap_or(Value::nil())
    }

    /// The distinct lexical names bound along `env`'s frame chain, innermost-first,
    /// stopping at the global scope (whose names are runtime globals, not lexicals).
    /// Used by the compiling VM (ADR-076 §2c): a nested `(fn …)` must snapshot the
    /// enclosing lexical environment it closes over, so the compiler asks which
    /// names that env actually binds. The set is a static property of the closure's
    /// definition site (every instance of the same source closure binds the same
    /// names), so it's safe to derive once and bake into the cached body.
    pub fn env_chain_names(&self, env: EnvId) -> Vec<Symbol> {
        let mut names: Vec<Symbol> = Vec::new();
        let mut cur = env;
        let mut depth = 0;
        while cur != EnvId::GLOBAL && (cur.region() == LOCAL || cur.region() == RUNTIME) {
            let frame = self.env_frame(cur);
            for &(s, _) in frame.vars.iter() {
                if !names.contains(&s) {
                    names.push(s);
                }
            }
            match frame.parent {
                Some(p) => cur = p,
                None => break,
            }
            depth += 1;
            if depth > 10_000 {
                break; // safety belt — env chains shouldn't be this deep
            }
        }
        names
    }

    /// Resolve a name in the shared global table, going through this process's
    /// [`global_ic`](Self::global_ic) inline cache. On a version match the cached
    /// (immovable PRELUDE/RUNTIME) handle is returned without touching the
    /// `RwLock`; otherwise the locked table is read and the entry re-stamped.
    /// Only reached after the local chain and dynamics have missed, so it never
    /// shadows a lexical or dynamic binding. An *unbound* name isn't cached (so it
    /// resolves the moment it's later `def`'d).
    #[inline]
    fn global_lookup_cached(&self, sym: Symbol) -> Option<Value> {
        let cur = self.runtime.version.load(Ordering::Relaxed);
        if let Some(&(ver, val)) = self.global_ic.borrow().get(&sym) {
            if ver == cur {
                return Some(val);
            }
        }
        if let Some(val) = self.runtime.globals_read().get(&sym).copied() {
            self.global_ic.borrow_mut().insert(sym, (cur, val));
            return Some(val);
        }
        // ADR-070: an intra-package qualified reference resolves through its rooted name
        // (`commands/cmd-open` → `bedit/commands/cmd-open`). Only on the MISS path, so an
        // ordinary hit pays nothing; `root_qualified_ref` memoizes the symbol→symbol answer, and
        // the resolved value is cached under the ORIGINAL symbol, so a hot reference costs
        // the same as any other after the first lookup.
        let rooted = self.root_qualified_ref(sym)?;
        let val = self.runtime.globals_read().get(&rooted).copied();
        if let Some(val) = val {
            self.global_ic.borrow_mut().insert(sym, (cur, val));
        }
        val
    }

    // ===== Global bindings (RUNTIME) ============================================

    /// The current global-binding **epoch** — bumped on every `def`/`defmacro`
    /// (and hot-reload) via `runtime.version`. The compiling VM stamps it into a
    /// `Node::Prim2`'s inline-op guard at compile time and re-validates against it
    /// at run time, so a primitive baked inline (`+` → inline `i64` add) self-heals
    /// to the general call path the moment the operator is redefined. Mirrors the
    /// version `global_lookup_cached` already keys the symbol inline-cache on.
    pub fn global_epoch(&self) -> u64 {
        self.runtime.version.load(Ordering::Relaxed)
    }

    /// Address of the global-epoch counter (`runtime.version`), so JIT'd code can read the
    /// epoch with a **raw load** instead of a `brood_rt_global_epoch` FFI *call* on every loop
    /// back-edge / linked call (the call was ~20% of a hoisted-global loop like `loop`). The
    /// counter is an `AtomicU64` living in the `Arc<RuntimeCode>` — a stable address for the
    /// process (`runtime_collect` mutates `version` in place via `Arc::get_mut`, never replaces
    /// the `Arc`), so a JIT'd arm fetches this once at entry and loads through it each iteration.
    /// A plain `u64` load matches the `Relaxed` atomic load (a plain `mov` on the host); the
    /// guard only needs to *eventually* observe a concurrent `def`'s bump, which it does.
    /// It is a formal data race in the abstract model (a plain load vs the writers'
    /// `fetch_add(Relaxed)`), but a benign one on every supported target — and not even
    /// ThreadSanitizer-observable, since TSan instruments rustc-compiled code, not the
    /// Cranelift-JIT'd machine code that performs this load. An atomic op here would buy
    /// nothing on these targets and reinstate the FFI cost, so the plain load stays.
    #[cfg(feature = "jit")]
    pub(crate) fn global_epoch_ptr(&self) -> *const u64 {
        &self.runtime.version as *const AtomicU64 as *const u64
    }

    /// Shared-JIT cache lookup (ADR-101, the spawn lever): the native code published
    /// for a RUNTIME/PRELUDE arm's `(closure_id, argc)` `share_key`, as
    /// `(code_ptr, compile_epoch)`. The caller ([`crate::eval::compile::jit_tier`])
    /// checks `compile_epoch == global_epoch()` before installing — a `def` or RUNTIME
    /// compaction bumps `version`, so a stale entry is never used. See
    /// `RuntimeCode::jit_code_cache`.
    #[cfg(feature = "jit")]
    pub(crate) fn jit_shared_lookup(&self, key: (u64, u16)) -> Option<(*mut u8, u64)> {
        let cache = self.runtime.jit_code_cache.read().ok()?;
        cache.get(&key).map(|&(ptr, epoch)| (ptr as *mut u8, epoch))
    }

    /// Publish a RUNTIME/PRELUDE arm's freshly-installed native code to the shared
    /// cache so the runtime's other processes can install it directly instead of
    /// recompiling. Idempotent overwrite (last writer wins — all writers store the
    /// same code for the same epoch; a newer epoch's recompile correctly replaces an
    /// older entry). See `RuntimeCode::jit_code_cache`.
    #[cfg(feature = "jit")]
    pub(crate) fn jit_shared_publish(&self, key: (u64, u16), code: *mut u8, epoch: u64) {
        if let Ok(mut cache) = self.runtime.jit_code_cache.write() {
            cache.insert(key, (code as usize, epoch));
        }
    }

    /// Shared-JIT lookup for the **inlined** upgrade — the [`Self::jit_shared_lookup`]
    /// counterpart over `jit_inline_cache`. Lets a process install another process's
    /// already-compiled inlined native for the same `(closure_id, argc)` instead of
    /// waiting on its own deferred compile; the caller checks `compile_epoch ==
    /// global_epoch()` before installing, so a `def`/compaction invalidates it.
    #[cfg(feature = "jit")]
    pub(crate) fn jit_inline_lookup(&self, key: (u64, u16)) -> Option<(*mut u8, u64)> {
        let cache = self.runtime.jit_inline_cache.read().ok()?;
        cache.get(&key).map(|&(ptr, epoch)| (ptr as *mut u8, epoch))
    }

    /// Publish a freshly-installed **inlined** native to the shared inline cache — the
    /// [`Self::jit_shared_publish`] counterpart over `jit_inline_cache`. Idempotent
    /// overwrite (last writer wins; all writers store equivalent code for the same
    /// epoch). See `RuntimeCode::jit_inline_cache`.
    #[cfg(feature = "jit")]
    pub(crate) fn jit_inline_publish(&self, key: (u64, u16), code: *mut u8, epoch: u64) {
        if let Ok(mut cache) = self.runtime.jit_inline_cache.write() {
            cache.insert(key, (code as usize, epoch));
        }
    }

    /// Is `sym` bound in the global table (prelude + user `def`s)? An authoritative,
    /// non-racy read of `runtime.globals` (which is seeded with the prelude). Used by
    /// the unbound-symbol diagnostic to tell a *spuriously*-unbound known global (the
    /// fan-out race) apart from a genuinely-undefined name (a typo) — so the
    /// scheduler-race hint only fires for the former.
    pub fn global_defined(&self, sym: Symbol) -> bool {
        self.runtime.globals_read().get(&sym).is_some()
    }

    /// Is `sym` **reserved** — a name the language itself ships (prelude, builtin, or
    /// embedded std module)? A global `def` of one is refused (ADR-166); the caller
    /// raises, because that is where a user-facing error belongs. Always false while
    /// this process is loading an embedded module, which is the one context allowed to
    /// (re)define its own surface.
    pub fn is_reserved_global(&self, sym: Symbol) -> bool {
        !self.in_module_load() && self.runtime.is_sealed(sym)
    }

    /// Reserve `sym`, so a later user `def` of it is refused. Called for every global
    /// an embedded module defines while loading.
    pub fn reserve_global(&self, sym: Symbol) {
        self.runtime.seal(sym);
    }

    /// True while this process is loading an embedded std module.
    pub fn in_module_load(&self) -> bool {
        self.cold().is_some_and(|c| c.module_load_depth > 0)
    }

    /// Enter/leave an embedded-module load. Paired by `%load-module-source`, which
    /// decrements even when the load throws — a leaked exemption would silently
    /// un-reserve the language.
    pub fn enter_module_load(&mut self) {
        self.cold_mut().module_load_depth += 1;
    }
    pub fn leave_module_load(&mut self) {
        let d = &mut self.cold_mut().module_load_depth;
        *d = d.saturating_sub(1);
    }

    /// **Atomically** update a global that holds a registry (KI-22).
    ///
    /// Every load-time registry — `*impls*`, `*features*`, `*abilities*`, `*methods*`,
    /// `*record-ids*`, … — is one global holding a whole map or list, and Brood updates it
    /// as `(def *X* (assoc *X* …))`. That reads, computes and writes as three separate
    /// steps, so two processes registering at the same time each read the old value and each
    /// write their own successor: the later write silently drops the earlier one. Measured
    /// **218 of 500** concurrent registrations lost, after which the op dispatched to
    /// `:default` — a wrong answer, not a crash, and `impl` is hot-reloadable by design.
    ///
    /// The whole read-modify-write happens here, under `registry_lock`, so it is atomic by
    /// construction: no CAS (and so no ABA question), no retry loop, no spinning, and no
    /// callback into Brood while a lock is held. Two earlier in-language attempts failed
    /// exactly there — optimistic retry cannot close the read-write window, and a ticket lock
    /// either burns CPU busy-waiting or desynchronises when a bounded wait times out.
    ///
    /// `op` selects the update; `path` is `[k]` or `[k1 k2]` (nested one level, for
    /// `*impls*`/`*methods*`, whose shape is `ability -> id -> fn`):
    /// - `:assoc` — set `path` to `val`, creating the intermediate map if absent.
    /// - `:assoc-new` — the same, but only when `path` is currently **absent**. The
    ///   presence test has to be inside the lock too: a derived method mirror that checks
    ///   "absent?" outside it can clobber an authored impl registered in between.
    /// - `:dissoc` — remove `path` (one key).
    /// - `:cons-new` — prepend `val` to a list-valued global unless it is already a member
    ///   (`provide` / `*features*`).
    ///
    /// Returns true when the registry was written, false when the op declined (`:assoc-new`
    /// onto a present key, `:cons-new` of an existing member) — so Brood can still report
    /// "already there" without a second, racy read.
    pub fn registry_update(
        &mut self,
        env: EnvId,
        sym: Symbol,
        op: RegistryOp,
        path: &[Value],
        val: Value,
    ) -> bool {
        // Clone the Arc so the guard borrows a LOCAL, leaving `&mut self` free for the map
        // ops between the read and the write. Recover from a poisoned lock rather than
        // propagate: a panicking registrar leaves the registry structurally sound (values
        // are immutable), and wedging every later registration would be worse.
        let rt = self.runtime.clone();
        let _guard = rt.registry_lock.lock().unwrap_or_else(|e| e.into_inner());

        // `def` binds at `env_root(env)`, which is NOT always `EnvId::GLOBAL`: during prelude
        // load the root is a bootstrap env whose bindings later seed the shared runtime. A
        // write straight to the globals table there is silently dropped (it cost the prelude
        // its own `Display`/`Inspect` impls). Read and write the same place `def` would.
        let root = self.env_root(env);
        let cur = self.env_get(env, sym).unwrap_or(Value::nil());
        let k1 = path.first().copied().unwrap_or(Value::nil());

        let next = match op {
            RegistryOp::ConsNew => {
                if self.list_contains(cur, val) {
                    return false;
                }
                self.alloc_pair(val, cur)
            }
            RegistryOp::Dissoc => match cur.unpack() {
                ValueRef::Map(id) => {
                    if path.len() >= 2 {
                        // NESTED dissoc, symmetric with `:assoc`'s two-key path: remove `k2`
                        // from the inner map at `k1`, leaving that map (and every sibling
                        // key) in place. Without this, a two-key `:dissoc` silently used
                        // only `k1` and removed the WHOLE inner map — which is how
                        // `unregister-impl`, retracting one id of `[ability op]`, destroyed
                        // every impl of that op including the language's `:default`.
                        let k2 = path[1];
                        match self.map_get(id, k1).map(|v| v.unpack()) {
                            Some(ValueRef::Map(inner)) => {
                                let inner_next = self.map_dissoc(inner, k2);
                                self.map_assoc(id, k1, inner_next)
                            }
                            // no inner map at `k1`: nothing to remove
                            _ => return false,
                        }
                    } else {
                        self.map_dissoc(id, k1)
                    }
                }
                _ => return false,
            },
            RegistryOp::Assoc | RegistryOp::AssocNew => {
                let outer = match cur.unpack() {
                    ValueRef::Map(id) => id,
                    // An uninitialised registry (nil) starts as an empty map rather than
                    // failing — the same shape `(or *X* {})` had at the call sites.
                    _ => match self.alloc_empty_map().unpack() {
                        ValueRef::Map(id) => id,
                        _ => unreachable!("alloc_empty_map returns a map"),
                    },
                };
                if path.len() >= 2 {
                    let k2 = path[1];
                    let inner_cur = self.map_get(outer, k1);
                    let inner_id = match inner_cur.map(|v| v.unpack()) {
                        Some(ValueRef::Map(id)) => id,
                        _ => match self.alloc_empty_map().unpack() {
                            ValueRef::Map(id) => id,
                            _ => unreachable!("alloc_empty_map returns a map"),
                        },
                    };
                    if op == RegistryOp::AssocNew && self.map_get(inner_id, k2).is_some() {
                        return false;
                    }
                    let inner = self.map_assoc(inner_id, k2, val);
                    // `map_assoc` can collect, so re-resolve the outer handle before using it.
                    let outer = match self.env_get(env, sym).unwrap_or(Value::nil()).unpack() {
                        ValueRef::Map(id) => id,
                        _ => outer,
                    };
                    self.map_assoc(outer, k1, inner)
                } else {
                    if op == RegistryOp::AssocNew && self.map_get(outer, k1).is_some() {
                        return false;
                    }
                    self.map_assoc(outer, k1, val)
                }
            }
        };
        // Reuse `env_define`'s global path: it promotes into the shared RUNTIME region and
        // bumps the version that invalidates every process's global inline cache.
        self.env_define(root, sym, next);
        true
    }

    /// `(%registry-cas! 'sym old new)` — compare-and-swap a registry global under the same
    /// lock as [`Self::registry_update`]. Rebinds `sym` to `new` and returns true **only if**
    /// its current value still equals `old`; otherwise leaves it alone and returns false, so
    /// the caller can recompute against the value that won and retry.
    ///
    /// This is the general form of `registry_update`. That one has to name every shape it
    /// supports as an op (`:assoc`, `:cons-new`, …), which covers a registry whose update is
    /// one map/list operation and nothing else. The registries in `std/` are not all like
    /// that: `face-set` merges into the *existing* entry, `attach` strips an id across every
    /// bucket before consing onto one, `register-repl-command` filters by name-overlap and
    /// appends. Expressing those as ops would mean a Rust op per shape — and the transform
    /// itself is policy, which belongs in Brood. A CAS lets the transform stay an ordinary
    /// Brood function (`registry-swap!` in the prelude retries around it) while the
    /// read-decide-write stays indivisible.
    ///
    /// Equality is structural, so an ABA against an equal-valued registry is indistinguishable
    /// — and harmless: the retry would recompute the same answer.
    pub fn registry_cas(&mut self, env: EnvId, sym: Symbol, old: Value, new: Value) -> bool {
        let rt = self.runtime.clone();
        let _guard = rt.registry_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Read through the chain and write at the root, exactly as `def` does (see
        // `registry_update`: the root is NOT always `EnvId::GLOBAL` during prelude load).
        // Matching `def` is what makes a `defdyn` registry safe to convert — an active
        // `binding` shadows the root write for both spellings identically.
        let root = self.env_root(env);
        let cur = self.env_get(env, sym).unwrap_or(Value::nil());
        if !self.equal(cur, old) {
            return false;
        }
        self.env_define(root, sym, new);
        true
    }

    /// Structural `member?` over a proper list — the `:cons-new` presence test, kept inside
    /// [`Self::registry_update`]'s lock so `provide` cannot double-add under a race.
    fn list_contains(&self, list: Value, needle: Value) -> bool {
        let mut cur = list;
        while let ValueRef::Pair(id) = cur.unpack() {
            let (head, tail) = {
                let p = self.pair(id);
                (p.0, p.1)
            };
            if self.equal(head, needle) {
                return true;
            }
            cur = tail;
        }
        false
    }

    pub fn env_define(&mut self, env: EnvId, sym: Symbol, val: Value) {
        if env == EnvId::GLOBAL {
            // Privacy (ADR-146) is no longer derived from the name here — a
            // `defn-`/`def-` records it explicitly through the `%mark-private`
            // primitive, and the prelude's privates are seeded in `seeded`.
            // Dedup an unchanged hot-reload redefinition (Stage 5): if `sym` is
            // already bound to a closure structurally identical to `val`, keep the
            // existing (already-promoted) binding rather than append a duplicate
            // into the append-only RUNTIME region. Bounds the leak for the common
            // save-without-change / formatter-churn path; any *real* edit differs
            // structurally and falls through to the normal promote+rebind.
            let existing = self.runtime.globals_read().get(&sym).copied();
            if let Some(old) = existing {
                let unchanged = match (old.unpack(), val.unpack()) {
                    (ValueRef::Fn(o), ValueRef::Fn(n)) => self.closures_structurally_equal(o, n),
                    (ValueRef::Macro(o), ValueRef::Macro(n)) => {
                        self.closures_structurally_equal(o, n)
                    }
                    _ => false,
                };
                if unchanged {
                    return;
                }
            }
            // Global code/data is shared across inner processes, so promote it
            // into the shared RUNTIME region before binding. `rehome_to_current`
            // then re-homes a value that promote left in a *non-current* generation
            // (an already-RUNTIME handle promote passes through unchanged) into the
            // current one, so a `def` can never re-pin a draining generation through
            // the shared globals table (ADR-091 Stage 5 soundness). No-op until aging
            // has created a non-current generation to re-home out of.
            let shared = self.rehome_to_current(self.promote(val));
            self.runtime.globals_write().insert(sym, shared);
            // Invalidate every process's global inline cache (late binding).
            self.runtime.version.fetch_add(1, Ordering::Relaxed);
        } else if env.is_old() {
            // The frame was tenured (a minor collection promoted it while it was
            // still being bound — e.g. a collection during a `let` rhs eval). Mutate
            // it in the old space and remember it: this push can create an
            // OLD->YOUNG edge (`val` is a fresh nursery value), which the next minor
            // collection must trace and rewrite, since it otherwise never scans old.
            // De-dup: repeated binds into the same tenured frame (a long `let`
            // body, a binding loop) would otherwise re-push it every time,
            // growing `remembered` — and the minor's rewrite walk — without
            // bound until the next tenure clears it. The linear scan is fine:
            // deduped, the set holds one entry per *distinct* old frame mutated
            // since the last minor, which is tiny.
            self.old_mut().envs[env.index()].vars.push((sym, val));
            if !self.remembered.contains(&env) {
                self.remembered.push(env);
            }
        } else {
            self.local.envs[env.index()].vars.push((sym, val));
        }
    }

    // ----- dynamic-variable bindings (the `binding` form) -----

    /// Push a dynamic binding of `sym` to `val` (the innermost wins on lookup).
    /// Paired with [`Heap::pop_dynamic`] by the `%binding` primitive, which pops
    /// exactly what it pushed when its body returns — even on error.
    pub fn push_dynamic(&mut self, sym: Symbol, val: Value) {
        self.dynamics.push((sym, val));
    }

    /// Pop the most recent dynamic binding (the matching unwind of `push_dynamic`).
    pub fn pop_dynamic(&mut self) {
        self.dynamics.pop();
    }

    /// The current (innermost) value of dynamic variable `sym`, if a `binding` for
    /// it is active — the read side of [`push_dynamic`]. An empty stack (the common
    /// case) costs one `is_empty` check. Used by `spawn` to inherit a propagating
    /// causal context (`*trace-context*`) into a child without an explicit hand-off.
    pub fn current_dynamic(&self, sym: Symbol) -> Option<Value> {
        if self.dynamics.is_empty() {
            return None;
        }
        self.dynamics
            .iter()
            .rev()
            .find(|&&(s, _)| s == sym)
            .map(|&(_, v)| v)
    }

    /// The debugger's durable per-process trace context (ADR-174), or `None`. A
    /// settable slot (unlike a `binding`): `spawn` copies it into a child, `send`
    /// ships it, `receive` overwrites it on pop. GC-traced with [`dynamics`].
    #[cfg(feature = "dev-tools")]
    pub fn trace_context(&self) -> Option<Value> {
        self.trace_context
    }

    /// Set (or clear) the durable per-process trace context. `own` marks it as this
    /// process's own context (propagated by `spawn`) versus one adopted from a message
    /// (not propagated). The value must be a promoted/LOCAL handle valid in this heap;
    /// it is then traced like a root.
    #[cfg(feature = "dev-tools")]
    pub fn set_trace_context(&mut self, v: Option<Value>, own: bool) {
        self.trace_context = v;
        self.trace_context_own = own;
    }

    /// Whether the current [`trace_context`] is the process's OWN (propagate on spawn),
    /// not merely adopted from a message.
    #[cfg(feature = "dev-tools")]
    pub fn trace_context_own(&self) -> bool {
        self.trace_context_own
    }

    /// Snapshot the runtime's global bindings (`symbol -> value`). Cheap: the
    /// values are `Copy` handles. Pair with [`Heap::restore_globals`] to run code
    /// against a *private copy* of the globals — mutations to the live table can
    /// then be rolled back (this is what the `%isolate` primitive does for
    /// `:isolated` tests). Only meaningful when no other process is writing the
    /// table concurrently.
    pub fn snapshot_globals(&self) -> GlobalsSnapshot {
        // The snapshot holds raw RUNTIME handles off the graph; suppress RUNTIME compaction
        // until the paired `restore_globals` reinstalls (or discards) it, so a relocation
        // can't strand those handles (KI-6). Structural — every caller of the protocol is
        // covered, not just `%isolate`. The `#[must_use]` guard + by-value restore make
        // forgetting-to-restore a compiler warning and double-restore impossible.
        self.begin_rt_collect_block();
        GlobalsSnapshot {
            saved: self.runtime.globals_read().clone(),
            block_depth: self.rt_collect_block.get(),
        }
    }

    /// Every symbol currently bound in the global table (prelude + user `def`s).
    /// For tooling/introspection — `(global-names)` feeds completion and
    /// workspace-symbol queries (see `docs/lsp.md`). Returns just the keys, so
    /// no `Value`s are cloned.
    pub fn global_symbols(&self) -> Vec<Symbol> {
        self.runtime.globals_read().keys().copied().collect()
    }

    /// The public exports of module `prefix` (a `mod/` segment, trailing slash included) as
    /// `(bare, qualified)` symbol pairs — the set `(:use mod)` refers. Public = a *direct*
    /// `mod/name` global whose bare tail is non-empty and carries no `--` private marker
    /// (matching `%refer`'s scan). Cached + count-keyed (see
    /// [`module_exports_cache`](Self::module_exports_cache)) so the checker's per-file
    /// `(:use …)` resolution builds the index by ONE pass over the globals instead of
    /// rescanning every global per file (O(files²)). Empty when the module isn't loaded (no
    /// `mod/*` globals) — the checker then `require`s it first. Checker use only.
    pub fn module_public_exports(&self, prefix: &str) -> Vec<(Symbol, Symbol)> {
        let count = self.runtime.globals_read().len();
        let fresh = matches!(self.check.borrow().as_ref()
            .and_then(|c| c.exports.as_ref()), Some((c, _)) if *c == count);
        if !fresh {
            let mut map: std::collections::HashMap<String, Vec<(Symbol, Symbol)>> =
                std::collections::HashMap::new();
            for g in self.global_symbols() {
                let name = crate::core::value::symbol_name(g);
                if let Some(slash) = name.rfind('/') {
                    let bare = &name[slash + 1..];
                    // `g` is a live enumerated global, so `is_private` (the recorded
                    // fact) is exact here; the module is definitionally loaded.
                    if !bare.is_empty() && !self.is_private(g) {
                        let bare_sym = crate::core::value::intern(bare);
                        map.entry(name[..=slash].to_string())
                            .or_default()
                            .push((bare_sym, g));
                    }
                }
            }
            self.check_mut().exports = Some((count, std::sync::Arc::new(map)));
        }
        self.check
            .borrow()
            .as_ref()
            .and_then(|c| c.exports.as_ref())
            .and_then(|(_, m)| m.get(prefix).cloned())
            .unwrap_or_default()
    }

    /// The set of `mod/` namespace prefixes present in the loaded image (the checker's
    /// `known_ns`), cached + shared as an `Arc` (see [`known_ns_cache`](Self::known_ns_cache)).
    /// Count-keyed like [`module_public_exports`](Self::module_public_exports) — checker-only,
    /// sound because a whole-project check does no `def`s per file, so an O(1) `Arc` clone on
    /// all but the first file (was an O(globals) scan per file → O(files²)).
    pub fn known_ns_prefixes(&self) -> std::sync::Arc<std::collections::HashSet<String>> {
        let count = self.runtime.globals_read().len();
        if let Some((c, arc)) = self
            .check
            .borrow()
            .as_ref()
            .and_then(|c| c.known_ns.as_ref())
        {
            if *c == count {
                return std::sync::Arc::clone(arc);
            }
        }
        let mut set = std::collections::HashSet::new();
        for sym in self.global_symbols() {
            let name = crate::core::value::symbol_name(sym);
            if let Some(slash) = name.rfind('/') {
                set.insert(name[..=slash].to_string());
            }
        }
        let arc = std::sync::Arc::new(set);
        self.check_mut().known_ns = Some((count, std::sync::Arc::clone(&arc)));
        arc
    }

    // ── Phase-2 incremental-check dependency recorder (ADR-119) ───────────────
    // Per-process (this heap travels with the green process), so `check-file-deps`
    // can run concurrently across the worker pool without clobbering. See the
    // `check_dep_rec` field and `types::check::deps`.

    /// Start recording global observations into a fresh record on this heap.
    pub(crate) fn begin_check_dep_record(&self) {
        self.check_mut().dep_rec = Some(CheckDepRec::default());
    }
    /// Drain and return the recorded observations (clearing the recorder).
    pub(crate) fn take_check_dep_record(&self) -> Option<CheckDepRec> {
        self.check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.take())
    }
    /// Record an observed global symbol (binding/arity/sig).
    pub(crate) fn rec_check_dep_sym(&self, sym: Symbol) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            d.syms.insert(sym);
        }
    }
    /// Record a queried `mod/` known-namespace prefix.
    pub(crate) fn rec_check_dep_ns(&self, prefix: &str) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            if !d.known_ns.contains(prefix) {
                d.known_ns.insert(prefix.to_string());
            }
        }
    }
    /// Record a `mod/` prefix whose export set was read.
    pub(crate) fn rec_check_dep_exports(&self, prefix: &str) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            if !d.exports.contains(prefix) {
                d.exports.insert(prefix.to_string());
            }
        }
    }
    /// Record that the `*protocols*` table was consulted.
    pub(crate) fn rec_check_dep_protocols(&self) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            d.protocols = true;
        }
    }
    /// Record a global DEFINED in the file being checked (excluded from dep-keys).
    pub(crate) fn rec_check_dep_own(&self, sym: Symbol) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            d.own.insert(sym);
        }
    }

    /// Register a user-declared `(sig name type)` signature: `sym` is the
    /// module-qualified global symbol (the same key `def` would produce), `type_value`
    /// the raw type-expression form (e.g. `(int -> int)`). The value is `promote`d into
    /// the shared RUNTIME region first — the store is shared across the runtime's
    /// processes (`Arc`) and must outlive the LOCAL heap, exactly like a global. Read
    /// by the checker via [`Heap::declared_sig_value`]. Idempotent: a re-`def`/reload
    /// just overwrites. (No `version` bump — declared sigs aren't consulted by the
    /// per-process global inline cache.)
    pub fn set_declared_sig(&mut self, sym: Symbol, type_value: Value) {
        // Re-home into the current generation, for the same reason as a global `def`
        // (`declared_sigs` is a shared root the drain scans; a stale old-gen handle
        // stored here would re-pin a draining generation — ADR-091 Stage 5).
        let shared = self.rehome_to_current(self.promote(type_value));
        self.runtime
            .declared_sigs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym, shared);
    }

    /// The raw type-expression `Value` a `(sig …)` declared for the qualified global
    /// `sym`, or `None`. The checker (`sig_of`) parses it to a signature and gives it
    /// precedence over primitive/curated/inferred sigs. See [`Heap::set_declared_sig`].
    pub fn declared_sig_value(&self, sym: Symbol) -> Option<Value> {
        self.runtime
            .declared_sigs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&sym)
            .copied()
    }

    /// Restore the runtime's global bindings from a [`Heap::snapshot_globals`]
    /// snapshot, discarding every `def` made since it was taken — so a name `def`'d
    /// since the snapshot becomes unbound again, and a rebound name returns to its
    /// earlier value. The `def`'d code the bindings referenced is now unreachable and
    /// is reclaimed by the next RUNTIME compaction (which this call re-enables — see
    /// [`rt_collect_block`](Self::rt_collect_block) — after `snapshot_globals` suppressed it).
    pub fn restore_globals(&self, snapshot: GlobalsSnapshot) {
        // LIFO check: the live suppression depth must still equal what this snapshot set,
        // or snapshots were restored out of order and we'd release the wrong scope's
        // suppression (re-exposing an outer snapshot to KI-6). The newtype already rules
        // out restore-without-snapshot and double-restore; this catches reordering.
        debug_assert_eq!(
            self.rt_collect_block.get(),
            snapshot.block_depth,
            "restore_globals out of order — globals snapshots must be restored LIFO"
        );
        *self.runtime.globals_write() = snapshot.saved;
        // Wholesale table swap — invalidate every stamped global inline cache.
        self.runtime.version.fetch_add(1, Ordering::Relaxed);
        // Release the compaction suppression `snapshot_globals` took: the snapshot is no
        // longer outstanding, so a relocation can no longer strand it (KI-6).
        self.end_rt_collect_block();
    }

    /// Walk to the global scope at the bottom of the frame chain.
    pub fn env_root(&self, env: EnvId) -> EnvId {
        let mut cur = env;
        loop {
            if cur == EnvId::GLOBAL {
                return EnvId::GLOBAL;
            }
            match self.env_frame(cur).parent {
                Some(p) => cur = p,
                None => return cur, // the prelude builder's local root
            }
        }
    }
}

/// Forwarding table for [`Heap::promote`]: LOCAL slot index → the RUNTIME handle
/// it was promoted to, for the two handle kinds that can form a cycle (a closure
/// capturing its own binding scope). Lets a cyclic graph terminate — the back-edge
/// resolves to the already-reserved RUNTIME handle — and collapses a shared (DAG)
/// closure/env to one RUNTIME copy. Pairs/vectors/maps are acyclic by construction
/// so they need no forwarding (they'd only ever be a finite tree to re-copy).
#[derive(Default)]
struct PromoteForward {
    closures: HashMap<u32, ClosureId>,
    envs: HashMap<u32, EnvId>,
}

#[cfg(all(test, feature = "jit"))]
mod vecstore_layout_tests {
    use super::*;
    use crate::core::value::Value;

    /// Pin the byte layout the JIT hardcodes for its inline small-vector read
    /// (`jit_lower.rs`): the `Inline` discriminant is 0, and `len`/`items` sit at
    /// the advertised offsets within a slot. A `#[repr(u8)]` layout drift (e.g.
    /// bumping `INLINE_VEC_CAP` or reordering fields) fails here rather than
    /// silently miscompiling every `nth`.
    #[test]
    fn vecstore_jit_layout() {
        let v = VecStore::Inline {
            len: 2,
            items: [Value::int(7), Value::int(9)],
        };
        let base = &v as *const VecStore as usize;
        // Discriminant byte at offset 0 (repr(u8), RFC 2195).
        let tag = unsafe { *(base as *const u8) };
        assert_eq!(tag as i64, VecStore::JIT_INLINE_TAG, "Inline discriminant");
        if let VecStore::Inline { len, items } = &v {
            assert_eq!(
                len as *const u8 as usize - base,
                VecStore::JIT_LEN_OFF as usize,
                "Inline.len offset"
            );
            assert_eq!(
                items.as_ptr() as usize - base,
                VecStore::JIT_ITEMS_OFF as usize,
                "Inline.items offset"
            );
        }
        assert_eq!(
            std::mem::size_of::<VecStore>() as i64,
            VecStore::JIT_STRIDE,
            "slab stride"
        );
        // The JIT reads a Value element as 3 i64 words; the slab stride between
        // elements is `size_of::<Value>()`.
        assert_eq!(std::mem::size_of::<Value>(), 24, "Value stride");
        // Spill layout: discriminant 1 @0, cached ptr @8, cached len @16 — the
        // JIT's pointer-read path loads exactly these.
        let sp = VecStore::spill(vec![Value::int(1), Value::int(2), Value::int(3)]);
        let sbase = &sp as *const VecStore as usize;
        let stag = unsafe { *(sbase as *const u8) };
        assert_eq!(stag as i64, VecStore::JIT_SPILL_TAG, "Spill discriminant");
        if let VecStore::Spill { ptr, len, vec } = &sp {
            assert_eq!(
                ptr as *const *const Value as usize - sbase,
                VecStore::JIT_SPILL_PTR_OFF as usize,
                "Spill.ptr offset"
            );
            assert_eq!(
                len as *const u64 as usize - sbase,
                VecStore::JIT_SPILL_LEN_OFF as usize,
                "Spill.len offset"
            );
            assert_eq!(*ptr, vec.as_ptr(), "cached ptr matches the buffer");
            assert_eq!(*len as usize, vec.len(), "cached len matches");
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
