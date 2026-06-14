//! GPU rendering backend for the GUI window (the `gui-gpu` feature).
//!
//! Replaces the CPU softbuffer present + per-pixel blit with an OpenGL (ES 3.0)
//! instanced-quad pipeline: every solid-fill op (Clear / Rect / VSpans / Cells) becomes
//! a coloured quad uploaded once and drawn with a single instanced draw call, then the
//! swapchain is presented. The window's swap interval is set to *immediate* (no vsync),
//! so the frame rate is bounded by work, not the monitor refresh — the two things that
//! made the CPU path slow for high-cell-count sims (Game of Life).
//!
//! Scope (prototype): solid quads only. Text (`Op::Text`) is not yet drawn — glyphs will
//! upload to a GL texture atlas in a later increment. Shaping (cosmic-text) is untouched;
//! this module only owns the *render target*. winit, input, and the draw-op protocol are
//! all unchanged.

use std::ffi::CString;
use std::num::NonZeroU32;
use std::rc::Rc;

use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext, Version};
use glutin::display::{Display, DisplayApiPreference};
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

use crate::gui::Op;

const DEFAULT_BG: [f32; 3] = [12.0 / 255.0, 12.0 / 255.0, 16.0 / 255.0];

const VERT_SRC: &str = r#"#version 300 es
layout(location = 0) in vec2 corner;   // unit quad 0..1
layout(location = 1) in vec4 rect;     // x, y, w, h in pixels (top-left origin)
layout(location = 2) in vec3 color;
uniform vec2 viewport;                 // framebuffer size in px
out vec3 v_color;
void main() {
    vec2 px = rect.xy + corner * rect.zw;
    vec2 ndc = vec2(px.x / viewport.x * 2.0 - 1.0,
                    1.0 - px.y / viewport.y * 2.0);
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_color = color;
}
"#;

const FRAG_SRC: &str = r#"#version 300 es
precision mediump float;
in vec3 v_color;
out vec4 frag;
void main() { frag = vec4(v_color, 1.0); }
"#;

/// One open window's GL state: the glutin context/surface + the glow instanced-quad
/// pipeline. Lives on the GUI thread; the context is kept current (one window).
pub struct GlWindow {
    window: Rc<Window>,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    inst_vbo: glow::Buffer,
    u_viewport: Option<glow::UniformLocation>,
}

impl GlWindow {
    /// Create a GL context + surface on an existing winit window, compile the quad
    /// pipeline, and switch the swapchain to immediate (non-vsync) present.
    pub fn new(window: Rc<Window>) -> Result<GlWindow, String> {
        let rdh = window
            .display_handle()
            .map_err(|e| format!("display handle: {e}"))?
            .as_raw();
        let rwh = window
            .window_handle()
            .map_err(|e| format!("window handle: {e}"))?
            .as_raw();

        // EGL works on both Wayland and X11/Mesa — the platforms this runtime targets.
        let display =
            unsafe { Display::new(rdh, DisplayApiPreference::Egl) }.map_err(|e| format!("egl display: {e}"))?;

        let template = ConfigTemplateBuilder::new()
            .compatible_with_native_window(rwh)
            .with_alpha_size(0)
            .build();
        let config = unsafe { display.find_configs(template) }
            .map_err(|e| format!("gl configs: {e}"))?
            .next()
            .ok_or_else(|| "no gl config".to_string())?;

        // Request GLES 3.0 (the shader version above); falls within Mesa's support.
        let ctx_attrs = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(Some(Version::new(3, 0))))
            .build(Some(rwh));
        let not_current = unsafe { display.create_context(&config, &ctx_attrs) }
            .map_err(|e| format!("gl context: {e}"))?;

        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let surf_attrs = SurfaceAttributesBuilder::<WindowSurface>::new()
            .build(rwh, NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());
        let surface = unsafe { display.create_window_surface(&config, &surf_attrs) }
            .map_err(|e| format!("gl surface: {e}"))?;

        let context = not_current
            .make_current(&surface)
            .map_err(|e| format!("make current: {e}"))?;

        // Immediate present — no vsync, so frame rate is work-bound, not refresh-bound.
        let _ = surface.set_swap_interval(&context, SwapInterval::DontWait);

        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                let c = CString::new(s).unwrap();
                display.get_proc_address(c.as_c_str()).cast()
            })
        };

        let (program, vao, inst_vbo, u_viewport) = unsafe { build_pipeline(&gl)? };

        Ok(GlWindow {
            window,
            surface,
            context,
            gl,
            program,
            vao,
            inst_vbo,
            u_viewport,
        })
    }

    /// Match the GL surface to the current window size (call on resize).
    pub fn resize(&self, w: u32, h: u32) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(w.max(1)), NonZeroU32::new(h.max(1))) {
            self.surface.resize(&self.context, w, h);
        }
    }

    /// Draw one frame: clear, expand the solid-fill ops to quads, one instanced draw,
    /// then present. `cw`/`ch` are the cell pixel size, `inset` the content margin —
    /// the same coordinate contract the CPU `paint` uses, so positions agree.
    pub fn paint(&mut self, frame: &[Op], cw: usize, ch: usize, inset: usize) {
        let size = self.window.inner_size();
        let (fw, fh) = (size.width.max(1), size.height.max(1));
        self.resize(fw, fh);

        let mut insts: Vec<f32> = Vec::new();
        let mut push = |x: f32, y: f32, w: f32, h: f32, c: [u8; 3]| {
            insts.extend_from_slice(&[
                x,
                y,
                w,
                h,
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            ]);
        };
        let cwf = cw as f32;
        let chf = ch as f32;
        let insetf = inset as f32;

        for op in frame {
            match op {
                Op::Rect { row, col, w, h, face } => {
                    if let Some(bg) = face.bg {
                        push(
                            insetf + *col as f32 * cwf,
                            insetf + *row as f32 * chf,
                            *w as f32 * cwf,
                            *h as f32 * chf,
                            bg,
                        );
                    }
                }
                Op::VSpans { row0, col0, cols } => {
                    let top0 = insetf + *row0 as f32 * chf;
                    for (i, segs) in cols.iter().enumerate() {
                        let left = insetf + (*col0 as usize + i) as f32 * cwf;
                        let mut y = top0;
                        for (sh, color) in segs {
                            let span_h = *sh as f32 * chf;
                            if let Some(rgb) = color {
                                push(left, y, cwf, span_h, *rgb);
                            }
                            y += span_h;
                        }
                    }
                }
                Op::Cells { row0, col0, w, aspect, bits, color } => {
                    if let Some(rgb) = color {
                        let asp = (*aspect).max(1) as usize;
                        let cell_w = (asp as f32) * cwf;
                        let wmod = (*w).max(1) as u64;
                        // Enumerate set bits by walking the limbs ONCE — O(limbs + live),
                        // not the O(live × limbs) clone+set_bit scan (quadratic on big boards).
                        for (li, word) in bits.magnitude().iter_u64_digits().enumerate() {
                            let mut word = word;
                            let base = (li as u64) * 64;
                            while word != 0 {
                                let bit = base + word.trailing_zeros() as u64;
                                let x = (bit % wmod) as f32;
                                let y = (bit / wmod) as f32;
                                push(
                                    insetf + (*col0 as f32 + x * asp as f32) * cwf,
                                    insetf + (*row0 as f32 + y) * chf,
                                    cell_w,
                                    chf,
                                    *rgb,
                                );
                                word &= word - 1;
                            }
                        }
                    }
                }
                // Text / cursor / zones: not drawn in the GPU prototype (glyph atlas TODO).
                _ => {}
            }
        }

        unsafe {
            let gl = &self.gl;
            gl.viewport(0, 0, fw as i32, fh as i32);
            gl.clear_color(DEFAULT_BG[0], DEFAULT_BG[1], DEFAULT_BG[2], 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            let n = insts.len() / 7;
            if n > 0 {
                gl.use_program(Some(self.program));
                gl.uniform_2_f32(self.u_viewport.as_ref(), fw as f32, fh as f32);
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.inst_vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&insts), glow::DYNAMIC_DRAW);
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, n as i32);
            }
        }
        let _ = self.surface.swap_buffers(&self.context);
    }
}

fn as_bytes(v: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

unsafe fn build_pipeline(
    gl: &glow::Context,
) -> Result<(glow::Program, glow::VertexArray, glow::Buffer, Option<glow::UniformLocation>), String> {
    let program = gl.create_program().map_err(|e| format!("program: {e}"))?;
    for (kind, src) in [(glow::VERTEX_SHADER, VERT_SRC), (glow::FRAGMENT_SHADER, FRAG_SRC)] {
        let sh = gl.create_shader(kind).map_err(|e| format!("shader: {e}"))?;
        gl.shader_source(sh, src);
        gl.compile_shader(sh);
        if !gl.get_shader_compile_status(sh) {
            return Err(format!("shader compile: {}", gl.get_shader_info_log(sh)));
        }
        gl.attach_shader(program, sh);
        gl.delete_shader(sh);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        return Err(format!("link: {}", gl.get_program_info_log(program)));
    }

    let vao = gl.create_vertex_array().map_err(|e| format!("vao: {e}"))?;
    gl.bind_vertex_array(Some(vao));

    // Static unit-quad corners drawn as a triangle strip.
    let quad: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let quad_vbo = gl.create_buffer().map_err(|e| format!("quad vbo: {e}"))?;
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad_vbo));
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&quad), glow::STATIC_DRAW);
    gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
    gl.enable_vertex_attrib_array(0);

    // Per-instance buffer: rect (vec4) + colour (vec3), stride 28 bytes, divisor 1.
    let inst_vbo = gl.create_buffer().map_err(|e| format!("inst vbo: {e}"))?;
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(inst_vbo));
    gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, 28, 0);
    gl.enable_vertex_attrib_array(1);
    gl.vertex_attrib_divisor(1, 1);
    gl.vertex_attrib_pointer_f32(2, 3, glow::FLOAT, false, 28, 16);
    gl.enable_vertex_attrib_array(2);
    gl.vertex_attrib_divisor(2, 1);

    let u_viewport = gl.get_uniform_location(program, "viewport");
    Ok((program, vao, inst_vbo, u_viewport))
}
