//! Each skinned vertex's centre of rotation (ADR-814), after Le & Hodgins 2016, "Real-time
//! skeletal skinning with optimized centers of rotation": the point the vertex's blended rotation
//! turns it about, so a bent or twisted joint keeps its cross-section rigid instead of collapsing
//! (linear blend skinning) or bulging (dual quaternions).
//!
//! A vertex's centre is the mean of the mesh's triangle centroids weighted by each triangle's area
//! and by how alike its skinning weights are to the vertex's own (their eq. 1 and 4):
//!
//! ```text
//! s(wp, wv) = Σ_{j≠k} wpj·wpk·wvj·wvk · e^(−(wpj·wvk − wpk·wvj)² / σ²),  σ = 0.1
//! p*ᵢ = Σₜ s(wᵢ, w̄ₜ)·cₜ·aₜ / Σₜ s(wᵢ, w̄ₜ)·aₜ
//! ```
//!
//! with w̄ₜ a triangle's mean weights, cₜ its centroid and aₜ its area. As they do, the sum walks
//! out from the vertex's own triangles across the mesh breadth first and stops where the similarity
//! falls under 10⁻⁶ (skinning weights are smooth on a surface); vertices are shared out between
//! threads. Their weight-space neighbour search, which also seeds the walk from parts of the mesh not
//! joined to the vertex, is not done: a vertex's alike weights lie on the surface around its joint,
//! which the walk reaches. Nor their subdivision of triangles whose corners' weights differ by more
//! than 0.1: a body mesh's triangles are small against its weights' change.
//!
//! A vertex on one bone has no centre to find (any point gives that bone's own motion): its centre
//! is itself.

use std::collections::VecDeque;

/// The width of the similarity's kernel (Le & Hodgins' σ).
pub const SIGMA: f32 = 0.1;
/// Under this similarity a triangle neither counts nor carries the walk on (their ε).
pub const CUTOFF: f32 = 1e-6;

/// A vertex's or triangle's weights as `(joint, weight)` pairs, the joints distinct.
type Sparse = Vec<(u16, f32)>;

fn sparse(influences: impl IntoIterator<Item = (u16, f32)>) -> Sparse {
    let mut out: Sparse = Vec::with_capacity(8);
    let influences: Vec<(u16, f32)> = influences.into_iter().collect();
    // (made to sum to one first, as `Mesh::from_parts` makes them before skinning: a mesh read four
    // of eight influences gives weights summing to less, and centres worked out from those parted
    // from the skinning's by up to 55 mm on her body, ADR-845)
    let total: f32 = influences.iter().map(|(_, weight)| weight.max(0.0)).sum();
    let scale = if total > 0.0 { 1.0 / total } else { 0.0 };
    for (joint, weight) in influences
        .into_iter()
        .map(|(joint, weight)| (joint, weight * scale))
    {
        if weight <= 0.0 {
            continue;
        }
        match out.iter_mut().find(|(seen, _)| *seen == joint) {
            Some((_, sum)) => *sum += weight,
            None => out.push((joint, weight)),
        }
    }
    out
}

/// Le & Hodgins' eq. 1: how alike two sets of weights are, by every pair of joints both carry.
pub fn similarity(p: &[(u16, f32)], v: &[(u16, f32)]) -> f32 {
    let weight_of = |set: &[(u16, f32)], joint: u16| {
        set.iter()
            .find(|(seen, _)| *seen == joint)
            .map_or(0.0, |(_, weight)| *weight)
    };
    let mut sum = 0.0;
    for (index, &(j, pj)) in p.iter().enumerate() {
        let vj = weight_of(v, j);
        if vj <= 0.0 {
            continue;
        }
        for &(k, pk) in &p[index + 1..] {
            let vk = weight_of(v, k);
            if vk <= 0.0 {
                continue;
            }
            let across = pj * vk - pk * vj;
            // (j, k) and (k, j) alike: the sum over j ≠ k counts each pair twice
            sum += 2.0 * pj * pk * vj * vk * (-(across * across) / (SIGMA * SIGMA)).exp();
        }
    }
    sum
}

/// Every vertex's centre of rotation (see the module): `positions` at rest, triangles `indices`,
/// and each vertex's four `joints` and `weights`, with its second four `joints1` and `weights1`
/// when it has eight (ADR-845; empty slices for four); computed by `workers` threads.
pub fn centres(
    positions: &[[f32; 3]],
    indices: &[u32],
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    joints1: &[[u16; 4]],
    weights1: &[[f32; 4]],
    workers: usize,
) -> Vec<[f32; 3]> {
    let count = positions.len();
    let vertex_weights: Vec<Sparse> = (0..count)
        .map(|index| {
            let set = |joints: &[[u16; 4]], weights: &[[f32; 4]]| {
                joints
                    .get(index)
                    .copied()
                    .unwrap_or([0; 4])
                    .into_iter()
                    .zip(weights.get(index).copied().unwrap_or([0.0; 4]))
            };
            sparse(set(joints, weights).chain(set(joints1, weights1)))
        })
        .collect();
    let triangles: Vec<[usize; 3]> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|corners| {
            [
                corners[0] as usize,
                corners[1] as usize,
                corners[2] as usize,
            ]
        })
        .filter(|corners| corners.iter().all(|corner| *corner < count))
        .collect();
    // each triangle's centroid, area and mean weights
    let mut centroids = Vec::with_capacity(triangles.len());
    let mut areas = Vec::with_capacity(triangles.len());
    let mut means: Vec<Sparse> = Vec::with_capacity(triangles.len());
    for corners in &triangles {
        let [a, b, c] = corners.map(|corner| positions[corner]);
        centroids.push([0, 1, 2].map(|axis| (a[axis] + b[axis] + c[axis]) / 3.0));
        let ab = [0, 1, 2].map(|axis| b[axis] - a[axis]);
        let ac = [0, 1, 2].map(|axis| c[axis] - a[axis]);
        let cross = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        areas.push(0.5 * (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt());
        let mut mean: Sparse = Vec::with_capacity(12);
        for corner in corners {
            for &(joint, weight) in &vertex_weights[*corner] {
                match mean.iter_mut().find(|(seen, _)| *seen == joint) {
                    Some((_, sum)) => *sum += weight / 3.0,
                    None => mean.push((joint, weight / 3.0)),
                }
            }
        }
        means.push(mean);
    }
    // the triangles at each POINT of the surface, for the walk: vertices at one position (a UV
    // seam's two sides, a hard edge's) are one point, so the walk crosses a seam and both sides
    // find one centre. Walked by vertex alone, each side found its centre from its own half, and
    // the seam opened as a dark crack as she bent (her belly, her collarbones, 2026-11-26)
    let mut point_of_position: std::collections::HashMap<[u32; 3], u32> =
        std::collections::HashMap::with_capacity(count);
    let point_of: Vec<u32> = positions
        .iter()
        .map(|position| {
            let next = point_of_position.len() as u32;
            *point_of_position
                .entry(position.map(f32::to_bits))
                .or_insert(next)
        })
        .collect();
    let mut at_point: Vec<Vec<u32>> = vec![Vec::new(); point_of_position.len()];
    for (triangle, corners) in triangles.iter().enumerate() {
        for corner in corners {
            at_point[point_of[*corner] as usize].push(triangle as u32);
        }
    }
    let at_vertex = |vertex: usize| &at_point[point_of[vertex] as usize];
    let centre_of = |vertex: usize, seen: &mut Vec<u32>, stamp: u32| -> [f32; 3] {
        let own = &vertex_weights[vertex];
        if own.len() < 2 {
            return positions[vertex];
        }
        let mut weighted = [0.0f64; 3];
        let mut total = 0.0f64;
        let mut queue: VecDeque<u32> = VecDeque::new();
        for &triangle in at_vertex(vertex) {
            if seen[triangle as usize] != stamp {
                seen[triangle as usize] = stamp;
                queue.push_back(triangle);
            }
        }
        while let Some(triangle) = queue.pop_front() {
            let triangle = triangle as usize;
            let alike = similarity(own, &means[triangle]);
            if alike < CUTOFF {
                continue;
            }
            let share = f64::from(alike) * f64::from(areas[triangle]);
            for axis in 0..3 {
                weighted[axis] += share * f64::from(centroids[triangle][axis]);
            }
            total += share;
            for corner in triangles[triangle] {
                for &next in at_vertex(corner) {
                    if seen[next as usize] != stamp {
                        seen[next as usize] = stamp;
                        queue.push_back(next);
                    }
                }
            }
        }
        if total <= 0.0 {
            positions[vertex]
        } else {
            weighted.map(|sum| (sum / total) as f32)
        }
    };
    let workers = workers.clamp(1, 64).min(count.max(1));
    let chunk = count.div_ceil(workers).max(1);
    let mut out = vec![[0.0f32; 3]; count];
    std::thread::scope(|scope| {
        for (part, slice) in out.chunks_mut(chunk).enumerate() {
            let centre_of = &centre_of;
            let triangle_count = triangles.len();
            scope.spawn(move || {
                // which triangles this vertex's walk has met: a stamp a vertex, never cleared
                let mut seen = vec![0u32; triangle_count];
                for (offset, centre) in slice.iter_mut().enumerate() {
                    let vertex = part * chunk + offset;
                    *centre = centre_of(vertex, &mut seen, offset as u32 + 1);
                }
            });
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tube along y bent at y = 0 between two bones: below on joint 0, above on joint 1,
    /// blended across the joint. A vertex on the blend has its centre on the tube's axis near the
    /// joint (where the two bones turn about each other), not on its own surface; a vertex on one
    /// bone alone is its own centre.
    #[test]
    fn a_blended_vertex_turns_about_the_joint_inside_its_tube() {
        let rings = 21;
        let sides = 12;
        let mut positions = Vec::new();
        let mut joints = Vec::new();
        let mut weights = Vec::new();
        for ring in 0..rings {
            let y = ring as f32 / (rings - 1) as f32 * 2.0 - 1.0;
            // the upper bone's share: 0 below −0.3, 1 above 0.3, linear between
            let upper = ((y + 0.3) / 0.6).clamp(0.0, 1.0);
            for side in 0..sides {
                let angle = side as f32 / sides as f32 * std::f32::consts::TAU;
                positions.push([0.2 * angle.cos(), y, 0.2 * angle.sin()]);
                joints.push([0, 1, 0, 0]);
                weights.push([1.0 - upper, upper, 0.0, 0.0]);
            }
        }
        let mut indices = Vec::new();
        for ring in 0..rings - 1 {
            for side in 0..sides {
                let a = (ring * sides + side) as u32;
                let b = (ring * sides + (side + 1) % sides) as u32;
                let c = a + sides as u32;
                let d = b + sides as u32;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        let found = centres(&positions, &indices, &joints, &weights, &[], &[], 4);
        // the ring at y = 0 (ring 10): blended half and half
        let blended = found[10 * sides];
        let off_axis = (blended[0] * blended[0] + blended[2] * blended[2]).sqrt();
        assert!(off_axis < 0.05, "on the axis: {blended:?}");
        assert!(blended[1].abs() < 0.1, "near the joint: {blended:?}");
        // the bottom ring is on joint 0 alone: its own centre
        assert_eq!(found[0], positions[0]);
        // and the similarity is symmetric and largest for the same weights
        let half = [(0u16, 0.5f32), (1, 0.5)];
        let tilted = [(0u16, 0.8f32), (1, 0.2)];
        assert!((similarity(&half, &tilted) - similarity(&tilted, &half)).abs() < 1e-7);
        assert!(similarity(&half, &half) > similarity(&half, &tilted));
    }

    /// The same tube bent 90° at its joint (the upper bone turned about z through the joint):
    /// blending the joints' matrices collapses the ring on the joint to about 0.71 of its size
    /// across; turned about their centres, its vertices keep it whole (ADR-814).
    #[test]
    fn a_bent_joint_keeps_its_cross_section_when_vertices_turn_about_their_centres() {
        use crate::host::gui::mesh::{Mesh, MeshParts, joint_rotations, skin_matrix, transform};
        let rings = 21;
        let sides = 12;
        let mut positions = Vec::new();
        let mut joints = Vec::new();
        let mut weights = Vec::new();
        for ring in 0..rings {
            let y = ring as f32 / (rings - 1) as f32 * 2.0 - 1.0;
            let upper = ((y + 0.3) / 0.6).clamp(0.0, 1.0);
            for side in 0..sides {
                let angle = side as f32 / sides as f32 * std::f32::consts::TAU;
                positions.extend_from_slice(&[0.2 * angle.cos(), y, 0.2 * angle.sin()]);
                joints.extend_from_slice(&[0u8, 1, 0, 0]);
                weights.extend_from_slice(&[1.0 - upper, upper, 0.0, 0.0]);
            }
        }
        let mut indices: Vec<u32> = Vec::new();
        for ring in 0..rings - 1 {
            for side in 0..sides {
                let a = (ring * sides + side) as u32;
                let b = (ring * sides + (side + 1) % sides) as u32;
                indices.extend_from_slice(&[
                    a,
                    b,
                    a + sides as u32,
                    b,
                    b + sides as u32,
                    a + sides as u32,
                ]);
            }
        }
        let bytes = |values: &[f32]| {
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<u8>>()
        };
        let plain = Mesh::from_parts(MeshParts {
            positions: bytes(&positions),
            joints: joints.clone(),
            weights: bytes(&weights),
            indices: indices
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            indices_wide: true,
            ..MeshParts::default()
        })
        .expect("a tube");
        let found = centres(
            &plain.positions,
            &plain.indices,
            &plain.joints,
            &plain.weights,
            &plain.joints1,
            &plain.weights1,
            2,
        );
        let mut turning = plain.clone();
        turning.centres = found;
        // joint 0 still, joint 1 turned 90° about z (through the joint, at the origin)
        let palette: Vec<f32> = [
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            [
                0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        ]
        .concat();
        let rotations = joint_rotations(&palette);
        let across =
            |mesh: &Mesh, rotations: Option<&crate::host::gui::mesh::JointRotations>| -> f32 {
                // the ring on the joint (ring 10), posed: its width across the bend, between its
                // two vertices on x (z is not turned)
                let ring: Vec<[f32; 3]> = (0..sides)
                    .map(|side| {
                        let index = 10 * sides + side;
                        let skin =
                            skin_matrix(mesh, Some(&palette), rotations, index).expect("skinned");
                        let posed = transform(&skin, mesh.positions[index], 1.0);
                        [posed[0], posed[1], posed[2]]
                    })
                    .collect();
                let (a, b) = (ring[0], ring[sides / 2]);
                ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
            };
        let blended = across(&plain, None);
        let turned = across(&turning, Some(&rotations));
        assert!(
            blended < 0.33,
            "the matrices blended collapse it: {blended}"
        );
        assert!(
            turned > 0.38,
            "turned about their centres it stays 0.4 across: {turned}"
        );
        // a rigid joint's rotation is itself, with nothing left over
        assert!(
            rotations.stretches[1]
                .iter()
                .zip([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
                .all(|(a, b)| (a - b).abs() < 1e-5)
        );
    }

    /// A UV seam: the tube's seam split into two columns of vertices at one place, joined by no
    /// triangle. Both sides of the seam find one centre (the walk goes over the surface's points);
    /// walked by vertex, each side found its own and the seam opened as she bent.
    #[test]
    fn a_seams_two_sides_find_one_centre() {
        let (rings, sides) = (21usize, 12usize);
        // a column a side, and one more at the seam's place: around index `sides` repeats 0
        let columns = sides + 1;
        let mut positions = Vec::new();
        let mut joints = Vec::new();
        let mut weights = Vec::new();
        for ring in 0..rings {
            let y = ring as f32 / (rings - 1) as f32 * 2.0 - 1.0;
            let upper = ((y + 0.3) / 0.6).clamp(0.0, 1.0);
            for column in 0..columns {
                let angle = (column % sides) as f32 / sides as f32 * std::f32::consts::TAU;
                positions.push([0.2 * angle.cos(), y, 0.2 * angle.sin()]);
                joints.push([0, 1, 0, 0]);
                weights.push([1.0 - upper, upper, 0.0, 0.0]);
            }
        }
        let mut indices = Vec::new();
        for ring in 0..rings - 1 {
            for column in 0..sides {
                let a = (ring * columns + column) as u32;
                let b = a + 1;
                let c = a + columns as u32;
                let d = b + columns as u32;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        let found = centres(&positions, &indices, &joints, &weights, &[], &[], 2);
        // the seam's two vertices on the joint's ring: column 0 and column `sides`
        let (left, right) = (found[10 * columns], found[10 * columns + sides]);
        let apart = (0..3)
            .map(|axis| (left[axis] - right[axis]).abs())
            .fold(0.0f32, f32::max);
        assert!(apart < 1e-6, "one centre: {left:?} against {right:?}");
        let off_axis = (left[0] * left[0] + left[2] * left[2]).sqrt();
        assert!(off_axis < 0.05, "on the axis: {left:?}");
    }
}
