//! A hair lock's sub-steps, natively (ADR-821): b3d-hair's `moved-with` loop over sub-steps
//! (`sub-step`: the Verlet step with its pull to the groom's shape, the roots' hold, the
//! distance-constraint passes keeping each bone off her body's shapes with friction, and the
//! ground), done for every lock in one call. The rules are b3d-hair's and stay documented there;
//! this is the same arithmetic, sum for sum in the same order, so a lock comes out as the Brood
//! version leaves it (a test holds them together). Every constant comes from the caller.

pub type Vector = [f64; 3];

/// What b3d-hair's step reads from its module's settings, passed in so they stay defined there.
#[derive(Clone, Copy, Debug)]
pub struct Constants {
    pub gravity: Vector,
    pub air_drag: f64,
    pub still: f64,
    pub carried: f64,
    pub slide: f64,
    pub rests: f64,
    pub pressed: f64,
    pub shove: f64,
    pub unfold_give: f64,
    /// how far past its bend limit (in its cosine) a joint must be before a step for that step to
    /// open it gently (`*past*`)
    pub past: f64,
    pub ground: f64,
    /// `*clearance*`'s furthest: how near a pane is looked for by default
    pub clearance: f64,
    pub passes: usize,
}

/// One of her body's shapes, as `shapes-of` lays it out: a capsule's eleven numbers
/// `[ax ay az bx by bz radius cx cy cz half]`, or a pane's seventeen (its twelfth, the `:pane`
/// mark, any number here).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape(pub Vec<f64>);

impl Shape {
    fn pane(&self) -> bool {
        self.0.len() > 11
    }
}

/// One lock as `moved-with` steps it this frame.
#[derive(Clone, Debug)]
pub struct Lock {
    pub points: Vec<Vector>,
    pub previous: Vec<Vector>,
    /// its posed points where the last call left them and where they are now (`before`, `now`)
    pub before: Vec<Vector>,
    pub now: Vec<Vector>,
    /// each bone's pre-load force as the head now turns it
    pub carries: Vec<Vector>,
    /// for each bone, the places in the frame's shapes it could touch
    pub nears: Vec<Vec<usize>>,
    pub stiffness: Vec<f64>,
    pub follows: Option<Vec<f64>>,
    pub damping: Vec<f64>,
    pub upright: f64,
    pub holds: Vec<f64>,
    pub lengths: Vec<f64>,
    pub margins: Vec<f64>,
    /// the bend limits already `relaxed` for how upright the head is
    pub limits: Vec<f64>,
    pub thickness: Option<Vec<f64>>,
}

fn add(a: Vector, b: Vector) -> Vector {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn subtract(a: Vector, b: Vector) -> Vector {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: Vector, factor: f64) -> Vector {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

fn size(a: Vector) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn dot(a: Vector, b: Vector) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: Vector) -> Vector {
    let length = size(a);
    if length < 1.0e-12 {
        [0.0, 0.0, 0.0]
    } else {
        let factor = 1.0 / length;
        [a[0] * factor, a[1] * factor, a[2] * factor]
    }
}

fn mix(a: Vector, b: Vector, amount: f64) -> Vector {
    [
        a[0] + (b[0] - a[0]) * amount,
        a[1] + (b[1] - a[1]) * amount,
        a[2] + (b[2] - a[2]) * amount,
    ]
}

fn unit(value: f64) -> f64 {
    1.0f64.min(0.0f64.max(value))
}

/// `turn-between`: the shortest turn taking `from` to `to`, a unit quaternion
fn turn_between(from: Vector, to: Vector) -> [f64; 4] {
    let [ax, ay, az] = from;
    let [bx, by, bz] = to;
    let cx = ay * bz - az * by;
    let cy = az * bx - ax * bz;
    let cz = ax * by - ay * bx;
    let lengths = ((ax * ax + ay * ay + az * az) * (bx * bx + by * by + bz * bz)).sqrt();
    let w = lengths + ax * bx + ay * by + az * bz;
    let magnitude = (cx * cx + cy * cy + cz * cz + w * w).sqrt();
    if magnitude <= 1.0e-9 * lengths {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [
            cx / magnitude,
            cy / magnitude,
            cz / magnitude,
            w / magnitude,
        ]
    }
}

/// `b3d-math/quat-rotate`
fn rotate(rotation: [f64; 4], point: Vector) -> Vector {
    let [x, y, z, w] = rotation;
    let [px, py, pz] = point;
    let tx = (y * pz - z * py) * 2.0;
    let ty = (z * px - x * pz) * 2.0;
    let tz = (x * py - y * px) * 2.0;
    [
        (px + tx * w) + (y * tz - z * ty),
        (py + ty * w) + (z * tx - x * tz),
        (pz + tz * w) + (x * ty - y * tx),
    ]
}

/// `shaped`: where the rest shape puts point `index`
fn shaped(posed: &[Vector], points: &[Vector], index: usize) -> Vector {
    let segment = subtract(posed[index], posed[index - 1]);
    if index < 2 {
        add(points[index - 1], segment)
    } else {
        let swing = turn_between(
            subtract(posed[index - 1], posed[index - 2]),
            subtract(points[index - 1], points[index - 2]),
        );
        add(points[index - 1], rotate(swing, segment))
    }
}

/// `at-length`
fn at_length(before: Vector, point: Vector, length: f64) -> Vector {
    let [bx, by, bz] = before;
    let dx = point[0] - bx;
    let dy = point[1] - by;
    let dz = point[2] - bz;
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    if distance < 1.0e-9 {
        [bx, by - length, bz]
    } else {
        let stretch = length / distance;
        [bx + dx * stretch, by + dy * stretch, bz + dz * stretch]
    }
}

/// `gripped`
fn gripped(kept: Vector, placed: Vector, old: Vector, constants: &Constants) -> Vector {
    let [kx, ky, kz] = kept;
    let nx = kx - placed[0];
    let ny = ky - placed[1];
    let nz = kz - placed[2];
    let push = (nx * nx + ny * ny + nz * nz).sqrt();
    if push < 1.0e-12 {
        return kept;
    }
    let nx = nx / push;
    let ny = ny / push;
    let nz = nz / push;
    let mx = kx - old[0];
    let my = ky - old[1];
    let mz = kz - old[2];
    let along = mx * nx + my * ny + mz * nz;
    let tx = mx - along * nx;
    let ty = my - along * ny;
    let tz = mz - along * nz;
    let slip = (tx * tx + ty * ty + tz * tz).sqrt();
    let held = if slip <= constants.slide * push {
        1.0
    } else {
        (constants.slide * push) / slip
    };
    [kx - held * tx, ky - held * ty, kz - held * tz]
}

/// `unfolded-fully`
fn unfolded_fully(above: Vector, p: Vector, q: Vector, limit: f64) -> Vector {
    let [ax, ay, az] = normalize(subtract(p, above));
    let [px, py, pz] = p;
    let dx = q[0] - px;
    let dy = q[1] - py;
    let dz = q[2] - pz;
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    let along = if length < 1.0e-12 {
        1.0
    } else {
        (ax * dx + ay * dy + az * dz) / length
    };
    if along >= limit {
        return q;
    }
    let cx = dx - along * length * ax;
    let cy = dy - along * length * ay;
    let cz = dz - along * length * az;
    let across = (cx * cx + cy * cy + cz * cz).sqrt();
    let sine = 0.0f64.max(1.0 - limit * limit).sqrt();
    if across < 1.0e-12 {
        [px + length * ax, py + length * ay, pz + length * az]
    } else {
        [
            px + length * (limit * ax + sine * (cx / across)),
            py + length * (limit * ay + sine * (cy / across)),
            pz + length * (limit * az + sine * (cz / across)),
        ]
    }
}

/// `bent-past?`: whether the joint at `p` bends further than `limit` (a cosine)
fn bent_past(above: Vector, p: Vector, q: Vector, limit: f64) -> bool {
    let ax = p[0] - above[0];
    let ay = p[1] - above[1];
    let az = p[2] - above[2];
    let dx = q[0] - p[0];
    let dy = q[1] - p[1];
    let dz = q[2] - p[2];
    let lengths = (ax * ax + ay * ay + az * az).sqrt() * (dx * dx + dy * dy + dz * dz).sqrt();
    lengths > 1.0e-18 && (ax * dx + ay * dy + az * dz) / lengths < limit
}

/// `unfolded`: at once, or `*unfold-give*` a step when `eased`
fn unfolded(
    above: Vector,
    p: Vector,
    q: Vector,
    limit: f64,
    eased: bool,
    constants: &Constants,
) -> Vector {
    let wanted = unfolded_fully(above, p, q, limit);
    let [qx, qy, qz] = q;
    let wx = wanted[0] - qx;
    let wy = wanted[1] - qy;
    let wz = wanted[2] - qz;
    let far = (wx * wx + wy * wy + wz * wz).sqrt();
    if !eased || far <= constants.unfold_give {
        wanted
    } else {
        let factor = constants.unfold_give / far;
        [qx + wx * factor, qy + wy * factor, qz + wz * factor]
    }
}

/// `unkicked`: `olds` with point `index`'s old place moved so a push gives it no speed
fn unkicked(
    olds: &mut [Vector],
    rides: &[Vector],
    index: usize,
    placed: Vector,
    end: Vector,
    kept: Vector,
    constants: &Constants,
) {
    let push = subtract(kept, placed);
    let far = size(push);
    if far < 1.0e-9 {
        return;
    }
    let normal = scale(push, 1.0 / far);
    let speed = subtract(placed, olds[index]);
    let ride = rides[index];
    let approach = dot(subtract(speed, ride), normal);
    let slid = if approach < 0.0 {
        subtract(speed, scale(normal, approach))
    } else {
        speed
    };
    let against = subtract(slid, ride);
    let going = size(against);
    let after = add(ride, scale(against, going / (going + constants.rests)));
    olds[index] = subtract(end, after);
}

/// `full-contact`: the closest points of the segment p + s u and the axis a + t v
#[allow(clippy::too_many_arguments)]
fn full_contact(p: Vector, u: Vector, uu: f64, a: Vector, v: Vector, vv: f64) -> [f64; 5] {
    let rx = p[0] - a[0];
    let ry = p[1] - a[1];
    let rz = p[2] - a[2];
    let tiny = 1.0e-12;
    let vr = v[0] * rx + v[1] * ry + v[2] * rz;
    let ur = u[0] * rx + u[1] * ry + u[2] * rz;
    let uv = u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let (s, t) = if uu <= tiny && vv <= tiny {
        (0.0, 0.0)
    } else if uu <= tiny {
        (0.0, unit(vr / vv))
    } else if vv <= tiny {
        (unit(-ur / uu), 0.0)
    } else {
        let denominator = uu * vv - uv * uv;
        let s = if denominator > tiny {
            unit((uv * vr - ur * vv) / denominator)
        } else {
            0.0
        };
        let t = (uv * s + vr) / vv;
        if t < 0.0 {
            (unit(-ur / uu), 0.0)
        } else if t > 1.0 {
            (unit((uv - ur) / uu), 1.0)
        } else {
            (s, t)
        }
    };
    let dx = (p[0] + s * u[0]) - (a[0] + t * v[0]);
    let dy = (p[1] + s * u[1]) - (a[1] + t * v[1]);
    let dz = (p[2] + s * u[2]) - (a[2] + t * v[2]);
    [s, dx, dy, dz, (dx * dx + dy * dy + dz * dz).sqrt()]
}

/// `capsule-contact`
fn capsule_contact(p: Vector, q: Vector, shape: &[f64], margin: f64) -> Option<[f64; 5]> {
    let [px, py, pz] = p;
    let (ax, ay, az, bx, by, bz) = (shape[0], shape[1], shape[2], shape[3], shape[4], shape[5]);
    let (radius, cx, cy, cz, half) = (shape[6], shape[7], shape[8], shape[9], shape[10]);
    let ux = q[0] - px;
    let uy = q[1] - py;
    let uz = q[2] - pz;
    let mx = (px + 0.5 * ux) - cx;
    let my = (py + 0.5 * uy) - cy;
    let mz = (pz + 0.5 * uz) - cz;
    let uu = ux * ux + uy * uy + uz * uz;
    let reach = 0.5 * uu.sqrt() + half + radius + margin;
    if mx * mx + my * my + mz * mz > reach * reach {
        return None;
    }
    let vx = bx - ax;
    let vy = by - ay;
    let vz = bz - az;
    let vv = vx * vx + vy * vy + vz * vz;
    let hx = (px + 0.5 * ux) - ax;
    let hy = (py + 0.5 * uy) - ay;
    let hz = (pz + 0.5 * uz) - az;
    let along = if vv > 1.0e-12 {
        (hx * vx + hy * vy + hz * vz) / vv
    } else {
        0.0
    };
    let along = along.clamp(0.0, 1.0);
    let ox = hx - along * vx;
    let oy = hy - along * vy;
    let oz = hz - along * vz;
    let near = radius + 0.5 * uu.sqrt() + margin;
    if ox * ox + oy * oy + oz * oz > near * near {
        return None;
    }
    Some(full_contact(
        p,
        [ux, uy, uz],
        uu,
        [ax, ay, az],
        [vx, vy, vz],
        vv,
    ))
}

/// `pane-contact`: `distance` signed, negative past the face
fn pane_contact(p: Vector, q: Vector, shape: &[f64], margin: f64) -> Option<[f64; 5]> {
    let [px, py, pz] = p;
    let [qx, qy, qz] = q;
    let (ox, oy, oz, nx, ny, nz) = (shape[0], shape[1], shape[2], shape[3], shape[4], shape[5]);
    let (ux, uy, uz, hw, hh) = (shape[12], shape[13], shape[14], shape[15], shape[16]);
    let dp = (px - ox) * nx + (py - oy) * ny + (pz - oz) * nz;
    let dq = (qx - ox) * nx + (qy - oy) * ny + (qz - oz) * nz;
    let s = if dp <= dq { 0.0 } else { 1.0 };
    let distance = dp.min(dq);
    let x = px + s * (qx - px);
    let y = py + s * (qy - py);
    let z = pz + s * (qz - pz);
    let rx = x - ox;
    let ry = y - oy;
    let rz = z - oz;
    let vertical = rx * ux + ry * uy + rz * uz;
    let ax = ny * uz - nz * uy;
    let ay = nz * ux - nx * uz;
    let az = nx * uy - ny * ux;
    let across = rx * ax + ry * ay + rz * az;
    if distance > margin || vertical.abs() > hh || across.abs() > hw || distance < -0.25 {
        None
    } else {
        Some([s, nx * distance, ny * distance, nz * distance, distance])
    }
}

fn contact(p: Vector, q: Vector, shape: &Shape, margin: f64) -> Option<[f64; 5]> {
    if shape.pane() {
        pane_contact(p, q, &shape.0, margin)
    } else {
        capsule_contact(p, q, &shape.0, margin)
    }
}

/// `kept-off`: the bone p → q moved so the whole of it keeps its margins off every shape
#[allow(clippy::too_many_arguments)]
fn kept_off(
    p: Vector,
    q: Vector,
    shapes: &[&Shape],
    from_margin: f64,
    to_margin: f64,
    pinned: bool,
    thickness: Option<f64>,
    constants: &Constants,
) -> (Vector, Vector) {
    let thick = constants.pressed * thickness.unwrap_or(0.0);
    let reach = 1.0e-4 + from_margin.max(to_margin).max(thick);
    let (mut p, mut q) = (p, q);
    for shape in shapes {
        let [px, py, pz] = p;
        let [qx, qy, qz] = q;
        let found = if pinned {
            contact(
                [0.5 * (px + qx), 0.5 * (py + qy), 0.5 * (pz + qz)],
                q,
                shape,
                reach,
            )
            .map(|mut found| {
                found[0] = 0.5 + 0.5 * found[0];
                found
            })
        } else {
            contact(p, q, shape, reach)
        };
        let Some([s, dx, dy, dz, distance]) = found else {
            continue;
        };
        let need = if shape.pane() {
            (0.0 - shape.0[6]).max(thick)
        } else {
            shape.0[6] + from_margin + s * (to_margin - from_margin)
        };
        if distance >= need {
            continue;
        }
        let [nx, ny, nz] = if shape.pane() {
            [shape.0[3], shape.0[4], shape.0[5]]
        } else if distance > 1.0e-9 {
            [dx / distance, dy / distance, dz / distance]
        } else {
            [0.0, 1.0, 0.0]
        };
        let depth = constants.shove.min(need - distance);
        let share = 1.0 / ((1.0 - s) * (1.0 - s) + s * s);
        let at_p = if pinned {
            0.0
        } else {
            depth * (1.0 - s) * share
        };
        let at_q = if pinned {
            depth / s.max(0.25)
        } else {
            depth * s * share
        };
        p = [px + nx * at_p, py + ny * at_p, pz + nz * at_p];
        q = [qx + nx * at_q, qy + ny * at_q, qz + nz * at_q];
    }
    (p, q)
}

/// `constrained-with`: one pass, root to tip (`rides` None: `olds` left as they are, as
/// `constrained` leaves them)
#[allow(clippy::too_many_arguments)]
fn constrained_with(
    points: &mut [Vector],
    olds: &mut [Vector],
    lock: &Lock,
    nears: &[Vec<&Shape>],
    rides: Option<&[Vector]>,
    constants: &Constants,
) {
    let lengths = &lock.lengths;
    let margins = &lock.margins;
    let limits = &lock.limits;
    for index in 1..points.len() {
        let before = points[index - 1];
        let length = lengths[index - 1];
        let placed = at_length(before, points[index], length);
        // (eased only where the joint was past its limit before the step by more than `*past*`)
        let eased = index >= 2
            && bent_past(
                olds[index - 2],
                olds[index - 1],
                olds[index],
                limits[index] - constants.past,
            );
        let placed = if index < 2 {
            placed
        } else {
            unfolded(
                points[index - 2],
                before,
                placed,
                limits[index],
                eased,
                constants,
            )
        };
        let thickness = lock
            .thickness
            .as_ref()
            .map(|thicknesses| thicknesses[index - 1].max(thicknesses[index]));
        let (base, kept) = kept_off(
            before,
            placed,
            &nears[index - 1],
            margins[index - 1],
            margins[index],
            index == 1,
            thickness,
            constants,
        );
        let base = if index < 2 || base == before {
            base
        } else {
            at_length(points[index - 2], base, lengths[index - 2])
        };
        if base != before {
            points[index - 1] = base;
        }
        if kept == placed && base == before {
            points[index] = placed;
            continue;
        }
        let pushed = at_length(base, gripped(kept, placed, olds[index], constants), length);
        let end = if index < 2 {
            pushed
        } else {
            let opened = unfolded(
                points[index - 2],
                base,
                pushed,
                limits[index],
                eased,
                constants,
            );
            if opened == pushed {
                pushed
            } else {
                let (_, again) = kept_off(
                    base,
                    opened,
                    &nears[index - 1],
                    margins[index - 1],
                    margins[index],
                    true,
                    thickness,
                    constants,
                );
                at_length(base, again, length)
            }
        };
        points[index] = end;
        if let Some(rides) = rides {
            unkicked(olds, rides, index, placed, end, kept, constants);
        }
    }
}

/// `sub-step`: `[points previous]` a step on
#[allow(clippy::too_many_arguments)]
fn sub_step(
    lock: &Lock,
    posed: &[Vector],
    points: &[Vector],
    previous: &[Vector],
    step: f64,
    nears: &[Vec<&Shape>],
    rides: &[Vector],
    constants: &Constants,
) -> (Vec<Vector>, Vec<Vector>) {
    let upright = lock.upright;
    let air = (-(step * constants.air_drag)).exp();
    let [gx, gy, gz] = constants.gravity;
    let squared = step * step;
    let mut stepped = Vec::with_capacity(points.len());
    stepped.push(posed[0]);
    for index in 1..points.len() {
        let [nx, ny, nz] = points[index];
        let [ox, oy, oz] = previous[index];
        let lead = if index == 1 {
            1.0 - constants.carried
        } else {
            1.0
        };
        let [ax, ay, az] = if index == 1 {
            posed[0]
        } else {
            points[index - 1]
        };
        let [bx, by, bz] = if index == 1 {
            points[0]
        } else {
            previous[index - 1]
        };
        let vx = nx - ox;
        let vy = ny - oy;
        let vz = nz - oz;
        let kept = 1.0f64.min(step * lock.damping[index - 1]);
        let rx = air * (vx - kept * (vx - lead * (ax - bx)));
        let ry = air * (vy - kept * (vy - lead * (ay - by)));
        let rz = air * (vz - kept * (vz - lead * (az - bz)));
        let moving = (rx * rx + ry * ry + rz * rz).sqrt();
        let freed = moving / (moving + constants.still);
        let dx = freed * rx;
        let dy = freed * ry;
        let dz = freed * rz;
        let share = lock.follows.as_ref().map_or(0.0, |follows| follows[index]);
        let [tx, ty, tz] = if share < 1.0e-6 {
            shaped(posed, points, index)
        } else {
            mix(shaped(posed, points, index), posed[index], share)
        };
        let stiffness = (0.25 + 0.75 * upright) * lock.stiffness[index - 1];
        let [cx, cy, cz] = lock.carries[index - 1];
        let fx = gx + cx;
        let fy = gy + cy;
        let fz = gz + cz;
        let pull = (squared * stiffness) / (1.0 + squared * stiffness);
        stepped.push([
            nx + dx + squared * fx + pull * (tx - nx),
            ny + dy + squared * fy + pull * (ty - ny),
            nz + dz + squared * fz + pull * (tz - nz),
        ]);
    }
    for index in 1..lock.holds.len().min(stepped.len()) {
        let share = upright * upright * lock.holds[index];
        if share > 0.0 {
            stepped[index] = mix(stepped[index], posed[index], share);
        }
    }
    let mut olds = points.to_vec();
    for _ in 0..constants.passes {
        constrained_with(&mut stepped, &mut olds, lock, nears, Some(rides), constants);
    }
    // never under the ground: a point that would be is put on it, its old place raised as far
    let mut grounded = stepped.clone();
    for index in 1..grounded.len() {
        if stepped[index][1] < constants.ground {
            grounded[index][1] = constants.ground;
            olds[index][1] = olds[index][1].max(constants.ground);
        }
    }
    olds[0] = posed[0];
    (grounded, olds)
}

/// `slid`
fn slid(before: &[Vector], now: &[Vector], amount: f64) -> Vec<Vector> {
    (0..now.len())
        .map(|index| {
            let [ax, ay, az] = before[index];
            let [bx, by, bz] = now[index];
            [
                ax + amount * (bx - ax),
                ay + amount * (by - ay),
                az + amount * (bz - az),
            ]
        })
        .collect()
}

/// `carried`
fn carried(points: &[Vector], from: &[Vector], to: &[Vector], share: f64) -> Vec<Vector> {
    (0..points.len())
        .map(|index| {
            if index == 0 {
                points[0]
            } else {
                let point = points[index];
                [
                    point[0] + (to[index][0] - from[index][0]) * share,
                    point[1] + (to[index][1] - from[index][1]) * share,
                    point[2] + (to[index][2] - from[index][2]) * share,
                ]
            }
        })
        .collect()
}

/// `shapes-between`, number by number
fn shapes_between(before: &[Shape], now: &[Shape], share: f64) -> Vec<Shape> {
    now.iter()
        .zip(before)
        .map(|(to, from)| {
            Shape(
                to.0.iter()
                    .zip(&from.0)
                    .map(|(&b, &a)| a + share * (b - a))
                    .collect(),
            )
        })
        .collect()
}

/// `moved-with`'s sub-steps for one lock: `(points previous)` `steps` sub-steps of `step`
/// seconds on, its posed points slid from `before` to `now` across them, her shapes from
/// `shapes_before` (when given, of the same count, and more than one step) to `shapes`
pub fn moved(
    lock: &Lock,
    steps: usize,
    step: f64,
    shapes_before: Option<&[Shape]>,
    shapes: &[Shape],
    constants: &Constants,
) -> (Vec<Vector>, Vec<Vector>) {
    let blended = shapes_before.filter(|before| steps > 1 && before.len() == shapes.len());
    let mut points = lock.points.clone();
    let mut previous = lock.previous.clone();
    for taken in 1..=steps {
        let from = slid(&lock.before, &lock.now, (taken - 1) as f64 / steps as f64);
        let to = slid(&lock.before, &lock.now, taken as f64 / steps as f64);
        let between;
        let frame: &[Shape] = match blended {
            Some(before) => {
                between = shapes_between(before, shapes, taken as f64 / steps as f64);
                &between
            }
            None => shapes,
        };
        let nears: Vec<Vec<&Shape>> = lock
            .nears
            .iter()
            .map(|places| places.iter().map(|&place| &frame[place]).collect())
            .collect();
        let rides: Vec<Vector> = (0..points.len())
            .map(|index| scale(subtract(to[index], from[index]), 1.0 - constants.carried))
            .collect();
        let (next, last) = sub_step(
            lock,
            &to,
            &carried(&points, &from, &to, constants.carried),
            &carried(&previous, &from, &to, constants.carried),
            step,
            &nears,
            &rides,
            constants,
        );
        points = next;
        previous = last;
    }
    (points, previous)
}

// ---- a lock planned natively (ADR-821 amended): b3d-hair's `moved-plan`, sum for sum, so the
// frame's whole step is one call (the plan in Brood was most of the hair's cost a frame) ----

/// A column-major 4×4 matrix, as b3d-math keeps one.
pub type Matrix = [f64; 16];

/// What b3d-hair's plan reads from its module's settings (`*rate*`, `*most-steps*`, `*drift*`,
/// `*clearance*`'s furthest).
#[derive(Clone, Copy, Debug)]
pub struct PlanConstants {
    pub rate: f64,
    pub most_steps: usize,
    pub drift: f64,
    pub clearance: f64,
    /// her curls' stretch, `bounce-tuning`'s `[gain hertz damping]` (none without it), and how far
    /// it may go (`*stretch*`) and how hard a push counts (`*bounce-most*`)
    pub bounce: Option<[f64; 3]>,
    pub stretch: f64,
    pub bounce_most: f64,
}

/// A lock's curls' stretch (`:bounce`): how far they are stretched, how fast, and the root's
/// velocity the last step (none on the first).
pub type Bounce = (f64, f64, Option<Vector>);

/// One lock as the state keeps it between frames: what `moved-plan` reads.
#[derive(Clone, Debug)]
pub struct Held {
    pub points: Vec<Vector>,
    pub previous: Vec<Vector>,
    /// the last frame's sub-step length (`:step`), none on the first
    pub last_step: Option<f64>,
    /// its posed points where the last call left them (`:posed`), none on the first
    pub posed: Option<Vec<Vector>>,
    pub preload: Vec<Vector>,
    pub up: Vector,
    pub nears: Vec<Vec<usize>>,
    /// where its points were when its near shapes were found (`:nears-at`)
    pub nears_at: Option<Vec<Vector>>,
    pub joints: Vec<usize>,
    /// its root joint's parent, none when it hangs from the skeleton's base
    pub root_parent: Option<usize>,
    pub stiffness: Vec<f64>,
    pub follows: Option<Vec<f64>>,
    pub damping: Vec<f64>,
    pub holds: Vec<f64>,
    pub lengths: Vec<f64>,
    pub margins: Vec<f64>,
    /// its bend limits as groomed (`relaxed` here for how upright the head is)
    pub limits: Vec<f64>,
    pub thickness: Option<Vec<f64>>,
    /// its curls' stretch the last frame (`:bounce`)
    pub bounce: Option<Bounce>,
}

/// The frame every lock is planned in.
pub struct Frame<'a> {
    pub worlds: &'a [Matrix],
    pub base: Matrix,
    /// the head's turn about her spine (`head-twist`): its angle and the spine's up
    pub twist: Option<(f64, Vector)>,
    pub dt: f64,
    pub moved_shapes: bool,
    pub shapes: &'a [Shape],
}

/// A lock planned: the lock its sub-steps take, and what `moved-done` keeps.
pub struct Planned {
    pub lock: Lock,
    pub steps: usize,
    pub step: f64,
    pub fresh: bool,
    /// its curls' stretch this frame (none without `bounce-tuning`, or with no time passed)
    pub bounce: Option<Bounce>,
}

/// `bounced`: the lock's curls' stretch `dt` seconds on, its root gone from `from` to `to`; the
/// spring each lock's own by its first joint (b3d-hair's spreads), in sub-steps of 1/rate s
fn bounced(
    held: &Held,
    upright: f64,
    tuning: [f64; 3],
    from: Vector,
    to: Vector,
    dt: f64,
    constants: &PlanConstants,
) -> Bounce {
    let [gain, hertz, damping] = tuning;
    let (mut b, mut speed, before) = held.bounce.unwrap_or((0.0, 0.0, None));
    let velocity = scale(subtract(to, from), 1.0 / dt.max(1.0e-6));
    let mut asked = match before {
        Some(before) => scale(subtract(velocity, before), 1.0 / dt.max(1.0e-6)),
        None => [0.0, 0.0, 0.0],
    };
    let strength = size(asked);
    if strength > constants.bounce_most {
        asked = scale(asked, constants.bounce_most / strength);
    }
    let along = normalize(subtract(
        *held.points.last().expect("a lock has points"),
        held.points[0],
    ));
    let first_joint = held.joints.first().copied().unwrap_or(0) as f64;
    let share = |factor: f64| {
        let turn = factor * first_joint;
        turn - turn.floor()
    };
    let omega = 2.0 * std::f64::consts::PI * hertz * (0.8 + 0.4 * share(0.618));
    let gain = gain * (0.6 + 0.8 * share(0.381));
    let damping = damping * (0.7 + 0.6 * share(0.755));
    let drive = -(gain * upright * dot(asked, along));
    let steps = (dt * constants.rate + 0.999).floor().max(1.0) as usize;
    let step = dt / steps as f64;
    for _ in 0..steps {
        speed += step * (drive - omega * omega * b - 2.0 * damping * omega * speed);
        b = (-constants.stretch).max(constants.stretch.min(b + step * speed));
    }
    (b, speed, Some(velocity))
}

fn translation(matrix: &Matrix) -> Vector {
    [matrix[12], matrix[13], matrix[14]]
}

/// `b3d-math/mat4-rotation` as the turn's three columns: each axis made a unit, and squared by two
/// of Björck's steps unless already square (to 1e-9) or one is scaled to nothing
fn rotation_columns(matrix: &Matrix) -> [Vector; 3] {
    let unit_of = |axis: Vector| {
        let length = size(axis);
        let factor = if length < 1.0e-12 { 0.0 } else { 1.0 / length };
        (scale(axis, factor), factor)
    };
    let (a, fa) = unit_of([matrix[0], matrix[1], matrix[2]]);
    let (b, fb) = unit_of([matrix[4], matrix[5], matrix[6]]);
    let (c, fc) = unit_of([matrix[8], matrix[9], matrix[10]]);
    if dot(a, b).abs() + dot(a, c).abs() + dot(b, c).abs() < 1.0e-9
        || fa == 0.0
        || fb == 0.0
        || fc == 0.0
    {
        return [a, b, c];
    }
    let squared = |[a, b, c]: [Vector; 3]| {
        let (sa, sb, sc) = (
            1.5 - 0.5 * dot(a, a),
            1.5 - 0.5 * dot(b, b),
            1.5 - 0.5 * dot(c, c),
        );
        let (ab, ac, bc) = (0.5 * dot(a, b), 0.5 * dot(a, c), 0.5 * dot(b, c));
        [
            subtract(subtract(scale(a, sa), scale(b, ab)), scale(c, ac)),
            subtract(subtract(scale(b, sb), scale(a, ab)), scale(c, bc)),
            subtract(subtract(scale(c, sc), scale(a, ac)), scale(b, bc)),
        ]
    };
    squared(squared([a, b, c]))
}

/// `points-of`: each joint's head, then the tip a last length along the last bone
fn points_of(worlds: &[Matrix], joints: &[usize], lengths: &[f64]) -> Vec<Vector> {
    let mut points: Vec<Vector> = joints
        .iter()
        .map(|&joint| translation(&worlds[joint]))
        .collect();
    if let (Some(&last), Some(&length)) = (joints.last(), lengths.last()) {
        let world = &worlds[last];
        let along = normalize([world[4], world[5], world[6]]);
        let head = *points.last().expect("a lock has joints");
        points.push(add(head, scale(along, length)));
    }
    points
}

/// `spins-of`: each point's turn back about the spine, `[cos sin]`, none below a share of 1e-6
fn spins_of(
    follows: Option<&[f64]>,
    twist: Option<(f64, Vector)>,
) -> Option<Vec<Option<[f64; 2]>>> {
    let (angle, _) = twist?;
    if angle.abs() < 1.0e-5 {
        return None;
    }
    Some(
        follows?
            .iter()
            .map(|&share| {
                (share >= 1.0e-6).then(|| {
                    let turn = -share * angle;
                    [turn.cos(), turn.sin()]
                })
            })
            .collect(),
    )
}

/// `spun`: Rodrigues' formula written out, as b3d-hair writes it
fn spun(vector: Vector, axis: Vector, [c, s]: [f64; 2]) -> Vector {
    let [vx, vy, vz] = vector;
    let [kx, ky, kz] = axis;
    let along = (kx * vx + ky * vy + kz * vz) * (1.0 - c);
    [
        vx * c + s * (ky * vz - kz * vy) + kx * along,
        vy * c + s * (kz * vx - kx * vz) + ky * along,
        vz * c + s * (kx * vy - ky * vx) + kz * along,
    ]
}

/// `within-drift?`
fn within_drift(points: &[Vector], before: &[Vector], drift: f64) -> bool {
    let limit = drift * drift;
    points.len() <= before.len()
        && points.iter().zip(before).all(|(&[x, y, z], &[a, b, c])| {
            (x - a) * (x - a) + (y - b) * (y - b) + (z - c) * (z - c) <= limit
        })
}

/// `within?`: whether a shape comes within `reach` of a point
fn shape_within(shape: &Shape, [x, y, z]: Vector, reach: f64) -> bool {
    let numbers = &shape.0;
    let (dx, dy, dz) = (numbers[7] - x, numbers[8] - y, numbers[9] - z);
    let apart = reach + numbers[10] + numbers[6];
    dx * dx + dy * dy + dz * dz <= apart * apart
}

/// `near-shapes`: for each bone, the places of the shapes it could touch this frame
fn near_shapes(points: &[Vector], shapes: &[Shape], constants: &PlanConstants) -> Vec<Vec<usize>> {
    let count = points.len() as f64;
    let sum = points
        .iter()
        .fold([0.0, 0.0, 0.0], |[sx, sy, sz], &[x, y, z]| {
            [sx + x, sy + y, sz + z]
        });
    let centre = [sum[0] / count, sum[1] / count, sum[2] / count];
    let spread = points.iter().fold(0.0f64, |far, &[x, y, z]| {
        far.max(
            (x - centre[0]) * (x - centre[0])
                + (y - centre[1]) * (y - centre[1])
                + (z - centre[2]) * (z - centre[2]),
        )
    });
    let slack = 0.01 + 2.0 * constants.drift + constants.clearance;
    let around: Vec<usize> = (0..shapes.len())
        .filter(|&at| shape_within(&shapes[at], centre, slack + spread.sqrt()))
        .collect();
    (1..points.len())
        .map(|index| {
            let (p, q) = (points[index - 1], points[index]);
            let u = subtract(q, p);
            let reach = slack + 0.5 * (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
            let middle = [
                0.5 * (p[0] + q[0]),
                0.5 * (p[1] + q[1]),
                0.5 * (p[2] + q[2]),
            ];
            around
                .iter()
                .copied()
                .filter(|&at| shape_within(&shapes[at], middle, reach))
                .collect()
        })
        .collect()
}

/// `relaxed`: the bend limits for a head `upright` (1 … 0)
fn relaxed(limits: &[f64], upright: f64) -> Vec<f64> {
    if upright >= 0.999 {
        return limits.to_vec();
    }
    limits
        .iter()
        .map(|&limit| {
            if limit <= -1.0 {
                limit
            } else {
                (limit.clamp(-1.0, 1.0).acos() + (1.0 - upright) * 2.6)
                    .min(3.1)
                    .cos()
            }
        })
        .collect()
}

/// `moved-plan` without its trial bounce (`bounce-tuning`, nil unless asked: b3d-hair plans in Brood
/// then): what the lock's sub-steps take, from the lock as the state keeps it
pub fn plan(held: &Held, frame: &Frame, constants: &PlanConstants) -> Planned {
    let above = match held.root_parent {
        Some(parent) => frame.worlds[parent],
        None => frame.base,
    };
    let [a, b, c] = rotation_columns(&above);
    // the turn applied to a vector, and its inverse to the world's up (b3d-math's `quat-rotate`)
    let turned = |v: Vector| add(add(scale(a, v[0]), scale(b, v[1])), scale(c, v[2]));
    let upright = dot(held.up, [a[1], b[1], c[1]]).max(0.0);
    let spins = spins_of(held.follows.as_deref(), frame.twist);
    let axis = frame.twist.map(|(_, axis)| axis).unwrap_or([0.0, 1.0, 0.0]);
    let posed = points_of(frame.worlds, &held.joints, &held.lengths);
    let pivot = translation(&above);
    let now: Vec<Vector> = match &spins {
        None => posed,
        Some(spins) => (0..posed.len())
            .map(|index| match spins.get(index).copied().flatten() {
                None => posed[index],
                Some(spin) => add(pivot, spun(subtract(posed[index], pivot), axis, spin)),
            })
            .collect(),
    };
    let before = held.posed.clone().unwrap_or_else(|| now.clone());
    // her curls' stretch, and the bones' lengths as stretched (`lengths-now`)
    let bounce = constants.bounce.filter(|_| frame.dt > 0.0).map(|tuning| {
        bounced(
            held, upright, tuning, before[0], now[0], frame.dt, constants,
        )
    });
    let lengths = match bounce {
        Some((b, _, _)) if 1.0 + b != 1.0 => held
            .lengths
            .iter()
            .map(|length| (1.0 + b) * length)
            .collect(),
        _ => held.lengths.clone(),
    };
    let steps = (constants.most_steps as f64)
        .min((frame.dt * constants.rate + 0.999).floor().max(1.0)) as usize;
    let step = frame.dt / steps as f64;
    let stretch = step / held.last_step.unwrap_or(step);
    let previous = slid(&held.points, &held.previous, stretch);
    let carries = (0..held.preload.len())
        .map(|index| {
            let force = turned(held.preload[index]);
            match spins
                .as_ref()
                .and_then(|spins| spins.get(index + 1).copied().flatten())
            {
                None => force,
                Some(spin) => spun(force, axis, spin),
            }
        })
        .collect();
    let fresh = !frame.moved_shapes
        && held
            .nears_at
            .as_ref()
            .is_some_and(|at| within_drift(&held.points, at, constants.drift));
    let nears = if fresh {
        held.nears.clone()
    } else {
        near_shapes(&held.points, frame.shapes, constants)
    };
    Planned {
        lock: Lock {
            points: held.points.clone(),
            previous,
            before,
            now,
            carries,
            nears,
            stiffness: held.stiffness.clone(),
            follows: held.follows.clone(),
            damping: held.damping.clone(),
            upright,
            holds: held.holds.clone(),
            lengths,
            margins: held.margins.clone(),
            limits: relaxed(&held.limits, upright),
            thickness: held.thickness.clone(),
        },
        steps,
        step,
        fresh,
        bounce,
    }
}

// ---- a lock's joints turned along its points natively (ADR-821 amended, stage 2): b3d-hair's
// `turned-lock`, sum for sum ----

/// A joint's local transform, as b3d-skeleton's pose keeps it.
#[derive(Clone, Copy, Debug)]
pub struct Local {
    pub rotation: [f64; 4],
    pub translation: Vector,
    pub scale: Vector,
}

/// `b3d-math/quat-multiply`
fn quat_multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz + (0.0 - az * by),
        aw * by + (0.0 - ax * bz) + ay * bw + az * bx,
        aw * bz + ax * by + (0.0 - ay * bx) + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `b3d-math/quat-normalize`
fn quat_normalize([x, y, z, w]: [f64; 4]) -> [f64; 4] {
    let size = (x * x + y * y + z * z + w * w).sqrt();
    if size < 1.0e-12 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [x / size, y / size, z / size, w / size]
    }
}

/// b3d-math's `rotation-of`: Shepperd's method over three unit columns
fn rotation_of([[m00, m10, m20], [m01, m11, m21], [m02, m12, m22]]: [Vector; 3]) -> [f64; 4] {
    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = 2.0 * (trace + 1.0).sqrt();
        quat_normalize([(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s])
    } else if m00 > m11 && m00 > m22 {
        let s = 2.0 * (1.0 + m00 + (0.0 - m11) + (0.0 - m22)).sqrt();
        quat_normalize([0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s])
    } else if m11 > m22 {
        let s = 2.0 * (1.0 + m11 + (0.0 - m00) + (0.0 - m22)).sqrt();
        quat_normalize([(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s])
    } else {
        let s = 2.0 * (1.0 + m22 + (0.0 - m00) + (0.0 - m11)).sqrt();
        quat_normalize([(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s])
    }
}

/// b3d-hair's `y-axis-of`: [0 1 0] turned by a unit quaternion
fn y_axis_of([x, y, z, w]: [f64; 4]) -> Vector {
    [
        2.0 * (x * y - w * z),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + w * x),
    ]
}

/// `b3d-math/mat4-from-trs`
fn from_trs(translation: Vector, [x, y, z, w]: [f64; 4], [sx, sy, sz]: Vector) -> Matrix {
    [
        sx * (1.0 - 2.0 * (y * y + z * z)),
        sx * (2.0 * (x * y + z * w)),
        sx * (2.0 * (x * z - y * w)),
        0.0,
        sy * (2.0 * (x * y - z * w)),
        sy * (1.0 - 2.0 * (x * x + z * z)),
        sy * (2.0 * (y * z + x * w)),
        0.0,
        sz * (2.0 * (x * z + y * w)),
        sz * (2.0 * (y * z - x * w)),
        sz * (1.0 - 2.0 * (x * x + y * y)),
        0.0,
        translation[0],
        translation[1],
        translation[2],
        1.0,
    ]
}

/// `b3d-math/mat4-transform-point`
fn transform_point(m: &Matrix, [x, y, z]: Vector) -> Vector {
    let w = m[3] * x + m[7] * y + m[11] * z + m[15];
    [
        (m[0] * x + m[4] * y + m[8] * z + m[12]) / w,
        (m[1] * x + m[5] * y + m[9] * z + m[13]) / w,
        (m[2] * x + m[6] * y + m[10] * z + m[14]) / w,
    ]
}

/// `turned-lock`: the lock's joints, root first, turned to point along its `points` under the
/// world matrix `start_above` (its root's parent's, or the base), each joint below the first a
/// bone down its parent by `stretch` (its curls' stretch): each joint's new local rotation and
/// translation and its world matrix
pub fn turned_lock(
    joints: &[usize],
    points: &[Vector],
    stretch: f64,
    pose: &[Local],
    start_above: &Matrix,
) -> Vec<([f64; 4], Vector, Matrix)> {
    let mut above = *start_above;
    let mut above_turn = rotation_of(rotation_columns(start_above));
    let mut found = Vec::with_capacity(joints.len());
    for (position, &joint) in joints.iter().enumerate() {
        let local = pose[joint];
        let own = quat_multiply(above_turn, local.rotation);
        let swing = turn_between(
            y_axis_of(own),
            subtract(points[position + 1], points[position]),
        );
        let turned = quat_normalize(quat_multiply(swing, own));
        let [cx, cy, cz, cw] = above_turn;
        let rotation = quat_multiply([-cx, -cy, -cz, cw], turned);
        let stretch_here = if position > 0 { stretch } else { 1.0 };
        let place = if stretch_here == 1.0 {
            local.translation
        } else {
            scale(local.translation, stretch_here)
        };
        let world = from_trs(transform_point(&above, place), turned, local.scale);
        found.push((rotation, place, world));
        above = world;
        above_turn = turned;
    }
    found
}

/// What b3d-hair's links between locks read (`*together*`, `*apart*`, `*laid*`, `*link-give*`)
#[derive(Clone, Copy, Debug)]
pub struct LinkConstants {
    pub together: f64,
    pub apart: f64,
    pub laid: [f64; 2],
    pub give: f64,
}

/// A link between locks `a` and `b`: its rest length at each point (`links-of`)
#[derive(Clone, Debug)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub rests: Vec<f64>,
}

/// One lock as `cohered` and `settled-again` read it: its points and old places, its bones'
/// lengths as its curls stretch them now (`lengths-now`), its margins, bend limits and hair
/// thickness, and the places in the frame's shapes near each bone (None: `:nears` nil)
#[derive(Clone, Debug)]
pub struct Settling {
    pub points: Vec<Vector>,
    pub previous: Vec<Vector>,
    pub lengths: Vec<f64>,
    pub margins: Vec<f64>,
    pub limits: Vec<f64>,
    pub thickness: Option<Vec<f64>>,
    pub nears: Option<Vec<Vec<usize>>>,
}

/// `grounded-from`: every point but the root that is under the ground put on it
fn grounded(points: &[Vector], ground: f64) -> Vec<Vector> {
    points
        .iter()
        .enumerate()
        .map(|(index, &[x, y, z])| {
            if index > 0 && y < ground {
                [x, ground, z]
            } else {
                [x, y, z]
            }
        })
        .collect()
}

/// `moved-along`: each of `previous` moved as its point moved, `before` → `now`
fn moved_along(previous: &[Vector], now: &[Vector], before: &[Vector]) -> Vec<Vector> {
    previous
        .iter()
        .zip(now.iter().zip(before.iter()))
        .map(|(old, (to, from))| {
            [
                old[0] + (to[0] - from[0]),
                old[1] + (to[1] - from[1]),
                old[2] + (to[2] - from[2]),
            ]
        })
        .collect()
}

/// b3d-hair's `cohered` (every link brought back within its bounds, one pass), then
/// `settled-again` for each lock the links moved: for each lock None when its points are as they
/// were, else its new `(points, previous)`. The same sums in the same order as the Brood forms,
/// so the result is theirs exactly (ADR-821 amended, stage 3).
pub fn linked(
    locks: &[Settling],
    links: &[Link],
    shapes: &[Shape],
    constants: &Constants,
    link: &LinkConstants,
) -> Vec<Option<(Vec<Vector>, Vec<Vector>)>> {
    let ground = constants.ground;
    let mut all: Vec<Vec<Vector>> = locks.iter().map(|lock| lock.points.clone()).collect();
    for each in links {
        for index in 1..each.rests.len() {
            let [px, py, pz] = all[each.a][index];
            let [qx, qy, qz] = all[each.b][index];
            let rest = each.rests[index];
            let gx = qx - px;
            let gy = qy - py;
            let gz = qz - pz;
            let squared = gx * gx + gy * gy + gz * gz;
            let laid = py <= ground + 0.01 && qy <= ground + 0.01;
            let least = (if laid { link.laid[0] } else { link.together }) * rest;
            let most = (if laid { link.laid[1] } else { link.apart }) * rest;
            if (squared >= least * least && squared <= most * most) || squared < 1.0e-18 {
                continue;
            }
            let distance = squared.sqrt();
            let wanted = if distance < least { least } else { most };
            let share = (-link.give).max(link.give.min(0.5 * (distance - wanted))) / distance;
            let sx = gx * share;
            let sy = gy * share;
            let sz = gz * share;
            all[each.a][index] = [px + sx, py + sy, pz + sz];
            all[each.b][index] = [qx - sx, qy - sy, qz - sz];
        }
    }
    locks
        .iter()
        .zip(all)
        .map(|(lock, points)| {
            let new = grounded(&points, ground);
            if new == lock.points {
                return None;
            }
            let previous = moved_along(&lock.previous, &new, &lock.points);
            let Some(nears) = &lock.nears else {
                return Some((new, previous));
            };
            let before = new.clone();
            let mut points = new;
            let mut olds = previous.clone();
            let held = Lock {
                points: Vec::new(),
                previous: Vec::new(),
                before: Vec::new(),
                now: Vec::new(),
                carries: Vec::new(),
                nears: Vec::new(),
                stiffness: Vec::new(),
                follows: None,
                damping: Vec::new(),
                upright: 1.0,
                holds: Vec::new(),
                lengths: lock.lengths.clone(),
                margins: lock.margins.clone(),
                limits: lock.limits.clone(),
                thickness: lock.thickness.clone(),
            };
            let near: Vec<Vec<&Shape>> = nears
                .iter()
                .map(|places| places.iter().map(|&place| &shapes[place]).collect())
                .collect();
            constrained_with(&mut points, &mut olds, &held, &near, None, constants);
            let points = grounded(&points, ground);
            let previous = moved_along(&previous, &points, &before);
            Some((points, previous))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constants() -> Constants {
        Constants {
            gravity: [0.0, -9.81, 0.0],
            air_drag: 3.0,
            still: 0.0005,
            carried: 0.7,
            slide: 0.6,
            rests: 0.002,
            pressed: 0.4,
            shove: 0.018,
            unfold_give: 0.003,
            past: 0.03,
            ground: 0.015,
            clearance: 0.03,
            passes: 1,
        }
    }

    fn hanging(points: Vec<Vector>, nears: Vec<Vec<usize>>) -> Lock {
        let bones = points.len() - 1;
        Lock {
            previous: points.clone(),
            before: points.clone(),
            now: points.clone(),
            carries: vec![[0.0; 3]; bones],
            nears,
            stiffness: vec![0.0; bones],
            follows: None,
            damping: vec![1.0; bones],
            upright: 1.0,
            holds: Vec::new(),
            lengths: vec![0.05; bones],
            margins: vec![0.0; points.len()],
            limits: vec![-1.0; points.len()],
            thickness: None,
            points,
        }
    }

    #[test]
    fn a_lock_falls_under_its_weight_and_keeps_its_lengths() {
        // three points held out level from a fixed root: a step on, the free ones have dropped
        // and every bone is still its length
        let lock = hanging(
            vec![[0.0, 1.0, 0.0], [0.05, 1.0, 0.0], [0.1, 1.0, 0.0]],
            vec![Vec::new(), Vec::new()],
        );
        let (points, _) = moved(&lock, 2, 1.0 / 120.0, None, &[], &constants());
        assert_eq!(points[0], [0.0, 1.0, 0.0]);
        assert!(points[2][1] < 1.0);
        for index in 1..points.len() {
            let length = size(subtract(points[index], points[index - 1]));
            assert!((length - 0.05).abs() < 1.0e-12, "bone {index} is {length}");
        }
    }

    #[test]
    fn a_bone_inside_a_capsule_is_pushed_off_it_no_further_than_a_shove() {
        // a lock hanging straight down through a capsule across its lower bone
        let lock = hanging(
            vec![[0.0, 1.0, 0.0], [0.0, 0.95, 0.0], [0.0, 0.9, 0.0]],
            vec![Vec::new(), vec![0]],
        );
        let capsule = Shape(vec![
            -0.1, 0.92, 0.01, 0.1, 0.92, 0.01, 0.02, 0.0, 0.92, 0.01, 0.1,
        ]);
        let (points, _) = moved(&lock, 1, 1.0 / 60.0, None, &[capsule], &constants());
        // pushed toward -z, off the capsule's axis at z 0.01, at most a shove in a step
        assert!(points[2][2] < 0.0, "{:?}", points[2]);
        assert!(points[2][2] > -0.018 - 1.0e-9, "{:?}", points[2]);
    }
}
