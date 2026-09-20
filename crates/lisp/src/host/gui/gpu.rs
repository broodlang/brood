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
//! one draw per glyph. The swapchain presents without vsync unless the window asked for
//! it, so a sim's frame rate is bounded by work, not the monitor refresh.
//!
//! The op walk mirrors the CPU painter's (`paint::render_ops`) arm for arm — the same
//! grid origin, the same scroll shift, the same region metrics and clip band, the same
//! cursor geometry — so a frame lands on the same pixels whichever target paints it. A
//! rounded or sub-cell rect is a signed-distance field in the fragment shader, the same
//! 1 px coverage ramp `fill_rrect` computes per pixel; a clip band is a per-instance rect
//! the fragment shader discards outside of. The one visible difference is text
//! anti-aliasing: the GPU samples one coverage per pixel (grey AA) where the CPU path
//! can render subpixel (LCD) text at 1×, and the contrast lift is not applied.
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

use crate::host::gui::backend::{snap_hairline, Renderer, CURSOR_FG};
use crate::host::gui::{CursorStyle, Op};
use crate::host::text_width::{cluster_cells, cluster_cells_at, TAB_WIDTH};

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
struct Viewport { size: vec2<f32>, _pad: vec2<f32> };
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

@fragment
fn fs_solid(in: SolidOut) -> @location(0) vec4<f32> {
    if (clipped(in.pos.xy, in.clip)) {
        discard;
    }
    if (in.aa == 0.0) {
        return in.color;
    }
    // The rounded-box signed distance from the pixel centre to the rect's rounded core
    // (the rect inset by the radius), as `fill_rrect` computes it: a 1 px coverage ramp
    // at the edge, so corners and fractional edges read smooth.
    let p = in.local - in.half;
    let r = min(in.radius, min(in.half.x, in.half.y));
    let q = abs(p) - (in.half - vec2<f32>(r, r));
    let d = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}

// --- textured quads ------------------------------------------------------------------

struct TexInst {
    @location(0) rect: vec4<f32>,   // x, y, w, h in pixels
    @location(1) uv: vec4<f32>,     // u0, v0, du, dv in texture space (a negative extent mirrors)
    @location(2) tint: vec4<f32>,   // straight rgba, 0..1
    @location(3) mode: u32,         // 1 = coverage mask recoloured with tint, 0 = colour
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
    if (in.mode == 1u) {
        return vec4<f32>(in.tint.rgb, t.a * in.tint.a);
    }
    return t * in.tint;
}
"#;

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
#[derive(Clone, Copy, PartialEq, Eq)]
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
/// coverage mask (`mode` 1 in the shader) rather than a colour bitmap.
#[derive(Clone, Copy)]
struct AtlasGlyph {
    page: usize,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    mono: bool,
}

/// A growable instance buffer: re-created (never mapped) when a frame outgrows it.
struct InstanceBuffer {
    buffer: wgpu::Buffer,
    capacity_bytes: u64,
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
        }
    }

    /// Upload `data`, growing (doubling) the buffer first when it does not fit.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, data: &[u8]) {
        let needed = data.len() as u64;
        if needed > self.capacity_bytes {
            let mut capacity = self.capacity_bytes.max(1);
            while capacity < needed {
                capacity *= 2;
            }
            *self = InstanceBuffer::new(device, label, capacity);
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, 0, data);
        }
    }
}

/// A run of consecutive quads drawn with one pipeline state: `count` instances starting
/// at `first` in that pipeline's instance buffer.
enum Batch {
    Solid { first: u32, count: u32 },
    Tex { key: TexKey, first: u32, count: u32 },
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
        match self.batches.last_mut() {
            Some(Batch::Solid { count, .. }) => *count += 1,
            _ => self.batches.push(Batch::Solid {
                first: index,
                count: 1,
            }),
        }
    }

    /// A plain opaque solid — the common cell-aligned fill.
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
        self.solid(x, y, w, h, [rgb[0], rgb[1], rgb[2], 255], 0.0, Edge::Plain);
    }

    /// A textured quad sampling `uv` (`[u0 v0 du dv]`, texture space) of `key`.
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
        mono: bool,
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
            u32::from(mono),
            rot.to_bits(),
            self.clip[0].to_bits(),
            self.clip[1].to_bits(),
            self.clip[2].to_bits(),
            self.clip[3].to_bits(),
        ]);
        match self.batches.last_mut() {
            Some(Batch::Tex { key: k, count, .. }) if *k == key => *count += 1,
            _ => self.batches.push(Batch::Tex {
                key,
                first: index,
                count: 1,
            }),
        }
    }
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

/// One open window's GPU state: the wgpu surface + device, the two instanced-quad
/// pipelines, the glyph atlas and the app's textures. Lives on the GUI thread. Field
/// order matters: the surface was created from the window's raw handles, so it must
/// drop before the window — Rust drops fields in declaration order.
pub struct GpuWindow {
    surface: wgpu::Surface<'static>,
    window: Rc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    configured: (u32, u32),
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    solid_pipeline: wgpu::RenderPipeline,
    solid_instances: InstanceBuffer,
    tex_pipeline: wgpu::RenderPipeline,
    tex_layout: wgpu::BindGroupLayout,
    tex_instances: InstanceBuffer,
    sampler: wgpu::Sampler,
    atlas: Vec<AtlasPage>,
    /// Cluster+face+px key → its atlas slot; `None` for a cluster that rasterised to
    /// nothing (or was too big for a page), remembered so it is not retried each frame.
    glyphs: HashMap<u64, Option<AtlasGlyph>>,
    textures: HashMap<u32, GpuTexture>,
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

/// The present mode: with `vsync`, FIFO (always available, and IS vsync). Without, the
/// platform's immediate/mailbox mode when it has one, else FIFO — the only mode some
/// compositors offer, so then the choice is theirs, not ours.
fn pick_present_mode(modes: &[wgpu::PresentMode], vsync: bool) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::Fifo
    } else if modes.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else if modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    }
}

/// A scroll region's shift in pixels for a `dy_frac` of the cell height, capped — the
/// CPU painter's `scroll_px`.
fn scroll_px(dy_frac: f32, ch: f32) -> f32 {
    (dy_frac * ch).round().clamp(-MAX_SCROLL_PX, MAX_SCROLL_PX)
}

/// The cursor's quads for one cell: `(x, y, w, h, colour)` each. Block is a 50% white
/// overlay plus a solid rim `t` thick; Bar a caret on the left edge; Underline a rule
/// along the bottom — the CPU `cursor_cell`'s geometry, thickness scaled with the cell so
/// it stays proportional on HiDPI (never under 2 px).
fn cursor_quads(
    left: f32,
    top: f32,
    cw: f32,
    ch: f32,
    style: CursorStyle,
) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
    let fg = [CURSOR_FG[0], CURSOR_FG[1], CURSOR_FG[2], 255];
    match style {
        CursorStyle::Block => {
            let t = (cw / 10.0).floor().max(2.0);
            vec![
                (left, top, cw, ch, [255, 255, 255, 128]),
                (left, top, t, ch, fg),
                (left + cw - t, top, t, ch, fg),
                (left, top, cw, t, fg),
                (left, top + ch - t, cw, t, fg),
            ]
        }
        CursorStyle::Bar => vec![(left, top, (cw / 8.0).floor().max(2.0), ch, fg)],
        CursorStyle::Underline => {
            let t = (ch / 10.0).floor().max(2.0);
            vec![(left, top + ch - t, cw, t, fg)]
        }
    }
}

impl GpuWindow {
    pub fn new(window: Rc<Window>, vsync: bool) -> Result<GpuWindow, String> {
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
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("brood gui"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("gpu device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = pick_format(&caps.formats).ok_or("gpu surface: no supported format")?;
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
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui quads"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
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
        let target = [Some(wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        };

        let solid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("solid"),
            bind_group_layouts: &[Some(&viewport_layout)],
            immediate_size: 0,
        });
        let solid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("solid quads"),
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
                entry_point: Some("fs_solid"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            multiview_mask: None,
            cache: None,
        });

        let tex_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("textured"),
            bind_group_layouts: &[Some(&viewport_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let tex_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("textured quads"),
            layout: Some(&tex_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
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
                module: &shader,
                entry_point: Some("fs_tex"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            multiview_mask: None,
            cache: None,
        });

        let solid_instances = InstanceBuffer::new(&device, "solid instances", 64 * 1024);
        let tex_instances = InstanceBuffer::new(&device, "textured instances", 64 * 1024);

        Ok(GpuWindow {
            surface,
            window,
            device,
            queue,
            config,
            configured: (w, h),
            viewport_buffer,
            viewport_bind_group,
            solid_pipeline,
            solid_instances,
            tex_pipeline,
            tex_layout,
            tex_instances,
            sampler,
            atlas: Vec::new(),
            glyphs: HashMap::new(),
            textures: HashMap::new(),
        })
    }

    /// Match the swapchain to the current window size (called before each paint).
    pub fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        if self.configured != (w, h) {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
            self.configured = (w, h);
        }
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
    }

    /// `gui-texture-free`: drop texture `tex` (a later `:sprite` naming it draws nothing).
    pub fn free_texture(&mut self, tex: u32) {
        self.textures.remove(&tex);
    }

    /// Pack a rasterised `w`×`h` cluster into the atlas, opening a new shelf or page as
    /// needed, and return where it landed. `None` for a glyph larger than a page.
    fn pack_glyph(&mut self, rgba: &[u8], w: u32, h: u32, mono: bool) -> Option<AtlasGlyph> {
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
            });
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
        let key = glyph_key(g, family, bold, italic, scale, renderer.metrics().px);
        if let Some(slot) = self.glyphs.get(&key) {
            return *slot;
        }
        let cg = renderer.cluster_glyph(g, family, bold, italic, scale);
        let (rgba, w, h, color) = (cg.rgba.clone(), cg.width as u32, cg.height as u32, cg.color);
        let packed = self.pack_glyph(&rgba, w, h, !color);
        self.glyphs.insert(key, packed);
        packed
    }

    /// Expand `ops` into quads on `fb`, in order, on grid `g` — the CPU painter's
    /// `render_ops`, arm for arm. Recursion is the two region ops: a scroll region
    /// re-enters with its shift (and clips at the grid top, as the CPU path clips every
    /// shifted op there); a cell region re-enters with its own metrics, origin and clip
    /// band, restoring the renderer's metrics after.
    fn expand(&mut self, fb: &mut FrameBuilder, renderer: &mut Renderer, ops: &[Op], g: Grid) {
        let atlas_size = ATLAS_SIZE as f32;
        let scale_factor = renderer.scale() as f32;
        for op in ops {
            match op {
                // The frame was cleared to the background before any op.
                Op::Clear => {}
                Op::ScrollRegion { dy_frac, ops } => {
                    let saved_clip = fb.clip;
                    // A shifted op is clipped at the grid origin, never painted into the
                    // inset above it — the CPU path's `clip_skip`.
                    fb.clip[1] = fb.clip[1].max(g.oy);
                    let inner = Grid {
                        scroll_dy: scroll_px(*dy_frac, g.ch),
                        ..g
                    };
                    self.expand(fb, renderer, ops, inner);
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
                        self.expand(fb, renderer, ops, inner);
                        renderer.set_metrics(saved);
                    }
                    fb.clip = saved_clip;
                }
                // Text: the cell BACKGROUNDS as solid quads (a coloured Life cell is a
                // space + `:bg`) — all of the op's cells first, so they form one solid
                // batch — then a textured quad per non-space cluster out of the atlas,
                // which together form one textured batch, then any underline.
                Op::Text { row, col, s, face } => {
                    let (mut fg, mut bg) = (
                        face.fg.unwrap_or(DEFAULT_FG),
                        face.bg.unwrap_or(DEFAULT_BG_RGB),
                    );
                    // A face with no `:bg` is transparent: the glyph composites over
                    // whatever is under it (an hl-line band), exactly like Emacs.
                    let mut paint_bg = face.bg.is_some();
                    if face.reverse {
                        std::mem::swap(&mut fg, &mut bg);
                        paint_bg = true;
                    }
                    let scale = face.scale.max(1) as usize;
                    let ch_s = scale as f32 * g.ch;
                    let top = g.oy + *row as f32 * g.ch - g.scroll_dy;
                    let mut clusters: Vec<(f32, f32, &str)> = Vec::new();
                    let mut cx = *col as usize;
                    for cluster in s.graphemes(true) {
                        // A raw tab advances to the next SCREEN stop, background only.
                        if cluster == "\t" {
                            let cells = cluster_cells_at(cluster, cx, TAB_WIDTH);
                            if paint_bg {
                                fb.fill(
                                    g.ox + cx as f32 * g.cw,
                                    top,
                                    cells as f32 * g.cw,
                                    ch_s,
                                    bg,
                                );
                            }
                            cx += cells;
                            continue;
                        }
                        let cells = cluster_cells(cluster);
                        if cells == 0 {
                            continue;
                        }
                        let block_w = (cells * scale) as f32 * g.cw;
                        let left = g.ox + cx as f32 * g.cw;
                        if paint_bg {
                            fb.fill(left, top, block_w, ch_s, bg);
                        }
                        if cluster != " " {
                            clusters.push((left, block_w, cluster));
                        }
                        cx += cells * scale;
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
                        fb.textured(
                            TexKey::Atlas(glyph.page),
                            *left,
                            top,
                            glyph.w as f32,
                            glyph.h as f32,
                            [
                                glyph.x as f32 / atlas_size,
                                glyph.y as f32 / atlas_size,
                                glyph.w as f32 / atlas_size,
                                glyph.h as f32 / atlas_size,
                            ],
                            [fg[0], fg[1], fg[2], 255],
                            glyph.mono,
                            0.0,
                        );
                    }
                    if face.underline {
                        // A rule near the block bottom in the text colour, scaled with
                        // the glyph so it stays proportional.
                        let uy = top + ch_s - 2.0 * scale as f32;
                        for (left, block_w, _) in &clusters {
                            fb.fill(*left, uy, *block_w, scale as f32, fg);
                        }
                    }
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
                Op::Cursor { row, col, style } => {
                    let left = g.ox + *col as f32 * g.cw;
                    let top = g.oy + *row as f32 * g.ch - g.scroll_dy;
                    for (x, y, w, h, rgba) in cursor_quads(left, top, g.cw, g.ch, *style) {
                        fb.solid(x, y, w, h, rgba, 0.0, Edge::Plain);
                    }
                }
                // Hover metadata, hit-tested in the event loop (ADR-080); nothing to draw.
                Op::CursorZone { .. } => {}
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
                        fb.textured(TexKey::User(*tex), *x, *y, *w, *h, *uv, *tint, false, *rot);
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
            }
        }
    }

    /// Draw one frame: expand the ops to quads in op order, upload, draw the batches,
    /// present. The cell pixel size and the grid origin come from the renderer — the
    /// same coordinate contract the CPU `paint` uses, so cell ops land on the same pixels
    /// whichever target paints them.
    pub(crate) fn paint(&mut self, frame: &[Op], renderer: &mut Renderer) {
        let (cw, ch) = (renderer.cell_w.max(1), renderer.cell_h.max(1));
        let size = self.window.inner_size();
        let (fw, fh) = (size.width.max(1), size.height.max(1));
        self.resize(fw, fh);

        let (fwf, fhf) = (fw as f32, fh as f32);
        let mut fb = FrameBuilder::new(fwf, fhf);
        // The grid origin, not the bare inset: the CPU painter centres the vertical
        // remainder (the rows that do not divide into whole cells), and a frame must land
        // on the same pixels whichever target paints it.
        let (ox, oy) = renderer.grid_origin(fw as usize, fh as usize);
        let grid = Grid {
            ox: ox as f32,
            oy: oy as f32,
            cw: cw as f32,
            ch: ch as f32,
            scroll_dy: 0.0,
        };
        self.expand(&mut fb, renderer, frame, grid);

        self.queue
            .write_buffer(&self.viewport_buffer, 0, as_bytes(&[fwf, fhf, 0.0, 0.0]));
        self.solid_instances.upload(
            &self.device,
            &self.queue,
            "solid instances",
            as_bytes(&fb.solids),
        );
        self.tex_instances.upload(
            &self.device,
            &self.queue,
            "textured instances",
            as_bytes_u32(&fb.texs),
        );

        let frame_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                // The swapchain went stale (a resize the OS has not told winit about yet,
                // a display change): reconfigure and skip this frame; the next paint draws.
                self.surface.configure(&self.device, &self.config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        let view = frame_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let bg = renderer.bg();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64 / 255.0,
                            g: bg[1] as f64 / 255.0,
                            b: bg[2] as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.viewport_bind_group, &[]);
            for batch in &fb.batches {
                match batch {
                    Batch::Solid { first, count } => {
                        pass.set_pipeline(&self.solid_pipeline);
                        pass.set_vertex_buffer(0, self.solid_instances.buffer.slice(..));
                        pass.draw(0..4, *first..first + count);
                    }
                    Batch::Tex { key, first, count } => {
                        let bind_group = match key {
                            TexKey::Atlas(page) => self.atlas.get(*page).map(|p| &p.bind_group),
                            TexKey::User(tex) => self.textures.get(tex).map(|t| &t.bind_group),
                        };
                        let Some(bind_group) = bind_group else {
                            continue;
                        };
                        pass.set_pipeline(&self.tex_pipeline);
                        pass.set_vertex_buffer(0, self.tex_instances.buffer.slice(..));
                        pass.set_bind_group(1, bind_group, &[]);
                        pass.draw(0..4, *first..first + count);
                    }
                }
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        if let Some(path) = dump_path() {
            dump_frame(
                &self.device,
                &self.queue,
                &frame_texture.texture,
                self.config.format,
                fw,
                fh,
                path,
            );
        }
        self.queue.present(frame_texture);
    }
}

/// The atlas key of a cluster under a face at a font size: what it is drawn from depends
/// on the cluster, the family, the weight/slant, the scale and the pixel size (a cell
/// region rasterises at its own px) — not the colour, which is a per-instance tint.
fn glyph_key(g: &str, family: Option<u32>, bold: bool, italic: bool, scale: u16, px: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    g.hash(&mut h);
    family.hash(&mut h);
    bold.hash(&mut h);
    italic.hash(&mut h);
    scale.hash(&mut h);
    px.to_bits().hash(&mut h);
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

/// Copy `texture` (the frame just drawn, `w`×`h`, `format`) into a mapped buffer and
/// write it to `path` as a PPM. Rows are padded to wgpu's 256-byte copy alignment and
/// unpadded on the way out; a BGRA surface is swizzled to RGB.
fn dump_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    path: &str,
) {
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
        return;
    }
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let Ok(data) = slice.get_mapped_range() else {
        return;
    };
    let mut out = Vec::with_capacity((w * h * 3) as usize + 32);
    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
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
    let _ = std::fs::write(path, out);
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
            false,
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
            false,
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
            false,
            0.0,
        );
        fb.solid(40.0, 0.0, 10.0, 10.0, [0, 255, 0, 255], 0.0, Edge::Plain);
        let shape: Vec<(bool, u32, u32)> = fb
            .batches
            .iter()
            .map(|b| match b {
                Batch::Solid { first, count } => (true, *first, *count),
                Batch::Tex { first, count, .. } => (false, *first, *count),
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
        let block = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Block);
        assert_eq!(block.len(), 5);
        assert_eq!(block[0], (100.0, 50.0, 10.0, 20.0, [255, 255, 255, 128]));
        assert_eq!(block[2].0, 108.0); // the right rim at cw - t
                                       // Bar: one caret on the left edge, at least 2 px.
        let bar = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Bar);
        assert_eq!(bar.len(), 1);
        assert_eq!((bar[0].2, bar[0].3), (2.0, 20.0));
        // Underline: one rule along the cell bottom.
        let under = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Underline);
        assert_eq!(under[0].1, 68.0);
    }

    #[test]
    fn a_scroll_shift_is_whole_pixels_and_capped() {
        assert_eq!(scroll_px(0.5, 16.0), 8.0);
        assert_eq!(scroll_px(-0.26, 16.0), -4.0);
        assert_eq!(scroll_px(1.0e9, 16.0), MAX_SCROLL_PX);
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
    fn no_vsync_takes_immediate_then_mailbox_then_fifo_and_vsync_is_fifo() {
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
        assert_eq!(
            pick_present_mode(&[Fifo, Mailbox, Immediate], false),
            Immediate
        );
        assert_eq!(pick_present_mode(&[Fifo, Mailbox], false), Mailbox);
        assert_eq!(pick_present_mode(&[Fifo], false), Fifo);
        assert_eq!(pick_present_mode(&[Fifo, Mailbox, Immediate], true), Fifo);
    }
}
