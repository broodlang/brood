//! EDGE BLEED: a background that reaches the edge of the cell grid continues to the edge
//! of the window.
//!
//! The grid rarely fills the window. The inset sits on the left, right and bottom (row 0 is
//! at the top edge, `Renderer::grid_origin`), and the sub-cell remainder (up to a cell less
//! one pixel) lands on the right and bottom. Painted with
//! the window background alone, that margin cuts every full-width band short of the
//! frame: a mode line, a current-line highlight or a gutter stops a few pixels from the
//! edge, which reads as unfinished. Emacs paints its mode line and fringes through the
//! frame's leftover pixels, and terminals offer the same (Ghostty's
//! `window-padding-color = extend`).
//!
//! It is done as a frame rewrite, before either painter sees the frame. Every opaque,
//! square background op that touches a grid edge gains `FRect`s covering the margin
//! beside it, in cell units measured from the same origin, so the CPU and GPU painters,
//! the CPU's strip damage and the GPU's fragment cache all treat the margin as part
//! of the frame without knowing about it.
//!
//! Text backgrounds do not bleed: a run's width is the shaper's to know. A rounded or
//! translucent fill does not either (a pill or a wash is not a band).
//! Inside a `ScrollRegion` only the sides bleed, since its rows move. A `CellRegion` paints
//! at its own metrics and is left alone. A `Fragment` is bled inside, and the result kept by
//! the fragment's address ([`Bleed`]), so an unchanged fragment stays one allocation.

use super::render::Renderer;
use crate::host::gui::{Face, FragmentOps, Op};
use std::collections::HashMap;
use std::sync::Arc;

/// How far a fill must be from an edge, in cells, to count as touching it: the ops are
/// whole cells or app-computed floats, so anything closer is the edge.
const TOUCH: f32 = 1e-3;

/// The grid and the margin around it, in CELL units: what [`edge_bleed`] needs to place
/// the margin pieces, and the key that says when a bled frame is stale.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Margins {
    pub(super) cols: f32,
    pub(super) rows: f32,
    pub(super) left: f32,
    pub(super) right: f32,
    pub(super) bottom: f32,
}

impl Margins {
    /// The margins of a `w_px`×`h_px` window painted by `r`: the grid
    /// [`grid_cells`] gives, placed at [`Renderer::grid_origin`].
    pub(super) fn of(r: &Renderer, w_px: usize, h_px: usize) -> Margins {
        let (cw, ch) = (r.cell_w.max(1), r.cell_h.max(1));
        let (cols, rows) = grid_cells(r, w_px, h_px);
        let (ox, oy) = r.grid_origin(w_px, h_px);
        let (grid_w, grid_h) = (cols as usize * cw, rows as usize * ch);
        let cells = |px: usize, cell: usize| px as f32 / cell as f32;
        Margins {
            cols: cols as f32,
            rows: rows as f32,
            left: cells(ox, cw),
            right: cells(w_px.saturating_sub(ox + grid_w), cw),
            bottom: cells(h_px.saturating_sub(oy + grid_h), ch),
        }
    }
}

/// The cell grid a `w_px`×`h_px` window holds: the window less the inset on the left, right
/// and bottom (none on top: row 0 starts at the window's top edge, [`Renderer::grid_origin`]),
/// in whole cells, at least one each way. What the app is told its size is.
pub(super) fn grid_cells(r: &Renderer, w_px: usize, h_px: usize) -> (u16, u16) {
    let inset = r.inset();
    let usable_w = w_px.saturating_sub(2 * inset);
    let usable_h = h_px.saturating_sub(inset);
    let cols = (usable_w / r.cell_w.max(1)).max(1).min(u16::MAX as usize) as u16;
    let rows = (usable_h / r.cell_h.max(1)).max(1).min(u16::MAX as usize) as u16;
    (cols, rows)
}

/// A window's bled frame, kept until the frame or the margins change, and the bled
/// version of every FRAGMENT it held (ADR-336: a memoised view piece, which arrives as
/// the same allocation every frame its deps leave alone).
///
/// The fragments are why this is a cache rather than a function. A fragment is matched
/// by its address, here and in the GPU painter's fragment cache, so a fragment rewritten
/// afresh each frame would be a new allocation each frame and every one of them a GPU
/// cache miss. Kept by the source's address, an unchanged fragment maps to the SAME bled
/// allocation frame after frame. An entry holds its source, so the address cannot be
/// freed and reused by another fragment while the entry lives; entries a frame did not
/// use are dropped when it is bled.
#[derive(Default)]
pub(super) struct Bleed {
    margins: Option<Margins>,
    /// `Some(None)`: bled, and nothing reached an edge. `None`: not bled yet.
    frame: Option<Option<Vec<Op>>>,
    fragments: HashMap<(usize, bool), Bled>,
}

struct Bled {
    source: Arc<[Op]>,
    bled: Option<Arc<[Op]>>,
}

impl Bleed {
    /// A new frame arrived: bleed it at the next paint. The fragments are kept, since
    /// most of the new frame's are the last one's.
    pub(super) fn invalidate(&mut self) {
        self.frame = None;
    }

    /// `frame` with the margin pieces of every edge-touching background inserted right
    /// after the op they continue, as the window with margins `m` paints it — `frame`
    /// itself when nothing reaches an edge.
    pub(super) fn frame<'a>(&'a mut self, frame: &'a [Op], m: Margins) -> &'a [Op] {
        if self.margins != Some(m) {
            self.margins = Some(m);
            self.fragments.clear();
            self.frame = None;
        }
        if self.frame.is_none() {
            let mut used = HashMap::new();
            let bled = bleed_ops(frame, &m, true, &mut self.fragments, &mut used);
            self.fragments = used;
            self.frame = Some(bled);
        }
        match &self.frame {
            Some(Some(bled)) => bled,
            _ => frame,
        }
    }
}

/// `frame` with the margin pieces of every edge-touching background inserted right after
/// the op they continue, or `None` when nothing reaches an edge (the frame paints as is,
/// and nothing was copied). Without a cache: every fragment is bled afresh — what a
/// one-off picture of a frame (`offscreen`, ADR-609) wants.
pub(super) fn edge_bleed(frame: &[Op], m: &Margins) -> Option<Vec<Op>> {
    bleed_ops(frame, m, true, &mut HashMap::new(), &mut HashMap::new())
}

/// Bleed `frame`; a fragment is looked up by address (and whether it may bleed up and
/// down) in `known`, bled when it is not there, and recorded in `used`.
fn bleed_ops(
    frame: &[Op],
    m: &Margins,
    vertical: bool,
    known: &mut HashMap<(usize, bool), Bled>,
    used: &mut HashMap<(usize, bool), Bled>,
) -> Option<Vec<Op>> {
    let mut out: Option<Vec<Op>> = None;
    for (i, op) in frame.iter().enumerate() {
        let (replaced, pieces) = match op {
            Op::ScrollRegion { dy_frac, ops } => (
                bleed_ops(ops, m, false, known, used).map(|ops| Op::ScrollRegion {
                    dy_frac: *dy_frac,
                    ops,
                }),
                Vec::new(),
            ),
            Op::Fragment(fragment) => (
                bleed_fragment(&fragment.0, m, vertical, known, used)
                    .map(|bled| Op::Fragment(FragmentOps(bled))),
                Vec::new(),
            ),
            _ => (None, band(op).map_or(Vec::new(), |b| b.pieces(m, vertical))),
        };
        if replaced.is_none() && pieces.is_empty() {
            if let Some(out) = out.as_mut() {
                out.push(op.clone());
            }
            continue;
        }
        let out = out.get_or_insert_with(|| {
            let mut v = Vec::with_capacity(frame.len() + 8);
            v.extend_from_slice(&frame[..i]);
            v
        });
        out.push(replaced.unwrap_or_else(|| op.clone()));
        out.extend(pieces);
    }
    out
}

/// A fragment's bled ops, `None` when nothing in it reaches an edge: the entry for its
/// address when there is one for this very allocation, else bled now.
fn bleed_fragment(
    source: &Arc<[Op]>,
    m: &Margins,
    vertical: bool,
    known: &mut HashMap<(usize, bool), Bled>,
    used: &mut HashMap<(usize, bool), Bled>,
) -> Option<Arc<[Op]>> {
    let key = (Arc::as_ptr(source) as *const Op as usize, vertical);
    let hit = |entry: &Bled| Arc::ptr_eq(&entry.source, source).then(|| entry.bled.clone());
    if let Some(bled) = used.get(&key).and_then(hit) {
        return bled;
    }
    let bled = match known.remove(&key).as_ref().and_then(hit) {
        Some(bled) => {
            carry_nested(source, vertical, known, used);
            bled
        }
        None => bleed_ops(source, m, vertical, known, used).map(Arc::from),
    };
    used.insert(
        key,
        Bled {
            source: source.clone(),
            bled: bled.clone(),
        },
    );
    bled
}

/// Keep the entries of the fragments nested in a reused one: it was not walked, so its
/// fragments were not looked up, and the frame that next changes it would find them gone.
fn carry_nested(
    ops: &[Op],
    vertical: bool,
    known: &mut HashMap<(usize, bool), Bled>,
    used: &mut HashMap<(usize, bool), Bled>,
) {
    for op in ops {
        match op {
            Op::ScrollRegion { ops, .. } => carry_nested(ops, false, known, used),
            Op::Fragment(fragment) => {
                let key = (Arc::as_ptr(&fragment.0) as *const Op as usize, vertical);
                if let Some(entry) = known.remove(&key) {
                    carry_nested(&entry.source, vertical, known, used);
                    used.insert(key, entry);
                }
            }
            _ => {}
        }
    }
}

/// An opaque, square background in cell units: `[x0, x1) × [y0, y1)` in `rgb`.
struct Band {
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    rgb: [u8; 3],
}

/// The band `op` fills, when it is one that may bleed.
fn band(op: &Op) -> Option<Band> {
    let fill = |face: &Face| if face.reverse { face.fg } else { face.bg };
    match op {
        Op::Rect {
            row,
            col,
            w,
            h,
            face,
            radius,
        } if *radius <= 0.0 && *w > 0 && *h > 0 => Some(Band {
            x0: *col as f32,
            x1: *col as f32 + *w as f32,
            y0: *row as f32,
            y1: *row as f32 + *h as f32,
            rgb: fill(face)?,
        }),
        Op::FRect {
            x,
            y,
            w,
            h,
            face,
            opacity,
            radius,
        } if *opacity >= 1.0 && *radius <= 0.0 && *w > 0.0 && *h > 0.0 => Some(Band {
            x0: *x,
            x1: *x + *w,
            y0: *y,
            y1: *y + *h,
            rgb: fill(face)?,
        }),
        _ => None,
    }
}

impl Band {
    /// The `FRect`s that carry this band over the margins it touches: the side pieces at
    /// its own rows, and (when `vertical`) the bottom piece, widened over a side
    /// margin the band also touches so a corner is covered too.
    fn pieces(&self, m: &Margins, vertical: bool) -> Vec<Op> {
        let left = m.left > 0.0 && self.x0 <= TOUCH;
        let right = m.right > 0.0 && self.x1 >= m.cols - TOUCH;
        let bottom = vertical && m.bottom > 0.0 && self.y1 >= m.rows - TOUCH;
        let piece = |x: f32, y: f32, w: f32, h: f32| Op::FRect {
            x,
            y,
            w,
            h,
            face: Face {
                bg: Some(self.rgb),
                ..Face::default()
            },
            opacity: 1.0,
            radius: 0.0,
        };
        let mut pieces = Vec::new();
        // clamped to the grid's rows only where rows are fixed: inside a scroll region a band
        // on the row gliding in sits past `rows`, and clamping it there left it no side piece
        let (y0, y1) = if vertical {
            (self.y0.max(0.0), self.y1.min(m.rows))
        } else {
            (self.y0, self.y1)
        };
        if y1 > y0 {
            if left {
                pieces.push(piece(-m.left, y0, m.left, y1 - y0));
            }
            if right {
                pieces.push(piece(m.cols, y0, m.right, y1 - y0));
            }
        }
        let x0 = if left { -m.left } else { self.x0.max(0.0) };
        let x1 = if right {
            m.cols + m.right
        } else {
            self.x1.min(m.cols)
        };
        if bottom {
            pieces.push(piece(x0, m.rows, x1 - x0, m.bottom));
        }
        pieces
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARGINS: Margins = Margins {
        cols: 80.0,
        rows: 24.0,
        left: 0.5,
        right: 0.75,
        bottom: 0.5,
    };

    fn rect(row: u16, col: u16, w: u16, h: u16, rgb: [u8; 3]) -> Op {
        Op::Rect {
            row,
            col,
            w,
            h,
            face: Face {
                bg: Some(rgb),
                ..Face::default()
            },
            radius: 0.0,
        }
    }

    /// The `[x y w h]` of every `FRect` in `ops`, in order.
    fn frects(ops: &[Op]) -> Vec<[f32; 4]> {
        ops.iter()
            .filter_map(|op| match op {
                Op::FRect { x, y, w, h, .. } => Some([*x, *y, *w, *h]),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_empty_rect_bleeds_nothing() {
        // paints nothing in the grid, so it must paint nothing in the margin either
        let frame = vec![rect(20, 0, 0, 1, [9, 9, 9]), rect(20, 0, 80, 0, [9, 9, 9])];
        assert!(edge_bleed(&frame, &MARGINS).is_none());
    }

    #[test]
    fn a_band_gliding_in_below_a_scroll_region_still_gets_its_sides() {
        // inside a scroll region rows move, so the row past the grid keeps its side pieces
        let band = Band {
            x0: 0.0,
            x1: 80.0,
            y0: 24.0,
            y1: 25.0,
            rgb: [9, 9, 9],
        };
        let moving = band.pieces(&MARGINS, false);
        assert_eq!(
            frects(&moving),
            vec![[-0.5, 24.0, 0.5, 1.0], [80.0, 24.0, 0.75, 1.0]]
        );
    }

    #[test]
    fn a_band_inside_the_grid_leaves_the_frame_alone() {
        let frame = vec![rect(3, 2, 10, 1, [1, 2, 3])];
        assert!(edge_bleed(&frame, &MARGINS).is_none());
    }

    #[test]
    fn a_full_width_row_continues_into_both_side_margins_after_itself() {
        // a mode line: row 20, every column
        let frame = vec![Op::Clear, rect(20, 0, 80, 1, [9, 9, 9]), Op::Clear];
        let bled = edge_bleed(&frame, &MARGINS).expect("bled");
        assert_eq!(
            frects(&bled),
            vec![[-0.5, 20.0, 0.5, 1.0], [80.0, 20.0, 0.75, 1.0]]
        );
        // inserted right after the rect they continue, so a later op still paints over them
        assert!(matches!(bled[0], Op::Clear));
        assert!(matches!(bled[1], Op::Rect { .. }));
        assert!(matches!(bled[4], Op::Clear));
    }

    #[test]
    fn a_band_on_the_left_edge_covers_the_bottom_corner() {
        // a gutter: column 0, every row
        let frame = vec![rect(0, 0, 4, 24, [5, 5, 5])];
        let bled = edge_bleed(&frame, &MARGINS).expect("bled");
        assert_eq!(
            frects(&bled),
            vec![[-0.5, 0.0, 0.5, 24.0], [-0.5, 24.0, 4.5, 0.5],]
        );
    }

    #[test]
    fn rounded_translucent_and_bare_fills_do_not_bleed() {
        let face = Face {
            bg: Some([1, 1, 1]),
            ..Face::default()
        };
        let frame = vec![
            Op::Rect {
                row: 0,
                col: 0,
                w: 80,
                h: 1,
                face,
                radius: 0.3,
            },
            Op::FRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 1.0,
                face,
                opacity: 0.5,
                radius: 0.0,
            },
            Op::Rect {
                row: 0,
                col: 0,
                w: 80,
                h: 1,
                face: Face::default(),
                radius: 0.0,
            },
        ];
        assert!(edge_bleed(&frame, &MARGINS).is_none());
    }

    #[test]
    fn a_reversed_face_bleeds_its_foreground() {
        let frame = vec![Op::Rect {
            row: 5,
            col: 70,
            w: 10,
            h: 1,
            face: Face {
                fg: Some([7, 8, 9]),
                reverse: true,
                ..Face::default()
            },
            radius: 0.0,
        }];
        let bled = edge_bleed(&frame, &MARGINS).expect("bled");
        assert!(matches!(
            &bled[1],
            Op::FRect { face, .. } if face.bg == Some([7, 8, 9])
        ));
    }

    #[test]
    fn inside_a_scroll_region_only_the_sides_bleed() {
        let frame = vec![Op::ScrollRegion {
            dy_frac: 0.5,
            ops: vec![rect(0, 0, 80, 1, [3, 3, 3])],
        }];
        let bled = edge_bleed(&frame, &MARGINS).expect("bled");
        let Op::ScrollRegion { ops, .. } = &bled[0] else {
            panic!("the region was not kept");
        };
        assert_eq!(
            frects(ops),
            vec![[-0.5, 0.0, 0.5, 1.0], [80.0, 0.0, 0.75, 1.0]]
        );
    }

    fn fragment(ops: Vec<Op>) -> Arc<[Op]> {
        Arc::from(ops)
    }

    /// The bled allocation each `Fragment` op in `ops` (the top level) points at.
    fn fragment_ptrs(ops: &[Op]) -> Vec<*const Op> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Fragment(f) => Some(Arc::as_ptr(&f.0) as *const Op),
                _ => None,
            })
            .collect()
    }

    /// bedit's mode line and gutter rows are memoised fragments (ADR-336): a band inside
    /// one bleeds like a band outside.
    #[test]
    fn a_band_inside_a_fragment_bleeds() {
        let line = fragment(vec![rect(20, 0, 80, 1, [9, 9, 9])]);
        let frame = vec![Op::Fragment(FragmentOps(line))];
        let bled = edge_bleed(&frame, &MARGINS).expect("bled");
        let Op::Fragment(inner) = &bled[0] else {
            panic!("the fragment was not kept");
        };
        assert_eq!(
            frects(&inner.0),
            vec![[-0.5, 20.0, 0.5, 1.0], [80.0, 20.0, 0.75, 1.0]]
        );
    }

    /// An unchanged fragment (the same allocation in the next frame) bleeds to the SAME
    /// allocation, so the GPU fragment cache, which matches by address, keeps hitting.
    /// Nested fragments inside a reused one keep theirs too.
    #[test]
    fn an_unchanged_fragment_keeps_its_bled_allocation_across_frames() {
        let inner = fragment(vec![rect(0, 0, 4, 1, [5, 5, 5])]);
        let outer = fragment(vec![
            Op::Fragment(FragmentOps(inner.clone())),
            rect(20, 0, 80, 1, [9, 9, 9]),
        ]);
        let mut bleed = Bleed::default();
        let frame1 = vec![Op::Fragment(FragmentOps(outer.clone()))];
        let first = fragment_ptrs(bleed.frame(&frame1, MARGINS));
        // a new frame holding the same fragment
        bleed.invalidate();
        let frame2 = vec![Op::Clear, Op::Fragment(FragmentOps(outer.clone()))];
        let second = fragment_ptrs(bleed.frame(&frame2, MARGINS));
        assert_eq!(first, second, "the reused fragment was bled afresh");
        // the outer changes, the inner it holds does not: the inner keeps its allocation
        let bled_inner = |bleed: &mut Bleed, frame: &[Op]| -> *const Op {
            let ops = bleed.frame(frame, MARGINS).to_vec();
            let Op::Fragment(outer) = &ops[0] else {
                panic!("no outer fragment");
            };
            fragment_ptrs(&outer.0)[0]
        };
        bleed.invalidate();
        let before = bled_inner(&mut bleed, &frame1);
        bleed.invalidate();
        let changed = fragment(vec![
            Op::Fragment(FragmentOps(inner.clone())),
            rect(21, 0, 80, 1, [9, 9, 9]),
        ]);
        let after = bled_inner(&mut bleed, &[Op::Fragment(FragmentOps(changed))]);
        assert_eq!(before, after, "the nested fragment was bled afresh");
    }

    /// A window resized (or a new inset or font) bleeds every fragment again.
    #[test]
    fn new_margins_bleed_the_frame_again() {
        let mut bleed = Bleed::default();
        let frame = vec![rect(20, 0, 80, 1, [9, 9, 9])];
        let wider = Margins {
            right: 0.25,
            ..MARGINS
        };
        assert_eq!(frects(bleed.frame(&frame, MARGINS))[1][2], 0.75);
        assert_eq!(frects(bleed.frame(&frame, wider))[1][2], 0.25);
    }

    #[test]
    fn no_margin_no_pieces() {
        let flush = Margins {
            left: 0.0,
            right: 0.0,
            bottom: 0.0,
            ..MARGINS
        };
        assert!(edge_bleed(&[rect(0, 0, 80, 24, [1, 1, 1])], &flush).is_none());
    }
}
