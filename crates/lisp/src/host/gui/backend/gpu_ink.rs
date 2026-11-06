//! Text's ink on the GPU against the CPU painter's (ADR-611), on a real device with no window.
//!
//! Both painters blend text in linear light with its weight corrected to the encoded lerp's
//! and lifted by `gui-text-contrast!` (`TextInk` on the CPU, `fs_tex_linear` in the shader).
//! The CPU decides per pixel and the GPU per glyph, against the ground it was given, so the
//! two can differ by a rounding here and there; what must not differ is the INK — the mean
//! luminance a page of light text on bedit's dark ground adds to the ground. And the knob
//! has to move both: a page at γ 1.8 carries visibly more ink than one at 1.0.
//!
//! A machine without a GPU (CI) runs the CPU half, and says it skipped the GPU one.

use super::fonts::default_families;
use super::ink::luminance;
use super::offscreen::picture;
use super::render::{Renderer, TextAa};
use crate::host::gui::gpu::{GpuWindow, hardware_adapters};
use crate::host::gui::{Face, Op};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 240;
/// bedit's text and its ground (Catppuccin Mocha's `text` on `base`).
const TEXT: [u8; 3] = [0xcd, 0xd6, 0xf4];
const BASE: [u8; 3] = [0x1e, 0x1e, 0x2e];
const LINES: [&str; 6] = [
    "(defn line-box (vertical px cell-h)",
    "  \"Where text sits in a cell.\"",
    "  (let (scale (/ px (:units-per-em vertical))",
    "        ascent (* (:ascent vertical) scale))",
    "    [(round (+ half-leading ascent)) x-middle]))",
    "fn text_alpha(cov: f32, fg: f32, bg: f32) -> f32 {",
];

fn page() -> Vec<Op> {
    let mut ops = vec![Op::Clear];
    ops.extend(LINES.iter().enumerate().map(|(row, line)| Op::Text {
        row: row as u16,
        col: 0,
        s: (*line).into(),
        face: Face {
            fg: Some(TEXT),
            ..Face::default()
        },
    }));
    // one line on its own ground: a run's `:bg`, the GPU's ground for its glyphs
    ops.push(Op::Text {
        row: LINES.len() as u16,
        col: 0,
        s: "a mode line on its own band".into(),
        face: Face {
            fg: Some([0xa6, 0xad, 0xc8]),
            bg: Some([0x31, 0x32, 0x44]),
            ..Face::default()
        },
    });
    ops
}

fn renderer(gamma: f32, text_aa: TextAa) -> Renderer {
    let mut renderer = Renderer::new(1.0, default_families(), 15.0);
    renderer.set_bg(Some(BASE));
    renderer.set_text_aa(text_aa);
    renderer.set_text_contrast(gamma);
    renderer
}

/// The mean luminance pixels of `stride` bytes add to the ground's.
fn ink(pixels: &[u8], stride: usize) -> f32 {
    let count = pixels.len() / stride;
    let total: f32 = pixels
        .chunks(stride)
        .map(|pixel| luminance([pixel[0], pixel[1], pixel[2]]))
        .sum();
    total / count as f32 - luminance(BASE)
}

fn cpu_ink(gamma: f32, text_aa: TextAa) -> f32 {
    let mut renderer = renderer(gamma, text_aa);
    let image = picture(&mut renderer, &page(), WIDTH as usize, HEIGHT as usize);
    ink(&image.rgba, 4)
}

/// The knob moves the CPU painter's ink: a page at γ 1.8 carries clearly more than at 1.0.
#[test]
fn the_contrast_setting_adds_ink_to_light_text() {
    for text_aa in [TextAa::Gray, TextAa::Subpixel] {
        let (plain, lifted) = (cpu_ink(1.0, text_aa), cpu_ink(1.8, text_aa));
        assert!(
            lifted > plain * 1.1,
            "{text_aa:?}: γ 1.8 carries {lifted}, γ 1.0 {plain}"
        );
    }
}

/// The GPU's text carries the CPU's ink, gray and subpixel, at γ 1.0 and 1.8, on every
/// hardware adapter here.
#[test]
fn gpu_text_carries_the_cpu_painters_ink_on_every_hardware_adapter() {
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        for text_aa in [TextAa::Gray, TextAa::Subpixel] {
            let mut previous = None;
            for gamma in [1.0, 1.8] {
                let mut gpu = match GpuWindow::offscreen(adapter, WIDTH, HEIGHT) {
                    Ok(gpu) => gpu,
                    Err(error) => {
                        eprintln!("skipped {name}: {error}");
                        continue;
                    }
                };
                let mut renderer = renderer(gamma, text_aa);
                gpu.paint(&page(), &mut renderer)
                    .expect("an offscreen frame is never skipped");
                let gpu_ink = ink(&gpu.read_pixels(), 3);
                let cpu_ink = cpu_ink(gamma, text_aa);
                eprintln!("{name} {text_aa:?} γ {gamma}: ink cpu {cpu_ink:.5} gpu {gpu_ink:.5}");
                if (gpu_ink - cpu_ink).abs() > cpu_ink * 0.03 {
                    failures.push(format!(
                        "{name} {text_aa:?} γ {gamma}: the GPU's ink {gpu_ink} is not the \
                         CPU's {cpu_ink}"
                    ));
                }
                if let Some(plain) = previous
                    && gpu_ink <= plain * 1.1
                {
                    failures.push(format!(
                        "{name} {text_aa:?}: γ {gamma} carries {gpu_ink} on the GPU, γ 1.0 {plain}"
                    ));
                }
                previous = Some(gpu_ink);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
