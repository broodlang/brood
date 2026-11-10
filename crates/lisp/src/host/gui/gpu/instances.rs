//! The growable instance buffers the quad pipelines draw from, and the diff
//! (`changed_span`) that lets a frame write only the words that moved since the last.

use super::*;

/// A growable instance buffer: re-created (never mapped) when a frame outgrows it.
/// `shadow` is what the buffer holds now, word for word, for `upload_changed` — the
/// frame instance buffers diff against it and write only the span that moved.
pub(super) struct InstanceBuffer {
    pub(super) buffer: wgpu::Buffer,
    pub(super) capacity_bytes: u64,
    pub(super) shadow: Vec<u32>,
}

impl InstanceBuffer {
    pub(super) fn new(device: &wgpu::Device, label: &str, capacity_bytes: u64) -> InstanceBuffer {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity_bytes,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        InstanceBuffer {
            buffer,
            capacity_bytes,
            shadow: Vec::new(),
        }
    }

    /// Re-create the buffer, doubling, until `needed` bytes fit; what it held is gone.
    pub(super) fn reserve(&mut self, device: &wgpu::Device, label: &str, needed: u64) {
        if needed > self.capacity_bytes {
            let mut capacity = self.capacity_bytes.max(1);
            while capacity < needed {
                capacity *= 2;
            }
            *self = InstanceBuffer::new(device, label, capacity);
        }
    }

    /// Upload `data`, growing (doubling) the buffer first when it does not fit.
    pub(super) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        data: &[u8],
    ) {
        self.reserve(device, label, data.len() as u64);
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, 0, data);
        }
    }

    /// Upload `words`, writing only the span that differs from what the buffer already
    /// holds (`changed_span`) — a frame that moved the cursor or one line rewrites a
    /// few hundred bytes, not the page. A buffer that has to grow is re-created and
    /// written whole. Returns the bytes written, for the trace.
    pub(super) fn upload_changed(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        words: &[u32],
        reuse: bool,
    ) -> usize {
        self.reserve(device, label, std::mem::size_of_val(words) as u64);
        let span = if reuse {
            changed_span(&self.shadow, words)
        } else {
            (!words.is_empty()).then_some(0..words.len())
        };
        let Some(span) = span else {
            self.shadow.truncate(words.len());
            return 0;
        };
        queue.write_buffer(
            &self.buffer,
            (span.start * 4) as u64,
            as_bytes_u32(&words[span.clone()]),
        );
        self.shadow.resize(words.len(), 0);
        self.shadow[span.clone()].copy_from_slice(&words[span.clone()]);
        span.len() * 4
    }
}

/// The word range of `new` that differs from `old` — what an upload over a buffer that
/// holds `old` must write to make it hold `new`. `None` when nothing differs (a shorter
/// `new` whose words all match needs no write: the tail past it is never drawn). The
/// search compares 64-word chunks first, so an unchanged page costs a memcmp.
pub(super) fn changed_span(old: &[u32], new: &[u32]) -> Option<std::ops::Range<usize>> {
    const CHUNK: usize = 64;
    let common = old.len().min(new.len());
    let (old_common, new_common) = (&old[..common], &new[..common]);
    let first_chunk = old_common
        .chunks(CHUNK)
        .zip(new_common.chunks(CHUNK))
        .position(|(a, b)| a != b);
    let first = match first_chunk {
        Some(chunk) => {
            let base = chunk * CHUNK;
            base + old_common[base..]
                .iter()
                .zip(&new_common[base..])
                .position(|(a, b)| a != b)
                .expect("a differing chunk has a differing word")
        }
        None if new.len() > common => common,
        None => return None,
    };
    let end = if new.len() > common {
        new.len()
    } else {
        // the last differing word, searched from the back the same way
        let tail = common - first;
        let last_from_back = old_common[first..]
            .rchunks(CHUNK)
            .zip(new_common[first..].rchunks(CHUNK))
            .position(|(a, b)| a != b)
            .expect("the first differing word is in this range");
        let chunk_end = tail - last_from_back * CHUNK;
        let chunk_start = chunk_end.saturating_sub(CHUNK);
        let within = old_common[first + chunk_start..first + chunk_end]
            .iter()
            .zip(&new_common[first + chunk_start..first + chunk_end])
            .rposition(|(a, b)| a != b)
            .expect("a differing chunk has a differing word");
        first + chunk_start + within + 1
    };
    Some(first..end)
}
