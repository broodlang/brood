//! The frame readback: copying a drawn frame into a mapped buffer as RGB bytes, and the
//! `BROOD_GUI_DUMP` debug flag that writes every presented frame to a PPM file.

/// `BROOD_GUI_DUMP=<path.ppm>`: the GPU path's half of the CPU painter's dump flag
/// (`paint::dump_canvas`) — read the presented frame back and write it as a binary PPM
/// after every paint, so what the GPU drew can be LOOKED at from a script (the desktop
/// forbids a screenshot to an unprivileged process). Read once; off by default. The
/// readback is a full round trip through a mapped buffer every frame, so it is a debug
/// aid, not a path anything else runs on.
pub(super) fn dump_path() -> Option<&'static str> {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| std::env::var("BROOD_GUI_DUMP").ok())
        .as_deref()
}

/// Write `texture` (the frame just drawn, `w`×`h`, `format`) to `path` as a PPM.
pub(super) fn dump_frame(
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
pub(super) fn read_frame(
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
