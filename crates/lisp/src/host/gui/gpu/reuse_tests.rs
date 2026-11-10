//! Unit tests of the GPU target's reuse: the changed-span diff behind the partial
//! instance upload, the run cache, the fragment cache (ADR-482), the render-pass plan and
//! the sRGB view and contrast settings it draws with.

use super::*;

#[test]
fn an_unchanged_buffer_writes_nothing() {
    let page: Vec<u32> = (0..1000).collect();
    assert_eq!(changed_span(&page, &page), None);
    // a shorter frame whose words all match needs no write either
    assert_eq!(changed_span(&page, &page[..400]), None);
}

#[test]
fn one_changed_word_writes_one_word() {
    let page: Vec<u32> = (0..1000).collect();
    for at in [0, 63, 64, 500, 999] {
        let mut next = page.clone();
        next[at] = 7_000_000;
        assert_eq!(changed_span(&page, &next), Some(at..at + 1), "at {at}");
    }
}

#[test]
fn the_span_runs_from_the_first_to_the_last_difference() {
    let page: Vec<u32> = (0..1000).collect();
    let mut next = page.clone();
    next[130] = 1;
    next[700] = 1;
    assert_eq!(changed_span(&page, &next), Some(130..701));
}

#[test]
fn a_longer_frame_writes_from_the_first_difference_to_its_end() {
    let page: Vec<u32> = (0..100).collect();
    let mut next = page.clone();
    next.extend([1, 2, 3]);
    assert_eq!(changed_span(&page, &next), Some(100..103));
    next[10] = 0;
    assert_eq!(changed_span(&page, &next), Some(10..103));
    // into an empty buffer: everything
    assert_eq!(changed_span(&[], &next), Some(0..103));
}

fn context(col: u16) -> RunContext {
    RunContext {
        place: RunPlace::Cell { col },
        metrics: [8.0f32.to_bits(), 16.0f32.to_bits(), 13.0f32.to_bits(), 1],
        ground: [0, 0, 0],
        fonts: (0, false, 0),
    }
}

fn run(text: &str, context: RunContext) -> TextRun {
    TextRun {
        text: text.to_string(),
        face: Face::default(),
        context,
        quads: vec![RunQuad::Fill {
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
            rgb: [5, 6, 7],
        }],
    }
}

#[test]
fn a_run_shaped_last_frame_is_replayed_and_one_unused_for_a_frame_is_dropped() {
    let mut cache = RunCache::default();
    let face = Face::default();
    let hash = run_hash("fn main", &face, &context(0));
    assert!(cache.take(hash, "fn main", &face, &context(0)).is_none());
    cache.put(hash, run("fn main", context(0)));
    cache.end_frame();
    // the next frame finds it (and keeps it for the one after) …
    assert!(cache.take(hash, "fn main", &face, &context(0)).is_some());
    cache.end_frame();
    assert!(cache.take(hash, "fn main", &face, &context(0)).is_some());
    // … a frame that does not draw it lets it go
    cache.end_frame();
    cache.end_frame();
    assert!(cache.take(hash, "fn main", &face, &context(0)).is_none());
}

#[test]
fn a_run_is_found_only_under_everything_it_was_shaped_with() {
    let mut cache = RunCache::default();
    let face = Face::default();
    let hash = run_hash("x\ty", &face, &context(0));
    cache.put(hash, run("x\ty", context(0)));
    // another column (a tab stop moves), another text, another face: a miss, even
    // when handed the same hash — a collision is never a wrong picture
    assert!(cache.take(hash, "x\ty", &face, &context(4)).is_none());
    assert!(cache.take(hash, "x\tz", &face, &context(0)).is_none());
    let bold = Face {
        bold: true,
        ..Face::default()
    };
    assert!(cache.take(hash, "x\ty", &bold, &context(0)).is_none());
    assert!(cache.take(hash, "x\ty", &face, &context(0)).is_some());
}

#[test]
fn a_replayed_run_lands_at_its_anchor() {
    let mut fb = FrameBuilder::new(800.0, 600.0);
    let quads = run("a", context(0)).quads;
    replay(&mut fb, &quads, 100.0, 200.0);
    assert_eq!(&fb.solids[..4], &[101.0, 202.0, 3.0, 4.0]);
}

// ---- the fragment cache (ADR-482) ----

const FW: f32 = 800.0;
const FH: f32 = 600.0;

fn fragment_context() -> FragmentContext {
    FragmentContext {
        grid: [
            0.0f32.to_bits(),
            0,
            16,
            u16::MAX as u32,
            0,
            8.0f32.to_bits(),
            16.0f32.to_bits(),
            0,
        ],
        clip: [0.0f32, 0.0, FW, FH].map(f32::to_bits),
        viewport: [FW.to_bits(), FH.to_bits()],
        metrics: [8, 16, 13.0f32.to_bits(), 1.2f32.to_bits()],
        family: 1,
        ground: [12, 12, 16],
        fonts: (0, false, 0),
        scale: 1.0f64.to_bits(),
        textures: 0,
        caret_hidden: false,
    }
}

fn ops(text: &str) -> std::sync::Arc<[Op]> {
    vec![Op::Text {
        row: 0,
        col: 0,
        s: text.into(),
        face: Face::default(),
    }]
    .into()
}

/// What expanding a fragment pushes, standing in for `expand`: a cell background, two
/// glyphs off one atlas page, a translucent overlay, a glyph off another page.
fn push_fragment_quads(fb: &mut FrameBuilder) {
    fb.fill(0.0, 0.0, 16.0, 16.0, [40, 40, 40]);
    let glyph = |fb: &mut FrameBuilder, page: usize, x: f32| {
        fb.textured(
            TexKey::Atlas(page),
            x,
            0.0,
            8.0,
            16.0,
            [0.0; 4],
            [255; 4],
            1,
            0.0,
            0.0,
        )
    };
    glyph(fb, 0, 0.0);
    glyph(fb, 0, 8.0);
    fb.solid(0.0, 0.0, 16.0, 16.0, [255, 0, 0, 100], 0.0, Edge::Plain);
    glyph(fb, 1, 16.0);
}

/// Record `push_fragment_quads` as the expansion of `fragment` in `context`, after
/// `before` set the frame up.
fn record_fragment(
    cache: &mut FragmentCache,
    fragment: &std::sync::Arc<[Op]>,
    context: FragmentContext,
    before: impl Fn(&mut FrameBuilder),
) {
    let mut fb = FrameBuilder::new(FW, FH);
    before(&mut fb);
    let mark = cache.mark(&fb);
    push_fragment_quads(&mut fb);
    cache.record(&fb, mark, fragment, context, false);
}

#[test]
fn a_replayed_fragment_builds_what_expanding_it_would() {
    let fragment = ops("ab");
    // recorded after an opaque fill, so its first quad merged into that batch
    let mut cache = FragmentCache::default();
    record_fragment(&mut cache, &fragment, fragment_context(), |fb| {
        fb.fill(100.0, 100.0, 4.0, 4.0, [1, 2, 3])
    });
    cache.end_frame();
    let entry = cache
        .take(&fragment, &fragment_context())
        .expect("the same fragment in the same context");
    // replayed after different ground — a glyph of page 0 open, then nothing at all —
    // the batches are what pushing its quads one by one builds there
    let preludes: [&dyn Fn(&mut FrameBuilder); 3] = [
        &|fb| {
            fb.textured(
                TexKey::Atlas(0),
                50.0,
                50.0,
                8.0,
                16.0,
                [0.0; 4],
                [255; 4],
                1,
                0.0,
                0.0,
            )
        },
        &|_| {},
        &|fb| fb.solid(200.0, 0.0, 4.0, 4.0, [0, 0, 0, 10], 0.0, Edge::Plain),
    ];
    for prelude in preludes {
        let mut replayed = FrameBuilder::new(FW, FH);
        prelude(&mut replayed);
        replay_fragment(&mut replayed, &entry);
        let mut expanded = FrameBuilder::new(FW, FH);
        prelude(&mut expanded);
        push_fragment_quads(&mut expanded);
        assert_eq!(replayed.solids, expanded.solids);
        assert_eq!(replayed.texs, expanded.texs);
        assert_eq!(replayed.batches, expanded.batches);
    }
    // the translucent quad keeps its segment linear though it joined a linear batch
    assert!(entry.segments.contains(&Segment::Solid {
        count: 1,
        linear: true
    }));
}

#[test]
fn a_fragment_is_replayed_only_in_the_context_it_was_expanded_in() {
    let fragment = ops("ab");
    let found = |change: &dyn Fn(&mut FragmentContext), blinks: bool| {
        let mut cache = FragmentCache::default();
        let mut fb = FrameBuilder::new(FW, FH);
        let mark = cache.mark(&fb);
        push_fragment_quads(&mut fb);
        cache.record(&fb, mark, &fragment, fragment_context(), blinks);
        cache.end_frame();
        let mut context = fragment_context();
        change(&mut context);
        cache.take(&fragment, &context).is_some()
    };
    assert!(found(&|_| {}, false));
    // a theme's background, a font size, a font (re)registration, ligatures, the
    // subpixel order, a scroll, a moved clip, a resize, a scale factor, a texture
    // upload: each is a different picture, so each misses
    let misses: [&dyn Fn(&mut FragmentContext); 11] = [
        &|c| c.ground = [250, 250, 250],
        &|c| c.metrics[2] = 15.0f32.to_bits(),
        &|c| c.metrics = [9, 18, c.metrics[2], c.metrics[3]],
        &|c| c.fonts.0 += 1,
        &|c| c.fonts.1 = true,
        &|c| c.fonts.2 = 1,
        &|c| c.grid[4] = 3.0f32.to_bits(),
        &|c| c.clip[3] = 300.0f32.to_bits(),
        &|c| c.viewport[0] = 1024.0f32.to_bits(),
        &|c| c.scale = 2.0f64.to_bits(),
        &|c| c.textures += 1,
    ];
    for (n, change) in misses.iter().enumerate() {
        assert!(
            !found(change, false),
            "change {n} replayed a stale fragment"
        );
    }
    // the blink phase matters only to a fragment holding a blinking caret
    assert!(found(&|c| c.caret_hidden = true, false));
    assert!(!found(&|c| c.caret_hidden = true, true));
    // and the key is the allocation: equal ops in another one are a miss
    let mut cache = FragmentCache::default();
    record_fragment(&mut cache, &fragment, fragment_context(), |_| {});
    cache.end_frame();
    assert!(cache.take(&ops("ab"), &fragment_context()).is_none());
}

#[test]
fn a_fragment_unused_for_a_frame_is_dropped_and_a_replay_carries_what_it_holds() {
    let (outer, inner) = (ops("outer"), ops("inner"));
    let where_drawn = fragment_context();
    let mut cache = FragmentCache::default();
    let mut runs = RunCache::default();
    // frame 1: the outer fragment expanded, the inner one (and a run) inside it
    runs.put(7, run("line", context(0)));
    let mut fb = FrameBuilder::new(FW, FH);
    let outer_mark = cache.mark(&fb);
    let inner_mark = cache.mark(&fb);
    cache.runs_used.push(7);
    push_fragment_quads(&mut fb);
    cache.record(&fb, inner_mark, &inner, where_drawn, false);
    cache.record(&fb, outer_mark, &outer, where_drawn, false);
    let entry = cache
        .take(&outer, &where_drawn)
        .expect("recorded this frame");
    assert_eq!(entry.runs, vec![7]);
    assert_eq!(entry.nested, vec![(fragment_address(&inner), where_drawn)]);
    cache.restore(entry);
    cache.end_frame();
    runs.end_frame();
    // frame 2: the outer one replayed whole; what it holds is carried, not replayed
    let entry = cache.take(&outer, &where_drawn).expect("drawn last frame");
    for &hash in &entry.runs {
        runs.keep(hash);
    }
    for (address, nested) in &entry.nested {
        cache.carry(*address, nested);
    }
    cache.restore(entry);
    cache.end_frame();
    runs.end_frame();
    // frame 3: the outer one changed; its inner fragment and run are still there
    assert!(cache.take(&inner, &where_drawn).is_some());
    assert!(
        runs.take(7, "line", &Face::default(), &context(0))
            .is_some()
    );
    // frame 4: nothing drawn the frame before — nothing kept
    cache.end_frame();
    cache.end_frame();
    assert!(cache.take(&outer, &where_drawn).is_none());
    assert!(cache.previous.is_empty() && cache.current.is_empty());
}

#[test]
fn text_and_translucent_geometry_share_a_linear_pass_and_opaque_fills_ride_along() {
    let mut fb = FrameBuilder::new(800.0, 600.0);
    fb.fill(0.0, 0.0, 10.0, 10.0, [1, 2, 3]);
    fb.textured(
        TexKey::Atlas(0),
        0.0,
        0.0,
        8.0,
        8.0,
        [0.0; 4],
        [255; 4],
        1,
        0.0,
        0.0,
    );
    // the cursor wash: translucent, so its batch blends in linear light
    fb.solid(0.0, 0.0, 10.0, 10.0, [255, 255, 255, 128], 0.0, Edge::Plain);
    fb.textured(
        TexKey::Atlas(0),
        8.0,
        0.0,
        8.0,
        8.0,
        [0.0; 4],
        [255; 4],
        1,
        0.0,
        0.0,
    );
    // text blends in linear light too (ADR-611): the whole frame is one linear pass
    assert_eq!(segments(&fb.batches, true), vec![(true, 0..4)]);
    // an app's texture (a sprite) blends in the encoded space: it takes a pass of its own
    fb.textured(
        TexKey::User(7),
        0.0,
        20.0,
        8.0,
        8.0,
        [0.0; 4],
        [255; 4],
        0,
        0.0,
        0.0,
    );
    assert_eq!(
        segments(&fb.batches, true),
        vec![(true, 0..4), (false, 4..5)]
    );
    fb.batches.pop();
    // without an sRGB view it is one pass, as before
    assert_eq!(segments(&fb.batches, false), vec![(false, 0..4)]);
    // a frame of opaque fills only: one pass, whatever is available
    let mut plain = FrameBuilder::new(800.0, 600.0);
    plain.fill(0.0, 0.0, 10.0, 10.0, [1, 2, 3]);
    assert_eq!(segments(&plain.batches, true), vec![(false, 0..1)]);
    assert_eq!(segments(&[], true), vec![(false, 0..0)]);
    // an opaque rounded chip between two runs of text: its anti-aliased rim rides the
    // open pass, so a mode line of chips costs no pass switch
    let mut chips = FrameBuilder::new(800.0, 600.0);
    chips.textured(
        TexKey::Atlas(0),
        0.0,
        0.0,
        8.0,
        8.0,
        [0.0; 4],
        [255; 4],
        1,
        0.0,
        0.0,
    );
    chips.solid(
        0.0,
        0.0,
        30.0,
        12.0,
        [40, 40, 60, 255],
        0.0,
        Edge::Rounded(3.0),
    );
    chips.textured(
        TexKey::Atlas(0),
        8.0,
        0.0,
        8.0,
        8.0,
        [0.0; 4],
        [255; 4],
        1,
        0.0,
        0.0,
    );
    assert_eq!(segments(&chips.batches, true).len(), 1);
}

#[test]
fn the_srgb_view_is_the_formats_sibling_when_the_surface_allows_one() {
    use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba16Float};
    assert_eq!(linear_view_format(Bgra8Unorm, true), Some(Bgra8UnormSrgb));
    assert_eq!(linear_view_format(Bgra8Unorm, false), None);
    // no sibling, or already sRGB: none
    assert_eq!(linear_view_format(Rgba16Float, true), None);
    assert_eq!(linear_view_format(Bgra8UnormSrgb, true), None);
}

#[test]
fn the_contrast_exponent_is_the_reciprocal_clamped_like_the_cpu_setting() {
    assert_eq!(contrast_exponent(1.0), 1.0);
    assert_eq!(contrast_exponent(2.0), 0.5);
    assert_eq!(contrast_exponent(10.0), 1.0 / 3.0);
    assert_eq!(contrast_exponent(f32::NAN), 1.0);
}
