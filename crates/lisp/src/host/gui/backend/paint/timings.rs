//! A one-shot timing of the CPU painter's common frames at 1080p (`#[ignore]`d: it reads a
//! clock, so it asserts nothing about speed). Run it on purpose, once:
//!
//!     cargo test -p brood --features gui --lib paint_timings -- --ignored --nocapture
//!
//! Each scenario is timed three times and the fastest reported, after a paint that warms the
//! glyph cache. The frames are an editor's: a gutter, highlighted code in several faces, an
//! hl-line band, a caret, a divider and a mode line.

use super::test_frames::{band, grid, paint_at, scrolled_sideways, text_in};
use super::*;
use crate::host::gui::{CursorStyle, Op};
use std::time::Instant;

const WIDTH: usize = 1920;
const HEIGHT: usize = 1080;

/// One line of a pane: its number, then code in four faces, at column `col`.
fn line_ops(row: u16, col: u16, line: usize, width: usize, out: &mut Vec<Op>) {
    out.push(text_in(row, col, &format!("{line:4} "), [110, 110, 120]));
    let chunks = [
        ("    (defn ", [200, 120, 220]),
        ("function-", [120, 180, 250]),
        ("argument ", [220, 220, 220]),
        ("\"string\" ", [150, 210, 130]),
    ];
    let mut column = col as usize + 5;
    let mut index = line;
    while column + 12 < col as usize + width {
        let (text, colour) = chunks[index % chunks.len()];
        let s = format!("{text}{}", index % 97);
        let cells = s.chars().count();
        if column + cells > col as usize + width {
            break;
        }
        out.push(text_in(row, column as u16, &s, colour));
        column += cells;
        index += 3;
    }
}

/// A pane of `rows` lines from `top`, `width` cells wide at column `col`.
fn pane(col: u16, width: usize, rows: usize, top: usize, out: &mut Vec<Op>) {
    out.push(band(0, col, 5, rows as u16, [30, 32, 38]));
    for row in 0..rows {
        line_ops(row as u16, col, top + row, width, out);
    }
}

fn mode_line(row: u16, columns: usize, label: &str, out: &mut Vec<Op>) {
    out.push(band(row, 0, columns as u16, 1, [50, 50, 70]));
    out.push(text_in(
        row,
        1,
        &format!("-:--- {label}  (Brood) L12 Git:main"),
        [230, 230, 230],
    ));
}

/// One pane over the whole window, the mode line its seam, `typed` on row 3 and the caret
/// after it.
fn editor(columns: usize, rows: usize, top: usize, typed: &str) -> Vec<Op> {
    let body = rows - 2;
    let mut ops = vec![
        Op::Clear,
        Op::Seam {
            row: Some(body as u16),
            top_band: true,
        },
        band(3, 0, columns as u16, 1, [40, 40, 50]),
    ];
    pane(0, columns, body, top, &mut ops);
    ops.push(text_in(3, 120, typed, [250, 250, 250]));
    ops.push(Op::Cursor {
        row: 3,
        col: 120 + typed.len() as u16,
        style: CursorStyle::Block,
        color: None,
        blink: None,
        under: None,
    });
    mode_line(body as u16, columns, "editor.blsp", &mut ops);
    ops
}

/// Two panes side by side, scrolled to `left` and `right`.
fn split(columns: usize, rows: usize, left: usize, right: usize) -> Vec<Op> {
    let body = rows - 2;
    let half = columns / 2;
    let mut ops = vec![Op::Clear];
    pane(0, half - 1, body, left, &mut ops);
    ops.push(band(0, (half - 1) as u16, 1, body as u16 + 1, [80, 80, 80]));
    pane(half as u16, columns - half, body, right, &mut ops);
    mode_line(body as u16, columns, "left.blsp | right.blsp", &mut ops);
    ops
}

/// The left half zoomed (a cell region at 22 px) on its line `top`, its row 4 carrying `typed`.
fn zoomed(columns: usize, rows: usize, top: usize, typed: &str) -> Vec<Op> {
    let body = rows - 2;
    let half = columns / 2;
    let mut inner = Vec::new();
    let inner_rows = body * 15 / 22;
    let inner_columns = (half - 1) * 15 / 22;
    pane(0, inner_columns, inner_rows, top, &mut inner);
    inner.push(text_in(4, 5, &format!("(typed {typed})"), [250, 250, 250]));
    inner.push(Op::Cursor {
        row: 4,
        col: (12 + typed.len()) as u16,
        style: CursorStyle::Bar,
        color: None,
        blink: None,
        under: None,
    });
    let mut ops = vec![
        Op::Clear,
        Op::CellRegion {
            x: 0,
            y: 0,
            w: (half - 1) as u16,
            h: body as u16,
            px: 22.0,
            ops: inner,
        },
    ];
    ops.push(band(0, (half - 1) as u16, 1, body as u16 + 1, [80, 80, 80]));
    pane(half as u16, columns - half, body, 40, &mut ops);
    mode_line(body as u16, columns, "zoomed", &mut ops);
    ops
}

/// The fastest of three runs of `step`, each after `setup`, in milliseconds.
fn fastest(mut setup: impl FnMut() -> Renderer, mut step: impl FnMut(&mut Renderer)) -> f64 {
    (0..3)
        .map(|_| {
            let mut renderer = setup();
            let start = Instant::now();
            step(&mut renderer);
            start.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min)
}

fn renderer() -> Renderer {
    Renderer::new(1.0, default_families(), 15.0)
}

/// The time `from` then `to` takes when the renderer last painted `from` (both at `size`)
/// with a warm glyph cache.
fn change(from: &[Op], to: &[Op], size: (usize, usize)) -> f64 {
    fastest(
        || {
            let mut r = renderer();
            paint_at(&mut r, to, size, true);
            paint_at(&mut r, from, size, true);
            r
        },
        |r| paint_at(r, to, size, false),
    )
}

#[test]
#[ignore = "a clock reading, run on purpose"]
fn paint_timings() {
    let size = (WIDTH, HEIGHT);
    let probe = renderer();
    let (columns, rows) = grid(&probe, WIDTH, HEIGHT);
    let frame = editor(columns, rows, 0, "ab");
    let full = fastest(
        || {
            let mut r = renderer();
            paint_at(&mut r, &frame, size, true);
            r
        },
        |r| paint_at(r, &frame, size, true),
    );
    let full_gray = fastest(
        || {
            let mut r = renderer();
            r.set_text_aa(TextAa::Gray);
            paint_at(&mut r, &frame, size, true);
            r
        },
        |r| paint_at(r, &frame, size, true),
    );
    let keystroke = change(&frame, &editor(columns, rows, 0, "abc"), size);
    let split_scroll = change(
        &split(columns, rows, 0, 0),
        &split(columns, rows, 1, 0),
        size,
    );
    let small = (WIDTH - 37, HEIGHT - 45);
    let (small_columns, small_rows) = grid(&probe, small.0, small.1);
    let stale = editor(small_columns, small_rows, 0, "ab");
    let resize = fastest(
        || {
            let mut r = renderer();
            paint_at(&mut r, &frame, size, true);
            paint_at(&mut r, &stale, small, true);
            r
        },
        |r| {
            let start = Instant::now();
            paint_at(r, &stale, size, false);
            let stale_ms = start.elapsed().as_secs_f64() * 1000.0;
            let stale_px = r.retained.drawn_px;
            paint_at(r, &frame, size, false);
            eprintln!(
                "[paint-timings] resize: stale {stale_ms:.2} ms drew {stale_px} px, laid out drew {} px",
                r.retained.drawn_px
            );
        },
    );
    // the first harness's resize: no seam, so no row moves with the height
    let plain = |ops: Vec<Op>| -> Vec<Op> {
        ops.into_iter()
            .filter(|op| !matches!(op, Op::Seam { .. }))
            .collect()
    };
    let (plain_stale, plain_frame) = (plain(stale.clone()), plain(frame.clone()));
    let resize_plain = fastest(
        || {
            let mut r = renderer();
            paint_at(&mut r, &plain_frame, size, true);
            paint_at(&mut r, &plain_stale, small, true);
            r
        },
        |r| {
            paint_at(r, &plain_stale, size, false);
            paint_at(r, &plain_frame, size, false);
        },
    );
    let zoom_typing = change(
        &zoomed(columns, rows, 0, "ab"),
        &zoomed(columns, rows, 0, "abc"),
        size,
    );
    // the bookkeeping a paint does whatever it draws: the frame kept, both frames flattened
    let to_vec = fastest(renderer, |_| {
        std::hint::black_box(frame.to_vec());
    });
    let rows_map = probe.rows(WIDTH, HEIGHT);
    let strips = Strips::new(rows_map, HEIGHT);
    let flatten = fastest(renderer, |_| {
        std::hint::black_box(Leaves::of(&frame, &strips, false));
    });
    // the glyph cache probes of a full redraw (one per cell drawn), SipHash against FxHash
    let keys: Vec<super::super::render::GlyphKey> = (0..columns * rows)
        .map(|cell| {
            let character = char::from(b'!' + (cell % 90) as u8);
            (
                super::super::render::ClusterKey::Char(character),
                0,
                cell % 7 == 0,
                false,
                1,
                true,
                15.0f32.to_bits(),
            )
        })
        .collect();
    let sip: std::collections::HashMap<_, usize> = keys.iter().cloned().zip(0..).collect();
    let fx: super::super::render::GlyphMap<usize> = keys.iter().cloned().zip(0..).collect();
    let sip_ms = fastest(renderer, |_| {
        std::hint::black_box(keys.iter().filter(|key| sip.contains_key(key)).count());
    });
    let fx_ms = fastest(renderer, |_| {
        std::hint::black_box(keys.iter().filter(|key| fx.contains_key(key)).count());
    });
    eprintln!(
        "[paint-timings] {} glyph probes: SipHash {sip_ms:.3} ms, FxHash {fx_ms:.3} ms",
        keys.len()
    );
    eprintln!(
        "[paint-timings] {WIDTH}x{HEIGHT} grid {columns}x{rows}, {} ops: full redraw {full:.2} ms \
         (gray {full_gray:.2} ms), keystroke {keystroke:.2} ms, split scroll {split_scroll:.2} ms, \
         resize (stale + laid out) {resize:.2} ms (no seam {resize_plain:.2} ms), zoomed-pane keystroke {zoom_typing:.2} ms | \
         frame kept {to_vec:.3} ms, frame flattened {flatten:.3} ms",
        frame.len()
    );
}

/// A full redraw and a large resize at 1080p and 4K, drawn on one thread and on several
/// (`draw_rects`): what `THREADED_MIN_PX` is chosen from.
#[test]
#[ignore = "a clock reading, run on purpose"]
fn paint_thread_timings() {
    let threads = gui_threads();
    for (width, height) in [(1920usize, 1080usize), (3840, 2160)] {
        let size = (width, height);
        let probe = renderer();
        let (columns, rows) = grid(&probe, width, height);
        let frame = editor(columns, rows, 0, "ab");
        let small = (width * 2 / 3, height * 2 / 3);
        let (small_columns, small_rows) = grid(&probe, small.0, small.1);
        let stale = editor(small_columns, small_rows, 0, "ab");
        let on = |count: usize| {
            let full = fastest(
                || {
                    let mut r = renderer();
                    r.retained.threads = Some(count);
                    paint_at(&mut r, &frame, size, true);
                    r
                },
                |r| paint_at(r, &frame, size, true),
            );
            let resize = fastest(
                || {
                    let mut r = renderer();
                    r.retained.threads = Some(count);
                    paint_at(&mut r, &frame, size, true);
                    paint_at(&mut r, &stale, small, true);
                    r
                },
                |r| {
                    paint_at(r, &stale, size, false);
                    paint_at(r, &frame, size, false);
                },
            );
            (full, resize)
        };
        let (one_full, one_resize) = on(1);
        let (many_full, many_resize) = on(threads);
        let (four_full, four_resize) = on(4);
        eprintln!(
            "[paint-threads] {width}x{height}: full redraw {one_full:.2} ms on 1 thread, \
             {four_full:.2} on 4, {many_full:.2} on {threads}; resize from 2/3 size \
             {one_resize:.2} / {four_resize:.2} / {many_resize:.2} ms"
        );
    }
}

/// The draw of `changed` whole rows at 1080p on one thread and on several: where threading
/// starts to pay (`THREADED_MIN_PX`).
#[test]
#[ignore = "a clock reading, run on purpose"]
fn paint_thread_threshold() {
    let threads = gui_threads();
    let size = (WIDTH, HEIGHT);
    let probe = renderer();
    let (columns, rows) = grid(&probe, WIDTH, HEIGHT);
    let before = editor(columns, rows, 0, "ab");
    for changed in [2usize, 5, 10, 20] {
        // the pane scrolled by `changed` lines on its top rows only: those rows redrawn
        let mut after = before.clone();
        for op in after.iter_mut() {
            if let Op::Text { row, s, .. } = op
                && (*row as usize) < changed
            {
                s.push('~');
            }
        }
        let time = |count: usize| {
            fastest(
                || {
                    let mut r = renderer();
                    r.retained.threads = Some(count);
                    paint_at(&mut r, &after, size, true);
                    paint_at(&mut r, &before, size, true);
                    r
                },
                |r| paint_at(r, &after, size, false),
            )
        };
        let (one, many) = (time(1), time(threads));
        let mut r = renderer();
        paint_at(&mut r, &before, size, true);
        paint_at(&mut r, &after, size, false);
        eprintln!(
            "[paint-threshold] {changed} rows ({} px): {one:.2} ms on 1 thread, {many:.2} on {threads}",
            r.retained.drawn_px
        );
    }
}

/// The frames ADR-746 copies or shares out: both panes of a split scrolled opposite ways in
/// one frame, a scroll inside a zoomed pane, and a full redraw of a frame holding a picture
/// on threads — each with what it drew, copied and on how many bands.
#[test]
#[ignore = "a clock reading, run on purpose"]
fn paint_scroll_and_picture_timings() {
    use crate::host::picture::{Anchor, Fit, PictureRef, from_rgba};
    let size = (WIDTH, HEIGHT);
    let probe = renderer();
    let (columns, rows) = grid(&probe, WIDTH, HEIGHT);
    let report = |label: &str, from: &[Op], to: &[Op]| {
        let ms = change(from, to, size);
        let mut r = renderer();
        paint_at(&mut r, from, size, true);
        paint_at(&mut r, to, size, false);
        eprintln!(
            "[paint-scroll] {label}: {ms:.2} ms, drew {} px, copied {} rows",
            r.retained.drawn_px, r.retained.blit_rows
        );
    };
    report(
        "split, both panes opposite ways",
        &split(columns, rows, 5, 5),
        &split(columns, rows, 6, 4),
    );
    report(
        "split, one pane",
        &split(columns, rows, 5, 5),
        &split(columns, rows, 6, 5),
    );
    report(
        "zoomed-pane scroll",
        &zoomed(columns, rows, 0, "ab"),
        &zoomed(columns, rows, 1, "ab"),
    );
    // a 480×360 picture in a block over the right of the editor, drawn whole on threads
    let rgba: Vec<u8> = (0..480 * 360)
        .flat_map(|index: usize| [(index % 251) as u8, (index / 480 % 256) as u8, 90, 255])
        .collect();
    let picture = from_rgba(480, 360, &rgba).expect("a picture");
    let mut frame = editor(columns, rows, 0, "ab");
    frame.push(Op::Image {
        row: 6,
        col: (columns - 50) as u16,
        w: 40,
        h: 16,
        picture: PictureRef(picture),
        fit: Fit::Contain,
        anchor: Anchor::Center,
    });
    let threads = gui_threads();
    for count in [1, threads] {
        let ms = fastest(
            || {
                let mut r = renderer();
                r.retained.threads = Some(count);
                paint_at(&mut r, &frame, size, true);
                r
            },
            |r| paint_at(r, &frame, size, true),
        );
        let mut r = renderer();
        r.retained.threads = Some(count);
        paint_at(&mut r, &frame, size, true);
        eprintln!(
            "[paint-scroll] picture frame, full redraw on {count} threads: {ms:.2} ms, {} bands",
            r.retained.bands
        );
    }
}

/// Line `line` of code as spans in four faces, `length` cells long: wider than a pane, so a
/// sideways scroll shows another part of it.
fn code_spans(line: usize, length: usize) -> Vec<(usize, String, [u8; 3])> {
    let chunks = [
        ("(defn ", [200, 120, 220]),
        ("function-", [120, 180, 250]),
        ("argument ", [220, 220, 220]),
        ("\"string\" ", [150, 210, 130]),
    ];
    let mut spans = Vec::new();
    let (mut column, mut index) = (4, line);
    while column < length {
        let (text, colour) = chunks[index % chunks.len()];
        let text = format!("{text}{}", index % 97);
        column += 1;
        spans.push((column, text.clone(), colour));
        column += text.len();
        index += 3;
    }
    spans
}

/// A pane of `rows` lines from `top` at column `col`, `width` cells wide: a five-cell gutter
/// that stays, then lines longer than the pane scrolled `hscroll` columns left.
fn sideways_pane(
    col: usize,
    width: usize,
    rows: usize,
    (top, hscroll): (usize, usize),
    out: &mut Vec<Op>,
) {
    out.push(band(0, col as u16, 5, rows as u16, [30, 32, 38]));
    for row in 0..rows {
        let line = top + row;
        out.push(text_in(
            row as u16,
            col as u16,
            &format!("{line:4} "),
            [110, 110, 120],
        ));
        scrolled_sideways(
            row as u16,
            &code_spans(line, width * 2),
            hscroll,
            (col + 5, width - 5),
            out,
        );
    }
}

/// The window's panes scrolled: one over the whole window at `left`, or two side by side at
/// `left` and `right`, each `(top line, columns scrolled sideways)`.
fn sideways(
    columns: usize,
    rows: usize,
    left: (usize, usize),
    right: Option<(usize, usize)>,
) -> Vec<Op> {
    let body = rows - 2;
    let mut ops = vec![Op::Clear];
    match right {
        None => sideways_pane(0, columns, body, left, &mut ops),
        Some(right) => {
            let half = columns / 2;
            sideways_pane(0, half - 1, body, left, &mut ops);
            ops.push(band(0, (half - 1) as u16, 1, body as u16 + 1, [80, 80, 80]));
            sideways_pane(half, columns - half, body, right, &mut ops);
        }
    }
    mode_line(body as u16, columns, "sideways", &mut ops);
    ops
}

/// A zoomed pane (a cell region at 22 px) at the window cells `(x, y)`, `w`×`h` of them, over the
/// window's clear, showing `content` (its own columns × rows: lines that fit its smallest rect);
/// beside it a sidebar of the window's own 30 columns wide, under it the mode line.
fn zoomed_at(
    columns: usize,
    rows: usize,
    (x, y, w, h): (usize, usize, usize, usize),
    content: (usize, usize),
) -> Vec<Op> {
    let body = rows - 2;
    let mut inner = Vec::new();
    pane(0, content.0, content.1, 0, &mut inner);
    let mut ops = vec![Op::Clear];
    pane((columns - 30) as u16, 30, body, 40, &mut ops);
    ops.push(Op::CellRegion {
        x: x as u16,
        y: y as u16,
        w: w as u16,
        h: h as u16,
        px: 22.0,
        ops: inner,
    });
    mode_line(body as u16, columns, "zoomed", &mut ops);
    ops
}

/// The frames whose pixels moved sideways or with a zoomed pane: a pane scrolled sideways
/// (whole window, one of a split, both opposite ways, and down a line at once), and a zoomed
/// pane moved and resized — each with what it drew and copied.
#[test]
#[ignore = "a clock reading, run on purpose"]
fn paint_move_timings() {
    let size = (WIDTH, HEIGHT);
    let probe = renderer();
    let (columns, rows) = grid(&probe, WIDTH, HEIGHT);
    let report = |label: &str, from: &[Op], to: &[Op]| {
        let ms = change(from, to, size);
        let mut r = renderer();
        paint_at(&mut r, from, size, true);
        paint_at(&mut r, to, size, false);
        eprintln!(
            "[paint-move] {label}: {ms:.2} ms, drew {} px, copied {} rows",
            r.retained.drawn_px, r.retained.blit_rows
        );
    };
    report(
        "sideways, whole window, 4 columns",
        &sideways(columns, rows, (0, 10), None),
        &sideways(columns, rows, (0, 14), None),
    );
    report(
        "sideways, one pane of a split, 4 columns",
        &sideways(columns, rows, (0, 10), Some((0, 10))),
        &sideways(columns, rows, (0, 14), Some((0, 10))),
    );
    report(
        "sideways, both panes of a split, opposite ways",
        &sideways(columns, rows, (0, 10), Some((0, 14))),
        &sideways(columns, rows, (0, 14), Some((0, 10))),
    );
    report(
        "a line down and 4 columns sideways at once, whole window",
        &sideways(columns, rows, (0, 10), None),
        &sideways(columns, rows, (1, 14), None),
    );
    // nothing moved: what trying the copies costs a frame that draws everything
    report(
        "another buffer in the window, nothing to copy",
        &sideways(columns, rows, (0, 10), None),
        &sideways(columns, rows, (300, 0), None),
    );
    report(
        "another buffer in both panes of a split, nothing to copy",
        &sideways(columns, rows, (0, 10), Some((40, 0))),
        &sideways(columns, rows, (300, 0), Some((500, 3))),
    );
    let (w, h) = (columns / 2, rows * 2 / 3);
    // lines that fit the smallest rect, enough of them to fill the largest
    let content = ((w - 4) * 15 / 22 - 3, (h + 2) * 15 / 22 + 1);
    report(
        "zoomed pane moved 6 columns right",
        &zoomed_at(columns, rows, (10, 4, w, h), content),
        &zoomed_at(columns, rows, (16, 4, w, h), content),
    );
    report(
        "zoomed pane moved 3 rows down, 2 columns left",
        &zoomed_at(columns, rows, (10, 4, w, h), content),
        &zoomed_at(columns, rows, (8, 7, w, h), content),
    );
    report(
        "zoomed pane grown 4 columns, 2 rows",
        &zoomed_at(columns, rows, (10, 4, w, h), content),
        &zoomed_at(columns, rows, (10, 4, w + 4, h + 2), content),
    );
    report(
        "zoomed pane shrunk 4 columns, 2 rows",
        &zoomed_at(columns, rows, (10, 4, w, h), content),
        &zoomed_at(columns, rows, (10, 4, w - 4, h - 2), content),
    );
}
