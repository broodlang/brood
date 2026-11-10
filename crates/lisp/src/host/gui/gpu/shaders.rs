//! The WGSL sources of every GPU pipeline: the solid and textured quads, subpixel (LCD)
//! text, the lights, the passes over a whole texture, and the particles' compute step and
//! quads. Strings only; `pipelines.rs` and `effects.rs` compile them.

/// Both pipelines. A quad is four vertices of a triangle strip generated from the vertex
/// index (no vertex buffer): `corner` is (0,0) (1,0) (0,1) (1,1), and every per-quad
/// value rides in the instance buffer. Pixel coordinates, top-left origin, mapped to NDC
/// through the viewport uniform — the CPU painter's coordinate contract. A quad turns
/// about its own centre by `rot` radians (clockwise on screen, since y points down).
/// Every quad carries a clip rect `[x0 y0 x1 y1]` the fragment stage discards outside
/// of — how a cell region's band and a scroll region's grid-top clip are honoured.
pub(super) const SHADER_SRC: &str = r#"
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
    @location(6) ground: f32,       // a glyph's ground's linear luminance (`text_alpha`)
};
struct TexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
    @location(2) @interpolate(flat) mode: u32,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) ground: f32,
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
    out.ground = inst.ground;
    return out;
}

@fragment
fn fs_tex(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    let t = textureSample(tex, samp, in.uv);
    // The encoded target, where the surface has no sRGB view (else text is drawn by
    // `fs_tex_linear`, ADR-611): the blend unit lerps the bytes as given, which has the
    // weight `TextInk` corrects to, without its linear-light hue.
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

// --- text in linear light (ADR-611): the CPU painter's `TextInk`, into the sRGB view ----

fn srgb_encode1(c: f32) -> f32 {
    let x = clamp(c, 0.0, 1.0);
    return select(1.055 * pow(x, 1.0 / 2.4) - 0.055, x * 12.92, x <= 0.0031308);
}

fn srgb_decode1(c: f32) -> f32 {
    let x = clamp(c, 0.0, 1.0);
    return select(pow((x + 0.055) / 1.055, 2.4), x / 12.92, x <= 0.04045);
}

fn luminance(linear: vec3<f32>) -> f32 {
    return dot(linear, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// ink.rs `text_alpha`: the linear-light alpha whose result has the luminance an encoded
// lerp at `cov` would — hue mixed in linear light, weight the encoded blend's.
fn text_alpha(cov: f32, fg: f32, bg: f32) -> f32 {
    if (abs(fg - bg) <= 0.001) {
        return cov;
    }
    let blended = srgb_decode1(srgb_encode1(fg) * cov + srgb_encode1(bg) * (1.0 - cov));
    return clamp((blended - bg) / (fg - bg), 0.0, 1.0);
}

// A glyph's coverage, lifted (`cov^(1/γ)`) for text lighter than its ground (mode 2 / 4).
fn lifted(cov: vec3<f32>, mode: u32) -> vec3<f32> {
    if (mode == 2u || mode == 4u) {
        return pow(cov, vec3<f32>(viewport.text.x));
    }
    return cov;
}

fn text_alpha3(cov: vec3<f32>, fg: f32, bg: f32) -> vec3<f32> {
    return vec3<f32>(text_alpha(cov.r, fg, bg), text_alpha(cov.g, fg, bg), text_alpha(cov.b, fg, bg));
}

@fragment
fn fs_tex_linear(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    let t = textureSample(tex, samp, in.uv);
    if (in.mode == 0u) {
        let c = t * in.tint;
        return vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    // a grey mask, or an LCD one drawn grey (its alpha is its greatest channel)
    let fg = srgb_to_linear(in.tint.rgb);
    let cov = lifted(vec3<f32>(t.a), in.mode).x;
    return vec4<f32>(fg, text_alpha(cov, luminance(fg), in.ground) * in.tint.a);
}

fn lcd_alpha_linear(in: TexOut) -> vec3<f32> {
    let fg = srgb_to_linear(in.tint.rgb);
    let cov = lifted(textureSample(tex, samp, in.uv).rgb, in.mode);
    return text_alpha3(cov, luminance(fg), in.ground);
}

@fragment
fn fs_tex_lcd_red_linear(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    return vec4<f32>(srgb_to_linear(in.tint.rgb), lcd_alpha_linear(in).r * in.tint.a);
}

@fragment
fn fs_tex_lcd_green_linear(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    return vec4<f32>(srgb_to_linear(in.tint.rgb), lcd_alpha_linear(in).g * in.tint.a);
}

@fragment
fn fs_tex_lcd_blue_linear(in: TexOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    return vec4<f32>(srgb_to_linear(in.tint.rgb), lcd_alpha_linear(in).b * in.tint.a);
}
"#;

/// Subpixel (LCD) text: [`SHADER_SRC`] plus a fragment entry with two outputs, blended as
/// `colour·coverage + dst·(1 − coverage)` PER CHANNEL (`BlendFactor::Src1`) — which a single
/// output cannot say, since its one alpha weighs all three channels alike. Compiled only on
/// an adapter with `DUAL_SOURCE_BLENDING` ([`lcd_shader_source`] puts the `enable` first).
/// The atlas texel's R, G and B are each channel's coverage, already in the subpixel order
/// the rasteriser was asked for; the lift (mode 4) is taken per channel, as the CPU's
/// `blend_text_rgb` does.
pub(super) const LCD_SRC: &str = r#"
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

// Into the sRGB view (ADR-611): each channel's alpha corrected as `fs_tex_linear`'s is.
@fragment
fn fs_tex_lcd_linear(in: TexOut) -> LcdOut {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    let cov = lcd_alpha_linear(in);
    var out: LcdOut;
    let alpha = max(cov.r, max(cov.g, cov.b)) * in.tint.a;
    out.colour = vec4<f32>(srgb_to_linear(in.tint.rgb), alpha);
    out.coverage = vec4<f32>(cov * in.tint.a, alpha);
    return out;
}
"#;

/// The WGSL of the LCD text pipeline: the `enable` directive has to come before any
/// declaration, so it heads the shared source rather than living in [`LCD_SRC`].
pub(super) fn lcd_shader_source() -> String {
    format!("enable dual_source_blending;\n{SHADER_SRC}{LCD_SRC}")
}

/// The effects' shaders (`effects.rs` holds the CPU painter's copy of every formula in
/// them): lights drawn additively into a light map, here; the passes over a whole
/// texture (`POST_SRC`); and the particles — a compute shader that spawns and steps them
/// (`PARTICLES_SRC`) and the instanced quads that draw them straight out of the same
/// buffer (`PARTICLE_DRAW_SRC`).
pub(super) const LIGHT_SRC: &str = r#"
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
pub(super) const POST_SRC: &str = r#"
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
pub(super) const PARTICLES_SRC: &str = r#"
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

pub(super) const PARTICLE_DRAW_SRC: &str = r#"
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
