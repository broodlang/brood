//! Timestamp queries round a frame's scene passes (ADR-801): the shadow passes' span and the
//! colour pass, each scene's, read back a few frames late so nothing waits on the card, and
//! kept in `scene_gpu_times`.
//!
//! The stamps go on the passes themselves (`timestamp_writes`), so only
//! `Features::TIMESTAMP_QUERY` is needed. Queries are written in order and only the ones
//! written are resolved: a query never written is never read (Vulkan would wait on it).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::host::gui::scene_gpu_times::{SceneGpuSample, record};

/// How many scenes a frame are timed; any more are drawn untimed.
pub(super) const TIMED_SCENES: usize = 4;
/// Two stamps round the shadow passes, two round the colour pass and two round the contact pass
/// (ADR-844), a scene.
const PASS_QUERIES: u32 = 6 * TIMED_SCENES as u32;
/// Under `BROOD_GUI_GPU_TIMES=draws`, a stamp before each draw of a colour pass and one after
/// its last, at most this many a frame: past it the rest of the frame's draws go untimed.
const DRAW_QUERIES: u32 = 1024;
/// Read-back buffers in flight: a frame is stamped only when one is free.
const SLOTS: usize = 3;

const FREE: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

/// Where one scene's stamps went in the query set, by kind.
#[derive(Clone, Default)]
struct SceneStamps {
    shadow: Option<(u32, u32)>,
    colour: Option<(u32, u32)>,
    /// The contact compute pass (ADR-805, ADR-844): posed, relaxed, pressed, smoothed.
    contacts: Option<(u32, u32)>,
    /// Each timed draw of the colour pass, in order, with the stamp written before it; the
    /// stamp after the last is `draws_end`.
    draws: Vec<DrawStamp>,
    draws_end: Option<u32>,
}

/// One draw of a colour pass as `BROOD_GUI_GPU_TIMES=draws` tells it: its mesh handle, its
/// surface, its triangles, and the stamp written before it.
#[derive(Clone, Copy)]
pub(super) struct DrawStamp {
    pub(super) mesh: u32,
    pub(super) surface: crate::host::gui::mesh::Surface,
    pub(super) triangles: u32,
    pub(super) query: u32,
}

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// The stamped frame's layout, kept until its numbers are read.
    stamps: Vec<SceneStamps>,
    written: u32,
}

pub(super) struct SceneTimer {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    /// Nanoseconds a tick.
    period: f64,
    /// This frame's slot, when it is stamped.
    armed: Option<usize>,
    stamps: Vec<SceneStamps>,
    next: u32,
    /// The query set's size: the passes' stamps, and the draws' when they are timed.
    queries: u32,
    /// Whether each draw of a colour pass is timed (`BROOD_GUI_GPU_TIMES=draws`).
    draws: bool,
}

impl SceneTimer {
    /// A timer on `device`, or `None` without timestamp queries or under
    /// `BROOD_GUI_GPU_TIMES=0`.
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY)
            || std::env::var("BROOD_GUI_GPU_TIMES").as_deref() == Ok("0")
        {
            return None;
        }
        let period = f64::from(queue.get_timestamp_period());
        if period <= 0.0 {
            return None;
        }
        // each draw too, where asked and the device can stamp inside a pass
        let draws = std::env::var("BROOD_GUI_GPU_TIMES").as_deref() == Ok("draws")
            && device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES);
        let queries = PASS_QUERIES + if draws { DRAW_QUERIES } else { 0 };
        let bytes = u64::from(queries) * 8;
        let slots = (0..SLOTS)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("scene timestamps read back"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                stamps: Vec::new(),
                written: 0,
            })
            .collect();
        Some(SceneTimer {
            query_set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("scene timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: queries,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene timestamps resolved"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            slots,
            period,
            armed: None,
            stamps: Vec::new(),
            next: 0,
            queries,
            draws,
        })
    }

    /// Before a frame's scenes: keep what earlier frames' stamps measured, and stamp this
    /// frame if a read-back buffer is free.
    pub(super) fn begin_frame(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &mut self.slots {
            match slot.state.load(Ordering::Acquire) {
                MAPPED => {
                    let sample = {
                        let Ok(view) = slot
                            .buffer
                            .slice(..u64::from(slot.written) * 8)
                            .get_mapped_range()
                        else {
                            slot.buffer.unmap();
                            slot.state.store(FREE, Ordering::Release);
                            continue;
                        };
                        let ticks: Vec<u64> = view
                            .as_chunks::<8>()
                            .0
                            .iter()
                            .map(|bytes| u64::from_le_bytes(*bytes))
                            .collect();
                        if let Some(line) = draws_line(&slot.stamps, &ticks, self.period) {
                            eprintln!("{line}");
                        }
                        measure(&slot.stamps, &ticks, self.period)
                    };
                    slot.buffer.unmap();
                    slot.state.store(FREE, Ordering::Release);
                    if let Some(sample) = sample {
                        record(sample);
                    }
                }
                FAILED => slot.state.store(FREE, Ordering::Release),
                _ => {}
            }
        }
        self.armed = self
            .slots
            .iter()
            .position(|slot| slot.state.load(Ordering::Acquire) == FREE);
        self.stamps.clear();
        self.next = 0;
    }

    fn take(&mut self) -> Option<(u32, u32)> {
        if self.armed.is_none() || self.next + 2 > self.queries {
            return None;
        }
        let pair = (self.next, self.next + 1);
        self.next += 2;
        Some(pair)
    }

    /// The query set, for a stamp written inside a pass.
    pub(super) fn query_set(&self) -> &wgpu::QuerySet {
        &self.query_set
    }

    /// The stamp to write before a draw of scene `scene`'s colour pass, under
    /// `BROOD_GUI_GPU_TIMES=draws`: `None` when draws are not timed or the frame's stamps are
    /// used up (room is always kept for the stamp after the last draw).
    pub(super) fn draw_stamp(
        &mut self,
        scene: usize,
        mesh: u32,
        surface: crate::host::gui::mesh::Surface,
        triangles: u32,
    ) -> Option<u32> {
        if !self.draws
            || self.armed.is_none()
            || scene >= TIMED_SCENES
            || self.next + 2 > self.queries
        {
            return None;
        }
        let query = self.next;
        self.next += 1;
        self.scene_entry(scene).draws.push(DrawStamp {
            mesh,
            surface,
            triangles,
            query,
        });
        Some(query)
    }

    /// The stamp to write after the last draw of scene `scene`'s colour pass: `None` when none
    /// of its draws was stamped.
    pub(super) fn draws_end_stamp(&mut self, scene: usize) -> Option<u32> {
        if !self.draws || self.armed.is_none() || self.stamps.get(scene)?.draws.is_empty() {
            return None;
        }
        let query = self.next;
        self.next += 1;
        self.scene_entry(scene).draws_end = Some(query);
        Some(query)
    }

    /// Scene `scene`'s shadow stamps, taken before its first shadow pass: the first pass
    /// writes the start, the last the end.
    pub(super) fn shadow_stamps(&mut self, scene: usize) -> Option<(u32, u32)> {
        if scene >= TIMED_SCENES {
            return None;
        }
        let pair = self.take();
        self.scene_entry(scene).shadow = pair;
        pair
    }

    /// Scene `scene`'s contact pass stamps (ADR-844).
    pub(super) fn contact_stamps(&mut self, scene: usize) -> Option<(u32, u32)> {
        if scene >= TIMED_SCENES {
            return None;
        }
        let pair = self.take();
        self.scene_entry(scene).contacts = pair;
        pair
    }

    /// The contact pass's timestamp writes: its start and end index into the query set.
    pub(super) fn compute_writes(&self, pair: (u32, u32)) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(pair.0),
            end_of_pass_write_index: Some(pair.1),
        }
    }

    /// Scene `scene`'s colour pass stamps.
    pub(super) fn colour_stamps(&mut self, scene: usize) -> Option<(u32, u32)> {
        if scene >= TIMED_SCENES {
            return None;
        }
        let pair = self.take();
        self.scene_entry(scene).colour = pair;
        pair
    }

    fn scene_entry(&mut self, scene: usize) -> &mut SceneStamps {
        if self.stamps.len() <= scene {
            self.stamps.resize(scene + 1, SceneStamps::default());
        }
        &mut self.stamps[scene]
    }

    /// A pass's timestamp writes: the start and/or the end index into the query set.
    pub(super) fn writes(
        &self,
        start: Option<u32>,
        end: Option<u32>,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if start.is_none() && end.is_none() {
            return None;
        }
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: start,
            end_of_pass_write_index: end,
        })
    }

    /// After the frame's scenes, in the same encoder: resolve what was written into this
    /// frame's read-back buffer.
    pub(super) fn end_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(index) = self.armed else {
            return;
        };
        if self.next == 0 {
            self.armed = None;
            return;
        }
        encoder.resolve_query_set(&self.query_set, 0..self.next, &self.resolve, 0);
        let slot = &mut self.slots[index];
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &slot.buffer, 0, u64::from(self.next) * 8);
        slot.stamps = std::mem::take(&mut self.stamps);
        slot.written = self.next;
        slot.state.store(MAPPING, Ordering::Release);
    }

    /// After the frame is submitted: ask for its read-back buffer, read at a later frame's
    /// `begin_frame`.
    pub(super) fn after_submit(&mut self) {
        let Some(index) = self.armed.take() else {
            return;
        };
        let slot = &self.slots[index];
        let state = slot.state.clone();
        slot.buffer.map_async(
            wgpu::MapMode::Read,
            ..u64::from(slot.written) * 8,
            move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { FAILED },
                    Ordering::Release,
                );
            },
        );
    }
}

/// One frame's sample from its stamps' layout and the ticks read back.
/// One frame's timed draws as a trace line, `[gui-scene-gpu-draws] scene:mesh:surface:triangles:us …`
/// in draw order: each draw from its stamp to the next one's. `None` when no draw was timed.
fn draws_line(stamps: &[SceneStamps], ticks: &[u64], period: f64) -> Option<String> {
    let mut line = String::from("[gui-scene-gpu-draws]");
    let mut any = false;
    for (scene, stamp) in stamps.iter().enumerate() {
        let Some(end) = stamp.draws_end else {
            continue;
        };
        let boundaries: Vec<u32> = stamp
            .draws
            .iter()
            .map(|draw| draw.query)
            .chain(std::iter::once(end))
            .collect();
        for (draw, pair) in stamp.draws.iter().zip(boundaries.windows(2)) {
            let (Some(start), Some(finish)) =
                (ticks.get(pair[0] as usize), ticks.get(pair[1] as usize))
            else {
                continue;
            };
            let micros = finish.saturating_sub(*start) as f64 * period / 1000.0;
            line.push_str(&format!(
                " {scene}:{}:{:?}:{}:{micros:.1}",
                draw.mesh, draw.surface, draw.triangles
            ));
            any = true;
        }
    }
    any.then_some(line)
}

fn measure(stamps: &[SceneStamps], ticks: &[u64], period: f64) -> Option<SceneGpuSample> {
    let span = |pair: Option<(u32, u32)>| -> f64 {
        pair.and_then(|(start, end)| {
            let (start, end) = (*ticks.get(start as usize)?, *ticks.get(end as usize)?);
            Some(end.saturating_sub(start) as f64 * period / 1000.0)
        })
        .unwrap_or(0.0)
    };
    let timed = stamps.iter().filter(|scene| scene.colour.is_some()).count();
    if timed == 0 {
        return None;
    }
    Some(SceneGpuSample {
        shadow_us: stamps.iter().map(|scene| span(scene.shadow)).sum(),
        scene_us: stamps.iter().map(|scene| span(scene.colour)).sum(),
        contacts_us: stamps.iter().map(|scene| span(scene.contacts)).sum(),
        scenes: timed as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_measured_from_its_stamps_in_ticks() {
        let stamps = [
            SceneStamps {
                shadow: Some((0, 1)),
                colour: Some((2, 3)),
                ..SceneStamps::default()
            },
            SceneStamps {
                shadow: None,
                colour: Some((4, 5)),
                ..SceneStamps::default()
            },
        ];
        // a tick of 2 ns: 500 ticks a microsecond
        let ticks = [1_000, 2_000, 2_000, 5_000, 6_000, 6_500];
        let sample = measure(&stamps, &ticks, 2.0).expect("timed");
        assert_eq!(sample.shadow_us, 2.0);
        assert_eq!(sample.scene_us, 6.0 + 1.0);
        assert_eq!(sample.scenes, 2);
    }

    #[test]
    fn each_draw_is_timed_from_its_stamp_to_the_next() {
        use crate::host::gui::mesh::Surface;
        let draw = |mesh, surface, query| DrawStamp {
            mesh,
            surface,
            triangles: 10,
            query,
        };
        let stamps = [SceneStamps {
            colour: Some((0, 1)),
            draws: vec![draw(4, Surface::Skin, 2), draw(7, Surface::Strands, 3)],
            draws_end: Some(4),
            ..SceneStamps::default()
        }];
        // a tick of 1 ns: draws of 3 us and 5 us
        let ticks = [0, 10_000, 1_000, 4_000, 9_000];
        assert_eq!(
            draws_line(&stamps, &ticks, 1.0).as_deref(),
            Some("[gui-scene-gpu-draws] 0:4:Skin:10:3.0 0:7:Strands:10:5.0")
        );
        assert_eq!(draws_line(&[SceneStamps::default()], &ticks, 1.0), None);
    }

    #[test]
    fn a_frame_without_a_colour_pass_measures_nothing() {
        assert_eq!(measure(&[SceneStamps::default()], &[], 1.0), None);
    }
}
