//! The GPU target's fragment replay (ADR-482), checked on a real device with no window.
//!
//! An unchanged memo fragment's quads are pushed whole from last frame's expansion rather
//! than expanded again (`gpu::FragmentCache`). The CPU painter's column-limited repaint
//! is checked pixel for pixel by `paint.rs`'s tests; the replay can only be checked where
//! the quads are drawn, on a GPU. So each hardware adapter on the machine draws one
//! sequence of frames into an offscreen texture of a window's size and format, three
//! ways — replay on, fragment replay alone off, every reuse off (`BROOD_GUI_DAMAGE=0`) —
//! and the pixels read back must be identical, frame for frame, save the device's own
//! noise (`DEVICE_NOISE_PIXELS`).
//!
//! A machine without a GPU (CI) has no hardware adapter: the test says so and passes.
//! Software adapters (llvmpipe, lavapipe) are left out, as the window leaves them out.

use super::render::{Renderer, TextAa, default_families};
use crate::host::gui::gpu::{GpuWindow, hardware_adapters};
use crate::host::gui::{CursorBlink, CursorStyle, Face, FragmentOps, Op};
use std::sync::Arc;

const WIDTH: u32 = 800;
const HEIGHT: u32 = 600;
const FONT_PX: f32 = 14.0;

/// How many pixels two draws of one frame may differ in and still count as the same
/// picture. Intel's Raptor Lake UHD (Mesa ANV, Vulkan) draws grey-anti-aliased text with
/// the odd stray pixel: one or two pixels, a faint text tint where the ground belongs or
/// the reverse, somewhere different each run — between two windows with every reuse OFF
/// as often as between replay and expansion, read back twice the same, with or without
/// the sRGB view, never under the subpixel pipeline and never on NVIDIA. A replay that
/// went wrong pushes, drops or misplaces whole quads, and the smallest quad there is (a
/// hairline under a cell) is a cell wide — so a difference this small is the device, and
/// anything larger fails.
const DEVICE_NOISE_PIXELS: usize = 3;

/// What changes about the window between two frames, besides its ops.
#[derive(Clone)]
enum Change {
    Nothing,
    Background([u8; 3]),
    FontPx(f32),
    CaretHidden(bool),
    UploadTexture { texture: u32, colour: [u8; 4] },
    FreeTexture(u32),
    Resize(u32, u32),
}

/// One frame of the sequence: what changed, the ops drawn, and whether the replay is
/// expected to push any fragment whole (the sequence is no check of replay if it never
/// does).
struct Step {
    label: &'static str,
    change: Change,
    ops: Vec<Op>,
    replays: bool,
}

/// How a window draws: which reuse it has on.
#[derive(Clone, Copy, Debug)]
enum Mode {
    Replay,
    FragmentsExpanded,
    NoReuse,
}

fn face(foreground: [u8; 3]) -> Face {
    Face {
        fg: Some(foreground),
        ..Face::default()
    }
}

fn fragment(ops: Vec<Op>) -> Arc<[Op]> {
    Arc::from(ops)
}

fn op(fragment: &Arc<[Op]>) -> Op {
    Op::Fragment(FragmentOps(fragment.clone()))
}

/// One editor line as a fragment: a keyword, the rest of the line, a highlighted span.
fn line(row: u16, text: &str, keyword_colour: [u8; 3]) -> Arc<[Op]> {
    fragment(vec![
        Op::Text {
            row,
            col: 0,
            s: format!("{row:>4} (defn"),
            face: face(keyword_colour),
        },
        Op::Text {
            row,
            col: 10,
            s: text.to_string(),
            face: face([205, 214, 244]),
        },
        Op::Rect {
            row,
            col: 60,
            w: 6,
            h: 1,
            face: Face {
                bg: Some([69, 71, 90]),
                ..Face::default()
            },
            radius: 2.0,
        },
    ])
}

fn document(rows: u16, keyword_colour: [u8; 3]) -> Vec<Arc<[Op]>> {
    (0..rows)
        .map(|row| {
            line(
                row,
                &format!("line-{row} [x y] (+ x y {row}) ; a comment with ligatures -> =>"),
                keyword_colour,
            )
        })
        .collect()
}

/// A frame of `lines` plus an unmemoised mode line under them.
fn page(lines: &[Arc<[Op]>]) -> Vec<Op> {
    let mut ops = vec![Op::Clear];
    ops.extend(lines.iter().map(op));
    ops.push(Op::Text {
        row: 25,
        col: 0,
        s: "-:--  scratch.blsp   (Brood)".to_string(),
        face: face([166, 227, 161]),
    });
    ops
}

fn caret(row: u16, col: u16) -> Arc<[Op]> {
    fragment(vec![
        Op::Text {
            row,
            col: 0,
            s: "the line the caret is on".to_string(),
            face: face([249, 226, 175]),
        },
        Op::Cursor {
            row,
            col,
            style: CursorStyle::Block,
            color: Some([245, 224, 220]),
            blink: Some(CursorBlink {
                interval_ms: 500,
                blinks: 0,
            }),
            under: None,
        },
    ])
}

fn sprite(texture: u32) -> Arc<[Op]> {
    fragment(vec![Op::Sprite {
        tex: texture,
        x: 500.0,
        y: 300.0,
        w: 64.0,
        h: 64.0,
        uv: [0.0, 0.0, 1.0, 1.0],
        tint: [255, 255, 255, 255],
        rot: 0.0,
    }])
}

/// The sequence: every case the replay has to get right, one after another, each
/// followed by the unchanged frame that replays it.
fn steps() -> Vec<Step> {
    let mauve = [203, 166, 247];
    let peach = [250, 179, 135];
    let mut lines = document(22, mauve);
    let mut steps = Vec::new();
    let mut push = |label, change, ops, replays| {
        steps.push(Step {
            label,
            change,
            ops,
            replays,
        })
    };

    push("first frame", Change::Nothing, page(&lines), false);
    push("unchanged", Change::Nothing, page(&lines), true);
    lines[5] = line(5, "an edited line, longer than it was before", mauve);
    push("one line edited", Change::Nothing, page(&lines), true);
    push("after the edit", Change::Nothing, page(&lines), true);

    // the window background is the ground a run's contrast is judged against
    push(
        "background changed",
        Change::Background([239, 241, 245]),
        page(&lines),
        false,
    );
    push("new background", Change::Nothing, page(&lines), true);
    // a theme switch: every line recoloured, new fragments throughout
    let recoloured = document(22, peach);
    push("palette changed", Change::Nothing, page(&recoloured), false);
    push("new palette", Change::Nothing, page(&recoloured), true);

    let scrolled = |dy_frac: f32, lines: &[Arc<[Op]>]| {
        vec![
            Op::Clear,
            Op::ScrollRegion {
                dy_frac,
                ops: lines.iter().map(op).collect(),
            },
        ]
    };
    push(
        "scrolled a quarter line",
        Change::Nothing,
        scrolled(0.25, &lines),
        false,
    );
    push("scroll held", Change::Nothing, scrolled(0.25, &lines), true);
    push(
        "scrolled further",
        Change::Nothing,
        scrolled(0.6, &lines),
        false,
    );
    push(
        "scroll settled",
        Change::Nothing,
        scrolled(0.0, &lines),
        false,
    );
    push("settled", Change::Nothing, scrolled(0.0, &lines), true);

    push("font size up", Change::FontPx(18.0), page(&lines), false);
    push("at the new size", Change::Nothing, page(&lines), true);
    push(
        "font size back",
        Change::FontPx(FONT_PX),
        page(&lines),
        false,
    );

    let with_caret = |lines: &[Arc<[Op]>], caret: &Arc<[Op]>| {
        let mut ops = page(lines);
        ops.push(op(caret));
        ops
    };
    let caret_line = caret(23, 7);
    push(
        "caret shown",
        Change::CaretHidden(false),
        with_caret(&lines, &caret_line),
        true,
    );
    push(
        "caret hidden",
        Change::CaretHidden(true),
        with_caret(&lines, &caret_line),
        true,
    );
    push(
        "caret still hidden",
        Change::Nothing,
        with_caret(&lines, &caret_line),
        true,
    );
    push(
        "caret shown again",
        Change::CaretHidden(false),
        with_caret(&lines, &caret_line),
        true,
    );

    let picture = sprite(1);
    let with_sprite = |lines: &[Arc<[Op]>]| {
        let mut ops = page(lines);
        ops.push(op(&picture));
        ops
    };
    // drawn before its texture exists: nothing, until the upload
    push(
        "sprite without its texture",
        Change::Nothing,
        with_sprite(&lines),
        true,
    );
    push(
        "texture uploaded",
        Change::UploadTexture {
            texture: 1,
            colour: [255, 0, 128, 255],
        },
        with_sprite(&lines),
        false,
    );
    push("texture held", Change::Nothing, with_sprite(&lines), true);
    push(
        "texture replaced",
        Change::UploadTexture {
            texture: 1,
            colour: [0, 200, 80, 255],
        },
        with_sprite(&lines),
        false,
    );
    push(
        "replaced texture held",
        Change::Nothing,
        with_sprite(&lines),
        true,
    );
    push(
        "texture freed",
        Change::FreeTexture(1),
        with_sprite(&lines),
        false,
    );

    // a pane: one fragment holding the line fragments, inside a cell region
    let pane = |inner: &[Arc<[Op]>]| {
        let mut ops: Vec<Op> = inner.iter().map(op).collect();
        ops.push(Op::Rect {
            row: 0,
            col: 0,
            w: 1,
            h: inner.len() as u16,
            face: Face {
                bg: Some([88, 91, 112]),
                ..Face::default()
            },
            radius: 0.0,
        });
        fragment(ops)
    };
    let mut inner = document(10, mauve);
    let outer = pane(&inner);
    let in_region = |outer: &Arc<[Op]>, x: u16| {
        vec![
            Op::Clear,
            Op::CellRegion {
                x,
                y: 2,
                w: 80,
                h: 12,
                px: 12.0,
                ops: vec![op(outer)],
            },
        ]
    };
    push(
        "nested fragments",
        Change::Nothing,
        in_region(&outer, 2),
        false,
    );
    push("nested held", Change::Nothing, in_region(&outer, 2), true);
    inner[3] = line(3, "an inner line edited", peach);
    let outer = pane(&inner);
    push(
        "an inner line edited",
        Change::Nothing,
        in_region(&outer, 2),
        true,
    );
    push(
        "edited pane held",
        Change::Nothing,
        in_region(&outer, 2),
        true,
    );
    push("region moved", Change::Nothing, in_region(&outer, 6), false);
    push(
        "moved region held",
        Change::Nothing,
        in_region(&outer, 6),
        true,
    );

    let without = |lines: &[Arc<[Op]>]| {
        let kept: Vec<Arc<[Op]>> = lines
            .iter()
            .enumerate()
            .filter(|(row, _)| !(8..16).contains(row))
            .map(|(_, line)| line.clone())
            .collect();
        page(&kept)
    };
    // the nested frames came between: nothing of `lines` is cached any more
    push(
        "eight lines dropped",
        Change::Nothing,
        without(&lines),
        false,
    );
    push(
        "dropped lines re-added",
        Change::Nothing,
        page(&lines),
        true,
    );
    push("re-added held", Change::Nothing, page(&lines), true);

    // A fragment dropped for good, then a new one of the same size and a different
    // picture: the allocator may hand the new one the old one's address, the cache's key.
    // Two frames without it let the cache let go of the old one first.
    let short: Vec<Arc<[Op]>> = lines[..16].to_vec();
    push("lines gone", Change::Nothing, page(&short), true);
    push("lines still gone", Change::Nothing, page(&short), true);
    drop(lines);
    let replacements: Vec<Arc<[Op]>> = (16..22)
        .map(|row| line(row, "a different line in a reused allocation", peach))
        .collect();
    let mut refilled = short.clone();
    refilled.extend(replacements);
    push(
        "new lines, maybe old addresses",
        Change::Nothing,
        page(&refilled),
        true,
    );

    push(
        "window narrowed",
        Change::Resize(640, 400),
        page(&refilled),
        false,
    );
    push("narrow window held", Change::Nothing, page(&refilled), true);
    push(
        "window widened",
        Change::Resize(WIDTH, HEIGHT),
        page(&refilled),
        false,
    );
    push("wide window held", Change::Nothing, page(&refilled), true);
    steps
}

/// A window drawing in `mode` with text anti-aliased as `text_aa`.
struct Target {
    gpu: GpuWindow,
    renderer: Renderer,
}

impl Target {
    fn new(adapter: &wgpu::Adapter, mode: Mode, text_aa: TextAa) -> Result<Target, String> {
        let mut gpu = GpuWindow::offscreen(adapter, WIDTH, HEIGHT)?;
        match mode {
            Mode::Replay => gpu.set_reuse(true),
            Mode::FragmentsExpanded => {
                gpu.set_reuse(true);
                gpu.set_replay_fragments(false);
            }
            Mode::NoReuse => gpu.set_reuse(false),
        }
        let mut renderer = Renderer::new(1.0, default_families(), FONT_PX);
        renderer.set_text_aa(text_aa);
        // a contrast lift, so text is drawn according to the ground it is on (γ 1 is none)
        renderer.set_text_contrast(1.6);
        Ok(Target { gpu, renderer })
    }

    fn apply(&mut self, change: &Change) {
        match change {
            Change::Nothing => {}
            Change::Background(rgb) => self.renderer.set_bg(Some(*rgb)),
            Change::FontPx(px) => self.renderer.set_font(None, Some(*px)),
            Change::CaretHidden(hidden) => self.renderer.caret_hidden = *hidden,
            Change::UploadTexture { texture, colour } => {
                // a checkerboard of `colour` and black, so a stale texture shows
                let side = 16u32;
                let rgba: Vec<u8> = (0..side * side)
                    .flat_map(|index| {
                        if (index % side / 4 + index / side / 4).is_multiple_of(2) {
                            *colour
                        } else {
                            [0, 0, 0, 255]
                        }
                    })
                    .collect();
                self.gpu.upload_texture(*texture, &rgba, side, side);
            }
            Change::FreeTexture(texture) => self.gpu.free_texture(*texture),
            Change::Resize(width, height) => self.gpu.resize(*width, *height),
        }
    }

    /// Draw `step`; its pixels and the encode time in microseconds.
    fn draw(&mut self, step: &Step) -> (Vec<u8>, u64) {
        self.apply(&step.change);
        let times = self
            .gpu
            .paint(&step.ops, &mut self.renderer)
            .expect("an offscreen frame is never skipped");
        (self.gpu.read_pixels(), times.encode_us)
    }
}

/// Where two frames differ: how many pixels, and the first.
fn difference(left: &[u8], right: &[u8], width: u32) -> Option<(usize, (u32, u32))> {
    if left.len() != right.len() {
        return Some((usize::MAX, (0, 0)));
    }
    let mut first = None;
    let mut count = 0;
    for (index, (a, b)) in left.chunks(3).zip(right.chunks(3)).enumerate() {
        if a != b {
            count += 1;
            first.get_or_insert((index as u32 % width, index as u32 / width));
        }
    }
    first.map(|at| (count, at))
}

/// The window's size during `step` of `steps`, for the readback's row width.
fn widths(steps: &[Step]) -> Vec<u32> {
    let mut width = WIDTH;
    steps
        .iter()
        .map(|step| {
            if let Change::Resize(new_width, _) = step.change {
                width = new_width;
            }
            width
        })
        .collect()
}

/// How many pixels are not the most common colour: a frame of nothing but background
/// would compare equal to anything and prove nothing.
/// The steps whose picture must differ from the one before: a check that what they
/// change is on screen at all, since a caret below the window would compare equal
/// whether or not its blink was replayed.
const VISIBLE_CHANGES: &[&str] = &[
    "one line edited",
    "background changed",
    "caret hidden",
    "caret shown again",
    "texture uploaded",
    "texture replaced",
    "an inner line edited",
    "region moved",
    "new lines, maybe old addresses",
];

fn drawn_pixels(pixels: &[u8]) -> usize {
    let ground = &pixels[..3];
    pixels.chunks(3).filter(|pixel| pixel != &ground).count()
}

#[test]
fn replayed_fragments_draw_what_expanded_ones_do_on_every_hardware_adapter() {
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let steps = steps();
    let widths = widths(&steps);
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        for text_aa in [TextAa::Gray, TextAa::Subpixel] {
            let mut targets = Vec::new();
            for mode in [Mode::Replay, Mode::FragmentsExpanded, Mode::NoReuse] {
                match Target::new(adapter, mode, text_aa) {
                    Ok(target) => targets.push((mode, target)),
                    Err(error) => eprintln!("skipped {name}: {error}"),
                }
            }
            if targets.len() < 3 {
                continue;
            }
            let (mut replayed, mut noise) = (0, 0);
            let mut previous: Option<Vec<u8>> = None;
            for (step, &width) in steps.iter().zip(&widths) {
                let mut frames = Vec::new();
                for (mode, target) in &mut targets {
                    let (pixels, _) = target.draw(step);
                    let (hits, _) = target.gpu.last_fragments();
                    frames.push((*mode, pixels, hits));
                }
                let (_, reference, hits) = &frames[0];
                if VISIBLE_CHANGES.contains(&step.label) && previous.as_ref() == Some(reference) {
                    failures.push(format!(
                        "{name} {text_aa:?} '{}': drew the same picture as the frame before",
                        step.label
                    ));
                }
                replayed += hits;
                if step.replays && *hits == 0 {
                    failures.push(format!(
                        "{name} {text_aa:?} '{}': expected a replay, none happened",
                        step.label
                    ));
                }
                if drawn_pixels(reference) < 500 {
                    failures.push(format!(
                        "{name} {text_aa:?} '{}': the frame is nearly blank",
                        step.label
                    ));
                }
                for (mode, pixels, _) in &frames[1..] {
                    match difference(reference, pixels, width) {
                        None => {}
                        Some((count, (x, y))) if count <= DEVICE_NOISE_PIXELS => {
                            noise += 1;
                            eprintln!(
                                "{name} {text_aa:?} '{}': {count} stray pixels at ({x}, {y}) \
                                 against {mode:?} — device noise, not replay",
                                step.label
                            );
                        }
                        Some((count, (x, y))) => failures.push(format!(
                            "{name} {text_aa:?} '{}': replay differs from {mode:?} in {count} \
                             pixels, first at ({x}, {y})",
                            step.label
                        )),
                    }
                }
                previous = Some(frames.swap_remove(0).1);
            }
            eprintln!(
                "{name} {text_aa:?}: {} frames compared three ways, {replayed} fragments \
                 replayed, {noise} comparisons with stray pixels",
                steps.len()
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The replay's effect on the frame build, one shot: a page of 60 line fragments, one
/// line changed per frame, encoded with replay on and off. Ignored — a timing, not a
/// check; `cargo test … gpu_replay -- --ignored --nocapture` prints it.
#[test]
#[ignore]
fn replay_timing_for_a_page_with_one_line_changed() {
    let Some(adapter) = hardware_adapters().into_iter().next() else {
        eprintln!("skipped: no hardware GPU adapter here");
        return;
    };
    let info = adapter.get_info();
    let mut lines = document(60, [203, 166, 247]);
    let mut frames = vec![page(&lines)];
    for edit in 0..8 {
        let row = (edit * 7 % 60) as u16;
        lines[row as usize] = line(row, &format!("edit {edit} on this line"), [250, 179, 135]);
        frames.push(page(&lines));
    }
    for mode in [Mode::Replay, Mode::FragmentsExpanded, Mode::NoReuse] {
        let mut target = Target::new(&adapter, mode, TextAa::Subpixel).expect("a device");
        // tall enough to show all sixty lines, as a bedit frame would
        target.gpu.resize(1600, 1300);
        // the first frame shapes everything either way; the rest change one line each
        let mut encode: Vec<u64> = frames
            .iter()
            .map(|ops| {
                let step = Step {
                    label: "timing",
                    change: Change::Nothing,
                    ops: ops.clone(),
                    replays: false,
                };
                target.draw(&step).1
            })
            .skip(1)
            .collect();
        encode.sort_unstable();
        eprintln!(
            "{} ({:?}) {mode:?}: encode median {}us, min {}us, max {}us over {} frames",
            info.name,
            info.backend,
            encode[encode.len() / 2],
            encode[0],
            encode[encode.len() - 1],
            encode.len()
        );
    }
}
