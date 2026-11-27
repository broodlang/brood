//! Incremental paints that must land on a from-scratch raster's pixels, byte for byte: a
//! window resized (the old canvas kept, ADR-739), the panes of a side-by-side split scrolled
//! one at a time and opposite ways (a copy over each pane's columns, ADR-746), a keystroke
//! and a scroll in a zoomed pane (a cell region diffed and copied on its own grid), a zoomed
//! pane moved and resized (its old pixels copied to its new place, ADR-749), a pane scrolled
//! sideways, alone, beside another and down at once (a copy by a two-way offset, ADR-749), and
//! a picture drawn on threads. Each also says it did less than a whole-frame raster, or the
//! shortcut is untested.

use super::test_frames::{band, grid, paint_at, text_in};
use super::*;
use crate::host::gui::{CursorStyle, Face, Op};

fn renderer() -> Renderer {
    Renderer::new(1.0, default_families(), 14.0)
}

/// Text `s` at cell `(row, col)` in the tests' one gray.
fn text_at(row: u16, col: u16, s: &str) -> Op {
    text_in(row, col, s, [200, 200, 200])
}

const WORDS: [&str; 10] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
];

/// One step of a run: the frame, the window's size, and the blink phase of its caret.
struct Step {
    frame: Vec<Op>,
    size: (usize, usize),
    caret_hidden: bool,
}

fn step(frame: Vec<Op>, size: (usize, usize)) -> Step {
    Step {
        frame,
        size,
        caret_hidden: false,
    }
}

/// What one incremental paint did: the pixels it drew and the rows it copied.
#[derive(Debug, Clone, Copy)]
struct Work {
    drawn: usize,
    blit_rows: usize,
}

/// Paint `steps` in order on one renderer, incrementally, each checked against a from-scratch
/// raster of that frame at that size, layout and blink phase.
fn run(steps: &[Step]) -> Vec<Work> {
    let mut incremental = renderer();
    let mut work = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        incremental.caret_hidden = step.caret_hidden;
        paint_at(&mut incremental, &step.frame, step.size, false);
        let mut full = renderer();
        full.caret_hidden = step.caret_hidden;
        paint_at(&mut full, &step.frame, step.size, true);
        assert!(
            incremental.retained.canvas == full.retained.canvas,
            "step {index}: the incremental raster differs from a full one"
        );
        work.push(Work {
            drawn: incremental.retained.drawn_px,
            blit_rows: incremental.retained.blit_rows,
        });
    }
    work
}

/// An editor's frame for a `columns`×`rows` grid: lines, a current-line band, a caret, a
/// mode line declared as the seam (so the rows under it are anchored to the bottom), and an
/// echo line.
fn editor(columns: usize, rows: usize) -> Vec<Op> {
    let body = (rows - 2) as u16;
    let mut ops = vec![
        Op::Clear,
        Op::Seam {
            row: Some(body),
            top_band: true,
        },
        band(1, 0, columns as u16, 1, [40, 40, 52]),
    ];
    for row in 0..body {
        let word = WORDS[row as usize % WORDS.len()];
        ops.push(text_at(row, 0, &format!("{row:2} {word} {word}")));
    }
    ops.push(Op::Cursor {
        row: 1,
        col: 4,
        style: CursorStyle::Block,
        color: None,
        blink: None,
        under: None,
    });
    ops.push(band(body, 0, columns as u16, 1, [60, 60, 90]));
    ops.push(text_at(
        body,
        1,
        &format!("-:--- notes.txt {columns}x{rows}"),
    ));
    ops.push(text_at(body + 1, 0, "echo"));
    ops
}

#[test]
fn a_resized_window_keeps_its_canvas_and_draws_what_is_new() {
    let probe = renderer();
    let (cw, ch) = (probe.cell_w, probe.cell_h);
    let small = (60 * cw + 5, 30 * ch + 3);
    let large = (62 * cw + 2, 31 * ch + 7); // a drag: two columns and a row more
    let taller = (large.0, large.1 + ch / 2); // the same grid, a deeper leftover
    let narrow = (49 * cw + 1, 27 * ch + 1);
    let laid_out = |size: (usize, usize)| {
        let (columns, rows) = grid(&probe, size.0, size.1);
        editor(columns, rows)
    };
    let steps = [
        step(laid_out(small), small),
        // the configure: the frame laid out for the old size, painted at the new one
        step(laid_out(small), large),
        // then the app's frame for the new grid
        step(laid_out(large), large),
        step(laid_out(large), taller),
        step(laid_out(taller), taller),
        // smaller, both ways
        step(laid_out(taller), narrow),
        step(laid_out(narrow), narrow),
    ];
    let work = run(&steps);
    let area = |(width, height): (usize, usize)| width * height;
    for (index, limit) in [(1, area(large)), (2, area(large)), (3, area(taller))] {
        assert!(
            work[index].drawn * 2 < limit,
            "step {index} drew {} px of {limit}: {work:?}",
            work[index].drawn
        );
    }
}

/// Two panes side by side, each scrolled to its own top line, a divider between them.
fn split(left: usize, right: usize) -> Vec<Op> {
    let mut ops = vec![Op::Clear];
    for row in 0..7u16 {
        let line = left + row as usize;
        ops.push(text_at(row, 0, &format!("{line} {}", WORDS[line % 10])));
    }
    ops.push(band(0, 13, 1, 8, [80, 80, 80]));
    for row in 0..7u16 {
        let line = right + row as usize;
        ops.push(text_at(
            row,
            14,
            &format!("{} {line}", WORDS[(line + 3) % 10]),
        ));
    }
    ops.push(text_at(7, 0, &format!("L{left} | L{right}")));
    ops
}

#[test]
fn one_pane_of_a_split_scrolls_by_copying_its_columns() {
    let probe = renderer();
    let size = (28 * probe.cell_w, 9 * probe.cell_h);
    let steps = [
        step(split(0, 0), size),
        step(split(1, 0), size), // the left pane, one line
        step(split(3, 0), size), // two at once
        step(split(3, 2), size), // the right pane alone
        // both, opposite ways: one strip, two deltas, each pane its own move
        step(split(2, 3), size),
        step(split(2, 3), size),
        step(split(4, 1), size), // both again, two lines each
    ];
    let work = run(&steps);
    for index in [1, 2, 3, 4, 6] {
        assert!(
            work[index].blit_rows >= 4 * probe.cell_h,
            "step {index} copied too little: {work:?}"
        );
    }
    assert_eq!(work[5].drawn, 0, "an identical frame drew: {work:?}");
    // opposite ways, each pane copies all but the line it scrolled in: two strips of a pane
    // (one line each) drawn, not the panes
    let pane_area = 13 * probe.cell_w * 7 * probe.cell_h;
    assert!(
        work[4].drawn * 2 < pane_area,
        "the opposite-ways scroll drew {} px: {work:?}",
        work[4].drawn
    );
}

/// A zoomed pane: a cell region at 22 px over the left of the window, its line 2 carrying
/// `typed` and a blinking bar caret after it; beside it a plain pane and a mode line.
fn zoomed(typed: &str, scrolled: f32) -> Vec<Op> {
    let mut inner = vec![band(0, 0, 3, 5, [30, 30, 40])];
    for row in 0..5u16 {
        inner.push(text_at(row, 0, &format!("{row} {}", WORDS[row as usize])));
    }
    inner.push(Op::ScrollRegion {
        dy_frac: scrolled,
        ops: vec![text_at(3, 8, "scrolled")],
    });
    inner.push(text_at(2, 8, typed));
    inner.push(Op::Cursor {
        row: 2,
        col: 8 + typed.len() as u16,
        style: CursorStyle::Bar,
        color: None,
        blink: Some(crate::host::gui::CursorBlink {
            interval_ms: 500,
            blinks: 10,
        }),
        under: None,
    });
    vec![
        Op::Clear,
        Op::CellRegion {
            x: 0,
            y: 0,
            w: 16,
            h: 7,
            px: 22.0,
            ops: inner,
        },
        band(0, 16, 1, 7, [80, 80, 80]),
        text_at(1, 17, "right pane"),
        text_at(7, 0, &format!("zoomed: {typed}")),
    ]
}

#[test]
fn a_keystroke_in_a_zoomed_pane_draws_the_cells_it_changed() {
    let probe = renderer();
    let size = (30 * probe.cell_w, 9 * probe.cell_h);
    let region_area = 16 * probe.cell_w * 7 * probe.cell_h;
    let mut blinked = step(zoomed("abcd", 0.0), size);
    blinked.caret_hidden = true;
    let steps = [
        step(zoomed("ab", 0.0), size),
        step(zoomed("abc", 0.0), size),
        step(zoomed("abcd", 0.0), size),
        blinked, // the caret's blink phase alone
        step(zoomed("abcd", 0.5), size),
        step(zoomed("", 0.5), size),
    ];
    let work = run(&steps);
    for index in 1..4 {
        assert!(
            work[index].drawn * 3 < region_area,
            "step {index} drew {} px of a {region_area} px region: {work:?}",
            work[index].drawn
        );
    }
}

/// A zoomed pane (a cell region at 22 px) showing its lines from `top`, a gutter wash down
/// its left and a current-line band on its row 1; beside it a plain pane, under it a mode
/// line saying where it is. With `popup`, a window-level box drawn over the pane's rows 2–3,
/// which does not scroll with them.
fn zoomed_lines(top: usize, popup: bool) -> Vec<Op> {
    let mut inner = vec![
        band(0, 0, 2, 10, [30, 30, 40]),
        band(1, 2, 10, 1, [40, 40, 52]),
    ];
    for row in 0..10u16 {
        let line = top + row as usize;
        inner.push(text_at(row, 0, &format!("{line:2} {}", WORDS[line % 10])));
    }
    let mut ops = vec![
        Op::Clear,
        Op::CellRegion {
            x: 0,
            y: 0,
            w: 16,
            h: 15,
            px: 22.0,
            ops: inner,
        },
        band(0, 16, 1, 15, [80, 80, 80]),
        text_at(1, 17, "right pane"),
        text_at(15, 0, &format!("zoomed L{top}")),
    ];
    if popup {
        ops.push(band(2, 3, 6, 2, [90, 60, 60]));
        ops.push(text_at(2, 4, "popup"));
    }
    ops
}

#[test]
fn a_scroll_in_a_zoomed_pane_copies_its_rows() {
    let probe = renderer();
    let size = (30 * probe.cell_w, 17 * probe.cell_h);
    let region_area = 16 * probe.cell_w * 15 * probe.cell_h;
    let steps = [
        step(zoomed_lines(0, false), size),
        step(zoomed_lines(1, false), size), // one line up
        step(zoomed_lines(3, false), size), // two at once
        step(zoomed_lines(2, false), size), // one back down
        step(zoomed_lines(2, false), size),
        // a box over the pane that stays put: the rows under it are not a translation there
        step(zoomed_lines(2, true), size),
        step(zoomed_lines(4, true), size),
        step(zoomed_lines(3, true), size),
    ];
    let work = run(&steps);
    // under the box, the rows it covers are drawn: the rest are still copied
    for (index, share) in [(1, 2), (2, 2), (3, 2), (6, 1), (7, 1)] {
        assert!(
            work[index].blit_rows > 0,
            "step {index} copied nothing: {work:?}"
        );
        assert!(
            work[index].drawn * share < region_area,
            "step {index} drew {} px of a {region_area} px region: {work:?}",
            work[index].drawn
        );
    }
    assert_eq!(work[4].drawn, 0, "an identical frame drew: {work:?}");
}

/// A zoomed pane (a cell region at 22 px) at the window cells `(x, y)`, `w`×`h` of them, over
/// nothing but the window's clear: a gutter wash, a current-line band and short lines that keep
/// to its smallest rect, line 2 carrying `typed`. Beside it a sidebar of the window's own, under
/// it a mode line; with `popup`, a window-level box at window cells (6, 8) that stays put.
fn zoomed_placed((x, y, w, h): (u16, u16, u16, u16), typed: &str, popup: bool) -> Vec<Op> {
    let mut inner = vec![
        band(0, 0, 2, 8, [30, 30, 40]),
        band(1, 2, 4, 1, [40, 40, 52]),
    ];
    for row in 0..8u16 {
        let word: String = WORDS[row as usize].chars().take(4).collect();
        inner.push(text_at(row, 0, &format!("{row} {word}")));
    }
    inner.push(text_at(2, 2, typed));
    let mut ops = vec![
        Op::Clear,
        Op::CellRegion {
            x,
            y,
            w,
            h,
            px: 22.0,
            ops: inner,
        },
    ];
    for row in 0..18u16 {
        ops.push(text_at(row, 26, &format!("s{row}")));
    }
    ops.push(text_at(19, 0, "zoomed"));
    if popup {
        ops.push(band(8, 6, 6, 2, [90, 60, 60]));
        ops.push(text_at(8, 7, "popup"));
    }
    ops
}

#[test]
fn a_zoomed_pane_moved_or_resized_copies_its_old_pixels() {
    let probe = renderer();
    let size = (30 * probe.cell_w, 20 * probe.cell_h);
    // each frame: the pane's rect in window cells, its typed line, and the box over it
    let frames: [((u16, u16, u16, u16), &str, bool); 12] = [
        ((2, 1, 16, 10), "", false),
        ((5, 1, 16, 10), "", false), // three columns right, over its own old pixels
        ((4, 3, 16, 10), "", false), // two rows down and a column left at once
        ((4, 3, 16, 10), "", false),
        ((4, 3, 18, 12), "", false), // grown: the old pixels anchored top-left, the edge drawn
        ((4, 3, 13, 8), "", false),  // shrunk: what it no longer covers drawn
        ((2, 2, 16, 11), "", false), // moved and grown at once
        // moved while a line in it changes: copied, then that line's cells drawn
        ((3, 1, 16, 11), "typed", false),
        // a box over the pane that stays put: the rows under it are no translation
        ((3, 1, 16, 11), "typed", true),
        ((5, 2, 16, 11), "typed", true),
        // past the window's bottom, and back
        ((5, 12, 16, 10), "", false),
        ((5, 2, 16, 10), "", false),
    ];
    let steps: Vec<Step> = frames
        .iter()
        .map(|&(rect, typed, popup)| step(zoomed_placed(rect, typed, popup), size))
        .collect();
    let work = run(&steps);
    // drawing the pane whole where it is and where it was: what each move did before
    let whole = |index: usize| {
        let area = |(_, _, w, h): (u16, u16, u16, u16)| {
            w as usize * probe.cell_w * h as usize * probe.cell_h
        };
        area(frames[index].0) + area(frames[index - 1].0)
    };
    // back from past the bottom, the rows it had there are copied; the rest of it is new
    for (index, share) in [
        (1, 2),
        (2, 2),
        (4, 2),
        (5, 2),
        (6, 2),
        (7, 2),
        (9, 2),
        (11, 1),
    ] {
        assert!(
            work[index].blit_rows > 0,
            "step {index} copied nothing: {work:?}"
        );
        assert!(
            work[index].drawn * share < whole(index),
            "step {index} drew {} px of {}: {work:?}",
            work[index].drawn,
            whole(index)
        );
    }
    assert_eq!(work[3].drawn, 0, "an identical frame drew: {work:?}");
}

/// A long line's spans `(column, text, fg)`, four faces in turn — a line of code wider than
/// any pane, so a sideways scroll shows another part of it; `plain`: one span, one face.
fn long_line(line: usize, plain: bool) -> Vec<(usize, String, [u8; 3])> {
    let faces = [
        [200, 120, 220],
        [120, 180, 250],
        [220, 220, 220],
        [150, 210, 130],
    ];
    if plain {
        let text: String = (0..12)
            .map(|word| WORDS[(line + word) % WORDS.len()])
            .collect::<Vec<_>>()
            .join(" ");
        return vec![(0, text, faces[2])];
    }
    let mut spans = Vec::new();
    let mut column = 0;
    for word in 0..12 {
        let text = format!("{}{}", WORDS[(line + word) % WORDS.len()], line % 7);
        let length = text.len();
        spans.push((column, text, faces[(line + word) % faces.len()]));
        column += length + 1;
    }
    spans
}

/// A pane at column `left`, `width` cells wide and 8 rows deep, showing lines from `top`
/// scrolled `hscroll` columns left: a three-cell gutter of line numbers that stays, then the
/// lines cut to what shows.
fn sideways_pane(
    left: usize,
    width: usize,
    (top, hscroll): (usize, usize),
    plain: bool,
) -> Vec<Op> {
    let mut ops = vec![band(0, left as u16, 3, 8, [30, 30, 40])];
    for row in 0..8u16 {
        let line = top + row as usize;
        ops.push(text_at(row, left as u16, &format!("{line:2} ")));
        super::test_frames::scrolled_sideways(
            row,
            &long_line(line, plain),
            hscroll,
            (left + 3, width - 3),
            &mut ops,
        );
    }
    ops
}

/// The window's text scrolled sideways: one pane across it (`right` `None`), or two side by
/// side, each `(top, hscroll)`, the left in four faces, the right in one; a mode line under.
fn sideways(left: (usize, usize), right: Option<(usize, usize)>) -> Vec<Op> {
    let mut ops = vec![Op::Clear];
    match right {
        None => ops.extend(sideways_pane(0, 30, left, false)),
        Some(right) => {
            ops.extend(sideways_pane(0, 14, left, false));
            ops.push(band(0, 14, 1, 8, [80, 80, 80]));
            ops.extend(sideways_pane(15, 15, right, true));
        }
    }
    ops.push(text_at(8, 0, "sideways"));
    ops
}

#[test]
fn a_pane_scrolled_sideways_copies_its_columns() {
    let probe = renderer();
    let size = (30 * probe.cell_w, 9 * probe.cell_h);
    let pane_area = 11 * probe.cell_w * 8 * probe.cell_h;
    let steps = [
        step(sideways((0, 0), None), size),
        step(sideways((0, 4), None), size), // the whole window, four columns
        step(sideways((0, 1), None), size), // three back
        step(sideways((0, 1), None), size),
        step(sideways((0, 0), Some((0, 0))), size),
        step(sideways((0, 3), Some((0, 0))), size), // the left pane of a split
        step(sideways((0, 3), Some((0, 5))), size), // the right one, in one face
        step(sideways((0, 1), Some((0, 7))), size), // both, opposite ways
        step(sideways((1, 3), Some((0, 7))), size), // down a line and sideways at once
        step(sideways((1, 3), Some((2, 5))), size), // the one-face pane, the same
        step(sideways((1, 6), Some((2, 2))), size), // both again, each the other way
        // another buffer in both: nothing to copy, every changed cell drawn
        step(sideways((40, 2), Some((60, 9))), size),
    ];
    let work = run(&steps);
    // half a pane where one pane moved; both moved, less than one
    for (index, share) in [
        (1, 2),
        (2, 2),
        (5, 2),
        (6, 2),
        (7, 1),
        (8, 1),
        (9, 1),
        (10, 1),
    ] {
        assert!(
            work[index].blit_rows >= 4 * probe.cell_h,
            "step {index} copied too little: {work:?}"
        );
        assert!(
            work[index].drawn * share < pane_area,
            "step {index} drew {} px of a {pane_area} px pane: {work:?}",
            work[index].drawn
        );
    }
    assert_eq!(work[3].drawn, 0, "an identical frame drew: {work:?}");
}

/// Paint `frames` in order on a renderer that draws its rects on `threads` threads, whatever
/// their size, and answer the canvas after each.
fn canvases(frames: &[Vec<Op>], size: (usize, usize), threads: usize) -> Vec<Vec<u32>> {
    let mut renderer = renderer();
    renderer.retained.threads = Some(threads);
    frames
        .iter()
        .map(|frame| {
            paint_at(&mut renderer, frame, size, false);
            let drawn = renderer.retained.drawn_px > 0;
            assert!(
                threads == 1 || !drawn || renderer.retained.bands > 1,
                "{threads} threads fell back to one: {} bands",
                renderer.retained.bands
            );
            renderer.retained.canvas.clone()
        })
        .collect()
}

/// Bands drawn on threads are the bytes one thread draws: full frames, a resize, a scroll, a
/// zoomed pane, a block caret over text, pixel text and a rounded panel — with more threads
/// than rows to share, and an odd count.
#[test]
fn a_frame_drawn_on_threads_is_the_frame_drawn_on_one() {
    let probe = renderer();
    let size = (40 * probe.cell_w + 3, 14 * probe.cell_h + 5);
    let mut decorated = editor(40, 14);
    decorated.push(Op::Cursor {
        row: 2,
        col: 3,
        style: CursorStyle::Block,
        color: None,
        blink: None,
        under: Some(("g".into(), Face::default())),
    });
    decorated.push(Op::TextPx {
        x: 37.5,
        y: 21.0,
        s: "pixel text".into(),
        face: Face {
            fg: Some([250, 200, 100]),
            ..Face::default()
        },
        align: crate::host::gui::Align::Left,
        px: 0.0,
    });
    decorated.push(Op::Rect {
        row: 4,
        col: 20,
        w: 12,
        h: 4,
        face: Face {
            bg: Some([70, 40, 90]),
            ..Face::default()
        },
        radius: 0.4,
    });
    let frames = vec![
        decorated.clone(),
        editor(40, 14),
        split(0, 0),
        split(1, 0),
        zoomed("ab", 0.0),
        zoomed("abc", 0.5),
        zoomed_placed((2, 1, 16, 10), "", false),
        zoomed_placed((5, 3, 18, 9), "", false),
        sideways((0, 0), Some((0, 0))),
        sideways((1, 3), Some((0, 5))),
        decorated,
    ];
    let single = canvases(&frames, size, 1);
    for threads in [2, 3, 8, 64] {
        assert!(
            canvases(&frames, size, threads) == single,
            "{threads} threads drew other bytes than one"
        );
    }
}

/// A 48×40 picture of stripes and a gradient, partly transparent.
fn picture() -> crate::host::picture::PictureRef {
    let rgba: Vec<u8> = (0..48 * 40)
        .flat_map(|index: usize| {
            let (x, y) = (index % 48, index / 48);
            [
                (x * 5) as u8,
                (y * 6) as u8,
                if x % 2 == 0 { 200 } else { 40 },
                (128 + y * 3) as u8,
            ]
        })
        .collect();
    crate::host::picture::PictureRef(
        crate::host::picture::from_rgba(48, 40, &rgba).expect("a picture"),
    )
}

/// An editor's frame with a picture in a block at `(row, 22)`, 10×4 cells, text drawn over it.
fn with_picture(row: u16) -> Vec<Op> {
    let mut frame = editor(40, 14);
    frame.push(Op::Image {
        row,
        col: 22,
        w: 10,
        h: 4,
        picture: picture(),
        fit: crate::host::picture::Fit::Contain,
        anchor: crate::host::picture::Anchor::Center,
    });
    frame.push(text_at(row + 1, 20, "over the picture"));
    frame
}

/// A frame holding a picture is drawn on threads all the same: the bands draw everything but
/// the picture's rows, which the GUI thread draws after them — the bytes one thread draws,
/// and only those rows drawn twice.
#[test]
fn a_picture_on_threads_leaves_only_its_rows_to_the_gui_thread() {
    let probe = renderer();
    let size = (40 * probe.cell_w + 3, 14 * probe.cell_h + 5);
    let frames = vec![
        with_picture(3),
        with_picture(5),
        editor(40, 14),
        with_picture(8),
    ];
    let single = canvases(&frames, size, 1);
    for threads in [2, 3, 8] {
        assert!(
            canvases(&frames, size, threads) == single,
            "{threads} threads drew other bytes than one"
        );
    }
    let mut r = renderer();
    r.retained.threads = Some(4);
    paint_at(&mut r, &frames[0], size, true);
    let picture_rows = 4 * probe.cell_h;
    assert!(
        r.retained.drawn_again_px > 0 && r.retained.drawn_again_px <= picture_rows * size.0,
        "drew {} px again, the picture's rows are {} px",
        r.retained.drawn_again_px,
        picture_rows * size.0
    );
}
