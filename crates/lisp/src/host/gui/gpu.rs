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
//! blending otherwise matches: text and translucent geometry are drawn through an sRGB view
//! of the frame and blend in linear light, text with its alpha corrected to the encoded
//! lerp's weight and lifted by the contrast setting (`fs_tex_linear`, the CPU's `TextInk`,
//! ADR-611).
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
//!
//! The children: `shaders` (the WGSL), `pipelines` (the shared pipeline builder and blend
//! states), `setup` (surface, device and pipeline assembly), `instances` (the growable
//! instance buffers and their diff), `frame_builder` (the frame's quads and batches),
//! `atlas` (the glyph atlas), `run_cache` and `fragment_cache` (the reuse), `text_runs`
//! (shaping a text run), `expand` (the op walk), `passes` (the render passes), `effects`
//! (lights, post-processing and particles) and `frame_dump` (the readback).

mod atlas;
mod effects;
mod expand;
mod fragment_cache;
mod frame_builder;
mod frame_dump;
mod instances;
mod passes;
mod pipelines;
mod run_cache;
mod setup;
mod shaders;
mod text_runs;

#[cfg(test)]
mod reuse_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use setup::hardware_adapters;

use atlas::*;
use effects::*;
use expand::*;
use fragment_cache::*;
use frame_builder::*;
use frame_dump::*;
use instances::*;
use passes::*;
use pipelines::*;
use run_cache::*;
#[cfg(test)]
use setup::*;
use shaders::*;

use std::collections::HashMap;
use std::rc::Rc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

use unicode_segmentation::UnicodeSegmentation;

use crate::host::gui::backend::{
    CURSOR_FG, Renderer, RowMap, caret_cells, caret_glyph, cursor_rim, holds_blinking_caret,
    luminance, paint_stall_ms, snap_hairline, text_px_width, text_units, unit_cells,
};
use crate::host::gui::effects::{EmitterStep, FrameEffects};
use crate::host::gui::{Align, CursorStyle, Face, MAX_PARTICLES, Op};
// A raw tab advances to the next SCREEN stop of the fixed `TAB_WIDTH`, as the CPU painter's
// does (`backend.rs`): the painter cannot read a program's `*tab-width*` (ADR-537), so a
// program that wants its own width expands tabs before it paints (`string/expand-tabs`).
use crate::host::text_width::{TAB_WIDTH, cluster_cells, cluster_cells_at};

/// Which texture a run of textured quads samples: a glyph-atlas page, or a texture the
/// app uploaded for its sprites.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TexKey {
    Atlas(usize),
    User(u32),
    /// A picture an `[:image …]` op drew, at one size (`PictureTextures`, ADR-686).
    Picture(u32),
}

/// A texture the app uploaded, bound for the textured pipeline.
struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

/// The textures `[:image …]` ops drew (ADR-686): each picture uploaded once per size it is
/// drawn at — already resampled to exactly those pixels, so the nearest sampler maps texel
/// to pixel — and the least recently used freed past a byte budget. A memoised fragment
/// replaying a picture's quads does not touch its entry, so the budget, not a frame's use,
/// decides what goes; a free bumps `texture_generation`, which makes every fragment expand
/// (and touch its pictures) again.
#[derive(Default)]
struct PictureTextures {
    textures: HashMap<u32, GpuTexture>,
    by_size: HashMap<([u8; 16], u32, u32), (u32, u64)>,
    bytes: usize,
    clock: u64,
    next_id: u32,
}

/// What one window keeps uploaded for its pictures, in bytes.
const PICTURE_TEXTURE_BUDGET: usize = 128 * 1024 * 1024;

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
    /// The text pipelines into the sRGB view (`fs_tex_linear`, `fs_tex_lcd_linear` …): glyphs
    /// blend in linear light with the encoded lerp's weight, as the CPU painter's `TextInk`
    /// (ADR-611). `None` with no sRGB view, and then text blends in the encoded space.
    tex_linear_pipeline: Option<wgpu::RenderPipeline>,
    lcd_linear: Option<LcdText>,
    tex_layout: wgpu::BindGroupLayout,
    tex_instances: InstanceBuffer,
    sampler: wgpu::Sampler,
    atlas: Vec<AtlasPage>,
    /// Cluster+face+px key → its atlas slot; `None` for a cluster that rasterised to
    /// nothing (or was too big for a page), remembered so it is not retried each frame.
    glyphs: HashMap<u64, Option<AtlasGlyph>>,
    textures: HashMap<u32, GpuTexture>,
    /// The pictures `[:image …]` ops drew, each uploaded once per size it is drawn at.
    pictures: PictureTextures,
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

    /// The texture of `picture` at `width`×`height` for an `[:image …]` op (ADR-686): the
    /// one uploaded before at that size, or the picture resampled (through the renderer's
    /// `ScaledPictures`, which the CPU painter shares) and uploaded now. `None` past the
    /// device's largest texture.
    fn picture_texture(
        &mut self,
        renderer: &mut Renderer,
        picture: &std::sync::Arc<crate::host::picture::Picture>,
        width: u32,
        height: u32,
    ) -> Option<u32> {
        let largest = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > largest || height > largest {
            return None;
        }
        let pictures = &mut self.pictures;
        pictures.clock += 1;
        if let Some((id, used)) = pictures.by_size.get_mut(&(picture.key, width, height)) {
            *used = pictures.clock;
            return Some(*id);
        }
        // not resampled to this size yet (ADR-695): the stand-in's texture, at its own size —
        // the quad is drawn at the asked size, so the sampler stretches it until the exact
        // pixels land and `pictures_landed` makes every fragment expand again
        let scaled = renderer.pictures.get(picture, width, height);
        let scaled = scaled.image();
        let (width, height) = (scaled.width(), scaled.height());
        if width > largest || height > largest {
            return None;
        }
        let key = (picture.key, width, height);
        if let Some((id, used)) = self.pictures.by_size.get_mut(&key) {
            *used = self.pictures.clock;
            return Some(*id);
        }
        let (texture, bind_group) = self.create_texture(width, height);
        self.write_rgba(&texture, 0, 0, scaled.as_raw(), width, height);
        let pictures = &mut self.pictures;
        let id = pictures.next_id;
        pictures.next_id = pictures.next_id.wrapping_add(1);
        pictures.textures.insert(id, GpuTexture { bind_group });
        pictures.by_size.insert(key, (id, pictures.clock));
        pictures.bytes += width as usize * height as usize * 4;
        while pictures.bytes > PICTURE_TEXTURE_BUDGET && pictures.by_size.len() > 1 {
            let Some((oldest, (gone, _))) = pictures
                .by_size
                .iter()
                .filter(|(entry, _)| **entry != key)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(entry, value)| (*entry, *value))
            else {
                break;
            };
            pictures.by_size.remove(&oldest);
            pictures.textures.remove(&gone);
            pictures.bytes -= oldest.1 as usize * oldest.2 as usize * 4;
        }
        self.texture_generation += 1;
        Some(id)
    }

    /// `gui-texture-free`: drop texture `tex` (a later `:sprite` naming it draws nothing).
    pub fn free_texture(&mut self, tex: u32) {
        self.textures.remove(&tex);
        self.texture_generation += 1;
    }

    /// A picture resampled off the GUI thread landed (ADR-695): a memoised fragment that drew
    /// its stand-in's texture expands again, and finds the exact size.
    pub fn pictures_landed(&mut self) {
        self.texture_generation += 1;
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
        let (ox, _) = renderer.grid_origin(fw as usize, fh as usize);
        let grid = Grid {
            ox: ox as f32,
            rows: renderer.rows(fw as usize, fh as usize),
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
            // passes (more than one only where an app's textures or particles and text interleave); lcd:
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
