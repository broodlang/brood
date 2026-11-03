//! GPU render target for the GUI window (the `gui-gpu` feature), on wgpu.
//!
//! Replaces the CPU softbuffer present + per-pixel blit with two instanced-quad
//! pipelines. Every op becomes quads: a solid-fill op (Clear / Rect / FRect / VSpans /
//! Cells / CellsRgb / Quad, a text op's cell background, a cursor, an underline) a
//! coloured quad, a glyph or a `Sprite` a textured one. The quads are drawn in op order
//! in **batches** — a run of consecutive solids is one instanced draw, a run of textured
//! quads from the same texture another — so a frame costs a handful of draw calls
//! whatever its op count, and a sprite drawn after a rect lands on top of it, as the CPU
//! painter's op order promises. Glyphs share one texture (an atlas the rasterised
//! clusters are packed into as they first appear), so a page of text is one batch, not
//! one draw per glyph. The swapchain presents MAILBOX by default (low latency, no
//! tearing), FIFO under `{:vsync true}`, and IMMEDIATE only under `{:vsync false}`, so a
//! sim that asks can still run at the rate its work allows (ADR-440).
//!
//! The op walk mirrors the CPU painter's (`paint::render_ops`) arm for arm — the same
//! grid origin, the same scroll shift, the same region metrics and clip band, the same
//! cursor geometry — so a frame lands on the same pixels whichever target paints it. A
//! rounded or sub-cell rect is a signed-distance field in the fragment shader, the same
//! 1 px coverage ramp `fill_rrect` computes per pixel; a clip band is a per-instance rect
//! the fragment shader discards outside of. Text is subpixel (LCD) where the CPU path's
//! would be — `gui-text-aa!`, `:auto` meaning at a 1× scale — in one pass with dual-source
//! blending where the adapter has it (the second fragment output is each channel's
//! coverage, `fs_tex_lcd`), else in three, one colour channel each (`LcdText`). The
//! blending otherwise matches: text lerps in the encoded space with the contrast lift in the
//! shader, and translucent geometry blends in linear light through an sRGB view of the frame.
//!
//! A frame is re-walked, but not re-shaped: a text run's quads are kept and replayed
//! while it repeats (`RunCache`), an unchanged memoised fragment's instances are pushed
//! whole without walking its ops (`FragmentCache`, ADR-482), and the instance buffers are
//! written only where they changed (`changed_span`) — the GPU's answer to the CPU
//! painter's row diff.
//!
//! This module is MECHANISM only. It knows a quad, a texture, a UV rect, a tint and an
//! angle; what a sprite sheet is, how a line is a thin quad at an angle, which frame an
//! animation is on, are Brood (`std/gui.blsp` and the engine above it).
//!
//! wgpu, not OpenGL: the runtime is meant to run on Windows and macOS as well as Linux,
//! and the GLES 3.0 context the first prototype requested exists on neither (CGL is
//! desktop-GL only and deprecated; Windows needs ANGLE). wgpu picks Vulkan / Metal / DX12
//! per platform behind one code path — and is what an in-browser build would draw with.

use std::collections::HashMap;
use std::rc::Rc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

use unicode_segmentation::UnicodeSegmentation;

use crate::host::gui::backend::{
    CURSOR_FG, Renderer, caret_cells, caret_glyph, cursor_rim, holds_blinking_caret, luma,
    paint_stall_ms, snap_hairline, text_px_width, text_units, unit_cells,
};
use crate::host::gui::effects::{EmitterStep, FrameEffects};
use crate::host::gui::{Align, CursorStyle, Face, MAX_PARTICLES, Op};
// A raw tab advances to the next SCREEN stop of the fixed `TAB_WIDTH`, as the CPU painter's
// does (`backend.rs`): the painter cannot read a program's `*tab-width*` (ADR-537), so a
// program that wants its own width expands tabs before it paints (`string/expand-tabs`).
use crate::host::text_width::{TAB_WIDTH, cluster_cells, cluster_cells_at};

const DEFAULT_FG: [u8; 3] = [0xe5, 0xe5, 0xe5];
const DEFAULT_BG_RGB: [u8; 3] = [12, 12, 16];

/// The most a scroll region shifts its ops, in pixels — the CPU painter's cap, so a
/// runaway `dy_frac` moves a region off-screen rather than to infinity.
const MAX_SCROLL_PX: f32 = 16384.0;

/// Both pipelines. A quad is four vertices of a triangle strip generated from the vertex
/// index (no vertex buffer): `corner` is (0,0) (1,0) (0,1) (1,1), and every per-quad
/// value rides in the instance buffer. Pixel coordinates, top-left origin, mapped to NDC
/// through the viewport uniform — the CPU painter's coordinate contract. A quad turns
/// about its own centre by `rot` radians (clockwise on screen, since y points down).
/// Every quad carries a clip rect `[x0 y0 x1 y1]` the fragment stage discards outside
/// of — how a cell region's band and a scroll region's grid-top clip are honoured.
const SHADER_SRC: &str = r#"
// `text.x` is 1/γ for the text contrast lift (`gui-text-contrast!`, ADR-337); 1.0 is none.
struct Viewport { size: vec2<f32>, text: vec2<f32> };
@group(0) @binding(0) var<uniform> viewport: Viewport;

fn corner(vi: u32) -> vec2<f32> {
    return vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
}

// The screen position of a quad corner: the corner's offset from the quad's centre,
// rotated, plus the centre.
fn place(rect: vec4<f32>, c: vec2<f32>, rot: f32) -> vec2<f32> {
    let centre = rect.xy + rect.zw * 0.5;
    let offset = (c - vec2<f32>(0.5, 0.5)) * rect.zw;
    let s = sin(rot);
    let co = cos(rot);
    return centre + vec2<f32>(offset.x * co - offset.y * s, offset.x * s + offset.y * co);
}

fn to_ndc(px: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / viewport.size.x * 2.0 - 1.0,
                     1.0 - px.y / viewport.size.y * 2.0, 0.0, 1.0);
}

fn clipped(pos: vec2<f32>, clip: vec4<f32>) -> bool {
    return pos.x < clip.x || pos.y < clip.y || pos.x >= clip.z || pos.y >= clip.w;
}

// --- solid quads -------------------------------------------------------------------

struct SolidInst {
    @location(0) rect: vec4<f32>,   // x, y, w, h in pixels
    @location(1) color: vec4<f32>,  // straight rgba, 0..1
    @location(2) rot: f32,          // radians about the centre
    @location(3) radius: f32,       // corner radius in pixels (with `aa`)
    @location(4) aa: f32,           // 1 = SDF edge (rounded / fractional), 0 = plain fill
    @location(5) clip: vec4<f32>,   // x0, y0, x1, y1 in pixels
};
struct SolidOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,  // the fragment's offset from the quad's top-left, px
    @location(2) @interpolate(flat) half: vec2<f32>,
    @location(3) @interpolate(flat) radius: f32,
    @location(4) @interpolate(flat) aa: f32,
    @location(5) @interpolate(flat) clip: vec4<f32>,
};

@vertex
fn vs_solid(@builtin(vertex_index) vi: u32, inst: SolidInst) -> SolidOut {
    var out: SolidOut;
    let c = corner(vi);
    out.pos = to_ndc(place(inst.rect, c, inst.rot));
    out.color = inst.color;
    out.local = c * inst.rect.zw;
    out.half = inst.rect.zw * 0.5;
    out.radius = inst.radius;
    out.aa = inst.aa;
    out.clip = inst.clip;
    return out;
}

// How much of the pixel a solid quad covers: 0 outside its clip, 1 for a plain fill,
// the SDF ramp for a rounded or fractional one.
fn solid_coverage(in: SolidOut) -> f32 {
    if (clipped(in.pos.xy, in.clip)) {
        return 0.0;
    }
    if (in.aa == 0.0) {
        return 1.0;
    }
    // The rounded-box signed distance from the pixel centre to the rect's rounded core
    // (the rect inset by the radius), as `fill_rrect` computes it: a 1 px coverage ramp
    // at the edge, so corners and fractional edges read smooth.
    let p = in.local - in.half;
    let r = min(in.radius, min(in.half.x, in.half.y));
    let q = abs(p) - (in.half - vec2<f32>(r, r));
    let d = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    return clamp(0.5 - d, 0.0, 1.0);
}

// sRGB-encoded 0..1 -> linear light, the exact curve the CPU painter's table holds.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

// Into an encoded (non-sRGB) target: the blend happens on the encoded values.
@fragment
fn fs_solid(in: SolidOut) -> @location(0) vec4<f32> {
    let coverage = solid_coverage(in);
    if (coverage <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}

// Into an sRGB view of the same frame: the colour is decoded here, the hardware blends
// in linear light and encodes on write — the CPU painter's `blend`, which is right for
// translucent geometry (a cursor wash), as ADR-358 says text is not.
@fragment
fn fs_solid_linear(in: SolidOut) -> @location(0) vec4<f32> {
    let coverage = solid_coverage(in);
    if (coverage <= 0.0) {
        discard;
    }
    return vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a * coverage);
}

// --- textured quads ------------------------------------------------------------------

struct TexInst {
    @location(0) rect: vec4<f32>,   // x, y, w, h in pixels
    @location(1) uv: vec4<f32>,     // u0, v0, du, dv in texture space (a negative extent mirrors)
    @location(2) tint: vec4<f32>,   // straight rgba, 0..1
    @location(3) mode: u32,         // 0 = colour, 1 = coverage mask recoloured with tint,
                                    // 2 = a mask whose coverage takes the contrast lift,
                                    // 3 / 4 = the same per colour channel (LCD, `fs_tex_lcd`)
    @location(4) rot: f32,          // radians about the centre
    @location(5) clip: vec4<f32>,   // x0, y0, x1, y1 in pixels
};
struct TexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
    @location(2) @interpolate(flat) mode: u32,
    @location(3) @interpolate(flat) clip: vec4<f32>,
};

@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

@vertex
fn vs_tex(@builtin(vertex_index) vi: u32, inst: TexInst) -> TexOut {
    var out: TexOut;
    let c = corner(vi);
    out.pos = to_ndc(place(inst.rect, c, inst.rot));
    out.uv = inst.uv.xy + c * inst.uv.zw;
    out.tint = inst.tint;
    out.mode = inst.mode;
    out.clip = inst.clip;
    return out;
}

@fragment
fn fs_tex(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    let t = textureSample(tex, samp, in.uv);
    // Text lerps in the ENCODED space (ADR-358): the target is non-sRGB, so the blend
    // unit composites the bytes as given — `blend_text`, exactly.
    // An LCD mask (3 / 4) reaches this pipeline only if no LCD one was made; its alpha is
    // the greatest of its channels' coverage, so it draws as grey text.
    if (in.mode == 1u || in.mode == 3u) {
        return vec4<f32>(in.tint.rgb, t.a * in.tint.a);
    }
    // The lift (ADR-337): `cov^(1/γ)`, decided per glyph where the CPU decides per
    // pixel (`mode` 2 is set only for text lighter than the ground it is drawn on).
    if (in.mode == 2u || in.mode == 4u) {
        return vec4<f32>(in.tint.rgb, pow(t.a, viewport.text.x) * in.tint.a);
    }
    return t * in.tint;
}

// Subpixel (LCD) text on an adapter WITHOUT dual-source blending: the same quads drawn three
// times, each pass writing one colour channel (`ColorWrites::RED` / `GREEN` / `BLUE`) with that
// channel's coverage as its alpha — so plain alpha blending gives `fg·cov + dst·(1 − cov)` per
// channel, over whatever is underneath. Mode 4 takes the lift per channel, as `fs_tex_lcd`.
fn lcd_coverage(in: TexOut) -> vec3<f32> {
    let cov = textureSample(tex, samp, in.uv).rgb;
    if (in.mode == 4u) {
        return pow(cov, vec3<f32>(viewport.text.x));
    }
    return cov;
}

@fragment
fn fs_tex_lcd_red(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    return vec4<f32>(in.tint.rgb, lcd_coverage(in).r * in.tint.a);
}

@fragment
fn fs_tex_lcd_green(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    return vec4<f32>(in.tint.rgb, lcd_coverage(in).g * in.tint.a);
}

@fragment
fn fs_tex_lcd_blue(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    return vec4<f32>(in.tint.rgb, lcd_coverage(in).b * in.tint.a);
}
"#;

/// Subpixel (LCD) text: [`SHADER_SRC`] plus a fragment entry with two outputs, blended as
/// `colour·coverage + dst·(1 − coverage)` PER CHANNEL (`BlendFactor::Src1`) — which a single
/// output cannot say, since its one alpha weighs all three channels alike. Compiled only on
/// an adapter with `DUAL_SOURCE_BLENDING` ([`lcd_shader_source`] puts the `enable` first).
/// The atlas texel's R, G and B are each channel's coverage, already in the subpixel order
/// the rasteriser was asked for; the lift (mode 4) is taken per channel, as the CPU's
/// `blend_text_rgb` does.
const LCD_SRC: &str = r#"
struct LcdOut {
    @location(0) @blend_src(0) colour: vec4<f32>,
    @location(0) @blend_src(1) coverage: vec4<f32>,
};

@fragment
fn fs_tex_lcd(in: TexOut) -> LcdOut {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    let t = textureSample(tex, samp, in.uv);
    var cov = t.rgb;
    if (in.mode == 4u) {
        cov = pow(cov, vec3<f32>(viewport.text.x));
    }
    var out: LcdOut;
    let alpha = max(cov.r, max(cov.g, cov.b)) * in.tint.a;
    out.colour = vec4<f32>(in.tint.rgb, alpha);
    out.coverage = vec4<f32>(cov * in.tint.a, alpha);
    return out;
}
"#;

/// The WGSL of the LCD text pipeline: the `enable` directive has to come before any
/// declaration, so it heads the shared source rather than living in [`LCD_SRC`].
fn lcd_shader_source() -> String {
    format!("enable dual_source_blending;\n{SHADER_SRC}{LCD_SRC}")
}

/// The blend the LCD pipeline draws with: each colour channel weighed by its own coverage
/// (`Src1`); alpha `a + dst·(1 − a)` with `a` the greatest coverage, as grey text's is under
/// `ALPHA_BLENDING`.
const LCD_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Src1,
        dst_factor: wgpu::BlendFactor::OneMinusSrc1,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrc1Alpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// The effects' shaders (`effects.rs` holds the CPU painter's copy of every formula in
/// them): lights drawn additively into a light map, here; the passes over a whole
/// texture (`POST_SRC`); and the particles — a compute shader that spawns and steps them
/// (`PARTICLES_SRC`) and the instanced quads that draw them straight out of the same
/// buffer (`PARTICLE_DRAW_SRC`).
const LIGHT_SRC: &str = r#"
struct Viewport { size: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> viewport: Viewport;

fn corner(vi: u32) -> vec2<f32> {
    return vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
}

fn to_ndc(px: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / viewport.size.x * 2.0 - 1.0,
                     1.0 - px.y / viewport.size.y * 2.0, 0.0, 1.0);
}

// --- lights: a quad per light, its falloff added into the light map --------------------

struct LightInst {
    @location(0) at: vec4<f32>,     // x, y, radius, intensity
    @location(1) color: vec4<f32>,  // rgb 0..1
};
struct LightOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,  // -1..1 across the light's square
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_light(@builtin(vertex_index) vi: u32, inst: LightInst) -> LightOut {
    var out: LightOut;
    let c = corner(vi);
    let r = inst.at.z;
    out.pos = to_ndc(inst.at.xy - vec2<f32>(r, r) + c * 2.0 * r);
    out.local = c * 2.0 - 1.0;
    out.color = inst.color.rgb * inst.at.w;
    return out;
}

// effects.rs `light_falloff`
@fragment
fn fs_light(in: LightOut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    if (d >= 1.0) {
        discard;
    }
    let k = 1.0 - d * d;
    return vec4<f32>(in.color * k * k, 1.0);
}

"#;

/// The passes over a whole texture: the light map multiplied onto the scene, bloom's bright
/// pass and blur, and the final composite with the CRT.
const POST_SRC: &str = r#"
// --- passes over a whole texture -----------------------------------------------------

struct Post {
    threshold: f32, knee: f32, bloom: f32, curvature: f32,
    scanlines: f32, vignette: f32, step: vec2<f32>,
    sigma: f32, taps: f32, _pad: vec2<f32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var glow: texture_2d<f32>;
@group(0) @binding(2) var smooth_sampler: sampler;
@group(0) @binding(3) var<uniform> post: Post;

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle that covers the target; uv 0..1 from the top-left.
@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FullOut {
    var out: FullOut;
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    out.uv = uv;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    return out;
}

// The light map, blended as source × destination onto the scene.
@fragment
fn fs_multiply(in: FullOut) -> @location(0) vec4<f32> {
    return vec4<f32>(min(textureSample(source, smooth_sampler, in.uv).rgb, vec3<f32>(1.0)), 1.0);
}

// effects.rs `apply_bloom`'s bright pass: the light above the threshold, soft-kneed.
@fragment
fn fs_bright(in: FullOut) -> @location(0) vec4<f32> {
    let c = textureSample(source, smooth_sampler, in.uv).rgb;
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    let k = clamp((luma - post.threshold) / post.knee, 0.0, 1.0);
    return vec4<f32>(c * k, 1.0);
}

// One axis of a gaussian blur, `post.step` apart (a texel along x, then along y).
@fragment
fn fs_blur(in: FullOut) -> @location(0) vec4<f32> {
    var sum = textureSample(source, smooth_sampler, in.uv).rgb;
    var weight = 1.0;
    let taps = i32(post.taps);
    for (var i = 1; i <= taps; i = i + 1) {
        let w = exp(-f32(i * i) / (2.0 * post.sigma * post.sigma));
        let off = post.step * f32(i);
        sum = sum + (textureSample(source, smooth_sampler, in.uv + off).rgb
                   + textureSample(source, smooth_sampler, in.uv - off).rgb) * w;
        weight = weight + 2.0 * w;
    }
    return vec4<f32>(sum / weight, 1.0);
}

// effects.rs `crt_uv`
fn crt_uv(uv: vec2<f32>, curvature: f32) -> vec2<f32> {
    let c = uv * 2.0 - 1.0;
    let b = vec2<f32>(c.x * (1.0 + curvature * c.y * c.y * 0.25),
                      c.y * (1.0 + curvature * c.x * c.x * 0.25));
    return b * 0.5 + 0.5;
}

// The scene (exact pixels), plus the bloom, through the CRT: effects.rs `apply_crt`.
@fragment
fn fs_final(in: FullOut) -> @location(0) vec4<f32> {
    let uv = crt_uv(in.uv, post.curvature);
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let dims = vec2<f32>(textureDimensions(source));
    let texel = vec2<i32>(min(uv * dims, dims - vec2<f32>(1.0)));
    var c = textureLoad(source, texel, 0).rgb;
    c = c + textureSample(glow, smooth_sampler, uv).rgb * post.bloom;
    let row = u32(in.pos.y);
    var shade = 1.0;
    if (row % 2u == 1u) {
        shade = 1.0 - post.scanlines;
    }
    let d = in.uv - vec2<f32>(0.5);
    shade = shade * clamp(1.0 - post.vignette * dot(d, d) * 2.0, 0.0, 1.0);
    return vec4<f32>(min(c, vec3<f32>(1.0)) * shade, 1.0);
}
"#;

/// The particle compute shader and the particle quads. Separate from `EFFECTS_SRC`
/// because the compute entry point's bindings (a writable buffer) are not allowed in a
/// vertex stage.
const PARTICLES_SRC: &str = r#"
struct Particle { pos: vec2<f32>, vel: vec2<f32>, age: f32, life: f32, _pad: vec2<f32> };
struct Params {
    origin: vec2<f32>, gravity: vec2<f32>,
    speed: vec2<f32>, life: vec2<f32>,
    size: vec2<f32>, angle: f32, spread: f32,
    color: vec4<f32>, color_end: vec4<f32>,
    dt: f32, drag: f32, spawn_first: u32, spawn_count: u32,
    capacity: u32, seed: u32, square: u32, _pad: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;

// effects.rs `hash` (lowbias32) and `rand`.
fn hash(x: u32) -> u32 {
    var v = x;
    v = v ^ (v >> 16u);
    v = v * 0x7feb352du;
    v = v ^ (v >> 15u);
    v = v * 0x846ca68bu;
    v = v ^ (v >> 16u);
    return v;
}
fn rand(seed: u32, slot: u32, k: u32) -> f32 {
    return f32(hash(seed ^ hash(slot * 4u + k)) >> 8u) / 16777216.0;
}

// effects.rs `spawn` then `integrate`.
@compute @workgroup_size(64)
fn cs_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.capacity) {
        return;
    }
    var p = particles[i];
    let rel = (i + params.capacity - params.spawn_first) % params.capacity;
    if (rel < params.spawn_count) {
        let angle = params.angle + (rand(params.seed, i, 0u) - 0.5) * params.spread;
        let speed = params.speed.x + (params.speed.y - params.speed.x) * rand(params.seed, i, 1u);
        let life = params.life.x + (params.life.y - params.life.x) * rand(params.seed, i, 2u);
        p.pos = params.origin;
        p.vel = vec2<f32>(cos(angle), sin(angle)) * speed;
        p.age = 0.0;
        p.life = max(life, 0.001);
    }
    if (p.life > 0.0 && p.age < p.life) {
        let keep = max(1.0 - params.drag * params.dt, 0.0);
        p.vel = (p.vel + params.gravity * params.dt) * keep;
        p.pos = p.pos + p.vel * params.dt;
        p.age = p.age + params.dt;
    }
    particles[i] = p;
}
"#;

const PARTICLE_DRAW_SRC: &str = r#"
struct Viewport { size: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> viewport: Viewport;
struct Params {
    origin: vec2<f32>, gravity: vec2<f32>,
    speed: vec2<f32>, life: vec2<f32>,
    size: vec2<f32>, angle: f32, spread: f32,
    color: vec4<f32>, color_end: vec4<f32>,
    dt: f32, drag: f32, spawn_first: u32, spawn_count: u32,
    capacity: u32, seed: u32, square: u32, _pad: u32,
};
@group(1) @binding(0) var<uniform> params: Params;

struct ParticleInst {
    @location(0) pos: vec2<f32>,
    @location(1) age_life: vec2<f32>,
};
struct ParticleOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_particle(@builtin(vertex_index) vi: u32, inst: ParticleInst) -> ParticleOut {
    var out: ParticleOut;
    let age = inst.age_life.x;
    let life = inst.age_life.y;
    if (life <= 0.0 || age >= life) {
        // a free slot: a quad with no area, nothing rasterised
        out.pos = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        out.local = vec2<f32>(0.0);
        out.color = vec4<f32>(0.0);
        return out;
    }
    let t = clamp(age / life, 0.0, 1.0);
    let size = mix(params.size.x, params.size.y, t);
    let c = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = inst.pos + (c - vec2<f32>(0.5)) * size;
    out.pos = vec4<f32>(px.x / viewport.size.x * 2.0 - 1.0,
                        1.0 - px.y / viewport.size.y * 2.0, 0.0, 1.0);
    out.local = c - vec2<f32>(0.5);
    out.color = mix(params.color, params.color_end, t);
    return out;
}

// effects.rs `particle_coverage`
@fragment
fn fs_particle(in: ParticleOut) -> @location(0) vec4<f32> {
    var coverage = 1.0;
    if (params.square == 0u) {
        let edge = clamp(1.0 - 2.0 * length(in.local), 0.0, 1.0);
        coverage = edge * edge;
    }
    let a = in.color.a * coverage;
    if (a <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, a);
}
"#;

/// The uniform an emitter's compute step and its quads read (`Params` in the shaders).
const PARTICLE_PARAMS_BYTES: u64 = 112;
/// One light instance: x, y, radius, intensity, then rgb and a pad.
const LIGHT_FLOATS: usize = 8;
/// The `Post` uniform: twelve floats, padded to 48 bytes.
const POST_BYTES: u64 = 48;

/// One solid instance: rect (4) + rgba (4) + rot (1) + radius (1) + aa (1) + clip (4).
const SOLID_FLOATS: usize = 15;
/// One textured instance: rect (4) + uv (4) + tint (4) + mode (1 u32 as its bits) + rot (1)
/// + clip (4).
const TEX_WORDS: usize = 18;

/// The glyph atlas page size. 2048² RGBA is 16 MB — a few thousand cells' worth of
/// glyphs at an editor size, and every device wgpu runs on allows at least 8192².
const ATLAS_SIZE: u32 = 2048;

/// Which texture a run of textured quads samples: a glyph-atlas page, or a texture the
/// app uploaded for its sprites.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TexKey {
    Atlas(usize),
    User(u32),
}

/// A texture the app uploaded, bound for the textured pipeline.
struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

/// One page of the glyph atlas: the texture plus a shelf packer's cursor. Glyphs are
/// packed left to right along a shelf whose height is the tallest glyph on it; a glyph
/// that does not fit starts a new shelf below, and one that does not fit the page starts
/// a new page. Simple, and enough: every glyph of one font size is about the same height.
struct AtlasPage {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    next_x: u32,
    next_y: u32,
    shelf_h: u32,
}

/// Where a rasterised cluster sits in the atlas: page + pixel rect + whether it is a
/// coverage mask (`mode` 1 in the shader) rather than a colour bitmap, and whether that
/// mask is per colour channel (`lcd`, `mode` 3).
#[derive(Clone, Copy)]
struct AtlasGlyph {
    page: usize,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    mono: bool,
    lcd: bool,
}

/// A growable instance buffer: re-created (never mapped) when a frame outgrows it.
/// `shadow` is what the buffer holds now, word for word, for `upload_changed` — the
/// frame instance buffers diff against it and write only the span that moved.
struct InstanceBuffer {
    buffer: wgpu::Buffer,
    capacity_bytes: u64,
    shadow: Vec<u32>,
}

impl InstanceBuffer {
    fn new(device: &wgpu::Device, label: &str, capacity_bytes: u64) -> InstanceBuffer {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity_bytes,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        InstanceBuffer {
            buffer,
            capacity_bytes,
            shadow: Vec::new(),
        }
    }

    /// Re-create the buffer, doubling, until `needed` bytes fit; what it held is gone.
    fn reserve(&mut self, device: &wgpu::Device, label: &str, needed: u64) {
        if needed > self.capacity_bytes {
            let mut capacity = self.capacity_bytes.max(1);
            while capacity < needed {
                capacity *= 2;
            }
            *self = InstanceBuffer::new(device, label, capacity);
        }
    }

    /// Upload `data`, growing (doubling) the buffer first when it does not fit.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, data: &[u8]) {
        self.reserve(device, label, data.len() as u64);
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, 0, data);
        }
    }

    /// Upload `words`, writing only the span that differs from what the buffer already
    /// holds (`changed_span`) — a frame that moved the cursor or one line rewrites a
    /// few hundred bytes, not the page. A buffer that has to grow is re-created and
    /// written whole. Returns the bytes written, for the trace.
    fn upload_changed(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        words: &[u32],
        reuse: bool,
    ) -> usize {
        self.reserve(device, label, std::mem::size_of_val(words) as u64);
        let span = if reuse {
            changed_span(&self.shadow, words)
        } else {
            (!words.is_empty()).then_some(0..words.len())
        };
        let Some(span) = span else {
            self.shadow.truncate(words.len());
            return 0;
        };
        queue.write_buffer(
            &self.buffer,
            (span.start * 4) as u64,
            as_bytes_u32(&words[span.clone()]),
        );
        self.shadow.resize(words.len(), 0);
        self.shadow[span.clone()].copy_from_slice(&words[span.clone()]);
        span.len() * 4
    }
}

/// The word range of `new` that differs from `old` — what an upload over a buffer that
/// holds `old` must write to make it hold `new`. `None` when nothing differs (a shorter
/// `new` whose words all match needs no write: the tail past it is never drawn). The
/// search compares 64-word chunks first, so an unchanged page costs a memcmp.
fn changed_span(old: &[u32], new: &[u32]) -> Option<std::ops::Range<usize>> {
    const CHUNK: usize = 64;
    let common = old.len().min(new.len());
    let (old_common, new_common) = (&old[..common], &new[..common]);
    let first_chunk = old_common
        .chunks(CHUNK)
        .zip(new_common.chunks(CHUNK))
        .position(|(a, b)| a != b);
    let first = match first_chunk {
        Some(chunk) => {
            let base = chunk * CHUNK;
            base + old_common[base..]
                .iter()
                .zip(&new_common[base..])
                .position(|(a, b)| a != b)
                .expect("a differing chunk has a differing word")
        }
        None if new.len() > common => common,
        None => return None,
    };
    let end = if new.len() > common {
        new.len()
    } else {
        // the last differing word, searched from the back the same way
        let tail = common - first;
        let last_from_back = old_common[first..]
            .rchunks(CHUNK)
            .zip(new_common[first..].rchunks(CHUNK))
            .position(|(a, b)| a != b)
            .expect("the first differing word is in this range");
        let chunk_end = tail - last_from_back * CHUNK;
        let chunk_start = chunk_end.saturating_sub(CHUNK);
        let within = old_common[first + chunk_start..first + chunk_end]
            .iter()
            .zip(&new_common[first + chunk_start..first + chunk_end])
            .rposition(|(a, b)| a != b)
            .expect("a differing chunk has a differing word");
        first + chunk_start + within + 1
    };
    Some(first..end)
}

/// A run of consecutive quads drawn with one pipeline state: `count` instances starting
/// at `first` in that pipeline's instance buffer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Batch {
    /// `linear`: some quad in the run is translucent, so the run is drawn through the
    /// sRGB view and blends in linear light (`segments`). A run of opaque fills — their
    /// anti-aliased rims included — rides whichever pass is open.
    Solid {
        first: u32,
        count: u32,
        linear: bool,
    },
    /// `lcd`: the quads are per-channel text masks (`mode` 3 / 4), drawn with the LCD
    /// pipeline; a run never mixes them with grey or colour quads.
    Tex {
        key: TexKey,
        first: u32,
        count: u32,
        lcd: bool,
    },
    /// Emitter `id`'s particles, straight out of its buffer.
    Particles { id: u32 },
}

/// A solid quad's edge treatment: a plain fill, or the SDF ramp (with a corner radius in
/// pixels) a rounded or sub-cell rect wants.
#[derive(Clone, Copy)]
enum Edge {
    Plain,
    Rounded(f32),
}

/// The frame under construction: both instance buffers' contents and the batch list, in
/// op order. Solids and textured quads keep separate buffers (different strides) and the
/// batch list says in which order to draw runs of each. `clip` is the rect every quad
/// pushed while it is set is clipped to — the whole viewport outside any region.
struct FrameBuilder {
    solids: Vec<f32>,
    texs: Vec<u32>,
    batches: Vec<Batch>,
    fw: f32,
    fh: f32,
    clip: [f32; 4],
    /// The batch the top-level `Overlay` starts at: no batch merges across it, so the scene
    /// and the unlit layer draw as separate runs.
    overlay_at: Option<usize>,
    /// How many regions deep the walk is (an `Overlay` counts only at the top).
    nested: u32,
}

impl FrameBuilder {
    fn new(fw: f32, fh: f32) -> FrameBuilder {
        FrameBuilder {
            solids: Vec::new(),
            texs: Vec::new(),
            batches: Vec::new(),
            fw,
            fh,
            clip: [0.0, 0.0, fw, fh],
            overlay_at: None,
            nested: 0,
        }
    }

    /// Whether a quad can put a pixel on screen and inside the clip. A rotated quad is
    /// judged by the disc its rotation sweeps, so a long thin quad turned across the
    /// viewport's corner is kept rather than culled by its unrotated rect.
    fn visible(&self, x: f32, y: f32, w: f32, h: f32, rot: f32) -> bool {
        let [cx0, cy0, cx1, cy1] = self.clip;
        if cx1 <= cx0 || cy1 <= cy0 {
            return false;
        }
        if rot == 0.0 {
            return quad_visible(x, y, w, h, self.fw, self.fh)
                && x < cx1
                && y < cy1
                && x + w > cx0
                && y + h > cy0;
        }
        let radius = (w * w + h * h).sqrt() * 0.5;
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        quad_visible(
            cx - radius,
            cy - radius,
            radius * 2.0,
            radius * 2.0,
            self.fw,
            self.fh,
        )
    }

    /// A solid quad; `rgba[3]` is its alpha.
    fn solid(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4], rot: f32, edge: Edge) {
        if !self.visible(x, y, w, h, rot) {
            return;
        }
        let (radius, aa) = match edge {
            Edge::Plain => (0.0, 0.0),
            Edge::Rounded(r) => (r.max(0.0), 1.0),
        };
        let index = (self.solids.len() / SOLID_FLOATS) as u32;
        self.solids.extend_from_slice(&[
            x,
            y,
            w,
            h,
            rgba[0] as f32 / 255.0,
            rgba[1] as f32 / 255.0,
            rgba[2] as f32 / 255.0,
            rgba[3] as f32 / 255.0,
            rot,
            radius,
            aa,
            self.clip[0],
            self.clip[1],
            self.clip[2],
            self.clip[3],
        ]);
        // Only a translucent fill needs linear light. An opaque shape's anti-aliased rim
        // blends in the encoded space, as text does, and rides whichever pass is open —
        // a pane divider or a mode-line chip between two runs of text costs no pass switch.
        let blends = rgba[3] < 255;
        let merge = self.can_merge();
        match self.batches.last_mut() {
            Some(Batch::Solid { count, linear, .. }) if merge => {
                *count += 1;
                *linear |= blends;
            }
            _ => self.batches.push(Batch::Solid {
                first: index,
                count: 1,
                linear: blends,
            }),
        }
    }

    /// Whether a quad may join the last batch: not when that batch ends the scene.
    fn can_merge(&self) -> bool {
        self.overlay_at.is_none_or(|at| at < self.batches.len())
    }

    /// Solid instances recorded earlier (`FragmentCache::record`), already culled and
    /// clipped: they join the open solid batch as the first of them would have
    /// (`solid`), and the rest follow it — `linear` when any of them is translucent.
    fn push_solids(&mut self, instances: &[f32], linear: bool) {
        let count = (instances.len() / SOLID_FLOATS) as u32;
        if count == 0 {
            return;
        }
        let first = (self.solids.len() / SOLID_FLOATS) as u32;
        self.solids.extend_from_slice(instances);
        let merge = self.can_merge();
        match self.batches.last_mut() {
            Some(Batch::Solid {
                count: open,
                linear: open_linear,
                ..
            }) if merge => {
                *open += count;
                *open_linear |= linear;
            }
            _ => self.batches.push(Batch::Solid {
                first,
                count,
                linear,
            }),
        }
    }

    /// Textured instances recorded earlier, sampling `key`: `push_solids`' twin.
    fn push_texs(&mut self, key: TexKey, lcd: bool, instances: &[u32]) {
        let count = (instances.len() / TEX_WORDS) as u32;
        if count == 0 {
            return;
        }
        let first = (self.texs.len() / TEX_WORDS) as u32;
        self.texs.extend_from_slice(instances);
        let merge = self.can_merge();
        match self.batches.last_mut() {
            Some(Batch::Tex {
                key: open_key,
                count: open,
                lcd: open_lcd,
                ..
            }) if *open_key == key && *open_lcd == lcd && merge => *open += count,
            _ => self.batches.push(Batch::Tex {
                key,
                first,
                count,
                lcd,
            }),
        }
    }

    /// A plain opaque solid — the common cell-aligned fill.
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
        self.solid(x, y, w, h, [rgb[0], rgb[1], rgb[2], 255], 0.0, Edge::Plain);
    }

    /// A textured quad sampling `uv` (`[u0 v0 du dv]`, texture space) of `key`; `mode`
    /// is the shader's: 0 a colour texel, 1 a coverage mask tinted, 2 a mask lifted, 3 / 4
    /// the same per colour channel (`glyph_mode`).
    #[allow(clippy::too_many_arguments)]
    fn textured(
        &mut self,
        key: TexKey,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        tint: [u8; 4],
        mode: u32,
        rot: f32,
    ) {
        if !self.visible(x, y, w, h, rot) {
            return;
        }
        let index = (self.texs.len() / TEX_WORDS) as u32;
        self.texs.extend_from_slice(&[
            x.to_bits(),
            y.to_bits(),
            w.to_bits(),
            h.to_bits(),
            uv[0].to_bits(),
            uv[1].to_bits(),
            uv[2].to_bits(),
            uv[3].to_bits(),
            (tint[0] as f32 / 255.0).to_bits(),
            (tint[1] as f32 / 255.0).to_bits(),
            (tint[2] as f32 / 255.0).to_bits(),
            (tint[3] as f32 / 255.0).to_bits(),
            mode,
            rot.to_bits(),
            self.clip[0].to_bits(),
            self.clip[1].to_bits(),
            self.clip[2].to_bits(),
            self.clip[3].to_bits(),
        ]);
        let merge = self.can_merge();
        let lcd = mode >= 3;
        match self.batches.last_mut() {
            Some(Batch::Tex {
                key: k,
                count,
                lcd: l,
                ..
            }) if *k == key && *l == lcd && merge => *count += 1,
            _ => self.batches.push(Batch::Tex {
                key,
                first: index,
                count: 1,
                lcd,
            }),
        }
    }
}

// ---- the run cache: damage tracking for the GPU target (ADR-440) -------------------
//
// The CPU painter retains its canvas and re-rasterises only the rows a frame changed
// (`paint::strip_diff`). A swapchain image is not retained — every frame is drawn whole
// — but drawing is the cheap half on a GPU. The expensive half is on the CPU: walking a
// text op's graphemes, measuring each cluster and looking its glyph up in the atlas, for
// every op of every frame. So the GPU target retains the RESULT of that walk instead: a
// text run's quads, relative to where the run starts, keyed by everything they depend
// on. A frame that repeats a run — every line a keystroke did not touch, every line a
// scroll merely moved, every frame of a cursor blink — replays its quads with an offset
// (`RunCache::take`); only a changed run is shaped again. The replay then lands in the
// instance buffers, which are written only where they changed (`changed_span`), so a
// blink uploads the cursor's few quads and a typed character the tail after it.
//
// Keyed WITHOUT position, deliberately: a run's row and the scroll offset only move it,
// so a scroll (whole lines or a sub-cell glide) reuses every run it keeps on screen —
// the GPU's answer to the CPU painter's scroll blit, with no blit. Its column is in the
// key for a cell run (a tab advances to the next SCREEN stop, so where a run starts
// changes its shape); its alignment for a pixel run. The metrics, the resolved family
// and the window background (the contrast lift's ground) are in the key too, so a font
// or theme change misses rather than replays stale quads.

/// One quad of a cached run, relative to the run's anchor (`expand` adds the anchor).
#[derive(Clone, Copy)]
enum RunQuad {
    Fill {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgb: [u8; 3],
    },
    Glyph {
        page: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        tint: [u8; 4],
        mode: u32,
    },
}

/// Where a run is placed: on the cell grid starting at a column, or in pixel space
/// with an alignment about its `x`.
#[derive(Clone, Copy, PartialEq, Hash)]
enum RunPlace {
    Cell { col: u16 },
    Pixel { align: u8 },
}

/// Everything a run's quads depend on except where it is drawn.
#[derive(Clone, Copy, PartialEq, Hash)]
struct RunContext {
    place: RunPlace,
    /// cell width, cell height, font px (bits) and the resolved default family
    metrics: [u32; 4],
    /// the window background: the ground a run without `:bg` is lifted against
    ground: [u8; 3],
    /// the font registration the glyphs were baked under, whether ligature units were
    /// shaped (ADR-442), and the subpixel order (`GpuWindow::lcd_order`): each changes
    /// the quads a run produces
    fonts: (u64, bool, u8),
}

/// A cached run: its key (checked on a hash hit, so a collision is a miss, never a
/// wrong picture) and its quads.
struct TextRun {
    text: String,
    face: Face,
    context: RunContext,
    quads: Vec<RunQuad>,
}

/// The runs of the last frame and of the one being built. A run is looked up in this
/// frame's map first (two identical lines share one), then taken from the last frame's;
/// whatever the last frame had and this one did not use is dropped when the frame ends
/// (`end_frame`), so the cache holds one screen's runs, never a history.
#[derive(Default)]
struct RunCache {
    current: HashMap<u64, TextRun>,
    previous: HashMap<u64, TextRun>,
    /// this frame's runs replayed / shaped, for the trace
    hits: u32,
    misses: u32,
}

fn run_hash(text: &str, face: &Face, context: &RunContext) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text, face, context).hash(&mut hasher);
    hasher.finish()
}

impl RunCache {
    /// The run's quads when this frame or the last already shaped it.
    fn take(
        &mut self,
        hash: u64,
        text: &str,
        face: &Face,
        context: &RunContext,
    ) -> Option<&[RunQuad]> {
        let matches =
            |run: &TextRun| run.text == text && run.face == *face && run.context == *context;
        if !self.current.get(&hash).is_some_and(matches) {
            let run = self.previous.remove(&hash).filter(matches)?;
            self.current.insert(hash, run);
        }
        self.hits += 1;
        self.current.get(&hash).map(|run| run.quads.as_slice())
    }

    /// Keep a run the last frame drew for the next one without replaying it — a run inside a
    /// fragment whose instances were pushed whole (`FragmentCache`).
    fn keep(&mut self, hash: u64) {
        if !self.current.contains_key(&hash)
            && let Some(run) = self.previous.remove(&hash)
        {
            self.current.insert(hash, run);
        }
    }

    /// Remember a run this frame shaped.
    fn put(&mut self, hash: u64, run: TextRun) {
        self.misses += 1;
        self.current.insert(hash, run);
    }

    /// The frame is drawn: what it used is the next frame's to reuse, and nothing else.
    fn end_frame(&mut self) {
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        self.hits = 0;
        self.misses = 0;
    }
}

/// `BROOD_GUI_DAMAGE=0` turns the GPU target's reuse off as it does the CPU painter's
/// row diff: every run shaped and every instance buffer written whole, each frame — the
/// escape hatch if reuse ever shows a stale picture. Read once.
fn reuse_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("BROOD_GUI_DAMAGE")
            .map(|v| v != "0")
            .unwrap_or(true)
    })
}

/// An encoded (sRGB) clear colour as linear light, for a pass drawn through the sRGB view —
/// which encodes what it is given, so the encoded value would be encoded twice.
fn decode_clear(colour: wgpu::Color) -> wgpu::Color {
    let decode = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    wgpu::Color {
        r: decode(colour.r),
        g: decode(colour.g),
        b: decode(colour.b),
        a: colour.a,
    }
}

/// The draw order of `batches` as render passes: `(linear, range)` each, where a linear
/// pass draws through the swapchain's sRGB view (translucent solids, blended in linear
/// light) and the rest through the encoded view (text, sprites,
/// particles — and an opaque fill, which looks the same in either, joins whichever pass
/// is open). Consecutive batches of one kind share a pass, so a frame pays a pass switch
/// only where its translucent geometry and its text interleave. Without an sRGB view
/// (`linear_available` false) it is one encoded pass, as before.
fn segments(batches: &[Batch], linear_available: bool) -> Vec<(bool, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space: Option<bool> = None;
    for (index, batch) in batches.iter().enumerate() {
        let wants = match batch {
            Batch::Solid { linear, .. } => (*linear && linear_available).then_some(true),
            Batch::Tex { .. } | Batch::Particles { .. } => Some(false),
        };
        let Some(wants) = wants else {
            continue;
        };
        match space {
            None => space = Some(wants),
            Some(open) if open != wants => {
                out.push((open, start..index));
                start = index;
                space = Some(wants);
            }
            Some(_) => {}
        }
    }
    out.push((space.unwrap_or(false), start..batches.len()));
    out
}

/// Push a shaped run's quads onto the frame at `(anchor_x, anchor_y)`. Each goes through
/// the builder's own fill / textured, so culling, the clip band and batching are what
/// they would be had the run been shaped this frame.
fn replay(fb: &mut FrameBuilder, quads: &[RunQuad], anchor_x: f32, anchor_y: f32) {
    for quad in quads {
        match *quad {
            RunQuad::Fill { x, y, w, h, rgb } => fb.fill(anchor_x + x, anchor_y + y, w, h, rgb),
            RunQuad::Glyph {
                page,
                x,
                y,
                w,
                h,
                uv,
                tint,
                mode,
            } => fb.textured(
                TexKey::Atlas(page),
                anchor_x + x,
                anchor_y + y,
                w,
                h,
                uv,
                tint,
                mode,
                0.0,
            ),
        }
    }
}

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
struct FragmentContext {
    /// the grid it is expanded on (`Grid`: origin, cell size, scroll shift), as bits
    grid: [u32; 5],
    /// the clip band in effect and the viewport, which decide what was culled
    clip: [u32; 4],
    viewport: [u32; 2],
    /// the renderer's metrics (cell width, cell height, font px bits) and line height
    /// bits — a cell region inside derives its own metrics from these
    metrics: [u32; 4],
    /// the resolved default family, and the window background (the ground a run's
    /// contrast lift is judged against)
    family: u32,
    ground: [u8; 3],
    /// the font registration, ligatures and subpixel order (`RunContext::fonts`)
    fonts: (u64, bool, u8),
    /// the scale factor's bits: a hairline snaps to whole device pixels by it
    scale: u64,
    /// how many times the app's textures were uploaded or freed
    textures: u64,
    /// the blink phase — compared only for a fragment that holds a blinking caret
    caret_hidden: bool,
}

/// A run of one fragment's instances drawn with one pipeline state, as `Batch` holds it
/// minus where the run starts (that is wherever the fragment lands next frame).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Segment {
    Solid { count: u32, linear: bool },
    Tex { key: TexKey, count: u32, lcd: bool },
    Particles { id: u32 },
}

/// One fragment's expansion: its instances in op order and the segments they batch into,
/// the text runs and nested fragments it used (carried forward with it on a hit, so a
/// later frame that changes the fragment still finds its unchanged lines shaped), and
/// the ops themselves — held so the allocation, whose address is the key, cannot be freed
/// and reused by another fragment while the entry lives.
struct FragmentQuads {
    ops: std::sync::Arc<[Op]>,
    context: FragmentContext,
    blinks: bool,
    solids: Vec<f32>,
    texs: Vec<u32>,
    segments: Vec<Segment>,
    runs: Vec<u64>,
    nested: Vec<(usize, FragmentContext)>,
}

impl FragmentQuads {
    /// Whether this entry is the expansion of `ops` in `context`.
    fn matches(&self, ops: &std::sync::Arc<[Op]>, context: &FragmentContext) -> bool {
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
struct FragmentMark {
    solids: usize,
    texs: usize,
    batches: usize,
    /// the instance count of the batch the fragment's first quads may merge into
    open_count: u32,
    overlay_at: Option<usize>,
    runs: usize,
    nested: usize,
}

/// The fragments of the last frame and of the one being built, by the address of their
/// ops (a fragment drawn at two places has an entry per context), plus this frame's log
/// of the runs and fragments used — what an enclosing fragment's entry records.
#[derive(Default)]
struct FragmentCache {
    current: HashMap<usize, Vec<FragmentQuads>>,
    previous: HashMap<usize, Vec<FragmentQuads>>,
    runs_used: Vec<u64>,
    fragments_used: Vec<(usize, FragmentContext)>,
    /// this frame's fragments replayed / expanded, for the trace
    hits: u32,
    misses: u32,
}

fn fragment_address(ops: &std::sync::Arc<[Op]>) -> usize {
    ops.as_ptr() as *const () as usize
}

impl FragmentCache {
    /// The entry for `ops` in `context`, out of this frame's (a fragment drawn twice) or the
    /// last frame's; the caller replays it and hands it back (`restore`).
    fn take(
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
    fn restore(&mut self, entry: FragmentQuads) {
        let address = fragment_address(&entry.ops);
        self.current.entry(address).or_default().push(entry);
    }

    /// Carry entry `(address, context)` from the last frame into this one without
    /// replaying it — a fragment nested in one that was replayed whole.
    fn carry(&mut self, address: usize, context: &FragmentContext) {
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
    fn mark(&self, fb: &FrameBuilder) -> FragmentMark {
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
    fn record(
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
    fn end_frame(&mut self) {
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
fn replay_fragment(fb: &mut FrameBuilder, entry: &FragmentQuads) {
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

/// The sRGB sibling of the swapchain `format` that translucent geometry is drawn
/// through, when the surface may be viewed in it (`SURFACE_VIEW_FORMATS`) and `format`
/// has one; `None` otherwise, and then every quad blends in the encoded space.
fn linear_view_format(
    format: wgpu::TextureFormat,
    views_allowed: bool,
) -> Option<wgpu::TextureFormat> {
    let linear = format.add_srgb_suffix();
    (views_allowed && linear != format && format.remove_srgb_suffix() == format).then_some(linear)
}

/// A render pipeline over one shader module's entry points, the parts every pipeline here
/// shares spelled once.
#[allow(clippy::too_many_arguments)]
fn pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    (vertex, fragment): (&str, &str),
    layouts: &[&wgpu::BindGroupLayout],
    instances: Option<wgpu::VertexBufferLayout>,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    let buffers = [instances];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers: if buffers[0].is_some() { &buffers } else { &[] },
        },
        primitive: wgpu::PrimitiveState {
            topology,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Colour added onto what is there: a light into the light map.
const ADD: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::OVER,
};

/// Colour × alpha added onto what is there: an additive particle.
const ADD_ALPHA: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::OVER,
};

/// What is there × the colour drawn: the light map onto the scene.
const MULTIPLY: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Dst,
        dst_factor: wgpu::BlendFactor::Zero,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// The lighting and post-processing pipelines, made the first time a frame asks for an
/// effect, and the frame-sized textures they render through.
struct EffectsGpu {
    light_pipeline: wgpu::RenderPipeline,
    multiply_pipeline: wgpu::RenderPipeline,
    bright_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    final_pipeline: wgpu::RenderPipeline,
    post_layout: wgpu::BindGroupLayout,
    smooth_sampler: wgpu::Sampler,
    light_instances: InstanceBuffer,
    /// One `Post` uniform per pass that reads one: bright, blur across, blur down, final.
    post_bright: wgpu::Buffer,
    post_across: wgpu::Buffer,
    post_down: wgpu::Buffer,
    post_final: wgpu::Buffer,
    targets: Option<EffectTargets>,
}

/// The textures an effect frame renders through, at one frame size: the scene, the light
/// map, and bloom's two half-size ping-pong targets — with each pass's bind group.
struct EffectTargets {
    size: (u32, u32),
    scene: wgpu::TextureView,
    light: wgpu::TextureView,
    bloom_a: wgpu::TextureView,
    bloom_b: wgpu::TextureView,
    half: (u32, u32),
    multiply: wgpu::BindGroup,
    bright: wgpu::BindGroup,
    across: wgpu::BindGroup,
    down: wgpu::BindGroup,
    composite: wgpu::BindGroup,
}

impl EffectsGpu {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        viewport_layout: &wgpu::BindGroupLayout,
    ) -> EffectsGpu {
        let light_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui lights"),
            source: wgpu::ShaderSource::Wgsl(LIGHT_SRC.into()),
        });
        let post_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui post"),
            source: wgpu::ShaderSource::Wgsl(POST_SRC.into()),
        });
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let strip = wgpu::PrimitiveTopology::TriangleStrip;
        let list = wgpu::PrimitiveTopology::TriangleList;
        let light_pipeline = pipeline(
            device,
            "lights",
            &light_module,
            ("vs_light", "fs_light"),
            &[viewport_layout],
            Some(wgpu::VertexBufferLayout {
                array_stride: (LIGHT_FLOATS * 4) as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
            }),
            format,
            Some(ADD),
            strip,
        );
        let full = |label: &str, fragment: &str, blend: Option<wgpu::BlendState>| {
            pipeline(
                device,
                label,
                &post_module,
                ("vs_full", fragment),
                &[&post_layout],
                None,
                format,
                blend,
                list,
            )
        };
        let uniform = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: POST_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        EffectsGpu {
            light_pipeline,
            multiply_pipeline: full("light map", "fs_multiply", Some(MULTIPLY)),
            bright_pipeline: full("bloom bright pass", "fs_bright", None),
            blur_pipeline: full("bloom blur", "fs_blur", None),
            final_pipeline: full("final composite", "fs_final", None),
            smooth_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("linear"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            }),
            post_layout,
            light_instances: InstanceBuffer::new(device, "light instances", 4 * 1024),
            post_bright: uniform("post: bright"),
            post_across: uniform("post: blur across"),
            post_down: uniform("post: blur down"),
            post_final: uniform("post: final"),
            targets: None,
        }
    }

    /// The textures for a `w`×`h` frame, made (or remade, on a resize) as needed.
    fn targets(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        w: u32,
        h: u32,
    ) -> &EffectTargets {
        if self.targets.as_ref().is_none_or(|t| t.size != (w, h)) {
            let view = |label: &str, w: u32, h: u32| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            let half = ((w / 2).max(1), (h / 2).max(1));
            let scene = view("scene", w, h);
            let light = view("light map", w, h);
            let bloom_a = view("bloom a", half.0, half.1);
            let bloom_b = view("bloom b", half.0, half.1);
            // the `glow` of every pass but the final one, which reads no glow: a pass may not
            // bind a texture it renders into
            let none = view("no glow", 1, 1);
            let group = |label: &str,
                         source: &wgpu::TextureView,
                         glow: &wgpu::TextureView,
                         post: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: &self.post_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(glow),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.smooth_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: post.as_entire_binding(),
                        },
                    ],
                })
            };
            let targets = EffectTargets {
                size: (w, h),
                multiply: group("light map", &light, &none, &self.post_final),
                bright: group("bright pass", &scene, &none, &self.post_bright),
                across: group("blur across", &bloom_a, &none, &self.post_across),
                down: group("blur down", &bloom_b, &none, &self.post_down),
                composite: group("final", &scene, &bloom_a, &self.post_final),
                scene,
                light,
                bloom_a,
                bloom_b,
                half,
            };
            self.targets = Some(targets);
        }
        self.targets.as_ref().expect("made above")
    }
}

/// The particle pipelines — the compute step and the two ways of drawing a particle —
/// made the first time a frame carries an emitter, and each live emitter's buffers.
struct ParticlesGpu {
    step_pipeline: wgpu::ComputePipeline,
    step_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    blend_pipeline: wgpu::RenderPipeline,
    add_pipeline: wgpu::RenderPipeline,
    emitters: HashMap<u32, GpuEmitter>,
}

/// One emitter on the device: its ring of particles (`capacity` × 32 bytes, read by the
/// compute step and drawn as instances), and the `Params` uniform both read.
struct GpuEmitter {
    capacity: u32,
    particles: wgpu::Buffer,
    params: wgpu::Buffer,
    step_group: wgpu::BindGroup,
    draw_group: wgpu::BindGroup,
    additive: bool,
}

impl ParticlesGpu {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        viewport_layout: &wgpu::BindGroupLayout,
    ) -> ParticlesGpu {
        let step_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui particle step"),
            source: wgpu::ShaderSource::Wgsl(PARTICLES_SRC.into()),
        });
        let draw_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui particles"),
            source: wgpu::ShaderSource::Wgsl(PARTICLE_DRAW_SRC.into()),
        });
        let uniform_entry = |visibility| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let step_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle step"),
            entries: &[
                uniform_entry(wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle draw"),
            entries: &[uniform_entry(
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            )],
        });
        let step_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle step"),
            bind_group_layouts: &[Some(&step_layout)],
            immediate_size: 0,
        });
        let step_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("particle step"),
            layout: Some(&step_pipeline_layout),
            module: &step_module,
            entry_point: Some("cs_step"),
            compilation_options: Default::default(),
            cache: None,
        });
        let instances = || wgpu::VertexBufferLayout {
            array_stride: (crate::host::gui::effects::PARTICLE_FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 16,
                    shader_location: 1,
                },
            ],
        };
        let draw = |label: &str, blend| {
            pipeline(
                device,
                label,
                &draw_module,
                ("vs_particle", "fs_particle"),
                &[viewport_layout, &draw_layout],
                Some(instances()),
                format,
                Some(blend),
                wgpu::PrimitiveTopology::TriangleStrip,
            )
        };
        ParticlesGpu {
            blend_pipeline: draw("particles", wgpu::BlendState::ALPHA_BLENDING),
            add_pipeline: draw("additive particles", ADD_ALPHA),
            step_pipeline,
            step_layout,
            draw_layout,
            emitters: HashMap::new(),
        }
    }

    /// Match the device's emitters to this paint's steps — a new one gets its buffers, a
    /// new capacity a new ring, a finished one is dropped — and write each step's uniform.
    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, steps: &[EmitterStep]) {
        self.emitters
            .retain(|id, _| steps.iter().any(|step| step.id == *id));
        for step in steps {
            let capacity = step.spec.max.clamp(1, MAX_PARTICLES);
            if self
                .emitters
                .get(&step.id)
                .is_none_or(|e| e.capacity != capacity)
            {
                let emitter = self.emitter(device, capacity);
                self.emitters.insert(step.id, emitter);
            }
            let emitter = self.emitters.get_mut(&step.id).expect("inserted above");
            emitter.additive = step.spec.additive;
            queue.write_buffer(
                &emitter.params,
                0,
                as_bytes_u32(&params_words(step, capacity)),
            );
        }
    }

    fn emitter(&self, device: &wgpu::Device, capacity: u32) -> GpuEmitter {
        // zero-initialised by wgpu: every slot starts free (`life` 0)
        let particles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles"),
            size: capacity as u64 * (crate::host::gui::effects::PARTICLE_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle params"),
            size: PARTICLE_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let step_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle step"),
            layout: &self.step_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particles.as_entire_binding(),
                },
            ],
        });
        let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle draw"),
            layout: &self.draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        GpuEmitter {
            capacity,
            particles,
            params,
            step_group,
            draw_group,
            additive: false,
        }
    }
}

/// A step as the shaders' `Params` uniform, word by word in its declared order.
fn params_words(step: &EmitterStep, capacity: u32) -> [u32; 28] {
    let s = &step.spec;
    let f = |x: f32| x.to_bits();
    let channel = |c: [u8; 4], i: usize| f(c[i] as f32 / 255.0);
    [
        f(step.x),
        f(step.y),
        f(s.gravity[0]),
        f(s.gravity[1]),
        f(s.speed[0]),
        f(s.speed[1]),
        f(s.life[0]),
        f(s.life[1]),
        f(s.size[0]),
        f(s.size[1]),
        f(s.angle),
        f(s.spread),
        channel(s.color, 0),
        channel(s.color, 1),
        channel(s.color, 2),
        channel(s.color, 3),
        channel(s.color_end, 0),
        channel(s.color_end, 1),
        channel(s.color_end, 2),
        channel(s.color_end, 3),
        f(step.dt),
        f(s.drag),
        step.spawn_first,
        step.spawn_count,
        capacity,
        step.seed,
        u32::from(s.square),
        0,
    ]
}

/// Where the ops being expanded land: the grid origin and cell size in effect (a
/// `CellRegion` installs its own), the scroll shift an enclosing `ScrollRegion` applies,
/// and the clip rect to restore when a region ends. The CPU painter threads the same
/// values through `render_ops`.
#[derive(Clone, Copy)]
struct Grid {
    ox: f32,
    oy: f32,
    cw: f32,
    ch: f32,
    scroll_dy: f32,
}

/// How a device draws subpixel (LCD) text: in ONE pass with dual-source blending (each
/// channel weighed by its own coverage, `LCD_BLEND`), or in three, one per colour channel
/// under a write mask (`fs_tex_lcd_red` / `_green` / `_blue`) — the same result over any
/// destination, two more draws per run of LCD glyphs, and no adapter feature needed.
enum LcdText {
    DualSource(wgpu::RenderPipeline),
    PerChannel([wgpu::RenderPipeline; 3]),
}

/// Where a `GpuWindow`'s frames are drawn: the window's swapchain, or — for the headless
/// harness that checks the GPU path on a real device with no window on anyone's screen —
/// one texture of the size and format a window would have. Field order matters: the
/// surface was created from the window's raw handles, so it must drop before the window —
/// Rust drops fields in declaration order.
enum Target {
    Window {
        surface: wgpu::Surface<'static>,
        window: Rc<Window>,
    },
    #[cfg(test)]
    Offscreen { texture: wgpu::Texture },
}

/// One open window's GPU state: the wgpu surface + device, the two instanced-quad
/// pipelines, the glyph atlas and the app's textures. Lives on the GUI thread. The target
/// comes first: it must drop before the device it was configured on.
pub struct GpuWindow {
    target: Target,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    configured: (u32, u32),
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    solid_pipeline: wgpu::RenderPipeline,
    /// The solid pipeline into the swapchain's sRGB view, for translucent and
    /// anti-aliased geometry (linear-light blending); `None` when the surface cannot be
    /// viewed in a second format, and then every quad blends in the encoded space.
    solid_linear_pipeline: Option<wgpu::RenderPipeline>,
    /// That sRGB view's format.
    linear_format: Option<wgpu::TextureFormat>,
    solid_instances: InstanceBuffer,
    tex_pipeline: wgpu::RenderPipeline,
    /// How subpixel text is drawn on this device (`LcdText`); `None`, should the driver reject
    /// both ways, draws every glyph grey.
    lcd: Option<LcdText>,
    tex_layout: wgpu::BindGroupLayout,
    tex_instances: InstanceBuffer,
    sampler: wgpu::Sampler,
    atlas: Vec<AtlasPage>,
    /// Cluster+face+px key → its atlas slot; `None` for a cluster that rasterised to
    /// nothing (or was too big for a page), remembered so it is not retried each frame.
    glyphs: HashMap<u64, Option<AtlasGlyph>>,
    textures: HashMap<u32, GpuTexture>,
    viewport_layout: wgpu::BindGroupLayout,
    /// Lighting and post-processing, made the first time a frame asks for either.
    effects: Option<EffectsGpu>,
    /// The particle systems, made the first time a frame carries an emitter.
    particles: Option<ParticlesGpu>,
    /// Every text run the last frame drew, as quads — what a frame that repeats a run
    /// reuses instead of re-shaping it (`RunCache`, ADR-440).
    runs: RunCache,
    /// Every memoised fragment the last frame drew, as instances — what a frame that
    /// repeats one pushes whole instead of expanding it (`FragmentCache`, ADR-482).
    fragments: FragmentCache,
    /// Bumped by every texture upload and free: a fragment holding a sprite draws
    /// according to which textures exist, so it is part of the fragment key.
    texture_generation: u64,
    /// Whether the run cache, the fragment cache and the partial instance upload are on —
    /// `BROOD_GUI_DAMAGE` (`reuse_enabled`), per window so the headless harness can draw
    /// one sequence both ways in one process.
    reuse: bool,
    /// Whether an unchanged fragment's quads are replayed (`expand_fragment`) — on with
    /// `reuse`; the harness turns it off alone, to compare replay against expansion with
    /// everything else the same.
    replay_fragments: bool,
    /// The last frame's fragment hits and misses (`FragmentCache`), read by the harness
    /// to see that a sequence it compares replayed anything at all.
    #[cfg(test)]
    last_fragments: (u32, u32),
}

/// Whether a quad at `(x, y)` sized `w x h` could put any pixel inside a `fw x fh`
/// viewport.
///
/// The GPU would clip an off-screen quad away for free, but this path **buffers** every
/// quad before drawing — unlike the CPU painter, whose fills clip against the framebuffer
/// and cost nothing when off-screen. `Op::Cells` pushes one quad per live bit of a
/// caller-supplied bitboard, so without a cull a board far larger than the window grows
/// the instance buffer (60 bytes a quad) without bound instead of drawing nothing.
///
/// Written as a **positive** test on purpose: a frame is ordinary Brood data, so a
/// coordinate can be NaN, and every comparison against NaN is false. Phrased this way
/// that culls the quad; phrased as a negation it would buffer it.
fn quad_visible(x: f32, y: f32, w: f32, h: f32, fw: f32, fh: f32) -> bool {
    w > 0.0 && h > 0.0 && x + w > 0.0 && y + h > 0.0 && x < fw && y < fh
}

/// Pick the swapchain format. A **non-sRGB** 8-bit format is preferred so the shader's
/// raw colour bytes land on screen as given — the same bytes the CPU painter writes —
/// rather than being re-encoded as if they were linear light. Every desktop backend
/// offers one; the first supported format is the fallback when none does.
fn pick_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(|f| {
            matches!(
                f,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
            )
        })
        .or_else(|| formats.first().copied())
}

/// The present mode for the window's `:vsync` option (ADR-440):
///
/// - `None` (the default) — MAILBOX where the platform has it, else FIFO. Mailbox is the
///   low-latency mode that never tears: a frame replaces the one waiting for the next
///   refresh rather than queueing behind it, so what reaches the screen is the newest.
///   An editor wants exactly that; a torn frame is a visible defect in text.
/// - `Some(true)` — FIFO, which is vsync: every frame is shown, in step with the refresh.
/// - `Some(false)` — the work-bound rate, tearing allowed: IMMEDIATE, then MAILBOX, then
///   FIFO (the only mode some compositors offer, so then the choice is theirs).
fn pick_present_mode(modes: &[wgpu::PresentMode], vsync: Option<bool>) -> wgpu::PresentMode {
    use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
    let preference: &[wgpu::PresentMode] = match vsync {
        Some(true) => &[Fifo],
        Some(false) => &[Immediate, Mailbox],
        None => &[Mailbox],
    };
    preference
        .iter()
        .copied()
        .find(|mode| modes.contains(mode))
        .unwrap_or(Fifo)
}

/// A scroll region's shift in pixels for a `dy_frac` of the cell height, capped — the
/// CPU painter's `scroll_px`.
fn scroll_px(dy_frac: f32, ch: f32) -> f32 {
    (dy_frac * ch).round().clamp(-MAX_SCROLL_PX, MAX_SCROLL_PX)
}

/// The cursor's quads for one cell: `(x, y, w, h, colour)` each. Block is a 50% wash
/// plus a solid rim; Hollow the rim alone, thinner (`cursor_rim`); Bar a caret on the
/// left edge; Underline a rule along the bottom — the CPU `cursor_cell`'s geometry,
/// thickness scaled with the cell so it stays proportional on HiDPI. `color` is the
/// op's caret colour (`None`: a white wash and `CURSOR_FG` strokes).
fn cursor_quads(
    left: f32,
    top: f32,
    cw: f32,
    ch: f32,
    style: CursorStyle,
    color: Option<[u8; 3]>,
) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
    let stroke = color.unwrap_or(CURSOR_FG);
    let fg = [stroke[0], stroke[1], stroke[2], 255];
    let rim = |t: f32| {
        vec![
            (left, top, t, ch, fg),
            (left + cw - t, top, t, ch, fg),
            (left, top, cw, t, fg),
            (left, top + ch - t, cw, t, fg),
        ]
    };
    match style {
        CursorStyle::Block | CursorStyle::Hollow => {
            let t = cursor_rim(cw.max(0.0) as usize, style) as f32;
            let mut quads = Vec::with_capacity(5);
            if style == CursorStyle::Block {
                let wash = color.unwrap_or([255, 255, 255]);
                quads.push((left, top, cw, ch, [wash[0], wash[1], wash[2], 128]));
            }
            quads.extend(rim(t));
            quads
        }
        CursorStyle::Bar => vec![(left, top, (cw / 8.0).floor().max(2.0), ch, fg)],
        CursorStyle::Underline => {
            let t = (ch / 10.0).floor().max(2.0);
            vec![(left, top + ch - t, cw, t, fg)]
        }
    }
}

impl GpuWindow {
    pub fn new(window: Rc<Window>, vsync: Option<bool>) -> Result<GpuWindow, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let raw_display_handle = window
            .display_handle()
            .map_err(|e| format!("display handle: {e}"))?
            .as_raw();
        let raw_window_handle = window
            .window_handle()
            .map_err(|e| format!("window handle: {e}"))?
            .as_raw();
        // SAFETY: the handles stay valid for as long as `window` lives, and `GpuWindow`
        // holds the `Rc<Window>` and declares `surface` before it, so the surface is
        // dropped first.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(raw_display_handle),
                raw_window_handle,
            })
        }
        .map_err(|e| format!("gpu surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|e| format!("gpu adapter: {e}"))?;
        // A machine with no GPU still offers an adapter when Mesa is installed: llvmpipe or
        // lavapipe, the GPU emulated on the CPU. Drawing through it costs more than the CPU
        // painter, which knows what changed, so it counts as no GPU — the caller falls back
        // with nothing to configure. `BROOD_GUI_GPU=1` still asks for it (the GPU-only
        // effects, a `BROOD_GUI_DUMP` readback on a machine without a GPU).
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu
            && std::env::var("BROOD_GUI_GPU").as_deref() != Ok("1")
        {
            return Err(format!("only a software adapter ({})", info.name));
        }
        let (device, queue) = request_device(&adapter)?;
        // The driver is loaded now: shut the GUI thread's gate before its exit handler runs.
        crate::host::gui::backend::close_at_exit();

        let caps = surface.get_capabilities(&adapter);
        let format = pick_format(&caps.formats).ok_or("gpu surface: no supported format")?;
        // The sRGB view of the same swapchain image the translucent geometry is drawn
        // through (linear-light blending, as the CPU painter's `blend`), when the
        // platform lets a surface be viewed in a second format. Without it every quad
        // blends in the encoded space, as before; text always does (ADR-358).
        let linear_format = linear_view_format(
            format,
            adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS),
        );
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if dump_path().is_some() {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: w,
            height: h,
            present_mode: pick_present_mode(&caps.present_modes, vsync),
            // One frame in flight: the CPU never runs more than a frame ahead of the
            // display, so a keystroke's frame is the next one shown, not the one after.
            // An editor's frames are cheap; the throughput a deeper queue buys a game is
            // latency here (ADR-440).
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: linear_format.into_iter().collect(),
        };
        surface.configure(&device, &config);
        Ok(GpuWindow::assemble(
            Target::Window { surface, window },
            device,
            queue,
            config,
            linear_format,
        ))
    }

    /// A target with no window: the frames are drawn into one `w`×`h` texture of the
    /// format a window on this platform is drawn in, its sRGB sibling viewable as the
    /// window's swapchain is where the surface allows it — the same pipelines, batches and
    /// passes, read back with `read_pixels`. What the headless harness checks the GPU path
    /// with on a real device, on nobody's screen.
    #[cfg(test)]
    pub(crate) fn offscreen(adapter: &wgpu::Adapter, w: u32, h: u32) -> Result<GpuWindow, String> {
        let (device, queue) = request_device(adapter)?;
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let linear_format = linear_view_format(format, true);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: w.max(1),
            height: h.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: linear_format.into_iter().collect(),
        };
        let texture = offscreen_texture(&device, &config);
        Ok(GpuWindow::assemble(
            Target::Offscreen { texture },
            device,
            queue,
            config,
            linear_format,
        ))
    }

    /// The pipelines, buffers and caches over a device whose target is ready.
    fn assemble(
        target: Target,
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        linear_format: Option<wgpu::TextureFormat>,
    ) -> GpuWindow {
        let format = config.format;
        let (w, h) = (config.width, config.height);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui quads"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });
        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport"),
            layout: &viewport_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Nearest: a glyph is drawn at the size it was rasterised, pixel for pixel, and
        // a sprite scaled up stays crisp (pixel art) rather than blurring.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        };

        let solid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("solid"),
            bind_group_layouts: &[Some(&viewport_layout)],
            immediate_size: 0,
        });
        // One solid pipeline per target format: the encoded one every frame draws with,
        // and (when the surface has an sRGB view) the linear-light one translucent
        // geometry is drawn with — the same vertex stage, its own fragment entry.
        let solid_quads = |label: &str, target_format: wgpu::TextureFormat, fragment: &str| {
            let target = [Some(wgpu::ColorTargetState {
                format: target_format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&solid_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_solid"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: (SOLID_FLOATS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x4, 1 => Float32x4, 2 => Float32, 3 => Float32,
                            4 => Float32, 5 => Float32x4
                        ],
                    })],
                },
                primitive,
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &target,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let solid_pipeline = solid_quads("solid quads", format, "fs_solid");
        let solid_linear_pipeline = linear_format
            .map(|linear| solid_quads("solid quads, linear light", linear, "fs_solid_linear"));

        let tex_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("textured"),
            bind_group_layouts: &[Some(&viewport_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        // The textured quads: one vertex layout, drawn three ways — `fs_tex` (colour, grey
        // text) under alpha blending, `fs_tex_lcd` (subpixel text) under `LCD_BLEND`, or
        // subpixel text one channel per pass (`fs_tex_lcd_red` …, writing only that channel).
        let textured_quads = |label: &str,
                              module: &wgpu::ShaderModule,
                              fragment: &str,
                              blend: wgpu::BlendState,
                              write_mask: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&tex_pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_tex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: (TEX_WORDS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Uint32,
                            4 => Float32, 5 => Float32x4
                        ],
                    })],
                },
                primitive,
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tex_pipeline = textured_quads(
            "textured quads",
            &shader,
            "fs_tex",
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::ColorWrites::ALL,
        );
        // Subpixel text: one dual-source pass where the device has it, else three passes,
        // one per colour channel. Built inside a validation scope: a driver that rejects
        // either costs subpixel text, not the window.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let lcd = if device
            .features()
            .contains(wgpu::Features::DUAL_SOURCE_BLENDING)
        {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("lcd text"),
                source: wgpu::ShaderSource::Wgsl(lcd_shader_source().into()),
            });
            LcdText::DualSource(textured_quads(
                "lcd text",
                &module,
                "fs_tex_lcd",
                LCD_BLEND,
                wgpu::ColorWrites::ALL,
            ))
        } else {
            let channel = |fragment: &str, mask: wgpu::ColorWrites| {
                textured_quads(
                    "lcd text, one channel",
                    &shader,
                    fragment,
                    wgpu::BlendState::ALPHA_BLENDING,
                    mask,
                )
            };
            LcdText::PerChannel([
                channel("fs_tex_lcd_red", wgpu::ColorWrites::RED),
                channel("fs_tex_lcd_green", wgpu::ColorWrites::GREEN),
                channel("fs_tex_lcd_blue", wgpu::ColorWrites::BLUE),
            ])
        };
        let lcd = match pollster::block_on(scope.pop()) {
            None => Some(lcd),
            Some(error) => {
                eprintln!("brood gui: subpixel text unavailable, drawing grey: {error}");
                None
            }
        };

        let solid_instances = InstanceBuffer::new(&device, "solid instances", 64 * 1024);
        let tex_instances = InstanceBuffer::new(&device, "textured instances", 64 * 1024);

        GpuWindow {
            target,
            device,
            queue,
            config,
            configured: (w, h),
            viewport_buffer,
            viewport_bind_group,
            solid_pipeline,
            solid_linear_pipeline,
            linear_format,
            solid_instances,
            tex_pipeline,
            lcd,
            tex_layout,
            tex_instances,
            sampler,
            atlas: Vec::new(),
            glyphs: HashMap::new(),
            textures: HashMap::new(),
            viewport_layout,
            effects: None,
            particles: None,
            runs: RunCache::default(),
            fragments: FragmentCache::default(),
            texture_generation: 0,
            reuse: reuse_enabled(),
            replay_fragments: reuse_enabled(),
            #[cfg(test)]
            last_fragments: (0, 0),
        }
    }
}

/// Every hardware adapter on this machine — the software ones (llvmpipe, lavapipe) left
/// out, as `GpuWindow::new` leaves them out. Empty on a machine without a GPU (CI).
#[cfg(test)]
pub(crate) fn hardware_adapters() -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .into_iter()
        .filter(|adapter| adapter.get_info().device_type != wgpu::DeviceType::Cpu)
        .collect()
}

/// The device and queue on `adapter`: dual-source blending where it has it.
fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("brood gui"),
        // Dual-source blending where the adapter has it: subpixel text in one pass
        // (`LCD_SRC`) rather than three (`LcdText`). `BROOD_GUI_NO_DUAL_SOURCE=1` declines
        // it, to see the three-pass path on an adapter that has the feature.
        required_features: if std::env::var("BROOD_GUI_NO_DUAL_SOURCE").as_deref() == Ok("1") {
            wgpu::Features::empty()
        } else {
            adapter.features() & wgpu::Features::DUAL_SOURCE_BLENDING
        },
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| format!("gpu device: {e}"))
}

/// The texture an offscreen target draws into: the window's size and format, drawable
/// through its sRGB view as a swapchain image is, and copyable for the readback.
#[cfg(test)]
fn offscreen_texture(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen frame"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.format,
        usage: config.usage,
        view_formats: &config.view_formats,
    })
}

impl GpuWindow {
    /// Match the target to the current window size (called before each paint).
    pub fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        if self.configured != (w, h) {
            self.config.width = w;
            self.config.height = h;
            match &mut self.target {
                Target::Window { surface, .. } => surface.configure(&self.device, &self.config),
                #[cfg(test)]
                Target::Offscreen { texture } => {
                    *texture = offscreen_texture(&self.device, &self.config)
                }
            }
            self.configured = (w, h);
        }
    }

    /// The size the next frame is drawn at: the window's, or the offscreen texture's.
    fn target_size(&self) -> (u32, u32) {
        match &self.target {
            Target::Window { window, .. } => {
                let size = window.inner_size();
                (size.width.max(1), size.height.max(1))
            }
            #[cfg(test)]
            Target::Offscreen { .. } => self.configured,
        }
    }

    /// The image to draw this frame into, and the swapchain image to present after —
    /// `None` for an offscreen target, which presents nothing. `None` altogether when the
    /// swapchain has no image to give (the frame is skipped).
    fn acquire(&self) -> Option<(wgpu::Texture, Option<wgpu::SurfaceTexture>)> {
        let surface = match &self.target {
            Target::Window { surface, .. } => surface,
            #[cfg(test)]
            Target::Offscreen { texture } => return Some((texture.clone(), None)),
        };
        let frame_texture = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                // The swapchain went stale (a resize the OS has not told winit about yet,
                // a display change): reconfigure and try once more — nothing asks for
                // another paint, so a frame skipped here would leave the old image up.
                surface.configure(&self.device, &self.config);
                match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    _ => return None,
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return None,
        };
        Some((frame_texture.texture.clone(), Some(frame_texture)))
    }

    /// The offscreen target's last frame as RGB bytes, row by row (`read_frame`).
    #[cfg(test)]
    pub(crate) fn read_pixels(&self) -> Vec<u8> {
        let Target::Offscreen { texture } = &self.target else {
            panic!("read_pixels reads an offscreen target");
        };
        let (w, h) = self.configured;
        read_frame(&self.device, &self.queue, texture, self.config.format, w, h)
            .expect("the offscreen frame reads back")
    }

    /// Turn the reuse (`reuse_enabled`) on or off for this target alone — fragment
    /// replay with it.
    #[cfg(test)]
    pub(crate) fn set_reuse(&mut self, on: bool) {
        self.reuse = on;
        self.replay_fragments = on;
    }

    /// Turn fragment replay alone on or off, the run cache and partial upload as they are.
    #[cfg(test)]
    pub(crate) fn set_replay_fragments(&mut self, on: bool) {
        self.replay_fragments = on;
    }

    /// The last frame's fragment cache hits and misses.
    #[cfg(test)]
    pub(crate) fn last_fragments(&self) -> (u32, u32) {
        self.last_fragments
    }

    /// An empty `w`×`h` RGBA8 texture on the device, with the bind group the textured
    /// pipeline samples it through.
    fn create_texture(&self, w: u32, h: u32) -> (wgpu::Texture, wgpu::BindGroup) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        (texture, bind_group)
    }

    /// Write a straight-alpha RGBA bitmap into `texture` at `(x, y)`.
    fn write_rgba(&self, texture: &wgpu::Texture, x: u32, y: u32, rgba: &[u8], w: u32, h: u32) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// `gui-texture`: keep `rgba` (`w*h*4` bytes) on the device as texture `tex`. A
    /// re-upload under a live handle replaces it; a size that does not match the bytes
    /// is dropped (the Brood side validates, this is the backstop).
    pub fn upload_texture(&mut self, tex: u32, rgba: &[u8], w: u32, h: u32) {
        if w == 0 || h == 0 || rgba.len() as u64 != w as u64 * h as u64 * 4 {
            return;
        }
        let (texture, bind_group) = self.create_texture(w, h);
        self.write_rgba(&texture, 0, 0, rgba, w, h);
        self.textures.insert(tex, GpuTexture { bind_group });
        self.texture_generation += 1;
    }

    /// `gui-texture-free`: drop texture `tex` (a later `:sprite` naming it draws nothing).
    pub fn free_texture(&mut self, tex: u32) {
        self.textures.remove(&tex);
        self.texture_generation += 1;
    }

    /// Pack a rasterised `w`×`h` cluster into the atlas, opening a new shelf or page as
    /// needed, and return where it landed. `None` for a glyph larger than a page.
    fn pack_glyph(
        &mut self,
        rgba: &[u8],
        w: u32,
        h: u32,
        mono: bool,
        lcd: bool,
    ) -> Option<AtlasGlyph> {
        if w == 0 || h == 0 || w > ATLAS_SIZE || h > ATLAS_SIZE {
            return None;
        }
        let mut page_index = self.atlas.len().saturating_sub(1);
        loop {
            if page_index >= self.atlas.len() {
                let (texture, bind_group) = self.create_texture(ATLAS_SIZE, ATLAS_SIZE);
                self.atlas.push(AtlasPage {
                    texture,
                    bind_group,
                    next_x: 0,
                    next_y: 0,
                    shelf_h: 0,
                });
            }
            let page = &mut self.atlas[page_index];
            if page.next_x + w > ATLAS_SIZE {
                page.next_y += page.shelf_h;
                page.next_x = 0;
                page.shelf_h = 0;
            }
            if page.next_y + h > ATLAS_SIZE {
                page_index += 1;
                continue;
            }
            let (x, y) = (page.next_x, page.next_y);
            page.next_x += w;
            page.shelf_h = page.shelf_h.max(h);
            let texture = &self.atlas[page_index].texture;
            self.write_rgba(texture, x, y, rgba, w, h);
            return Some(AtlasGlyph {
                page: page_index,
                x,
                y,
                w,
                h,
                mono,
                lcd,
            });
        }
    }

    /// The subpixel order glyphs are baked in for this window: the renderer's
    /// (`Renderer::lcd_order`) when this device could make the LCD pipeline, else 0 (grey).
    fn lcd_order(&self, renderer: &Renderer) -> u8 {
        if self.lcd.is_some() {
            renderer.lcd_order()
        } else {
            0
        }
    }

    /// The atlas slot for cluster `g` under a face at the renderer's current metrics,
    /// rasterising and packing it on first sight. `None` when it has no pixels.
    fn glyph_slot(
        &mut self,
        renderer: &mut Renderer,
        g: &str,
        family: Option<u32>,
        bold: bool,
        italic: bool,
        scale: u16,
    ) -> Option<AtlasGlyph> {
        let metrics = renderer.metrics();
        let lcd_order = self.lcd_order(renderer);
        let key = glyph_key(
            g,
            family.unwrap_or(renderer.default_family()),
            bold,
            italic,
            scale,
            metrics.px,
            (metrics.cell_w, metrics.cell_h),
            (renderer.font_generation(), lcd_order),
        );
        if let Some(slot) = self.glyphs.get(&key) {
            return *slot;
        }
        let cg = renderer.cluster_glyph(g, family, bold, italic, scale, lcd_order != 0);
        let (rgba, w, h, color, lcd) = (
            cg.rgba.clone(),
            cg.width as u32,
            cg.height as u32,
            cg.color,
            cg.subpixel,
        );
        let packed = self.pack_glyph(&rgba, w, h, !color, lcd);
        self.glyphs.insert(key, packed);
        packed
    }

    /// Draw text run `s` in `face` with its anchor at `(anchor_x, anchor_y)` — a cell
    /// run's grid origin and row top, a pixel run's `x` and top — replaying the quads
    /// the last frame shaped for it when it repeats, shaping (and remembering) it when
    /// it does not (`RunCache`).
    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &mut self,
        fb: &mut FrameBuilder,
        renderer: &mut Renderer,
        s: &str,
        face: &Face,
        place: RunPlace,
        g: Grid,
        anchor_x: f32,
        anchor_y: f32,
    ) {
        let metrics = renderer.metrics();
        let context = RunContext {
            place,
            metrics: [
                g.cw.to_bits(),
                g.ch.to_bits(),
                metrics.px.to_bits(),
                renderer.default_family(),
            ],
            ground: renderer.bg(),
            fonts: (
                renderer.font_generation(),
                renderer.ligatures(),
                self.lcd_order(renderer),
            ),
        };
        let reuse = self.reuse;
        let hash = run_hash(s, face, &context);
        if reuse {
            // an enclosing fragment's entry carries the runs it drew (`FragmentCache::record`)
            self.fragments.runs_used.push(hash);
        }
        if reuse && let Some(quads) = self.runs.take(hash, s, face, &context) {
            replay(fb, quads, anchor_x, anchor_y);
            return;
        }
        let quads = self.shape_run(renderer, s, face, place, g.cw, g.ch, context.ground);
        replay(fb, &quads, anchor_x, anchor_y);
        if reuse {
            self.runs.put(
                hash,
                TextRun {
                    text: s.to_string(),
                    face: *face,
                    context,
                    quads,
                },
            );
        }
    }

    /// Shape a text run into quads relative to its anchor: every cell background first
    /// (one solid batch), then a glyph per non-space cluster out of the atlas (one
    /// textured batch), then any underline — the CPU painter's text walk. A cell run
    /// starts at its column (a raw tab advances to the next SCREEN stop, background
    /// only); a pixel run at its alignment's shift by the run's shaped width.
    #[allow(clippy::too_many_arguments)]
    fn shape_run(
        &mut self,
        renderer: &mut Renderer,
        s: &str,
        face: &Face,
        place: RunPlace,
        cw: f32,
        ch: f32,
        ground: [u8; 3],
    ) -> Vec<RunQuad> {
        let atlas_size = ATLAS_SIZE as f32;
        let (mut fg, mut bg) = (
            face.fg.unwrap_or(DEFAULT_FG),
            face.bg.unwrap_or(DEFAULT_BG_RGB),
        );
        // A face with no `:bg` is transparent: the glyph composites over whatever is
        // under it (an hl-line band), exactly like Emacs.
        let mut paint_bg = face.bg.is_some();
        if face.reverse {
            std::mem::swap(&mut fg, &mut bg);
            paint_bg = true;
        }
        // The contrast lift applies to text lighter than what it lands on. The CPU
        // compares per pixel; here the ground is the run's own background, or — for a
        // transparent face — the window's, since what else lies under it is not known
        // until the blend (ADR-440).
        let lift = luma(fg) > luma(if paint_bg { bg } else { ground });
        let scale = face.scale.max(1) as usize;
        let ch_s = scale as f32 * ch;
        let mut quads = Vec::new();
        let mut clusters: Vec<(f32, f32, &str)> = Vec::new();
        match place {
            RunPlace::Cell { col } => {
                let mut cx = col as usize;
                // Ligature units when `gui-ligatures!` is on: a punctuation run is one
                // glyph canvas spanning its cells (`text_units`, ADR-442).
                for cluster in text_units(s, renderer.ligatures()) {
                    if cluster == "\t" {
                        let cells = cluster_cells_at(cluster, cx, TAB_WIDTH);
                        if paint_bg {
                            quads.push(RunQuad::Fill {
                                x: cx as f32 * cw,
                                y: 0.0,
                                w: cells as f32 * cw,
                                h: ch_s,
                                rgb: bg,
                            });
                        }
                        cx += cells;
                        continue;
                    }
                    let cells = unit_cells(cluster);
                    if cells == 0 {
                        continue;
                    }
                    let block_w = (cells * scale) as f32 * cw;
                    let left = cx as f32 * cw;
                    if paint_bg {
                        quads.push(RunQuad::Fill {
                            x: left,
                            y: 0.0,
                            w: block_w,
                            h: ch_s,
                            rgb: bg,
                        });
                    }
                    if cluster != " " {
                        clusters.push((left, block_w, cluster));
                    }
                    cx += cells * scale;
                }
            }
            RunPlace::Pixel { align } => {
                let width = text_px_width(s, cw as usize, scale) as f32;
                let left0 = match align {
                    1 => -width / 2.0,
                    2 => -width,
                    _ => 0.0,
                };
                let mut cx = 0.0f32;
                for cluster in s.graphemes(true) {
                    let cells = cluster_cells(cluster);
                    if cells == 0 {
                        continue;
                    }
                    let block_w = (cells * scale) as f32 * cw;
                    let left = left0 + cx;
                    if paint_bg {
                        quads.push(RunQuad::Fill {
                            x: left,
                            y: 0.0,
                            w: block_w,
                            h: ch_s,
                            rgb: bg,
                        });
                    }
                    if cluster != " " {
                        clusters.push((left, block_w, cluster));
                    }
                    cx += block_w;
                }
            }
        }
        for (left, _, cluster) in &clusters {
            let Some(glyph) = self.glyph_slot(
                renderer,
                cluster,
                face.family,
                face.bold,
                face.italic,
                face.scale,
            ) else {
                continue;
            };
            quads.push(RunQuad::Glyph {
                page: glyph.page,
                x: *left,
                y: 0.0,
                w: glyph.w as f32,
                h: glyph.h as f32,
                uv: [
                    glyph.x as f32 / atlas_size,
                    glyph.y as f32 / atlas_size,
                    glyph.w as f32 / atlas_size,
                    glyph.h as f32 / atlas_size,
                ],
                tint: [fg[0], fg[1], fg[2], 255],
                mode: glyph_mode(glyph.mono, glyph.lcd, lift),
            });
        }
        if face.underline {
            // A rule (or a wave) near the block bottom, in its own colour or the text's,
            // scaled with the glyph so it stays proportional.
            let underline_y = ch_s - 2.0 * scale as f32;
            let rgb = face.underline_rgb.unwrap_or(fg);
            for (left, block_w, _) in &clusters {
                for (x, y, w, h) in crate::host::gui::underline_rects(
                    left.round() as isize,
                    underline_y.round() as isize,
                    block_w.round().max(0.0) as usize,
                    scale,
                    face.underline_wave,
                ) {
                    quads.push(RunQuad::Fill {
                        x: x as f32,
                        y: y as f32,
                        w: w as f32,
                        h: h as f32,
                        rgb,
                    });
                }
            }
        }
        quads
    }

    /// What a fragment expanded on `fb` at grid `g` depends on besides its ops.
    fn fragment_context(&self, fb: &FrameBuilder, renderer: &Renderer, g: Grid) -> FragmentContext {
        let metrics = renderer.metrics();
        FragmentContext {
            grid: [
                g.ox.to_bits(),
                g.oy.to_bits(),
                g.cw.to_bits(),
                g.ch.to_bits(),
                g.scroll_dy.to_bits(),
            ],
            clip: fb.clip.map(f32::to_bits),
            viewport: [fb.fw.to_bits(), fb.fh.to_bits()],
            metrics: [
                metrics.cell_w as u32,
                metrics.cell_h as u32,
                metrics.px.to_bits(),
                renderer.line_height().to_bits(),
            ],
            family: renderer.default_family(),
            ground: renderer.bg(),
            fonts: (
                renderer.font_generation(),
                renderer.ligatures(),
                self.lcd_order(renderer),
            ),
            scale: renderer.scale().to_bits(),
            textures: self.texture_generation,
            caret_hidden: renderer.caret_hidden,
        }
    }

    /// Expand a memoised fragment (ADR-470): push the instances it produced last frame
    /// when it is the same allocation in the same context (`FragmentCache`), carrying its
    /// runs and nested fragments forward; expand its ops and record them when it is not.
    fn expand_fragment(
        &mut self,
        fb: &mut FrameBuilder,
        renderer: &mut Renderer,
        ops: &std::sync::Arc<[Op]>,
        g: Grid,
    ) {
        if !self.replay_fragments {
            self.expand(fb, renderer, ops, g);
            return;
        }
        let context = self.fragment_context(fb, renderer, g);
        if let Some(entry) = self.fragments.take(ops, &context) {
            replay_fragment(fb, &entry);
            for &hash in &entry.runs {
                self.runs.keep(hash);
            }
            for (address, nested) in &entry.nested {
                self.fragments.carry(*address, nested);
            }
            // an enclosing fragment being recorded holds this one, and what it holds
            self.fragments.runs_used.extend_from_slice(&entry.runs);
            self.fragments
                .fragments_used
                .extend_from_slice(&entry.nested);
            self.fragments
                .fragments_used
                .push((fragment_address(ops), entry.context));
            self.fragments.hits += 1;
            self.fragments.restore(entry);
            return;
        }
        let mark = self.fragments.mark(fb);
        self.expand(fb, renderer, ops, g);
        let blinks = ops.iter().any(holds_blinking_caret);
        self.fragments.record(fb, mark, ops, context, blinks);
        self.fragments.misses += 1;
    }

    /// Expand `ops` into quads on `fb`, in order, on grid `g` — the CPU painter's
    /// `render_ops`, arm for arm. Recursion is the two region ops: a scroll region
    /// re-enters with its shift (and clips at the grid top, as the CPU path clips every
    /// shifted op there); a cell region re-enters with its own metrics, origin and clip
    /// band, restoring the renderer's metrics after.
    fn expand(&mut self, fb: &mut FrameBuilder, renderer: &mut Renderer, ops: &[Op], g: Grid) {
        let scale_factor = renderer.scale() as f32;
        for op in ops {
            match op {
                // The frame was cleared to the background before any op.
                Op::Clear => {}
                // A memoised fragment is its ops in place (ADR-470), in the same grid.
                Op::Fragment(fragment) => self.expand_fragment(fb, renderer, &fragment.0, g),
                Op::ScrollRegion { dy_frac, ops } => {
                    let saved_clip = fb.clip;
                    // A shifted op is clipped at the grid origin, never painted into the
                    // inset above it — the CPU path's `clip_skip`.
                    fb.clip[1] = fb.clip[1].max(g.oy);
                    let inner = Grid {
                        scroll_dy: scroll_px(*dy_frac, g.ch),
                        ..g
                    };
                    fb.nested += 1;
                    self.expand(fb, renderer, ops, inner);
                    fb.nested -= 1;
                    fb.clip = saved_clip;
                }
                Op::CellRegion {
                    x, y, h, px, ops, ..
                } => {
                    // The rect is in the PARENT's cells (and moves with an enclosing
                    // scroll); everything inside is on the region's own grid from its
                    // top-left. The clip band is narrowed to the rect's rows, so an op that
                    // overruns the region vertically is clipped rather than painted over
                    // its neighbour. Width is not clipped (the CPU bands rows, not columns).
                    let region = renderer.metrics_at(*px);
                    if region.cell_w == 0 || region.cell_h == 0 {
                        continue;
                    }
                    let top = g.oy + *y as f32 * g.ch - g.scroll_dy;
                    let bottom = top + *h as f32 * g.ch;
                    let saved_clip = fb.clip;
                    fb.clip[1] = fb.clip[1].max(top).max(0.0);
                    fb.clip[3] = fb.clip[3].min(bottom);
                    if fb.clip[3] > fb.clip[1] {
                        let saved = renderer.metrics();
                        renderer.set_metrics(region);
                        let inner = Grid {
                            ox: g.ox + *x as f32 * g.cw,
                            oy: top.max(0.0),
                            cw: region.cell_w as f32,
                            ch: region.cell_h as f32,
                            scroll_dy: 0.0,
                        };
                        fb.nested += 1;
                        self.expand(fb, renderer, ops, inner);
                        fb.nested -= 1;
                        renderer.set_metrics(saved);
                    }
                    fb.clip = saved_clip;
                }
                // Text: the cell BACKGROUNDS as solid quads (a coloured Life cell is a
                // space + `:bg`) — all of the op's cells first, so they form one solid
                // batch — then a textured quad per non-space cluster out of the atlas,
                // which together form one textured batch, then any underline. Shaped once
                // and replayed while the run repeats (`draw_run`).
                Op::Text { row, col, s, face } => {
                    let top = g.oy + *row as f32 * g.ch - g.scroll_dy;
                    self.draw_run(
                        fb,
                        renderer,
                        s,
                        face,
                        RunPlace::Cell { col: *col },
                        g,
                        g.ox,
                        top,
                    );
                }
                Op::Rect {
                    row,
                    col,
                    w,
                    h,
                    face,
                    radius,
                } => {
                    let bg = if face.reverse { face.fg } else { face.bg };
                    if let Some(bg) = bg {
                        let (x, y, pw, ph) = (
                            g.ox + *col as f32 * g.cw,
                            g.oy + *row as f32 * g.ch - g.scroll_dy,
                            *w as f32 * g.cw,
                            *h as f32 * g.ch,
                        );
                        if *radius > 0.0 {
                            fb.solid(
                                x,
                                y,
                                pw,
                                ph,
                                [bg[0], bg[1], bg[2], 255],
                                0.0,
                                Edge::Rounded(*radius * g.cw),
                            );
                        } else {
                            fb.fill(x, y, pw, ph, bg);
                        }
                    }
                }
                Op::FRect {
                    x,
                    y,
                    w,
                    h,
                    face,
                    opacity,
                    radius,
                } => {
                    // Sub-cell rect: cell-unit floats → px via the same origin and
                    // metrics every op shares, then an AA, alpha-blended fill. A hairline
                    // snaps to whole device pixels first, as on the CPU path.
                    let bg = if face.reverse { face.fg } else { face.bg };
                    if let Some(bg) = bg {
                        let (fx, fw, fy, fh, r) = snap_hairline(
                            g.ox + *x * g.cw,
                            *w * g.cw,
                            g.oy + *y * g.ch - g.scroll_dy,
                            *h * g.ch,
                            *radius * g.cw,
                            scale_factor,
                        );
                        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                        fb.solid(
                            fx,
                            fy,
                            fw,
                            fh,
                            [bg[0], bg[1], bg[2], alpha],
                            0.0,
                            Edge::Rounded(r),
                        );
                    }
                }
                // A blinking caret in its hidden half (ADR-469) draws nothing.
                Op::Cursor { blink: Some(_), .. } if renderer.caret_hidden => {}
                // A block with the text under it, Emacs's box (ADR-515): the cell solid in
                // the caret's colour, the text over it in its own face, coloured the window
                // background.
                Op::Cursor {
                    row,
                    col,
                    style: CursorStyle::Block,
                    color,
                    under: Some(under),
                    ..
                } => {
                    let left = g.ox + *col as f32 * g.cw;
                    let top = g.oy + *row as f32 * g.ch - g.scroll_dy;
                    let cells = caret_cells(&under.0);
                    let fill = color.unwrap_or(CURSOR_FG);
                    fb.solid(
                        left,
                        top,
                        cells as f32 * g.cw,
                        g.ch,
                        [fill[0], fill[1], fill[2], 255],
                        0.0,
                        Edge::Plain,
                    );
                    let glyph = caret_glyph(*row, *col, under, renderer.bg());
                    self.expand(fb, renderer, std::slice::from_ref(&glyph), g);
                }
                Op::Cursor {
                    row,
                    col,
                    style,
                    color,
                    ..
                } => {
                    let left = g.ox + *col as f32 * g.cw;
                    let top = g.oy + *row as f32 * g.ch - g.scroll_dy;
                    for (x, y, w, h, rgba) in cursor_quads(left, top, g.cw, g.ch, *style, *color) {
                        fb.solid(x, y, w, h, rgba, 0.0, Edge::Plain);
                    }
                }
                // Hover metadata, hit-tested in the event loop (ADR-080); nothing to draw.
                Op::CursorZone { .. } => {}
                // Played when the frame arrived (`UserEvent::Draw`); nothing to draw.
                Op::Sound { .. } | Op::Music { .. } => {}
                // Read off the whole frame by `paint` (`FrameEffects`); nothing to expand.
                Op::Ambient { .. }
                | Op::Light { .. }
                | Op::Bloom { .. }
                | Op::Crt { .. }
                | Op::Burst { .. } => {}
                Op::Overlay => {
                    if fb.nested == 0 && fb.overlay_at.is_none() {
                        fb.overlay_at = Some(fb.batches.len());
                    }
                }
                // Drawn in op order, out of the emitter's own buffer.
                Op::Emitter { id, .. } => fb.batches.push(Batch::Particles { id: *id }),
                Op::VSpans { row0, col0, cols } => {
                    let top0 = g.oy + *row0 as f32 * g.ch - g.scroll_dy;
                    for (i, segs) in cols.iter().enumerate() {
                        let left = g.ox + (*col0 as usize + i) as f32 * g.cw;
                        let mut y = top0;
                        for (sh, color) in segs {
                            let span_h = *sh as f32 * g.ch;
                            if let Some(rgb) = color {
                                fb.fill(left, y, g.cw, span_h, *rgb);
                            }
                            y += span_h;
                        }
                    }
                }
                Op::Cells {
                    row0,
                    col0,
                    w,
                    aspect,
                    bytes,
                    color,
                } => {
                    if let Some(rgb) = color {
                        let asp = (*aspect).max(1) as usize;
                        let cell_w = (asp as f32) * g.cw;
                        let wmod = (*w).max(1) as usize;
                        for (bi, &byte) in bytes.iter().enumerate() {
                            let mut b = byte;
                            let base = bi * 8;
                            while b != 0 {
                                let bit = base + b.trailing_zeros() as usize;
                                let x = (bit % wmod) as f32;
                                let y = (bit / wmod) as f32;
                                fb.fill(
                                    g.ox + (*col0 as f32 + x * asp as f32) * g.cw,
                                    g.oy + (*row0 as f32 + y) * g.ch - g.scroll_dy,
                                    cell_w,
                                    g.ch,
                                    *rgb,
                                );
                                b &= b - 1;
                            }
                        }
                    }
                }
                Op::CellsRgb {
                    row0,
                    col0,
                    w,
                    aspect,
                    bytes,
                    colors,
                    default,
                } => {
                    let asp = (*aspect).max(1) as usize;
                    let cell_w = (asp as f32) * g.cw;
                    let wmod = (*w).max(1) as usize;
                    for (bi, &byte) in bytes.iter().enumerate() {
                        let mut b = byte;
                        let base = bi * 8;
                        while b != 0 {
                            let bit = base + b.trailing_zeros() as usize;
                            let rgb = colors.get(&(bit as u64)).copied().unwrap_or(*default);
                            let x = (bit % wmod) as f32;
                            let y = (bit / wmod) as f32;
                            fb.fill(
                                g.ox + (*col0 as f32 + x * asp as f32) * g.cw,
                                g.oy + (*row0 as f32 + y) * g.ch - g.scroll_dy,
                                cell_w,
                                g.ch,
                                rgb,
                            );
                            b &= b - 1;
                        }
                    }
                }
                // Pixel space: window coordinates as given, untouched by grid or scroll.
                Op::Sprite {
                    tex,
                    x,
                    y,
                    w,
                    h,
                    uv,
                    tint,
                    rot,
                } => {
                    if self.textures.contains_key(tex) {
                        fb.textured(TexKey::User(*tex), *x, *y, *w, *h, *uv, *tint, 0, *rot);
                    }
                }
                Op::Quad {
                    x,
                    y,
                    w,
                    h,
                    color,
                    rot,
                } => {
                    fb.solid(*x, *y, *w, *h, *color, *rot, Edge::Plain);
                }
                // Pixel-space text: the `Text` walk from a pixel top-left (after the
                // alignment's shift by the run's shaped width), untouched by grid or scroll.
                Op::TextPx {
                    x,
                    y,
                    s,
                    face,
                    align,
                } => {
                    let align = match align {
                        Align::Left => 0,
                        Align::Center => 1,
                        Align::Right => 2,
                    };
                    self.draw_run(fb, renderer, s, face, RunPlace::Pixel { align }, g, *x, *y);
                }
            }
        }
    }

    /// Draw one frame: expand the ops to quads in op order, upload, draw the batches,
    /// present. The cell pixel size and the grid origin come from the renderer — the
    /// same coordinate contract the CPU `paint` uses, so cell ops land on the same pixels
    /// whichever target paints them.
    pub(crate) fn paint(&mut self, frame: &[Op], renderer: &mut Renderer) -> Option<PaintTimes> {
        // `BROOD_GUI_TRACE=1` (or a `BROOD_STALL_MS` threshold): the same per-paint line the
        // CPU painter prints, split into this target's phases. Without it the GPU path —
        // the default in a `--with-gui-gpu` build — could not be measured at all.
        let trace = paint_stall_ms();
        let t0 = std::time::Instant::now();
        let (cw, ch) = (renderer.cell_w.max(1), renderer.cell_h.max(1));
        let (fw, fh) = self.target_size();
        self.resize(fw, fh);

        let (fwf, fhf) = (fw as f32, fh as f32);
        let mut fb = FrameBuilder::new(fwf, fhf);
        let (ox, oy) = renderer.grid_origin(fw as usize, fh as usize);
        let grid = Grid {
            ox: ox as f32,
            oy: oy as f32,
            cw: cw as f32,
            ch: ch as f32,
            scroll_dy: 0.0,
        };
        self.expand(&mut fb, renderer, frame, grid);
        let (run_hits, run_misses) = (self.runs.hits, self.runs.misses);
        let (fragment_hits, fragment_misses) = (self.fragments.hits, self.fragments.misses);
        #[cfg(test)]
        {
            self.last_fragments = (fragment_hits, fragment_misses);
        }
        self.runs.end_frame();
        self.fragments.end_frame();
        // The particles step once per paint, as the CPU painter's do; an emitter the frame
        // no longer carries still draws its last particles, at the end of the scene.
        let steps = renderer.emitters.step(frame);
        let fx = FrameEffects::of(frame);
        let at = fb.overlay_at.unwrap_or(fb.batches.len());
        let lingering: Vec<Batch> = steps
            .iter()
            .filter(|step| !step.present)
            .map(|step| Batch::Particles { id: step.id })
            .collect();
        let split = at + lingering.len();
        fb.batches.splice(at..at, lingering);
        let t_expand = std::time::Instant::now();

        self.queue.write_buffer(
            &self.viewport_buffer,
            0,
            as_bytes(&[fwf, fhf, contrast_exponent(renderer.text_contrast()), 0.0]),
        );
        let uploaded = self.solid_instances.upload_changed(
            &self.device,
            &self.queue,
            "solid instances",
            as_words(&fb.solids),
            self.reuse,
        ) + self.tex_instances.upload_changed(
            &self.device,
            &self.queue,
            "textured instances",
            &fb.texs,
            self.reuse,
        );
        if !steps.is_empty() {
            let format = self.config.format;
            self.particles
                .get_or_insert_with(|| {
                    ParticlesGpu::new(&self.device, format, &self.viewport_layout)
                })
                .sync(&self.device, &self.queue, &steps);
        }
        if fx.any() {
            self.prepare_effects(&fx, fw, fh);
        }
        let t_upload = std::time::Instant::now();

        let (frame_texture, surface_texture) = self.acquire()?;
        // acquiring the next swapchain image is where a vsync'd present waits for the display
        let t_acquire = std::time::Instant::now();
        let view = frame_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // The same image viewed as sRGB, for the geometry that blends in linear light.
        let linear_view = self.linear_format.map(|format| {
            frame_texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(format),
                ..Default::default()
            })
        });
        let bg = renderer.bg();
        let background = wgpu::Color {
            r: bg[0] as f64 / 255.0,
            g: bg[1] as f64 / 255.0,
            b: bg[2] as f64 / 255.0,
            a: 1.0,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        if let Some(particles) = &self.particles {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("particle step"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&particles.step_pipeline);
            for emitter in particles.emitters.values() {
                pass.set_bind_group(0, &emitter.step_group, &[]);
                pass.dispatch_workgroups(emitter.capacity.div_ceil(64), 1, 1);
            }
        }
        let passes = match self.effects.as_ref().and_then(|e| e.targets.as_ref()) {
            Some(targets) if fx.any() => {
                let effects = self.effects.as_ref().expect("targets are effects'");
                // the scene, lit, then the overlay: all into the scene texture
                // An effects frame blends in the encoded space throughout: its scene
                // target is one format, and the passes over it are light, not text.
                self.draw_pass(
                    &mut encoder,
                    &targets.scene,
                    None,
                    Some(background),
                    &fb.batches[..split],
                );
                if let Some(ambient) = fx.ambient {
                    let ambient = wgpu::Color {
                        r: ambient[0] as f64 / 255.0,
                        g: ambient[1] as f64 / 255.0,
                        b: ambient[2] as f64 / 255.0,
                        a: 1.0,
                    };
                    let mut pass =
                        render_pass(&mut encoder, "light map", &targets.light, Some(ambient));
                    pass.set_pipeline(&effects.light_pipeline);
                    pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                    pass.set_vertex_buffer(0, effects.light_instances.buffer.slice(..));
                    pass.draw(0..4, 0..fx.lights.len() as u32);
                    drop(pass);
                    let mut pass = render_pass(&mut encoder, "lighting", &targets.scene, None);
                    pass.set_pipeline(&effects.multiply_pipeline);
                    pass.set_bind_group(0, &targets.multiply, &[]);
                    pass.draw(0..3, 0..1);
                }
                self.draw_pass(
                    &mut encoder,
                    &targets.scene,
                    None,
                    None,
                    &fb.batches[split..],
                );
                if fx.bloom.is_some() {
                    for (label, target, group, pipeline) in [
                        (
                            "bloom bright pass",
                            &targets.bloom_a,
                            &targets.bright,
                            &effects.bright_pipeline,
                        ),
                        (
                            "bloom blur across",
                            &targets.bloom_b,
                            &targets.across,
                            &effects.blur_pipeline,
                        ),
                        (
                            "bloom blur down",
                            &targets.bloom_a,
                            &targets.down,
                            &effects.blur_pipeline,
                        ),
                    ] {
                        let mut pass =
                            render_pass(&mut encoder, label, target, Some(wgpu::Color::BLACK));
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, group, &[]);
                        pass.draw(0..3, 0..1);
                    }
                }
                let mut pass = render_pass(&mut encoder, "final", &view, Some(wgpu::Color::BLACK));
                pass.set_pipeline(&effects.final_pipeline);
                pass.set_bind_group(0, &targets.composite, &[]);
                pass.draw(0..3, 0..1);
                drop(pass);
                1
            }
            _ => self.draw_pass(
                &mut encoder,
                &view,
                linear_view.as_ref(),
                Some(background),
                &fb.batches,
            ),
        };
        self.queue.submit(std::iter::once(encoder.finish()));
        let t_submit = std::time::Instant::now();
        if let Some(path) = dump_path() {
            dump_frame(
                &self.device,
                &self.queue,
                &frame_texture,
                self.config.format,
                fw,
                fh,
                path,
            );
        }
        if let Some(surface_texture) = surface_texture {
            self.queue.present(surface_texture);
        }
        let t_present = std::time::Instant::now();
        let micros =
            |a: std::time::Instant, b: std::time::Instant| b.duration_since(a).as_micros() as u64;
        let times = PaintTimes {
            encode_us: micros(t0, t_expand),
            upload_us: micros(t_expand, t_upload),
            acquire_us: micros(t_upload, t_acquire),
            submit_us: micros(t_acquire, t_submit),
            present_us: micros(t_submit, t_present),
        };
        if let Some(ms) = trace
            && t_present.duration_since(t0).as_millis() >= ms
        {
            // runs: text runs replayed / shaped; frags: memoised fragments pushed whole / expanded; up: instance bytes written (a frame that
            // changed one line writes that line's tail, not the page); passes: render
            // passes (more than one only where translucent geometry and text interleave); lcd:
            // how this device draws subpixel text (`LcdText`).
            let lcd = match &self.lcd {
                Some(LcdText::DualSource(_)) => "dual-source",
                Some(LcdText::PerChannel(_)) => "per-channel",
                None => "none",
            };
            eprintln!(
                "[gui-paint-gpu] {}us: fb={fw}x{fh} ops={} batches={} passes={passes} \
                 runs={run_hits}/{} frags={fragment_hits}/{} up={uploaded}B lcd={lcd} | encode={}us \
                 upload={}us acquire={}us submit={}us present={}us",
                micros(t0, t_present),
                frame.len(),
                fb.batches.len(),
                run_hits + run_misses,
                fragment_hits + fragment_misses,
                times.encode_us,
                times.upload_us,
                times.acquire_us,
                times.submit_us,
                times.present_us,
            );
        }
        Some(times)
    }
}

/// Where one GPU paint's time went, in microseconds —
/// what the input-to-present trace splits a latency sample by (ADR-440). `encode` is the
/// op walk (shaping the runs that changed), `upload` the instance writes, `acquire`
/// waiting for a swapchain image (where a FIFO present blocks), `submit` recording and
/// submitting the passes, `present` handing the image to the compositor.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaintTimes {
    pub(crate) encode_us: u64,
    pub(crate) upload_us: u64,
    pub(crate) acquire_us: u64,
    pub(crate) submit_us: u64,
    pub(crate) present_us: u64,
}

impl PaintTimes {
    /// The phases in `gui::PAINT_PHASES` order.
    pub(crate) fn phases(self) -> [u64; 5] {
        [
            self.encode_us,
            self.upload_us,
            self.acquire_us,
            self.submit_us,
            self.present_us,
        ]
    }
}

impl GpuWindow {
    /// Draw `batches` in order into `view` — cleared to `clear` first, or drawn over what
    /// is there. With `linear_view` (the same image viewed as sRGB) the batches are drawn
    /// as `segments`: translucent geometry through it, blended in linear light, the rest
    /// through `view`. Returns how many render passes it took, for the trace.
    fn draw_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        linear_view: Option<&wgpu::TextureView>,
        clear: Option<wgpu::Color>,
        batches: &[Batch],
    ) -> usize {
        let linear = linear_view.zip(self.solid_linear_pipeline.as_ref());
        let plan = segments(batches, linear.is_some());
        let mut clear = clear;
        for (in_linear, range) in &plan {
            let (target, solid_pipeline) = match linear {
                Some((linear_view, linear_pipeline)) if *in_linear => {
                    (linear_view, linear_pipeline)
                }
                _ => (view, &self.solid_pipeline),
            };
            // A clear value is written through the view's format: the sRGB view encodes it,
            // so the encoded background has to be decoded first or it lands twice as light.
            let load = clear.take().map(|colour| {
                if *in_linear {
                    decode_clear(colour)
                } else {
                    colour
                }
            });
            let mut pass = render_pass(encoder, "frame", target, load);
            pass.set_bind_group(0, &self.viewport_bind_group, &[]);
            for batch in &batches[range.clone()] {
                match batch {
                    Batch::Solid { first, count, .. } => {
                        pass.set_pipeline(solid_pipeline);
                        pass.set_vertex_buffer(0, self.solid_instances.buffer.slice(..));
                        pass.draw(0..4, *first..first + count);
                    }
                    Batch::Tex {
                        key,
                        first,
                        count,
                        lcd,
                    } => {
                        let bind_group = match key {
                            TexKey::Atlas(page) => self.atlas.get(*page).map(|p| &p.bind_group),
                            TexKey::User(tex) => self.textures.get(tex).map(|t| &t.bind_group),
                        };
                        let Some(bind_group) = bind_group else {
                            continue;
                        };
                        pass.set_vertex_buffer(0, self.tex_instances.buffer.slice(..));
                        pass.set_bind_group(1, bind_group, &[]);
                        let pipelines: &[wgpu::RenderPipeline] = match &self.lcd {
                            Some(LcdText::DualSource(pipeline)) if *lcd => {
                                std::slice::from_ref(pipeline)
                            }
                            Some(LcdText::PerChannel(channels)) if *lcd => channels,
                            _ => std::slice::from_ref(&self.tex_pipeline),
                        };
                        for pipeline in pipelines {
                            pass.set_pipeline(pipeline);
                            pass.draw(0..4, *first..first + count);
                        }
                    }
                    Batch::Particles { id } => {
                        let Some(particles) = &self.particles else {
                            continue;
                        };
                        let Some(emitter) = particles.emitters.get(id) else {
                            continue;
                        };
                        pass.set_pipeline(if emitter.additive {
                            &particles.add_pipeline
                        } else {
                            &particles.blend_pipeline
                        });
                        pass.set_vertex_buffer(0, emitter.particles.slice(..));
                        pass.set_bind_group(1, &emitter.draw_group, &[]);
                        pass.draw(0..4, 0..emitter.capacity);
                    }
                }
            }
        }
        plan.len()
    }

    /// Ready the effect passes for a `w`×`h` frame: the pipelines and targets, the lights'
    /// instances, and each pass's uniform.
    fn prepare_effects(&mut self, fx: &FrameEffects, w: u32, h: u32) {
        let format = self.config.format;
        let effects = self
            .effects
            .get_or_insert_with(|| EffectsGpu::new(&self.device, format, &self.viewport_layout));
        let half = effects.targets(&self.device, format, w, h).half;
        let lights: Vec<f32> = fx
            .lights
            .iter()
            .flat_map(|&(x, y, radius, color, intensity)| {
                [
                    x,
                    y,
                    radius,
                    intensity,
                    color[0] as f32 / 255.0,
                    color[1] as f32 / 255.0,
                    color[2] as f32 / 255.0,
                    1.0,
                ]
            })
            .collect();
        effects.light_instances.upload(
            &self.device,
            &self.queue,
            "light instances",
            as_bytes(&lights),
        );
        let (threshold, intensity, radius) = fx.bloom.unwrap_or((1.0, 0.0, 0.0));
        let (scanlines, curvature, vignette) = fx.crt.unwrap_or((0.0, 0.0, 0.0));
        // the blur runs at half size: a radius of r frame pixels is r/2 texels there
        let texels = radius / 2.0;
        let sigma = (texels / 2.0).max(0.5);
        let taps = texels.ceil().clamp(1.0, 32.0);
        let post = |step: [f32; 2], bloom: f32| {
            [
                threshold,
                (1.0 - threshold).max(1e-3),
                bloom,
                curvature,
                scanlines,
                vignette,
                step[0],
                step[1],
                sigma,
                taps,
                0.0,
                0.0,
            ]
        };
        let write = |buffer: &wgpu::Buffer, words: [f32; 12]| {
            self.queue.write_buffer(buffer, 0, as_bytes(&words));
        };
        write(&effects.post_bright, post([0.0, 0.0], 0.0));
        write(&effects.post_across, post([1.0 / half.0 as f32, 0.0], 0.0));
        write(&effects.post_down, post([0.0, 1.0 / half.1 as f32], 0.0));
        write(&effects.post_final, post([0.0, 0.0], intensity));
    }
}

/// A render pass over `view` with one colour attachment: cleared to `clear`, or loading
/// what is there when it is `None`.
fn render_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// The textured shader's `mode` for a glyph: 0 a colour bitmap (an emoji), else a coverage
/// mask — 1 grey, 3 per channel (LCD) — plus one when the text takes the contrast lift
/// (`lift`: lighter than its ground).
fn glyph_mode(mono: bool, lcd: bool, lift: bool) -> u32 {
    match (mono, lcd) {
        (false, _) => 0,
        (true, false) => 1 + lift as u32,
        (true, true) => 3 + lift as u32,
    }
}

/// The atlas key of a cluster under a face at a font size: what it is drawn from depends
/// on the cluster, the RESOLVED family (a face without one is drawn in the window's
/// default, which `gui-font!` changes), the weight/slant, the scale, the pixel size (a
/// cell region rasterises at its own px), the cell box the cluster is baked into (a
/// line-height change resizes it at the same px) and how it was baked: the font registration
/// (a re-registered family keyword names other faces) and the subpixel order (0 grey, 1
/// R-G-B, 2 B-G-R — `gui-text-aa!` bakes the same cluster three ways) — not the colour, a
/// per-instance tint.
fn glyph_key(
    g: &str,
    family: u32,
    bold: bool,
    italic: bool,
    scale: u16,
    px: f32,
    cell: (usize, usize),
    baked: (u64, u8),
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    g.hash(&mut h);
    family.hash(&mut h);
    bold.hash(&mut h);
    italic.hash(&mut h);
    scale.hash(&mut h);
    px.to_bits().hash(&mut h);
    cell.hash(&mut h);
    baked.hash(&mut h);
    h.finish()
}

fn as_bytes(v: &[f32]) -> &[u8] {
    // SAFETY: `f32` has no padding or invalid bit patterns; the slice covers exactly the
    // floats' storage.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn as_bytes_u32(v: &[u32]) -> &[u8] {
    // SAFETY: as `as_bytes`.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn as_words(v: &[f32]) -> &[u32] {
    // SAFETY: `f32` and `u32` have the same size and alignment, and every bit pattern is
    // a valid `u32`; the slice covers exactly the floats' storage.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u32, v.len()) }
}

/// The shader's contrast exponent for a `gui-text-contrast!` γ: `1/γ`, the power the
/// lifted coverage is raised to (`contrast_lut`'s curve, computed per fragment).
fn contrast_exponent(gamma: f32) -> f32 {
    if gamma.is_finite() {
        1.0 / gamma.clamp(0.5, 3.0)
    } else {
        1.0
    }
}

/// `BROOD_GUI_DUMP=<path.ppm>`: the GPU path's half of the CPU painter's dump flag
/// (`paint::dump_canvas`) — read the presented frame back and write it as a binary PPM
/// after every paint, so what the GPU drew can be LOOKED at from a script (the desktop
/// forbids a screenshot to an unprivileged process). Read once; off by default. The
/// readback is a full round trip through a mapped buffer every frame, so it is a debug
/// aid, not a path anything else runs on.
fn dump_path() -> Option<&'static str> {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| std::env::var("BROOD_GUI_DUMP").ok())
        .as_deref()
}

/// Write `texture` (the frame just drawn, `w`×`h`, `format`) to `path` as a PPM.
fn dump_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    path: &str,
) {
    let Some(rgb) = read_frame(device, queue, texture, format, w, h) else {
        return;
    };
    let mut out = Vec::with_capacity(rgb.len() + 32);
    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
    out.extend_from_slice(&rgb);
    let _ = std::fs::write(path, out);
}

/// Copy `texture` (`w`×`h`, `format`) into a mapped buffer and return its pixels as RGB
/// bytes, row by row. Rows are padded to wgpu's 256-byte copy alignment and unpadded on
/// the way out; a BGRA surface is swizzled to RGB. `None` when the device could not wait
/// for the copy or map the buffer.
fn read_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
) -> Option<Vec<u8>> {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = (4 * w).div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame dump"),
        size: padded_row as u64 * h as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frame dump"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
        return None;
    }
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let Ok(data) = slice.get_mapped_range() else {
        return None;
    };
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for row in data.chunks(padded_row as usize).take(h as usize) {
        for px in row[..(4 * w) as usize].chunks(4) {
            if bgra {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            } else {
                out.extend_from_slice(&[px[0], px[1], px[2]]);
            }
        }
    }
    drop(data);
    buffer.unmap();
    Some(out)
}

#[cfg(test)]
mod tests {
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
        fb.textured(TexKey::Atlas(0), 0.0, 0.0, 8.0, 8.0, uv, [255; 4], 3, 0.0);
        fb.textured(TexKey::Atlas(0), 8.0, 0.0, 8.0, 8.0, uv, [255; 4], 4, 0.0);
        fb.textured(TexKey::Atlas(0), 16.0, 0.0, 8.0, 8.0, uv, [255; 4], 1, 0.0);
        fb.textured(TexKey::Atlas(0), 24.0, 0.0, 8.0, 8.0, uv, [255; 4], 3, 0.0);
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
        fb.textured(TexKey::User(7), 0.0, 0.0, 8.0, 8.0, uv, [255; 4], 0, 0.0);
        fb.textured(TexKey::User(7), 8.0, 0.0, 8.0, 8.0, uv, [255; 4], 0, 0.0);
        fb.textured(TexKey::User(9), 16.0, 0.0, 8.0, 8.0, uv, [255; 4], 0, 0.0);
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
}

#[cfg(test)]
mod reuse_tests {
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
                0.0f32.to_bits(),
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
    fn translucent_geometry_gets_a_linear_pass_and_opaque_fills_ride_along() {
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
        );
        assert_eq!(
            segments(&fb.batches, true),
            vec![(false, 0..2), (true, 2..3), (false, 3..4)]
        );
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
}
