//! The render-pipeline builder every pipeline here shares, the blend states they draw
//! with, and the choice of the sRGB view translucent geometry and text are drawn through.

/// The blend the LCD pipeline draws with: each colour channel weighed by its own coverage
/// (`Src1`); alpha `a + dst·(1 − a)` with `a` the greatest coverage, as grey text's is under
/// `ALPHA_BLENDING`.
pub(super) const LCD_BLEND: wgpu::BlendState = wgpu::BlendState {
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

/// The sRGB sibling of the swapchain `format` that translucent geometry is drawn
/// through, when the surface may be viewed in it (`SURFACE_VIEW_FORMATS`) and `format`
/// has one; `None` otherwise, and then every quad blends in the encoded space.
pub(super) fn linear_view_format(
    format: wgpu::TextureFormat,
    views_allowed: bool,
) -> Option<wgpu::TextureFormat> {
    let linear = format.add_srgb_suffix();
    (views_allowed && linear != format && format.remove_srgb_suffix() == format).then_some(linear)
}

/// A render pipeline over one shader module's entry points, the parts every pipeline here
/// shares spelled once.
#[allow(clippy::too_many_arguments)]
pub(super) fn pipeline(
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
pub(super) const ADD: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::OVER,
};

/// Colour × alpha added onto what is there: an additive particle.
pub(super) const ADD_ALPHA: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::OVER,
};

/// What is there × the colour drawn: the light map onto the scene.
pub(super) const MULTIPLY: wgpu::BlendState = wgpu::BlendState {
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
