//! Reading back a pressed draw's displacement at a list of its vertices (ADR-847): after the first
//! scene's contact passes a small compute pass of its own gathers each listed vertex's drawn place
//! less its skinned place (the posed buffer's first and fourth vec4, ADR-846) into a buffer, copied
//! into a read-back buffer, mapped after submit and unpacked at a later frame's start into
//! `scene_displacements::record`. One read-back in flight; a standing request is served whenever it
//! is free. The same gather reads listed vertices' drawn places instead (ADR-848, a reader of its
//! own: `scene_positions::record`), several draws at once.

/// What a reader gathers and where its samples go.
#[derive(Clone, Debug)]
pub(super) enum Kind {
    /// One draw's displacements (ADR-847).
    Displacements { draw: usize },
    /// Several draws' drawn places (ADR-848): whether each watched draw was posed.
    Positions { posed: Vec<bool> },
}

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::host::gui::scene_displacements::DisplacementSample;
use crate::host::gui::scene_positions::PositionSample;

use super::scenes::POSED_BYTES;

const FREE: u8 = 0;
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

/// The gather: `list` holds what to gather (0 each vertex's displacement, its drawn place less its
/// skinned place; 1 its drawn place) then each listed vertex's index into the posed buffer
/// (`UNPOSED` for one not posed this frame: 0 out); each into `out`.
const GATHER_SRC: &str = "
@group(0) @binding(0) var<storage, read> posed: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> list: array<u32>;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<f32>>;

const STRIDE: u32 = POSED_VEC4S;

@compute @workgroup_size(64)
fn cs_gather(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = arrayLength(&list) - 1u;
    if (id.x >= count) {
        return;
    }
    let index = list[1u + id.x];
    if (index == 0xffffffffu) {
        out[id.x] = vec4<f32>(0.0);
        return;
    }
    let at = index * STRIDE;
    if (list[0] == 1u) {
        out[id.x] = vec4<f32>(posed[at].xyz, 1.0);
    } else {
        out[id.x] = vec4<f32>(posed[at].xyz - posed[at + 3u].xyz, 0.0);
    }
}
";

/// A listed vertex not posed this frame (its draw not pressed, or past its draw's vertices).
pub(super) const UNPOSED: u32 = u32::MAX;

/// Each of `vertices` as an index into the posed buffer of a draw whose posed vertices start at
/// `start` and number `count` (`UNPOSED` past its end, or every one where it was not posed).
pub(super) fn posed_indices(range: Option<(u32, u32)>, vertices: &[u32]) -> Vec<u32> {
    vertices
        .iter()
        .map(|&vertex| match range {
            Some((start, count)) if vertex < count => start + vertex,
            _ => UNPOSED,
        })
        .collect()
}

pub(super) struct DisplacementReader {
    pipeline: Option<(wgpu::ComputePipeline, wgpu::BindGroupLayout)>,
    list: Option<wgpu::Buffer>,
    out: Option<wgpu::Buffer>,
    readback: Option<wgpu::Buffer>,
    state: Arc<AtomicU8>,
    kind: Kind,
    count: u64,
}

impl DisplacementReader {
    pub(super) fn new() -> DisplacementReader {
        DisplacementReader {
            pipeline: None,
            list: None,
            out: None,
            readback: None,
            state: Arc::new(AtomicU8::new(FREE)),
            kind: Kind::Displacements { draw: 0 },
            count: 0,
        }
    }

    /// At a frame's start: unpack what an earlier frame read back.
    pub(super) fn begin_frame(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        match self.state.load(Ordering::Acquire) {
            MAPPED => {
                if let Some(buffer) = &self.readback {
                    let sample = buffer
                        .slice(..self.count * 16)
                        .get_mapped_range()
                        .map(|view| {
                            let floats: Vec<f32> = view
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .map(|word| f32::from_le_bytes(*word))
                                .collect();
                            floats
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .flat_map(|vertex| [vertex[0], vertex[1], vertex[2]])
                                .collect::<Vec<f32>>()
                        })
                        .ok();
                    buffer.unmap();
                    if let Some(values) = sample {
                        match &self.kind {
                            Kind::Displacements { draw } => {
                                crate::host::gui::scene_displacements::record(DisplacementSample {
                                    draw: *draw,
                                    displacements: values,
                                })
                            }
                            Kind::Positions { posed } => {
                                crate::host::gui::scene_positions::record(PositionSample {
                                    positions: values,
                                    posed: posed.clone(),
                                })
                            }
                        }
                    }
                }
                self.state.store(FREE, Ordering::Release);
            }
            FAILED => self.state.store(FREE, Ordering::Release),
            _ => {}
        }
    }

    /// Whether it can take a gather this frame.
    pub(super) fn free(&self) -> bool {
        self.state.load(Ordering::Acquire) == FREE
    }

    /// After the contact passes: gather what `kind` asks of the posed vertices at `indices`
    /// (`posed_indices`), from `posed`.
    pub(super) fn gather(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        posed: &wgpu::Buffer,
        kind: Kind,
        indices: &[u32],
    ) {
        if !self.free() || indices.is_empty() {
            return;
        }
        let (pipeline, layout) = self.pipeline.get_or_insert_with(|| {
            let source = GATHER_SRC.replace("POSED_VEC4S", &format!("{}u", POSED_BYTES / 16));
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("displacement gather"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            };
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("displacement gather"),
                entries: &[storage(0, true), storage(1, true), storage(2, false)],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("displacement gather"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("displacement gather"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("cs_gather"),
                compilation_options: Default::default(),
                cache: None,
            });
            (pipeline, layout)
        });
        let count = indices.len() as u64;
        let mut words = Vec::with_capacity(indices.len() + 1);
        words.push(u32::from(matches!(kind, Kind::Positions { .. })));
        words.extend_from_slice(indices);
        let list_bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        let fits = |buffer: &Option<wgpu::Buffer>, size: u64| {
            buffer.as_ref().is_some_and(|buffer| buffer.size() == size)
        };
        if !fits(&self.list, list_bytes.len() as u64) {
            self.list = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("displacement list"),
                size: list_bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if !fits(&self.out, count * 16) {
            self.out = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("displacements"),
                size: count * 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
            self.readback = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("displacements read-back"),
                size: count * 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let (Some(list), Some(out), Some(readback)) = (&self.list, &self.out, &self.readback)
        else {
            return;
        };
        queue.write_buffer(list, 0, &list_bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("displacement gather"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: posed.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: list.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("displacement gather"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((count as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(out, 0, readback, 0, count * 16);
        self.kind = kind;
        self.count = count;
        self.state.store(COPIED, Ordering::Release);
    }

    /// After the frame is submitted: ask for the copy, read at a later frame's start.
    pub(super) fn after_submit(&mut self) {
        if self.state.load(Ordering::Acquire) != COPIED {
            return;
        }
        let Some(buffer) = &self.readback else {
            return;
        };
        self.state.store(MAPPING, Ordering::Release);
        let state = self.state.clone();
        buffer.map_async(wgpu::MapMode::Read, ..self.count * 16, move |result| {
            state.store(
                if result.is_ok() { MAPPED } else { FAILED },
                Ordering::Release,
            );
        });
    }
}
