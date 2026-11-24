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
                                    // 3 / 4 = the same per colour channel (LCD, `fs_tex_lcd`),
                                    // 5 = colour premultiplied (a smooth texture, ADR-783)
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
    if (in.mode == 5u) {
        return straight(t) * in.tint;
    }
    return t * in.tint;
}

// A smooth texture's texel (ADR-783): kept premultiplied, so the linear sampler has mixed
// colour weighted by coverage — a transparent neighbour fades an edge without darkening it.
// Back to straight alpha for the pipeline's blend, which multiplies by alpha again: the
// premultiplied blend `src + dst·(1 − a)`, in the same pipeline as every other sprite.
fn straight(texel: vec4<f32>) -> vec4<f32> {
    if (texel.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(min(texel.rgb / texel.a, vec3<f32>(1.0)), texel.a);
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
    if (in.mode == 0u || in.mode == 5u) {
        let c = select(t, straight(t), in.mode == 5u) * in.tint;
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

/// The mesh shader (ADR-786, ADR-788, ADR-793, ADR-794): morph, skin and transform a vertex; shade a
/// fragment with the fixed model `host::gui::mesh` writes for the CPU painter (`shade`, `tone`,
/// `ambient`, `sh_irradiance`, `fresnel`, `specular_lobes`, `light_arrival`,
/// `shadow_visibility`, `sample_shadow`, `soft_shadow`, `receiver_slope`, `bilinear_tap`,
/// `stored_depth`, `pixel_noise`, `disc_tap`, `skin_diffuse`, `detail_tilt`, `detail_normal`,
/// `detail_specular`) — keep the two in step. Writes
/// premultiplied, sRGB-encoded colour, the space the scene's picture is composited in.
/// `vs_shadow` / `fs_shadow` render a shadow map: the same posing, seen through the shadow
/// view the instance index names, alpha-tested for a `:cutout` or `:blend` surface.
pub(super) const MESH_SRC: &str = r#"
struct Light {
    position: vec4<f32>,        // xyz, range (0: no window)
    direction: vec4<f32>,       // xyz: the way the light travels, size (metres; radians directional)
    color: vec4<f32>,           // rgb, shadow softness (metres at the receiver; 0 and no size: 3×3)
    cone: vec4<f32>,            // cos inner, cos outer, shadow bias, shadow normal bias
    shadow: vec4<u32>,          // kind (0 directional, 1 point, 2 spot), first view, view count
};
struct ShadowView {
    view_proj: mat4x4<f32>,
    info: vec4<f32>,            // texel size (at 1 m for a perspective view), resolution, near, far
};
struct SceneU {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    lights: array<Light, 8>,
    sky: vec4<f32>,
    ground: vec4<f32>,
    params: vec4<f32>,          // exposure, light count, shadow layer side, ambient is SH (0 or 1)
    ambient_sh: array<vec4<f32>, 9>,    // L00, L1-1, L10, L11, L2-2, L2-1, L20, L21, L22 (rgb)
    shadows: array<ShadowView, 24>,
    occlusion: vec4<f32>,       // ambient occlusion radius (metres; 0: none), strength, view + 1 (0: none)
    picture: vec4<f32>,         // the picture's width and height in pixels
};
struct DrawU {
    model: mat4x4<f32>,
    color: vec4<f32>,
    skin: vec4<f32>,            // a `:skin` surface: its curvature (1 / mesh units) to 1 / mm, times `:scatter`
    material: vec4<f32>,        // roughness, specular, rim, cutoff
    lobes: vec4<f32>,           // roughness times a, times b, mix of a, F0
    flags: vec4<u32>,           // surface, texture bits (`scenes::texture_bits`), palette offset, skinned
    morph_info: vec4<u32>,      // morphed (0 or 1), vertex count, palette joint count, first offset
    strand: vec4<f32>,          // a `:strands` surface's width at the root and the tip (metres)
    detail: vec4<f32>,          // the detail tile's repeats along u and v, its strength, its cavity
};
@group(0) @binding(0) var<uniform> scene: SceneU;
@group(0) @binding(1) var<storage, read> palette: array<mat4x4<f32>>;
@group(0) @binding(2) var shadow_maps: texture_depth_2d_array;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;
// `mesh::skin_table` (ADR-794): the diffuse light under skin, linear rgb, the cosine to the light
// across and the curvature down; read with `textureLoad` only (`skin_diffuse`)
@group(0) @binding(4) var skin_table: texture_2d<f32>;
@group(1) @binding(0) var<uniform> draw: DrawU;
// this frame's morph offsets, blended on the CPU from the sparse targets (`mesh::add_morphs`):
// two vec4s a vertex (position, normal), a morphed draw's from `morph_info.w`, so a vertex
// reads one pair whatever the number of targets
@group(2) @binding(0) var<storage, read> morphs: array<vec4<f32>>;
@group(3) @binding(0) var base_texture: texture_2d<f32>;
@group(3) @binding(1) var normal_texture: texture_2d<f32>;
@group(3) @binding(2) var texture_sampler: sampler;
@group(3) @binding(3) var occlusion_texture: texture_2d<f32>;
// a material's `:detail` tile (ADR-797): a tangent-space normal's x and y in red and green
// about DETAIL_FLAT, the cavity in blue; a flat, open texel where the material has none
@group(3) @binding(4) var detail_texture: texture_2d<f32>;

struct VertexIn {
    @builtin(vertex_index) index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
    @location(6) curvature: f32,
};
struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) curvature: f32,
};

// a vertex morphed, and the matrix that skins and places it
struct Posed {
    matrix: mat4x4<f32>,
    position: vec3<f32>,
    normal: vec3<f32>,
};

fn pose(input: VertexIn) -> Posed {
    var position = input.position;
    var normal = input.normal;
    if (draw.morph_info.x != 0u) {
        let at = (draw.morph_info.w + input.index) * 2u;
        position = position + morphs[at].xyz;
        normal = normal + morphs[at + 1u].xyz;
    }
    var matrix = draw.model;
    if (draw.flags.w == 1u) {
        // as `skin_matrix` skins: a joint past the palette's end is skipped, and a vertex no
        // joint of the palette moves stays rigid
        let start = draw.flags.z;
        let joint_count = draw.morph_info.z;
        var skin = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
        var moved = false;
        for (var slot = 0u; slot < 4u; slot = slot + 1u) {
            let joint = input.joints[slot];
            let weight = input.weights[slot];
            if (weight > 0.0 && joint < joint_count) {
                skin = skin + palette[start + joint] * weight;
                moved = true;
            }
        }
        if (moved) {
            matrix = draw.model * skin;
        }
    }
    return Posed(matrix, position, normal);
}

@vertex
fn vs_mesh(input: VertexIn) -> VertexOut {
    let posed = pose(input);
    let world = posed.matrix * vec4<f32>(posed.position, 1.0);
    var out: VertexOut;
    out.clip = scene.view_proj * world;
    out.world = world.xyz;
    out.normal = (posed.matrix * vec4<f32>(posed.normal, 0.0)).xyz;
    out.uv = input.uv;
    out.tangent = vec4<f32>((posed.matrix * vec4<f32>(input.tangent.xyz, 0.0)).xyz, input.tangent.w);
    out.curvature = input.curvature;
    if (draw.flags.x == 5u) {
        // a ribbon turned to face the camera: its tangent the strand's direction (the mesh's
        // normal), its fourth the coverage
        let direction = safe_normalize(out.normal);
        let ribbon = strand_vertex(world.xyz, direction, input.uv, draw.strand.xy);
        out.clip = scene.view_proj * vec4<f32>(ribbon.world, 1.0);
        out.world = ribbon.world;
        out.normal = ribbon.normal;
        out.tangent = vec4<f32>(direction, ribbon.coverage);
    }
    return out;
}

// `mesh::strand_sample_step`: the coverage a strand's fragment at `world` needs at `sample`
fn strand_sample_step(world: vec3<f32>, sample: u32) -> f32 {
    let seed = dot(world, vec3<f32>(12.9898, 78.233, 37.719)) * 1000.0 + f32(sample) * 7.31;
    return fract(sin(seed) * 43758.547);
}

struct StrandVertex {
    world: vec3<f32>,
    normal: vec3<f32>,
    coverage: f32,
};

// `mesh::strand_vertex`: a strand's point on its ribbon's side `uv.x`, the ribbon `widths`
// (root, tip; `uv.y` 1 at the root) wide but never under a pixel, turned to face the camera
fn strand_vertex(centre: vec3<f32>, direction: vec3<f32>, uv: vec2<f32>, widths: vec2<f32>) -> StrandVertex {
    let width = widths.y + (widths.x - widths.y) * clamp(uv.y, 0.0, 1.0);
    let view = safe_normalize(scene.eye.xyz - centre);
    let across = safe_normalize(cross(direction, view));
    let facing = safe_normalize(cross(across, direction));
    let near = scene.view_proj * vec4<f32>(centre, 1.0);
    let far = scene.view_proj * vec4<f32>(centre + across * 0.01, 1.0);
    var pixel = width;
    if (near.w > 1e-6 && far.w > 1e-6) {
        let moved = (far.xy / far.w - near.xy / near.w) * scene.picture.xy * 0.5;
        let pixels = length(moved);
        if (pixels > 1e-6) {
            pixel = 0.01 / pixels;
        }
    }
    let drawn = max(width, pixel);
    var coverage = 0.0;
    if (drawn > 0.0) {
        coverage = clamp(width / drawn, 0.0, 1.0);
    }
    let side = select(-1.0, 1.0, uv.x >= 0.0);
    var out: StrandVertex;
    out.world = centre + across * (drawn * 0.5 * side);
    out.normal = safe_normalize(across * side + facing);
    out.coverage = coverage;
    return out;
}

// `vector` made unit length, or +z when it has none: the CPU's `normalize`, never NaN
fn safe_normalize(vector: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(vector, vector);
    if (length_squared > 1e-24) {
        return vector * inverseSqrt(length_squared);
    }
    return vec3<f32>(0.0, 0.0, 1.0);
}

fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    return select(pow((color + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), color / 12.92, color <= vec3<f32>(0.04045));
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    let clamped = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(clamped, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055), clamped * 12.92, clamped <= vec3<f32>(0.0031308));
}

fn straight(texel: vec4<f32>, premultiplied: bool) -> vec4<f32> {
    if (premultiplied && texel.a > 1e-6) {
        return vec4<f32>(texel.rgb / texel.a, texel.a);
    }
    return texel;
}

fn shininess(roughness: f32) -> f32 {
    let clamped = clamp(roughness, 0.04, 1.0);
    return clamp(2.0 / (clamped * clamped * clamped * clamped) - 2.0, 1.0, 4096.0);
}

// the inverse square, held below a centimetre, windowed to nothing at the range: the CPU's
// `distance_falloff`
fn distance_falloff(distance: f32, range: f32) -> f32 {
    let inverse_square = 1.0 / max(distance * distance, 1e-4);
    if (range > 0.0) {
        let ratio = distance / range;
        let window = clamp(1.0 - ratio * ratio * ratio * ratio, 0.0, 1.0);
        return inverse_square * window * window;
    }
    return inverse_square;
}

// the CPU's `cone_falloff`, given the cosines of the half-angles
fn cone_falloff(cos_angle: f32, cos_inner: f32, cos_outer: f32) -> f32 {
    let amount = clamp((cos_angle - cos_outer) / max(cos_inner - cos_outer, 1e-4), 0.0, 1.0);
    return amount * amount * (3.0 - 2.0 * amount);
}

struct Arrival {
    to_light: vec3<f32>,
    amount: f32,
};

// the CPU's `light_arrival`
fn light_arrival(light: Light, world: vec3<f32>) -> Arrival {
    if (light.shadow.x == 0u) {
        return Arrival(safe_normalize(-light.direction.xyz), 1.0);
    }
    let toward = light.position.xyz - world;
    let to_light = safe_normalize(toward);
    var amount = distance_falloff(length(toward), light.position.w);
    if (light.shadow.x == 2u) {
        let axis = safe_normalize(light.direction.xyz);
        amount = amount * cone_falloff(-dot(to_light, axis), light.cone.x, light.cone.y);
    }
    return Arrival(to_light, amount);
}

// the CPU's `cube_face`: which of a point light's six views sees `offset` from it
fn cube_face(offset: vec3<f32>) -> u32 {
    let magnitude = abs(offset);
    if (magnitude.x >= magnitude.y && magnitude.x >= magnitude.z) {
        return select(1u, 0u, offset.x >= 0.0);
    }
    if (magnitude.y >= magnitude.z) {
        return select(3u, 2u, offset.y >= 0.0);
    }
    return select(5u, 4u, offset.z >= 0.0);
}

// the CPU's `project_raw`: where `point` falls in shadow view `view`'s map, as texels (a
// texel's centre at its index + 0.5) and clip depth, wherever it is; `inside` false only
// behind the view
struct Projected {
    at: vec2<f32>,
    depth: f32,
    inside: bool,
};

fn project_raw(view: ShadowView, point: vec3<f32>) -> Projected {
    let clip = view.view_proj * vec4<f32>(point, 1.0);
    if (!(clip.w > 1e-6)) {
        return Projected(vec2<f32>(0.0), 0.0, false);
    }
    let unit = vec2<f32>(clip.x / clip.w * 0.5 + 0.5, 0.5 - clip.y / clip.w * 0.5);
    return Projected(unit * view.info.y, clip.z / clip.w, true);
}

// the CPU's `project_into`: `inside` false outside the map or past its depth too
fn project_into(view: ShadowView, point: vec3<f32>) -> Projected {
    var projected = project_raw(view, point);
    let side = view.info.y;
    projected.inside = projected.inside && projected.depth >= 0.0 && projected.depth <= 1.0
        && projected.at.x >= 0.0 && projected.at.x <= side && projected.at.y >= 0.0 && projected.at.y <= side;
    return projected;
}

// the CPU's `sample_shadow`: a 3×3 filter of bilinear taps is a 4×4 block of comparisons
// weighted (1 − f, 1, 1, f) each way, gathered here 2×2 at a time
fn shadow_sample(view_index: u32, point: vec3<f32>) -> f32 {
    let view = scene.shadows[view_index];
    let projected = project_into(view, point);
    if (!projected.inside) {
        return 1.0;
    }
    let depth = projected.depth;
    let texels = projected.at - vec2<f32>(0.5);
    let corner = floor(texels);
    let fraction = texels - corner;
    let weights_x = vec4<f32>(1.0 - fraction.x, 1.0, 1.0, fraction.x);
    let weights_y = vec4<f32>(1.0 - fraction.y, 1.0, 1.0, fraction.y);
    let layer_side = scene.params.z;
    var lit = 0.0;
    for (var block_y = 0u; block_y < 2u; block_y = block_y + 1u) {
        for (var block_x = 0u; block_x < 2u; block_x = block_x + 1u) {
            // the block whose top-left texel is (corner − 1 + 2 block): gathered where its four
            // texels meet, so no rounding picks a neighbour
            let meet = corner + vec2<f32>(f32(2u * block_x), f32(2u * block_y));
            let gathered = textureGatherCompare(shadow_maps, shadow_sampler, meet / layer_side, i32(view_index), depth);
            let left = weights_x[2u * block_x];
            let right = weights_x[2u * block_x + 1u];
            let top = weights_y[2u * block_y];
            let bottom = weights_y[2u * block_y + 1u];
            // a gather's order: (left, bottom), (right, bottom), (right, top), (left, top)
            lit = lit + gathered.x * left * bottom + gathered.y * right * bottom
                + gathered.z * right * top + gathered.w * left * top;
        }
    }
    return lit / 9.0;
}

// the CPU's `stored_depth`: a texel of view `view_index`'s map, reads off its left or top edge
// clamped to it, off its right or bottom edge reading the layer (cleared to the far plane)
fn stored_depth(view_index: u32, texel: vec2<f32>) -> f32 {
    let last = scene.params.z - 1.0;
    let clamped = clamp(texel, vec2<f32>(0.0), vec2<f32>(last));
    return textureLoad(shadow_maps, vec2<i32>(clamped), i32(view_index), 0);
}

// the CPU's `bilinear_tap`: a receiver at `depth` compared with the four texels about `at`
// (texel centres at whole numbers here), weighted by nearness — one gather where they meet
fn bilinear_tap(view_index: u32, at: vec2<f32>, depth: f32) -> f32 {
    let corner = floor(at);
    let fraction = at - corner;
    let gathered = textureGatherCompare(shadow_maps, shadow_sampler, (corner + vec2<f32>(1.0)) / scene.params.z, i32(view_index), depth);
    // a gather's order: (left, bottom), (right, bottom), (right, top), (left, top)
    return mix(mix(gathered.w, gathered.z, fraction.x), mix(gathered.x, gathered.y, fraction.x), fraction.y);
}

// the CPU's `pixel_noise`: interleaved gradient noise at a pixel of the picture
fn pixel_noise(pixel: vec2<f32>) -> f32 {
    return fract(52.982918 * fract(0.06711056 * pixel.x + 0.00583715 * pixel.y));
}

// the CPU's `disc_tap`: tap `index` of 32 on a golden-angle spiral over the unit disc, turned
fn disc_tap(index: u32, turn: f32) -> vec2<f32> {
    let radius = sqrt((f32(index) + 0.5) / 32.0);
    let angle = f32(index) * 2.399963 + turn;
    return radius * vec2<f32>(cos(angle), sin(angle));
}

// the CPU's `ShadowView::distance`: a clip depth back to a distance along the view's axis
fn view_distance(view: ShadowView, perspective: bool, depth: f32) -> f32 {
    let near = view.info.z;
    let far = view.info.w;
    if (perspective) {
        return near * far / max(far - depth * (far - near), 1e-6);
    }
    return near + depth * (far - near);
}

// the CPU's `basis`: a right and an up square to the unit vector `forward`
fn right_of(forward: vec3<f32>) -> vec3<f32> {
    let hint = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(forward.y) > 0.99);
    return safe_normalize(cross(forward, hint));
}

// the CPU's `receiver_slope`: how the receiver's plane changes depth a texel across the map
fn receiver_slope(view: ShadowView, point: vec3<f32>, normal: vec3<f32>, to_light: vec3<f32>) -> vec2<f32> {
    let origin = project_raw(view, point);
    if (!origin.inside) {
        return vec2<f32>(0.0);
    }
    let step = view.info.x * max((view.view_proj * vec4<f32>(point, 1.0)).w, 1e-6);
    let tangent = right_of(normal);
    let bitangent = cross(tangent, normal);
    let first = project_raw(view, point + tangent * step);
    let second = project_raw(view, point + bitangent * step);
    let away = project_raw(view, point - to_light * step);
    if (!(first.inside && second.inside && away.inside)) {
        return vec2<f32>(0.0);
    }
    let first_at = first.at - origin.at;
    let second_at = second.at - origin.at;
    let first_depth = first.depth - origin.depth;
    let second_depth = second.depth - origin.depth;
    let determinant = first_at.x * second_at.y - first_at.y * second_at.x;
    if (abs(determinant) < 1e-3) {
        return vec2<f32>(0.0);
    }
    let cap = 4.0 * abs(away.depth - origin.depth);
    let slope = vec2<f32>(first_depth * second_at.y - second_depth * first_at.y,
                          second_depth * first_at.x - first_depth * second_at.x) / determinant;
    return clamp(slope, vec2<f32>(-cap), vec2<f32>(cap));
}

// the CPU's `soft_shadow`: a disc of 32 bilinear taps, its radius `softness` texels, or with a
// light `size` the penumbra its blockers cast there (percentage-closer soft shadows), each tap
// compared against the receiver's plane where it reads
fn soft_shadow(view_index: u32, perspective: bool, point: vec3<f32>, size: f32, softness_given: f32, turn: f32, slope: vec2<f32>) -> f32 {
    let view = scene.shadows[view_index];
    let projected = project_into(view, point);
    if (!projected.inside) {
        return 1.0;
    }
    let depth = projected.depth;
    let softness = max(softness_given, 1.0);
    var radius = softness;
    if (size > 0.0) {
        var blockers = 0.0;
        var found = 0u;
        for (var index = 0u; index < 32u; index = index + 1u) {
            let offset = disc_tap(index, turn) * softness;
            let stored = stored_depth(view_index, floor(projected.at + offset));
            if (stored < depth + dot(slope, offset)) {
                blockers = blockers + view_distance(view, perspective, stored);
                found = found + 1u;
            }
        }
        if (found == 0u) {
            return 1.0;
        }
        let blocker = blockers / f32(found);
        let receiver = view_distance(view, perspective, depth);
        let gap = max(receiver - blocker, 0.0);
        var penumbra = size * gap;
        var texel = view.info.x;
        if (perspective) {
            penumbra = penumbra / max(blocker, 1e-4);
            texel = texel * receiver;
        }
        radius = clamp(penumbra * 0.5 / max(texel, 1e-9), 1.0, softness);
    }
    var lit = 0.0;
    for (var index = 0u; index < 32u; index = index + 1u) {
        let offset = disc_tap(index, turn) * radius;
        lit = lit + bilinear_tap(view_index, projected.at - vec2<f32>(0.5) + offset, depth + dot(slope, offset));
    }
    return lit / 32.0;
}

// the CPU's `shadow_visibility`: the receiver moved toward the light and along its surface's
// normal by the light's biases, in texels where it stands, then filtered 3×3 or soft
fn shadow_visibility(light: Light, world: vec3<f32>, surface_normal: vec3<f32>, to_light: vec3<f32>, turn: f32) -> f32 {
    var at = light.shadow.y;
    if (light.shadow.z == 6u) {
        at = at + cube_face(world - light.position.xyz);
    }
    let view = scene.shadows[at];
    let texel = view.info.x * max((view.view_proj * vec4<f32>(world, 1.0)).w, 0.0);
    let facing = clamp(abs(dot(surface_normal, to_light)), 1e-3, 1.0);
    let slope = min(sqrt(max(1.0 - facing * facing, 0.0)) / facing, 4.0);
    let toward = light.cone.z * texel * (1.0 + slope);
    let along = light.cone.w * texel;
    let biased = world + surface_normal * along + to_light * toward;
    // the CPU's `shadow_softness`: the softness in metres as texels where the receiver stands,
    // held to 1 … 32, or 32 for a light with a size and no softness
    var softness = 0.0;
    if (light.color.w > 0.0 && texel > 0.0) {
        softness = clamp(light.color.w / texel, 1.0, 32.0);
    } else if (light.direction.w > 0.0) {
        softness = 32.0;
    }
    if (softness > 0.0) {
        let receiver_plane = receiver_slope(view, biased, surface_normal, to_light);
        return soft_shadow(at, light.shadow.x != 0u, biased, light.direction.w, softness, turn, receiver_plane);
    }
    return shadow_sample(at, biased);
}

// the CPU's `sh_irradiance`: Ramamoorthi and Hanrahan's irradiance from the scene's nine
// coefficients, held at 0, over π
fn sh_irradiance(normal: vec3<f32>) -> vec3<f32> {
    let c1 = 0.429043;
    let c2 = 0.511664;
    let c3 = 0.743125;
    let c4 = 0.886227;
    let c5 = 0.247708;
    let x = normal.x;
    let y = normal.y;
    let z = normal.z;
    let sh = scene.ambient_sh;
    let irradiance = c4 * sh[0].rgb
        + 2.0 * c2 * (y * sh[1].rgb + z * sh[2].rgb + x * sh[3].rgb)
        + 2.0 * c1 * (x * y * sh[4].rgb + y * z * sh[5].rgb + x * z * sh[7].rgb)
        + (c3 * z * z - c5) * sh[6].rgb
        + c1 * (x * x - y * y) * sh[8].rgb;
    return max(irradiance, vec3<f32>(0.0)) / 3.14159265;
}

// the CPU's `ambient`
fn ambient(normal: vec3<f32>) -> vec3<f32> {
    if (scene.params.w > 0.5) {
        return sh_irradiance(normal);
    }
    return mix(scene.ground.rgb, scene.sky.rgb, normal.y * 0.5 + 0.5);
}

// the CPU's `fresnel`: Schlick's, its grazing reflectance min(50 F0, 1)
fn fresnel(f0: f32, view_dot_half: f32) -> f32 {
    let grazing = pow(1.0 - clamp(view_dot_half, 0.0, 1.0), 5.0);
    return f0 + (min(50.0 * f0, 1.0) - f0) * grazing;
}

// one Blinn-Phong lobe normalised to (n + 8) / 8, the BRDF's 1/π folded into the light
fn specular_lobe(roughness: f32, normal_dot_half: f32) -> f32 {
    let exponent = shininess(roughness);
    return (exponent + 8.0) / 8.0 * pow(max(normal_dot_half, 0.0), exponent);
}

// the CPU's `specular_lobes`
fn specular_lobes(normal_dot_half: f32) -> f32 {
    let first = specular_lobe(draw.material.x * draw.lobes.x, normal_dot_half);
    if (draw.lobes.z >= 1.0) {
        return first;
    }
    return draw.lobes.z * first + (1.0 - draw.lobes.z) * specular_lobe(draw.material.x * draw.lobes.y, normal_dot_half);
}

// `mesh::view_distance` (the CPU's name): how far along the camera's view a clip depth lies
fn camera_distance(view_proj: mat4x4<f32>, depth: f32) -> f32 {
    let depth_row = vec4<f32>(view_proj[0].z, view_proj[1].z, view_proj[2].z, view_proj[3].z);
    let w_row = vec4<f32>(view_proj[0].w, view_proj[1].w, view_proj[2].w, view_proj[3].w);
    let w_length = dot(w_row.xyz, w_row.xyz);
    if (w_length < 1e-12) {
        return depth / max(sqrt(dot(depth_row.xyz, depth_row.xyz)), 1e-12);
    }
    let slope = dot(depth_row.xyz, w_row.xyz) / w_length;
    let offset = depth_row.w - slope * w_row.w;
    let gap = depth - slope;
    if (abs(gap) < 1e-12) {
        return 3.4028235e38;
    }
    return offset / gap;
}

// `mesh::occlusion_tap`: a golden-angle spiral over the hemisphere, as a share of the radius
fn occlusion_tap(index: u32) -> vec3<f32> {
    let share = (f32(index) + 0.5) / 16.0;
    let angle = f32(index) * 2.399963;
    let across = sqrt(share);
    let reach = 0.2 + 0.8 * share * share;
    return vec3<f32>(cos(angle) * across * reach, sin(angle) * across * reach, sqrt(1.0 - share) * reach);
}

// `mesh::ambient_occlusion`: how much of the ambient light reaches `world` past what the
// camera's depth view (scene.occlusion.z, one past its index; 0 without) holds round it
fn ambient_occlusion(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    let radius = scene.occlusion.x;
    let at = u32(scene.occlusion.z);
    if (at == 0u || radius <= 0.0) {
        return 1.0;
    }
    let view = scene.shadows[at - 1u];
    let own = view.view_proj * vec4<f32>(world, 1.0);
    if (own.w <= 1e-6) {
        return 1.0;
    }
    let own_distance = camera_distance(view.view_proj, own.z / own.w);
    var helper = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(normal.y) < 0.99) {
        helper = vec3<f32>(0.0, 1.0, 0.0);
    }
    let across = safe_normalize(cross(helper, normal));
    let along = cross(normal, across);
    let side = u32(view.info.y);
    var occluded = 0.0;
    for (var index = 0u; index < 16u; index = index + 1u) {
        let tap = occlusion_tap(index);
        let point = world + (across * tap.x + along * tap.y + normal * tap.z) * radius;
        let clip = view.view_proj * vec4<f32>(point, 1.0);
        if (clip.w <= 1e-6) {
            continue;
        }
        let across_unit = clip.x / clip.w * 0.5 + 0.5;
        let down_unit = 0.5 - clip.y / clip.w * 0.5;
        if (across_unit < 0.0 || across_unit >= 1.0 || down_unit < 0.0 || down_unit >= 1.0) {
            continue;
        }
        let column = min(u32(across_unit * f32(side)), side - 1u);
        let row = min(u32(down_unit * f32(side)), side - 1u);
        let stored = camera_distance(view.view_proj, textureLoad(shadow_maps, vec2<i32>(i32(column), i32(row)), i32(at - 1u), 0));
        let reached = camera_distance(view.view_proj, clip.z / clip.w);
        if (stored < reached - radius * 0.05) {
            occluded = occluded + min(radius / max(abs(own_distance - stored), 1e-6), 1.0);
        }
    }
    return clamp(1.0 - scene.occlusion.y * occluded / 16.0, 0.0, 1.0);
}

// a `:hair` highlight (`mesh::shade`): across the strand shifted along the normal, sharpened
fn hair_highlight(strand: vec3<f32>, normal: vec3<f32>, half_vector: vec3<f32>, shift: f32, sharpness: f32) -> f32 {
    let along = dot(safe_normalize(strand + normal * shift), half_vector);
    return pow(sqrt(max(1.0 - along * along, 0.0)), sharpness);
}

// `mesh::skin_diffuse`: the skin table read between its four nearest texels, 65 columns for
// the cosine to the light (−1 … 1), 32 rows by the square root of the curvature per millimetre
const SKIN_TABLE_LAST = vec2<f32>(64.0, 31.0);

fn skin_diffuse(facing: f32, curvature: f32) -> vec3<f32> {
    let across = clamp(facing * 0.5 + 0.5, 0.0, 1.0) * SKIN_TABLE_LAST.x;
    let down = sqrt(clamp(curvature, 0.0, 1.0)) * SKIN_TABLE_LAST.y;
    let column = min(floor(across), SKIN_TABLE_LAST.x - 1.0);
    let row = min(floor(down), SKIN_TABLE_LAST.y - 1.0);
    let across_share = across - column;
    let down_share = down - row;
    let corner = vec2<i32>(i32(column), i32(row));
    let upper_left = textureLoad(skin_table, corner, 0).rgb;
    let upper_right = textureLoad(skin_table, corner + vec2<i32>(1, 0), 0).rgb;
    let lower_left = textureLoad(skin_table, corner + vec2<i32>(0, 1), 0).rgb;
    let lower_right = textureLoad(skin_table, corner + vec2<i32>(1, 1), 0).rgb;
    let upper = upper_left + (upper_right - upper_left) * across_share;
    let lower = lower_left + (lower_right - lower_left) * across_share;
    return upper + (lower - upper) * down_share;
}

// `mesh::DETAIL_FLAT`: the byte 128, a detail texel that does not tilt
const DETAIL_FLAT = 128.0 / 255.0;

// `mesh::detail_tilt`
fn detail_tilt(texel: vec4<f32>) -> vec2<f32> {
    return (texel.rg - vec2<f32>(DETAIL_FLAT)) * 2.0;
}

// `mesh::detail_normal`: the partial-derivative blend, both z's multiplied through; not unit
// length
fn detail_normal(surface: vec3<f32>, tilt: vec2<f32>, strength: f32) -> vec3<f32> {
    let upright = sqrt(max(1.0 - tilt.x * tilt.x - tilt.y * tilt.y, 0.0));
    return vec3<f32>(surface.xy * upright + tilt * strength * surface.z, surface.z * upright);
}

// `mesh::detail_specular`: how much of the specular and the rim a cavity leaves
fn detail_specular(cavity: f32, amount: f32) -> f32 {
    return clamp(1.0 - amount * (1.0 - cavity), 0.0, 1.0);
}

fn shade(base: vec3<f32>, normal: vec3<f32>, view: vec3<f32>, world: vec3<f32>, surface_normal: vec3<f32>, occlusion: f32, turn: f32, strand: vec3<f32>, curvature: f32, specular_kept: f32) -> vec3<f32> {
    var out = base * ambient(normal) * occlusion;
    let f0 = draw.lobes.w;
    // a `:hair` surface's highlights: as sharp as its roughness asks
    let exponent = shininess(draw.material.x);
    let count = u32(scene.params.y);
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        let light = scene.lights[slot];
        let arrival = light_arrival(light, world);
        let arriving = arrival.amount;
        var seen = 1.0;
        if (light.shadow.z > 0u) {
            seen = shadow_visibility(light, world, surface_normal, arrival.to_light, turn);
        }
        let to_light = arrival.to_light;
        let facing = dot(normal, to_light);
        let lambert = max(facing, 0.0);
        let half_vector = safe_normalize(to_light + view);
        if (draw.flags.x == 4u || draw.flags.x == 5u) {
            let primary = hair_highlight(strand, normal, half_vector, -0.08, exponent);
            let secondary = hair_highlight(strand, normal, half_vector, 0.12, exponent * 0.25);
            let lit_side = clamp(facing + 0.25, 0.0, 1.0);
            let hair_diffuse = clamp(0.25 + 0.75 * facing, 0.0, 1.0);
            let shine = draw.material.y * lit_side * (vec3<f32>(primary) + secondary * base);
            out = out + light.color.rgb * arriving * seen * (base * hair_diffuse + shine);
            continue;
        }
        let specular = fresnel(f0, dot(view, half_vector)) * specular_lobes(dot(normal, half_vector)) * lambert * specular_kept;
        var diffuse = vec3<f32>(lambert * seen);
        if (draw.flags.x == 0u) {
            // the CPU's: the light under the skin in place of Lambert's, the shadow fading out
            // of what the scattering moved toward the terminator
            let scattered_seen = mix(1.0, seen, smoothstep(0.0, 0.3, facing));
            let scattered = skin_diffuse(facing, curvature * draw.skin.x);
            diffuse = diffuse + (scattered - vec3<f32>(lambert)) * scattered_seen;
        }
        out = out + light.color.rgb * arriving * (base * diffuse + vec3<f32>(specular * seen));
    }
    let rim = draw.material.z * specular_kept * pow(1.0 - max(dot(normal, view), 0.0), 3.0);
    return out + rim * scene.sky.rgb;
}

fn tone(color: vec3<f32>, exposure: f32) -> vec3<f32> {
    let exposed = max(color * exposure, vec3<f32>(0.0));
    let mapped = (exposed * (2.51 * exposed + vec3<f32>(0.03))) / (exposed * (2.43 * exposed + vec3<f32>(0.59)) + vec3<f32>(0.14));
    return linear_to_srgb(mapped);
}

struct MeshFragment {
    @location(0) color: vec4<f32>,
    // the samples it covers: all of them, but a `:hair` surface's coverage of its 4 (each
    // sample a step of `mesh::HAIR_COVERAGE_STEPS`, as the CPU painter's 2×2 sub-samples)
    @builtin(sample_mask) mask: u32,
};

@fragment
fn fs_mesh(fragment: VertexOut, @builtin(front_facing) front: bool) -> MeshFragment {
    // every texture sampled before any branch (uniform control flow); a missing one is a
    // white or flat-normal pixel bound in its place
    let base_texel = straight(textureSample(base_texture, texture_sampler, fragment.uv), (draw.flags.y & 2u) != 0u);
    let normal_texel = straight(textureSample(normal_texture, texture_sampler, fragment.uv), (draw.flags.y & 8u) != 0u);
    let occlusion_texel = straight(textureSample(occlusion_texture, texture_sampler, fragment.uv), (draw.flags.y & 32u) != 0u);
    // the detail tile at its own repeats, so its mip level follows them (the CPU's `detail_lod`)
    let detail_texel = straight(textureSample(detail_texture, texture_sampler, fragment.uv * draw.detail.xy), (draw.flags.y & 128u) != 0u);
    let texel = select(vec4<f32>(1.0), base_texel, (draw.flags.y & 1u) != 0u);
    var alpha = texel.a * draw.color.a;
    if (draw.flags.x == 2u) {
        if (alpha < draw.material.w) {
            discard;
        }
        alpha = 1.0;
    } else if (draw.flags.x == 3u) {
        if (alpha <= 0.002) {
            discard;
        }
    } else {
        alpha = 1.0;
    }
    var mask = 0xFFFFFFFFu;
    if (draw.flags.x == 4u || draw.flags.x == 5u) {
        var coverage = clamp((texel.a * draw.color.a - 0.5 * draw.material.w) / max(draw.material.w, 1e-6), 0.0, 1.0);
        mask = select(0u, 1u, coverage > 0.125) | select(0u, 2u, coverage > 0.625)
            | select(0u, 4u, coverage > 0.875) | select(0u, 8u, coverage > 0.375);
        // a strand thinner than a pixel covers only its share of one, at samples its own
        // position picks, so overlapping strands add up (`mesh::strand_sample_step`)
        if (draw.flags.x == 5u) {
            coverage = coverage * fragment.tangent.w;
            mask = 0u;
            for (var sample = 0u; sample < 4u; sample = sample + 1u) {
                if (coverage > strand_sample_step(fragment.world, sample)) {
                    mask = mask | (1u << sample);
                }
            }
        }
        if (mask == 0u) {
            discard;
        }
    }
    let base = srgb_to_linear(texel.rgb) * draw.color.rgb;
    var normal = safe_normalize(fragment.normal);
    if (!front) {
        normal = -normal;
    }
    // the shadow lookups move the receiver along the surface's own normal, not the normal map's
    let surface_normal = normal;
    // the normal map's normal, and the detail tile's laid over it (or over the bare surface)
    // unless it is flat there: the CPU's `fill_triangle`
    var local = vec3<f32>(0.0, 0.0, 1.0);
    var framed = (draw.flags.y & 4u) != 0u;
    if (framed) {
        local = normal_texel.rgb * 2.0 - vec3<f32>(1.0);
    }
    if ((draw.flags.y & 256u) != 0u) {
        let tilt = detail_tilt(detail_texel);
        if (tilt.x != 0.0 || tilt.y != 0.0) {
            local = detail_normal(local, tilt, draw.detail.z);
            framed = true;
        }
    }
    // its cavity damps the specular and the rim, never the diffuse
    var specular_kept = 1.0;
    if ((draw.flags.y & 64u) != 0u) {
        specular_kept = detail_specular(detail_texel.b, draw.detail.w);
    }
    if (framed) {
        let tangent = safe_normalize(fragment.tangent.xyz);
        let bitangent = cross(normal, tangent);
        let sign = select(1.0, -1.0, fragment.tangent.w < 0.0);
        normal = safe_normalize(tangent * local.x + sign * bitangent * local.y + normal * local.z);
    }
    let view = safe_normalize(scene.eye.xyz - fragment.world);
    // the ambient's occlusion: its map's, times the scene's screen-space ambient occlusion
    let occlusion = select(1.0, occlusion_texel.r, (draw.flags.y & 16u) != 0u)
        * ambient_occlusion(fragment.world, surface_normal);
    // a soft shadow's taps turned by the pixel's noise, the CPU painter's at the same pixel
    let turn = 6.28318531 * pixel_noise(floor(fragment.clip.xy));
    // a hair's strand: the bitangent, as its UV v runs
    var strand = safe_normalize(cross(surface_normal, safe_normalize(fragment.tangent.xyz)) * select(1.0, -1.0, fragment.tangent.w < 0.0));
    // a strand's own direction
    if (draw.flags.x == 5u) {
        strand = safe_normalize(fragment.tangent.xyz);
    }
    let lit = tone(shade(base, normal, view, fragment.world, surface_normal, occlusion, turn, strand, fragment.curvature, specular_kept), scene.params.x);
    return MeshFragment(vec4<f32>(lit * alpha, alpha), mask);
}

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_shadow(input: VertexIn, @builtin(instance_index) view_index: u32) -> ShadowOut {
    let posed = pose(input);
    var out: ShadowOut;
    var world = (posed.matrix * vec4<f32>(posed.position, 1.0)).xyz;
    if (draw.flags.x == 5u) {
        // the ribbon as the camera sees it, as the CPU painter's posed vertices are
        let direction = safe_normalize((posed.matrix * vec4<f32>(posed.normal, 0.0)).xyz);
        world = strand_vertex(world, direction, input.uv, draw.strand.xy).world;
    }
    out.clip = scene.shadows[view_index].view_proj * vec4<f32>(world, 1.0);
    out.uv = input.uv;
    return out;
}

// a `:cutout`, `:blend` or `:hair` surface casts only where its alpha (read at the texture's
// own size, not a mip level) reaches its cutoff, as the CPU's `render_shadow_map` tests it
@fragment
fn fs_shadow(fragment: ShadowOut) {
    let base_texel = straight(textureSampleLevel(base_texture, texture_sampler, fragment.uv, 0.0), (draw.flags.y & 2u) != 0u);
    let alpha = select(1.0, base_texel.a, (draw.flags.y & 1u) != 0u) * draw.color.a;
    if ((draw.flags.x == 2u || draw.flags.x == 3u || draw.flags.x == 4u || draw.flags.x == 5u) && alpha < draw.material.w) {
        discard;
    }
}
"#;
