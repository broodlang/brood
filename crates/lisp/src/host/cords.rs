//! A garment's cords drawn taut over her skin, natively (ADR-843): womanblend's
//! `womanblend-outfit-ties/cords-solved` step for every cord in one call. Each frame each cord's
//! points start where the skin under them carries their rest points, then a few passes draw each
//! point toward its neighbours' middle (in place, in order) and push every point out of the skin
//! spheres near it; each bone is then turned and stretched along its segment. The arithmetic is
//! the Brood original's, step for step, so the two agree to rounding.

use crate::host::gui::mesh::{
    JointRotations, Mesh, MeshParts, joint_rotations, skin_matrix, transform,
};
use crate::host::hair::{Matrix, Vector, from_trs, quat_multiply, transform_point};

/// A skin: `(joint, weight)` pairs, the weights summing to one.
pub type Skin = Vec<(usize, f64)>;

/// A skin sphere: its centre in the bind pose, its radius, and the skin that carries it.
#[derive(Clone, Debug)]
pub struct Sphere {
    pub centre: Vector,
    pub radius: f64,
    pub skin: Skin,
    /// The shape keys that move the skin it was fitted at: `(target, delta)` in the bind pose.
    pub deltas: Vec<(usize, Vector)>,
    /// Its centre of rotation in the bind pose, when it is skinned about one (ADR-814): a cord
    /// whose spheres all have one is posed as `gui/posed-positions` poses a mesh with `:centres`.
    pub pivot: Option<Vector>,
}

/// `centre` moved by its keys' `deltas` at this frame's key `weights` (by target), before it is
/// skinned: where her drawn skin has swollen or pressed it
fn keyed(centre: Vector, deltas: &[(usize, Vector)], weights: &[f64]) -> Vector {
    deltas.iter().fold(centre, |moved, &(target, delta)| {
        match weights.get(target) {
            Some(&weight) if weight != 0.0 => add(moved, scale(delta, weight)),
            _ => moved,
        }
    })
}

/// One cord as `cords-of` describes it.
#[derive(Clone, Debug)]
pub struct Cord {
    /// Its points in the bind pose: each bone's head and the last bone's tail.
    pub rest: Vec<Vector>,
    /// Each point's skin.
    pub skins: Vec<Skin>,
    /// Each point's spheres: indices into `spheres`.
    pub near: Vec<Vec<usize>>,
    pub spheres: Vec<Sphere>,
    /// Each bone's rest rotation, a unit quaternion.
    pub rotations: Vec<[f64; 4]>,
}

/// The solve's settings, womanblend's `*cord-…*`.
#[derive(Clone, Copy, Debug)]
pub struct Constants {
    pub passes: usize,
    pub pull: f64,
    pub step: f64,
    pub radius: f64,
    pub tries: usize,
}

/// `b3d-math/mat4-multiply`: `b`'s transform first, then `a`'s.
pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = a[row] * b[column * 4]
                + a[4 + row] * b[column * 4 + 1]
                + a[8 + row] * b[column * 4 + 2]
                + a[12 + row] * b[column * 4 + 3];
        }
    }
    out
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

fn length(a: Vector) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// `b3d-math/vec3-normalize`: the zero vector stays zero.
fn normalize(a: Vector) -> Vector {
    let size = length(a);
    if size < 1.0e-12 {
        [0.0, 0.0, 0.0]
    } else {
        [a[0] / size, a[1] / size, a[2] / size]
    }
}

fn cross(a: Vector, b: Vector) -> Vector {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Vector, b: Vector) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `b3d-math/quat-axis-angle`
fn axis_angle(axis: Vector, angle: f64) -> [f64; 4] {
    let size = length(axis);
    let [x, y, z] = if size < 1.0e-12 {
        [0.0, 0.0, 0.0]
    } else {
        scale(axis, 1.0 / size)
    };
    let sine = (0.5 * angle).sin();
    [x * sine, y * sine, z * sine, (0.5 * angle).cos()]
}

/// `quat-between`: the shortest rotation turning the unit direction `from` onto `to`
fn quat_between(from: Vector, to: Vector) -> [f64; 4] {
    let axis = cross(from, to);
    let sine = length(axis);
    let cosine = dot(from, to);
    if sine > 1.0e-9 {
        axis_angle(scale(axis, 1.0 / sine), sine.atan2(cosine))
    } else if cosine > 0.0 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        let side = cross(from, [1.0, 0.0, 0.0]);
        let axis = if length(side) > 1.0e-6 {
            normalize(side)
        } else {
            [0.0, 0.0, 1.0]
        };
        axis_angle(axis, std::f64::consts::PI)
    }
}

/// `carried`: `point` (bind pose) carried by `skin` through the skinning `matrices`
fn carried(matrices: &[Option<Matrix>], skin: &Skin, point: Vector) -> Vector {
    skin.iter().fold([0.0; 3], |sum, &(joint, weight)| {
        match matrices.get(joint).copied().flatten() {
            Some(matrix) => add(sum, scale(transform_point(&matrix, point), weight)),
            None => sum,
        }
    })
}

/// `cleared`: `point` out of the sphere of `spheres` (posed, those at `indices`) it is deepest in,
/// again until it is in none or the tries run out
fn cleared(
    mut point: Vector,
    spheres: &[(Vector, f64)],
    indices: &[usize],
    constants: &Constants,
) -> Vector {
    for _ in 0..constants.tries {
        let mut found: Option<(Vector, f64)> = None;
        let mut depth = 0.0;
        for &index in indices {
            let Some(&(centre, radius)) = spheres.get(index) else {
                continue;
            };
            let inside = (radius + constants.radius) - length(subtract(point, centre));
            if inside > depth {
                found = Some((centre, radius));
                depth = inside;
            }
        }
        let Some((centre, radius)) = found else {
            return point;
        };
        let away = subtract(point, centre);
        let distance = length(away);
        if distance <= 1.0e-9 {
            return point;
        }
        point = add(centre, scale(away, (radius + constants.radius) / distance));
    }
    point
}

/// `cord-solved`: the cord's points this frame
pub fn cord_solved(
    cord: &Cord,
    matrices: &[Option<Matrix>],
    spheres: &[(Vector, f64)],
    constants: &Constants,
) -> Vec<Vector> {
    let count = cord.rest.len();
    if count == 0 {
        return Vec::new();
    }
    let last = count - 1;
    let empty = Skin::new();
    let begun: Vec<Vector> = (0..count)
        .map(|k| carried(matrices, cord.skins.get(k).unwrap_or(&empty), cord.rest[k]))
        .collect();
    let mut points = begun.clone();
    let none: Vec<usize> = Vec::new();
    for _ in 0..constants.passes {
        points[0] = begun[0];
        points[last] = begun[last];
        for k in 1..last {
            let point = points[k];
            let toward = subtract(scale(add(points[k - 1], points[k + 1]), 0.5), point);
            let distance = length(toward);
            if distance < 1.0e-9 {
                continue;
            }
            let moved = (constants.pull * distance).min(constants.step);
            points[k] = add(point, scale(toward, moved / distance));
        }
        for (k, point) in points.iter_mut().enumerate() {
            *point = cleared(
                *point,
                spheres,
                cord.near.get(k).unwrap_or(&none),
                constants,
            );
        }
    }
    points
}

/// `cord-bone-worlds`: each bone's world matrix, at its segment's start, turned from its rest
/// direction to the segment's and stretched along its length (+Y) to it
pub fn bone_worlds(cord: &Cord, points: &[Vector]) -> Vec<Matrix> {
    let bones = cord.rotations.len().min(points.len().saturating_sub(1));
    (0..bones)
        .map(|k| {
            let now = subtract(points[k + 1], points[k]);
            let was = subtract(cord.rest[k + 1], cord.rest[k]);
            let to = if length(now) > 1.0e-9 {
                normalize(now)
            } else {
                normalize(was)
            };
            let turn = quat_between(normalize(was), to);
            from_trs(
                points[k],
                quat_multiply(turn, cord.rotations[k]),
                [1.0, length(now) / length(was).max(1.0e-9), 1.0],
            )
        })
        .collect()
}

/// A pivoted cord's spheres as a mesh, as `gui/posed-positions` reads one with `:centres`: one
/// vertex a sphere, its skin as four u16 joints and f32 weights (padded with joint 0 at no weight),
/// its pivot its centre of rotation (ADR-814). Built once with the cords (`prepare`): only the
/// spheres' places change a frame, and `skin_matrix` does not read them. None for a cord whose
/// spheres are not all pivoted.
pub fn sphere_mesh(cord: &Cord) -> Option<Mesh> {
    if cord.spheres.is_empty() || !cord.spheres.iter().all(|sphere| sphere.pivot.is_some()) {
        return None;
    }
    let mut parts = MeshParts {
        joints_wide: true,
        indices_wide: true,
        ..MeshParts::default()
    };
    for (index, sphere) in cord.spheres.iter().enumerate() {
        sphere
            .centre
            .iter()
            .for_each(|value| parts.positions.extend((*value as f32).to_le_bytes()));
        for slot in 0..4 {
            let (joint, weight) = sphere.skin.get(slot).copied().unwrap_or((0, 0.0));
            parts.joints.extend((joint as u16).to_le_bytes());
            parts.weights.extend((weight as f32).to_le_bytes());
        }
        let pivot = sphere.pivot.unwrap_or(sphere.centre);
        pivot
            .iter()
            .for_each(|value| parts.centres.extend((*value as f32).to_le_bytes()));
        for _ in 0..3 {
            parts.indices.extend((index as u32).to_le_bytes());
        }
    }
    Mesh::from_parts(parts).ok()
}

/// A pivoted cord's spheres posed as `gui/posed-positions` poses its `mesh`: each keyed centre
/// (as f32) skinned by `skin_matrix` with the palette's `rotations`, the same code and the same
/// f32 arithmetic
fn pivoted_spheres(
    cord: &Cord,
    mesh: &Mesh,
    palette: &[f32],
    rotations: &JointRotations,
    weights: &[f64],
) -> Vec<(Vector, f64)> {
    cord.spheres
        .iter()
        .enumerate()
        .map(|(index, sphere)| {
            let centre = keyed(sphere.centre, &sphere.deltas, weights).map(|value| value as f32);
            let posed = match skin_matrix(mesh, Some(palette), Some(rotations), index) {
                Some(skin) => {
                    let moved = transform(&skin, centre, 1.0);
                    [moved[0], moved[1], moved[2]]
                }
                None => centre,
            };
            (posed.map(f64::from), sphere.radius)
        })
        .collect()
}

/// The cords read back from `pack`'s bytes with what a frame needs beside them, made once
/// (`prepare`) and kept by the builtin while the same bytes are passed.
pub struct Prepared {
    pub cords: Vec<Cord>,
    pub inverse_binds: Vec<Matrix>,
    pub constants: Constants,
    /// Each cord's sphere mesh (`sphere_mesh`), None where it is not pivoted.
    pub meshes: Vec<Option<Mesh>>,
}

/// `pack`'s bytes read back and their pivoted cords' sphere meshes built.
pub fn prepare(bytes: &[u8]) -> Result<Prepared, String> {
    let (cords, inverse_binds, constants) = unpack(bytes)?;
    let meshes = cords.iter().map(sphere_mesh).collect();
    Ok(Prepared {
        cords,
        inverse_binds,
        constants,
        meshes,
    })
}

/// Every cord solved against the skeleton's `worlds` and `inverse_binds`: each cord's points and
/// bone worlds. Only the joints the skins name are multiplied out. `meshes` are the cords' sphere
/// meshes (`sphere_mesh`, one a cord, None where not pivoted).
pub fn cords_solved(
    cords: &[Cord],
    meshes: &[Option<Mesh>],
    worlds: &[Matrix],
    inverse_binds: &[Matrix],
    weights: &[f64],
    constants: &Constants,
) -> Vec<(Vec<Vector>, Vec<Matrix>)> {
    let joints = worlds.len().min(inverse_binds.len());
    let mut matrices: Vec<Option<Matrix>> = vec![None; joints];
    let mut want = |skin: &Skin| {
        for &(joint, _) in skin {
            if joint < joints && matrices[joint].is_none() {
                matrices[joint] = Some(multiply(&worlds[joint], &inverse_binds[joint]));
            }
        }
    };
    for cord in cords {
        cord.skins.iter().for_each(&mut want);
        cord.spheres.iter().for_each(|sphere| want(&sphere.skin));
    }
    // (the palette as f32 and its joints' rotations, for the cords posed about centres: once)
    let mut skinned: Option<(Vec<f32>, JointRotations)> = None;
    cords
        .iter()
        .enumerate()
        .map(|(index, cord)| {
            let spheres: Vec<(Vector, f64)> = match meshes.get(index).and_then(Option::as_ref) {
                Some(mesh) => {
                    let (palette, rotations) = skinned.get_or_insert_with(|| {
                        let palette: Vec<f32> = (0..joints)
                            .flat_map(|joint| multiply(&worlds[joint], &inverse_binds[joint]))
                            .map(|value| value as f32)
                            .collect();
                        let rotations = joint_rotations(&palette);
                        (palette, rotations)
                    });
                    pivoted_spheres(cord, mesh, palette, rotations, weights)
                }
                None => cord
                    .spheres
                    .iter()
                    .map(|sphere| {
                        (
                            carried(
                                &matrices,
                                &sphere.skin,
                                keyed(sphere.centre, &sphere.deltas, weights),
                            ),
                            sphere.radius,
                        )
                    })
                    .collect(),
            };
            let points = cord_solved(cord, &matrices, &spheres, constants);
            let bones = bone_worlds(cord, &points);
            (points, bones)
        })
        .collect()
}

/// The cords, the skeleton's inverse binds and the settings packed once into bytes (ADR-843):
/// what `%cords-packed` hands Brood to keep, so a frame reads them back natively instead of
/// converting every sphere and key delta from Brood values. Little-endian: a version, the
/// settings, the inverse binds, then each cord.
pub fn pack(cords: &[Cord], inverse_binds: &[Matrix], constants: &Constants) -> Vec<u8> {
    let mut out = Vec::new();
    let word = |out: &mut Vec<u8>, value: u64| out.extend(value.to_le_bytes());
    let real = |out: &mut Vec<u8>, value: f64| out.extend(value.to_le_bytes());
    let vector =
        |out: &mut Vec<u8>, value: Vector| value.iter().for_each(|&v| out.extend(v.to_le_bytes()));
    let skin = |out: &mut Vec<u8>, skin: &Skin| {
        out.extend((skin.len() as u64).to_le_bytes());
        for &(joint, weight) in skin {
            out.extend((joint as u64).to_le_bytes());
            out.extend(weight.to_le_bytes());
        }
    };
    word(&mut out, PACK_VERSION);
    word(&mut out, constants.passes as u64);
    real(&mut out, constants.pull);
    real(&mut out, constants.step);
    real(&mut out, constants.radius);
    word(&mut out, constants.tries as u64);
    word(&mut out, inverse_binds.len() as u64);
    for matrix in inverse_binds {
        matrix.iter().for_each(|&value| real(&mut out, value));
    }
    word(&mut out, cords.len() as u64);
    for cord in cords {
        word(&mut out, cord.rest.len() as u64);
        for k in 0..cord.rest.len() {
            vector(&mut out, cord.rest[k]);
            skin(&mut out, &cord.skins[k]);
            word(&mut out, cord.near[k].len() as u64);
            cord.near[k]
                .iter()
                .for_each(|&index| word(&mut out, index as u64));
        }
        word(&mut out, cord.rotations.len() as u64);
        for rotation in &cord.rotations {
            rotation.iter().for_each(|&value| real(&mut out, value));
        }
        word(&mut out, cord.spheres.len() as u64);
        for sphere in &cord.spheres {
            vector(&mut out, sphere.centre);
            real(&mut out, sphere.radius);
            skin(&mut out, &sphere.skin);
            word(&mut out, sphere.deltas.len() as u64);
            for &(target, delta) in &sphere.deltas {
                word(&mut out, target as u64);
                vector(&mut out, delta);
            }
            match sphere.pivot {
                Some(pivot) => {
                    word(&mut out, 1);
                    vector(&mut out, pivot);
                }
                None => word(&mut out, 0),
            }
        }
    }
    out
}

const PACK_VERSION: u64 = 2;

/// Reading `pack`'s bytes back, each count checked against what is left.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self) -> Result<[u8; 8], String> {
        let end = self.at + 8;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| "the packed cords end early".to_string())?;
        self.at = end;
        Ok(slice.try_into().unwrap_or([0; 8]))
    }
    fn word(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn count(&mut self, each: usize) -> Result<usize, String> {
        let count = self.word()? as usize;
        if count.saturating_mul(each.max(8)) > self.bytes.len() - self.at {
            return Err("the packed cords name more than they hold".to_string());
        }
        Ok(count)
    }
    fn real(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take()?))
    }
    fn vector(&mut self) -> Result<Vector, String> {
        Ok([self.real()?, self.real()?, self.real()?])
    }
    fn skin(&mut self, joints: usize) -> Result<Skin, String> {
        let count = self.count(16)?;
        (0..count)
            .map(|_| {
                let joint = self.word()? as usize;
                if joint >= joints {
                    return Err(format!("the packed cords name joint {joint} of {joints}"));
                }
                Ok((joint, self.real()?))
            })
            .collect()
    }
}

/// `pack`'s bytes read back: the cords, the inverse binds and the settings.
pub fn unpack(bytes: &[u8]) -> Result<(Vec<Cord>, Vec<Matrix>, Constants), String> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.word()? != PACK_VERSION {
        return Err("the packed cords are not %cords-packed's (or an older build's)".to_string());
    }
    let constants = Constants {
        passes: reader.word()? as usize,
        pull: reader.real()?,
        step: reader.real()?,
        radius: reader.real()?,
        tries: reader.word()? as usize,
    };
    let joints = reader.count(128)?;
    let inverse_binds = (0..joints)
        .map(|_| {
            let mut matrix = [0.0; 16];
            for value in matrix.iter_mut() {
                *value = reader.real()?;
            }
            Ok(matrix)
        })
        .collect::<Result<Vec<Matrix>, String>>()?;
    let cord_count = reader.count(32)?;
    let mut cords = Vec::with_capacity(cord_count);
    for _ in 0..cord_count {
        let points = reader.count(40)?;
        let mut rest = Vec::with_capacity(points);
        let mut skins = Vec::with_capacity(points);
        let mut near = Vec::with_capacity(points);
        for _ in 0..points {
            rest.push(reader.vector()?);
            skins.push(reader.skin(joints)?);
            let count = reader.count(8)?;
            near.push(
                (0..count)
                    .map(|_| reader.word().map(|index| index as usize))
                    .collect::<Result<Vec<usize>, String>>()?,
            );
        }
        let rotation_count = reader.count(32)?;
        let rotations = (0..rotation_count)
            .map(|_| {
                Ok([
                    reader.real()?,
                    reader.real()?,
                    reader.real()?,
                    reader.real()?,
                ])
            })
            .collect::<Result<Vec<[f64; 4]>, String>>()?;
        let sphere_count = reader.count(48)?;
        let mut spheres = Vec::with_capacity(sphere_count);
        for _ in 0..sphere_count {
            let centre = reader.vector()?;
            let radius = reader.real()?;
            let skin = reader.skin(joints)?;
            let delta_count = reader.count(32)?;
            let deltas = (0..delta_count)
                .map(|_| Ok((reader.word()? as usize, reader.vector()?)))
                .collect::<Result<Vec<(usize, Vector)>, String>>()?;
            let pivot = match reader.word()? {
                0 => None,
                _ => Some(reader.vector()?),
            };
            spheres.push(Sphere {
                centre,
                radius,
                skin,
                deltas,
                pivot,
            });
        }
        if near.iter().flatten().any(|&index| index >= spheres.len()) {
            return Err("the packed cords name a sphere past their own".to_string());
        }
        cords.push(Cord {
            rest,
            skins,
            near,
            spheres,
            rotations,
        });
    }
    Ok((cords, inverse_binds, constants))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: Matrix = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];

    fn constants() -> Constants {
        Constants {
            passes: 6,
            pull: 0.5,
            step: 0.003,
            radius: 0.0019,
            tries: 10,
        }
    }

    fn straight(points: usize, spheres: Vec<Sphere>, near: Vec<Vec<usize>>) -> Cord {
        Cord {
            rest: (0..points).map(|k| [0.03 * k as f64, 0.0, 0.0]).collect(),
            skins: vec![vec![(0, 1.0)]; points],
            near,
            spheres,
            rotations: vec![[0.0, 0.0, 0.0, 1.0]; points - 1],
        }
    }

    #[test]
    fn a_straight_cord_at_rest_stays_where_it_lies() {
        let cord = straight(5, Vec::new(), vec![Vec::new(); 5]);
        let solved = cords_solved(
            std::slice::from_ref(&cord),
            &[None],
            &[IDENTITY],
            &[IDENTITY],
            &[],
            &constants(),
        );
        let (points, bones) = &solved[0];
        assert_eq!(points, &cord.rest);
        assert_eq!(bones.len(), 4);
        assert!((bones[2][12] - 0.06).abs() < 1e-12, "bone 2 at its head");
        assert!((bones[2][5] - 1.0).abs() < 1e-12, "unstretched");
    }

    #[test]
    fn a_point_inside_a_sphere_is_put_on_its_surface_and_its_neighbours_drawn() {
        let sphere = Sphere {
            centre: [0.06, -0.01, 0.0],
            radius: 0.02,
            skin: vec![(0, 1.0)],
            deltas: Vec::new(),
            pivot: None,
        };
        let mut near = vec![Vec::new(); 5];
        near[2] = vec![0];
        let cord = straight(5, vec![sphere], near);
        let (points, _) = &cords_solved(
            &[cord],
            &[None],
            &[IDENTITY],
            &[IDENTITY],
            &[],
            &constants(),
        )[0];
        let distance = length(subtract(points[2], [0.06, -0.01, 0.0]));
        assert!(
            (distance - 0.0219).abs() < 1e-9,
            "on the sphere and its margin: {distance}"
        );
        assert_eq!(points[0], [0.0, 0.0, 0.0], "the ends pinned");
        assert!(points[1][1] > 0.0, "the neighbour drawn up toward it");
    }

    #[test]
    fn a_moved_skin_carries_the_cord() {
        let cord = straight(3, Vec::new(), vec![Vec::new(); 3]);
        let mut moved = IDENTITY;
        moved[13] = 0.5;
        let (points, bones) =
            &cords_solved(&[cord], &[None], &[moved], &[IDENTITY], &[], &constants())[0];
        assert_eq!(points[1], [0.03, 0.5, 0.0]);
        assert!((bones[0][13] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_sphere_follows_her_keys_before_it_is_skinned() {
        let sphere = Sphere {
            centre: [0.06, -0.03, 0.0],
            radius: 0.02,
            skin: vec![(0, 1.0)],
            deltas: vec![(1, [0.0, 0.01, 0.0])],
            pivot: None,
        };
        let mut near = vec![Vec::new(); 5];
        near[2] = vec![0];
        let cord = straight(5, vec![sphere], near);
        let (still, _) = &cords_solved(
            std::slice::from_ref(&cord),
            &[None],
            &[IDENTITY],
            &[IDENTITY],
            &[],
            &constants(),
        )[0];
        assert_eq!(
            still[2], cord.rest[2],
            "unkeyed the sphere is clear of the cord"
        );
        let (swollen, _) = &cords_solved(
            &[cord],
            &[None],
            &[IDENTITY],
            &[IDENTITY],
            &[0.0, 1.0],
            &constants(),
        )[0];
        let distance = length(subtract(swollen[2], [0.06, -0.02, 0.0]));
        assert!(
            (distance - 0.0219).abs() < 1e-9,
            "the keyed sphere lifts it: {distance}"
        );
    }

    #[test]
    fn packed_cords_read_back_the_same() {
        let sphere = Sphere {
            centre: [0.06, -0.03, 0.0],
            radius: 0.02,
            skin: vec![(0, 0.75), (1, 0.25)],
            deltas: vec![(3, [0.0, 0.01, 0.0])],
            pivot: Some([0.05, 0.0, 0.0]),
        };
        let mut near = vec![Vec::new(); 5];
        near[2] = vec![0];
        let cord = straight(5, vec![sphere], near);
        let bytes = pack(
            std::slice::from_ref(&cord),
            &[IDENTITY, IDENTITY],
            &constants(),
        );
        let (cords, binds, read) = unpack(&bytes).expect("reads back");
        assert_eq!(binds, vec![IDENTITY, IDENTITY]);
        assert_eq!(read.passes, 6);
        assert_eq!(cords[0].rest, cord.rest);
        assert_eq!(cords[0].spheres[0].skin, cord.spheres[0].skin);
        assert_eq!(cords[0].spheres[0].deltas, cord.spheres[0].deltas);
        assert_eq!(cords[0].spheres[0].pivot, Some([0.05, 0.0, 0.0]));
        assert_eq!(cords[0].near, cord.near);
        assert!(
            unpack(&bytes[..bytes.len() - 8]).is_err(),
            "cut short, refused"
        );
    }

    #[test]
    fn the_turn_between_opposite_directions_is_a_half_turn() {
        let turn = quat_between([0.0, 1.0, 0.0], [0.0, -1.0, 0.0]);
        assert!(turn[3].abs() < 1e-12, "a half turn: {turn:?}");
    }
}
