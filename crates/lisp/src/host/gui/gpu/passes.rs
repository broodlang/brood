//! The render passes a frame's batches are drawn in: which batches go through the sRGB
//! view and which through the encoded one (`segments`), the clear colour each view needs,
//! and the draw of every batch kind.

use super::*;

/// An encoded (sRGB) clear colour as linear light, for a pass drawn through the sRGB view —
/// which encodes what it is given, so the encoded value would be encoded twice.
pub(super) fn decode_clear(colour: wgpu::Color) -> wgpu::Color {
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
/// pass draws through the swapchain's sRGB view (text, ADR-611, and translucent solids,
/// blended in linear light) and the rest through the encoded view (an app's textures and
/// particles — and an opaque fill, which looks the same in either, joins whichever pass is
/// open). Consecutive batches of one kind share a pass, so an editor's frame — text and
/// fills — is one linear pass, and a frame pays a switch only where a sprite or particles
/// interleave with text. Without an sRGB view (`linear_available` false) it is one encoded
/// pass.
pub(super) fn segments(
    batches: &[Batch],
    linear_available: bool,
) -> Vec<(bool, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space: Option<bool> = None;
    for (index, batch) in batches.iter().enumerate() {
        let wants = match batch {
            Batch::Solid { linear, .. } => (*linear && linear_available).then_some(true),
            Batch::Tex {
                key: TexKey::Atlas(_),
                ..
            } => Some(linear_available),
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

impl GpuWindow {
    /// Draw `batches` in order into `view` — cleared to `clear` first, or drawn over what
    /// is there. With `linear_view` (the same image viewed as sRGB) the batches are drawn
    /// as `segments`: translucent geometry through it, blended in linear light, the rest
    /// through `view`. Returns how many render passes it took, for the trace.
    pub(super) fn draw_pass(
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
                        // Text in a linear pass takes the linear-light pipelines (ADR-611).
                        let (tex_pipeline, lcd_pipelines) = match &self.tex_linear_pipeline {
                            Some(linear) if *in_linear => (linear, &self.lcd_linear),
                            _ => (&self.tex_pipeline, &self.lcd),
                        };
                        let pipelines: &[wgpu::RenderPipeline] = match lcd_pipelines {
                            Some(LcdText::DualSource(pipeline)) if *lcd => {
                                std::slice::from_ref(pipeline)
                            }
                            Some(LcdText::PerChannel(channels)) if *lcd => channels,
                            _ => std::slice::from_ref(tex_pipeline),
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
}

/// A render pass over `view` with one colour attachment: cleared to `clear`, or loading
/// what is there when it is `None`.
pub(super) fn render_pass<'e>(
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
