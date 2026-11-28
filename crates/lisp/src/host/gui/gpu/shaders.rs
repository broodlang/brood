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
    // An outline (`aa` 1 + its width, ADR-832): only the band `width` wide inside the edge,
    // its distance |d + width/2| − width/2, with the same 1 px ramp on both of its sides.
    if (in.aa > 1.0) {
        let width = in.aa - 1.0;
        let band = abs(d + 0.5 * width) - 0.5 * width;
        return clamp(0.5 - band, 0.0, 1.0);
    }
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
/// `detail_specular`, and ADR-795's `lobe_exponent`, `highlight_exponent`, `lobe_spread`,
/// `pixel_span`, `sh_radiance`, `ambient_radiance`, `environment_brdf`, `specular_occlusion`,
/// `horizon`, `overlap`, `emitter_coverage`, and ADR-796's `eye_frame` (in `vs_mesh`) and
/// `eye_seen`) — keep the two in step. Writes
/// premultiplied, sRGB-encoded colour, the space the scene's picture is composited in.
/// `vs_shadow` / `fs_shadow` render a shadow map: the same posing, seen through the shadow
/// view the instance index names, alpha-tested for a `:cutout`, `:blend` or `:eye` surface.
/// The casters' pyramid (ADR-837): the `:contact-caster` view's depth halved level by level,
/// each texel the nearest of the 2×2 below it (an odd row or column left over taken by the last), so
/// one level's few texels say whether any caster lies anywhere near a pixel. `first` copies the
/// view's depth layer into level 0; `next` makes each level from the one before. Apart from
/// `MESH_SRC`, whose group 0 holds other bindings.
pub(super) const CASTER_PYRAMID_SRC: &str = r#"
@group(0) @binding(0) var depth_in: texture_depth_2d;
@group(0) @binding(1) var first_out: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var level_in: texture_2d<f32>;
@group(0) @binding(3) var next_out: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn first(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(first_out);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let depth = textureLoad(depth_in, vec2<i32>(id.xy), 0);
    textureStore(first_out, vec2<i32>(id.xy), vec4<f32>(depth, 0.0, 0.0, 1.0));
}

@compute @workgroup_size(8, 8)
fn next(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(next_out);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let below = vec2<i32>(textureDimensions(level_in));
    let last = below - vec2<i32>(1);
    let corner = vec2<i32>(id.xy) * 2;
    // (a level is half the one below rounded down: its last texel takes the odd row or column
    // left over too, so no caster at the view's edge is lost)
    let here = vec2<i32>(id.xy);
    let edge = vec2<i32>(size) - vec2<i32>(1);
    let wide = select(2, 3, here.x == edge.x && below.x > 2 * i32(size.x));
    let tall = select(2, 3, here.y == edge.y && below.y > 2 * i32(size.y));
    var nearest = 1.0;
    for (var row = 0; row < tall; row = row + 1) {
        for (var column = 0; column < wide; column = column + 1) {
            let texel = min(corner + vec2<i32>(column, row), last);
            nearest = min(nearest, textureLoad(level_in, texel, 0).r);
        }
    }
    textureStore(next_out, vec2<i32>(id.xy), vec4<f32>(nearest, 0.0, 0.0, 1.0));
}
"#;

pub(super) const MESH_SRC: &str = r#"
struct Light {
    position: vec4<f32>,        // xyz, range (0: no window)
    direction: vec4<f32>,       // xyz: the way the light travels, size (metres; radians directional)
    color: vec4<f32>,           // rgb, shadow softness (metres at the receiver; 0 and no size: 3×3)
    cone: vec4<f32>,            // cos inner, cos outer, shadow bias, shadow normal bias
    shadow: vec4<u32>,          // kind (0 directional, 1 point, 2 spot), first view, view count, shaped (0 or 1)
};
// a shape glossy surfaces show (`mesh::PlannedEmitter`, ADR-795)
struct Emitter {
    centre: vec4<f32>,          // xyz, shape (1 disc, 2 rect, 3 sphere, 4 a directional light's disc)
    facing: vec4<f32>,          // xyz: the unit way its light travels, one past its light's index (0: none)
    right: vec4<f32>,           // xyz: a rectangle's width direction, its half width (a disc's or sphere's radius)
    radiance: vec4<f32>,        // rgb, a rectangle's half height
};
struct ShadowView {
    view_proj: mat4x4<f32>,
    info: vec4<f32>,            // texel size (at 1 m for a perspective view), resolution, near, far
    map: vec4<f32>,             // where its map is kept (ADR-800): its array's side, the array (0 or 1), its layer; 1 for a perspective view (ADR-827)
    hair: vec4<f32>,            // where its hair can be in its opacity maps, in their texels: left, top, right, bottom (ADR-827; left past right: none)
};
struct SceneU {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    lights: array<Light, 8>,
    sky: vec4<f32>,
    ground: vec4<f32>,
    params: vec4<f32>,          // exposure, light count, spare (ADR-800: each view's `map` has its side), ambient is SH (0 or 1)
    ambient_sh: array<vec4<f32>, 9>,    // L00, L1-1, L10, L11, L2-2, L2-1, L20, L21, L22 (rgb)
    shadows: array<ShadowView, 24>,
    occlusion: vec4<f32>,       // ambient occlusion radius (metres; 0: none), strength, view + 1 (0: none), the `:contact-caster` view + 1 (ADR-837; 0: none)
    picture: vec4<f32>,         // the picture's width and height in pixels, how many emitters
    emitters: array<Emitter, 8>,
};
struct DrawU {
    model: mat4x4<f32>,
    color: vec4<f32>,
    skin: vec4<f32>,            // a `:skin` surface: its curvature (1 / mesh units) to 1 / mm, times `:scatter`; the `:coat`'s strength, its roughness
    material: vec4<f32>,        // roughness, specular, z by texture bit 512: a `:fibres` surface's `:density`, any other's `:see-through` (ADR-842); cutoff
    lobes: vec4<f32>,           // roughness times a, times b, mix of a, F0
    flags: vec4<u32>,           // surface, texture bits (`scenes::texture_bits`), palette offset, skinned
    morph_info: vec4<u32>,      // morphed (0 or 1), vertex count, palette joint count, first offset
    strand: vec4<f32>,          // a `:strands` surface's width at the root and the tip (metres); a hair surface's two highlights' shifts (`:shift`); a standard, cutout or blended surface's `:sheen-cloth` (colour, roughness; ADR-836)
    detail: vec4<f32>,          // the detail tile's repeats along u and v, its strength, its cavity
    // an `:eye` surface's optics (`mesh::Eye`, ADR-796; zeros on any other surface)
    eye_centres: array<vec4<f32>, 2>,   // each eye's centre in the mesh at rest; [0].w: how many eyes
    eye_axes: array<vec4<f32>, 2>,      // each eye's unit axis at rest
    eye_up: vec4<f32>,          // xyz: up in the iris's texture, at rest; the refractive index
    eye_shape: vec4<f32>,       // the eyeball's radius, the iris's, the cornea's, the iris's depth under the cornea's top
    eye_uv: vec4<f32>,          // the middle of the first eye's iris in the texture, then the second's
    eye_iris: vec4<f32>,        // the iris's radius in the texture, the pupil's share of the iris, the slope of the bowl the iris is lit as
    sheen: vec4<f32>,           // a hair surface's second highlight's colour (`:sheen`, ADR-799); any other surface's `:surroundings` (ADR-831); w 1 when given
    deform: vec4<u32>,          // pressed by the scene's contacts (ADR-805), its first posed vertex, whether it has a softness, its vertex count
    loose: vec4<f32>,           // its flesh lagging its bones (ADR-812): the amount (0 none), its lagging palette's offset, its joint count
    clamp_info: vec4<f32>,      // x: how many `:clamp-planes` its strands keep to (ADR-813); y: the UV set its `:wrinkles` read by (ADR-817); z: the strands a `:fibres` shadow stands for (ADR-813)
    clamps: array<vec4<f32>, 4>,  // each plane: its unit normal toward the strands' side, `normal · point`; on an `:eye` its pupils (ADR-835: [0] [1] each eye's scale, offset across and up, ruff width; [2] [3] its two waves, turns + phase's share of a turn, depth)
    wrinkles: array<vec4<f32>, 2>,  // a skin's eight wrinkle zones' weights (ADR-817; zero: none)
    fibre_look: vec4<f32>,      // a `:fibres` surface's scatter strength, scatter depth, colour spread, tip lighten (ADR-825)
    fibre_cut: vec4<f32>,       // its cuticle tilt, roughness spread, absorption a metre (ADR-825)
};
@group(0) @binding(0) var<uniform> scene: SceneU;
@group(0) @binding(1) var<storage, read> palette: array<mat4x4<f32>>;
@group(0) @binding(2) var shadow_maps: texture_depth_2d_array;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;
// `mesh::skin_table` (ADR-794): the diffuse light under skin, linear rgb, the cosine to the light
// across and the curvature down; read with `textureLoad` only (`skin_diffuse`)
@group(0) @binding(4) var skin_table: texture_2d<f32>;
// ADR-800: the shadow maps smaller than the largest, in a second array at their own largest size
// (`shadow_maps` holds the largest); a view's `map.y` says which, read through `map_gather` and
// `map_load`
@group(0) @binding(5) var smaller_shadow_maps: texture_depth_2d_array;
// ADR-827: the hair's opacity maps, a layer a shadow view by its index, at `HAIR_OPACITY_SIDE`: the
// nearest hair's depth, and the hair's density (each fragment's `-ln(1 - :shadow-keep)`) summed in
// the four slices behind it that end `HAIR_SLICES` metres past it (`hair_transmittance`)
@group(0) @binding(7) var hair_nearest: texture_depth_2d_array;
@group(0) @binding(8) var hair_opacity: texture_2d_array<f32>;
// the casters' pyramid (ADR-837, `CASTER_PYRAMID_SRC`): the `:contact-caster` view's nearest depth
// over ever larger squares, 1.0 where no caster is drawn (a 1×1 stand-in without a casters' view)
@group(0) @binding(9) var caster_pyramid: texture_2d<f32>;
// what the colour pass drew before its see-through draws (ADR-842: the first part of the split,
// resolved; display-encoded as the picture is), a 1×1 stand-in when it is not split
@group(0) @binding(10) var behind_picture: texture_2d<f32>;
const HAIR_OPACITY_SIDE: f32 = 1024.0;
const HAIR_SLICES: vec4<f32> = vec4<f32>(0.02, 0.05, 0.10, 0.25);
@group(1) @binding(0) var<uniform> draw: DrawU;
// this frame's morph offsets, blended on the CPU from the sparse targets (`mesh::add_morphs`):
// two vec4s a vertex (position, normal), a morphed draw's from `morph_info.w`, so a vertex
// reads one pair whatever the number of targets
@group(2) @binding(0) var<storage, read> morphs: array<vec4<f32>>;

// --- contacts (ADR-805): a pressed draw's skin laid onto what presses it, soft flesh rising in
// a ring round it; `mesh::deform_by_contacts`'s arithmetic, posed once a frame by the compute
// passes below and read back by the vertex stages
@group(2) @binding(1) var<storage, read> posed_vertices: array<vec4<f32>>;

struct ContactList {
    count: vec4<u32>,
    // six vec4 a contact: `[x y z kind]`, `[x y z w]`, its drag, its own limb's joints, its one limb's, its `:within`
    // (`scenes::contact_words`)
    shapes: array<vec4<f32>, 384>,
};
@group(3) @binding(8) var<storage, read> mesh_vertices: array<f32>;
@group(3) @binding(9) var<uniform> contact_list: ContactList;
@group(3) @binding(10) var<storage, read_write> contact_depths: array<atomic<u32>, 65>;
// (its 65th: how many garment vertices the push-out net moved this frame, ADR-839)
const PUSH_OUT_COUNTER: u32 = 64u;
@group(3) @binding(11) var<storage, read_write> posed_out: array<vec4<f32>>;
// each vertex's softness (ADR-805) and its garment cover (ADR-839), a vec4 a vertex (two spare); after
// the vertices (`draw.deform.w` of them) its push-out colliders: a vec4 of how many, then three vec4 a
// sphere (ADR-839; a compute stage binds at most eight storage buffers, so they share this one)
@group(3) @binding(12) var<storage, read> mesh_softness: array<vec4<f32>>;

// (`mesh::RING_WIDTH`, `RING_SPREAD`, `RING_LIFT`, `EDGE_BAND`, `EDGE_SPREAD`, `FIRM_INSET`, `FIRM_SOFTNESS`)
const RING_WIDTH: f32 = 0.012;
const RING_SPREAD: f32 = 2.0;
const RING_LIFT: f32 = 0.35;
const EDGE_BAND: f32 = 0.004;
const EDGE_SPREAD: f32 = 0.5;
const FIRM_INSET: f32 = 0.25;
const FIRM_SOFTNESS: f32 = 0.5;
const MAX_SQUASH: f32 = 0.02;
// `mesh::SCATTER_LIFT`: how far off the skin (in its own metres, times `:scatter`: `skin.x` is
// `:scatter` over 1000) the light under it near the terminator looks up its
// light's shadow (ADR-815)
const SCATTER_LIFT: f32 = 0.008;

// `mesh::smooth_outside`: `distance`'s smooth maximum with 0 over `band`
fn smooth_outside(distance: f32, band: f32) -> f32 {
    let share = max((band - abs(distance)) / max(band, 1e-9), 0.0);
    return max(distance, 0.0) + share * share * band * 0.25;
}

// `mesh::Contact::signed_distance`: how far `point` lies outside contact `slot` (negative inside),
// in w, and the unit direction out of it there, in xyz
fn contact_distance(slot: u32, point: vec3<f32>) -> vec4<f32> {
    let first = contact_list.shapes[slot * 6u];
    let second = contact_list.shapes[slot * 6u + 1u];
    let kind = u32(first.w + 0.5);
    if (kind == 0u) {
        return vec4<f32>(first.xyz, dot(first.xyz, point) - second.x);
    }
    var nearest = first.xyz;
    if (kind == 2u) {
        let along = second.xyz - first.xyz;
        let length_squared = dot(along, along);
        var share = 0.0;
        if (length_squared > 1e-12) {
            share = clamp(dot(point - first.xyz, along) / length_squared, 0.0, 1.0);
        }
        nearest = first.xyz + along * share;
    }
    let offset = point - nearest;
    let length = sqrt(dot(offset, offset));
    var direction = vec3<f32>(0.0, 1.0, 0.0);
    if (length > 1e-9) {
        direction = offset / length;
    }
    return vec4<f32>(direction, length - second.w);
}

// `mesh::skipped_by`: whether contact `slot` is this vertex's own limb, its weights on the contact's
// `:skip` joints passing a half
fn contact_skipped(slot: u32, input: VertexIn) -> bool {
    // its own limb (`:skip`), or not the one limb it presses (`:only`, its first joint -1: any)
    let skip = contact_list.shapes[slot * 6u + 3u];
    let only = contact_list.shapes[slot * 6u + 4u];
    var own = 0.0;
    var one = 0.0;
    for (var at = 0u; at < 4u; at = at + 1u) {
        let joint = f32(input.joints[at]);
        if (any(vec4<f32>(joint) == skip)) {
            own = own + input.weights[at];
        }
        if (any(vec4<f32>(joint) == only)) {
            one = one + input.weights[at];
        }
    }
    return own > 0.5 || (only.x >= 0.0 && one <= 0.5);
}

// whether contact `slot` is a garment's (`:garment`, ADR-839, in its drag's fourth float): it pushes
// `:garment` draws out of her and presses nothing else
fn contact_garment(slot: u32) -> bool {
    return contact_list.shapes[slot * 6u + 2u].w > 0.5;
}

// `mesh::GARMENT_MARGIN`: how far outside a garment contact the push-out net leaves a garment's
// vertex, metres (ADR-839)
const GARMENT_MARGIN: f32 = 0.0005;

// `mesh::within_weight`: how much contact `slot` acts at `point`, on its `:within` side only,
// eased in over `WITHIN_BAND` (ADR-816); everywhere when it has none (all zero)
const WITHIN_BAND: f32 = 0.02;
fn contact_within(slot: u32, point: vec3<f32>) -> f32 {
    let within = contact_list.shapes[slot * 6u + 5u];
    if (dot(within.xyz, within.xyz) <= 1e-12) {
        return 1.0;
    }
    return smoothstep(0.0, WITHIN_BAND, dot(within.xyz, point) - within.w);
}

// a mesh's vertex `index` read from its vertex buffer as storage (`VERTEX_FLOATS` 28 a vertex)
fn vertex_at(index: u32) -> VertexIn {
    let at = index * 28u;
    var input: VertexIn;
    input.index = index;
    input.position = vec3<f32>(mesh_vertices[at], mesh_vertices[at + 1u], mesh_vertices[at + 2u]);
    input.normal = vec3<f32>(mesh_vertices[at + 3u], mesh_vertices[at + 4u], mesh_vertices[at + 5u]);
    input.uv = vec2<f32>(mesh_vertices[at + 6u], mesh_vertices[at + 7u]);
    input.tangent = vec4<f32>(mesh_vertices[at + 8u], mesh_vertices[at + 9u], mesh_vertices[at + 10u], mesh_vertices[at + 11u]);
    input.joints = vec4<u32>(bitcast<u32>(mesh_vertices[at + 12u]), bitcast<u32>(mesh_vertices[at + 13u]), bitcast<u32>(mesh_vertices[at + 14u]), bitcast<u32>(mesh_vertices[at + 15u]));
    input.weights = vec4<f32>(mesh_vertices[at + 16u], mesh_vertices[at + 17u], mesh_vertices[at + 18u], mesh_vertices[at + 19u]);
    input.curvature = mesh_vertices[at + 20u];
    input.centre = vec3<f32>(mesh_vertices[at + 21u], mesh_vertices[at + 22u], mesh_vertices[at + 23u]);
    input.uv2 = vec2<f32>(mesh_vertices[at + 24u], mesh_vertices[at + 25u]);
    input.loose_weight = mesh_vertices[at + 26u];
    input.standoff = mesh_vertices[at + 27u];
    return input;
}

// `mesh::softness_at`: a pressed vertex's softness, wholly soft for a mesh without one
fn contact_softness(index: u32) -> f32 {
    if ((draw.deform.z & 1u) != 0u) {
        return clamp(mesh_softness[index].x, 0.0, 1.0);
    }
    return 1.0;
}

// how far short of a contact this vertex stops (ADR-824): its standoff, or its garment cover where
// that is larger and the draw says the garment is worn (`:covered`, ADR-839)
fn vertex_standoff(index: u32, input: VertexIn) -> f32 {
    if ((draw.deform.z & 2u) != 0u) {
        return max(input.standoff, mesh_softness[index].y);
    }
    return input.standoff;
}

// `mesh::skin_matrix` over the draw's lagging palette (ADR-812): where `position` would be were
// its bones where the game's followers carry them; `world` itself when none of them moves it
fn loose_world(input: VertexIn, position: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let skin = skinning(input, u32(draw.loose.y + 0.5), u32(draw.loose.z + 0.5));
    if (skin[3].w == 0.0) {
        return world;
    }
    return ((draw.model * skin) * vec4<f32>(position, 1.0)).xyz;
}

// pass A: each pressed vertex posed (into the scene's posed buffer) and each contact's deepest
// press into it kept (a positive float's bits order as the float does)
@compute @workgroup_size(64)
fn cs_contact_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= draw.deform.w) {
        return;
    }
    let input = vertex_at(id.x);
    let posed = pose(input);
    var world = (posed.matrix * vec4<f32>(posed.position, 1.0)).xyz;
    // its soft flesh toward where its bones' followers carry it (`mesh::pose_vertices`, ADR-812)
    if (draw.loose.x > 0.0) {
        let share = clamp(contact_softness(id.x) * draw.loose.x * input.loose_weight, 0.0, 1.0);
        if (share > 0.0) {
            world = world + (loose_world(input, posed.position, world) - world) * share;
        }
    }
    let normal = safe_normalize((posed.matrix * vec4<f32>(posed.normal, 0.0)).xyz);
    let tangent = safe_normalize((posed.matrix * vec4<f32>(input.tangent.xyz, 0.0)).xyz);
    let at = (draw.deform.y + id.x) * 3u;
    posed_out[at] = vec4<f32>(world, 1.0);
    posed_out[at + 1u] = vec4<f32>(normal, 0.0);
    posed_out[at + 2u] = vec4<f32>(tangent, input.tangent.w);
    // bone, or the limb that presses (softness 0), neither pressed nor sizing a ring
    if (contact_softness(id.x) <= 0.0) {
        return;
    }
    let count = min(contact_list.count.x, 64u);
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        // (stopping its standoff short of the surface, ADR-824)
        let distance = contact_distance(slot, world).w - vertex_standoff(id.x, input);
        // (a garment never sizes a contact's depth: the flesh under it does, ADR-839)
        if (distance < 0.0 && !contact_skipped(slot, input) && !contact_garment(slot) && (draw.flags.y & 8192u) == 0u) {
            // (as far as it acts there: its `:within` side, ADR-816)
            atomicMax(&contact_depths[slot], bitcast<u32>(-distance * contact_within(slot, world)));
        }
    }
}

// a pressed mesh's squeezed skin relaxed (ADR-822), from two buffers (the contact group may hold
// only eight storage buffers): its indices (the point and vertex counts, each vertex's point, each
// point's first vertex, each point's ring as `points + 1` offsets, then the rings) and its vec4s
// (each point at rest: its offset from its ring's centroid along its normal and across its frame's
// two tangents, its ring's mean edge length; then the frame's work, three a point: as posed with
// its squeeze mask in w, then two halves relaxed between)
@group(3) @binding(17) var<storage, read> ring_index: array<u32>;
@group(3) @binding(18) var<storage, read_write> ring_data: array<vec4<f32>>;
// each pass moves a squeezed point this share of the way to where its ring puts it
const RELAX_STEP: f32 = 0.5;
// the squeeze (its ring's edges posed against rest) the relax begins at, and is whole by
const SQUEEZE_FROM: f32 = 0.95;
const SQUEEZE_WHOLE: f32 = 0.75;
// the furthest a point slides from where its bones put it (it carries the textures with it)
const RELAX_SLIDE: f32 = 0.015;

fn ring_point_count() -> u32 {
    return ring_index[0];
}

fn ring_weld(vertex: u32) -> u32 {
    return ring_index[2u + vertex];
}

fn ring_rep(point: u32) -> u32 {
    return ring_index[2u + ring_index[1] + point];
}

fn ring_offset(at: u32) -> u32 {
    return ring_index[2u + ring_index[1] + ring_index[0] + at];
}

fn ring_neighbour(slot: u32) -> u32 {
    return ring_index[3u + ring_index[1] + 2u * ring_index[0] + slot];
}

// a point's place at rest (after the rest offsets)
fn ring_rest_place(point: u32) -> vec3<f32> {
    return ring_data[ring_point_count() + point].xyz;
}

// work slot `at` (0 … 3 points): as posed, then the two halves (after the rest places)
fn ring_work(at: u32) -> vec4<f32> {
    return ring_data[2u * ring_point_count() + at];
}

fn ring_set_work(at: u32, value: vec4<f32>) {
    ring_data[2u * ring_point_count() + at] = value;
}

// a point's posed place and the normal its bones and keys carry there
fn ring_posed(point: u32) -> vec3<f32> {
    return posed_out[(draw.deform.y + ring_rep(point)) * 3u].xyz;
}

fn ring_carried(point: u32) -> vec3<f32> {
    return safe_normalize(posed_out[(draw.deform.y + ring_rep(point)) * 3u + 1u].xyz);
}

// each point as posed, into the work and its first half
@compute @workgroup_size(64)
fn cs_relax_gather(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = ring_point_count();
    if (id.x >= count) {
        return;
    }
    let at = vec4<f32>(ring_posed(id.x), 0.0);
    ring_set_work(id.x, at);
    ring_set_work(count + id.x, at);
}

// each point's squeeze: its most squeezed ring edge, posed against that edge at rest (skin squeezed
// one way is often stretched the other, so a mean stays near whole where it buckles), as a mask
// rising from nothing at `SQUEEZE_FROM` to whole at `SQUEEZE_WHOLE`
@compute @workgroup_size(64)
fn cs_relax_mask(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = ring_point_count();
    if (id.x >= count) {
        return;
    }
    let start = ring_offset(id.x);
    let end = ring_offset(id.x + 1u);
    let rest = ring_data[id.x].w;
    if (end <= start || rest <= 0.0) {
        return;
    }
    let here = ring_work(id.x);
    let home = ring_rest_place(id.x);
    var squeeze = 1.0;
    for (var slot = start; slot < end; slot = slot + 1u) {
        let other = ring_neighbour(slot);
        let at_rest = length(ring_rest_place(other) - home);
        if (at_rest > 1e-6) {
            squeeze = min(squeeze, length(ring_work(other).xyz - here.xyz) / at_rest);
        }
    }
    ring_set_work(id.x, vec4<f32>(here.xyz, 1.0 - smoothstep(SQUEEZE_WHOLE, SQUEEZE_FROM, squeeze)));
}

// one relax pass from one half of the work into the other: a squeezed point moved toward its
// ring's centroid plus its rest offset, turned into its posed frame, by its mask
fn ring_relax(id: u32, read_half: u32, write_half: u32) {
    let count = ring_point_count();
    if (id >= count) {
        return;
    }
    let here = ring_work(read_half * count + id).xyz;
    let posed = ring_work(id);
    let start = ring_offset(id);
    let end = ring_offset(id + 1u);
    if (posed.w <= 0.0 || end <= start) {
        ring_set_work(write_half * count + id, vec4<f32>(here, 0.0));
        return;
    }
    var centroid = vec3<f32>(0.0);
    for (var slot = start; slot < end; slot = slot + 1u) {
        centroid = centroid + ring_work(read_half * count + ring_neighbour(slot)).xyz;
    }
    centroid = centroid / f32(end - start);
    let normal = ring_carried(id);
    let toward = ring_work(read_half * count + ring_neighbour(start)).xyz - here;
    let tangent = safe_normalize(toward - normal * dot(toward, normal));
    let other = cross(normal, tangent);
    let rest = ring_data[id];
    let wanted = centroid + normal * rest.x + tangent * rest.y + other * rest.z;
    let moved = here + (wanted - here) * (posed.w * RELAX_STEP);
    let slide = moved - posed.xyz;
    let far = length(slide);
    var placed = moved;
    if (far > RELAX_SLIDE) {
        placed = posed.xyz + slide * (RELAX_SLIDE / far);
    }
    ring_set_work(write_half * count + id, vec4<f32>(placed, 0.0));
}

@compute @workgroup_size(64)
fn cs_relax_even(@builtin(global_invocation_id) id: vec3<u32>) {
    ring_relax(id.x, 1u, 2u);
}

@compute @workgroup_size(64)
fn cs_relax_odd(@builtin(global_invocation_id) id: vec3<u32>) {
    ring_relax(id.x, 2u, 1u);
}

// each vertex back at its point's relaxed place (the passes end in the first half)
@compute @workgroup_size(64)
fn cs_relax_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= draw.deform.w) {
        return;
    }
    let count = ring_point_count();
    posed_out[(draw.deform.y + id.x) * 3u] = vec4<f32>(ring_work(count + ring_weld(id.x)).xyz, 1.0);
}

// pass B: each posed vertex pressed (`mesh::deform_by_contacts`), its tangent kept square to its
// new normal
@compute @workgroup_size(64)
fn cs_contact_deform(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= draw.deform.w) {
        return;
    }
    let at = (draw.deform.y + id.x) * 3u;
    var point = posed_out[at].xyz;
    var normal = posed_out[at + 1u].xyz;
    let tangent = posed_out[at + 2u];
    let softness = contact_softness(id.x);
    if (softness <= 0.0) {
        return;
    }
    let input = vertex_at(id.x);
    let count = min(contact_list.count.x, 64u);
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        // (a garment's contact pushes garments out, never her skin in, ADR-839)
        if (contact_skipped(slot, input) || contact_garment(slot)) {
            continue;
        }
        // (as far as skin can be squashed, never a raw depth: `mesh::deform_by_contacts`)
        let depth = min(bitcast<f32>(atomicLoad(&contact_depths[slot])), MAX_SQUASH);
        let reached = contact_distance(slot, point);
        // (stopping its standoff short of the surface: hair lying between, ADR-824)
        let measured = vec4<f32>(reached.xyz, reached.w - vertex_standoff(id.x, input));
        // (how much it acts here: on its `:within` side only, `mesh::within_weight`, ADR-816)
        let weight = contact_within(slot, point);
        if (weight <= 0.0) {
            continue;
        }
        let out = measured.xyz;
        let distance = measured.w;
        // a garment (ADR-839) is cloth, not flesh: inside a contact it is laid on the surface, no
        // eased band, inset or ring, so it stays parallel to her skin pressed a cover deeper by the
        // same contact along the same direction (a ringed cup and her plain skin crossed near a
        // finger's end, 61 [10cd1d], 2026-11-27)
        if ((draw.flags.y & 8192u) != 0u) {
            if (distance < 0.0) {
                point = point - out * distance * weight;
                normal = safe_normalize(normal + (-out - normal) * weight);
            }
            continue;
        }
        // (`mesh::deform_by_contacts`: the patch's edge eased over a band, a firm part sunk a
        // little behind the surface, the ring rising from nothing at the edge)
        let band = EDGE_BAND + EDGE_SPREAD * depth;
        let eased = smooth_outside(distance, band);
        let into = 1.0 - smoothstep(-band, band, distance);
        let inset = FIRM_INSET * depth * clamp(1.0 - softness / FIRM_SOFTNESS, 0.0, 1.0) * into;
        let moved = min(eased - distance - inset, MAX_SQUASH * softness) * weight;
        var lift = 0.0;
        var slope = 0.0;
        if (depth > 0.0 && distance > 0.0) {
            let width = (RING_WIDTH + RING_SPREAD * depth) * (0.5 + softness);
            let reach = distance / width;
            if (reach < 3.0) {
                let crest = RING_LIFT * softness * depth;
                let fall = exp(0.5 - reach * reach) * 1.41421356;
                lift = crest * reach * fall * weight;
                slope = crest / width * fall * (1.0 - 2.0 * reach * reach) * weight;
            }
        }
        let sideways = out - normal * dot(out, normal);
        point = point + out * moved + normal * lift;
        // (its `:drag`: wholly inside the patch, less through its edge and ring)
        let reach = (RING_WIDTH + RING_SPREAD * depth) * (0.5 + softness);
        let outside = max(distance, 0.0) / reach;
        let held = (into + (1.0 - into) * exp(-outside * outside)) * weight;
        point = point + contact_list.shapes[slot * 6u + 2u].xyz * held;
        let turned = safe_normalize(normal + (-out - normal) * into * weight);
        if (dot(sideways, sideways) > 1e-8) {
            normal = safe_normalize(turned - safe_normalize(sideways) * slope);
        } else {
            normal = turned;
        }
    }
    posed_out[at] = vec4<f32>(point, 1.0);
    posed_out[at + 1u] = vec4<f32>(normal, 0.0);
    posed_out[at + 2u] = vec4<f32>(safe_normalize(tangent.xyz - normal * dot(normal, tangent.xyz)), tangent.w);
}
// The press made unable to fold its skin (ADR-844). `cs_contact_deform` moves each vertex on its
// own, along the contact's direction by its own distance: where her skin curves into a fold (the top
// of her inner thigh into her groin) points on both sides of the curve were laid onto one strip of
// the contact's plane and the triangles between them turned over, and the fold cast the lamp's
// shadow as a dark crease. So the press is taken as a displacement a point (what it moved each
// point by, against where the relax left it), that field smoothed over the skin's rings
// `PRESS_SMOOTH_ITERATIONS` times (`PRESS_SMOOTH_STEP` of the way to its ring's mean a pass, the
// pressed area grown by a ring a pass, so its edge eases out too), and each vertex put back where
// the relax left its point plus its point's smoothed displacement. A field smoothed this way changes
// slowly from point to point, so neighbours on two sides of a curve can no longer land together.
const PRESS_SMOOTH_STEP: f32 = 0.5;

// each point's press: where the press put its vertex less where the relax left it (the work's first
// half), into the work's posed slots, `w` whether it moved
@compute @workgroup_size(64)
fn cs_press_gather(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = ring_point_count();
    if (id.x >= count) {
        return;
    }
    let before = ring_work(count + id.x).xyz;
    let after = ring_posed(id.x);
    let moved = after - before;
    ring_set_work(id.x, vec4<f32>(moved, select(0.0, 1.0, dot(moved, moved) > 1e-14)));
}

// one smoothing pass of the press field from one set of slots into another: a point in or beside
// the pressed area moved toward its ring's mean
fn press_smooth(id: u32, read_from: u32, write_to: u32) {
    let count = ring_point_count();
    if (id >= count) {
        return;
    }
    let here = ring_work(read_from + id);
    let start = ring_offset(id);
    let end = ring_offset(id + 1u);
    if (end <= start) {
        ring_set_work(write_to + id, here);
        return;
    }
    var mean = vec3<f32>(0.0);
    var near = here.w;
    for (var slot = start; slot < end; slot = slot + 1u) {
        let other = ring_work(read_from + ring_neighbour(slot));
        mean = mean + other.xyz;
        near = max(near, other.w);
    }
    mean = mean / f32(end - start);
    if (near <= 0.0) {
        ring_set_work(write_to + id, here);
        return;
    }
    ring_set_work(write_to + id, vec4<f32>(here.xyz + (mean - here.xyz) * PRESS_SMOOTH_STEP, near));
}

@compute @workgroup_size(64)
fn cs_press_smooth_out(@builtin(global_invocation_id) id: vec3<u32>) {
    press_smooth(id.x, 0u, 2u * ring_point_count());
}

@compute @workgroup_size(64)
fn cs_press_smooth_back(@builtin(global_invocation_id) id: vec3<u32>) {
    press_smooth(id.x, 2u * ring_point_count(), 0u);
}

// each vertex where the relax left its point plus its point's smoothed press (the passes end in the
// posed slots)
@compute @workgroup_size(64)
fn cs_press_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= draw.deform.w) {
        return;
    }
    let count = ring_point_count();
    let point = ring_weld(id.x);
    let at = (draw.deform.y + id.x) * 3u;
    posed_out[at] = vec4<f32>(ring_work(count + point).xyz + ring_work(point).xyz, 1.0);
}

// collider `index`'s centre this frame: its joints' palette blended by its weights, as a vertex
// of the skin it was fitted to is skinned (linear: a sphere moves rigidly with its patch), then the
// draw's model matrix; at rest where no joint of the palette moves it
fn collider_centre(index: u32) -> vec3<f32> {
    let at = draw.deform.w + 1u + index * 3u;
    let rest = mesh_softness[at];
    let joints = mesh_softness[at + 1u];
    let weights = mesh_softness[at + 2u];
    var skin = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var moved = false;
    if (draw.flags.w != 0u) {
        for (var slot = 0u; slot < 4u; slot = slot + 1u) {
            let joint = u32(joints[slot] + 0.5);
            if (weights[slot] > 0.0 && joint < draw.morph_info.z) {
                skin = skin + palette[draw.flags.z + joint] * weights[slot];
                moved = true;
            }
        }
    }
    var matrix = draw.model;
    if (moved) {
        matrix = draw.model * skin;
    }
    return (matrix * vec4<f32>(rest.xyz, 1.0)).xyz;
}

// ADR-839: the push-out net. A `:garment` draw's vertex found closer to one of the garment contacts
// (her skin's spheres and capsules) than `GARMENT_MARGIN` is moved out to it along the contact's
// own normal there, and counted: a safety net, never the fit, every use a fault the probes report
// (`gui/scene-push-outs`). The last contact step; any other draw returns at once
@compute @workgroup_size(64)
fn cs_garment_push_out(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= draw.deform.w || (draw.flags.y & 8192u) == 0u) {
        return;
    }
    let at = (draw.deform.y + id.x) * 3u;
    var point = posed_out[at].xyz;
    var pushed = false;
    let count = min(contact_list.count.x, 64u);
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        if (!contact_garment(slot)) {
            continue;
        }
        let weight = contact_within(slot, point);
        if (weight <= 0.0) {
            continue;
        }
        let reached = contact_distance(slot, point);
        if (reached.w < GARMENT_MARGIN) {
            point = point + reached.xyz * (GARMENT_MARGIN - reached.w) * weight;
            pushed = true;
        }
    }
    // and its own colliders, her skin's spheres near it posed this frame (`:push-colliders`)
    let colliders = min(u32(mesh_softness[draw.deform.w].x + 0.5), 1024u);
    for (var index = 0u; index < colliders; index = index + 1u) {
        let centre = collider_centre(index);
        let radius = mesh_softness[draw.deform.w + 1u + index * 3u].w;
        let offset = point - centre;
        let length = sqrt(dot(offset, offset));
        if (length < radius + GARMENT_MARGIN && length > 1e-9) {
            point = centre + offset / length * (radius + GARMENT_MARGIN);
            pushed = true;
        }
    }
    if (pushed) {
        posed_out[at] = vec4<f32>(point, 1.0);
        atomicAdd(&contact_depths[PUSH_OUT_COUNTER], 1u);
    }
}
@group(3) @binding(0) var base_texture: texture_2d<f32>;
@group(3) @binding(1) var normal_texture: texture_2d<f32>;
@group(3) @binding(2) var texture_sampler: sampler;
@group(3) @binding(3) var occlusion_texture: texture_2d<f32>;
// a material's `:detail` tile (ADR-797): a tangent-space normal's x and y in red and green
// about DETAIL_FLAT, the cavity in blue; a flat, open texel where the material has none
@group(3) @binding(4) var detail_texture: texture_2d<f32>;
// a skin's regions (ADR-826): roughness, thickness for the scatter, the detail tile's strength over two,
// oil for the coat, in its UVs (texture bit 2048)
@group(3) @binding(5) var regions_texture: texture_2d<f32>;
// this fragment's regions (ADR-826): its roughness (below 0: the material's), how much its scatter,
// detail tile and coat are scaled
var<private> region: vec4<f32> = vec4<f32>(-1.0, 1.0, 1.0, 1.0);

// the roughness a surface is lit with: a skin region's where its map gives one, else the material's
fn surface_roughness() -> f32 {
    if (region.x >= 0.0) {
        return region.x;
    }
    return draw.material.x;
}
// a skin's expression wrinkles (ADR-817): the compress and stretch maps and the zones' two masks
@group(3) @binding(13) var wrinkle_compress: texture_2d<f32>;
@group(3) @binding(14) var wrinkle_stretch: texture_2d<f32>;
@group(3) @binding(15) var wrinkle_mask_first: texture_2d<f32>;
@group(3) @binding(16) var wrinkle_mask_second: texture_2d<f32>;

// `mesh::wrinkle_tilt`: each zone's mask times its weight, summed apart for the compress map
// (weights above 0) and the stretch map (below), times each map's tilt
fn wrinkle_tilt(compress: vec4<f32>, stretch: vec4<f32>, first: vec4<f32>, second: vec4<f32>) -> vec2<f32> {
    let low = draw.wrinkles[0];
    let high = draw.wrinkles[1];
    let pushed = dot(first, max(low, vec4<f32>(0.0))) + dot(second, max(high, vec4<f32>(0.0)));
    let drawn = dot(first, max(-low, vec4<f32>(0.0))) + dot(second, max(-high, vec4<f32>(0.0)));
    return detail_tilt(compress) * pushed + detail_tilt(stretch) * drawn;
}

struct VertexIn {
    @builtin(vertex_index) index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
    @location(6) curvature: f32,
    // its centre of rotation (ADR-814): what it turns about when its draw's `flags.w` is 2
    @location(7) centre: vec3<f32>,
    // its second UV set (ADR-817)
    @location(8) uv2: vec2<f32>,
    // how much of its draw's lag it takes (ADR-823; the contact passes, which draw the lag)
    @location(9) loose_weight: f32,
    // how far short of a contact's surface it stops when pressed (ADR-824: hair lying between)
    @location(10) standoff: f32,
};
struct VertexOut {
    // invariant: the hair's depth pass and its front pass must put a sample at the same depth
    // to the bit, for the front pass's equal test (ADR-802)
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) curvature: f32,
    // an `:eye` surface's frame in the world (`mesh::EyeFrame`; zeros on any other surface):
    // its eye's centre and which eye it is, the eye's axis, its texture's up
    @location(5) eye_centre: vec4<f32>,
    @location(6) eye_axis: vec3<f32>,
    @location(7) eye_up: vec3<f32>,
    // a `:fibres` strand (ADR-813): its real width and its drawn width, in pixels, for its coverage
    @location(8) strand_px: vec2<f32>,
    // a `:fibres` strand's baked self-shadow (ADR-813): the hairs crossed toward L are max(0, x + yzw·L),
    // yzw turned into the world
    @location(9) visibility: vec4<f32>,
    // its second UV set (`mesh::Mesh::uvs2`, ADR-817): what `:wrinkles {:uv 1}` reads by
    @location(10) uv2: vec2<f32>,
    // its position in the mesh as it was made, before any morph, skinning or press: what a
    // garment's live stretch is measured against (`:show-stretch`, ADR-842)
    @location(11) made: vec3<f32>,
};

// the garment's stretch at this fragment (ADR-840, `:stretch-from-uv2`: its second UV set holds
// the cloth's stretch against its cut, weft and warp, 1 at rest); (1, 1) for any other draw. The
// set is a glTF UV map, whose v is the maker's 1 − v (glTF's origin at the image's top left:
// Blender's exporter writes every map so), so the warp is 1 − v (ADR-842; read as v it was about
// −0.03 on the bikini, womanblend-61 [ddb383])
fn fabric_stretch(fragment: VertexOut) -> vec2<f32> {
    if ((draw.flags.y & 16384u) == 0u) {
        return vec2<f32>(1.0);
    }
    return clamp(vec2<f32>(fragment.uv2.x, 1.0 - fragment.uv2.y), vec2<f32>(0.25), vec2<f32>(4.0));
}

// how much of the light a stretched knit stops here (ADR-840): its yarn keeps its area while the
// cloth's grows by weft × warp, so its cover is `:knit-cover` (at rest) over that, and through
// `:knit-layers` of it, 1 − (1 − cover)^layers. A blended garment's alpha, its lit skin drawn first
// showing where the loops open; 1 for any draw without the stretch or a knit cover
// (`DrawU::clamps[0]`: the cover, the layers)
fn knit_alpha(fragment: VertexOut) -> f32 {
    let knit = draw.clamps[0];
    if ((draw.flags.y & 16384u) == 0u || knit.x <= 0.0) {
        return 1.0;
    }
    let stretch = fabric_stretch(fragment);
    let cover = clamp(knit.x / (stretch.x * stretch.y), 0.0, 1.0);
    return 1.0 - pow(1.0 - cover, max(knit.y, 1.0));
}

// where the detail tile is read: its repeats over the UVs, and on a stretched garment opened
// along each axis by the stretch, so the knit's loops grow as the cloth is pulled (ADR-840)
fn detail_uv(fragment: VertexOut) -> vec2<f32> {
    return fragment.uv * draw.detail.xy / fabric_stretch(fragment);
}

// a garment's stretch shown (`:show-stretch`, texture bit 32768, ADR-842): its cloth's principal
// stretch now against its cut, as a colour, and the direction of the stronger one as fine strokes.
// Live: the pose's own stretch is measured in the cloth's plane from the screen's derivatives,
// F = ∂position/∂uv for the drawn position and for the made one (UVs in metres of fabric), the
// principal stretches the square roots of the eigenvalues of G_made⁻¹ G_drawn (G = FᵀF), times
// the stretch the cloth was fitted with (its second UV set, weft × warp, as one mean). The derivatives
// are taken by the caller in uniform control flow. Returns the albedo it is lit with.
fn stretch_shown(fragment: VertexOut, world_dx: vec3<f32>, world_dy: vec3<f32>, made_dx: vec3<f32>, made_dy: vec3<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec3<f32> {
    // ∂uv/∂screen inverted: the screen's step a unit of uv takes
    let turn = uv_dx.x * uv_dy.y - uv_dy.x * uv_dx.y;
    if (abs(turn) < 1e-14) {
        return vec3<f32>(0.5);
    }
    let du = vec2<f32>(uv_dy.y, -uv_dx.y) / turn;
    let dv = vec2<f32>(-uv_dy.x, uv_dx.x) / turn;
    let drawn_u = world_dx * du.x + world_dy * du.y;
    let drawn_v = world_dx * dv.x + world_dy * dv.y;
    let made_u = made_dx * du.x + made_dy * du.y;
    let made_v = made_dx * dv.x + made_dy * dv.y;
    let g_drawn = mat2x2<f32>(dot(drawn_u, drawn_u), dot(drawn_u, drawn_v), dot(drawn_u, drawn_v), dot(drawn_v, drawn_v));
    let made_uu = dot(made_u, made_u);
    let made_uv = dot(made_u, made_v);
    let made_vv = dot(made_v, made_v);
    let made_det = made_uu * made_vv - made_uv * made_uv;
    if (made_det < 1e-20) {
        return vec3<f32>(0.5);
    }
    let made_inverse = mat2x2<f32>(made_vv, -made_uv, -made_uv, made_uu) * (1.0 / made_det);
    let m = made_inverse * g_drawn;
    let half_trace = 0.5 * (m[0][0] + m[1][1]);
    let det = m[0][0] * m[1][1] - m[1][0] * m[0][1];
    let spread = sqrt(max(half_trace * half_trace - det, 0.0));
    let major = sqrt(max(half_trace + spread, 0.0));
    // its eigenvector: the direction in uv the cloth is pulled most
    var along = vec2<f32>(m[1][0], half_trace + spread - m[0][0]);
    if (dot(along, along) < 1e-12) {
        along = vec2<f32>(half_trace + spread - m[1][1], m[0][1]);
    }
    if (dot(along, along) < 1e-12) {
        along = vec2<f32>(1.0, 0.0);
    }
    along = normalize(along);
    let fitted = fabric_stretch(fragment);
    let stretch = major * sqrt(fitted.x * fitted.y);
    // slack blue (0.85) → rest white (1) → yellow (1.15) → red (1.3)
    let blue = vec3<f32>(0.08, 0.25, 0.9);
    let white = vec3<f32>(0.85);
    let yellow = vec3<f32>(0.95, 0.8, 0.1);
    let red = vec3<f32>(0.9, 0.08, 0.04);
    var colour = mix(blue, white, smoothstep(0.85, 1.0, stretch));
    colour = mix(colour, yellow, smoothstep(1.0, 1.15, stretch));
    colour = mix(colour, red, smoothstep(1.15, 1.3, stretch));
    // the strokes: lines along the pull, 6 mm apart across it, about a pixel wide, darker as
    // the pose pulls harder; none near rest
    if (major > 1.02 && abs(stretch - 1.0) > 0.03) {
        let across = vec2<f32>(-along.y, along.x);
        let stripe = dot(fragment.uv, across) / 0.006;
        let width = max(abs(dot(uv_dx, across)) + abs(dot(uv_dy, across)), 1e-6) / 0.006;
        let line = 1.0 - smoothstep(0.0, width, abs(fract(stripe + 0.5) - 0.5));
        let strength = clamp((major - 1.0) / 0.25, 0.25, 0.85);
        colour = colour * (1.0 - strength * line);
    }
    return colour;
}

// a vertex morphed, and the matrix that skins and places it
struct Posed {
    matrix: mat4x4<f32>,
    position: vec3<f32>,
    normal: vec3<f32>,
};

// `mesh::skin_matrix`: vertex `input`'s skinning matrix from the palette at `start` with
// `joint_count` joints, w 0 in its last column when no joint of the palette moves it. Its
// joints' matrices blended; or, for a draw whose vertices turn about their centres (`flags.w`
// 2, ADR-814) and a vertex on two joints or more, its joints' rotations blended (their
// quaternions, sign-aligned with the heaviest's, at `start + joint_count + joint`, then
// normalised) and their stretches, the vertex turned about its centre and that centre put where
// the blended matrices put it
fn skinning(input: VertexIn, start: u32, joint_count: u32) -> mat4x4<f32> {
    var skin = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var moving = 0u;
    var heaviest = 0u;
    var most = 0.0;
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        let joint = input.joints[slot];
        let weight = input.weights[slot];
        if (weight > 0.0 && joint < joint_count) {
            skin = skin + palette[start + joint] * weight;
            moving = moving + 1u;
            if (weight > most) {
                most = weight;
                heaviest = joint;
            }
        }
    }
    if (draw.flags.w != 2u || moving < 2u) {
        return skin;
    }
    let reference = palette[start + joint_count + heaviest][0];
    var quaternion = vec4<f32>(0.0);
    var stretch = mat3x3<f32>(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        let joint = input.joints[slot];
        let weight = input.weights[slot];
        if (weight > 0.0 && joint < joint_count) {
            let entry = palette[start + joint_count + joint];
            let q = entry[0];
            quaternion = quaternion + q * select(weight, -weight, dot(q, reference) < 0.0);
            stretch = stretch + mat3x3<f32>(entry[1].xyz, entry[2].xyz, entry[3].xyz) * weight;
        }
    }
    let length = sqrt(dot(quaternion, quaternion));
    if (length < 1e-8) {
        return skin;
    }
    let q = quaternion / length;
    let rotation = mat3x3<f32>(
        vec3<f32>(1.0 - 2.0 * (q.y * q.y + q.z * q.z), 2.0 * (q.x * q.y + q.z * q.w), 2.0 * (q.x * q.z - q.y * q.w)),
        vec3<f32>(2.0 * (q.x * q.y - q.z * q.w), 1.0 - 2.0 * (q.x * q.x + q.z * q.z), 2.0 * (q.y * q.z + q.x * q.w)),
        vec3<f32>(2.0 * (q.x * q.z + q.y * q.w), 2.0 * (q.y * q.z - q.x * q.w), 1.0 - 2.0 * (q.x * q.x + q.y * q.y)));
    let turned = rotation * stretch;
    let centre = (skin * vec4<f32>(input.centre, 1.0)).xyz - turned * input.centre;
    return mat4x4<f32>(vec4<f32>(turned[0], 0.0), vec4<f32>(turned[1], 0.0), vec4<f32>(turned[2], 0.0), vec4<f32>(centre, 1.0));
}

fn pose(input: VertexIn) -> Posed {
    var position = input.position;
    var normal = input.normal;
    if (draw.morph_info.x != 0u) {
        let at = (draw.morph_info.w + input.index) * 2u;
        position = position + morphs[at].xyz;
        normal = normal + morphs[at + 1u].xyz;
    }
    var matrix = draw.model;
    if (draw.flags.w != 0u) {
        // as `skin_matrix` skins: a joint past the palette's end is skipped, and a vertex no
        // joint of the palette moves stays rigid
        let skin = skinning(input, draw.flags.z, draw.morph_info.z);
        if (skin[3].w != 0.0) {
            matrix = draw.model * skin;
        }
    }
    return Posed(matrix, position, normal);
}

@vertex
fn vs_mesh(input: VertexIn) -> VertexOut {
    // a pressed draw (ADR-805): its vertex as the contact passes posed and pressed it
    if (draw.deform.x == 1u) {
        let at = (draw.deform.y + input.index) * 3u;
        var pressed: VertexOut;
        pressed.world = posed_vertices[at].xyz;
        pressed.clip = scene.view_proj * vec4<f32>(pressed.world, 1.0);
        pressed.normal = posed_vertices[at + 1u].xyz;
        pressed.uv = input.uv;
        pressed.uv2 = input.uv2;
        pressed.made = input.position;
        pressed.tangent = posed_vertices[at + 2u];
        pressed.curvature = input.curvature;
        pressed.eye_centre = vec4<f32>(0.0);
        pressed.eye_axis = vec3<f32>(0.0);
        pressed.eye_up = vec3<f32>(0.0);
        return pressed;
    }
    let posed = pose(input);
    let world = posed.matrix * vec4<f32>(posed.position, 1.0);
    var out: VertexOut;
    out.clip = scene.view_proj * world;
    out.world = world.xyz;
    out.normal = (posed.matrix * vec4<f32>(posed.normal, 0.0)).xyz;
    out.uv = input.uv;
    out.uv2 = input.uv2;
    out.made = input.position;
    out.tangent = vec4<f32>((posed.matrix * vec4<f32>(input.tangent.xyz, 0.0)).xyz, input.tangent.w);
    out.curvature = input.curvature;
    out.eye_centre = vec4<f32>(0.0);
    out.eye_axis = vec3<f32>(0.0);
    out.eye_up = vec3<f32>(0.0);
    out.strand_px = vec2<f32>(0.0);
    out.visibility = vec4<f32>(0.0);
    if (draw.flags.x == 6u && draw.eye_centres[0].w > 0.0) {
        // `mesh::eye_frame`: the eye whose centre at rest is nearest, moved as this vertex is
        var index = 0u;
        let first = posed.position - draw.eye_centres[0].xyz;
        let second = posed.position - draw.eye_centres[1].xyz;
        if (draw.eye_centres[0].w > 1.0 && dot(second, second) < dot(first, first)) {
            index = 1u;
        }
        out.eye_centre = vec4<f32>((posed.matrix * vec4<f32>(draw.eye_centres[index].xyz, 1.0)).xyz, f32(index));
        out.eye_axis = (posed.matrix * vec4<f32>(draw.eye_axes[index].xyz, 0.0)).xyz;
        out.eye_up = (posed.matrix * vec4<f32>(draw.eye_up.xyz, 0.0)).xyz;
    }
    if (draw.flags.x == 5u) {
        // a ribbon turned to face the camera: its tangent the strand's direction (the mesh's
        // normal), its fourth the coverage
        let clamped = clamped_strand(world.xyz, safe_normalize(out.normal));
        let direction = clamped.direction;
        let ribbon = strand_vertex(clamped.centre, direction, input.uv, draw.strand.xy);
        out.clip = scene.view_proj * vec4<f32>(ribbon.world, 1.0);
        out.world = ribbon.world;
        out.normal = ribbon.normal;
        out.tangent = vec4<f32>(direction, ribbon.coverage);
        out.strand_px = ribbon.pixels;
        // (its tangent slot carries its baked self-shadow, ADR-813: a strand's direction is its normal)
        out.visibility = vec4<f32>(input.tangent.x, (posed.matrix * vec4<f32>(input.tangent.yzw, 0.0)).xyz);
    }
    return out;
}

// `mesh::strand_sample_step`: the coverage a strand's fragment at `world` needs at `sample`
// the samples a pixel the scene is drawn with: 8 where the adapter can, else 4 (ADR-799;
// `scenes::scene_samples` sets it when the pipeline is made)
override SAMPLES: u32 = 4u;
// a trial of what hair costs (`BROOD_GUI_HAIR_TRIAL`): 0 drawn as ever, 1 its coverage
// alone (lit by nothing: its base colour), 2 with no hair-on-hair shadow
override HAIR_TRIAL: u32 = 0u;
// a `:fibres` fragment's strand value and baked self-shadow, set by `mesh_lit` for `shade`
var<private> fibre_strand: f32 = 0.0;
var<private> fibre_visibility: vec4<f32> = vec4<f32>(0.0);

// a `:hair` surface's coverage step at `sample`, as `mesh::HAIR_COVERAGE_STEPS` holds them:
// eight, (k + 0.5) / 8 in an order that spreads them; with four samples the four of them that
// cover a pixel's steps evenly, as the CPU's alternate halves average to
fn hair_step(sample: u32) -> f32 {
    var eight = array<f32, 8>(0.0625, 0.5625, 0.8125, 0.3125, 0.1875, 0.6875, 0.9375, 0.4375);
    var four = array<f32, 4>(0.125, 0.625, 0.875, 0.375);
    if (SAMPLES >= 8u) {
        return eight[sample & 7u];
    }
    return four[sample & 3u];
}

fn strand_sample_step(world: vec3<f32>, sample: u32) -> f32 {
    let seed = dot(world, vec3<f32>(12.9898, 78.233, 37.719)) * 1000.0 + f32(sample) * 7.31;
    return fract(sin(seed) * 43758.547);
}

struct StrandVertex {
    world: vec3<f32>,
    normal: vec3<f32>,
    coverage: f32,
    // its real width and its drawn width in pixels (a `:fibres` strand reads them, ADR-813)
    pixels: vec2<f32>,
};

// `mesh::strand_vertex`: a strand's point on its ribbon's side `uv.x`, the ribbon `widths`
// (root, tip; `uv.y` 1 at the root) wide but never under a pixel, turned to face the camera
const CLAMP_BAND: f32 = 0.008;
const CLAMP_HOLD: f32 = 0.0005;

struct ClampedStrand {
    centre: vec3<f32>,
    direction: vec3<f32>,
};

// `mesh::clamped_strand`: a strand's point and direction laid against the draw's `:clamp-planes`
// (ADR-813), eased within `CLAMP_BAND` of each plane to no nearer than `CLAMP_HOLD`
fn clamped_strand(centre_given: vec3<f32>, direction_given: vec3<f32>) -> ClampedStrand {
    var centre = centre_given;
    var direction = direction_given;
    let reach = CLAMP_BAND - CLAMP_HOLD;
    let count = min(u32(draw.clamp_info.x + 0.5), 4u);
    for (var index = 0u; index < count; index = index + 1u) {
        let plane = draw.clamps[index];
        let distance = dot(plane.xyz, centre) - plane.w;
        if (distance < CLAMP_BAND) {
            let slope = exp((distance - CLAMP_BAND) / reach);
            let eased = CLAMP_HOLD + reach * slope;
            let across = dot(direction, plane.xyz);
            centre = centre + plane.xyz * (eased - distance);
            let bent = direction + plane.xyz * (slope - 1.0) * across;
            // (straight into the plane: nothing left along it, so it keeps its own direction)
            if (dot(bent, bent) >= 1e-8) {
                direction = normalize(bent);
            }
        }
    }
    return ClampedStrand(centre, direction);
}

fn strand_vertex(centre: vec3<f32>, direction: vec3<f32>, uv: vec2<f32>, widths: vec2<f32>) -> StrandVertex {
    let view = safe_normalize(scene.eye.xyz - centre);
    // seen end-on it narrows, never under `mesh::STRAND_END_ON` of its width
    let along = dot(direction, view);
    let width = (widths.y + (widths.x - widths.y) * clamp(uv.y, 0.0, 1.0))
        * max(sqrt(max(1.0 - along * along, 0.0)), 0.3);
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
    var drawn = max(width, pixel);
    // a `:fibres` strand (ADR-813) is drawn at its real width and 0.7 of a pixel more each side: room for
    // its exact coverage, worked out in the fragment stage
    if ((draw.flags.y & 512u) != 0u) {
        drawn = width + 1.4 * pixel;
    }
    var coverage = 0.0;
    if (drawn > 0.0) {
        coverage = clamp(width / drawn, 0.0, 1.0);
    }
    let side = select(-1.0, 1.0, uv.x >= 0.0);
    var out: StrandVertex;
    out.world = centre + across * (drawn * 0.5 * side);
    out.normal = safe_normalize(across * side + facing);
    out.coverage = coverage;
    out.pixels = vec2<f32>(width / max(pixel, 1e-12), drawn / max(pixel, 1e-12));
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

// a comparison gather of the shadow map `map` (a view's `map`, ADR-800) names, at `coords` in
// that array's own unit square; `map_load` one of its texels. Gathers and loads have no implicit
// derivatives, so choosing the array in non-uniform control flow is sound.
fn map_gather(map: vec4<f32>, coords: vec2<f32>, depth: f32) -> vec4<f32> {
    let layer = i32(map.z);
    if (map.y > 0.5) {
        return textureGatherCompare(smaller_shadow_maps, shadow_sampler, coords, layer, depth);
    }
    return textureGatherCompare(shadow_maps, shadow_sampler, coords, layer, depth);
}

fn map_load(map: vec4<f32>, texel: vec2<i32>) -> f32 {
    let layer = i32(map.z);
    if (map.y > 0.5) {
        return textureLoad(smaller_shadow_maps, texel, layer, 0);
    }
    return textureLoad(shadow_maps, texel, layer, 0);
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
    let map_side = view.map.x;
    var lit = 0.0;
    for (var block_y = 0u; block_y < 2u; block_y = block_y + 1u) {
        for (var block_x = 0u; block_x < 2u; block_x = block_x + 1u) {
            // the block whose top-left texel is (corner − 1 + 2 block): gathered where its four
            // texels meet, so no rounding picks a neighbour
            let meet = corner + vec2<f32>(f32(2u * block_x), f32(2u * block_y));
            let gathered = map_gather(view.map, meet / map_side, depth);
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
// clamped to it, off its right or bottom edge reading the layer (cleared to the far plane) up
// to its array's side
fn stored_depth(view_index: u32, texel: vec2<f32>) -> f32 {
    let map = scene.shadows[view_index].map;
    let clamped = clamp(texel, vec2<f32>(0.0), vec2<f32>(map.x - 1.0));
    return map_load(map, vec2<i32>(clamped));
}

// the CPU's `bilinear_tap`: a receiver at `depth` compared with the four texels about `at`
// (texel centres at whole numbers here), weighted by nearness — one gather where they meet
fn bilinear_tap(view_index: u32, at: vec2<f32>, depth: f32) -> f32 {
    let corner = floor(at);
    let fraction = at - corner;
    let map = scene.shadows[view_index].map;
    let gathered = map_gather(map, (corner + vec2<f32>(1.0)) / map.x, depth);
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

// `mesh::hair_shadow`: the light left for a hair at `world` after its way through the hair
// before it: how far it lies behind the map's first surface, a mean of HAIR_SHADOW_TAPS (16)
// taps of a disc HAIR_SHADOW_TEXELS (6) texels round it, absorbed at HAIR_ABSORPTION (40) a metre
fn hair_shadow(light: Light, world: vec3<f32>, turn: f32) -> f32 {
    if (HAIR_TRIAL == 2u) {
        return 1.0;
    }
    var at = light.shadow.y;
    if (light.shadow.z == 6u) {
        at = at + cube_face(world - light.position.xyz);
    }
    let view = scene.shadows[at];
    let projected = project_into(view, world);
    if (!projected.inside) {
        return 1.0;
    }
    let perspective = light.shadow.x != 0u;
    let receiver = view_distance(view, perspective, projected.depth);
    var way = 0.0;
    for (var tap = 0u; tap < 16u; tap = tap + 1u) {
        let offset = disc_tap(tap * 2u, turn) * 6.0;
        let stored = stored_depth(at, floor(projected.at + offset));
        way = way + max(receiver - view_distance(view, perspective, stored), 0.0);
    }
    return exp(-draw.fibre_cut.z * way / 16.0);
}

// the shadow view of `light` that `world` is in: its own, or a cube's face
fn light_view(light: Light, world: vec3<f32>) -> u32 {
    if (light.shadow.z == 6u) {
        return light.shadow.y + cube_face(world - light.position.xyz);
    }
    return light.shadow.y;
}

// ADR-827: the hair's density between view `view_index`'s light and a point `distance` along it,
// at a texel of its opacity maps: none before the nearest hair, each slice's sum at its end, and
// straight between (a slice's hair taken as spread evenly through it)
fn hair_density_at(view_index: u32, texel: vec2<i32>, view: ShadowView, perspective: bool, distance: f32) -> f32 {
    let nearest = textureLoad(hair_nearest, texel, i32(view_index), 0);
    let beyond = distance - view_distance(view, perspective, nearest);
    if (beyond <= 0.0) {
        return 0.0;
    }
    let sums = textureLoad(hair_opacity, texel, i32(view_index), 0);
    var start = 0.0;
    var before = 0.0;
    for (var slice = 0u; slice < 4u; slice = slice + 1u) {
        let end = HAIR_SLICES[slice];
        if (beyond < end) {
            return mix(before, sums[slice], (beyond - start) / (end - start));
        }
        start = end;
        before = sums[slice];
    }
    return sums.w;
}

// ADR-827: how much of `light` the hair lets through to `world` (in view `view_index`): `exp(-density)`
// averaged over the light's penumbra there, as `soft_shadow` widens an opaque one: the gap from
// the nearest hair to the point times the light's size (over the hair's distance, for a
// perspective view), 16 taps of a disc that wide; read bilinearly from the four texels about it
// where that is under a texel (or the light has no size). A lock lifted off her forehead stamped
// a hard-edged blotch on it under the key, a light 55 cm wide
fn hair_transmittance(light: Light, view_index: u32, world: vec3<f32>, turn: f32) -> f32 {
    let view = scene.shadows[view_index];
    let projected = project_into(view, world);
    if (!projected.inside) {
        return 1.0;
    }
    let perspective = view.map.w > 0.5;
    let distance = view_distance(view, perspective, projected.depth);
    let at = projected.at * (HAIR_OPACITY_SIDE / max(view.info.y, 1.0)) - vec2<f32>(0.5);
    // (outside where its hair can be, no texel loaded: most of what a light falls on)
    if (at.x < view.hair.x || at.y < view.hair.y || at.x > view.hair.z || at.y > view.hair.w) {
        return 1.0;
    }
    let last = vec2<i32>(i32(HAIR_OPACITY_SIDE) - 1);
    // the penumbra's radius in the opacity map's texels (each `HAIR_OPACITY_SIDE / resolution` of
    // the view's), from the mean distance of the hair before the point over the widest penumbra
    // (16 texels), as `soft_shadow` finds its blockers: the one texel under the point alone left
    // a lit square where its own hair lay behind the skin
    var radius = 0.0;
    let size = light.direction.w;
    var nearest = 0.0;
    var found = 0u;
    if (size > 0.0) {
        for (var tap = 0u; tap < 16u; tap = tap + 1u) {
            let offset = disc_tap(tap * 2u + 1u, turn) * 16.0;
            let texel_at = clamp(vec2<i32>(floor(at + vec2<f32>(0.5) + offset)), vec2<i32>(0), last);
            let hair_at = view_distance(view, perspective, textureLoad(hair_nearest, texel_at, i32(view_index), 0));
            if (hair_at < distance) {
                nearest = nearest + hair_at;
                found = found + 1u;
            }
        }
        if (found == 0u) {
            return 1.0;
        }
        nearest = nearest / f32(found);
    }
    if (size > 0.0 && distance > nearest) {
        var penumbra = size * (distance - nearest);
        var texel = view.info.x * max(view.info.y, 1.0) / HAIR_OPACITY_SIDE;
        if (perspective) {
            penumbra = penumbra / max(nearest, 1e-4);
            texel = texel * distance;
        }
        radius = min(penumbra * 0.5 / max(texel, 1e-9), 16.0);
    }
    if (radius > 1.0) {
        var through = 0.0;
        for (var tap = 0u; tap < 16u; tap = tap + 1u) {
            let offset = disc_tap(tap * 2u, turn) * radius;
            let texel_at = clamp(vec2<i32>(floor(at + vec2<f32>(0.5) + offset)), vec2<i32>(0), last);
            through = through + exp(-hair_density_at(view_index, texel_at, view, perspective, distance));
        }
        return through / 16.0;
    }
    let corner = floor(at);
    let fraction = at - corner;
    let base = vec2<i32>(corner);
    let top_left = hair_density_at(view_index, clamp(base, vec2<i32>(0), last), view, perspective, distance);
    let top_right = hair_density_at(view_index, clamp(base + vec2<i32>(1, 0), vec2<i32>(0), last), view, perspective, distance);
    let bottom_left = hair_density_at(view_index, clamp(base + vec2<i32>(0, 1), vec2<i32>(0), last), view, perspective, distance);
    let bottom_right = hair_density_at(view_index, clamp(base + vec2<i32>(1, 1), vec2<i32>(0), last), view, perspective, distance);
    let density = mix(mix(top_left, top_right, fraction.x), mix(bottom_left, bottom_right, fraction.x), fraction.y);
    return exp(-density);
}

// `mesh::hair_light`: that light channel by channel, taking the hair's colour as it passes
// (HAIR_TINT 0.45)
fn hair_light(seen: f32, base: vec3<f32>) -> vec3<f32> {
    let most = max(max(base.r, base.g), max(base.b, 1e-3));
    return pow(vec3<f32>(max(seen, 0.0)), vec3<f32>(1.0) - 0.45 * base / most);
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

// `mesh::lobe_exponent`: `shininess` with no ceiling of 4096, the roughness held to 0.04 … 1
fn lobe_exponent(roughness: f32) -> f32 {
    let clamped = clamp(roughness, 0.04, 1.0);
    return max(2.0 / (clamped * clamped * clamped * clamped) - 2.0, 1.0);
}

// `mesh::highlight_exponent`: past 4096 only as sharp as a pixel resolves where the
// reflection turns `turn` radians across half of one
fn highlight_exponent(roughness: f32, turn: f32) -> f32 {
    let exponent = lobe_exponent(roughness);
    if (exponent <= 4096.0 || turn <= 0.0) {
        return exponent;
    }
    let resolved = 2.0 * 3.14159265 / (turn * turn) - 8.0;
    return min(exponent, max(resolved, 4096.0));
}

// `mesh::lobe_spread`: half the side of the box a lobe of solid angle 8π / (n + 8) is
fn lobe_spread(exponent: f32, turn: f32) -> f32 {
    return max(sqrt(8.0 * 3.14159265 / (exponent + 8.0)) * 0.5, turn);
}

// `mesh::pixel_span`: the metres a picture pixel spans at `world`
fn pixel_span(world: vec3<f32>, view: vec3<f32>) -> f32 {
    let across = right_of(view);
    let near = scene.view_proj * vec4<f32>(world, 1.0);
    let far = scene.view_proj * vec4<f32>(world + across * 0.01, 1.0);
    if (near.w > 1e-6 && far.w > 1e-6) {
        let moved = (far.xy / far.w - near.xy / near.w) * scene.picture.xy * 0.5;
        let pixels = length(moved);
        if (pixels > 1e-6) {
            return 0.01 / pixels;
        }
    }
    return 0.0;
}

// one Blinn-Phong lobe normalised to (n + 8) / 8, the BRDF's 1/π folded into the light
fn specular_lobe(roughness: f32, normal_dot_half: f32, turn: f32) -> f32 {
    let exponent = highlight_exponent(roughness, turn);
    return (exponent + 8.0) / 8.0 * pow(max(normal_dot_half, 0.0), exponent);
}

// the CPU's `specular_lobes`
fn specular_lobes(normal_dot_half: f32, turn: f32) -> f32 {
    let first = specular_lobe(surface_roughness() * draw.lobes.x, normal_dot_half, turn);
    if (draw.lobes.z >= 1.0) {
        return first;
    }
    return draw.lobes.z * first + (1.0 - draw.lobes.z) * specular_lobe(surface_roughness() * draw.lobes.y, normal_dot_half, turn);
}

// `mesh::sh_radiance`: the scene's nine coefficients along `direction`, band l scaled by
// exp(−2 l² / n), held at 0
fn sh_radiance(direction: vec3<f32>, exponent: f32) -> vec3<f32> {
    let x = direction.x;
    let y = direction.y;
    let z = direction.z;
    let first = exp(-2.0 / exponent);
    let second = exp(-8.0 / exponent);
    let sh = scene.ambient_sh;
    let radiance = 0.282095 * sh[0].rgb
        + 0.488603 * first * (y * sh[1].rgb + z * sh[2].rgb + x * sh[3].rgb)
        + 1.092548 * second * (x * y * sh[4].rgb + y * z * sh[5].rgb + x * z * sh[7].rgb)
        + 0.315392 * (3.0 * z * z - 1.0) * second * sh[6].rgb
        + 0.546274 * (x * x - y * y) * second * sh[8].rgb;
    return max(radiance, vec3<f32>(0.0));
}

// `mesh::ambient_radiance`
fn ambient_radiance(direction: vec3<f32>, exponent: f32) -> vec3<f32> {
    if (scene.params.w > 0.5) {
        return sh_radiance(direction, exponent);
    }
    return mix(scene.ground.rgb, scene.sky.rgb, direction.y * exp(-2.0 / exponent) * 0.5 + 0.5);
}

// `mesh::environment_brdf`: Karis's EnvBRDFApprox, (scale, bias)
fn environment_brdf(roughness: f32, normal_dot_view: f32) -> vec2<f32> {
    let rough = clamp(roughness, 0.0, 1.0);
    let curve = vec4<f32>(1.0 - rough, 0.0425 - 0.0275 * rough, 1.04 - 0.572 * rough, 0.022 * rough - 0.04);
    let grazing = min(curve.x * curve.x, exp2(-9.28 * normal_dot_view)) * curve.x + curve.y;
    // held to 0 … 1 together, as the CPU holds them
    let scale = max(curve.z - 1.04 * grazing, 0.0);
    return vec2<f32>(scale, clamp(curve.w + 1.04 * grazing, 0.0, 1.0 - scale));
}

// `mesh::specular_occlusion`: Lagarde's
fn specular_occlusion(normal_dot_view: f32, occlusion: f32, roughness: f32) -> f32 {
    let power = exp2(-16.0 * clamp(roughness, 0.0, 1.0) - 1.0);
    return clamp(pow(max(normal_dot_view + occlusion, 0.0), power) - 1.0 + occlusion, 0.0, 1.0);
}

// `mesh::horizon`
fn horizon(reflected: vec3<f32>, surface_normal: vec3<f32>) -> f32 {
    let above = clamp(1.0 + dot(reflected, surface_normal), 0.0, 1.0);
    return above * above;
}

// `mesh::overlap`
fn overlap(offset: f32, half: f32, reach: f32) -> f32 {
    return clamp((min(offset + reach, half) - max(offset - reach, -half)) / (2.0 * reach), 0.0, 1.0);
}

// `mesh::emitter_coverage`: how much of a lobe leaving `world` along `reflected` lands on it
fn emitter_coverage(emitter: Emitter, world: vec3<f32>, reflected: vec3<f32>, spread: f32) -> f32 {
    let shape = u32(emitter.centre.w);
    var centre = emitter.centre.xyz;
    var facing = emitter.facing.xyz;
    if (shape == 3u) {
        facing = safe_normalize(world - centre);
    } else if (shape == 4u) {
        centre = world - facing;
    }
    let toward = -dot(reflected, facing);
    let height = dot(world - centre, facing);
    if (!(toward > 1e-4 && height > 0.0)) {
        return 0.0;
    }
    let distance = height / toward;
    let reach = max(distance * spread / sqrt(toward), 1e-9);
    let hit = world + reflected * distance - centre;
    if (shape == 2u) {
        let up = cross(facing, emitter.right.xyz);
        return overlap(dot(hit, emitter.right.xyz), emitter.right.w, reach)
            * overlap(dot(hit, up), emitter.radiance.w, reach);
    }
    let radius = emitter.right.w;
    let edge = clamp((radius + reach - length(hit)) / (2.0 * min(radius, reach)), 0.0, 1.0);
    return edge * min(3.14159265 * radius * radius / (4.0 * reach * reach), 1.0);
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
// how much of what lies behind a knit on an opaque garment shows through it here (ADR-842): its
// opened cover's open share (`knit_alpha`), only where the camera's view beneath the cloth
// (`scene.params.z`, one past its index) finds a surface within `:knit-contact`
// (`DrawU::clamps[0].z`) of it, fading to none there: only skin the cloth lies on shows, never
// what a panel stands off. The gap is taken square to the cloth (the view's along the eye's ray,
// times how squarely the cloth faces it). 0 for any other draw, or with no such view
fn knit_through(fragment: VertexOut, surface_normal: vec3<f32>) -> f32 {
    let knit = draw.clamps[0];
    let at = u32(scene.params.z);
    if ((draw.flags.y & 16384u) == 0u || knit.x <= 0.0 || at == 0u || draw.flags.x == 3u) {
        return 0.0;
    }
    let view = scene.shadows[at - 1u];
    let clip = view.view_proj * vec4<f32>(fragment.world, 1.0);
    if (clip.w <= 1e-6) {
        return 0.0;
    }
    let across_unit = clip.x / clip.w * 0.5 + 0.5;
    let down_unit = 0.5 - clip.y / clip.w * 0.5;
    if (across_unit < 0.0 || across_unit >= 1.0 || down_unit < 0.0 || down_unit >= 1.0) {
        return 0.0;
    }
    // (the least gap of the 2×2 texels round the point, so the contact's edge fades smoothly as
    // she moves rather than stepping texel by texel)
    let side = i32(view.info.y);
    let corner = vec2<i32>(floor(vec2<f32>(across_unit, down_unit) * f32(side) - vec2<f32>(0.5)));
    var nearest = 1.0;
    for (var tap = 0; tap < 4; tap = tap + 1) {
        let texel = clamp(corner + vec2<i32>(tap % 2, tap / 2), vec2<i32>(0), vec2<i32>(side - 1));
        nearest = min(nearest, map_load(view.map, texel));
    }
    let beneath = camera_distance(view.view_proj, nearest);
    let own = camera_distance(view.view_proj, clip.z / clip.w);
    let to_eye = safe_normalize(scene.eye.xyz - fragment.world);
    let facing = clamp(abs(dot(surface_normal, to_eye)), 0.05, 1.0);
    let gap = max(beneath - own, 0.0) * facing;
    let contact = 1.0 - smoothstep(0.0, max(knit.z, 1e-4), gap);
    return (1.0 - knit_alpha(fragment)) * contact;
}

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
        let stored = camera_distance(view.view_proj, map_load(view.map, vec2<i32>(i32(column), i32(row))));
        let reached = camera_distance(view.view_proj, clip.z / clip.w);
        if (stored < reached - radius * 0.05) {
            occluded = occluded + min(radius / max(abs(own_distance - stored), 1e-6), 1.0);
        }
    }
    return clamp(1.0 - scene.occlusion.y * occluded / 16.0, 0.0, 1.0);
}

// contact shadows (ADR-818, the GPU painter only): how much of a shadowed light reaches `world` past what lies
// within `CONTACT_REACH` of it toward the light, as the camera's depth view holds it
const CONTACT_REACH: f32 = 0.03;
const CONTACT_STEPS: u32 = 12u;
const CONTACT_THICKNESS: f32 = 0.03;
// how deep a `:contact-caster` is taken to be behind its front surface (ADR-837 amended): cloth
// a few millimetres thick (her bikini's hems 3.2 mm round, its cups lined), so a ray toward a
// light that passes behind a hem seen edge-on, not through it, is not blocked (with the 3 cm the
// camera's view assumes, every edge seen side-on drew a near-black line on the skin beside it)
const CASTER_THICKNESS: f32 = 0.008;
// how far along its reflected direction a surface looks for what hides the room from it
const REFLECTION_REACH: f32 = 0.04;
fn contact_shadow(light: Light, world: vec3<f32>, to_light: vec3<f32>, surface_normal: vec3<f32>, to_eye: vec3<f32>) -> f32 {
    return screen_blocked_in(u32(scene.occlusion.z), world, to_light, CONTACT_REACH, surface_normal, to_eye, light_spread(light, world), CONTACT_THICKNESS);
}
// on skin (ADR-837): the same march, through the depth of the `:contact-caster` draws alone
// (cloth), so what lies on her shades her and her own creases, never in that view, do not
fn contact_shadow_on_skin(light: Light, world: vec3<f32>, to_light: vec3<f32>, surface_normal: vec3<f32>, to_eye: vec3<f32>) -> f32 {
    // (most of her skin has no cloth near it: the march, 12 steps toward each light, only where
    // the casters' pyramid holds a caster within reach, else she paid 2.5 ms at her bust for nothing)
    if (!caster_within_reach(u32(scene.occlusion.w), world)) {
        return 1.0;
    }
    return screen_blocked_in(u32(scene.occlusion.w), world, to_light, CONTACT_REACH, surface_normal, to_eye, light_spread(light, world), CASTER_THICKNESS);
}
// whether any caster lies within `CONTACT_REACH` of `world` as the casters' view (one before `at`)
// sees it, in any direction, so for every light at once: the casters' pyramid at the level whose
// texel spans the reach in that view's pixels there, the 3×3 texels round the point's (a disc of
// the reach then lies inside them), any nearer than the far plane (ADR-837). It cannot
// miss: what blocks a light from `world` within the reach lies inside that disc. (Probes along
// each light's way missed up to 94 % of the shade: a hem is crossed in the march's first
// millimetres.) Where the view cannot say, it answers yes, and the march decides.
fn caster_within_reach(at: u32, world: vec3<f32>) -> bool {
    if (at == 0u) {
        return false;
    }
    let view = scene.shadows[at - 1u];
    let side = f32(view.info.y);
    let clip = view.view_proj * vec4<f32>(world, 1.0);
    if (clip.w <= 1e-6) {
        return true;
    }
    let at_texel = vec2<f32>(clip.x / clip.w * 0.5 + 0.5, 0.5 - clip.y / clip.w * 0.5) * side;
    // the reach in the view's pixels at this depth, a quarter more for a point of the march
    // nearer the camera than `world`, and a pixel for the texel the march rounds to
    let row_scale = length(vec3<f32>(view.view_proj[0].x, view.view_proj[1].x, view.view_proj[2].x));
    let reach = 1.25 * CONTACT_REACH * side * max(row_scale, 1e-6) / (2.0 * clip.w) + 1.0;
    let levels = i32(textureNumLevels(caster_pyramid));
    let level = clamp(i32(ceil(log2(reach))), 0, levels - 1);
    let size = vec2<i32>(textureDimensions(caster_pyramid, level));
    let corner = vec2<i32>(floor(at_texel / f32(1 << u32(level)))) - vec2<i32>(1);
    for (var row = 0; row < 3; row = row + 1) {
        for (var column = 0; column < 3; column = column + 1) {
            let texel = clamp(corner + vec2<i32>(column, row), vec2<i32>(0), size - vec2<i32>(1));
            if (textureLoad(caster_pyramid, texel, level).r < 1.0) {
                return true;
            }
        }
    }
    return false;
}
// a light's unshadowed share of a skin point's light (ADR-837): what reaches it, by its colour
// and the skin's own response to it (`skin_diffuse`, which still lights past the terminator, so
// not a bare cosine: cut at n·l ≤ 0 the shading moved 60 levels). The contact march runs only for
// a light whose share is at least `CONTACT_SHARE` of the brightest light's there: a dimmer light's
// contact shade changes the pixel by at most that share of it (2 %: within two levels of her skin)
const CONTACT_SHARE: f32 = 0.02;
fn skin_share(light: Light, arrival: Arrival, diffuse_normal: vec3<f32>, curvature: f32) -> f32 {
    let response = light.color.rgb * skin_diffuse(dot(diffuse_normal, arrival.to_light), curvature);
    return arrival.amount * dot(response, vec3<f32>(0.2126, 0.7152, 0.0722));
}
// how wide a light is seen from `world`: the tangent of half the angle it spans (ADR-837
// amended): a point light 0 (its contact shadow hard), her 1.3 m key two metres off about 0.33
fn light_spread(light: Light, world: vec3<f32>) -> f32 {
    let size = light.direction.w;
    if (size <= 0.0) {
        return 0.0;
    }
    if (light.shadow.x == 0u) {
        return tan(min(size * 0.5, 1.5));
    }
    return size * 0.5 / max(length(light.position.xyz - world), 0.05);
}

// how open the way from `world` along `direction` is, over `reach` metres, as the camera's depth
// view holds what lies there (ADR-818): 1 open, toward 0 where something within a few centimetres
// stands in the way (the contact shadow toward a light; toward the reflected direction, the
// room's reflection hidden by her own cheeks and lips in her mouth)
fn screen_blocked(world: vec3<f32>, to_light: vec3<f32>, reach: f32, surface_normal: vec3<f32>, to_eye: vec3<f32>) -> f32 {
    return screen_blocked_in(u32(scene.occlusion.z), world, to_light, reach, surface_normal, to_eye, 0.0, CONTACT_THICKNESS);
}
// (through the view one before `at`: the camera's depth, or its casters' alone, ADR-837)
// `spread`: the light's width seen from `world` (`light_spread`); 0 marches one hard ray. Wider,
// what stands in the way blocks only part of the light: a blocker `h` metres in front of the ray
// `d` metres along it covers h / (d · spread) of the light's disc (its penumbra), so skin under a
// strap standing off her is shaded only where the strap hides most of her softbox
fn screen_blocked_in(at: u32, world: vec3<f32>, to_light: vec3<f32>, reach: f32, surface_normal: vec3<f32>, to_eye: vec3<f32>, spread: f32, thickness: f32) -> f32 {
    if (at == 0u) {
        return 1.0;
    }
    let view = scene.shadows[at - 1u];
    let side = u32(view.info.y);
    // (the gap that counts: past what two of the depth view's pixels span in depth across this
    // surface as the camera sees it, so skin seen at a grazing angle never shades itself: a fixed
    // gap streaked her collarbone and shoulder top)
    let own = view.view_proj * vec4<f32>(world, 1.0);
    let row_scale = length(vec3<f32>(view.view_proj[0].x, view.view_proj[1].x, view.view_proj[2].x));
    let pixel_world = 2.0 * max(own.w, 1e-4) / (f32(side) * max(row_scale, 1e-6));
    let facing = clamp(abs(dot(surface_normal, to_eye)), 0.05, 1.0);
    let gap = 0.001 + 2.0 * pixel_world * sqrt(1.0 - facing * facing) / facing;
    var shade = 0.0;
    for (var step = 0u; step < CONTACT_STEPS; step = step + 1u) {
        let share = (f32(step) + 1.0) / f32(CONTACT_STEPS);
        let point = world + to_light * reach * share;
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
        let stored = camera_distance(view.view_proj, map_load(view.map, vec2<i32>(i32(column), i32(row))));
        let reached = camera_distance(view.view_proj, clip.z / clip.w);
        let ahead = reached - stored;
        let past_thickness = 1.0 - smoothstep(0.5 * thickness, thickness, ahead);
        if (spread > 0.0) {
            // a light with a size: the share of its disc the blocker covers, the darkest step
            shade = max(shade, clamp((ahead - gap) / (reach * share * spread), 0.0, 1.0) * past_thickness);
        } else {
            // a point: (the nearest step in shade counts most; the gap in front eased in from 1 to
            // 3 mm and out toward the thickness, so the shadow's edge grades with the depth, not cut
            // at a pixel)
            shade = max(shade, (1.0 - share * share) * smoothstep(gap, gap + 0.002, ahead) * past_thickness);
        }
    }
    return 1.0 - shade;
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

// `mesh::EyeSeen`: what a sight line meets in an eye
struct EyeSeen {
    through: bool,
    uv: vec2<f32>,
    pupil: f32,
    normal: vec3<f32>,
    cornea: f32,
    curvature: f32,
    iris_normal: vec3<f32>,
    axis: vec3<f32>,
    // what its colour is multiplied by for the limbal ring (`mesh::limbus_shade`): 1 away from it
    limbus: f32,
    // how much of the pupillary ruff covers it (ADR-835): 0 away from the pupil's edge
    ruff: f32,
};

// `mesh::limbus_shade`: the limbal ring's shade at `rim` iris radii from the axis (ADR-796); its
// strength and width ride in `eye_iris.w` and `eye_axes[1].w`
fn limbus_shade(rim: f32) -> f32 {
    let width = max(draw.eye_axes[1].w, 1e-3);
    let off = (rim - 1.0) / width;
    return 1.0 - draw.eye_iris.w * exp(-off * off);
}

// `mesh::eye_seen` (ADR-796): the line through the fragment met with the cornea's sphere on the
// viewer's side, bent there (Snell), followed to the iris's plane; the texture read at that
// point's place on the iris's disc. A line that misses the cornea's cap (the white) reflects
// about the eyeball's own normal, the rest as the mesh has it (ADR-804).
fn eye_seen(centre: vec4<f32>, frame_axis: vec3<f32>, frame_up: vec3<f32>, world: vec3<f32>, view: vec3<f32>, uv: vec2<f32>, normal: vec3<f32>, curvature: f32, pixel: f32) -> EyeSeen {
    var seen: EyeSeen;
    seen.through = false;
    seen.ruff = 0.0;
    seen.uv = uv;
    seen.pupil = 0.0;
    seen.normal = normal;
    seen.cornea = 0.0;
    seen.iris_normal = normal;
    seen.axis = vec3<f32>(0.0);
    seen.curvature = curvature;
    seen.limbus = 1.0;
    let scale = sqrt(dot(frame_axis, frame_axis));
    if (scale <= 1e-12) {
        return seen;
    }
    // the white reflects as the ball it is (ADR-804): its own normal, not the mesh's
    let ball = safe_normalize(world - centre.xyz);
    seen.normal = ball;
    let axis = frame_axis / scale;
    let iris_radius = draw.eye_shape.y * scale;
    let cornea_radius = draw.eye_shape.z * scale;
    let ball_radius = draw.eye_shape.x * scale;
    // the white's share of the limbal ring: by its distance from the axis, on the eye's front
    let from_centre = world - centre.xyz;
    let height = dot(from_centre, axis);
    if (height > 0.0) {
        let off_axis = from_centre - axis * height;
        seen.limbus = limbus_shade(sqrt(dot(off_axis, off_axis)) / max(iris_radius, 1e-9));
    }
    let limbus_height = sqrt(max(ball_radius * ball_radius - iris_radius * iris_radius, 0.0));
    let cornea_height = limbus_height - sqrt(max(cornea_radius * cornea_radius - iris_radius * iris_radius, 0.0));
    let iris_height = cornea_height + cornea_radius - draw.eye_shape.w * scale;
    let from_cornea = world - centre.xyz - axis * cornea_height;
    let along = dot(from_cornea, view);
    let discriminant = along * along - (dot(from_cornea, from_cornea) - cornea_radius * cornea_radius);
    if (discriminant < 0.0) {
        return seen;
    }
    let travel = sqrt(discriminant) - along;
    if (travel < 0.0) {
        return seen;
    }
    let entry = from_cornea + view * travel;
    let entry_along = dot(entry, axis);
    if (entry_along + cornea_height < limbus_height) {
        return seen;
    }
    let cornea_normal = entry / cornea_radius;
    let cosine = dot(cornea_normal, view);
    let ratio = 1.0 / max(draw.eye_up.w, 1.0);
    let inside = max(1.0 - ratio * ratio * (1.0 - cosine * cosine), 0.0);
    let bend = ratio * cosine - sqrt(inside);
    let bent = bend * cornea_normal - ratio * view;
    let sinking = dot(bent, axis);
    if (sinking > -1e-6) {
        return seen;
    }
    let reach = (iris_height - entry_along - cornea_height) / sinking;
    let hit = entry + axis * cornea_height + bent * reach;
    let up = safe_normalize(frame_up - axis * dot(frame_up, axis));
    let right = cross(up, axis);
    let across = dot(hit, right) / iris_radius;
    let above = dot(hit, up) / iris_radius;
    let rim = sqrt(across * across + above * above);
    // each eye's own pupil (ADR-835): its size, its middle off the iris's, its outline out of
    // round by its waves; its edge as sharp as a pixel allows, a dark scalloped ruff round it
    let which = select(0u, 1u, centre.w >= 0.5);
    let own = draw.clamps[which];
    let pupil_share = draw.eye_iris.y * own.x;
    if (pupil_share > 0.0) {
        let from_pupil = vec2<f32>(across - own.y, above - own.z);
        let reach = length(from_pupil);
        let angle = atan2(from_pupil.y, from_pupil.x);
        let waves = draw.clamps[2u + which];
        let outline = 1.0
            + waves.y * cos(floor(waves.x) * angle + fract(waves.x) * 6.2831853)
            + waves.w * cos(floor(waves.z) * angle + fract(waves.z) * 6.2831853);
        let edge = pupil_share * outline;
        // half a pixel, in iris radii, never under a hundredth of the pupil's radius
        let soft = max(0.5 * pixel / iris_radius, 0.01 * pupil_share);
        seen.pupil = 1.0 - smoothstep(edge - soft, edge + soft, reach);
        // the ruff: its crenellations a few dozen round, their phase the eye's own
        let crenels = 0.75 + 0.25 * cos(36.0 * angle + 2.4 * f32(which));
        let ruff_out = edge + own.w * crenels;
        seen.ruff = (1.0 - smoothstep(ruff_out - soft, ruff_out + soft, reach)) * (1.0 - seen.pupil) * select(0.0, 1.0, own.w > 0.0);
    }
    let entry_out = entry - axis * entry_along;
    let entry_rim = sqrt(dot(entry_out, entry_out)) / iris_radius;
    // (`mesh::CORNEA_EDGE`)
    let cornea = 1.0 - smoothstep(0.85, 1.0, entry_rim);
    var iris_uv = draw.eye_uv.xy;
    if (centre.w >= 0.5) {
        iris_uv = draw.eye_uv.zw;
    }
    seen.through = true;
    seen.uv = iris_uv + vec2<f32>(across, -above) * draw.eye_iris.x;
    seen.limbus = limbus_shade(rim);
    // the cornea's normal, giving way to the ball's toward the limbus (ADR-804)
    seen.normal = safe_normalize(ball + (cornea_normal - ball) * cornea);
    seen.cornea = cornea;
    seen.curvature = curvature + (1.0 / cornea_radius - curvature) * cornea;
    // the iris as a shallow bowl: its normal leans toward the axis, more the further out
    let slope = draw.eye_iris.z / max(rim, 1.0);
    seen.iris_normal = safe_normalize(axis - slope * (right * across + up * above));
    seen.axis = axis;
    return seen;
}


// ADR-813: hair lit as Karis 2016 does it (UE4's `HairShading`): Marschner's three paths
// made cheap. R, the white shine off the cuticle, shifted toward the root; TT, light through the
// fibre, tinted by its colour (lights her outline from behind); TRT, the second, coloured shine;
// and Karis's stand-in for light scattered through many fibres, a wrapped Lambert about a normal
// facing the viewer, tinted deeper the more it was shadowed. `tangent` the strand's direction,
// `seen` how much of the light its shadow lets through. Brood's lights put no 1/π on diffuse,
// so the whole is π times UE's.
// one hair fibre's transmittance, crossed once through its middle (two radii), from its colour:
// the absorption Chiang & Bitterli 2016 ("A practical and controllable hair and fur model") give
// for a hair of that albedo at its roughness β, σa = (ln A / (5.969 − 0.215β + 2.532β² − 10.73β³
// + 5.574β⁴ + 0.245β⁵))², so a light hair passes much and a dark one little, each its own hue
fn fibre_transmittance(colour: vec3<f32>) -> vec3<f32> {
    let b = clamp(draw.material.x, 0.0, 1.0);
    let b2 = b * b;
    let denominator = 5.969 - 0.215 * b + 2.532 * b2 - 10.73 * b2 * b + 5.574 * b2 * b2 + 0.245 * b2 * b2 * b;
    let ratio = log(clamp(colour, vec3<f32>(1e-4), vec3<f32>(1.0))) / denominator;
    // (ADR-827) a fibre's absorption is its two melanins', whose spectra tie the channels
    // together (d'Eon et al. 2011, as pbrt holds them): the absorption Chiang's inversion asks of
    // each channel apart, fitted by least squares with neither amount below none. Each channel
    // apart, a red hair's red was hardly absorbed at all (green 7.6 times red, where pheomelanin
    // absorbs it 2.1 times), so light through her hair came out a pure, bright red
    return exp(-2.0 * melanin_fit(ratio * ratio));
}

// the absorption `wanted`, channel by channel, as the nearest sum of eumelanin's and pheomelanin's
// (amounts never below none): their absorption a unit of each, d'Eon et al. 2011
fn melanin_fit(wanted: vec3<f32>) -> vec3<f32> {
    let eumelanin = vec3<f32>(0.419, 0.697, 1.37);
    let pheomelanin = vec3<f32>(0.187, 0.4, 1.05);
    let eu_eu = dot(eumelanin, eumelanin);
    let pheo_pheo = dot(pheomelanin, pheomelanin);
    let eu_pheo = dot(eumelanin, pheomelanin);
    let eu_wanted = dot(eumelanin, wanted);
    let pheo_wanted = dot(pheomelanin, wanted);
    let determinant = eu_eu * pheo_pheo - eu_pheo * eu_pheo;
    var eu_amount = (eu_wanted * pheo_pheo - pheo_wanted * eu_pheo) / determinant;
    var pheo_amount = (pheo_wanted * eu_eu - eu_wanted * eu_pheo) / determinant;
    // (one below none: the best of the other alone)
    if (eu_amount < 0.0) {
        eu_amount = 0.0;
        pheo_amount = max(pheo_wanted / pheo_pheo, 0.0);
    } else if (pheo_amount < 0.0) {
        pheo_amount = 0.0;
        eu_amount = max(eu_wanted / eu_eu, 0.0);
    }
    return eu_amount * eumelanin + pheo_amount * pheomelanin;
}

fn hair_gaussian(width: f32, angle: f32) -> f32 {
    return exp(-0.5 * angle * angle / (width * width)) / (2.50662827 * width);
}

// Schlick at hair's index, 1.55
fn hair_fresnel(cosine: f32) -> f32 {
    return 0.0465 + 0.9535 * pow(1.0 - clamp(cosine, 0.0, 1.0), 5.0);
}

// `known`: 1 when the light has a shadow map (what lies between it and the strand is known), 0
// when not: then it lights the strand only by what faces it (R, TRT, the scatter toward the
// viewer), never THROUGH it, since nothing says her head is not in the way (39, 2026-11-26: a
// shadowless fill behind her head lit the back of her hair near white through it)
fn karis_hair(base: vec3<f32>, tangent: vec3<f32>, view: vec3<f32>, to_light: vec3<f32>, seen: f32, known: f32) -> vec3<f32> {
    let roughness = clamp(draw.material.x, 1.0 / 255.0, 1.0);
    // each strand a little rougher or smoother than the next (`_STRAND`, ADR-813; `:roughness-spread`)
    let spread = draw.fibre_cut.y;
    let width = roughness * roughness * (1.0 - 0.5 * spread + spread * fract(fibre_strand * 7.13));
    let sin_light = clamp(dot(tangent, to_light), -1.0, 1.0);
    let sin_view = clamp(dot(tangent, view), -1.0, 1.0);
    let cos_difference = cos(0.5 * abs(asin(sin_view) - asin(sin_light)));
    let light_across = to_light - sin_light * tangent;
    let view_across = view - sin_view * tangent;
    let cos_phi = dot(light_across, view_across)
        * inverseSqrt(dot(light_across, light_across) * dot(view_across, view_across) + 1e-4);
    let cos_half_phi = sqrt(clamp(0.5 + 0.5 * cos_phi, 0.0, 1.0));
    // the cuticle's tilt (2°) and the index the fibre's cross-section shows (Karis's fit)
    // (each strand's own, ± half of it: `_STRAND`; `:cuticle-tilt`)
    let tilt = draw.fibre_cut.x * (1.0 + (fract(fibre_strand) - 0.5));
    let index = 1.19 / cos_difference + 0.36 * cos_difference;
    let colour = max(base, vec3<f32>(1e-4));
    let along = sin_light + sin_view;
    // R
    let shift = 2.0 * sin(-2.0 * tilt)
        * (cos(-2.0 * tilt) * cos_half_phi * sqrt(max(1.0 - sin_view * sin_view, 0.0)) + sin(-2.0 * tilt) * sin_view);
    let reflected = hair_gaussian(width * 1.41421356 * max(cos_half_phi, 1e-3), along - shift)
        * 0.25 * cos_half_phi
        * hair_fresnel(sqrt(clamp(0.5 + 0.5 * dot(view, to_light), 0.0, 1.0)));
    // TT
    let inverse = 1.0 / index;
    let entry = cos_half_phi * (1.0 + inverse * (0.6 - 0.8 * cos_phi));
    let entering = hair_fresnel(cos_difference * sqrt(clamp(1.0 - entry * entry, 0.0, 1.0)));
    let through = pow(colour, vec3<f32>(0.5 * sqrt(max(1.0 - entry * entry * inverse * inverse, 0.0)) / cos_difference));
    let transmitted = hair_gaussian(width * 0.5, along - tilt)
        * exp(-3.65 * cos_phi - 3.98) * (1.0 - entering) * (1.0 - entering) * through * known;
    // TRT
    let inside = hair_fresnel(cos_difference * 0.5);
    let twice = pow(colour, vec3<f32>(0.8 / cos_difference));
    let second = hair_gaussian(width * 2.0, along - 4.0 * tilt)
        * exp(17.0 * cos_phi - 16.78) * (1.0 - inside) * (1.0 - inside) * inside * twice;
    // light scattered through many fibres
    let facing = safe_normalize(view - tangent * sin_view);
    let wrapped = clamp((dot(facing, to_light) + 1.0) * 0.25, 0.0, 1.0);
    let scatter = 0.31830989 * mix(wrapped, 1.0 - abs(sin_light), 0.33 * known) * draw.fibre_look.x;
    // (in shade the light has come through more hair: darker and a little deeper in colour, its
    // colour raised to `:scatter-depth` times the shaded share, ADR-825. Divided by its own
    // luminance, as it was, the shade kept its brightness and red went past red: red hair
    // crimson where real hair goes dark auburn, 39's photo patches)
    let tint = pow(colour, vec3<f32>(draw.fibre_look.y * (1.0 - clamp(seen, 0.0, 1.0))));
    let shine = vec3<f32>(reflected * draw.material.y * 2.0) + transmitted + second;
    return 3.14159265 * (shine + sqrt(colour) * scatter * tint);
}
// ADR-831: an ambient `visibility` (its occlusion) with the light bounced back off surroundings
// of `albedo`, per channel: GTAO's multi-bounce fit to path-traced references (Jimenez, Wu,
// Pesce, Jarabo, "Practical Real-Time Strategies for Accurate Indirect Occlusion", SIGGRAPH
// 2016): never darker than the occlusion alone, a red surround giving back mostly red
fn bounced_occlusion(visibility: f32, albedo: vec3<f32>) -> vec3<f32> {
    let a = 2.0404 * albedo - vec3<f32>(0.3324);
    let b = -4.7951 * albedo + vec3<f32>(0.6417);
    let c = 2.7552 * albedo + vec3<f32>(0.6903);
    let v = vec3<f32>(visibility);
    return max(v, ((a * v + b) * v + c) * v);
}
// ADR-836: a cloth's sheen, the soft light a knit's fibres throw back at grazing angles: Charlie's
// distribution (Estevez & Kulla, "Production Friendly Microfacet Sheen BRDF", 2017) over the
// sheen roughness `r` (alpha r * r, as glTF's KHR_materials_sheen) …
fn sheen_distribution(r: f32, normal_dot_half: f32) -> f32 {
    let alpha = max(r * r, 0.0049);
    let sine_squared = max(1.0 - normal_dot_half * normal_dot_half, 0.0);
    return (2.0 + 1.0 / alpha) * pow(sine_squared, 0.5 / alpha) / (2.0 * 3.14159265);
}
// … with Neubelt & Pettineo's visibility ("Crafting a Next-Gen Material Pipeline for The Order:
// 1886", 2013) …
fn sheen_visibility(normal_dot_light: f32, normal_dot_view: f32) -> f32 {
    return 1.0 / max(4.0 * (normal_dot_light + normal_dot_view - normal_dot_light * normal_dot_view), 1e-4);
}
// … and how much of the light that sheen takes over the hemisphere, its directional albedo:
// integrated numerically over r 0.3–1 and n·v 0.05–1 and fitted (womanblend-f9's
// tools/sheen-albedo-fit.py there, the fit within 0.05 of the integral); what the base layer is
// scaled down by, so cloth with sheen is never lit brighter than the light
fn sheen_albedo(r: f32, normal_dot_view: f32) -> f32 {
    let rough = clamp(r, 0.3, 1.0);
    let cosine = clamp(normal_dot_view, 0.05, 1.0);
    let inverse = 1.0 / (cosine + 0.15);
    return clamp(-0.30065 + 1.4614 * rough - 0.60645 * rough * rough - 0.44483 * cosine
        - 0.19257 * rough * cosine + (0.34488 - 0.61552 * rough + 0.27141 * rough * rough) * inverse
        + 0.21332 * cosine * cosine, 0.0, 1.0);
}
// (a surface has a sheen when it is a standard, cutout or blended one with `:sheen-cloth`, carried in
// `DrawU::strand`, which only hair reads: its colour, then its roughness above 0)
fn has_sheen() -> bool {
    return draw.flags.x >= 1u && draw.flags.x <= 3u && draw.strand.w > 0.0;
}
// (`diffuse_normal` is what the light falls on and `mirror_surface` the surface a reflection must
// not go under: `normal` and `surface_normal` but for an `:eye` under its cornea, ADR-796)
fn shade(base: vec3<f32>, normal: vec3<f32>, view: vec3<f32>, world: vec3<f32>, surface_normal: vec3<f32>, occlusion: f32, turn: f32, strand: vec3<f32>, curvature: f32, specular_kept: f32, diffuse_normal: vec3<f32>, mirror_surface: vec3<f32>, under_cornea: vec4<f32>) -> vec3<f32> {
    var out = base * ambient(diffuse_normal) * occlusion;
    // what the occlusion took away given back in part by the surroundings' colour (ADR-831):
    // inside her mouth the gums' red, so a shadowed tooth goes dim and warm, not grey
    if (draw.flags.x != 4u && draw.flags.x != 5u && draw.sheen.w > 0.5) {
        out = base * ambient(diffuse_normal) * bounced_occlusion(occlusion, draw.sheen.rgb);
    }
    // a cloth's sheen (ADR-836): the base layer under it scaled down by what the sheen takes, and
    // the ambient's share of the sheen itself
    var sheen_kept = 1.0;
    if (has_sheen()) {
        let looking = clamp(dot(normal, view), 0.0, 1.0);
        let taken = sheen_albedo(draw.strand.w, looking);
        sheen_kept = 1.0 - max(draw.strand.x, max(draw.strand.y, draw.strand.z)) * taken;
        out = out * sheen_kept + draw.strand.rgb * taken * ambient(normal) * occlusion;
    }
    // a `:fibres` strand's ambient through the hair round it, as a light's comes (ADR-828): the
    // bake's mean crossings over every direction (its constant term), each fibre passing its own
    // colour's share and scattering a little away. A grey exp(-0.05 crossings) let inner strands
    // keep nearly all of it: the gaps between locks never went dark (her brown hair's shade
    // quartile L* 21 against a photograph's 13, womanblend-39)
    if ((draw.flags.y & 512u) != 0u && dot(fibre_visibility, fibre_visibility) > 0.0) {
        let crossed = draw.material.z * max(fibre_visibility.x, 0.0);
        out = out * pow(fibre_transmittance(base), vec3<f32>(crossed)) * exp(-0.03 * crossed);
    }
    let f0 = draw.lobes.w;
    // a `:hair` surface's highlights: as sharp as its roughness asks
    let exponent = shininess(surface_roughness());
    let reflects = draw.flags.x != 4u && draw.flags.x != 5u;
    // how far the reflection turns across half a pixel: twice the curvature, times the pixel
    var reflection_turn = 0.0;
    if (reflects && curvature > 0.0) {
        reflection_turn = 2.0 * curvature * pixel_span(world, view);
    }
    // the `:coat`: its strength, its roughness
    var coat = vec2<f32>(0.0);
    if (reflects && draw.skin.y > 0.0) {
        coat = vec2<f32>(draw.skin.y * region.w, draw.skin.z);
    }
    // per light: how much of its shape's image is left by its shadow, cone and range window
    var shaping: array<f32, 8>;
    let count = u32(scene.params.y);
    // on skin, the brightest light's unshadowed direct share here (ADR-837): the contact march
    // runs only for a light giving at least `CONTACT_SHARE` of it
    var brightest = 0.0;
    let skin_curvature = curvature * draw.skin.x * region.y;
    if (draw.flags.x == 0u) {
        for (var slot = 0u; slot < count; slot = slot + 1u) {
            let light = scene.lights[slot];
            brightest = max(brightest, skin_share(light, light_arrival(light, world), diffuse_normal, skin_curvature));
        }
    }
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        let light = scene.lights[slot];
        let arrival = light_arrival(light, world);
        let arriving = arrival.amount;
        var seen = 1.0;
        var contact = 1.0;
        var share = 0.0;
        if (draw.flags.x == 0u) {
            share = skin_share(light, arrival, diffuse_normal, skin_curvature);
        }
        let marched_on_skin = share > 0.0 && share >= CONTACT_SHARE * brightest;
        // what is opaque in a strand's way (ADR-827): her head shadows her hair
        var opaque = 1.0;
        if (light.shadow.z > 0u) {
            // the `:fibres` hair in the way (ADR-827: its opacity maps; none, 1)
            let view_index = light_view(light, world);
            let hair_through = hair_transmittance(light, view_index, world, turn);
            if (draw.flags.x == 5u) {
                // a strand: the opaque maps a centimetre toward the light (its own scalp, under
                // it, does not shadow it), and the hair before it
                opaque = shadow_sample(view_index, world + arrival.to_light * 0.01);
                seen = opaque * hair_through;
            } else if (draw.flags.x == 4u) {
                // hair shades hair by how much of it the light crossed, not in or out
                seen = hair_shadow(light, world, turn) * hair_through;
            } else {
                // (and the shadow of what is too near and small for its map, ADR-818)
                // (never on skin: light travels millimetres under it, so its own creases never show
                // shadows this sharp: on her neck and collarbones they drew hard dark lines)
                if (draw.flags.x != 0u) {
                    contact = contact_shadow(light, world, arrival.to_light, surface_normal, view);
                } else if (marched_on_skin) {
                    contact = contact_shadow_on_skin(light, world, arrival.to_light, surface_normal, view);
                }
                seen = shadow_visibility(light, world, surface_normal, arrival.to_light, turn) * contact * hair_through;
            }
        } else if (draw.flags.x == 0u) {
            // skin under an unshadowed light: only what the casters' view holds (ADR-837)
            if (marched_on_skin) {
                contact = contact_shadow_on_skin(light, world, arrival.to_light, surface_normal, view);
            }
            seen = contact;
        } else if (draw.flags.x != 4u && draw.flags.x != 5u) {
            // a light with no shadow map still stops at what stands a few millimetres in its
            // way (ADR-818, amended 2026-11-27): her fill, unshadowed, lit the side teeth behind
            // her lips and cheeks with a third of all their light, the most of any source
            contact = contact_shadow(light, world, arrival.to_light, surface_normal, view);
            seen = contact;
        }
        let to_light = arrival.to_light;
        // `mesh::light_under_cornea`: under a cornea the light is bent toward the eye's axis on
        // its way in, and only what the eye's front does not reflect gets in
        var falling = to_light;
        var let_in = 1.0;
        if (under_cornea.w > 0.0) {
            let cosine = dot(under_cornea.xyz, to_light);
            if (cosine <= 0.0) {
                let_in = 0.0;
            } else {
                let ratio = under_cornea.w;
                let inside = max(1.0 - ratio * ratio * (1.0 - cosine * cosine), 0.0);
                falling = ratio * to_light + (sqrt(inside) - ratio * cosine) * under_cornea.xyz;
                let head_on = (1.0 - ratio) / (1.0 + ratio);
                let_in = 1.0 - fresnel(head_on * head_on, cosine);
            }
        }
        let facing = dot(diffuse_normal, falling);
        var lambert = max(facing, 0.0);
        if (under_cornea.w > 0.0) {
            lambert = lambert * let_in;
        }
        let mirror_lambert = max(dot(normal, to_light), 0.0);
        let half_vector = safe_normalize(to_light + view);
        // a `:fibres` strand (ADR-813): Karis's three paths and his scattered light, all of it
        // under the hair's own shadow
        if ((draw.flags.y & 512u) != 0u) {
            // its baked self-shadow where it has one (ADR-813): the hairs crossed toward this light, each
            // passing some of it as ITS OWN colour lets it through (`fibre_transmittance`) and scattering
            // a little away; the opacity maps' estimate otherwise. (A fixed ginger fibre once tinted
            // every hair colour the same orange by the lamp and from behind: 39, 2026-11-26)
            // Either way under what is opaque in its way: the bake counts hair alone, so without
            // it the key in front of her lit the back of her hair through her head, a brighter,
            // purer red the more hair it crossed (ADR-827)
            var through = vec3<f32>(seen);
            // the shaded share the scattered light's tint is raised by (ADR-825): none where the
            // bake already coloured the light by the fibres it crossed, or it was coloured twice,
            // each channel apart the second time (the under-layer behind her jaw a near-black
            // crimson, G/R 0.10)
            var baked = false;
            if (dot(fibre_visibility, fibre_visibility) > 0.0) {
                baked = true;
                let crossed = draw.material.z * max(0.0, fibre_visibility.x + dot(fibre_visibility.yzw, to_light));
                through = pow(fibre_transmittance(base), vec3<f32>(crossed)) * exp(-0.03 * crossed) * opaque;
            }
            let shadowed = select(dot(through, vec3<f32>(0.2126, 0.7152, 0.0722)), 1.0, baked);
            out = out + light.color.rgb * arriving * through * karis_hair(base, strand, view, to_light, shadowed, select(0.0, 1.0, light.shadow.z > 0u));
            continue;
        }
        if (draw.flags.x == 4u || draw.flags.x == 5u) {
            // the two highlights' shifts (`:shift`, ADR-799) and the second one's colour
            // (`:sheen`, else the hair's own)
            let primary = hair_highlight(strand, normal, half_vector, draw.strand.z, exponent);
            let secondary = hair_highlight(strand, normal, half_vector, draw.strand.w, exponent * 0.25);
            let sheen = select(base, draw.sheen.rgb, draw.sheen.w > 0.5);
            let lit_side = clamp(facing + 0.25, 0.0, 1.0);
            let hair_diffuse = clamp(0.25 + 0.75 * facing, 0.0, 1.0);
            let shine = draw.material.y * lit_side * (vec3<f32>(primary) + secondary * sheen);
            out = out + light.color.rgb * arriving * hair_light(seen, base) * (base * hair_diffuse + shine);
            continue;
        }
        // a shaped light's highlight is its shape's image, below: its inverse square is the
        // image's size, so only its cone and range window are kept
        var specular = 0.0;
        if (light.shadow.w != 0u) {
            var spread_out = 1.0;
            if (light.shadow.x != 0u) {
                let toward = light.position.xyz - world;
                spread_out = max(dot(toward, toward), 1e-4);
            }
            shaping[slot] = seen * arriving * spread_out;
        } else {
            specular = fresnel(f0, dot(view, half_vector)) * specular_lobes(dot(normal, half_vector), reflection_turn) * mirror_lambert * specular_kept;
            if (coat.x > 0.0) {
                specular = specular + coat.x * fresnel(0.02, dot(view, half_vector)) * specular_lobe(coat.y, dot(normal, half_vector), reflection_turn) * mirror_lambert;
            }
        }
        var diffuse = vec3<f32>(lambert * seen);
        if (draw.flags.x == 0u || (draw.flags.x == 6u && under_cornea.w == 0.0)) {
            // the CPU's: the light under the skin in place of Lambert's; toward the terminator
            // what the scattering moved takes the light's shadow looked up just off the skin
            // (`mesh::SCATTER_LIFT`), clear of the body's own shadow there but not of a hand's or
            // an arm's (ADR-815: unshadowed, the red wrap shone in a cast shadow)
            var lifted = seen;
            if (light.shadow.z > 0u && facing < 0.3) {
                lifted = contact * shadow_visibility(light, world + surface_normal * (SCATTER_LIFT * 1000.0 * draw.skin.x * region.y), surface_normal, arrival.to_light, turn);
            }
            let scattered_seen = mix(lifted, seen, smoothstep(0.0, 0.3, facing));
            let scattered = skin_diffuse(facing, curvature * draw.skin.x * region.y);
            diffuse = diffuse + (scattered - vec3<f32>(lambert)) * scattered_seen;
        }
        var lit = base * diffuse + vec3<f32>(specular * seen);
        if (has_sheen()) {
            // a cloth's sheen under each light (ADR-836): Charlie's distribution, Neubelt's
            // visibility, under the light's shadow; the base layer kept as above. Times π: this
            // renderer folds the BRDF's 1/π into the light (a matte surface is base · light · cos,
            // `specular_lobe` normalised (n + 8) / 8), so a radiometric BRDF is π times its own
            let looking = clamp(dot(normal, view), 1e-4, 1.0);
            let sheen = sheen_distribution(draw.strand.w, dot(normal, half_vector)) * sheen_visibility(mirror_lambert, looking) * mirror_lambert;
            lit = lit * sheen_kept + draw.strand.rgb * (3.14159265 * sheen * seen);
        }
        out = out + light.color.rgb * arriving * lit;
    }
    if (!reflects) {
        return out;
    }
    // the surroundings, reflected (ADR-795): a layer at a time, each a weight, a roughness and
    // a reflectance at normal incidence — the material's lobes, then its coat (`mesh::shade`)
    let facing_view = dot(normal, view);
    let normal_dot_view = clamp(facing_view, 0.0, 1.0);
    let reflected = 2.0 * facing_view * normal - view;
    let kept = horizon(reflected, mirror_surface);
    let weights = vec3<f32>(draw.lobes.z * specular_kept, (1.0 - draw.lobes.z) * specular_kept, coat.x);
    let roughnesses = vec3<f32>(surface_roughness() * draw.lobes.x, surface_roughness() * draw.lobes.y, coat.y);
    let reflectances = vec3<f32>(f0, f0, 0.02);
    let emitter_count = u32(scene.picture.z);
    // (the room's reflection hidden where something of the scene stands within `REFLECTION_REACH`
    // along the reflected direction, as the depth view holds it: deep in her mouth her cheeks and
    // lips hide the room from her side teeth, which shone grey as chrome, ADR-818)
    let reflection_open = select(screen_blocked(world, reflected, REFLECTION_REACH, surface_normal, view), 1.0, draw.flags.x == 0u);
    for (var layer = 0u; layer < 3u; layer = layer + 1u) {
        let weight = weights[layer];
        if (weight <= 0.0) {
            continue;
        }
        let roughness = roughnesses[layer];
        let reflectance = reflectances[layer];
        let lobe = lobe_exponent(roughness);
        let open = weight * kept * reflection_open * specular_occlusion(normal_dot_view, occlusion, roughness);
        let fit = environment_brdf(roughness, normal_dot_view);
        let surrounding = open * (reflectance * fit.x + min(50.0 * reflectance, 1.0) * fit.y);
        out = out + ambient_radiance(reflected, lobe) * surrounding;
        let mirrored = open * fresnel(reflectance, normal_dot_view);
        let spread = lobe_spread(lobe, reflection_turn);
        for (var index = 0u; index < emitter_count; index = index + 1u) {
            let emitter = scene.emitters[index];
            var left = 1.0;
            let owner = u32(emitter.facing.w);
            if (owner > 0u) {
                left = shaping[owner - 1u];
            }
            if (left <= 0.0) {
                continue;
            }
            out = out + emitter.radiance.rgb * (mirrored * left * emitter_coverage(emitter, world, reflected, spread));
        }
    }
    return out;
}

fn aces_fit(x: vec3<f32>) -> vec3<f32> {
    return (x * (2.51 * x + vec3<f32>(0.03))) / (x * (2.43 * x + vec3<f32>(0.59)) + vec3<f32>(0.14));
}

// ADR-830: Narkowicz's ACES fit on the luminance alone, the colour scaled with it so its hue and
// its chroma are the scene's, and moved toward white only past the display's top. On each
// channel apart the curve's slope raised every mid-tone's saturation by about a third and turned
// lit skin yellow while skin in shade stayed red (the hue moved with brightness)
fn tone(color: vec3<f32>, exposure: f32) -> vec3<f32> {
    let exposed = max(color * exposure, vec3<f32>(0.0));
    let luminance = dot(exposed, vec3<f32>(0.2126, 0.7152, 0.0722));
    let mapped_luminance = aces_fit(vec3<f32>(luminance)).x;
    var mapped = exposed * (mapped_luminance / max(luminance, 1e-6));
    let peak = max(mapped.r, max(mapped.g, mapped.b));
    if (peak > 1.0) {
        mapped = mix(mapped, vec3<f32>(mapped_luminance), clamp((peak - 1.0) / max(peak - mapped_luminance, 1e-6), 0.0, 1.0));
    }
    return linear_to_srgb(clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0)));
}

// the samples a `:hair` or `:strands` fragment covers (each a step of
// `mesh::HAIR_COVERAGE_STEPS`: `hair_step`), from its texel's alpha: what `fs_mesh` writes
// as its sample mask and the hair's depth pass writes to depth (ADR-802)
fn hair_mask(texel: vec4<f32>, fragment: VertexOut) -> u32 {
    var coverage = clamp((texel.a * draw.color.a - 0.5 * draw.material.w) / max(draw.material.w, 1e-6), 0.0, 1.0);
    var mask = 0u;
    for (var sample = 0u; sample < SAMPLES; sample = sample + 1u) {
        if (coverage > hair_step(sample)) {
            mask = mask | (1u << sample);
        }
    }
    // a strand thinner than a pixel covers only its share of one, at samples its own
    // position picks, so overlapping strands add up (`mesh::strand_sample_step`)
    // (a strand as wide as a pixel, a wisp, keeps the fixed steps above: `mesh::STRAND_WHOLE`)
    if (draw.flags.x == 5u && fragment.tangent.w < 0.999) {
        coverage = coverage * fragment.tangent.w;
        mask = 0u;
        for (var sample = 0u; sample < SAMPLES; sample = sample + 1u) {
            if (coverage > strand_sample_step(fragment.world, sample)) {
                mask = mask | (1u << sample);
            }
        }
    }
    return mask;
}

struct MeshFragment {
    @location(0) color: vec4<f32>,
    // the samples it covers: all of them, but a `:hair` surface's coverage of its `SAMPLES`
    // (each a step of `mesh::HAIR_COVERAGE_STEPS`: `hair_step`)
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
    let detail_texel = straight(textureSample(detail_texture, texture_sampler, detail_uv(fragment)), (draw.flags.y & 128u) != 0u);
    let texel = select(vec4<f32>(1.0), base_texel, (draw.flags.y & 1u) != 0u);
    // a skin's regions (ADR-826, texture bit 2048): its roughness; its thickness, which scales the
    // scatter (thin skin, ears and nostrils, glows; the body's 0.7 is the material's own); the detail
    // tile's strength (stored over two: 0.5 is the material's); its oil, which scales the coat (the
    // body's 0.3 is the material's)
    if ((draw.flags.y & 2048u) != 0u) {
        // (un-premultiplied: its alpha is the oil, bit 4096)
        let regions = straight(textureSample(regions_texture, texture_sampler, fragment.uv), (draw.flags.y & 4096u) != 0u);
        region = vec4<f32>(regions.x, max(1.0 + (0.7 - regions.y) * 1.5, 0.0), regions.z * 2.0, regions.w / 0.3);
    }
    // the texture's footprint at this pixel, for an eye's second read (taken before any branch)
    let uv_dx = dpdx(fragment.uv);
    let uv_dy = dpdy(fragment.uv);
    var alpha = texel.a * draw.color.a;
    if (draw.flags.x == 2u || draw.flags.x == 6u) {
        if (alpha < draw.material.w) {
            discard;
        }
        alpha = 1.0;
    } else if (draw.flags.x == 3u) {
        // a stretched knit lets her skin show between its opened loops (ADR-840)
        alpha = alpha * knit_alpha(fragment);
        if (alpha <= 0.002) {
            discard;
        }
    } else {
        alpha = 1.0;
    }
    var mask = 0xFFFFFFFFu;
    if (draw.flags.x == 4u || draw.flags.x == 5u) {
        mask = hair_mask(texel, fragment);
        if (mask == 0u) {
            discard;
        }
        if (HAIR_TRIAL == 1u) {
            var flat: MeshFragment;
            flat.color = vec4<f32>(linear_to_srgb(srgb_to_linear(texel.rgb) * draw.color.rgb), 1.0);
            flat.mask = mask;
            return flat;
        }
    }
    return MeshFragment(mesh_lit(fragment, front, texel, normal_texel, occlusion_texel, detail_texel, uv_dx, uv_dy, alpha), mask);
}

// `fs_mesh`'s colour from its textures, its coverage decided: no `discard` anywhere in it, so
// the hair's front pass (`fs_hair_front`) keeps the early depth test (ADR-802)
fn mesh_lit(fragment: VertexOut, front: bool, texel: vec4<f32>, normal_texel: vec4<f32>, occlusion_texel: vec4<f32>, detail_texel: vec4<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>, alpha: f32) -> vec4<f32> {
    // the surface direction its image runs up (decreasing v, glTF's +Y of a normal map), from the
    // screen's derivatives (Schüler's cotangent frame): what a normal map's green tilts toward,
    // whatever handedness its tangent's w says (taken here, in uniform control flow)
    let world_dx = dpdx(fragment.world);
    let world_dy = dpdy(fragment.world);
    let uv_turn = uv_dx.x * uv_dy.y - uv_dy.x * uv_dx.y;
    let image_up = -(world_dy * uv_dx.x - world_dx * uv_dy.x) * sign(uv_turn);
    var base = srgb_to_linear(texel.rgb) * draw.color.rgb;
    // a garment's stretch shown in its albedo's place (ADR-842; the flag is the draw's, uniform)
    let stretch_view = (draw.flags.y & 32768u) != 0u;
    if (stretch_view) {
        base = stretch_shown(fragment, world_dx, world_dy, dpdx(fragment.made), dpdy(fragment.made), uv_dx, uv_dy);
    }
    // a `:fibres` strand (ADR-813): its own lightness (`_STRAND`, ±`:colour-spread`), darker toward its
    // root, its last third lighter toward its tip (`:tip-lighten`: sun and wear, ADR-825), and its
    // value and baked self-shadow handed to the light loop
    let fibres = (draw.flags.y & 512u) != 0u;
    if (fibres) {
        fibre_strand = fragment.curvature;
        fibre_visibility = fragment.visibility;
        let root = clamp(fragment.uv.y, 0.0, 1.0);
        let spread = draw.fibre_look.z;
        base = base * (1.0 - spread + 2.0 * spread * fract(fibre_strand)) * mix(1.0, 0.75, root * root);
        // (`uv.y` 1 at the root, 0 at the tip: the last third eases toward the tip)
        let toward_tip = clamp((0.3333 - root) * 3.0, 0.0, 1.0);
        base = mix(base, sqrt(base), draw.fibre_look.w * toward_tip);
    }
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
            local = detail_normal(local, tilt, draw.detail.z * region.z);
            framed = true;
        }
    }
    // her expression's wrinkles (ADR-817, the CPU's `fill_triangle`): the compress and stretch
    // maps' tilts by the zones' masks and weights, laid over the rest
    if ((draw.flags.y & 1024u) != 0u) {
        // (read at their own size, `:data`, by the UV set they name: `clamp_info.y` 1 the second)
        let at = select(fragment.uv, fragment.uv2, draw.clamp_info.y > 0.5);
        let tilt = wrinkle_tilt(
            textureSampleLevel(wrinkle_compress, texture_sampler, at, 0.0),
            textureSampleLevel(wrinkle_stretch, texture_sampler, at, 0.0),
            textureSampleLevel(wrinkle_mask_first, texture_sampler, at, 0.0),
            textureSampleLevel(wrinkle_mask_second, texture_sampler, at, 0.0));
        if (tilt.x != 0.0 || tilt.y != 0.0) {
            local = detail_normal(local, tilt, 1.0);
            framed = true;
        }
    }
    // its cavity damps the specular and the rim, never the diffuse
    var specular_kept = 1.0;
    if ((draw.flags.y & 64u) != 0u) {
        specular_kept = detail_specular(detail_texel.b, draw.detail.w * region.z);
    }
    if (framed) {
        let tangent = safe_normalize(fragment.tangent.xyz);
        let bitangent = cross(normal, tangent);
        // (its handedness from the image's own way up on the surface, not the tangent's w: her
        // export's w was reversed on two thirds of her skin, and every baked relief, pores,
        // creases, her navel and nipples, was lit upside down, ADR-820; the w where the
        // derivatives say nothing, a texel seen edge on)
        var sign = select(1.0, -1.0, fragment.tangent.w < 0.0);
        let measured = dot(bitangent, image_up);
        if (abs(measured) > 1e-12) {
            sign = select(1.0, -1.0, measured < 0.0);
        }
        normal = safe_normalize(tangent * local.x + sign * bitangent * local.y + normal * local.z);
    }
    let view = safe_normalize(scene.eye.xyz - fragment.world);
    // an eye (ADR-796), as the CPU's `fill_triangle`: under the cornea the texture is read where
    // the bent sight line meets the iris, the pupil is drawn there, and the highlights are the
    // cornea's; the light still falls on the surface's own normal
    var curvature = fragment.curvature;
    var diffuse_normal = normal;
    var under_cornea = vec4<f32>(0.0);
    var mirror_surface = surface_normal;
    if (draw.flags.x == 6u && draw.eye_centres[0].w > 0.0) {
        let seen = eye_seen(fragment.eye_centre, fragment.eye_axis, fragment.eye_up, fragment.world, view, fragment.uv, normal, curvature, max(length(world_dx), length(world_dy)));
        if (seen.through) {
            let under = straight(textureSampleGrad(base_texture, texture_sampler, seen.uv, uv_dx, uv_dy), (draw.flags.y & 2u) != 0u);
            let under_texel = select(vec4<f32>(1.0), under, (draw.flags.y & 1u) != 0u);
            base = srgb_to_linear(under_texel.rgb) * draw.color.rgb * (1.0 - seen.pupil) * (1.0 - 0.85 * seen.ruff) * seen.limbus;
            normal = seen.normal;
            mirror_surface = seen.normal;
            diffuse_normal = seen.iris_normal;
            under_cornea = vec4<f32>(seen.axis, 1.0 / max(draw.eye_up.w, 1.0));
            curvature = seen.curvature;
        } else {
            // the white: reflecting as the ball; the light still falls on the mesh's normal
            // (ADR-804)
            normal = seen.normal;
            mirror_surface = seen.normal;
            // (and the limbal ring's edge over the white beside the iris)
            base = base * seen.limbus;
        }
    }
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
    var lit = tone(shade(base, normal, view, fragment.world, surface_normal, occlusion, turn, strand, curvature, specular_kept, diffuse_normal, mirror_surface, under_cornea), scene.params.x);
    // a see-through draw (ADR-842: `:see-through`, `DrawU::material.z` on any surface but
    // `:fibres`): opaque, its colour mixed by the share toward what was drawn behind it at this
    // pixel before the see-through draws (never another of them: the colour pass is split), both
    // as displayed, decoded to light to be mixed; none while its stretch is shown
    var see_through = select(draw.material.z, 0.0, (draw.flags.y & 512u) != 0u || stretch_view);
    // (and a knit's own: its opened loops over the skin it lies on, ADR-842)
    if (!stretch_view) {
        see_through = max(see_through, knit_through(fragment, surface_normal));
    }
    if (see_through > 0.0) {
        let behind = textureLoad(behind_picture, vec2<i32>(fragment.clip.xy), 0).rgb;
        lit = linear_to_srgb(mix(srgb_to_linear(lit), srgb_to_linear(behind), see_through));
    }
    return vec4<f32>(lit * alpha, alpha);
}

struct HairDepth {
    @builtin(sample_mask) mask: u32,
};

// the hair's depth pass (ADR-802): the samples a `:hair` or `:strands` fragment covers, as
// `fs_mesh` decides them, written to depth alone, before the colour pass
@fragment
fn fs_hair_depth(fragment: VertexOut) -> HairDepth {
    let texel = straight(textureSample(base_texture, texture_sampler, fragment.uv), (draw.flags.y & 2u) != 0u);
    let mask = hair_mask(select(vec4<f32>(1.0), texel, (draw.flags.y & 1u) != 0u), fragment);
    if (mask == 0u) {
        discard;
    }
    return HairDepth(mask);
}

// the hair's colour after its depth pass (ADR-802): the depth test (equal) keeps exactly the
// samples the depth pass wrote for the hair in front, so no mask and no `discard` here, and
// the samples behind are never shaded
@fragment
fn fs_hair_front(fragment: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let base_texel = straight(textureSample(base_texture, texture_sampler, fragment.uv), (draw.flags.y & 2u) != 0u);
    let normal_texel = straight(textureSample(normal_texture, texture_sampler, fragment.uv), (draw.flags.y & 8u) != 0u);
    let occlusion_texel = straight(textureSample(occlusion_texture, texture_sampler, fragment.uv), (draw.flags.y & 32u) != 0u);
    let detail_texel = straight(textureSample(detail_texture, texture_sampler, detail_uv(fragment)), (draw.flags.y & 128u) != 0u);
    let texel = select(vec4<f32>(1.0), base_texel, (draw.flags.y & 1u) != 0u);
    let uv_dx = dpdx(fragment.uv);
    let uv_dy = dpdy(fragment.uv);
    if (HAIR_TRIAL == 1u) {
        return vec4<f32>(linear_to_srgb(srgb_to_linear(texel.rgb) * draw.color.rgb), 1.0);
    }
    return mesh_lit(fragment, front, texel, normal_texel, occlusion_texel, detail_texel, uv_dx, uv_dy, 1.0);
}

// ADR-813 (`fibres.rs`): a `:fibres` strand's three nearest depths a pixel, each kept as
// 0xFFFFFFFF − its bits, so the nearest is the largest and a cleared slot (0) is empty
@group(0) @binding(6) var<storage, read_write> fibre_layers: array<atomic<u32>>;

// how much of its pixel a `:fibres` fragment covers: a pixel-wide box against the strand's real
// width, from the fragment's distance off the strand's middle (in pixels), times its colour's alpha
fn fibre_coverage(fragment: VertexOut) -> f32 {
    let off = fragment.uv.x * fragment.strand_px.y;
    let half = 0.5 * fragment.strand_px.x;
    let covered = clamp(min(off + 0.5, half) - max(off - 0.5, -half), 0.0, 1.0);
    return covered * draw.color.a;
}

// the fibres' layer pass: the fragment's depth among its pixel's three nearest, and its
// coverage multiplied out of the pixel's transmittance
@fragment
fn fs_fibre_layers(fragment: VertexOut) -> @location(0) vec4<f32> {
    let coverage = fibre_coverage(fragment);
    if (coverage < 0.002) {
        return vec4<f32>(1.0);
    }
    let pixel = vec2<u32>(floor(fragment.clip.xy));
    let base = (pixel.y * u32(scene.picture.x) + pixel.x) * 3u;
    // (its depth, nearer larger, in the top 24 bits and its coverage in the low 8: what the front
    // pass composites the three nearest by, front to back)
    let coverage_byte = u32(clamp(coverage, 0.0, 1.0) * 255.0 + 0.5);
    var kept = ((0xFFFFFFFFu - bitcast<u32>(fragment.clip.z)) & 0xFFFFFF00u) | max(coverage_byte, 1u);
    for (var slot = 0u; slot < 3u; slot = slot + 1u) {
        let before = atomicMax(&fibre_layers[base + slot], kept);
        if (before == 0u) {
            break;
        }
        kept = min(kept, before);
    }
    return vec4<f32>(1.0 - coverage);
}

// the fibres' front pass: shaded only at or before the pixel's third nearest depth (the depth
// test, early: no `discard` here), their colour and coverage added up
@fragment
fn fs_fibre_front(fragment: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let base_texel = straight(textureSample(base_texture, texture_sampler, fragment.uv), (draw.flags.y & 2u) != 0u);
    let normal_texel = straight(textureSample(normal_texture, texture_sampler, fragment.uv), (draw.flags.y & 8u) != 0u);
    let occlusion_texel = straight(textureSample(occlusion_texture, texture_sampler, fragment.uv), (draw.flags.y & 32u) != 0u);
    let detail_texel = straight(textureSample(detail_texture, texture_sampler, detail_uv(fragment)), (draw.flags.y & 128u) != 0u);
    let texel = select(vec4<f32>(1.0), base_texel, (draw.flags.y & 1u) != 0u);
    let uv_dx = dpdx(fragment.uv);
    let uv_dy = dpdy(fragment.uv);
    let coverage = fibre_coverage(fragment);
    let lit = mesh_lit(fragment, front, texel, normal_texel, occlusion_texel, detail_texel, uv_dx, uv_dy, 1.0);
    // composited front to back among the pixel's three nearest: weighed by its coverage and by
    // what the nearer of them let through (averaged alike, the front strand counted no more than
    // those behind it, and hair read over-blended)
    let pixel = vec2<u32>(floor(fragment.clip.xy));
    let base = (pixel.y * u32(scene.picture.x) + pixel.x) * 3u;
    let own = (0xFFFFFFFFu - bitcast<u32>(fragment.clip.z)) & 0xFFFFFF00u;
    var through = 1.0;
    for (var slot = 0u; slot < 3u; slot = slot + 1u) {
        let stored = atomicLoad(&fibre_layers[base + slot]);
        if (stored == 0u) {
            break;
        }
        if ((stored & 0xFFFFFF00u) > own) {
            through = through * (1.0 - f32(stored & 0xFFu) / 255.0);
        }
    }
    let weight = coverage * through;
    return vec4<f32>(lit.rgb * weight, weight);
}

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // (the shadow view it is drawn for: the hair's opacity pass reads that view's nearest depth)
    @location(1) @interpolate(flat) view: u32,
};

@vertex
fn vs_shadow(input: VertexIn, @builtin(instance_index) view_index: u32) -> ShadowOut {
    // a pressed draw (ADR-805): its vertex as the contact passes posed and pressed it
    if (draw.deform.x == 1u) {
        var pressed: ShadowOut;
        pressed.clip = scene.shadows[view_index].view_proj * vec4<f32>(posed_vertices[(draw.deform.y + input.index) * 3u].xyz, 1.0);
        pressed.uv = input.uv;
        pressed.view = view_index;
        return pressed;
    }
    let posed = pose(input);
    var out: ShadowOut;
    var world = (posed.matrix * vec4<f32>(posed.position, 1.0)).xyz;
    if (draw.flags.x == 5u) {
        // the ribbon as the camera sees it, as the CPU painter's posed vertices are
        let clamped = clamped_strand(world, safe_normalize((posed.matrix * vec4<f32>(posed.normal, 0.0)).xyz));
        // (a `:fibres` strand standing for `clamp_info.z` of them, as many times as wide)
        world = strand_vertex(clamped.centre, clamped.direction, input.uv, draw.strand.xy * max(draw.clamp_info.z, 1.0)).world;
    }
    out.clip = scene.shadows[view_index].view_proj * vec4<f32>(world, 1.0);
    out.uv = input.uv;
    out.view = view_index;
    return out;
}

// a `:cutout`, `:blend` or `:hair` surface casts only where its alpha (read at the texture's
// own size, not a mip level) reaches its cutoff, as the CPU's `render_shadow_map` tests it
@fragment
fn fs_shadow(fragment: ShadowOut) {
    let base_texel = straight(textureSampleLevel(base_texture, texture_sampler, fragment.uv, 0.0), (draw.flags.y & 2u) != 0u);
    let alpha = select(1.0, base_texel.a, (draw.flags.y & 1u) != 0u) * draw.color.a;
    if ((draw.flags.x == 2u || draw.flags.x == 3u || draw.flags.x == 4u || draw.flags.x == 5u || draw.flags.x == 6u) && alpha < draw.material.w) {
        discard;
    }
}

// ADR-827, the hair's opacity maps (deep opacity maps, Yuksel and Keyser 2008): a `:fibres`
// strand blocks only part of the light it crosses, so it is not in the depth maps at all (as a
// solid it stamped dark blotches on thin-hair skin; kept at random, Enderton et al. 2010, each
// kept stamp was still hard). First its nearest depth from the light, every fragment
// (`fs_hair_opacity_front`, alpha-tested as `fs_shadow`), then each fragment's density added into
// the slices behind that depth it lies before (`fs_hair_opacity`): `-ln(1 - :shadow-keep)` a
// fragment, so light through n of them is `(1 - keep)^n`, the share `:shadow-keep` always meant
@fragment
fn fs_hair_opacity_front(fragment: ShadowOut) {
    let base_texel = straight(textureSampleLevel(base_texture, texture_sampler, fragment.uv, 0.0), (draw.flags.y & 2u) != 0u);
    let alpha = select(1.0, base_texel.a, (draw.flags.y & 1u) != 0u) * draw.color.a;
    if (alpha < draw.material.w) {
        discard;
    }
}

@fragment
fn fs_hair_opacity(fragment: ShadowOut) -> @location(0) vec4<f32> {
    let base_texel = straight(textureSampleLevel(base_texture, texture_sampler, fragment.uv, 0.0), (draw.flags.y & 2u) != 0u);
    let alpha = select(1.0, base_texel.a, (draw.flags.y & 1u) != 0u) * draw.color.a;
    if (alpha < draw.material.w) {
        discard;
    }
    let view = scene.shadows[fragment.view];
    let perspective = view.map.w > 0.5;
    let nearest = textureLoad(hair_nearest, vec2<i32>(fragment.clip.xy), i32(fragment.view), 0);
    let beyond = view_distance(view, perspective, fragment.clip.z) - view_distance(view, perspective, nearest);
    let density = -log(1.0 - clamp(draw.clamp_info.w, 0.0, 0.99));
    return density * select(vec4<f32>(0.0), vec4<f32>(1.0), vec4<f32>(beyond) < HAIR_SLICES);
}
"#;
