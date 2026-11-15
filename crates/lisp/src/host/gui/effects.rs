//! Lighting, post-processing and particles: the frame ops a game asks the GPU for
//! (`Ambient` / `Light` / `Overlay`, `Bloom` / `Crt`, `Emitter` / `Burst`), with what both
//! painters share.
//!
//! The rule is ADR-374's: O(entities) on the Brood side, O(pixels) natively. A frame
//! carries one op per light and one per emitter; the light map, the blur and the
//! particles themselves are this side's. The GPU target does the work in shaders (a
//! compute shader steps the particles); the CPU painter does the SAME arithmetic here, in
//! software, because a frame vocabulary half the builds cannot draw is a trap (ADR-374
//! addendum 4). The two agree on everything a game can see — which particles exist,
//! where, how old — because the spawn bookkeeping is one `EmitterBook` and the
//! per-particle random numbers are one hash (`rand`, mirrored in WGSL).

use std::collections::HashMap;
use std::time::Instant;

use super::{EmitterSpec, MAX_PARTICLES, Op};

/// The longest step a particle system takes in one paint. A window that was hidden, or a
/// paint that stalled, resumes where it was rather than jumping its particles a second on.
const MAX_STEP_SECONDS: f32 = 0.1;

/// One particle, as the GPU stores it: position, velocity, age, life (0 = a free slot),
/// and two words of padding to a 32-byte stride.
pub(crate) const PARTICLE_FLOATS: usize = 8;

/// lowbias32 (Chris Wellons): a well-mixed u32 hash, written the same way in the WGSL
/// compute shader, so a particle spawned in slot `s` of step `seed` gets the same random
/// numbers on either target.
pub(crate) fn hash(x: u32) -> u32 {
    let mut v = x;
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^= v >> 16;
    v
}

/// Random number `k` (0..4) for slot `slot` of the step seeded `seed`, uniform in [0, 1).
pub(crate) fn rand(seed: u32, slot: u32, k: u32) -> f32 {
    (hash(seed ^ hash(slot.wrapping_mul(4).wrapping_add(k))) >> 8) as f32 / 16_777_216.0
}

/// What one paint asks of one emitter's particles: spawn `spawn_count` fresh ones into
/// the ring from slot `spawn_first` (seeded `seed`), then step every live one by `dt`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EmitterStep {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub spec: EmitterSpec,
    pub dt: f32,
    pub spawn_first: u32,
    pub spawn_count: u32,
    pub seed: u32,
    /// The frame carries this emitter (else its particles are only living out their lives).
    /// Read by the GPU renderer only; a `gui` build without `gui-gpu` never reads it.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub present: bool,
}

struct Emitter {
    x: f32,
    y: f32,
    spec: EmitterSpec,
    /// The next ring slot a spawn takes.
    cursor: u32,
    /// The fraction of a particle `rate × dt` left over, carried to the next step.
    owed: f32,
    /// Seconds since the last particle was spawned — once past the longest life, none live.
    since_spawn: f32,
    steps: u32,
    present: bool,
}

/// The emitters of one window, stepped once per paint. Shared by both painters: the GPU
/// target turns each step into a compute dispatch, the CPU painter into `CpuParticles::step`.
#[derive(Default)]
pub(crate) struct EmitterBook {
    emitters: HashMap<u32, Emitter>,
    /// Bursts from frames that ARRIVED since the last paint — an event fires once per
    /// frame drawn, not per repaint (`frame_arrived`).
    bursts: HashMap<u32, u32>,
    last_paint: Option<Instant>,
}

impl EmitterBook {
    /// A frame arrived (`gui-draw`): remember its bursts for the next paint.
    pub fn frame_arrived(&mut self, ops: &[Op]) {
        walk(ops, &mut |op| {
            if let Op::Burst { id, count } = op {
                let owed = self.bursts.entry(*id).or_insert(0);
                *owed = owed.saturating_add(*count).min(MAX_PARTICLES);
            }
        });
    }

    /// Advance every emitter to now: the frame's emitters take its position and spec,
    /// spawn their rate's worth and any burst; the ones it no longer carries stop
    /// spawning and are forgotten once their last particle has died.
    pub fn step(&mut self, frame: &[Op]) -> Vec<EmitterStep> {
        let now = Instant::now();
        let dt = self
            .last_paint
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0)
            .clamp(0.0, MAX_STEP_SECONDS);
        self.last_paint = Some(now);
        for emitter in self.emitters.values_mut() {
            emitter.present = false;
        }
        walk(frame, &mut |op| {
            if let Op::Emitter { id, x, y, spec } = op {
                let emitter = self.emitters.entry(*id).or_insert(Emitter {
                    x: *x,
                    y: *y,
                    spec: *spec,
                    cursor: 0,
                    owed: 0.0,
                    since_spawn: f32::INFINITY,
                    steps: 0,
                    present: true,
                });
                if emitter.spec.max != spec.max {
                    // a new capacity is a new ring
                    emitter.cursor = 0;
                }
                emitter.x = *x;
                emitter.y = *y;
                emitter.spec = *spec;
                emitter.present = true;
            }
        });
        let bursts = std::mem::take(&mut self.bursts);
        let mut steps = Vec::with_capacity(self.emitters.len());
        self.emitters.retain(|id, emitter| {
            let capacity = emitter.spec.max.clamp(1, MAX_PARTICLES);
            let mut spawn = 0u32;
            if emitter.present {
                let due = emitter.owed + emitter.spec.rate.max(0.0) * dt;
                let whole = due.floor().min(capacity as f32);
                emitter.owed = (due - whole).max(0.0);
                spawn = whole as u32 + bursts.get(id).copied().unwrap_or(0);
            }
            let spawn = spawn.min(capacity);
            if spawn > 0 {
                emitter.since_spawn = 0.0;
            } else {
                emitter.since_spawn += dt;
            }
            let longest = emitter.spec.life[0].max(emitter.spec.life[1]);
            if !emitter.present && emitter.since_spawn > longest {
                return false;
            }
            steps.push(EmitterStep {
                id: *id,
                x: emitter.x,
                y: emitter.y,
                spec: emitter.spec,
                dt,
                spawn_first: emitter.cursor % capacity,
                spawn_count: spawn,
                seed: hash(id.wrapping_mul(0x9e37_79b9) ^ emitter.steps),
                present: emitter.present,
            });
            emitter.cursor = (emitter.cursor % capacity + spawn) % capacity;
            emitter.steps = emitter.steps.wrapping_add(1);
            true
        });
        steps.sort_by_key(|step| step.id);
        steps
    }

    /// Whether any particle may still be alive — the window keeps repainting while so.
    pub fn alive(&self) -> bool {
        self.emitters
            .values()
            .any(|e| e.present || e.since_spawn <= e.spec.life[0].max(e.spec.life[1]))
    }
}

/// Every op of `ops`, regions included.
fn walk<'a>(ops: &'a [Op], f: &mut impl FnMut(&'a Op)) {
    for op in ops {
        match op {
            Op::ScrollRegion { ops, .. } | Op::CellRegion { ops, .. } | Op::AboveSeam { ops } => {
                walk(ops, f)
            }
            // A fragment never holds a frame-global op (`%gui-draw` splices one that would,
            // ADR-470), so the walk — every paint, over the whole frame — skips its ops.
            Op::Fragment(_) => {}
            other => f(other),
        }
    }
}

/// A fresh particle for `slot` of `step`: the arithmetic of the compute shader's spawn.
pub(crate) fn spawn(step: &EmitterStep, slot: u32) -> [f32; PARTICLE_FLOATS] {
    let s = &step.spec;
    let angle = s.angle + (rand(step.seed, slot, 0) - 0.5) * s.spread;
    let speed = s.speed[0] + (s.speed[1] - s.speed[0]) * rand(step.seed, slot, 1);
    let life = s.life[0] + (s.life[1] - s.life[0]) * rand(step.seed, slot, 2);
    let (sin, cos) = angle.sin_cos();
    [
        step.x,
        step.y,
        cos * speed,
        sin * speed,
        0.0,
        life.max(1e-3),
        0.0,
        0.0,
    ]
}

/// One particle stepped `dt`: the compute shader's integration. A dead one stays dead.
pub(crate) fn integrate(p: &mut [f32; PARTICLE_FLOATS], spec: &EmitterSpec, dt: f32) {
    if p[5] <= 0.0 || p[4] >= p[5] {
        return;
    }
    let keep = (1.0 - spec.drag * dt).max(0.0);
    p[2] = (p[2] + spec.gravity[0] * dt) * keep;
    p[3] = (p[3] + spec.gravity[1] * dt) * keep;
    p[0] += p[2] * dt;
    p[1] += p[3] * dt;
    p[4] += dt;
}

/// The CPU painter's particles: each emitter's ring, stepped as the GPU's would be.
#[derive(Default)]
pub(crate) struct CpuParticles {
    rings: HashMap<u32, (EmitterSpec, Vec<[f32; PARTICLE_FLOATS]>)>,
}

impl CpuParticles {
    pub fn step(&mut self, steps: &[EmitterStep]) {
        self.rings
            .retain(|id, _| steps.iter().any(|step| step.id == *id));
        for step in steps {
            let capacity = step.spec.max.clamp(1, MAX_PARTICLES) as usize;
            let (spec, ring) = self
                .rings
                .entry(step.id)
                .or_insert_with(|| (step.spec, Vec::new()));
            if ring.len() != capacity {
                *ring = vec![[0.0; PARTICLE_FLOATS]; capacity];
            }
            *spec = step.spec;
            for n in 0..step.spawn_count {
                let slot = (step.spawn_first + n) % capacity as u32;
                ring[slot as usize] = spawn(step, slot);
            }
            for p in ring.iter_mut() {
                integrate(p, spec, step.dt);
            }
        }
    }

    /// The live particles of emitter `id` as `(x, y, size, rgba)`, oldest slot first.
    pub fn particles(&self, id: u32) -> impl Iterator<Item = (f32, f32, f32, [f32; 4])> + '_ {
        self.rings.get(&id).into_iter().flat_map(|(spec, ring)| {
            ring.iter()
                .filter(|p| p[5] > 0.0 && p[4] < p[5])
                .map(move |p| {
                    let t = (p[4] / p[5]).clamp(0.0, 1.0);
                    let size = spec.size[0] + (spec.size[1] - spec.size[0]) * t;
                    (p[0], p[1], size, mix_rgba(spec.color, spec.color_end, t))
                })
        })
    }

    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.rings.keys().copied()
    }

    pub fn spec(&self, id: u32) -> Option<&EmitterSpec> {
        self.rings.get(&id).map(|(spec, _)| spec)
    }
}

/// Straight rgba `a` → `b` at `t`, as 0..1 floats.
pub(crate) fn mix_rgba(a: [u8; 4], b: [u8; 4], t: f32) -> [f32; 4] {
    let channel = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) / 255.0;
    [channel(0), channel(1), channel(2), channel(3)]
}

/// How much of a particle's colour covers a pixel `distance` from its centre, as a
/// fraction of its size: a square covers all of itself, a disc fades from its centre to
/// its rim (the shader's `particle_coverage`).
pub(crate) fn particle_coverage(distance: f32, square: bool) -> f32 {
    if square {
        1.0
    } else {
        let edge = (1.0 - 2.0 * distance).clamp(0.0, 1.0);
        edge * edge
    }
}

/// How much of a light reaches `fraction` of its radius out: full at the centre, nothing
/// at the rim, smooth at both ends (the shader's `light_falloff`).
pub(crate) fn light_falloff(fraction: f32) -> f32 {
    if fraction >= 1.0 {
        0.0
    } else {
        let k = 1.0 - fraction * fraction;
        k * k
    }
}

/// The effects a frame asks for, read once per paint: lighting (its ambient and lights),
/// where the unlit overlay starts in its top-level op list, and the post passes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FrameEffects {
    pub ambient: Option<[u8; 3]>,
    pub lights: Vec<(f32, f32, f32, [u8; 3], f32)>,
    pub overlay_at: Option<usize>,
    pub bloom: Option<(f32, f32, f32)>,
    pub crt: Option<(f32, f32, f32)>,
}

impl FrameEffects {
    pub fn of(frame: &[Op]) -> FrameEffects {
        let mut fx = FrameEffects {
            overlay_at: frame.iter().position(|op| matches!(op, Op::Overlay)),
            ..FrameEffects::default()
        };
        walk(frame, &mut |op| match op {
            Op::Ambient { color } if fx.ambient.is_none() => fx.ambient = Some(*color),
            Op::Light {
                x,
                y,
                radius,
                color,
                intensity,
            } if *radius > 0.0 && radius.is_finite() && x.is_finite() && y.is_finite() => fx
                .lights
                .push((*x, *y, *radius, *color, intensity.max(0.0))),
            Op::Bloom {
                threshold,
                intensity,
                radius,
            } if fx.bloom.is_none() => {
                fx.bloom = Some((
                    threshold.clamp(0.0, 1.0),
                    intensity.clamp(0.0, 8.0),
                    radius.clamp(0.0, 64.0),
                ))
            }
            Op::Crt {
                scanlines,
                curvature,
                vignette,
            } if fx.crt.is_none() => {
                fx.crt = Some((
                    scanlines.clamp(0.0, 1.0),
                    curvature.clamp(0.0, 1.0),
                    vignette.clamp(0.0, 1.0),
                ))
            }
            _ => {}
        });
        fx
    }

    /// Whether the frame needs more than one pass over its pixels.
    pub fn any(&self) -> bool {
        self.ambient.is_some() || self.bloom.is_some() || self.crt.is_some()
    }
}

// --- the CPU painter's passes, over a 0x00RRGGBB canvas ----------------------------------

fn unpack(p: u32) -> [f32; 3] {
    [
        ((p >> 16) & 0xff) as f32 / 255.0,
        ((p >> 8) & 0xff) as f32 / 255.0,
        (p & 0xff) as f32 / 255.0,
    ]
}

fn pack(c: [f32; 3]) -> u32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (byte(c[0]) << 16) | (byte(c[1]) << 8) | byte(c[2])
}

/// Multiply `canvas` (`w`×`h`) by the light map: `ambient`, plus every light's colour ×
/// intensity × falloff, capped at full brightness.
pub(crate) fn apply_lighting(canvas: &mut [u32], w: usize, h: usize, fx: &FrameEffects) {
    let Some(ambient) = fx.ambient else {
        return;
    };
    let base = [
        ambient[0] as f32 / 255.0,
        ambient[1] as f32 / 255.0,
        ambient[2] as f32 / 255.0,
    ];
    let mut map = vec![base; w * h];
    for &(lx, ly, radius, color, intensity) in &fx.lights {
        let x0 = (lx - radius).floor().max(0.0) as usize;
        let x1 = ((lx + radius).ceil().max(0.0) as usize).min(w);
        let y0 = (ly - radius).floor().max(0.0) as usize;
        let y1 = ((ly + radius).ceil().max(0.0) as usize).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - lx;
                let dy = y as f32 + 0.5 - ly;
                let reach = light_falloff((dx * dx + dy * dy).sqrt() / radius) * intensity;
                if reach > 0.0 {
                    let cell = &mut map[y * w + x];
                    for (channel, &c) in cell.iter_mut().zip(&color) {
                        *channel += c as f32 / 255.0 * reach;
                    }
                }
            }
        }
    }
    for (pixel, light) in canvas.iter_mut().zip(&map) {
        let c = unpack(*pixel);
        *pixel = pack([
            c[0] * light[0].min(1.0),
            c[1] * light[1].min(1.0),
            c[2] * light[2].min(1.0),
        ]);
    }
}

/// Bloom: the pixels brighter than `threshold` (with a soft knee), blurred about `radius`
/// pixels wide (three box passes, close to the GPU's gaussian), added back × `intensity`.
pub(crate) fn apply_bloom(
    canvas: &mut [u32],
    w: usize,
    h: usize,
    (threshold, intensity, radius): (f32, f32, f32),
) {
    if intensity <= 0.0 || w == 0 || h == 0 {
        return;
    }
    let knee = (1.0 - threshold).max(1e-3);
    let mut bright: Vec<[f32; 3]> = canvas
        .iter()
        .map(|&p| {
            let c = unpack(p);
            let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let k = ((luma - threshold) / knee).clamp(0.0, 1.0);
            [c[0] * k, c[1] * k, c[2] * k]
        })
        .collect();
    let box_radius = ((radius / 3.0).round() as usize).max(1);
    for _ in 0..3 {
        box_blur(&mut bright, w, h, box_radius);
    }
    for (pixel, glow) in canvas.iter_mut().zip(&bright) {
        let c = unpack(*pixel);
        *pixel = pack([
            c[0] + glow[0] * intensity,
            c[1] + glow[1] * intensity,
            c[2] + glow[2] * intensity,
        ]);
    }
}

/// A separable box blur of `radius` in place: rows, then columns, a running sum each.
fn box_blur(img: &mut [[f32; 3]], w: usize, h: usize, radius: usize) {
    let mut line: Vec<[f32; 3]> = Vec::with_capacity(w.max(h));
    let blur_line = |line: &mut Vec<[f32; 3]>, read: &dyn Fn(usize) -> [f32; 3], n: usize| {
        line.clear();
        let width = (2 * radius + 1) as f32;
        let mut sum = [0.0f32; 3];
        let at = |i: isize| read(i.clamp(0, n as isize - 1) as usize);
        for i in -(radius as isize)..=(radius as isize) {
            let c = at(i);
            for k in 0..3 {
                sum[k] += c[k];
            }
        }
        for i in 0..n as isize {
            line.push([sum[0] / width, sum[1] / width, sum[2] / width]);
            let (add, sub) = (at(i + radius as isize + 1), at(i - radius as isize));
            for k in 0..3 {
                sum[k] += add[k] - sub[k];
            }
        }
    };
    for y in 0..h {
        let row: Vec<[f32; 3]> = img[y * w..(y + 1) * w].to_vec();
        blur_line(&mut line, &|x| row[x], w);
        img[y * w..(y + 1) * w].copy_from_slice(&line);
    }
    for x in 0..w {
        let column = Vec::from_fn(h, |y| img[y * w + x]);
        blur_line(&mut line, &|y| column[y], h);
        for (y, c) in line.iter().enumerate() {
            img[y * w + x] = *c;
        }
    }
}

/// Where the CRT pass samples for output position `(u, v)` (0..1): the picture bulged by
/// `curvature`, or `None` past the tube's edge (drawn black). The shader's `crt_uv`.
pub(crate) fn crt_uv(u: f32, v: f32, curvature: f32) -> Option<(f32, f32)> {
    let (cx, cy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
    let (bx, by) = (
        cx * (1.0 + curvature * cy * cy * 0.25),
        cy * (1.0 + curvature * cx * cx * 0.25),
    );
    let (su, sv) = (bx * 0.5 + 0.5, by * 0.5 + 0.5);
    ((0.0..=1.0).contains(&su) && (0.0..=1.0).contains(&sv)).then_some((su, sv))
}

/// How much of a pixel's brightness the CRT keeps at `(u, v)`, row `row`: every other
/// row dimmed by `scanlines`, the corners by `vignette`. The shader's `crt_shade`.
pub(crate) fn crt_shade(u: f32, v: f32, row: u32, scanlines: f32, vignette: f32) -> f32 {
    let line = if row % 2 == 1 { 1.0 - scanlines } else { 1.0 };
    let (dx, dy) = (u - 0.5, v - 0.5);
    let edge = (1.0 - vignette * (dx * dx + dy * dy) * 2.0).clamp(0.0, 1.0);
    line * edge
}

/// The CRT pass: each output pixel sampled through `crt_uv` and shaded by `crt_shade`.
pub(crate) fn apply_crt(
    canvas: &mut [u32],
    w: usize,
    h: usize,
    (scanlines, curvature, vignette): (f32, f32, f32),
) {
    if w == 0 || h == 0 {
        return;
    }
    let source = canvas.to_vec();
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            canvas[y * w + x] = match crt_uv(u, v, curvature) {
                None => 0,
                Some((su, sv)) => {
                    let sx = ((su * w as f32) as usize).min(w - 1);
                    let sy = ((sv * h as f32) as usize).min(h - 1);
                    let c = unpack(source[sy * w + sx]);
                    let k = crt_shade(u, v, y as u32, scanlines, vignette);
                    pack([c[0] * k, c[1] * k, c[2] * k])
                }
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitter(id: u32, rate: f32, max: u32) -> Op {
        Op::Emitter {
            id,
            x: 10.0,
            y: 20.0,
            spec: EmitterSpec {
                rate,
                max,
                life: [0.5, 0.5],
                ..EmitterSpec::default()
            },
        }
    }

    #[test]
    fn the_random_numbers_are_uniform_floats_that_differ_by_slot_and_draw() {
        assert_eq!(hash(0), 0);
        let r = rand(7, 3, 1);
        assert!((0.0..1.0).contains(&r));
        assert_ne!(rand(7, 3, 1), rand(7, 3, 2));
    }

    #[test]
    fn a_burst_spawns_once_per_arrived_frame_not_per_paint() {
        let mut book = EmitterBook::default();
        let frame = vec![emitter(1, 0.0, 64), Op::Burst { id: 1, count: 10 }];
        book.frame_arrived(&frame);
        let first = book.step(&frame);
        assert_eq!(first[0].spawn_count, 10);
        let repaint = book.step(&frame);
        assert_eq!(
            repaint[0].spawn_count, 0,
            "a repaint of the same frame bursts nothing"
        );
    }

    #[test]
    fn spawns_walk_the_ring_and_wrap() {
        let mut book = EmitterBook::default();
        let frame = vec![emitter(1, 0.0, 8), Op::Burst { id: 1, count: 5 }];
        book.frame_arrived(&frame);
        assert_eq!(book.step(&frame)[0].spawn_first, 0);
        book.frame_arrived(&frame);
        let second = book.step(&frame)[0];
        assert_eq!((second.spawn_first, second.spawn_count), (5, 5));
        book.frame_arrived(&frame);
        assert_eq!(
            book.step(&frame)[0].spawn_first,
            2,
            "5 + 5 wraps to 2 in a ring of 8"
        );
    }

    #[test]
    fn an_emitter_the_frame_drops_lives_until_its_particles_have_died() {
        let mut book = EmitterBook::default();
        let lit = vec![emitter(1, 0.0, 8), Op::Burst { id: 1, count: 3 }];
        book.frame_arrived(&lit);
        book.step(&lit);
        let steps = book.step(&[]);
        assert_eq!(steps.len(), 1);
        assert!(!steps[0].present && steps[0].spawn_count == 0);
        assert!(book.alive());
    }

    #[test]
    fn a_cpu_particle_moves_by_its_velocity_and_dies_at_its_life() {
        let step = EmitterStep {
            id: 1,
            x: 0.0,
            y: 0.0,
            spec: EmitterSpec {
                speed: [100.0, 100.0],
                spread: 0.0,
                angle: 0.0,
                life: [0.25, 0.25],
                ..EmitterSpec::default()
            },
            dt: 0.1,
            spawn_first: 0,
            spawn_count: 1,
            seed: 1,
            present: true,
        };
        let mut particles = CpuParticles::default();
        particles.step(&[step]);
        let (x, y, _, _) = particles.particles(1).next().unwrap();
        assert!(
            (x - 10.0).abs() < 1e-4 && y.abs() < 1e-4,
            "moved 100 px/s × 0.1 s right"
        );
        let idle = EmitterStep {
            spawn_count: 0,
            ..step
        };
        // ages 0.1 → 0.2 → 0.3, past its 0.25
        particles.step(&[idle]);
        particles.step(&[idle]);
        assert_eq!(particles.particles(1).count(), 0, "dead after its 0.25 s");
    }

    #[test]
    fn a_light_brightens_its_centre_and_the_ambient_the_rest() {
        let (w, h) = (9, 9);
        let mut canvas = vec![0x00ff_ffffu32; w * h];
        let fx = FrameEffects {
            ambient: Some([51, 51, 51]),
            lights: vec![(4.5, 4.5, 4.0, [255, 255, 255], 1.0)],
            ..FrameEffects::default()
        };
        apply_lighting(&mut canvas, w, h, &fx);
        assert_eq!(canvas[4 * w + 4], 0x00ff_ffff, "full light at the centre");
        assert_eq!(canvas[0], 0x0033_3333, "the ambient where no light reaches");
    }

    #[test]
    fn the_crt_bulge_leaves_the_centre_and_blackens_past_the_glass() {
        assert_eq!(crt_uv(0.5, 0.5, 0.5), Some((0.5, 0.5)));
        assert_eq!(crt_uv(0.0, 0.0, 1.0), None);
        assert!(crt_shade(0.5, 0.5, 1, 0.5, 0.0) == 0.5);
    }

    #[test]
    fn the_overlay_splits_only_the_top_level() {
        let frame = vec![Op::Clear, Op::Ambient { color: [0, 0, 0] }, Op::Overlay];
        let fx = FrameEffects::of(&frame);
        assert_eq!(fx.overlay_at, Some(2));
        assert!(fx.any());
        assert!(!FrameEffects::of(&[Op::Clear]).any());
    }
}
