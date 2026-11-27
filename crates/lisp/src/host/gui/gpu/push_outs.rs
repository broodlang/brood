//! Reading back the push-out net's count (ADR-839): each scene's counter (the last word of its
//! `contact depths` buffer, `cs_garment_push_out`'s atomic) copied after its contact pass into
//! one of a few read-back buffers, mapped after the frame is submitted, summed and logged
//! (`scene_push_outs::record`) at a later frame's start, as `scene_timer` reads its timestamps.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::host::gui::scene_push_outs::record;

/// The most scenes a frame's counts are read for (each its own word in a read-back buffer).
const COUNTED_SCENES: u64 = 16;
/// Read-back buffers in flight.
const SLOTS: usize = 3;

const FREE: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// How many scenes' words it holds.
    written: u64,
}

pub(super) struct PushOutReader {
    slots: Vec<Slot>,
    /// The slot this frame copies into, when one was free at its start.
    armed: Option<usize>,
    /// Scenes copied this frame.
    copied: u64,
}

impl PushOutReader {
    pub(super) fn new(device: &wgpu::Device) -> PushOutReader {
        let slots = (0..SLOTS)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("push-out counts"),
                    size: COUNTED_SCENES * 4,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                written: 0,
            })
            .collect();
        PushOutReader {
            slots,
            armed: None,
            copied: 0,
        }
    }

    /// At a frame's start: log what an earlier frame counted, and take a free slot for this one.
    pub(super) fn begin_frame(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &mut self.slots {
            match slot.state.load(Ordering::Acquire) {
                MAPPED => {
                    let total = slot
                        .buffer
                        .slice(..slot.written * 4)
                        .get_mapped_range()
                        .map(|view| {
                            view.as_chunks::<4>()
                                .0
                                .iter()
                                .map(|word| u64::from(u32::from_le_bytes(*word)))
                                .sum::<u64>()
                        })
                        .ok();
                    slot.buffer.unmap();
                    slot.state.store(FREE, Ordering::Release);
                    if let Some(total) = total {
                        record(total);
                    }
                }
                FAILED => {
                    slot.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
        self.armed = self
            .slots
            .iter()
            .position(|slot| slot.state.load(Ordering::Acquire) == FREE);
        self.copied = 0;
    }

    /// After a scene's contact pass, in the same encoder: copy its count (the word after its
    /// `MAX_CONTACTS` depths) into this frame's slot.
    pub(super) fn copy(&mut self, encoder: &mut wgpu::CommandEncoder, depths: &wgpu::Buffer) {
        let Some(index) = self.armed else {
            return;
        };
        if self.copied >= COUNTED_SCENES {
            return;
        }
        let counter = crate::host::gui::mesh::MAX_CONTACTS as u64 * 4;
        encoder.copy_buffer_to_buffer(
            depths,
            counter,
            &self.slots[index].buffer,
            self.copied * 4,
            4,
        );
        self.copied += 1;
    }

    /// After the frame's scenes: mark this frame's slot as to be read, if any scene counted.
    pub(super) fn end_frame(&mut self) {
        let Some(index) = self.armed else {
            return;
        };
        if self.copied == 0 {
            self.armed = None;
            return;
        }
        let slot = &mut self.slots[index];
        slot.written = self.copied;
        slot.state.store(MAPPING, Ordering::Release);
    }

    /// After the frame is submitted: ask for its read-back buffer, read at a later frame's start.
    pub(super) fn after_submit(&mut self) {
        let Some(index) = self.armed.take() else {
            return;
        };
        let slot = &self.slots[index];
        if slot.state.load(Ordering::Acquire) != MAPPING {
            return;
        }
        let state = slot.state.clone();
        slot.buffer
            .map_async(wgpu::MapMode::Read, ..slot.written * 4, move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { FAILED },
                    Ordering::Release,
                );
            });
    }
}
