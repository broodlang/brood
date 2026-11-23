//! A scene marched into a G-buffer: `b2d-sculpt-view`'s `gbuffer`, byte for byte. Every pixel
//! (and every subsample) is computed on its own from the scene alone, so splitting the picture
//! among threads changes nothing in the bytes; the passes that read neighbours (flagging pixels
//! to subsample, lending light to a subsample) read the finished first pass.
//!
//! The layout, five RGBA texels a pixel (`b2d-sculpt-view`'s `encoded`): normal and coverage;
//! wide normal and occlusion; three lights' shadows and thinness; material index, 16-bit depth
//! and 255; the albedo's ratio to its material's and 255. A flagged pixel's fourth texel has
//! alpha 254, and its four subsamples follow the picture's rows, padded to whole rows.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::Scene;
use super::evaluate::{brood_abs, brood_max, brood_min, clamp01, length3, whole};

/// A light the bake measures shadows for: the unit direction it shines from, and the softness
/// of its shadow (none: it casts none).
#[derive(Clone, Copy, Debug)]
pub struct Light {
    pub towards: [f64; 3],
    pub softness: Option<f64>,
}

/// When pixels are marched again at four subsamples (`gbuffer`'s `:antialias`).
#[derive(Clone, Debug)]
pub struct Subsampling {
    /// A step in depth between neighbours past which both are flagged (units).
    pub depth_step: f64,
    /// Normals further apart than this cosine flag both.
    pub least_facing: f64,
    /// Material bytes every pixel of which is flagged.
    pub always: Vec<u8>,
}

/// What to bake: the scene from a view, over a picture of `columns` × `rows` pixels.
pub struct BakeRequest<'a> {
    pub scene: &'a Scene,
    /// Per region index, the scene that darkens a point of that region (shadows, occlusion,
    /// thinness); none, or a region past the end, for the scene itself.
    pub castings: Vec<Option<&'a Scene>>,
    pub centre: [f64; 3],
    /// The view's right, down and forward axes.
    pub axes: [[f64; 3]; 3],
    /// The picture's top-left, in picture units from the centre.
    pub at: [f64; 2],
    /// Units a pixel.
    pub pixel: f64,
    pub columns: usize,
    pub rows: usize,
    pub lights: Vec<Light>,
    pub subsampling: Option<Subsampling>,
    pub workers: usize,
}

/// The G-buffer's bytes, its height in rows of texels, and how many pixels were subsampled.
pub struct Baked {
    pub rgba: Vec<u8>,
    pub height: usize,
    pub subsamples: usize,
}

const PIXEL_BYTES: usize = 20;
const STEPS: i64 = 160;
const RELAXATION: f64 = 1.5;
const DEPTH_RANGE: f64 = 64.0;
const EMPTY: [u8; PIXEL_BYTES] = [0; PIXEL_BYTES];
const SUBSAMPLE_OFFSETS: [[f64; 2]; 4] = [
    [0.125, -0.375],
    [0.375, 0.125],
    [-0.125, 0.375],
    [-0.375, -0.125],
];
const NEIGHBOURS: [[i64; 2]; 9] = [
    [0, 0],
    [-1, 0],
    [1, 0],
    [0, -1],
    [0, 1],
    [-1, -1],
    [1, -1],
    [-1, 1],
    [1, 1],
];

type Vector = [f64; 3];

#[inline]
fn dot(a: &Vector, b: &Vector) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `along`: `point` moved `distance` along `direction`.
#[inline]
fn along(point: &Vector, direction: &Vector, distance: f64) -> Vector {
    [
        point[0] + distance * direction[0],
        point[1] + distance * direction[1],
        point[2] + distance * direction[2],
    ]
}

fn unit(vector: &Vector) -> Vector {
    let length = length3(vector[0], vector[1], vector[2]);
    [vector[0] / length, vector[1] / length, vector[2] / length]
}

/// `inside-bounds`: `[entering leaving]` along the ray within the box.
fn inside_bounds(bounds: &[f64; 6], origin: &Vector, direction: &Vector) -> (f64, f64) {
    let mut span = (0.0, 1000000.0);
    for axis in 0..3 {
        let (low, high) = (bounds[axis], bounds[axis + 3]);
        let (entering, leaving) = span;
        span = if brood_abs(direction[axis]) < 0.000001 {
            if origin[axis] >= low && origin[axis] <= high {
                span
            } else {
                (1.0, 0.0)
            }
        } else {
            let near = (low - origin[axis]) / direction[axis];
            let far = (high - origin[axis]) / direction[axis];
            (
                brood_max(entering, brood_min(near, far)),
                brood_min(leaving, brood_max(near, far)),
            )
        };
    }
    span
}

/// The march's answer: where along the ray it hit (none for a miss), how near it came and where.
struct Traced {
    hit: Option<f64>,
    nearest: f64,
    nearest_at: f64,
}

/// `march`: sphere tracing, over-relaxed far from the surface, stepping back on an overshoot.
fn march(
    scene: &Scene,
    origin: &Vector,
    direction: &Vector,
    entering: f64,
    leaving: f64,
    epsilon: f64,
) -> Traced {
    let mut travelled = entering;
    let mut step = 0;
    let mut nearest = 1000000.0;
    let mut nearest_at = entering;
    let mut previous_gap = 1000000.0;
    let mut stride = 0.0;
    let mut relaxed = true;
    loop {
        if travelled > leaving || step >= STEPS {
            return Traced {
                hit: None,
                nearest,
                nearest_at,
            };
        }
        let gap = scene.distance(
            origin[0] + direction[0] * travelled,
            origin[1] + direction[1] * travelled,
            origin[2] + direction[2] * travelled,
        );
        if stride > 0.9 * previous_gap && 0.9 * (gap + previous_gap) < stride {
            let back = travelled - stride;
            let safe = 0.9 * previous_gap;
            travelled = back + safe;
            step += 1;
            stride = safe;
            relaxed = false;
        } else if gap < epsilon {
            return Traced {
                hit: Some(travelled),
                nearest,
                nearest_at,
            };
        } else {
            let next_stride = if relaxed && gap > 10.0 * epsilon && gap > 0.6 * previous_gap {
                0.9 * RELAXATION * gap
            } else {
                0.9 * gap
            };
            let next_nearest = brood_min(gap, nearest);
            if gap < nearest {
                nearest_at = travelled;
            }
            nearest = next_nearest;
            travelled += next_stride;
            step += 1;
            previous_gap = gap;
            stride = next_stride;
        }
    }
}

/// `normal-at`: the gradient from a tetrahedron of four samples.
fn normal_at(scene: &Scene, point: &Vector, epsilon: f64) -> Vector {
    let [x, y, z] = *point;
    let a = scene.distance(x + epsilon, y - epsilon, z - epsilon);
    let b = scene.distance(x - epsilon, y - epsilon, z + epsilon);
    let c = scene.distance(x - epsilon, y + epsilon, z - epsilon);
    let d = scene.distance(x + epsilon, y + epsilon, z + epsilon);
    let gradient = [a - b - c + d, 0.0 - a - b + c + d, b - a - c + d];
    if dot(&gradient, &gradient) < 0.0000000001 {
        [0.0, 0.0, 1.0]
    } else {
        unit(&gradient)
    }
}

/// `shadow-from`: how much light reaches, marched towards it.
#[allow(clippy::too_many_arguments)]
fn shadow_from(
    scene: &Scene,
    start: &Vector,
    towards: &Vector,
    softness: f64,
    leaving: f64,
) -> f64 {
    let mut travelled = 0.1;
    let mut lit: f64 = 1.0;
    let mut step = 0;
    let mut previous_gap = 1000000.0;
    let mut stride = 0.0;
    let mut stretched = false;
    loop {
        if travelled > leaving || step >= 64 || lit < 0.01 {
            return lit;
        }
        let gap = scene.distance(
            start[0] + towards[0] * travelled,
            start[1] + towards[1] * travelled,
            start[2] + towards[2] * travelled,
        );
        if stretched && gap + previous_gap < stride {
            travelled = travelled - stride + previous_gap;
            step += 1;
            stride = previous_gap;
            stretched = false;
        } else if gap < 0.001 {
            return 0.0;
        } else {
            let least = 0.05 + 0.02 * travelled;
            let relaxed = RELAXATION * gap;
            let now_stretched = gap > 0.6 * previous_gap && relaxed > least;
            let next_stride = if now_stretched {
                relaxed
            } else {
                brood_max(gap, least)
            };
            lit = brood_min(lit, softness * gap / travelled);
            travelled += next_stride;
            step += 1;
            previous_gap = gap;
            stride = next_stride;
            stretched = now_stretched;
        }
    }
}

fn shadow(casting: &Scene, point: &Vector, normal: &Vector, light: &Light) -> f64 {
    let Some(softness) = light.softness else {
        return 1.0;
    };
    let start = along(point, normal, 0.08);
    let (_, leaving) = inside_bounds(&casting.bounds, &start, &light.towards);
    shadow_from(
        casting,
        &start,
        &light.towards,
        softness,
        brood_min(200.0, leaving),
    )
}

fn occlusion(casting: &Scene, point: &Vector, normal: &Vector) -> f64 {
    let mut dark = 0.0;
    let mut power = 1.0;
    for index in 1..=5 {
        let out = 0.35 * index as f64;
        power *= 2.0;
        let sample = along(point, normal, out);
        dark += (out - casting.distance(sample[0], sample[1], sample[2])) / power;
    }
    clamp01(1.0 - 1.6 * dark)
}

fn thinness(casting: &Scene, point: &Vector, towards: &Vector) -> f64 {
    let sample = along(point, towards, -1.5);
    clamp01(1.0 + casting.distance(sample[0], sample[1], sample[2]) / 1.5)
}

/// What a surface records beside its normal: `surface-at`'s, or a neighbour's lent.
struct Surface {
    wide: Vector,
    occlusion: f64,
    shadows: Vec<f64>,
    thinness: f64,
}

#[inline]
fn byte_of(value: f64) -> u8 {
    whole(255.0 * clamp01(value)) as u8
}

#[inline]
fn byte_of_signed(value: f64) -> u8 {
    byte_of(0.5 * (value + 1.0))
}

#[inline]
fn signed_of(byte: u8) -> f64 {
    2.0 * byte as f64 / 255.0 - 1.0
}

/// `depth-at`: the depth the bytes from `start` keep.
#[inline]
fn depth_at(bytes: &[u8], start: usize) -> f64 {
    2.0 * DEPTH_RANGE
        * ((256 * bytes[start + 13] as i64 + bytes[start + 14] as i64) as f64 / 65535.0)
        - DEPTH_RANGE
}

impl BakeRequest<'_> {
    fn casting_at(&self, point: &Vector) -> &Scene {
        match self.scene.region(point[0], point[1], point[2]) {
            Some(region) => self
                .castings
                .get(region)
                .copied()
                .flatten()
                .unwrap_or(self.scene),
            None => self.scene,
        }
    }

    /// `surface-at`'s measured light.
    fn surface_at(&self, point: &Vector, normal: &Vector, scatter: Option<f64>) -> Surface {
        let casting = self.casting_at(point);
        Surface {
            wide: match scatter {
                None => *normal,
                Some(epsilon) => normal_at(self.scene, point, epsilon),
            },
            shadows: self
                .lights
                .iter()
                .map(|light| {
                    if dot(normal, &light.towards) <= -0.3 {
                        1.0
                    } else {
                        shadow(casting, point, normal, light)
                    }
                })
                .collect(),
            thinness: match self.lights.first() {
                None => 0.0,
                Some(light) => thinness(casting, point, &light.towards),
            },
            occlusion: occlusion(casting, point, normal),
        }
    }

    /// `marched`: the twenty bytes of the surface the ray through picture point (u v) meets.
    /// `lender`, for a subsample: the first pass and the pixel's column and row.
    fn marched(
        &self,
        u: f64,
        v: f64,
        softness: f64,
        lender: Option<(&[u8], usize, usize)>,
    ) -> [u8; PIXEL_BYTES] {
        let [right, down, forward] = &self.axes;
        let origin = along(
            &along(&along(&self.centre, right, u), down, v),
            forward,
            -1000.0,
        );
        let (entering, leaving) = inside_bounds(&self.scene.bounds, &origin, forward);
        if entering > leaving {
            return EMPTY;
        }
        let traced = march(
            self.scene,
            &origin,
            forward,
            entering,
            leaving,
            0.2 * softness,
        );
        let coverage = match traced.hit {
            None => clamp01(0.5 - traced.nearest / softness),
            Some(_) => 1.0,
        };
        if coverage <= 0.0 {
            return EMPTY;
        }
        let travel = traced.hit.unwrap_or(traced.nearest_at);
        let point = along(&origin, forward, travel);
        let material_index = self.scene.material_at(point[0], point[1], point[2]);
        let material = self.scene.materials[material_index];
        let weight = 1.0 * material.alpha;
        if weight <= 0.0 {
            return EMPTY;
        }
        let albedo = self
            .scene
            .tinted(material_index, point[0], point[1], point[2]);
        let normal = normal_at(self.scene, &point, 0.5 * softness);
        let depth = travel - 1000.0;
        let lent = lender.and_then(|(base, column, row)| {
            self.lender_in(base, column, row, material.index, depth)
        });
        let surface = match lent {
            None => self.surface_at(&point, &normal, material.scatter),
            Some((base, start)) => Surface {
                wide: [
                    signed_of(base[start + 4]),
                    signed_of(base[start + 5]),
                    signed_of(base[start + 6]),
                ],
                occlusion: base[start + 7] as f64 / 255.0,
                shadows: vec![
                    base[start + 8] as f64 / 255.0,
                    base[start + 9] as f64 / 255.0,
                    base[start + 10] as f64 / 255.0,
                ],
                thinness: base[start + 11] as f64 / 255.0,
            },
        };
        encoded(
            &normal,
            &surface,
            coverage * weight,
            material.index,
            depth,
            &albedo,
            material.base.as_ref(),
        )
    }

    /// `lender-in`: the pixel, of (column row) and its eight neighbours, showing material
    /// `material` nearest `depth` and within the depth step of it.
    fn lender_in<'b>(
        &self,
        base: &'b [u8],
        column: usize,
        row: usize,
        material: u8,
        depth: f64,
    ) -> Option<(&'b [u8], usize)> {
        let depth_step = self
            .subsampling
            .as_ref()
            .map_or(0.0, |subsampling| subsampling.depth_step);
        let mut best: Option<usize> = None;
        let mut best_gap = depth_step;
        for [across, down] in NEIGHBOURS {
            let near_column = column as i64 + across;
            let near_row = row as i64 + down;
            if near_column < 0
                || near_column >= self.columns as i64
                || near_row < 0
                || near_row >= self.rows as i64
            {
                continue;
            }
            let start = PIXEL_BYTES * (near_column as usize + near_row as usize * self.columns);
            if base[start + 3] == 0 || base[start + 12] != material {
                continue;
            }
            let gap = brood_abs(depth_at(base, start) - depth);
            if gap < best_gap {
                best = Some(start);
                best_gap = gap;
            }
        }
        best.map(|start| (base, start))
    }
}

/// `encoded`: a surface's twenty bytes.
fn encoded(
    normal: &Vector,
    surface: &Surface,
    coverage: f64,
    material: u8,
    depth: f64,
    albedo: &[f64; 3],
    base: Option<&[f64; 3]>,
) -> [u8; PIXEL_BYTES] {
    let shadow = |index: usize| {
        surface
            .shadows
            .get(index)
            .map_or(255, |value| byte_of(*value))
    };
    let deep = whole(65535.0 * clamp01((depth + DEPTH_RANGE) / (2.0 * DEPTH_RANGE)));
    let albedo_byte = |index: usize| {
        let base = 1.0 * base.unwrap_or(albedo)[index];
        if base <= 0.0 {
            128
        } else {
            whole(127.5 * brood_max(0.0, brood_min(2.0, albedo[index] / base))) as u8
        }
    };
    [
        byte_of_signed(normal[0]),
        byte_of_signed(normal[1]),
        byte_of_signed(normal[2]),
        byte_of(coverage),
        byte_of_signed(surface.wide[0]),
        byte_of_signed(surface.wide[1]),
        byte_of_signed(surface.wide[2]),
        byte_of(surface.occlusion),
        shadow(0),
        shadow(1),
        shadow(2),
        byte_of(surface.thinness),
        material,
        (deep / 256) as u8,
        (deep % 256) as u8,
        255,
        albedo_byte(0),
        albedo_byte(1),
        albedo_byte(2),
        255,
    ]
}

/// `differs?`: whether two pixels show different things.
fn differs(bytes: &[u8], start: usize, other: usize, depth_step: f64, least_facing: f64) -> bool {
    bytes[start + 12] != bytes[other + 12]
        || brood_abs(depth_at(bytes, start) - depth_at(bytes, other)) > depth_step
        || signed_of(bytes[start]) * signed_of(bytes[other])
            + signed_of(bytes[start + 1]) * signed_of(bytes[other + 1])
            + signed_of(bytes[start + 2]) * signed_of(bytes[other + 2])
            < least_facing
}

/// `flagged`: the pixels to subsample, in order.
fn flagged(bytes: &[u8], columns: usize, rows: usize, subsampling: &Subsampling) -> Vec<usize> {
    let pixels = columns * rows;
    let covered = |index: usize| bytes[3 + PIXEL_BYTES * index] > 0;
    let mut marks = vec![false; pixels];
    for index in 0..pixels {
        if !covered(index) {
            continue;
        }
        let start = PIXEL_BYTES * index;
        if subsampling.always.contains(&bytes[start + 12]) {
            marks[index] = true;
        }
        let right = index + 1;
        if index % columns + 1 < columns
            && covered(right)
            && differs(
                bytes,
                start,
                start + PIXEL_BYTES,
                subsampling.depth_step,
                subsampling.least_facing,
            )
        {
            marks[index] = true;
            marks[right] = true;
        }
        let below = index + columns;
        if below < pixels
            && covered(below)
            && differs(
                bytes,
                start,
                PIXEL_BYTES * below,
                subsampling.depth_step,
                subsampling.least_facing,
            )
        {
            marks[index] = true;
            marks[below] = true;
        }
    }
    (0..pixels).filter(|&index| marks[index]).collect()
}

/// `items` values computed by `compute`, each `width` bytes, on `workers` threads in chunks; the
/// bytes land in order whatever thread computed them.
fn computed<F>(items: usize, width: usize, workers: usize, compute: F) -> Vec<u8>
where
    F: Fn(usize, &mut [u8]) + Sync,
{
    let mut out = vec![0u8; items * width];
    const CHUNK: usize = 32;
    let chunks = items.div_ceil(CHUNK);
    let workers = workers.clamp(1, chunks.max(1));
    if workers <= 1 || cfg!(target_arch = "wasm32") {
        for (index, slot) in out.chunks_exact_mut(width).enumerate() {
            compute(index, slot);
        }
        return out;
    }
    let next = AtomicUsize::new(0);
    let finished: Vec<Vec<(usize, Vec<u8>)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let chunk = next.fetch_add(1, Ordering::Relaxed);
                        if chunk >= chunks {
                            break;
                        }
                        let first = chunk * CHUNK;
                        let last = (first + CHUNK).min(items);
                        let mut bytes = vec![0u8; (last - first) * width];
                        for (offset, slot) in bytes.chunks_exact_mut(width).enumerate() {
                            compute(first + offset, slot);
                        }
                        done.push((chunk, bytes));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("an sdf bake worker panicked"))
            .collect()
    });
    for (chunk, bytes) in finished.into_iter().flatten() {
        let start = chunk * CHUNK * width;
        out[start..start + bytes.len()].copy_from_slice(&bytes);
    }
    out
}

/// March `request`'s picture into a G-buffer.
pub fn bake(request: &BakeRequest) -> Baked {
    let (columns, rows, pixel) = (request.columns, request.rows, request.pixel);
    let [at_u, at_v] = request.at;
    let mut base = computed(
        columns * rows,
        PIXEL_BYTES,
        request.workers,
        |index, slot| {
            let u = at_u + pixel * ((index % columns) as f64 + 0.5);
            let v = at_v + pixel * ((index / columns) as f64 + 0.5);
            slot.copy_from_slice(&request.marched(u, v, pixel, None));
        },
    );
    let flags = match &request.subsampling {
        Some(subsampling) if columns > 0 => flagged(&base, columns, rows, subsampling),
        _ => Vec::new(),
    };
    let records = {
        let first_pass: &[u8] = &base;
        computed(
            flags.len(),
            4 * PIXEL_BYTES,
            request.workers,
            |number, slot| {
                let index = flags[number];
                let (column, row) = (index % columns, index / columns);
                for (subsample, [across, down]) in SUBSAMPLE_OFFSETS.iter().enumerate() {
                    let u = at_u + pixel * (column as f64 + 0.5 + across);
                    let v = at_v + pixel * (row as f64 + 0.5 + down);
                    let bytes = request.marched(u, v, 0.5 * pixel, Some((first_pass, column, row)));
                    slot[subsample * PIXEL_BYTES..(subsample + 1) * PIXEL_BYTES]
                        .copy_from_slice(&bytes);
                }
            },
        )
    };
    for &index in &flags {
        base[15 + PIXEL_BYTES * index] = 254;
    }
    let row_length = PIXEL_BYTES * columns;
    if !flags.is_empty() {
        base.extend_from_slice(&records);
        let short = (row_length - base.len() % row_length) % row_length;
        base.resize(base.len() + short, 0);
    }
    let height = base.len().checked_div(row_length).unwrap_or(rows);
    Baked {
        rgba: base,
        height,
        subsamples: flags.len(),
    }
}
