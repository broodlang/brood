//! The parse cache under `%gui-draw`'s memoised fragments (ADR-470).
//!
//! `editor/ui`'s `ui-memo` hands a view fragment's ops back UNCHANGED — the very heap
//! value it emitted last turn — when the fragment's deps are `=`, and says so in its
//! marker: `[:ui/memo key deps ops true]`. A window whose display takes markers
//! (`gui-display`'s `:fragments? true`) is drawn the frame WITH them, and a reused one
//! is not parsed again: the ops it parsed to last frame are shared as one
//! `Op::Fragment`. That is the per-frame parse proportional to what changed rather than
//! to what is on screen, and the shared allocation is what lets the window's frame diff
//! and strip diff skip the fragment by pointer.
//!
//! **Why a hit is exact.** The cache holds the fragments of the last frame drawn to the
//! window, keyed by the key's hash, each with the heap value its ops were parsed from.
//! A hit needs all three of: the marker's reused flag (the loop's promise that `ops` is
//! the value the key had in the previous frame, which is the frame this cache was built
//! from — `ui-run` harvests and draws in the same turn), the same key hash, and the same
//! heap object. A key that occurs twice in one frame is never cached (it would be two
//! entries under one hash), and the cache belongs to the process that drew it: a draw
//! from another process starts empty, since another heap's ids mean nothing here.
//! Anything short of a hit parses the ops, so a miss costs what every frame cost before.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use crate::core::heap::Heap;
use crate::core::value::Value;
use crate::host::gui::{FragmentOps, Op};

/// One fragment of the last frame: the value its ops were parsed from, the parsed ops,
/// and the keys of every fragment nested in it — carried forward with it on a hit, so a
/// later frame that recomputes the outer fragment still finds its unchanged rows.
pub(super) struct CachedFragment {
    source: Value,
    ops: Arc<[Op]>,
    nested: Vec<u64>,
}

/// A window's cache: the process that drew it and its fragments by key hash.
struct WindowFragments {
    pid: Option<u64>,
    entries: HashMap<u64, CachedFragment>,
}

// Every window's cache, by window id. A `Value` in one is only ever compared
// (`same_heap_object`), never dereferenced, so keeping one across turns reaches no cell.
static CACHES: LazyLock<Mutex<HashMap<u64, WindowFragments>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One frame's parse: the previous frame's fragments (`previous`, consumed as they are
/// reused) and this frame's (`current`), plus the keys recorded so far — the stack a
/// fragment's `nested` list is cut from.
pub(super) struct FragmentPass {
    previous: HashMap<u64, CachedFragment>,
    current: HashMap<u64, CachedFragment>,
    duplicated: HashSet<u64>,
    recorded: Vec<u64>,
}

impl FragmentPass {
    /// A pass with nothing cached — every fragment parses: a frame painted once, with no
    /// window to keep its fragments for (`%gui-render-frame`, ADR-609).
    pub(super) fn empty() -> FragmentPass {
        FragmentPass::over(HashMap::new())
    }

    fn over(previous: HashMap<u64, CachedFragment>) -> FragmentPass {
        FragmentPass {
            previous,
            current: HashMap::new(),
            duplicated: HashSet::new(),
            recorded: Vec::new(),
        }
    }

    /// The pass for a frame drawn to `window` by the current process.
    pub(super) fn for_window(window: u64) -> FragmentPass {
        let pid = crate::process::current_pid();
        let previous = CACHES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&window)
            .filter(|cache| cache.pid == pid)
            .map(|cache| cache.entries)
            .unwrap_or_default();
        FragmentPass::over(previous)
    }

    /// Keep this frame's fragments as `window`'s cache for the next frame.
    pub(super) fn store(self, window: u64) {
        let pid = crate::process::current_pid();
        let entries = self.finish();
        CACHES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(window, WindowFragments { pid, entries });
    }

    /// This frame's fragments, less any key that occurred more than once.
    fn finish(mut self) -> HashMap<u64, CachedFragment> {
        for key in &self.duplicated {
            self.current.remove(key);
        }
        self.current
    }

    /// The number of fragments this frame will keep — what a test asks.
    #[cfg(test)]
    pub(super) fn kept(&self) -> usize {
        self.current
            .keys()
            .filter(|key| !self.duplicated.contains(key))
            .count()
    }

    /// Record `entry` under `key` for the next frame; a second occurrence of a key in
    /// one frame poisons it.
    fn insert(&mut self, key: u64, entry: CachedFragment) {
        self.recorded.push(key);
        if self.current.insert(key, entry).is_some() {
            self.duplicated.insert(key);
        }
    }

    /// The ops `source` parsed to under `key` last frame, when the marker says it was
    /// reused and it is that very value — re-recorded, with everything nested in it, for
    /// the next frame. `None` is a miss: parse.
    fn take_hit(&mut self, key: u64, source: Value) -> Option<Arc<[Op]>> {
        let hit = self
            .previous
            .get(&key)
            .is_some_and(|entry| entry.source.same_heap_object(source));
        if !hit {
            return None;
        }
        let entry = self.previous.remove(&key)?;
        for nested in &entry.nested {
            match self.previous.remove(nested) {
                Some(inner) => self.insert(*nested, inner),
                // taken already: the key occurs twice in this frame
                None if self.current.contains_key(nested) => {
                    self.duplicated.insert(*nested);
                }
                None => {}
            }
        }
        let ops = entry.ops.clone();
        self.insert(key, entry);
        Some(ops)
    }
}

/// Forget `window`'s fragments (it closed).
pub(super) fn forget(window: u64) {
    CACHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&window);
}

/// Whether `op` (or anything inside it) is an op the window reads from the WHOLE frame
/// rather than paints in place — a sound, the music, an effect, an emitter, the seam. A fragment
/// holding one is spliced instead of shared, so those scanners never look inside a
/// fragment.
fn frame_global(op: &Op) -> bool {
    match op {
        Op::Sound { .. }
        | Op::Music { .. }
        | Op::Ambient { .. }
        | Op::Light { .. }
        | Op::Overlay
        | Op::Bloom { .. }
        | Op::Crt { .. }
        | Op::Emitter { .. }
        | Op::Burst { .. }
        | Op::Seam { .. } => true,
        Op::ScrollRegion { ops, .. } | Op::CellRegion { ops, .. } | Op::AboveSeam { ops } => {
            ops.iter().any(frame_global)
        }
        _ => false,
    }
}

/// Parse a `[:ui/memo key deps ops reused?]` marker's `parts` onto `out`: the shared ops
/// of last frame on a hit, else its ops parsed by `parse` — as one `Op::Fragment`, or
/// spliced when they hold a frame-global op.
pub(super) fn parse_marker(
    heap: &Heap,
    parts: &[Value],
    pass: &mut FragmentPass,
    out: &mut Vec<Op>,
    parse: impl FnOnce(&Heap, Vec<Value>, &mut FragmentPass) -> Vec<Op>,
) {
    let (Some(key), Some(source)) = (parts.get(1), parts.get(3)) else {
        return;
    };
    let reused = matches!(parts.get(4), Some(Value::Bool(true)));
    let key = heap.hash_value(*key);
    if reused && let Some(ops) = pass.take_hit(key, *source) {
        out.push(Op::Fragment(FragmentOps(ops)));
        return;
    }
    let Some(items) = heap.list_or_vector_items(*source) else {
        return; // a marker whose ops are not a sequence paints nothing
    };
    let first_nested = pass.recorded.len();
    let ops = parse(heap, items, pass);
    if ops.iter().any(frame_global) {
        out.extend(ops);
        return;
    }
    let nested = pass.recorded[first_nested..].to_vec();
    let ops: Arc<[Op]> = ops.into();
    pass.insert(
        key,
        CachedFragment {
            source: *source,
            ops: ops.clone(),
            nested,
        },
    );
    out.push(Op::Fragment(FragmentOps(ops)));
}

#[cfg(test)]
mod tests {
    use super::super::{GuiOpTags, parse_gui_ops, tagged_ops};
    use super::*;
    use crate::core::value;

    fn text(heap: &mut Heap, row: i64, s: &str) -> Value {
        let s = heap.alloc_string(s);
        heap.alloc_vector(vec![
            Value::Keyword(value::intern("text")),
            Value::int(row),
            Value::int(0),
            s,
        ])
    }

    fn marker(heap: &mut Heap, key: &str, ops: Value, reused: bool) -> Value {
        heap.alloc_vector(vec![
            Value::Keyword(value::intern("ui/memo")),
            Value::Keyword(value::intern(key)),
            Value::nil(),
            ops,
            Value::Bool(reused),
        ])
    }

    /// A frame's ops through one pass, as `%gui-draw` parses them.
    fn parse(heap: &Heap, frame: Vec<Value>, pass: &mut FragmentPass) -> Vec<Op> {
        parse_gui_ops(heap, tagged_ops(heap, frame), &GuiOpTags::new(), pass)
    }

    fn next(pass: FragmentPass) -> FragmentPass {
        FragmentPass::over(pass.finish())
    }

    fn shared(op: &Op) -> &Arc<[Op]> {
        match op {
            Op::Fragment(FragmentOps(ops)) => ops,
            _ => panic!("not a fragment"),
        }
    }

    #[test]
    fn a_reused_fragment_is_last_frames_parse_and_anything_less_parses_again() {
        let mut heap = Heap::new();
        let line = text(&mut heap, 0, "alpha");
        let ops = heap.alloc_vector(vec![line]);
        let first_marker = marker(&mut heap, "row", ops, false);
        let mut pass = FragmentPass::empty();
        let first = parse(&heap, vec![first_marker], &mut pass);
        assert_eq!(first.len(), 1);
        assert_eq!(pass.kept(), 1);

        // the loop reused it: the same value, flagged — last frame's allocation
        let reused = marker(&mut heap, "row", ops, true);
        let mut pass = next(pass);
        let second = parse(&heap, vec![reused], &mut pass);
        assert!(Arc::ptr_eq(shared(&first[0]), shared(&second[0])));

        // not flagged reused: parsed again, equal but not shared
        let recomputed = marker(&mut heap, "row", ops, false);
        let mut pass = next(pass);
        let third = parse(&heap, vec![recomputed], &mut pass);
        assert!(!Arc::ptr_eq(shared(&second[0]), shared(&third[0])));
        assert!(third == second);

        // flagged, but another value with the same ops: parsed again
        let copy = heap.alloc_vector(vec![line]);
        let other = marker(&mut heap, "row", copy, true);
        let mut pass = next(pass);
        let fourth = parse(&heap, vec![other], &mut pass);
        assert!(!Arc::ptr_eq(shared(&third[0]), shared(&fourth[0])));
    }

    #[test]
    fn a_key_drawn_twice_in_a_frame_is_never_reused() {
        let mut heap = Heap::new();
        let line = text(&mut heap, 0, "a");
        let ops = heap.alloc_vector(vec![line]);
        let twice = marker(&mut heap, "row", ops, false);
        let mut pass = FragmentPass::empty();
        parse(&heap, vec![twice, twice], &mut pass);
        assert_eq!(pass.kept(), 0);
        let reused = marker(&mut heap, "row", ops, true);
        let mut pass = next(pass);
        parse(&heap, vec![reused], &mut pass);
        assert_eq!(pass.kept(), 1, "a miss parses and caches it afresh");
    }

    #[test]
    fn an_outer_hit_carries_its_inner_fragments_to_the_next_frame() {
        let mut heap = Heap::new();
        let line = text(&mut heap, 1, "inner");
        let inner_ops = heap.alloc_vector(vec![line]);
        let inner = marker(&mut heap, "line", inner_ops, false);
        let outer_ops = heap.alloc_vector(vec![inner]);
        let outer = marker(&mut heap, "pane", outer_ops, false);
        let mut pass = FragmentPass::empty();
        let first = parse(&heap, vec![outer], &mut pass);
        assert_eq!(pass.kept(), 2);
        let first_inner = shared(&first[0])[0].clone();

        // the whole pane reused: one hit, and the inner fragment rides along
        let outer_reused = marker(&mut heap, "pane", outer_ops, true);
        let mut pass = next(pass);
        parse(&heap, vec![outer_reused], &mut pass);
        assert_eq!(pass.kept(), 2);

        // the pane recomputed around the same row: the row is still last frame's parse
        let inner_reused = marker(&mut heap, "line", inner_ops, true);
        let other = text(&mut heap, 2, "new");
        let rebuilt_ops = heap.alloc_vector(vec![inner_reused, other]);
        let rebuilt = marker(&mut heap, "pane", rebuilt_ops, false);
        let mut pass = next(pass);
        let third = parse(&heap, vec![rebuilt], &mut pass);
        assert!(Arc::ptr_eq(
            shared(&first_inner),
            shared(&shared(&third[0])[0])
        ));
    }

    #[test]
    fn a_fragment_holding_a_sound_is_spliced_not_shared() {
        let mut heap = Heap::new();
        let sound = heap.alloc_vector(vec![Value::Keyword(value::intern("sound")), Value::int(3)]);
        let line = text(&mut heap, 0, "x");
        let ops = heap.alloc_vector(vec![sound, line]);
        let with_sound = marker(&mut heap, "beep", ops, false);
        let mut pass = FragmentPass::empty();
        let parsed = parse(&heap, vec![with_sound], &mut pass);
        assert!(matches!(parsed[0], Op::Sound { id: 3, .. }));
        assert!(matches!(parsed[1], Op::Text { .. }));
        assert_eq!(pass.kept(), 0);
    }
}
