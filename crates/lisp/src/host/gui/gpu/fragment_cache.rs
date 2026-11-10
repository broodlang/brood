//! The fragment cache: a memoised view fragment's instances recorded with the context it
//! was expanded in, and pushed whole on a later frame that draws the same allocation in
//! the same context (ADR-482).

use super::*;

// ---- the fragment cache: an unchanged fragment's quads replayed (ADR-482) ----------
//
// The run cache spares re-SHAPING an unchanged line, but every frame still walks every
// op: each text op hashes its string and face, looks its run up and replays its quads one
// by one through the builder. A memoised view fragment (ADR-470) arrives as the very
// allocation it arrived as last frame when nothing it depends on changed, so the window
// can know, by one pointer, that a whole pane body is what it drew last frame — and push
// the instance data it produced then, as a block, without looking inside.
//
// What a fragment's quads depend on besides its ops is the CONTEXT it is expanded in: the
// grid (origin, cell size, scroll shift), the clip band and the viewport (which quads were
// culled), the renderer's metrics, font, ground and subpixel order (what its runs shaped
// to), the scale factor (hairline snapping), which app textures exist (a sprite of a
// missing one draws nothing) and — for a fragment holding a blinking caret — the blink
// phase. All of it is the key (`FragmentContext`); any change misses and the fragment is
// expanded as before, so a theme, font, size or scroll change can never replay stale
// quads. Like the run cache it keeps exactly the fragments the last frame drew: an entry
// the next frame does not use is dropped when that frame ends.

/// Everything a fragment's quads depend on besides its ops — compared whole on a lookup.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct FragmentContext {
    /// the grid it is expanded on (`Grid`: left edge, row map, cell size, scroll shift), as
    /// bits
    pub(super) grid: [u32; 8],
    /// the clip band in effect and the viewport, which decide what was culled
    pub(super) clip: [u32; 4],
    pub(super) viewport: [u32; 2],
    /// the renderer's metrics (cell width, cell height, font px bits) and line height
    /// bits — a cell region inside derives its own metrics from these
    pub(super) metrics: [u32; 4],
    /// the resolved default family, and the window background (the ground a run's
    /// contrast lift is judged against)
    pub(super) family: u32,
    pub(super) ground: [u8; 3],
    /// the font registration, ligatures and subpixel order (`RunContext::fonts`)
    pub(super) fonts: (u64, bool, u8),
    /// the scale factor's bits: a hairline snaps to whole device pixels by it
    pub(super) scale: u64,
    /// how many times the app's textures were uploaded or freed
    pub(super) textures: u64,
    /// the blink phase — compared only for a fragment that holds a blinking caret
    pub(super) caret_hidden: bool,
}

/// A run of one fragment's instances drawn with one pipeline state, as `Batch` holds it
/// minus where the run starts (that is wherever the fragment lands next frame).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Segment {
    Solid { count: u32, linear: bool },
    Tex { key: TexKey, count: u32, lcd: bool },
    Particles { id: u32 },
}

/// One fragment's expansion: its instances in op order and the segments they batch into,
/// the text runs and nested fragments it used (carried forward with it on a hit, so a
/// later frame that changes the fragment still finds its unchanged lines shaped), and
/// the ops themselves — held so the allocation, whose address is the key, cannot be freed
/// and reused by another fragment while the entry lives.
pub(super) struct FragmentQuads {
    pub(super) ops: std::sync::Arc<[Op]>,
    pub(super) context: FragmentContext,
    pub(super) blinks: bool,
    pub(super) solids: Vec<f32>,
    pub(super) texs: Vec<u32>,
    pub(super) segments: Vec<Segment>,
    pub(super) runs: Vec<u64>,
    pub(super) nested: Vec<(usize, FragmentContext)>,
}

impl FragmentQuads {
    /// Whether this entry is the expansion of `ops` in `context`.
    pub(super) fn matches(&self, ops: &std::sync::Arc<[Op]>, context: &FragmentContext) -> bool {
        std::sync::Arc::ptr_eq(&self.ops, ops)
            && FragmentContext {
                caret_hidden: if self.blinks {
                    context.caret_hidden
                } else {
                    self.context.caret_hidden
                },
                ..*context
            } == self.context
    }
}

/// Where a fragment's expansion starts in the frame being built: what `record` cuts the
/// fragment's instances, segments, runs and nested fragments out of when it ends.
pub(super) struct FragmentMark {
    pub(super) solids: usize,
    pub(super) texs: usize,
    pub(super) batches: usize,
    /// the instance count of the batch the fragment's first quads may merge into
    pub(super) open_count: u32,
    pub(super) overlay_at: Option<usize>,
    pub(super) runs: usize,
    pub(super) nested: usize,
}

/// The fragments of the last frame and of the one being built, by the address of their
/// ops (a fragment drawn at two places has an entry per context), plus this frame's log
/// of the runs and fragments used — what an enclosing fragment's entry records.
#[derive(Default)]
pub(super) struct FragmentCache {
    pub(super) current: HashMap<usize, Vec<FragmentQuads>>,
    pub(super) previous: HashMap<usize, Vec<FragmentQuads>>,
    pub(super) runs_used: Vec<u64>,
    pub(super) fragments_used: Vec<(usize, FragmentContext)>,
    /// this frame's fragments replayed / expanded, for the trace
    pub(super) hits: u32,
    pub(super) misses: u32,
}

pub(super) fn fragment_address(ops: &std::sync::Arc<[Op]>) -> usize {
    ops.as_ptr() as *const () as usize
}

impl FragmentCache {
    /// The entry for `ops` in `context`, out of this frame's (a fragment drawn twice) or the
    /// last frame's; the caller replays it and hands it back (`restore`).
    pub(super) fn take(
        &mut self,
        ops: &std::sync::Arc<[Op]>,
        context: &FragmentContext,
    ) -> Option<FragmentQuads> {
        let address = fragment_address(ops);
        for generation in [&mut self.current, &mut self.previous] {
            if let Some(entries) = generation.get_mut(&address)
                && let Some(at) = entries.iter().position(|e| e.matches(ops, context))
            {
                return Some(entries.swap_remove(at));
            }
        }
        None
    }

    /// Keep `entry` for the next frame.
    pub(super) fn restore(&mut self, entry: FragmentQuads) {
        let address = fragment_address(&entry.ops);
        self.current.entry(address).or_default().push(entry);
    }

    /// Carry entry `(address, context)` from the last frame into this one without
    /// replaying it — a fragment nested in one that was replayed whole.
    pub(super) fn carry(&mut self, address: usize, context: &FragmentContext) {
        let in_current = self
            .current
            .get(&address)
            .is_some_and(|entries| entries.iter().any(|e| e.context == *context));
        if in_current {
            return;
        }
        if let Some(entries) = self.previous.get_mut(&address)
            && let Some(at) = entries.iter().position(|e| e.context == *context)
        {
            let entry = entries.remove(at);
            self.current.entry(address).or_default().push(entry);
        }
    }

    /// Start recording a fragment's expansion into `fb`.
    pub(super) fn mark(&self, fb: &FrameBuilder) -> FragmentMark {
        let open_count = match fb.batches.last() {
            Some(Batch::Solid { count, .. }) | Some(Batch::Tex { count, .. }) => *count,
            _ => 0,
        };
        FragmentMark {
            solids: fb.solids.len(),
            texs: fb.texs.len(),
            batches: fb.batches.len(),
            open_count,
            overlay_at: fb.overlay_at,
            runs: self.runs_used.len(),
            nested: self.fragments_used.len(),
        }
    }

    /// Finish recording: cut what the fragment pushed since `mark` out of `fb` and the
    /// logs into this frame's entry for `ops` in `context`. A fragment that set the frame's
    /// overlay point is not kept (its replay could not set it again); the expanders splice
    /// such a fragment anyway (`fragments::frame_global`).
    pub(super) fn record(
        &mut self,
        fb: &FrameBuilder,
        mark: FragmentMark,
        ops: &std::sync::Arc<[Op]>,
        context: FragmentContext,
        blinks: bool,
    ) {
        if fb.overlay_at != mark.overlay_at {
            return;
        }
        let solids = fb.solids[mark.solids..].to_vec();
        let texs = fb.texs[mark.texs..].to_vec();
        // A solid run is linear when any of its quads is translucent — read off the
        // instances, since the batch it merged into may have been linear already.
        let translucent = |from: usize, count: u32| {
            solids[from * SOLID_FLOATS..(from + count as usize) * SOLID_FLOATS]
                .chunks(SOLID_FLOATS)
                .any(|quad| quad[7] < 1.0)
        };
        let mut segments = Vec::new();
        let mut solid_at = 0usize;
        let mut push = |batch: Batch, count: u32| match batch {
            Batch::Solid { .. } => {
                segments.push(Segment::Solid {
                    count,
                    linear: translucent(solid_at, count),
                });
                solid_at += count as usize;
            }
            Batch::Tex { key, lcd, .. } => segments.push(Segment::Tex { key, count, lcd }),
            Batch::Particles { id } => segments.push(Segment::Particles { id }),
        };
        // the fragment's first quads may have joined the batch that was open
        if mark.batches > 0 {
            let open = &fb.batches[mark.batches - 1];
            let grown = match open {
                Batch::Solid { count, .. } | Batch::Tex { count, .. } => count - mark.open_count,
                Batch::Particles { .. } => 0,
            };
            if grown > 0 {
                push(*open, grown);
            }
        }
        for batch in &fb.batches[mark.batches..] {
            let count = match batch {
                Batch::Solid { count, .. } | Batch::Tex { count, .. } => *count,
                Batch::Particles { .. } => 0,
            };
            push(*batch, count);
        }
        let entry = FragmentQuads {
            ops: ops.clone(),
            context,
            blinks,
            solids,
            texs,
            segments,
            runs: self.runs_used[mark.runs..].to_vec(),
            nested: self.fragments_used[mark.nested..].to_vec(),
        };
        let address = fragment_address(ops);
        self.fragments_used.push((address, context));
        let entries = self.current.entry(address).or_default();
        entries.retain(|e| e.context != context);
        entries.push(entry);
    }

    /// The frame is drawn: what it used is the next frame's to reuse, and nothing else.
    pub(super) fn end_frame(&mut self) {
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        self.runs_used.clear();
        self.fragments_used.clear();
        self.hits = 0;
        self.misses = 0;
    }
}

/// Push a recorded fragment's instances onto the frame: each segment joins the open
/// batch when one quad of it would have (`FrameBuilder::push_solids` / `push_texs`), so
/// the batches are what expanding the fragment's ops here would have built.
pub(super) fn replay_fragment(fb: &mut FrameBuilder, entry: &FragmentQuads) {
    let (mut solid_at, mut tex_at) = (0usize, 0usize);
    for segment in &entry.segments {
        match *segment {
            Segment::Solid { count, linear } => {
                let end = solid_at + count as usize;
                fb.push_solids(
                    &entry.solids[solid_at * SOLID_FLOATS..end * SOLID_FLOATS],
                    linear,
                );
                solid_at = end;
            }
            Segment::Tex { key, count, lcd } => {
                let end = tex_at + count as usize;
                fb.push_texs(key, lcd, &entry.texs[tex_at * TEX_WORDS..end * TEX_WORDS]);
                tex_at = end;
            }
            Segment::Particles { id } => fb.batches.push(Batch::Particles { id }),
        }
    }
}
