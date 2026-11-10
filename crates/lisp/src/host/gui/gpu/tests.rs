//! Unit tests of the GPU target's pure parts: quad culling, batching and the clip band,
//! the cursor and scroll geometry, the shaders' validation, the particle uniform, and the
//! swapchain's format and present-mode choice — all without a device.

use super::*;

const FW: f32 = 800.0;
const FH: f32 = 600.0;

#[test]
fn an_on_screen_quad_is_kept() {
    assert!(quad_visible(10.0, 10.0, 5.0, 5.0, FW, FH));
    // Partly off each edge still touches the viewport.
    assert!(quad_visible(-2.0, -2.0, 5.0, 5.0, FW, FH));
    assert!(quad_visible(798.0, 598.0, 5.0, 5.0, FW, FH));
}

#[test]
fn a_quad_entirely_outside_the_viewport_is_culled() {
    assert!(!quad_visible(-10.0, 10.0, 5.0, 5.0, FW, FH)); // left
    assert!(!quad_visible(10.0, -10.0, 5.0, 5.0, FW, FH)); // above
    assert!(!quad_visible(FW, 10.0, 5.0, 5.0, FW, FH)); // right
    assert!(!quad_visible(10.0, FH, 5.0, 5.0, FW, FH)); // below
    // A bitboard cell far past the window: the `Op::Cells` case the cull exists for.
    assert!(!quad_visible(1_000_000.0, 1_000_000.0, 8.0, 16.0, FW, FH));
}

#[test]
fn a_degenerate_quad_is_culled() {
    assert!(!quad_visible(10.0, 10.0, 0.0, 5.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, 5.0, 0.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, -5.0, 5.0, FW, FH));
}

#[test]
fn a_nan_coordinate_is_culled_not_buffered() {
    // Every comparison against NaN is false, so a positively-phrased test culls; a
    // negatively-phrased one (`!(x + w <= 0.0 || …)`) would have buffered the quad.
    assert!(!quad_visible(f32::NAN, 10.0, 5.0, 5.0, FW, FH));
    assert!(!quad_visible(10.0, f32::NAN, 5.0, 5.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, f32::NAN, 5.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, 5.0, f32::NAN, FW, FH));
}

#[test]
fn an_infinity_is_judged_by_where_it_puts_the_quad() {
    // A quad starting at -inf with finite width never reaches the viewport: culled.
    assert!(!quad_visible(f32::NEG_INFINITY, 10.0, 5.0, 5.0, FW, FH));
    // A quad starting at +inf is past the right edge: culled.
    assert!(!quad_visible(f32::INFINITY, 10.0, 5.0, 5.0, FW, FH));
    // An infinitely wide quad from an on-screen x covers the viewport: kept.
    assert!(quad_visible(10.0, 10.0, f32::INFINITY, 5.0, FW, FH));
}

#[test]
fn a_saturating_extent_does_not_wrap_into_view() {
    // `x + w` overflowing to +inf must not read as "on screen" from the left.
    assert!(!quad_visible(f32::MAX, 10.0, f32::MAX, 5.0, FW, FH));
}

#[test]
fn a_rotated_quad_is_judged_by_the_disc_it_sweeps() {
    let fb = FrameBuilder::new(FW, FH);
    // A 400-wide, 4-tall bar whose unrotated rect sits wholly below the viewport,
    // turned a quarter turn about its centre: it stands up through the bottom edge, so
    // it is kept …
    assert!(!fb.visible(100.0, FH + 10.0, 400.0, 4.0, 0.0));
    assert!(fb.visible(100.0, FH + 10.0, 400.0, 4.0, std::f32::consts::FRAC_PI_2));
    // … while a small quad far away stays culled whatever its angle.
    assert!(!fb.visible(FW + 500.0, 100.0, 4.0, 4.0, 1.0));
}

/// Parse and validate WGSL as wgpu would on a device with `capabilities` — the only
/// check a shader gets without a GPU, which CI has none of.
fn validate_wgsl(source: &str, capabilities: wgpu::naga::valid::Capabilities) {
    let module = wgpu::naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), capabilities)
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
}

#[test]
fn the_text_shaders_validate_the_lcd_one_only_with_dual_source_blending() {
    use wgpu::naga::valid::Capabilities;
    validate_wgsl(SHADER_SRC, Capabilities::default());
    validate_wgsl(&lcd_shader_source(), Capabilities::DUAL_SOURCE_BLENDING);
    // the LCD source asks for the feature: a device without it must not be handed it
    let without = wgpu::naga::front::wgsl::parse_str(&lcd_shader_source()).map(|module| {
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            Capabilities::default(),
        )
        .validate(&module)
        .is_ok()
    });
    assert!(!matches!(without, Ok(true)));
}

#[test]
fn a_glyph_mode_says_colour_grey_or_lcd_and_whether_it_is_lifted() {
    assert_eq!(glyph_mode(false, false, true), 0); // an emoji draws in its own colours
    assert_eq!(glyph_mode(false, true, true), 0);
    assert_eq!(glyph_mode(true, false, false), 1);
    assert_eq!(glyph_mode(true, false, true), 2);
    assert_eq!(glyph_mode(true, true, false), 3);
    assert_eq!(glyph_mode(true, true, true), 4);
}

#[test]
fn lcd_glyphs_batch_apart_from_grey_ones_on_the_same_atlas_page() {
    // they draw with different pipelines, so one batch cannot hold both
    let mut fb = FrameBuilder::new(FW, FH);
    let uv = [0.0, 0.0, 0.1, 0.1];
    fb.textured(
        TexKey::Atlas(0),
        0.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        3,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::Atlas(0),
        8.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        4,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::Atlas(0),
        16.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        1,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::Atlas(0),
        24.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        3,
        0.0,
        0.0,
    );
    let shape: Vec<(u32, u32, bool)> = fb
        .batches
        .iter()
        .map(|b| match b {
            Batch::Tex {
                first, count, lcd, ..
            } => (*first, *count, *lcd),
            _ => unreachable!("only glyphs in this frame"),
        })
        .collect();
    assert_eq!(shape, vec![(0, 2, true), (2, 1, false), (3, 1, true)]);
}

#[test]
fn consecutive_quads_of_one_kind_form_one_batch_and_a_kind_change_starts_another() {
    let mut fb = FrameBuilder::new(FW, FH);
    fb.solid(0.0, 0.0, 10.0, 10.0, [255, 0, 0, 255], 0.0, Edge::Plain);
    fb.solid(20.0, 0.0, 10.0, 10.0, [255, 0, 0, 255], 0.0, Edge::Plain);
    let uv = [0.0, 0.0, 1.0, 1.0];
    fb.textured(
        TexKey::User(7),
        0.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        0,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::User(7),
        8.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        0,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::User(9),
        16.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        0,
        0.0,
        0.0,
    );
    fb.solid(40.0, 0.0, 10.0, 10.0, [0, 255, 0, 255], 0.0, Edge::Plain);
    let shape: Vec<(bool, u32, u32)> = fb
        .batches
        .iter()
        .map(|b| match b {
            Batch::Solid { first, count, .. } => (true, *first, *count),
            Batch::Tex { first, count, .. } => (false, *first, *count),
            Batch::Particles { .. } => unreachable!("no emitter in this frame"),
        })
        .collect();
    // Two solids → one batch; two sprites of texture 7 → one; texture 9 → its own;
    // the trailing solid a fourth, continuing the solid buffer at index 2.
    assert_eq!(
        shape,
        vec![(true, 0, 2), (false, 0, 2), (false, 2, 1), (true, 2, 1)]
    );
    assert_eq!(fb.solids.len(), 3 * SOLID_FLOATS);
    assert_eq!(fb.texs.len(), 3 * TEX_WORDS);
}

#[test]
fn a_clip_band_culls_what_lies_outside_it_and_rides_on_every_quad() {
    let mut fb = FrameBuilder::new(FW, FH);
    fb.clip = [0.0, 100.0, FW, 200.0];
    // Wholly above the band: culled. Straddling it: kept, carrying the band.
    fb.solid(10.0, 10.0, 10.0, 10.0, [255; 4], 0.0, Edge::Plain);
    assert!(fb.batches.is_empty());
    fb.solid(10.0, 95.0, 10.0, 10.0, [255; 4], 0.0, Edge::Plain);
    assert_eq!(fb.solids.len(), SOLID_FLOATS);
    assert_eq!(&fb.solids[SOLID_FLOATS - 4..], &[0.0, 100.0, FW, 200.0]);
    // A rounded edge is flagged for the SDF path with its radius.
    fb.solid(10.0, 120.0, 10.0, 10.0, [255; 4], 0.0, Edge::Rounded(3.0));
    assert_eq!(&fb.solids[SOLID_FLOATS + 9..SOLID_FLOATS + 11], &[3.0, 1.0]);
}

#[test]
fn cursor_quads_follow_the_cpu_geometry() {
    // Block: the overlay plus four rim bars, 2 px thick for a 10 px cell.
    let block = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Block, None);
    assert_eq!(block.len(), 5);
    assert_eq!(block[0], (100.0, 50.0, 10.0, 20.0, [255, 255, 255, 128]));
    assert_eq!(block[2].0, 108.0); // the right rim at cw - t
    // Bar: one caret on the left edge, at least 2 px.
    let bar = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Bar, None);
    assert_eq!(bar.len(), 1);
    assert_eq!((bar[0].2, bar[0].3), (2.0, 20.0));
    // Underline: one rule along the cell bottom.
    let under = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Underline, None);
    assert_eq!(under[0].1, 68.0);
    // Hollow: the rim alone, 1 px for a 10 px cell, in the op's colour.
    let hollow = cursor_quads(
        100.0,
        50.0,
        10.0,
        20.0,
        CursorStyle::Hollow,
        Some([1, 2, 3]),
    );
    assert_eq!(hollow.len(), 4);
    assert_eq!(hollow[0], (100.0, 50.0, 1.0, 20.0, [1, 2, 3, 255]));
    // A coloured block washes the cell in that colour at half alpha.
    let tinted = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Block, Some([9, 8, 7]));
    assert_eq!(tinted[0].4, [9, 8, 7, 128]);
}

#[test]
fn a_scroll_shift_is_whole_pixels_and_capped() {
    assert_eq!(scroll_px(0.5, 16.0), 8.0);
    assert_eq!(scroll_px(-0.26, 16.0), -4.0);
    assert_eq!(scroll_px(1.0e9, 16.0), MAX_SCROLL_PX);
}

#[test]
fn no_batch_merges_across_the_overlay() {
    let mut fb = FrameBuilder::new(100.0, 100.0);
    fb.solid(0.0, 0.0, 10.0, 10.0, [255, 0, 0, 255], 0.0, Edge::Plain);
    fb.overlay_at = Some(fb.batches.len());
    fb.solid(20.0, 0.0, 10.0, 10.0, [0, 255, 0, 255], 0.0, Edge::Plain);
    fb.solid(40.0, 0.0, 10.0, 10.0, [0, 0, 255, 255], 0.0, Edge::Plain);
    let counts: Vec<u32> = fb
        .batches
        .iter()
        .map(|b| match b {
            Batch::Solid { count, .. } => *count,
            _ => 0,
        })
        .collect();
    assert_eq!(
        counts,
        vec![1, 2],
        "the scene's run ends at the overlay; the overlay's merge"
    );
}

#[test]
fn a_step_is_the_shaders_params_word_for_word() {
    let step = EmitterStep {
        id: 1,
        x: 3.0,
        y: 4.0,
        spec: crate::host::gui::EmitterSpec::default(),
        dt: 0.5,
        spawn_first: 6,
        spawn_count: 7,
        seed: 8,
        present: true,
    };
    let words = params_words(&step, 9);
    assert_eq!(words.len() as u64 * 4, PARTICLE_PARAMS_BYTES);
    assert_eq!(f32::from_bits(words[0]), 3.0);
    assert_eq!(f32::from_bits(words[20]), 0.5, "dt");
    assert_eq!(
        &words[22..26],
        &[6, 7, 9, 8],
        "spawn_first, spawn_count, capacity, seed"
    );
}

#[test]
fn a_culled_quad_leaves_no_instance_and_no_batch() {
    let mut fb = FrameBuilder::new(FW, FH);
    fb.solid(-100.0, -100.0, 10.0, 10.0, [255; 4], 0.0, Edge::Plain);
    assert!(fb.batches.is_empty());
    assert!(fb.solids.is_empty());
}

#[test]
fn a_non_srgb_format_is_preferred_over_an_srgb_one() {
    use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba16Float};
    assert_eq!(
        pick_format(&[Bgra8UnormSrgb, Rgba16Float, Bgra8Unorm]),
        Some(Bgra8Unorm)
    );
    // Only sRGB on offer: the first supported one, rather than no window at all.
    assert_eq!(pick_format(&[Bgra8UnormSrgb]), Some(Bgra8UnormSrgb));
    assert_eq!(pick_format(&[]), None);
}

#[test]
fn the_default_is_mailbox_no_vsync_may_tear_and_vsync_is_fifo() {
    use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
    let all = [Fifo, Mailbox, Immediate];
    // the default never tears: mailbox, else fifo — never immediate
    assert_eq!(pick_present_mode(&all, None), Mailbox);
    assert_eq!(pick_present_mode(&[Fifo, Immediate], None), Fifo);
    // `:vsync false` asks for the work-bound rate
    assert_eq!(pick_present_mode(&all, Some(false)), Immediate);
    assert_eq!(pick_present_mode(&[Fifo, Mailbox], Some(false)), Mailbox);
    assert_eq!(pick_present_mode(&[Fifo], Some(false)), Fifo);
    // `:vsync true` is fifo
    assert_eq!(pick_present_mode(&all, Some(true)), Fifo);
}
