//! A scene evaluated at a point: `b2d-sculpt`'s shapes, joins, bounding tree, region rule,
//! material rule and colour zones, each operation in the Brood source's order so the answer is
//! the Brood one bit for bit. The helpers at the top are Brood's own `min`, `max`, `abs` and
//! `sqrt`, which differ from Rust's at signed zeros and ties.

use super::{Cull, Field, Grid, Op, Part, Scene, Shape, Tree};

/// Brood's `math/min`: the second only when the first orders after it.
#[inline]
pub(super) fn brood_min(a: f64, b: f64) -> f64 {
    if a > b { b } else { a }
}

/// Brood's `math/max`: the second only when the first orders before it.
#[inline]
pub(super) fn brood_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// Brood's `math/abs`: `(if (< n 0) (- n) n)`.
#[inline]
pub(super) fn brood_abs(value: f64) -> f64 {
    if value < 0.0 { 0.0 - value } else { value }
}

/// Brood's `math/sqrt`: 0.0 at and below zero (Brood raises below; a scene never reaches it).
#[inline]
pub(super) fn brood_sqrt(value: f64) -> f64 {
    if value <= 0.0 { 0.0 } else { value.sqrt() }
}

/// `clamp01`, as written in Brood (a NaN passes through, as it does there).
#[inline]
#[allow(clippy::manual_clamp)]
pub(super) fn clamp01(value: f64) -> f64 {
    if value < 0.0 {
        0.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

#[inline]
pub(super) fn mix(from: f64, to: f64, amount: f64) -> f64 {
    from + amount * (to - from)
}

#[inline]
pub(super) fn length3(x: f64, y: f64, z: f64) -> f64 {
    brood_sqrt(x * x + y * y + z * z)
}

#[inline]
fn smoothstep01(value: f64) -> f64 {
    let value = clamp01(value);
    value * value * (3.0 - 2.0 * value)
}

/// `b2d-math/whole`: the nearest integer, halves away from zero.
#[inline]
pub(super) fn whole(value: f64) -> i64 {
    if value < 0.0 {
        -((0.5 - value).floor() as i64)
    } else {
        (value + 0.5).floor() as i64
    }
}

#[inline]
fn sign_of(value: f64) -> f64 {
    if value < 0.0 { -1.0 } else { 1.0 }
}

/// A part's reach beyond its bound: `(+ (or blend 0.0) (math/abs (or amount 0.0)) 0.05)`.
pub(super) fn reach_of(blend: Option<f64>, amount: Option<f64>) -> f64 {
    blend.unwrap_or(0.0) + brood_abs(amount.unwrap_or(0.0)) + 0.05
}

/// `enclosing`: the sphere round spheres `bounds`, its middle their middles' average.
pub(super) fn enclosing(bounds: &[[f64; 4]]) -> ([f64; 3], f64) {
    let total = bounds.len() as f64;
    let average = |index: usize| bounds.iter().fold(0.0, |sum, bound| sum + bound[index]) / total;
    let centre = [average(0), average(1), average(2)];
    let radius = bounds.iter().fold(0.0, |most, bound| {
        brood_max(
            most,
            length3(
                bound[0] - centre[0],
                bound[1] - centre[1],
                bound[2] - centre[2],
            ) + bound[3],
        )
    });
    (centre, radius)
}

// --- shapes ------------------------------------------------------------------------------------

fn sphere(x: f64, y: f64, z: f64, centre: &[f64; 3], radius: f64) -> f64 {
    length3(x - centre[0], y - centre[1], z - centre[2]) - radius
}

fn ellipsoid(x: f64, y: f64, z: f64, centre: &[f64; 3], radii: &[f64; 3]) -> f64 {
    let across_x = (x - centre[0]) / radii[0];
    let across_y = (y - centre[1]) / radii[1];
    let across_z = (z - centre[2]) / radii[2];
    let scaled = length3(across_x, across_y, across_z);
    let bent = length3(
        across_x / radii[0],
        across_y / radii[1],
        across_z / radii[2],
    );
    if bent < 0.000001 {
        0.0 - brood_min(radii[0], brood_min(radii[1], radii[2]))
    } else {
        scaled * (scaled - 1.0) / bent
    }
}

fn limb(x: f64, y: f64, z: f64, from: &[f64; 3], to: &[f64; 3], radii: &[f64; 2]) -> f64 {
    let [ax, ay, az] = *from;
    let [start_radius, end_radius] = *radii;
    let span_x = to[0] - ax;
    let span_y = to[1] - ay;
    let span_z = to[2] - az;
    let squared = span_x * span_x + span_y * span_y + span_z * span_z;
    let narrowing = start_radius - end_radius;
    let reach = squared - narrowing * narrowing;
    let inverse = 1.0 / squared;
    let from_x = x - ax;
    let from_y = y - ay;
    let from_z = z - az;
    let along = from_x * span_x + from_y * span_y + from_z * span_z;
    let past = along - squared;
    let off_x = from_x * squared - span_x * along;
    let off_y = from_y * squared - span_y * along;
    let off_z = from_z * squared - span_z * along;
    let off = off_x * off_x + off_y * off_y + off_z * off_z;
    let along_squared = along * along * squared;
    let past_squared = past * past * squared;
    let bound = sign_of(narrowing) * narrowing * narrowing * off;
    if sign_of(past) * reach * past_squared > bound {
        brood_sqrt(off + past_squared) * inverse - end_radius
    } else if sign_of(along) * reach * along_squared < bound {
        brood_sqrt(off + along_squared) * inverse - start_radius
    } else {
        (brood_sqrt(off * reach * inverse) + along * narrowing) * inverse - start_radius
    }
}

fn rounded_box(x: f64, y: f64, z: f64, centre: &[f64; 3], half: &[f64; 3], rounding: f64) -> f64 {
    let out_x = brood_abs(x - centre[0]) - half[0] + rounding;
    let out_y = brood_abs(y - centre[1]) - half[1] + rounding;
    let out_z = brood_abs(z - centre[2]) - half[2] + rounding;
    let outside = length3(
        brood_max(out_x, 0.0),
        brood_max(out_y, 0.0),
        brood_max(out_z, 0.0),
    );
    let inside = brood_min(brood_max(out_x, brood_max(out_y, out_z)), 0.0);
    outside + inside - rounding
}

fn outline(x: f64, y: f64, points: &[[f64; 2]]) -> f64 {
    let mut best = 1000000000.0;
    let mut sign = 1.0;
    let mut previous = points.len() - 1;
    for (index, corner) in points.iter().enumerate() {
        let [corner_x, corner_y] = *corner;
        let [previous_x, previous_y] = points[previous];
        let edge_x = previous_x - corner_x;
        let edge_y = previous_y - corner_y;
        let from_x = x - corner_x;
        let from_y = y - corner_y;
        let length = edge_x * edge_x + edge_y * edge_y;
        let along = if length > 0.0 {
            clamp01((from_x * edge_x + from_y * edge_y) / length)
        } else {
            0.0
        };
        let off_x = from_x - edge_x * along;
        let off_y = from_y - edge_y * along;
        let near = off_x * off_x + off_y * off_y;
        let above = y >= corner_y;
        let below = y < previous_y;
        let left = edge_x * from_y > edge_y * from_x;
        let crosses = (above && below && left) || (!above && !below && !left);
        best = brood_min(best, near);
        if crosses {
            sign = 0.0 - sign;
        }
        previous = index;
    }
    sign * brood_sqrt(best)
}

fn plane(x: f64, y: f64, z: f64, at: &[f64; 3], normal: &[f64; 3]) -> f64 {
    (x - at[0]) * normal[0] + (y - at[1]) * normal[1] + (z - at[2]) * normal[2]
}

// --- the distance grid ---------------------------------------------------------------------------

const GRID_EASE: f64 = 2.0;

#[inline]
fn held(value: f64, low: f64, high: f64) -> f64 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

#[inline]
fn clamped_cell(value: i64, low: i64, high: i64) -> i64 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

impl Grid {
    #[inline]
    fn sample(&self, index: usize) -> f64 {
        self.distances[index] as f64
    }

    fn row(&self, base: usize, low: f64, middle: f64, high: f64) -> f64 {
        low * self.sample(base - 1) + middle * self.sample(base) + high * self.sample(base + 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn plane(
        &self,
        base: usize,
        low: f64,
        middle: f64,
        high: f64,
        z_low: f64,
        z_middle: f64,
        z_high: f64,
    ) -> f64 {
        let nx = self.dimensions[0] as usize;
        z_low * self.row(base - nx, low, middle, high)
            + z_middle * self.row(base, low, middle, high)
            + z_high * self.row(base + nx, low, middle, high)
    }

    /// `grid-inside`: the quadratic B-spline over the 27 voxels round the nearest.
    fn inside(&self, x: f64, y: f64, z: f64) -> f64 {
        let [nx, ny, nz] = self.dimensions;
        let across_x = (x - self.origin[0]) * self.inverse;
        let across_y = (y - self.origin[1]) * self.inverse;
        let across_z = (z - self.origin[2]) * self.inverse;
        let i = clamped_cell(whole(across_x), 1, nx - 2);
        let j = clamped_cell(whole(across_y), 1, ny - 2);
        let k = clamped_cell(whole(across_z), 1, nz - 2);
        let slab = (nx * nz) as usize;
        let base = (i + nx * (k + nz * j)) as usize;
        let centre = self.distances[base] as i64;
        let quantum = self.quantum;
        if brood_abs(centre as f64) > self.far_steps as f64 {
            let steps = if centre > 0 {
                centre - self.half_diagonal_steps
            } else {
                centre + self.half_diagonal_steps
            };
            return quantum * steps as f64;
        }
        let tx = held(across_x - i as f64, -0.5, 0.5);
        let ty = held(across_y - j as f64, -0.5, 0.5);
        let tz = held(across_z - k as f64, -0.5, 0.5);
        let x_low = 0.5 * (0.5 - tx) * (0.5 - tx);
        let x_middle = 0.75 - tx * tx;
        let x_high = 0.5 * (0.5 + tx) * (0.5 + tx);
        let z_low = 0.5 * (0.5 - tz) * (0.5 - tz);
        let z_middle = 0.75 - tz * tz;
        let z_high = 0.5 * (0.5 + tz) * (0.5 + tz);
        quantum
            * (0.5
                * (0.5 - ty)
                * (0.5 - ty)
                * self.plane(
                    base - slab,
                    x_low,
                    x_middle,
                    x_high,
                    z_low,
                    z_middle,
                    z_high,
                )
                + (0.75 - ty * ty)
                    * self.plane(base, x_low, x_middle, x_high, z_low, z_middle, z_high)
                + 0.5
                    * (0.5 + ty)
                    * (0.5 + ty)
                    * self.plane(
                        base + slab,
                        x_low,
                        x_middle,
                        x_high,
                        z_low,
                        z_middle,
                        z_high,
                    ))
    }

    /// `grid-distance`: inside the box the spline, outside the distance to the box joined with
    /// the spline at the nearest point of it.
    fn distance(&self, x: f64, y: f64, z: f64) -> f64 {
        let near_x = held(x, self.origin[0], self.far[0]);
        let near_y = held(y, self.origin[1], self.far[1]);
        let near_z = held(z, self.origin[2], self.far[2]);
        let out_x = x - near_x;
        let out_y = y - near_y;
        let out_z = z - near_z;
        let out = out_x * out_x + out_y * out_y + out_z * out_z;
        let at = self.inside(near_x, near_y, near_z);
        if out < 0.0000001 {
            at
        } else {
            let beyond = brood_max(at, 0.0);
            brood_sqrt(out + beyond * beyond)
        }
    }

    /// `grid-inset`: how far inside the box, negative outside.
    fn inset(&self, x: f64, y: f64, z: f64) -> f64 {
        brood_min(
            brood_min(
                brood_min(x - self.origin[0], self.far[0] - x),
                brood_min(y - self.origin[1], self.far[1] - y),
            ),
            brood_min(z - self.origin[2], self.far[2] - z),
        )
    }

    /// `grid-label`: the label byte of the voxel nearest, 0 outside the box.
    fn label(&self, x: f64, y: f64, z: f64) -> usize {
        let [nx, ny, nz] = self.dimensions;
        let i = whole((x - self.origin[0]) * self.inverse);
        let j = whole((y - self.origin[1]) * self.inverse);
        let k = whole((z - self.origin[2]) * self.inverse);
        if i < 0 || j < 0 || k < 0 || i >= nx || j >= ny || k >= nz {
            0
        } else {
            self.labels.as_bytes()[(i + nx * (k + nz * j)) as usize] as usize
        }
    }
}

impl Field {
    /// `field-distance`: the body grid, and the head grid eased in over `GRID_EASE` inside its box.
    fn distance(&self, x: f64, y: f64, z: f64) -> f64 {
        let Some(head) = &self.head else {
            return self.body.distance(x, y, z);
        };
        let inset = head.inset(x, y, z);
        if inset <= 0.0 {
            return self.body.distance(x, y, z);
        }
        let fine = head.inside(x, y, z);
        if inset >= GRID_EASE {
            fine
        } else {
            mix(
                self.body.distance(x, y, z),
                fine,
                smoothstep01(inset / GRID_EASE),
            )
        }
    }

    /// `field-label`: the head grid's within the inner half of its ease, else the body's.
    fn label(&self, x: f64, y: f64, z: f64) -> usize {
        match &self.head {
            Some(head) if head.inset(x, y, z) > 0.5 * GRID_EASE => head.label(x, y, z),
            _ => self.body.label(x, y, z),
        }
    }

    /// `grid-part-distance`, with `kept-distance` when only some labels are kept.
    fn part_distance(&self, x: f64, y: f64, z: f64) -> f64 {
        let distance = self.distance(x, y, z);
        match &self.keep {
            None => distance,
            Some(keep) => {
                if keep.get(self.label(x, y, z)).copied().unwrap_or(false) {
                    distance
                } else {
                    brood_max(distance, 0.45 * self.body.spacing)
                }
            }
        }
    }

    fn region(&self, x: f64, y: f64, z: f64) -> Option<usize> {
        self.regions.get(self.label(x, y, z)).copied().flatten()
    }
}

// --- joining ---------------------------------------------------------------------------------------

fn smooth_union(a: f64, b: f64, blend: f64) -> f64 {
    let weight = clamp01(0.5 + 0.5 * (b - a) / blend);
    mix(b, a, weight) - blend * weight * (1.0 - weight)
}

fn smooth_subtract(shape: f64, cut: f64, blend: f64) -> f64 {
    let weight = clamp01(0.5 - 0.5 * (shape + cut) / blend);
    mix(shape, 0.0 - cut, weight) + blend * weight * (1.0 - weight)
}

fn smooth_intersect(a: f64, b: f64, blend: f64) -> f64 {
    let weight = clamp01(0.5 - 0.5 * (b - a) / blend);
    mix(b, a, weight) + blend * weight * (1.0 - weight)
}

/// `hem-of`: a coat's hem width.
fn hem_of(amount: f64) -> f64 {
    brood_max(0.24, 4.0 * brood_abs(amount))
}

impl Part {
    /// The op's `:join`: the part's `distance` joined onto the distance `so_far`.
    fn join(&self, so_far: f64, distance: f64) -> f64 {
        match self.op {
            Op::Union => match self.blend {
                None => brood_min(so_far, distance),
                Some(blend) => smooth_union(so_far, distance, blend),
            },
            Op::Carve => match self.blend {
                None => brood_max(so_far, 0.0 - distance),
                Some(blend) => smooth_subtract(so_far, distance, blend),
            },
            Op::Intersect => match self.blend {
                None => brood_max(so_far, distance),
                Some(blend) => smooth_intersect(so_far, distance, blend),
            },
            Op::Swell => {
                let reach = self.blend.unwrap_or(1.0);
                so_far - self.amount() * smoothstep01((reach - distance) / (2.0 * reach))
            }
            Op::Tint => so_far,
            Op::Coat => {
                let amount = self.amount();
                let hem = hem_of(amount);
                so_far - amount * smoothstep01((0.5 * hem - distance) / hem)
            }
        }
    }

    /// The op's `:reaches?`: whether the part can change anything at a point `outside` its bound.
    fn reaches(&self, outside: f64, so_far: f64) -> bool {
        let reach = self.reach;
        match self.op {
            Op::Union => outside < so_far + reach,
            Op::Carve => outside < reach + brood_max(0.0, 0.0 - so_far),
            Op::Intersect => true,
            Op::Swell => outside < reach,
            Op::Tint => false,
            Op::Coat => outside < 3.0 * reach,
        }
    }

    #[inline]
    fn amount(&self) -> f64 {
        // validated present for the ops that read it
        self.amount.unwrap_or(0.0)
    }

    /// `outside-bound`: how far a point is outside the part's bounding sphere, 0 within it.
    #[inline]
    fn outside_bound(&self, x: f64, y: f64, z: f64) -> f64 {
        match self.bound {
            None => 0.0,
            Some([centre_x, centre_y, centre_z, radius]) => brood_max(
                0.0,
                length3(x - centre_x, y - centre_y, z - centre_z) - radius,
            ),
        }
    }

    /// `coat-edge`: how far outside its shape a coat still paints.
    fn coat_edge(&self) -> f64 {
        0.4 * hem_of(self.amount.unwrap_or(0.0))
    }
}

impl Scene {
    /// A leaf's own distance, its shell applied (`shape-function`).
    fn leaf_distance(&self, part: &Part, x: f64, y: f64, z: f64) -> f64 {
        let bare = match &part.shape {
            Shape::Sphere { centre, radius } => sphere(x, y, z, centre, *radius),
            Shape::Ellipsoid { centre, radii } => ellipsoid(x, y, z, centre, radii),
            Shape::Limb { from, to, radii } => limb(x, y, z, from, to, radii),
            Shape::Box {
                centre,
                half,
                rounding,
            } => rounded_box(x, y, z, centre, half, *rounding),
            Shape::Outline { first, count } => outline(x, y, &self.points[*first..*first + *count]),
            Shape::Plane { at, normal } => plane(x, y, z, at, normal),
            Shape::Grid { field } => self.fields[*field].part_distance(x, y, z),
            Shape::Group { .. } => unreachable!("a group is not a leaf"),
        };
        match part.shell {
            None => bare,
            Some(shell) => brood_abs(bare) - shell,
        }
    }

    // --- the compiled distance: `compiled-distance` / `run-tree` / `step-function` ----------------

    /// The model's distance at a point.
    pub fn distance(&self, x: f64, y: f64, z: f64) -> f64 {
        self.run(&self.tree, &self.parts, x, y, z, 1000000.0)
    }

    fn run(&self, tree: &Tree, parts: &[Part], x: f64, y: f64, z: f64, so_far: f64) -> f64 {
        match tree {
            Tree::Steps { start, end } => parts[*start..*end]
                .iter()
                .fold(so_far, |so_far, part| self.step(part, x, y, z, so_far)),
            Tree::Split {
                first,
                second,
                cull,
            } => {
                if let Some(Cull {
                    centre,
                    radius,
                    most_reach,
                    has_union,
                    has_carve,
                }) = cull
                {
                    let outside = length3(x - centre[0], y - centre[1], z - centre[2]) - radius;
                    let needed = most_reach
                        + if *has_union {
                            brood_max(so_far, 0.0)
                        } else {
                            0.0
                        }
                        + if *has_carve {
                            brood_max(0.0 - so_far, 0.0)
                        } else {
                            0.0
                        };
                    if outside >= needed {
                        return so_far;
                    }
                }
                let so_far = self.run(first, parts, x, y, z, so_far);
                self.run(second, parts, x, y, z, so_far)
            }
        }
    }

    fn step_distance(&self, part: &Part, x: f64, y: f64, z: f64) -> f64 {
        match &part.shape {
            Shape::Group { parts, tree } => self.run(tree, parts, x, y, z, 1000000.0),
            _ => self.leaf_distance(part, x, y, z),
        }
    }

    fn step(&self, part: &Part, x: f64, y: f64, z: f64, so_far: f64) -> f64 {
        match part.bound {
            None => part.join(so_far, self.step_distance(part, x, y, z)),
            Some([bound_x, bound_y, bound_z, bound_radius]) => {
                let outside = brood_max(
                    0.0,
                    length3(x - bound_x, y - bound_y, z - bound_z) - bound_radius,
                );
                if part.reaches(outside, so_far) {
                    part.join(so_far, self.step_distance(part, x, y, z))
                } else {
                    so_far
                }
            }
        }
    }

    // --- the plain walk the region and material rules use: `part-distance` / `parts-distance` ----

    pub(super) fn part_distance(&self, part: &Part, x: f64, y: f64, z: f64) -> f64 {
        match &part.shape {
            Shape::Group { parts, .. } => self.parts_distance(parts, x, y, z),
            _ => self.leaf_distance(part, x, y, z),
        }
    }

    fn parts_distance(&self, parts: &[Part], x: f64, y: f64, z: f64) -> f64 {
        parts.iter().fold(1000000.0, |so_far, part| {
            if part.reaches(part.outside_bound(x, y, z), so_far) {
                part.join(so_far, self.part_distance(part, x, y, z))
            } else {
                so_far
            }
        })
    }

    // --- the region: `region-of` -------------------------------------------------------------

    /// The region of the top-level union part a point lies deepest in (a grid's, its voxel's).
    pub fn region(&self, x: f64, y: f64, z: f64) -> Option<usize> {
        let mut found = None;
        let mut deepest = 1000000.0;
        for part in &self.parts {
            let grid = match part.shape {
                Shape::Grid { field } => Some(field),
                _ => None,
            };
            if (grid.is_none() && part.region.is_none()) || part.op != Op::Union {
                continue;
            }
            let distance = self.part_distance(part, x, y, z);
            if distance < deepest {
                found = match grid {
                    Some(field) => self.fields[field].region(x, y, z),
                    None => part.region,
                };
                deepest = distance;
            }
        }
        found
    }

    // --- the material: `material-of` / `material-step` ------------------------------------------

    /// The index of the material at a point (into the scene's materials).
    pub fn material_at(&self, x: f64, y: f64, z: f64) -> usize {
        self.material_of(
            &self.parts,
            x,
            y,
            z,
            self.fallback,
            1000000.0,
            self.fallback,
        )
        .0
    }

    #[allow(clippy::too_many_arguments)]
    fn material_of(
        &self,
        parts: &[Part],
        x: f64,
        y: f64,
        z: f64,
        found: usize,
        nearest: f64,
        inherited: usize,
    ) -> (usize, f64) {
        parts
            .iter()
            .fold((found, nearest), |(found, nearest), part| {
                self.material_step(part, x, y, z, found, nearest, inherited)
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn material_step(
        &self,
        part: &Part,
        x: f64,
        y: f64,
        z: f64,
        found: usize,
        nearest: f64,
        inherited: usize,
    ) -> (usize, f64) {
        const CLAIMED: f64 = -1000000.0;
        let own = part.material.unwrap_or(inherited);
        match (&part.shape, part.op) {
            (Shape::Group { parts, .. }, Op::Union)
                if part.outside_bound(x, y, z) < brood_max(nearest, 0.0) =>
            {
                let inside = self.material_of(parts, x, y, z, own, 1000000.0, own);
                let distance = inside.1;
                if distance <= CLAIMED || (nearest > CLAIMED && distance < nearest) {
                    inside
                } else {
                    (found, nearest)
                }
            }
            (_, Op::Union) if part.outside_bound(x, y, z) < brood_max(nearest, 0.05) => {
                let distance = self.part_distance(part, x, y, z);
                let reach = distance - part.blend.unwrap_or(0.0);
                if brood_abs(distance) < 0.05 {
                    (own, CLAIMED)
                } else if nearest > CLAIMED && reach < nearest {
                    (own, reach)
                } else {
                    (found, nearest)
                }
            }
            (_, Op::Coat)
                if part.outside_bound(x, y, z) <= part.coat_edge()
                    && self.part_distance(part, x, y, z) < part.coat_edge() =>
            {
                (own, CLAIMED)
            }
            _ => (found, nearest),
        }
    }

    // --- the colour zones: `tinted` -----------------------------------------------------------

    /// Material `material`'s albedo with the colour zones holding the point worked in.
    pub fn tinted(&self, material: usize, x: f64, y: f64, z: f64) -> [f64; 3] {
        let name = self.materials[material].name;
        let mut albedo = self.materials[material].albedo;
        for part in &self.tints {
            let Some(tint) = part.tint else { continue };
            if tint.on.is_none() || tint.on != name {
                continue;
            }
            let blend = part.blend.unwrap_or(1.0);
            let distance = self.part_distance(part, x, y, z);
            let weight = part.amount() * smoothstep01((blend - distance) / (2.0 * blend));
            if weight <= 0.0 {
                continue;
            }
            for (channel, value) in albedo.iter_mut().enumerate() {
                *value = mix(*value, tint.colour[channel], weight);
            }
        }
        albedo
    }
}
