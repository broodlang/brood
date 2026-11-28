//! Reading back a pressed draw's posed vertices (ADR-841): its range of the scene's posed buffer
//! (three vec4 a vertex: position, normal, tangent) copied after the contact passes into a
//! read-back buffer, mapped after the frame is submitted, and unpacked at a later frame's start
//! into `scene_posed::record`, as `push_outs` reads its count.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::host::gui::scene_posed::{PosedSample, record};

const FREE: u8 = 0;
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

use super::scenes::POSED_BYTES;

pub(super) struct PosedReader {
    buffer: Option<wgpu::Buffer>,
    state: Arc<AtomicU8>,
    /// The draw whose vertices it holds, and how many.
    draw: usize,
    count: u64,
}

impl PosedReader {
    pub(super) fn new() -> PosedReader {
        PosedReader {
            buffer: None,
            state: Arc::new(AtomicU8::new(FREE)),
            draw: 0,
            count: 0,
        }
    }

    /// At a frame's start: unpack what an earlier frame read back.
    pub(super) fn begin_frame(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        match self.state.load(Ordering::Acquire) {
            MAPPED => {
                if let Some(buffer) = &self.buffer {
                    let sample = buffer
                        .slice(..self.count * POSED_BYTES)
                        .get_mapped_range()
                        .map(|view| {
                            let floats: Vec<f32> = view
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .map(|word| f32::from_le_bytes(*word))
                                .collect();
                            let mut positions = Vec::with_capacity(self.count as usize * 3);
                            let mut normals = Vec::with_capacity(self.count as usize * 3);
                            for vertex in floats.as_chunks::<20>().0 {
                                positions.extend_from_slice(&vertex[0..3]);
                                normals.extend_from_slice(&vertex[4..7]);
                            }
                            PosedSample {
                                draw: self.draw,
                                positions,
                                normals,
                            }
                        })
                        .ok();
                    buffer.unmap();
                    if let Some(sample) = sample {
                        record(sample);
                    }
                }
                self.state.store(FREE, Ordering::Release);
            }
            FAILED => self.state.store(FREE, Ordering::Release),
            _ => {}
        }
    }

    /// Whether it can take a copy this frame.
    pub(super) fn free(&self) -> bool {
        self.state.load(Ordering::Acquire) == FREE
    }

    /// After the contact passes: copy `count` posed vertices from `start` of `posed` for `draw`.
    pub(super) fn copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        posed: &wgpu::Buffer,
        draw: usize,
        start: u32,
        count: u32,
    ) {
        if !self.free() || count == 0 {
            return;
        }
        let size = u64::from(count) * POSED_BYTES;
        if self
            .buffer
            .as_ref()
            .is_none_or(|buffer| buffer.size() < size)
        {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("posed read-back"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(buffer) = &self.buffer else {
            return;
        };
        encoder.copy_buffer_to_buffer(posed, u64::from(start) * POSED_BYTES, buffer, 0, size);
        self.draw = draw;
        self.count = u64::from(count);
        self.state.store(COPIED, Ordering::Release);
    }

    /// After the frame is submitted: ask for the copy, read at a later frame's start.
    pub(super) fn after_submit(&mut self) {
        if self.state.load(Ordering::Acquire) != COPIED {
            return;
        }
        let Some(buffer) = &self.buffer else {
            return;
        };
        self.state.store(MAPPING, Ordering::Release);
        let state = self.state.clone();
        buffer.map_async(
            wgpu::MapMode::Read,
            ..self.count * POSED_BYTES,
            move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { FAILED },
                    Ordering::Release,
                );
            },
        );
    }
}
