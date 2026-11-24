//! 3D meshes and the scenes that draw them (ADR-786): the MECHANISM a 3D game needs and
//! nothing more. A mesh is uploaded once (`gui-mesh`): positions, normals,
//! coordinates, optional tangents, the joints and weights that skin it to a skeleton, the
//! triangles, and morph targets. A `[:scene …]` op draws meshes into a pixel rect of the
//! frame: a camera (one view-projection matrix), a few directional lights over a sky/ground
//! ambient, the skeleton's joint matrices for this frame (a PALETTE), and per draw the mesh,
//! its model matrix, its morph weights and a material from a FIXED shading model. Loading a
//! glTF, posing a skeleton, a camera's maths, animation — all Brood policy above this.
//!
//! The shading model is fixed on purpose (ADR-413: a frame carries no shader code). It is
//! written ONCE here for the CPU painter, which rasterises a scene in software
//! ([`rasterize`]), and mirrored in WGSL for the GPU target (`gpu/shaders.rs`, `MESH_SRC`):
//! pre-integrated subsurface scattering for skin (ADR-794: a table of the diffuse light by
//! the light's angle and the surface's curvature, [`skin_table`]), Blinn-Phong specular
//! normalised by its exponent with Schlick's Fresnel (two lobes for skin), a fresnel rim, a
//! hemispherical or spherical-harmonic ambient times an occlusion map, shadow maps filtered
//! 3×3 or soft (ADR-793), a tangent-space normal map, a tiled detail normal over it whose
//! cavity damps the specular (ADR-797), alpha cut out (lashes, brows) or blended (hair), ACES
//! tone mapping and sRGB.
//! A scene is composited into the frame as a premultiplied picture where its op stands, so
//! 2D ops before it are behind it and ops after it in front.

use std::collections::HashMap;
use std::sync::Arc;

/// The most lights a scene lights with (a spec naming more is refused).
pub const MAX_LIGHTS: usize = 8;
/// The most lights of a scene that cast shadows (ADR-788).
pub const MAX_SHADOWS: usize = 4;
/// The most shadow maps a scene renders: one a directional or spot light, six (a cube's
/// faces) a point light.
pub const MAX_SHADOW_VIEWS: usize = MAX_SHADOWS * 6;
/// The smallest and largest side of a shadow map, in texels, and the side a light without
/// `:resolution` gets.
pub const MIN_SHADOW_SIDE: u32 = 16;
pub const MAX_SHADOW_SIDE: u32 = 4096;
pub const DEFAULT_SHADOW_SIDE: u32 = 1024;
/// The most texels a scene's shadow maps may hold together: the GPU target keeps them as one
/// array of layers as big as the largest, so `views × side²` — 128 MiB of 32-bit depth.
pub const MAX_SHADOW_TEXELS: u64 = 1 << 25;
/// How far, in shadow-map texels, a receiver is moved toward the light (times one plus the
/// slope of its surface to the light) and along its normal before it is compared, by default.
pub const DEFAULT_SHADOW_BIAS: f32 = 1.0;
pub const DEFAULT_SHADOW_NORMAL_BIAS: f32 = 1.5;
/// A spot or point light's shadow near plane, in metres, by default: nearer casters are clipped.
pub const DEFAULT_SHADOW_NEAR: f32 = 0.05;
/// The widest a soft shadow's penumbra grows, in shadow-map texels of radius, for a light with
/// a `:size` (or `:angle`) and no `:softness` (ADR-793).
pub const DEFAULT_SOFTNESS: f32 = 32.0;
/// The narrowest a soft shadow's penumbra is, in texels of radius: at contact its filter is
/// about the 3×3 one's width, so a touching shadow is crisp but not stair-stepped.
pub const MIN_SOFTNESS: f32 = 1.0;
/// How many depth-map reads a soft shadow takes: as many to look for blockers, then as many
/// filtered comparisons (ADR-793).
pub const SOFT_TAPS: u32 = 32;
/// A material's `:specular` times this is its reflectance at normal incidence, F0 (ADR-793):
/// UE4's mapping, so the default 0.5 is 4 %, a common dielectric's, and skin's 2.8 % is 0.35.
pub const SPECULAR_TO_F0: f32 = 0.08;
/// A `:skin` surface's two specular lobes by default: roughness times 0.75 and times 1.3,
/// weighted 0.85 and 0.15 (UE4's dual-lobe skin, ADR-793).
pub const SKIN_LOBES: [f32; 3] = [0.75, 1.3, 0.85];
/// The skin table's columns (ADR-794): the cosine between the normal and the light from −1 to
/// 1, a column at each end and an ODD count, so the terminator (cosine 0) is a column of its
/// own and the flat row, plain Lambert with its corner there, is read back exactly.
pub const SKIN_TABLE_WIDTH: usize = 65;
/// The skin table's rows: the surface's curvature, row `index` holding `(index / 31)²` per
/// millimetre — flat in row 0, a 1 mm radius in the last, the rows closest where a body's
/// curvatures are (a thigh's 0.0125 per millimetre is row 3.5, a nose's 0.1 row 10).
pub const SKIN_TABLE_HEIGHT: usize = 32;
/// The sharpest curvature the skin table holds, per millimetre (a radius of 1 mm); a sharper
/// surface is read as this.
pub const SKIN_MAX_CURVATURE: f32 = 1.0;
/// A `:skin` surface's `:scatter` by default: human skin on a mesh measured in metres.
pub const DEFAULT_SCATTER: f32 = 1.0;
/// The most a material's `:scatter` may be (a mesh in millimetres is 0.001, one in kilometres
/// would be this).
pub const MAX_SCATTER: f32 = 1.0e6;
/// The millimetres in one of a mesh's units, taken as metres: the diffusion profile's
/// distances are millimetres.
pub const SKIN_MILLIMETRES_PER_UNIT: f32 = 1000.0;
/// What a `:detail` texture's red or green channel reads where its normal does not tilt
/// (ADR-797): the byte 128, the one a mip chain's averages of a tile that tilts as much one way
/// as the other land on. Half, 127.5, is no byte, so `2t − 1` would leave every flat texel
/// tilted by a 255th.
pub const DETAIL_FLAT: f32 = 128.0 / 255.0;
/// A material's `:detail-strength` and `:detail-cavity` by default: the tile as it was baked.
pub const DEFAULT_DETAIL_STRENGTH: f32 = 1.0;
pub const DEFAULT_DETAIL_CAVITY: f32 = 1.0;
/// The most a `:detail-strength` may be, and the most times a `:detail-scale` may repeat the
/// tile across the UV square.
pub const MAX_DETAIL_STRENGTH: f32 = 16.0;
pub const MAX_DETAIL_SCALE: f32 = 4096.0;
/// The skin diffusion profile (ADR-794): d'Eon and Luebke's sum of six Gaussians fitted to a
/// three-layer skin model (GPU Gems 3, chapter 14), each `[variance mm², red, green, blue]`.
/// The weights sum to 1 a channel, so the skin's colour is its texture's and the profile only
/// moves light.
pub const SKIN_PROFILE: [[f64; 4]; 6] = [
    [0.0064, 0.233, 0.455, 0.649],
    [0.0484, 0.100, 0.336, 0.344],
    [0.187, 0.118, 0.198, 0.0],
    [0.567, 0.113, 0.007, 0.007],
    [1.99, 0.358, 0.004, 0.0],
    [7.41, 0.078, 0.0, 0.0],
];
/// The steps each Gaussian of the profile is integrated over when the skin table is built.
const SKIN_INTEGRATION_STEPS: usize = 128;
/// The shortest edge a mesh's curvature is read along, in its units: across a shorter one (two
/// vertices all but on one point: a pole's, a pinched seam's) the difference of two normals is
/// rounding, and over so short a length it would read as a needle's point.
const CURVATURE_SHORTEST_EDGE: f32 = 1e-6;
/// The neighbour averagings a mesh's curvature is smoothed by ([`vertex_curvature`]).
const CURVATURE_SMOOTHING_PASSES: usize = 2;
/// The distance under which a point or spot light's inverse-square falloff stops growing (a
/// surface touching the bulb is not infinitely bright).
pub const NEAR_LIGHT: f32 = 0.01;
/// The texels each cube face of a point light's shadow reaches past its 90°, so the filter of
/// a receiver on a face's edge stays on that face.
const CUBE_MARGIN: f32 = 3.0;
/// The largest side of a scene's picture, in pixels.
pub const MAX_SCENE_SIDE: u32 = 4096;
/// The taps the ambient occlusion reads a fragment: fixed points over the hemisphere its surface
/// faces (the same set in both painters, so their pictures agree).
pub const OCCLUSION_TAPS: usize = 16;
/// The largest side, in texels, of the camera's depth view the ambient occlusion reads: it
/// only needs the scene's shape, not its pixels.
pub const MAX_OCCLUSION_SIDE: u32 = 2048;
/// How far a `:hair` surface's two highlights are shifted along its normal, as a share of the
/// strand direction (the first toward the root, the second toward the tip), and how much
/// broader the second is (its exponent this share of the first's).
pub const HAIR_SHIFT_PRIMARY: f32 = -0.08;
pub const HAIR_SHIFT_SECONDARY: f32 = 0.12;
pub const HAIR_SECONDARY_SHARPNESS: f32 = 0.25;
/// The coverage each of a 2×2 block of CPU samples needs to be drawn by a `:hair` surface (an
/// ordered dither, row by row; the GPU target's samples 0 … 3 in the same places).
/// A `:strands` surface's width at the root and the tip by default, metres: a strand stands for
/// a few hairs (a clump), so that at a few metres it covers part of a pixel rather than none.
pub const DEFAULT_STRAND_WIDTH: [f32; 2] = [0.0005, 0.0002];
pub const HAIR_COVERAGE_STEPS: [[f32; 2]; 2] = [[0.125, 0.625], [0.875, 0.375]];

/// The reach of the ambient occlusion, in metres, when `:ambient-occlusion` names none: the
/// size of the folds it darkens (a mouth's corner, a crease beside the nose).
pub const DEFAULT_OCCLUSION_RADIUS: f32 = 0.03;
/// The most vertices one mesh may have: 84 bytes each on the GPU target stays under the
/// smallest buffer every device allows (256 MiB).
pub const MAX_VERTICES: usize = 3 << 20;
/// The most bytes of morph targets one mesh may carry as uploaded: 24 bytes a vertex a target
/// (position and normal offsets, every vertex, as a glTF buffer holds them), before they are
/// kept sparse. Neither painter keeps them dense: the GPU target uploads one blended offset a
/// vertex a frame. The cap bounds what one `gui/mesh` call may make the host unpack.
pub const MAX_MORPH_BYTES: usize = 128 << 20;
/// The most indices one mesh may have (4 bytes each on the GPU target, under 256 MiB).
pub const MAX_INDICES: usize = 63 << 20;

/// One morph target: offsets added to the positions and normals of the vertices it moves,
/// scaled by the target's weight in a draw. Kept sparse: a face's target moves a few thousand of
/// a body's seventy thousand vertices, so blending one costs what it moves, not the whole mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Morph {
    /// The vertices it moves (its position or its normal), ascending.
    pub vertices: Vec<u32>,
    /// Their offsets, one a listed vertex.
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

/// Each vertex `morphs` move (`(target, weight)` pairs; a target past `targets`, or of no
/// weight, skipped), with the weight and that target's offsets to its position and normal:
/// what both painters blend, touching only the vertices a target moves.
fn each_morphed(
    targets: &[Morph],
    morphs: &[(u32, f32)],
    mut visit: impl FnMut(usize, f32, [f32; 3], [f32; 3]),
) {
    for &(target, weight) in morphs {
        let Some(morph) = targets.get(target as usize) else {
            continue;
        };
        if weight == 0.0 {
            continue;
        }
        for ((&vertex, offset), turn) in morph
            .vertices
            .iter()
            .zip(&morph.positions)
            .zip(&morph.normals)
        {
            visit(vertex as usize, weight, *offset, *turn);
        }
    }
}

/// Adds `morphs` to `positions` and `normals`, one entry a vertex: the CPU painter's blend,
/// onto the mesh's own.
pub fn add_morphs(
    targets: &[Morph],
    morphs: &[(u32, f32)],
    positions: &mut [[f32; 3]],
    normals: &mut [[f32; 3]],
) {
    each_morphed(targets, morphs, |vertex, weight, offset, turn| {
        for axis in 0..3 {
            positions[vertex][axis] += weight * offset[axis];
            normals[vertex][axis] += weight * turn[axis];
        }
    });
}

/// Adds `morphs` to `packed`, the GPU target's offsets as its vertex shader reads them: 32
/// little-endian bytes a vertex, the position's offset (3 f32 and a pad), then the normal's.
/// Blended straight into the bytes it uploads, so a frame zeroes the draw's span once and
/// touches only the vertices its targets move.
pub fn add_morphs_packed(targets: &[Morph], morphs: &[(u32, f32)], packed: &mut [u8]) {
    let mut add = |at: usize, amount: f32| {
        let bytes: &mut [u8; 4] = (&mut packed[at..at + 4]).try_into().expect("four bytes");
        *bytes = (f32::from_le_bytes(*bytes) + amount).to_le_bytes();
    };
    each_morphed(targets, morphs, |vertex, weight, offset, turn| {
        for axis in 0..3 {
            add(vertex * 32 + axis * 4, weight * offset[axis]);
            add(vertex * 32 + 16 + axis * 4, weight * turn[axis]);
        }
    });
}

/// A mesh as uploaded and validated: every attribute has one entry per vertex (or none for
/// the optional ones), every index names a vertex, and the triangles are whole.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Tangents with the bitangent's sign in `w` (glTF's convention), for a normal map.
    pub tangents: Option<Vec<[f32; 4]>>,
    /// The four joints each vertex is skinned to (indices into a draw's palette) and their
    /// weights; empty for a rigid mesh.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub morphs: Vec<Morph>,
    /// How sharply the surface curves at each vertex, 1 over its radius in the mesh's own
    /// units, found once from its positions and normals ([`vertex_curvature`]): what a `:skin`
    /// surface's scattering reads (ADR-794).
    pub curvature: Vec<f32>,
    /// Where its vertices can be, for fitting a shadow map to a scene without posing them.
    pub bounds: MeshBounds,
}

/// How sharply a mesh curves at each vertex: 1 over the radius of the surface there, in the
/// mesh's own units (ADR-794). Along a triangle's edge the normal turns by `|Δnormal|` over
/// `|Δposition|`, which on a sphere of radius r is exactly 1/r; a vertex's curvature is the mean
/// over the edges that meet it, then averaged with its neighbours' [`CURVATURE_SMOOTHING_PASSES`]
/// times (light under skin travels millimetres, so a curvature finer than the mesh's edges means
/// nothing to it, and the mean of a few edges is noisy). Vertices at one position (a UV seam's
/// two sides, a hard edge's) are one point of the surface and get one value, so no seam shows.
/// It is the mesh at rest: a pose or a morph does not change it.
pub fn vertex_curvature(positions: &[[f32; 3]], normals: &[[f32; 3]], indices: &[u32]) -> Vec<f32> {
    // the surface's points: vertices at the same position share one
    let mut point_of_position: HashMap<[u32; 3], u32> = HashMap::with_capacity(positions.len());
    let points: Vec<u32> = positions
        .iter()
        .map(|position| {
            // −0.0 and 0.0 are one place
            let key = position.map(|coordinate| (coordinate + 0.0).to_bits());
            let next = point_of_position.len() as u32;
            *point_of_position.entry(key).or_insert(next)
        })
        .collect();
    let point_count = point_of_position.len();
    drop(point_of_position);
    let each_edge = |visit: &mut dyn FnMut(usize, usize)| {
        for triangle in indices.as_chunks::<3>().0 {
            for corner in 0..3 {
                visit(
                    triangle[corner] as usize,
                    triangle[(corner + 1) % 3] as usize,
                );
            }
        }
    };
    let mut totals = vec![0.0f32; point_count];
    let mut counts = vec![0u32; point_count];
    each_edge(&mut |from, to| {
        let along = subtract(positions[to], positions[from]);
        let length = dot(along, along).sqrt();
        if length <= CURVATURE_SHORTEST_EDGE {
            return;
        }
        let turn = subtract(normalize(normals[to]), normalize(normals[from]));
        let curvature = dot(turn, turn).sqrt() / length;
        if !curvature.is_finite() {
            return;
        }
        for end in [from, to] {
            totals[points[end] as usize] += curvature;
            counts[points[end] as usize] += 1;
        }
    });
    let mut curvature: Vec<f32> = totals
        .iter()
        .zip(&counts)
        .map(|(total, count)| {
            if *count > 0 {
                total / *count as f32
            } else {
                0.0
            }
        })
        .collect();
    for _ in 0..CURVATURE_SMOOTHING_PASSES {
        // each point with the points its edges reach, itself counted once
        let mut sums = curvature.clone();
        let mut shares = vec![1u32; point_count];
        each_edge(&mut |from, to| {
            let (first, second) = (points[from] as usize, points[to] as usize);
            if first == second {
                return;
            }
            sums[first] += curvature[second];
            shares[first] += 1;
            sums[second] += curvature[first];
            shares[second] += 1;
        });
        curvature = sums
            .iter()
            .zip(&shares)
            .map(|(sum, share)| sum / *share as f32)
            .collect();
    }
    points
        .iter()
        .map(|point| curvature[*point as usize])
        .collect()
}

/// An axis-aligned box: its least and greatest corner.
pub type Bounds = [[f32; 3]; 2];

/// Where a mesh's vertices can be once a draw has morphed and skinned them, kept per joint so
/// a posed bound stays tight (ADR-788): a skinned vertex is a weighted mean of its joints'
/// matrices applied to it, so it lies in the box around every joint's matrix applied to the
/// box of the vertices that joint moves. Both painters fit a shadow map from these alone, so
/// they fit it alike.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshBounds {
    /// The box of the vertices no joint moves (every vertex of a rigid mesh).
    pub rigid: Option<Bounds>,
    /// Each joint some vertex is weighted to, with the box of those vertices.
    pub joints: Vec<(u16, Bounds)>,
    /// Each morph target's box of position offsets.
    pub morphs: Vec<Bounds>,
}

fn extend(bounds: Option<Bounds>, point: [f32; 3]) -> Option<Bounds> {
    Some(match bounds {
        None => [point, point],
        Some([low, high]) => [
            [
                low[0].min(point[0]),
                low[1].min(point[1]),
                low[2].min(point[2]),
            ],
            [
                high[0].max(point[0]),
                high[1].max(point[1]),
                high[2].max(point[2]),
            ],
        ],
    })
}

/// The box around both, either of which may be none.
pub fn union(left: Option<Bounds>, right: Option<Bounds>) -> Option<Bounds> {
    match (left, right) {
        (Some(box_left), Some(box_right)) => {
            extend(extend(Some(box_left), box_right[0]), box_right[1])
        }
        (one, None) | (None, one) => one,
    }
}

fn corners(bounds: Bounds) -> [[f32; 3]; 8] {
    let [low, high] = bounds;
    let mut out = [[0.0f32; 3]; 8];
    for (index, corner) in out.iter_mut().enumerate() {
        *corner = [
            if index & 1 == 0 { low[0] } else { high[0] },
            if index & 2 == 0 { low[1] } else { high[1] },
            if index & 4 == 0 { low[2] } else { high[2] },
        ];
    }
    out
}

impl MeshBounds {
    fn of(
        positions: &[[f32; 3]],
        joints: &[[u16; 4]],
        weights: &[[f32; 4]],
        morphs: &[Morph],
    ) -> MeshBounds {
        let mut rigid = None;
        let mut by_joint: HashMap<u16, Bounds> = HashMap::new();
        for (index, position) in positions.iter().enumerate() {
            let mut moved = false;
            if !joints.is_empty() {
                for (joint, weight) in joints[index].iter().zip(weights[index]) {
                    if weight > 0.0 {
                        moved = true;
                        let grown = extend(by_joint.get(joint).copied(), *position);
                        by_joint.insert(*joint, grown.expect("extended"));
                    }
                }
            }
            if !moved {
                rigid = extend(rigid, *position);
            }
        }
        let mut joints: Vec<(u16, Bounds)> = by_joint.into_iter().collect();
        joints.sort_by_key(|(joint, _)| *joint);
        let morphs = morphs
            .iter()
            .map(|morph| {
                // the vertices it leaves stay put: the box holds no offset
                morph
                    .positions
                    .iter()
                    .fold([[0.0f32; 3]; 2], |[low, high], offset| {
                        [
                            [0, 1, 2].map(|axis| low[axis].min(offset[axis])),
                            [0, 1, 2].map(|axis| high[axis].max(offset[axis])),
                        ]
                    })
            })
            .collect();
        MeshBounds {
            rigid,
            joints,
            morphs,
        }
    }
}

/// A mesh's attributes as raw little-endian bytes, the way a glTF buffer holds them, before
/// [`Mesh::from_parts`] checks and unpacks them.
#[derive(Default)]
pub struct MeshParts {
    pub positions: Vec<u8>,
    pub normals: Vec<u8>,
    pub uvs: Vec<u8>,
    pub tangents: Vec<u8>,
    /// `u8` or `u16` joint indices, four a vertex, per `joints_wide`.
    pub joints: Vec<u8>,
    pub joints_wide: bool,
    pub weights: Vec<u8>,
    /// `u16` or `u32` indices, per `indices_wide`; empty for a plain triangle list (the
    /// vertices taken three at a time, as glTF allows).
    pub indices: Vec<u8>,
    pub indices_wide: bool,
    pub morphs: Vec<(Vec<u8>, Vec<u8>)>,
}

fn floats<const N: usize>(name: &str, bytes: &[u8], count: usize) -> Result<Vec<[f32; N]>, String> {
    if bytes.len() != count * N * 4 {
        return Err(format!(
            "{name}: {count} vertices need {} bytes, got {}",
            count * N * 4,
            bytes.len()
        ));
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .as_chunks::<N>()
        .0
        .iter()
        .map(|entry| {
            let mut out = [0.0f32; N];
            for (slot, word) in out.iter_mut().zip(entry) {
                let value = f32::from_le_bytes(*word);
                *slot = if value.is_finite() { value } else { 0.0 };
            }
            out
        })
        .collect())
}

impl Mesh {
    /// The mesh `parts` describe, or why it is not one: a byte count that is not a whole
    /// number of entries per vertex, an index past the last vertex, a ragged triangle list, a
    /// mesh too big for the GPU target to hold.
    pub fn from_parts(parts: MeshParts) -> Result<Mesh, String> {
        if parts.positions.is_empty() || !parts.positions.len().is_multiple_of(12) {
            return Err(format!(
                "positions: need a whole number of 12-byte vertices, got {} bytes",
                parts.positions.len()
            ));
        }
        let count = parts.positions.len() / 12;
        if count > MAX_VERTICES {
            return Err(format!("{count} vertices is more than {MAX_VERTICES}"));
        }
        let positions = floats::<3>("positions", &parts.positions, count)?;
        let normals = if parts.normals.is_empty() {
            vec![[0.0, 0.0, 1.0]; count]
        } else {
            floats::<3>("normals", &parts.normals, count)?
        };
        let uvs = if parts.uvs.is_empty() {
            vec![[0.0, 0.0]; count]
        } else {
            floats::<2>("uvs", &parts.uvs, count)?
        };
        let tangents = if parts.tangents.is_empty() {
            None
        } else {
            Some(floats::<4>("tangents", &parts.tangents, count)?)
        };
        let (joints, weights) = if parts.joints.is_empty() && parts.weights.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let width = if parts.joints_wide { 2 } else { 1 };
            if parts.joints.len() != count * 4 * width {
                return Err(format!(
                    "joints: {count} vertices need {} bytes, got {}",
                    count * 4 * width,
                    parts.joints.len()
                ));
            }
            let joints = parts
                .joints
                .chunks_exact(4 * width)
                .map(|entry| {
                    let mut out = [0u16; 4];
                    for (slot, joint) in out.iter_mut().enumerate() {
                        *joint = if parts.joints_wide {
                            u16::from_le_bytes([entry[slot * 2], entry[slot * 2 + 1]])
                        } else {
                            entry[slot] as u16
                        };
                    }
                    out
                })
                .collect();
            let mut weights = floats::<4>("weights", &parts.weights, count)?;
            for weight in &mut weights {
                let total: f32 = weight.iter().map(|share| share.max(0.0)).sum();
                for share in weight.iter_mut() {
                    *share = if total > 0.0 {
                        share.max(0.0) / total
                    } else {
                        0.0
                    };
                }
            }
            (joints, weights)
        };
        let indices: Vec<u32> = if parts.indices.is_empty() {
            if !count.is_multiple_of(3) {
                return Err(format!(
                    "indices: none given, so the {count} vertices must be whole triangles"
                ));
            }
            (0..count as u32).collect()
        } else {
            let width = if parts.indices_wide { 4 } else { 2 };
            if !parts.indices.len().is_multiple_of(width * 3) {
                return Err(format!(
                    "indices: need whole triangles of {width}-byte indices, got {} bytes",
                    parts.indices.len()
                ));
            }
            parts
                .indices
                .chunks_exact(width)
                .map(|entry| {
                    if parts.indices_wide {
                        u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]])
                    } else {
                        u16::from_le_bytes([entry[0], entry[1]]) as u32
                    }
                })
                .collect()
        };
        if indices.len() > MAX_INDICES {
            return Err(format!(
                "{} indices is more than {MAX_INDICES}",
                indices.len()
            ));
        }
        if let Some(bad) = indices.iter().find(|&&index| index as usize >= count) {
            return Err(format!("indices: {bad} names no vertex of {count}"));
        }
        let morph_bytes = parts.morphs.len() * count * 24;
        if morph_bytes > MAX_MORPH_BYTES {
            return Err(format!(
                "{} morph targets of {count} vertices need {morph_bytes} bytes, more than {MAX_MORPH_BYTES}",
                parts.morphs.len()
            ));
        }
        let mut morphs = Vec::with_capacity(parts.morphs.len());
        for (target, (morph_positions, morph_normals)) in parts.morphs.iter().enumerate() {
            let positions =
                floats::<3>(&format!("morph {target} positions"), morph_positions, count)?;
            let normals = if morph_normals.is_empty() {
                vec![[0.0; 3]; count]
            } else {
                floats::<3>(&format!("morph {target} normals"), morph_normals, count)?
            };
            // only the vertices it moves
            let mut morph = Morph {
                vertices: Vec::new(),
                positions: Vec::new(),
                normals: Vec::new(),
            };
            for (vertex, (offset, turn)) in positions.into_iter().zip(normals).enumerate() {
                if offset != [0.0; 3] || turn != [0.0; 3] {
                    morph.vertices.push(vertex as u32);
                    morph.positions.push(offset);
                    morph.normals.push(turn);
                }
            }
            morphs.push(morph);
        }
        let bounds = MeshBounds::of(&positions, &joints, &weights, &morphs);
        let curvature = vertex_curvature(&positions, &normals, &indices);
        Ok(Mesh {
            positions,
            normals,
            uvs,
            tangents,
            joints,
            weights,
            indices,
            morphs,
            curvature,
            bounds,
        })
    }
}

/// How a surface is shaded and blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Diffuse light scattered under the surface (ADR-794): read from [`skin_table`] by the
    /// light's angle and the mesh's curvature, so the terminator is a soft band, red reaching
    /// furthest, as wide as the surface is sharply curved.
    Skin,
    /// Plain diffuse.
    Standard,
    /// Standard, with texels under the material's `cutoff` alpha not drawn (lashes, brows).
    Cutout,
    /// Standard, blended over what is behind it by its alpha, after every other surface and
    /// without writing depth (hair cards).
    Blend,
    /// Hair: lit along its strands (the direction its mesh's bitangent runs, which a hair card's
    /// UV v follows from tip to root), with two highlights across them, a sharp white one
    /// shifted toward the root and a softer one tinted by the hair toward the tip
    /// (Kajiya-Kay, as Scheuermann shifts it), over a diffuse that light partly passes
    /// through. Its alpha is coverage: past half its `cutoff` a texel covers part of the
    /// pixel, all of it at one and a half `cutoff`s — that many of the GPU target's 4 samples
    /// (its sample mask) or the CPU painter's 2×2 sub-samples, each a step of
    /// `HAIR_COVERAGE_STEPS` — so fine strands thin out instead of stair-stepping; it is opaque
    /// where it covers, writing depth, drawn with the solid surfaces.
    Hair,
    /// Hair as strands: a mesh of ribbons, two vertices a point along each strand (`uv`: side
    /// −1 or 1, then 1 at the root … 0 at the tip), its normal the strand's direction. After
    /// skinning each ribbon turns to face the camera, as wide as `strand_width` (root to tip)
    /// but never under a pixel: a strand thinner than that is drawn a pixel wide covering that
    /// share of it, times its texture's coverage as `:hair`'s. Shaded as `:hair`, along its own
    /// direction, across a cylinder's normal ([`strand_vertex`]).
    Strands,
}

/// A draw's material: the fixed shading model's parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub surface: Surface,
    /// The base colour texture (a `gui-texture` handle; sRGB), multiplied by `color`.
    pub base: Option<u32>,
    /// A tangent-space normal map (a `gui-texture` handle; linear), used when the mesh has
    /// tangents.
    pub normal: Option<u32>,
    /// An ambient-occlusion map (a `gui-texture` handle; linear, its red channel, glTF's
    /// convention) multiplying the ambient light only (ADR-793).
    pub occlusion: Option<u32>,
    /// A detail tile (a `gui-texture` handle; linear, ADR-797), read `detail_scale` times
    /// across the UV square: red and green a tangent-space normal's x and y about
    /// [`DETAIL_FLAT`], laid over the surface's own ([`detail_normal`]) where the mesh has
    /// tangents; blue the cavity, 1 on the open surface falling to 0 in a pore, which damps
    /// the specular and the rim and nothing else ([`detail_specular`]). Not read by a `:hair`
    /// or `:strands` surface ([`Material::detail_texture`]).
    pub detail: Option<u32>,
    /// How many times the detail tile repeats across the UV square, along u and along v.
    pub detail_scale: [f32; 2],
    /// What the detail normal's slope is multiplied by: 0 none, 1 as baked.
    pub detail_strength: f32,
    /// How much of the cavity's damping the specular takes: 0 none, 1 all of it.
    pub detail_cavity: f32,
    /// Linear RGBA factor on the base colour.
    pub color: [f32; 4],
    /// 0 (mirror) … 1 (chalk).
    pub roughness: f32,
    /// The specular reflection's strength: its reflectance at normal incidence is
    /// `specular × SPECULAR_TO_F0`, rising toward 1 at grazing angles (Schlick, ADR-793).
    pub specular: f32,
    /// `[rough-a rough-b mix]`: two specular lobes, the material's roughness times `rough-a`
    /// weighted `mix` and times `rough-b` weighted `1 − mix`. None: [`SKIN_LOBES`] for skin,
    /// one lobe for any other surface ([`Material::lobes`]).
    pub lobes: Option<[f32; 3]>,
    /// How far light travels under a `:skin` surface, as a multiple of human skin's on a mesh
    /// measured in metres (ADR-794): it scales the mesh's curvature before the skin table is
    /// read. 0 is plain Lambert; a mesh in centimetres is 0.01. None: [`DEFAULT_SCATTER`].
    pub scatter: Option<f32>,
    /// The fresnel rim's strength, lit by the sky.
    pub rim: f32,
    /// The alpha under which a cutout texel is not drawn.
    pub cutoff: f32,
    /// Draw back faces too (their normals turned to face the viewer).
    pub double_sided: bool,
    /// Whether it casts a shadow from the scene's shadowed lights (ADR-788). A `:cutout` or
    /// `:blend` surface casts only where its alpha reaches `cutoff`.
    pub cast_shadow: bool,
    /// A `:strands` surface's width at the root and at the tip, metres ([`DEFAULT_STRAND_WIDTH`]).
    pub strand_width: [f32; 2],
}

impl Default for Material {
    fn default() -> Self {
        Material {
            surface: Surface::Standard,
            base: None,
            normal: None,
            occlusion: None,
            detail: None,
            detail_scale: [1.0, 1.0],
            detail_strength: DEFAULT_DETAIL_STRENGTH,
            detail_cavity: DEFAULT_DETAIL_CAVITY,
            color: [1.0, 1.0, 1.0, 1.0],
            roughness: 0.6,
            specular: 0.5,
            lobes: None,
            scatter: None,
            rim: 0.0,
            cutoff: 0.5,
            double_sided: false,
            cast_shadow: true,
            strand_width: DEFAULT_STRAND_WIDTH,
        }
    }
}

impl Material {
    /// Its specular lobes, `[rough-a rough-b mix]`: the ones it names, else skin's two
    /// ([`SKIN_LOBES`]) for a `:skin` surface and one for any other.
    pub fn lobes(&self) -> [f32; 3] {
        self.lobes.unwrap_or(if self.surface == Surface::Skin {
            SKIN_LOBES
        } else {
            [1.0, 1.0, 1.0]
        })
    }

    /// Its reflectance at normal incidence.
    pub fn f0(&self) -> f32 {
        (self.specular * SPECULAR_TO_F0).clamp(0.0, 1.0)
    }

    /// What a mesh's curvature (1 over its radius in the mesh's units) is multiplied by to read
    /// the skin table, whose curvatures are per millimetre: its `:scatter` over the millimetres
    /// in a unit. Both painters multiply by this one number (the GPU's rides in `DrawU`).
    pub fn scatter_scale(&self) -> f32 {
        self.scatter
            .unwrap_or(DEFAULT_SCATTER)
            .clamp(0.0, MAX_SCATTER)
            / SKIN_MILLIMETRES_PER_UNIT
    }

    /// The detail tile it is drawn with (ADR-797): its `:detail`, but none for a `:hair` or
    /// `:strands` surface, whose tangents are its strands and whose light is not this model's.
    /// Both painters ask here, so neither reads a tile the other does not.
    pub fn detail_texture(&self) -> Option<u32> {
        match self.surface {
            Surface::Hair | Surface::Strands => None,
            Surface::Skin | Surface::Standard | Surface::Cutout | Surface::Blend => self.detail,
        }
    }
}

/// What kind of light a [`Light`] is (ADR-788).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// From infinitely far, the same everywhere: the sun, the sky through a window.
    Directional,
    /// From a point, every way, falling off with the square of the distance: a bare bulb.
    Point,
    /// A point light held to a cone: a lamp with a shade, a torch.
    Spot,
}

/// A light's shadow map, as its `:shadow` asked for it (ADR-788).
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    /// The map's side in texels (each cube face's, for a point light).
    pub resolution: u32,
    /// How far, in texels, a receiver is moved toward the light (times one plus its slope to
    /// the light) before it is compared: against acne.
    pub bias: f32,
    /// How far, in texels, a receiver is moved along its normal before it is compared: against
    /// acne at grazing light, without the shadow leaving its caster.
    pub normal_bias: f32,
    /// A directional light's map covers a sphere of this radius about `center` (metres); none
    /// fits it to the scene's bounds every frame.
    pub extent: Option<f32>,
    pub center: Option<[f32; 3]>,
    /// A spot or point light's near plane (metres).
    pub near: f32,
    /// How soft the shadow's edge is, in metres at the receiver: the radius its filter reads
    /// over (ADR-788), or with the light's `size`/`angle` the most its penumbra grows to
    /// (ADR-793; `DEFAULT_SOFTNESS` texels with a size and no softness). 0 without a size keeps
    /// the tight 3×3 filter: a hard edge as sharp as the map allows.
    pub softness: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow {
            resolution: DEFAULT_SHADOW_SIDE,
            bias: DEFAULT_SHADOW_BIAS,
            normal_bias: DEFAULT_SHADOW_NORMAL_BIAS,
            extent: None,
            center: None,
            near: DEFAULT_SHADOW_NEAR,
            softness: 0.0,
        }
    }
}

/// A scene's ambient occlusion: how much of the ambient light (`sky`, `ground`) reaches a
/// surface past what is round it, read from the camera's depth of the scene's shadow-casting
/// draws (screen-space: folds, creases, corners and contact, deepening as the shapes move).
/// `radius` is how far round a point it looks, in metres; `strength` how dark a fully enclosed
/// point gets, 0 … 1. Lights are not touched: they have their shadows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientOcclusion {
    pub radius: f32,
    pub strength: f32,
}

/// A light (ADR-786, ADR-788). `direction` is the way its light TRAVELS (from the light into
/// the scene) for a directional or spot light; `position` is where a point or spot light is;
/// `color` is linear, intensity included — for a point or spot light the light arriving 1 m
/// away, falling off with the inverse square of the distance, windowed to nothing at `range`
/// when it is above 0. A spot's `cone` is its inner and outer half-angle (radians): full
/// inside the inner, nothing past the outer, a smoothstep between.
/// `size` is how big the light is, for a soft shadow's penumbra (ADR-793): a point or spot
/// light's diameter in metres, a directional light's apparent diameter in radians (the sun's
/// is 0.0093); 0, a point, softens only by its shadow's fixed `softness`.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    pub direction: [f32; 3],
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub range: f32,
    pub cone: [f32; 2],
    pub size: f32,
    pub shadow: Option<Shadow>,
}

impl Light {
    pub fn directional(direction: [f32; 3], color: [f32; 3]) -> Light {
        Light {
            kind: LightKind::Directional,
            direction,
            position: [0.0; 3],
            color,
            range: 0.0,
            cone: [0.0; 2],
            size: 0.0,
            shadow: None,
        }
    }

    pub fn point(position: [f32; 3], color: [f32; 3], range: f32) -> Light {
        Light {
            kind: LightKind::Point,
            position,
            range,
            ..Light::directional([0.0, -1.0, 0.0], color)
        }
    }

    pub fn spot(
        position: [f32; 3],
        direction: [f32; 3],
        color: [f32; 3],
        range: f32,
        cone: [f32; 2],
    ) -> Light {
        Light {
            kind: LightKind::Spot,
            position,
            range,
            cone,
            ..Light::directional(direction, color)
        }
    }

    /// How many shadow maps it renders: none unshadowed, six (a cube) for a point light.
    pub fn shadow_views(&self) -> usize {
        match (&self.shadow, self.kind) {
            (None, _) => 0,
            (Some(_), LightKind::Point) => 6,
            (Some(_), _) => 1,
        }
    }
}

/// One mesh drawn in a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub mesh: u32,
    /// Column-major model matrix (glTF's layout).
    pub model: [f32; 16],
    /// Which of the scene's palettes skins it; `None` draws it rigid.
    pub palette: Option<usize>,
    /// `(target, weight)` pairs, heaviest first: every one is blended (no cap; a face blends a
    /// smile, a blink and speech at once).
    pub morphs: Vec<(u32, f32)>,
    pub material: Material,
}

/// A `[:scene …]` op: meshes drawn into the pixel rect `x y w h` of the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Column-major view-projection matrix, clip depth 0 (near) … 1 (far).
    pub view_proj: [f32; 16],
    /// The camera's position, for the specular and the rim.
    pub eye: [f32; 3],
    pub lights: Vec<Light>,
    /// The ambient light from above and from below (linear).
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    /// The ambient light as nine L2 spherical-harmonic coefficients of the light arriving from
    /// every way (linear RGB each), used in place of `sky`/`ground` when given (ADR-793,
    /// [`sh_irradiance`] for the convention).
    pub ambient_sh: Option<[[f32; 3]; 9]>,
    /// The ambient occlusion, if any (`:ambient-occlusion`).
    pub ambient_occlusion: Option<AmbientOcclusion>,
    /// Scales the light before tone mapping.
    pub exposure: f32,
    /// What the rect shows where no mesh is: straight RGBA 0 … 1, transparent by default.
    pub background: [f32; 4],
    /// Joint matrices (column-major, 16 floats each), one list per skeleton.
    pub palettes: Vec<Arc<[f32]>>,
    pub draws: Vec<Draw>,
}

impl Scene {
    /// The picture's size in whole pixels.
    pub fn pixel_size(&self) -> (u32, u32) {
        let side = |extent: f32| {
            if extent.is_finite() {
                (extent.round().max(1.0) as u32).min(MAX_SCENE_SIDE)
            } else {
                1
            }
        };
        (side(self.w), side(self.h))
    }

    /// The draws in the order they are drawn: every non-blended one in the scene's order,
    /// then the blended ones in theirs.
    pub fn draw_order(&self) -> impl Iterator<Item = &Draw> {
        let solid = self
            .draws
            .iter()
            .filter(|draw| draw.material.surface != Surface::Blend);
        let blended = self
            .draws
            .iter()
            .filter(|draw| draw.material.surface == Surface::Blend);
        solid.chain(blended)
    }

    /// The palette `draw` is skinned by and how many joints it holds — `None` for a rigid draw
    /// (no palette named, or one holding no joint). Both painters skin only by joints below
    /// that count.
    pub fn palette_of(&self, draw: &Draw) -> Option<&[f32]> {
        let palette = draw.palette.and_then(|index| self.palettes.get(index))?;
        (palette.len() >= 16).then_some(&palette[..])
    }
}

// --- the shading model (mirrored in `gpu/shaders.rs`, MESH_SRC) -------------------------------

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

/// `vector` made unit length, or +z when it has none — the guard the shader mirrors
/// (`safe_normalize`).
fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt();
    if length > 1e-12 {
        [vector[0] / length, vector[1] / length, vector[2] / length]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

/// An sRGB-encoded channel (0 … 1) in linear light.
pub fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear channel (0 … 1) sRGB-encoded.
pub fn linear_to_srgb(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

/// The specular exponent for a roughness.
pub fn shininess(roughness: f32) -> f32 {
    let clamped = roughness.clamp(0.04, 1.0);
    (2.0 / (clamped * clamped * clamped * clamped) - 2.0).clamp(1.0, 4096.0)
}

/// How much of a point or spot light's `:color` (the light 1 m away) arrives `distance` metres
/// away: the inverse square, held below a centimetre, times the window that takes it smoothly
/// to nothing at `range` — `clamp(1 − (d/r)⁴)²`, Frostbite's and UE4's — when `range` is above
/// 0. The shader's `distance_falloff` is the same.
pub fn distance_falloff(distance: f32, range: f32) -> f32 {
    let inverse_square = 1.0 / (distance * distance).max(NEAR_LIGHT * NEAR_LIGHT);
    if range > 0.0 {
        let ratio = distance / range;
        let window = (1.0 - ratio * ratio * ratio * ratio).clamp(0.0, 1.0);
        inverse_square * window * window
    } else {
        inverse_square
    }
}

/// How much of a spot light leaves it at an angle whose cosine is `cos_angle` from its axis:
/// all inside the inner half-angle, none past the outer, a smoothstep between (`cone` is
/// `[inner outer]` in radians). The shader's `cone_falloff` is the same.
pub fn cone_falloff(cos_angle: f32, cone: [f32; 2]) -> f32 {
    let (cos_inner, cos_outer) = (cone[0].cos(), cone[1].cos());
    let amount = ((cos_angle - cos_outer) / (cos_inner - cos_outer).max(1e-4)).clamp(0.0, 1.0);
    amount * amount * (3.0 - 2.0 * amount)
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

/// The unit direction from `world` toward `light`, and how much of the light's colour arrives
/// there (before any shadow): all of a directional light's; a point light's by
/// [`distance_falloff`]; a spot's by that and [`cone_falloff`].
pub fn light_arrival(light: &Light, world: [f32; 3]) -> ([f32; 3], f32) {
    match light.kind {
        LightKind::Directional => (
            normalize([
                -light.direction[0],
                -light.direction[1],
                -light.direction[2],
            ]),
            1.0,
        ),
        LightKind::Point | LightKind::Spot => {
            let toward = subtract(light.position, world);
            let distance = dot(toward, toward).sqrt();
            let to_light = normalize(toward);
            let mut amount = distance_falloff(distance, light.range);
            if light.kind == LightKind::Spot {
                let axis = normalize(light.direction);
                let cos_angle = -dot(to_light, axis);
                amount *= cone_falloff(cos_angle, light.cone);
            }
            (to_light, amount)
        }
    }
}

/// The irradiance arriving at a surface whose unit normal is `normal` from an environment
/// whose radiance has the L2 spherical-harmonic coefficients `coefficients`, divided by π —
/// what a white diffuse surface reflects (ADR-793). The coefficients are the radiance
/// projected onto the real SH basis evaluated on world directions as given, in the order
/// L00, L1−1, L10, L11, L2−2, L2−1, L20, L21, L22, with Y00 = 0.282095, Y1−1 = 0.488603·y,
/// Y10 = 0.488603·z, Y11 = 0.488603·x, Y2−2 = 1.092548·xy, Y2−1 = 1.092548·yz, Y20 =
/// 0.315392·(3z² − 1), Y21 = 1.092548·xz, Y22 = 0.546274·(x² − y²) (three.js's
/// `SphericalHarmonics3` order and normalisation); the irradiance is Ramamoorthi and
/// Hanrahan's cosine-convolved quadratic form (c1 0.429043, c2 0.511664, c3 0.743125,
/// c4 0.886227, c5 0.247708), held at 0. A uniform radiance `L` is `L00 = 2√π·L ≈ 3.5449·L`,
/// every other coefficient 0, and lights as `:sky L :ground L` does. The shader's
/// `sh_irradiance` is the same.
pub fn sh_irradiance(coefficients: &[[f32; 3]; 9], normal: [f32; 3]) -> [f32; 3] {
    const C1: f32 = 0.429_043;
    const C2: f32 = 0.511_664;
    const C3: f32 = 0.743_125;
    const C4: f32 = 0.886_227;
    const C5: f32 = 0.247_708;
    let [x, y, z] = normal;
    let weights = [
        C4,
        2.0 * C2 * y,
        2.0 * C2 * z,
        2.0 * C2 * x,
        2.0 * C1 * x * y,
        2.0 * C1 * y * z,
        C3 * z * z - C5,
        2.0 * C1 * x * z,
        C1 * (x * x - y * y),
    ];
    [0, 1, 2].map(|channel| {
        let irradiance: f32 = weights
            .iter()
            .zip(coefficients)
            .map(|(weight, coefficient)| weight * coefficient[channel])
            .sum();
        irradiance.max(0.0) / std::f32::consts::PI
    })
}

/// The ambient light a surface facing `normal` gets, before its colour and occlusion: the
/// scene's `ambient_sh` irradiance when it has one, else `ground` blended to `sky` by how far
/// the normal points up.
pub fn ambient(scene: &Scene, normal: [f32; 3]) -> [f32; 3] {
    match &scene.ambient_sh {
        Some(coefficients) => sh_irradiance(coefficients, normal),
        None => {
            let up = normal[1] * 0.5 + 0.5;
            [0, 1, 2].map(|channel| {
                scene.ground[channel] + (scene.sky[channel] - scene.ground[channel]) * up
            })
        }
    }
}

/// `amount` taken smoothly from 0 at `low` to 1 at `high` (Hermite): WGSL's `smoothstep`.
pub fn smoothstep(low: f32, high: f32, amount: f32) -> f32 {
    let along = ((amount - low) / (high - low)).clamp(0.0, 1.0);
    along * along * (3.0 - 2.0 * along)
}

/// Schlick's Fresnel: the share of light a surface whose reflectance at normal incidence is
/// `f0` reflects when the view and the half vector meet at the cosine `view_dot_half`. Its
/// grazing reflectance is `min(50·f0, 1)` rather than 1 (UE4's): an F0 under 2 % is no real
/// material's, so `:specular 0` reflects nothing at any angle. The shader's `fresnel` is the
/// same.
pub fn fresnel(f0: f32, view_dot_half: f32) -> f32 {
    let grazing = (1.0 - view_dot_half.clamp(0.0, 1.0)).powi(5);
    f0 + ((50.0 * f0).min(1.0) - f0) * grazing
}

/// The specular reflection of `material`'s lobes for a normal and half vector meeting at the
/// cosine `normal_dot_half`, before Fresnel and the light's angle: each lobe Blinn-Phong
/// normalised to `(n + 8) / 8` (the `1/π` of the normalised BRDF folded into the light, as
/// the diffuse term's is: a light's `:color` is π times its radiance), weighted by its mix.
pub fn specular_lobes(material: &Material, normal_dot_half: f32) -> f32 {
    let [rough_a, rough_b, mix] = material.lobes();
    let lobe = |roughness: f32| {
        let exponent = shininess(roughness);
        (exponent + 8.0) / 8.0 * normal_dot_half.max(0.0).powf(exponent)
    };
    let first = lobe(material.roughness * rough_a);
    if mix >= 1.0 {
        first
    } else {
        mix * first + (1.0 - mix) * lobe(material.roughness * rough_b)
    }
}

/// The curvature, per millimetre, that row `row` of the skin table holds: `(row / last)²` of
/// [`SKIN_MAX_CURVATURE`], so [`skin_diffuse`] finds a curvature's row by a square root.
pub fn skin_table_curvature(row: usize) -> f32 {
    let along = row as f32 / (SKIN_TABLE_HEIGHT - 1) as f32;
    along * along * SKIN_MAX_CURVATURE
}

/// The skin table (ADR-794; Penner's pre-integrated skin shading): the diffuse light a
/// `:skin` surface gives back, red, green and blue, LINEAR, by the cosine between its normal
/// and the light ([`SKIN_TABLE_WIDTH`] columns, −1 … 1) and its curvature
/// ([`SKIN_TABLE_HEIGHT`] rows, [`skin_table_curvature`]), row by row. Built once, in Rust,
/// and the same numbers for both painters: the CPU's reads this slice, the GPU target uploads
/// it as a texture.
///
/// Each texel is Lambert's light blurred round a ring of the surface's radius r by the skin's
/// diffusion profile: `D(θ) = ∫ max(cos(θ + x), 0) · R(2r·sin(x/2)) dx / ∫ R(2r·sin(x/2)) dx`,
/// x from −π to π, `2r·sin(x/2)` the straight distance under the skin between two points of
/// the ring an angle x apart. `R` is [`SKIN_PROFILE`], each Gaussian of variance v taken as
/// `exp(−s²/2v) / √(2πv)`: the profile is a blur over the surface's two directions, and the
/// ring sees one, so each Gaussian is the 2D one of GPU Gems 3 with the direction along the
/// ring's axis integrated out, which leaves its weight what the table says. A flat surface
/// (row 0) is Lambert itself; on a curved one light is only moved along the ring, never added.
pub fn skin_table() -> &'static [[f32; 3]] {
    static TABLE: std::sync::OnceLock<Vec<[f32; 3]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(build_skin_table)
}

fn build_skin_table() -> Vec<[f32; 3]> {
    let mut table = Vec::with_capacity(SKIN_TABLE_WIDTH * SKIN_TABLE_HEIGHT);
    for row in 0..SKIN_TABLE_HEIGHT {
        let curvature = f64::from(skin_table_curvature(row));
        // each Gaussian round the ring: the angles it is read at with their weights, and the
        // weights' sum. Read out to six of its deviations (or the whole ring), so the
        // narrowest is integrated as finely as the widest.
        let rings: Vec<(Vec<(f64, f64)>, f64)> = SKIN_PROFILE
            .iter()
            .map(|gaussian| {
                if row == 0 {
                    return (Vec::new(), 0.0);
                }
                let variance = gaussian[0];
                let radius = 1.0 / curvature;
                let reach = 3.0 * variance.sqrt() / radius;
                let furthest = if reach >= 1.0 {
                    std::f64::consts::PI
                } else {
                    2.0 * reach.asin()
                };
                let step = 2.0 * furthest / SKIN_INTEGRATION_STEPS as f64;
                let samples: Vec<(f64, f64)> = (0..SKIN_INTEGRATION_STEPS)
                    .map(|index| {
                        let offset = -furthest + (index as f64 + 0.5) * step;
                        let distance = 2.0 * radius * (offset * 0.5).sin();
                        let weight = (-distance * distance / (2.0 * variance)).exp() * step
                            / (std::f64::consts::TAU * variance).sqrt();
                        (offset, weight)
                    })
                    .collect();
                let total = samples.iter().map(|(_, weight)| weight).sum();
                (samples, total)
            })
            .collect();
        for column in 0..SKIN_TABLE_WIDTH {
            let cosine = column as f64 / (SKIN_TABLE_WIDTH - 1) as f64 * 2.0 - 1.0;
            if row == 0 {
                table.push([cosine.max(0.0) as f32; 3]);
                continue;
            }
            let angle = cosine.clamp(-1.0, 1.0).acos();
            let mut light = [0.0f64; 3];
            let mut weight = [0.0f64; 3];
            for (gaussian, (samples, total)) in SKIN_PROFILE.iter().zip(&rings) {
                let lit: f64 = samples
                    .iter()
                    .map(|(offset, share)| (angle + offset).cos().max(0.0) * share)
                    .sum();
                for channel in 0..3 {
                    light[channel] += gaussian[channel + 1] * lit;
                    weight[channel] += gaussian[channel + 1] * total;
                }
            }
            table.push([0, 1, 2].map(|channel| (light[channel] / weight[channel]) as f32));
        }
    }
    table
}

/// The diffuse light, red, green and blue, a `:skin` surface gives back where its normal and
/// the light meet at the cosine `facing` and it curves `curvature` per millimetre: `table`
/// ([`skin_table`]) read between its four nearest texels — the columns evenly from −1 to 1,
/// the rows by the square root of the curvature over [`SKIN_MAX_CURVATURE`]. The shader's
/// `skin_diffuse` is the same arithmetic on the same texels (`textureLoad`, never a filtering
/// sampler, whose weights are 8 bits).
pub fn skin_diffuse(table: &[[f32; 3]], facing: f32, curvature: f32) -> [f32; 3] {
    let across = (facing * 0.5 + 0.5).clamp(0.0, 1.0) * (SKIN_TABLE_WIDTH - 1) as f32;
    let down = (curvature.clamp(0.0, SKIN_MAX_CURVATURE) / SKIN_MAX_CURVATURE).sqrt()
        * (SKIN_TABLE_HEIGHT - 1) as f32;
    let column = (across.floor() as usize).min(SKIN_TABLE_WIDTH - 2);
    let row = (down.floor() as usize).min(SKIN_TABLE_HEIGHT - 2);
    let across_share = across - column as f32;
    let down_share = down - row as f32;
    let texel = |column: usize, row: usize| table[row * SKIN_TABLE_WIDTH + column];
    let corners = [
        texel(column, row),
        texel(column + 1, row),
        texel(column, row + 1),
        texel(column + 1, row + 1),
    ];
    [0, 1, 2].map(|channel| {
        let upper =
            corners[0][channel] + (corners[1][channel] - corners[0][channel]) * across_share;
        let lower =
            corners[2][channel] + (corners[3][channel] - corners[2][channel]) * across_share;
        upper + (lower - upper) * down_share
    })
}

/// A detail texel's tilt (ADR-797): its normal's x and y in the tangent frame, from the red
/// and green channels about [`DETAIL_FLAT`]. Exactly `[0, 0]` for a flat texel (the byte 128).
pub fn detail_tilt(texel: [f32; 4]) -> [f32; 2] {
    [
        (texel[0] - DETAIL_FLAT) * 2.0,
        (texel[1] - DETAIL_FLAT) * 2.0,
    ]
}

/// The tangent-space normal `surface` (the normal map's, or `[0, 0, 1]` without one) with a
/// detail normal of `tilt` laid over it, its slope scaled by `strength` (ADR-797); not unit
/// length. The partial-derivative blend: each normal is a height field's, and the two heights
/// add, so their slopes do — `surface.xy / surface.z + strength × tilt / z`, the detail's z
/// reconstructed from its tilt — here multiplied through by both z's, so nothing is divided.
/// A detail that does not tilt gives `surface` back exactly, a flat surface the detail itself,
/// and the detail's angle θ becomes `atan(strength × tan θ)`.
pub fn detail_normal(surface: [f32; 3], tilt: [f32; 2], strength: f32) -> [f32; 3] {
    let upright = (1.0 - tilt[0] * tilt[0] - tilt[1] * tilt[1])
        .max(0.0)
        .sqrt();
    [
        surface[0] * upright + tilt[0] * strength * surface[2],
        surface[1] * upright + tilt[1] * strength * surface[2],
        surface[2] * upright,
    ]
}

/// How much of the specular (and the rim) a detail texel's `cavity` (its blue channel: 1 the
/// open surface, 0 the bottom of a pore) leaves, a material's `:detail-cavity` being `amount`
/// (ADR-797). 1 for an open texel or an amount of 0.
pub fn detail_specular(cavity: f32, amount: f32) -> f32 {
    (1.0 - amount * (1.0 - cavity)).clamp(0.0, 1.0)
}

/// The light leaving a surface point toward the eye, linear and before exposure: `base` its
/// linear colour, `normal` its unit normal facing the viewer's side, `view` the unit
/// direction from it to the eye, `world` where it is, `occlusion` how much of the ambient
/// reaches it (its occlusion map's times the scene's [`ambient_occlusion`], 1 for all of it),
/// `visibility` how much of each light (by index) its shadow lets through — 1 for a light it
/// holds no entry for — `strand` a `:hair` surface's unit strand direction (unused by the
/// others), `curvature` how sharply the mesh curves there, 1 over its radius in the mesh's
/// units ([`Mesh::curvature`]; read by a `:skin` surface only), and `specular_kept` how much
/// of the specular and the rim its detail tile's cavity leaves there ([`detail_specular`]; 1
/// without one, and not read by a `:hair` or `:strands` surface's highlights).
///
/// A `:skin` surface's diffuse light is [`skin_diffuse`]'s in place of Lambert's (ADR-794).
/// A shadow multiplies the Lambert part of it and fades out of the rest — the light the
/// scattering moved — toward the terminator, where a closed body always shadows itself and the
/// light there came under the skin from the lit side (ADR-793).
///
/// A light's `:color` is the irradiance it brings, π times its radiance, so the diffuse term
/// is the albedo itself (Lambert's `albedo / π` times π) and the specular's normalised
/// Blinn-Phong is `(n + 8) / 8` (its `(n + 8) / 8π` times π) — the two on one scale (ADR-793).
#[allow(clippy::too_many_arguments)]
pub fn shade(
    scene: &Scene,
    material: &Material,
    base: [f32; 3],
    normal: [f32; 3],
    view: [f32; 3],
    world: [f32; 3],
    occlusion: f32,
    visibility: &[f32],
    strand: [f32; 3],
    curvature: f32,
    specular_kept: f32,
) -> [f32; 3] {
    let ambient = ambient(scene, normal);
    let skin = (material.surface == Surface::Skin)
        .then(|| (skin_table(), curvature * material.scatter_scale()));
    let mut out = [0, 1, 2].map(|channel| base[channel] * ambient[channel] * occlusion);
    let f0 = material.f0();
    // a `:hair` surface's highlights: as sharp as its roughness asks
    let exponent = shininess(material.roughness);
    for (index, light) in scene.lights.iter().take(MAX_LIGHTS).enumerate() {
        let (to_light, arriving) = light_arrival(light, world);
        let seen = visibility.get(index).copied().unwrap_or(1.0);
        let facing = dot(normal, to_light);
        // the light past the terminator is light under the skin, which the body's own shadow
        // there would cut off in a hard line: the shadow fades out of what the scattering
        // moved toward the terminator (ADR-793)
        let scattered_seen = 1.0 + (seen - 1.0) * smoothstep(0.0, 0.3, facing);
        let lambert = facing.max(0.0);
        let half = normalize([
            to_light[0] + view[0],
            to_light[1] + view[1],
            to_light[2] + view[2],
        ]);
        if matches!(material.surface, Surface::Hair | Surface::Strands) {
            // across the strand, shifted along the normal: sin of its angle to the half vector,
            // sharpened; on the lit side only, the diffuse wrapped (light passes through hair)
            let highlight = |shift: f32, sharpness: f32| {
                let shifted = normalize([0, 1, 2].map(|axis| strand[axis] + normal[axis] * shift));
                let along = dot(shifted, half);
                (1.0 - along * along).max(0.0).sqrt().powf(sharpness)
            };
            let primary = highlight(HAIR_SHIFT_PRIMARY, exponent);
            let secondary = highlight(HAIR_SHIFT_SECONDARY, exponent * HAIR_SECONDARY_SHARPNESS);
            let lit_side = (facing + 0.25).clamp(0.0, 1.0);
            let diffuse = (0.25 + 0.75 * facing).clamp(0.0, 1.0);
            for channel in 0..3 {
                let shine = material.specular * lit_side * (primary + secondary * base[channel]);
                out[channel] +=
                    light.color[channel] * arriving * seen * (base[channel] * diffuse + shine);
            }
            continue;
        }
        // a pore reflects less than the open surface round it (ADR-797); the diffuse is not
        // touched
        let specular = fresnel(f0, dot(view, half))
            * specular_lobes(material, dot(normal, half))
            * lambert
            * specular_kept;
        let scattered = skin.map(|(table, curved)| skin_diffuse(table, facing, curved));
        for channel in 0..3 {
            let diffuse = match scattered {
                Some(light) => lambert * seen + (light[channel] - lambert) * scattered_seen,
                None => lambert * seen,
            };
            out[channel] +=
                light.color[channel] * arriving * (base[channel] * diffuse + specular * seen);
        }
    }
    let rim = material.rim * specular_kept * (1.0 - dot(normal, view).max(0.0)).powi(3);
    for channel in 0..3 {
        out[channel] += rim * scene.sky[channel];
    }
    out
}

/// Linear light after `exposure`, tone mapped (ACES, Narkowicz's fit) and sRGB-encoded.
pub fn tone(color: [f32; 3], exposure: f32) -> [f32; 3] {
    let map = |channel: f32| {
        let exposed = (channel * exposure).max(0.0);
        let mapped =
            (exposed * (2.51 * exposed + 0.03)) / (exposed * (2.43 * exposed + 0.59) + 0.14);
        linear_to_srgb(mapped)
    };
    [map(color[0]), map(color[1]), map(color[2])]
}

// --- shadow maps (ADR-788): planned here for both painters --------------------------------------

/// One shadow map a scene renders: the scene as a light sees it, depth only.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowView {
    /// Column-major, clip depth 0 (near) … 1 (far), as a camera's.
    pub view_proj: [f32; 16],
    /// Its side in texels.
    pub resolution: u32,
    /// The world size of one texel: 1 m in front of a perspective view (a spot's, a cube
    /// face's), anywhere in an orthographic one (a directional light's). Times a receiver's
    /// clip w (1 when orthographic) it is the texel's size there.
    pub texel: f32,
    /// The distances along its axis its depth 0 and 1 stand at (metres; a directional view's
    /// from an origin of its own, so only their difference means anything), and whether it is
    /// a perspective view, whose depth is not linear in distance: what turns a stored depth back
    /// into a distance ([`ShadowView::distance`]) for a soft shadow's penumbra (ADR-793).
    pub depth_range: [f32; 2],
    pub perspective: bool,
}

impl ShadowView {
    /// The distance along its axis a clip depth (0 … 1) stands at: linear in depth for an
    /// orthographic view, `near·far / (far − depth·(far − near))` for a perspective one.
    pub fn distance(&self, depth: f32) -> f32 {
        let [near, far] = self.depth_range;
        if self.perspective {
            near * far / (far - depth * (far - near)).max(1e-6)
        } else {
            near + depth * (far - near)
        }
    }
}

/// The shadow maps a scene renders this frame, and which belong to which light.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowPlan {
    pub views: Vec<ShadowView>,
    /// Per light, by index: its first view and how many it has — none for a light without a
    /// shadow (or one there is nothing to fit to).
    pub lights: [(u32, u32); MAX_LIGHTS],
    /// The side of the layers the GPU target keeps the views in: the largest view's.
    pub layer_side: u32,
    /// Which view is the camera's depth for the ambient occlusion — none without it (or no
    /// room left for it).
    pub occlusion: Option<u32>,
}

/// Where `draw`'s vertices can be in the world this frame — morphed, skinned and moved — from
/// its mesh's `bounds` alone: what both painters fit a shadow map to.
pub fn draw_bounds(scene: &Scene, draw: &Draw, bounds: &MeshBounds) -> Option<Bounds> {
    // what the morphs can add to a position, before skinning
    let mut grow = [[0.0f32; 3]; 2];
    for &(target, weight) in &draw.morphs {
        if let Some(offsets) = bounds.morphs.get(target as usize) {
            for axis in 0..3 {
                let (low, high) = (weight * offsets[0][axis], weight * offsets[1][axis]);
                grow[0][axis] += low.min(high);
                grow[1][axis] += low.max(high);
            }
        }
    }
    let grown = |[low, high]: Bounds| -> Bounds {
        [
            [
                low[0] + grow[0][0],
                low[1] + grow[0][1],
                low[2] + grow[0][2],
            ],
            [
                high[0] + grow[1][0],
                high[1] + grow[1][1],
                high[2] + grow[1][2],
            ],
        ]
    };
    let palette = scene.palette_of(draw);
    let mut out = None;
    let mut add = |part: Bounds, matrix: &[f32; 16]| {
        for corner in corners(grown(part)) {
            let moved = transform(matrix, corner, 1.0);
            out = extend(out, [moved[0], moved[1], moved[2]]);
        }
    };
    if let Some(rigid) = bounds.rigid {
        add(rigid, &draw.model);
    }
    for &(joint, part) in &bounds.joints {
        let start = joint as usize * 16;
        match palette {
            Some(palette) if start + 16 <= palette.len() => {
                let mut joint_matrix = [0.0f32; 16];
                joint_matrix.copy_from_slice(&palette[start..start + 16]);
                add(part, &multiply(&draw.model, &joint_matrix));
            }
            // a joint the palette does not hold leaves its vertices where the model puts them
            _ => add(part, &draw.model),
        }
    }
    out
}

/// A right and an up direction square to `forward` (a unit vector): world up projected, or
/// world +z when `forward` is nearly vertical.
fn basis(forward: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let hint = if forward[1].abs() > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let right = normalize(cross(forward, hint));
    (right, cross(right, forward))
}

/// A column-major matrix from its four rows.
fn from_rows(rows: [[f32; 4]; 4]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for (row, values) in rows.iter().enumerate() {
        for (column, value) in values.iter().enumerate() {
            out[column * 4 + row] = *value;
        }
    }
    out
}

/// A perspective view from `eye` along `forward` with a square field `tan_half` wide either
/// side of its axis, depth 0 at `near` … 1 at `far`.
fn perspective_view(
    eye: [f32; 3],
    forward: [f32; 3],
    tan_half: f32,
    near: f32,
    far: f32,
) -> [f32; 16] {
    let (right, up) = basis(forward);
    let scale = far / (far - near);
    let offset = far * near / (far - near);
    from_rows([
        [
            right[0] / tan_half,
            right[1] / tan_half,
            right[2] / tan_half,
            -dot(eye, right) / tan_half,
        ],
        [
            up[0] / tan_half,
            up[1] / tan_half,
            up[2] / tan_half,
            -dot(eye, up) / tan_half,
        ],
        [
            forward[0] * scale,
            forward[1] * scale,
            forward[2] * scale,
            -dot(eye, forward) * scale - offset,
        ],
        [forward[0], forward[1], forward[2], -dot(eye, forward)],
    ])
}

/// The axes a point light's six shadow views look along, in the order [`cube_face`] numbers
/// them.
const CUBE_FACES: [[f32; 3]; 6] = [
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, -1.0],
];

/// Which of a point light's six views sees a point `offset` from the light: the one along its
/// largest axis. The shader's `cube_face` is the same.
pub fn cube_face(offset: [f32; 3]) -> usize {
    let (across, up, deep) = (offset[0].abs(), offset[1].abs(), offset[2].abs());
    if across >= up && across >= deep {
        if offset[0] >= 0.0 { 0 } else { 1 }
    } else if up >= deep {
        if offset[1] >= 0.0 { 2 } else { 3 }
    } else if offset[2] >= 0.0 {
        4
    } else {
        5
    }
}

/// The farthest a point of `world` is from `eye`, for a light with no `:range` to reach to.
fn farthest(eye: [f32; 3], world: Option<Bounds>) -> Option<f32> {
    let bounds = world?;
    corners(bounds)
        .iter()
        .map(|corner| {
            let offset = subtract(*corner, eye);
            dot(offset, offset).sqrt()
        })
        .reduce(f32::max)
}

/// The shadow views of `light`, whose shadow is `shadow`, over a scene whose things lie in
/// `world`: none when there is nothing to fit them to.
fn light_views(light: &Light, shadow: &Shadow, world: Option<Bounds>) -> Vec<ShadowView> {
    let side = shadow.resolution.clamp(MIN_SHADOW_SIDE, MAX_SHADOW_SIDE);
    let far_of = || {
        let far = if light.range > 0.0 {
            Some(light.range)
        } else {
            farthest(light.position, world).map(|far| far * 1.01)
        }?;
        let near = shadow.near.min(far * 0.5).max(1e-4);
        Some((near, far.max(near * 2.0)))
    };
    match light.kind {
        LightKind::Directional => {
            let forward = normalize(light.direction);
            let (right, up) = basis(forward);
            let middle = |[low, high]: Bounds| [0, 1, 2].map(|axis| (low[axis] + high[axis]) * 0.5);
            let (center, radius) = match (shadow.extent, world) {
                (Some(extent), _) => (
                    shadow.center.or(world.map(middle)).unwrap_or([0.0; 3]),
                    extent,
                ),
                (None, Some(bounds)) => {
                    let diagonal = subtract(bounds[1], bounds[0]);
                    let half = (dot(diagonal, diagonal).sqrt() * 0.5).max(0.01);
                    // a quarter-octave step, so a fit following a moving figure changes its
                    // scale seldom rather than every frame
                    (middle(bounds), 2f32.powf((half.log2() * 4.0).ceil() / 4.0))
                }
                (None, None) => return Vec::new(),
            };
            let texel = 2.0 * radius / side as f32;
            // the centre on the texel grid: a fit that moves slides whole texels, so the edges
            // of the shadows it casts do not crawl
            let across = (dot(center, right) / texel).round() * texel;
            let down = (dot(center, up) / texel).round() * texel;
            let depth_center = dot(center, forward);
            // depth through the sphere, and back toward the light past everything in the
            // scene, which may cast into it
            let (mut near, mut far) = (-radius, radius);
            if let Some(bounds) = world {
                for corner in corners(bounds) {
                    let depth = dot(corner, forward) - depth_center;
                    near = near.min(depth);
                    far = far.max(depth);
                }
            }
            let pad = (far - near) * 0.01 + 0.01;
            let (near, far) = (near - pad, far + pad);
            let depth_scale = 1.0 / (far - near);
            let view_proj = from_rows([
                [
                    right[0] / radius,
                    right[1] / radius,
                    right[2] / radius,
                    -across / radius,
                ],
                [
                    up[0] / radius,
                    up[1] / radius,
                    up[2] / radius,
                    -down / radius,
                ],
                [
                    forward[0] * depth_scale,
                    forward[1] * depth_scale,
                    forward[2] * depth_scale,
                    -(depth_center + near) * depth_scale,
                ],
                [0.0, 0.0, 0.0, 1.0],
            ]);
            vec![ShadowView {
                view_proj,
                resolution: side,
                texel,
                depth_range: [near, far],
                perspective: false,
            }]
        }
        LightKind::Spot => {
            let Some((near, far)) = far_of() else {
                return Vec::new();
            };
            let tan_half = light.cone[1].tan().clamp(1e-3, 1e3);
            vec![ShadowView {
                view_proj: perspective_view(
                    light.position,
                    normalize(light.direction),
                    tan_half,
                    near,
                    far,
                ),
                resolution: side,
                texel: 2.0 * tan_half / side as f32,
                depth_range: [near, far],
                perspective: true,
            }]
        }
        LightKind::Point => {
            let Some((near, far)) = far_of() else {
                return Vec::new();
            };
            // a little wider than a quarter turn, so a receiver's filter on a face's edge
            // reads that face's own texels
            let tan_half = side as f32 / (side as f32 - 2.0 * CUBE_MARGIN);
            CUBE_FACES
                .iter()
                .map(|forward| ShadowView {
                    view_proj: perspective_view(light.position, *forward, tan_half, near, far),
                    resolution: side,
                    texel: 2.0 * tan_half / side as f32,
                    depth_range: [near, far],
                    perspective: true,
                })
                .collect()
        }
    }
}

/// The shadow maps `scene` renders this frame, its things lying in `world` (the union of its
/// draws' [`draw_bounds`]): the first [`MAX_SHADOWS`] shadowed lights' views, within
/// [`MAX_SHADOW_TEXELS`] (the spec parser refuses a scene asking for more).
pub fn shadow_plan(scene: &Scene, world: Option<Bounds>) -> ShadowPlan {
    let mut plan = ShadowPlan {
        views: Vec::new(),
        lights: [(0, 0); MAX_LIGHTS],
        layer_side: 0,
        occlusion: None,
    };
    let mut shadowed = 0;
    for (index, light) in scene.lights.iter().enumerate().take(MAX_LIGHTS) {
        let Some(shadow) = &light.shadow else {
            continue;
        };
        if shadowed == MAX_SHADOWS {
            break;
        }
        let views = light_views(light, shadow, world);
        if views.is_empty() {
            continue;
        }
        let layer_side = plan.layer_side.max(views[0].resolution);
        let layers = (plan.views.len() + views.len()) as u64;
        if layers * u64::from(layer_side) * u64::from(layer_side) > MAX_SHADOW_TEXELS {
            continue;
        }
        shadowed += 1;
        plan.lights[index] = (plan.views.len() as u32, views.len() as u32);
        plan.layer_side = layer_side;
        plan.views.extend(views);
    }
    // the ambient occlusion's view: the camera's own, square (its depth is read by where a
    // point falls in the picture, not texel for pixel), as fine as the picture up to a limit
    if scene.ambient_occlusion.is_some() && plan.views.len() < MAX_SHADOW_VIEWS {
        let (width, height) = scene.pixel_size();
        let side = width.max(height).clamp(MIN_SHADOW_SIDE, MAX_OCCLUSION_SIDE);
        let layer_side = plan.layer_side.max(side);
        let layers = (plan.views.len() + 1) as u64;
        if layers * u64::from(layer_side) * u64::from(layer_side) <= MAX_SHADOW_TEXELS {
            plan.occlusion = Some(plan.views.len() as u32);
            plan.layer_side = layer_side;
            plan.views.push(ShadowView {
                view_proj: scene.view_proj,
                resolution: side,
                texel: 0.0,
                // no penumbra is read from it: the occlusion turns its depths into distances
                // from the camera's own matrix (`view_distance`)
                depth_range: [0.0, 1.0],
                perspective: false,
            });
        }
    }
    plan
}

/// Where `point` falls in the depth map of `view`: its column and row in texels (0 … the
/// view's resolution at the map's edges, continuous: a texel's centre is at its index + 0.5)
/// and its clip depth, wherever it is — none only behind the view.
fn project_raw(view: &ShadowView, point: [f32; 3]) -> Option<([f32; 2], f32)> {
    let clip = transform(&view.view_proj, point, 1.0);
    if clip[3] <= 1e-6 || !clip[3].is_finite() {
        return None;
    }
    let side = view.resolution as f32;
    Some((
        [
            (clip[0] / clip[3] * 0.5 + 0.5) * side,
            (0.5 - clip[1] / clip[3] * 0.5) * side,
        ],
        clip[2] / clip[3],
    ))
}

/// [`project_raw`], or none when the point is outside the map or past its depth.
fn project_into(view: &ShadowView, point: [f32; 3]) -> Option<([f32; 2], f32)> {
    let (at, depth) = project_raw(view, point)?;
    let side = view.resolution as f32;
    let (span, unit) = (0.0..=side, 0.0..=1.0);
    (unit.contains(&depth) && span.contains(&at[0]) && span.contains(&at[1])).then_some((at, depth))
}

/// The depth stored at texel `(column, row)` of `view`'s map: a read off the map's left or top
/// edge reads the edge; off its right or bottom edge it reads the GPU target's layer of
/// `layer_side` texels, cleared to the far plane (1) where the view did not reach — as the
/// GPU's clamped reads of that layer do.
fn stored_depth(
    view: &ShadowView,
    layer_side: u32,
    depth_map: &[f32],
    column: i64,
    row: i64,
) -> f32 {
    let side = view.resolution as i64;
    let last = i64::from(layer_side.max(1)) - 1;
    let (column, row) = (column.clamp(0, last), row.clamp(0, last));
    if column >= side || row >= side {
        1.0
    } else {
        depth_map[(row * side + column) as usize]
    }
}

/// A bilinear percentage-closer tap: how lit a receiver at `depth` is at the continuous texel
/// position `(across, down)` (texel centres at whole numbers here), its four neighbours'
/// comparisons weighted by nearness. The shader's `bilinear_tap` is one comparison gather.
fn bilinear_tap(
    view: &ShadowView,
    layer_side: u32,
    depth_map: &[f32],
    [across, down]: [f32; 2],
    depth: f32,
) -> f32 {
    let (column, row) = (across.floor(), down.floor());
    let (fraction_x, fraction_y) = (across - column, down - row);
    let (column, row) = (column as i64, row as i64);
    let lit = |step_x: i64, step_y: i64| {
        let stored = stored_depth(view, layer_side, depth_map, column + step_x, row + step_y);
        if depth <= stored { 1.0 } else { 0.0 }
    };
    (lit(0, 0) * (1.0 - fraction_x) + lit(1, 0) * fraction_x) * (1.0 - fraction_y)
        + (lit(0, 1) * (1.0 - fraction_x) + lit(1, 1) * fraction_x) * fraction_y
}

/// How much of a light a receiver at `point` gets past the depth map of `view` (`depth_map`,
/// its `resolution`² texels, row 0 the top): a 3×3 percentage-closer filter of bilinear taps,
/// read as the weights of a 4×4 block of plain comparisons — 1 lit … 0 in shadow. A point
/// outside the map, or past its depth, is lit. Texel reads go through [`stored_depth`]. The
/// shader's `shadow_sample` is the same, a comparison gather per 2×2 of the block.
pub fn sample_shadow(
    view: &ShadowView,
    layer_side: u32,
    depth_map: &[f32],
    point: [f32; 3],
) -> f32 {
    let Some(([across, down], depth)) = project_into(view, point) else {
        return 1.0;
    };
    let (across, down) = (across - 0.5, down - 0.5);
    let (column, row) = (across.floor(), down.floor());
    let (fraction_x, fraction_y) = (across - column, down - row);
    let weights_x = [1.0 - fraction_x, 1.0, 1.0, fraction_x];
    let weights_y = [1.0 - fraction_y, 1.0, 1.0, fraction_y];
    let (column, row) = (column as i64 - 1, row as i64 - 1);
    let mut lit = 0.0;
    for (step_y, weight_y) in weights_y.iter().enumerate() {
        for (step_x, weight_x) in weights_x.iter().enumerate() {
            let stored = stored_depth(
                view,
                layer_side,
                depth_map,
                column + step_x as i64,
                row + step_y as i64,
            );
            if depth <= stored {
                lit += weight_x * weight_y;
            }
        }
    }
    lit / 9.0
}

/// Interleaved gradient noise (Jimenez 2014) at pixel `(column, row)` of the picture: 0 … 1,
/// a different value at each of neighbouring pixels, which turns a soft shadow's taps a
/// different way at each so their pattern becomes fine grain rather than banding. The
/// shader's `pixel_noise` is the same, at the same pixel.
pub fn pixel_noise(column: f32, row: f32) -> f32 {
    let inner = (0.067_110_56 * column + 0.005_837_15 * row).fract();
    (52.982_918 * inner).fract()
}

/// Tap `index` of [`SOFT_TAPS`] on a unit disc turned by `turn` radians: a Vogel (golden
/// angle) spiral, as even over the disc as a Poisson disc and needing no table to keep the
/// two painters in step. The shader's `disc_tap` is the same.
pub fn disc_tap(index: u32, turn: f32) -> [f32; 2] {
    const GOLDEN_ANGLE: f32 = 2.399_963;
    let radius = ((index as f32 + 0.5) / SOFT_TAPS as f32).sqrt();
    let angle = index as f32 * GOLDEN_ANGLE + turn;
    [radius * angle.cos(), radius * angle.sin()]
}

/// How a soft shadow is filtered at one receiver ([`soft_shadow`]).
pub struct SoftFilter {
    /// The light's size: a point or spot light's diameter in metres, a directional light's
    /// angle in radians; 0 filters over a fixed radius.
    pub size: f32,
    /// The filter's radius in texels — fixed, or with a size the most it grows to and how far
    /// the blocker search reaches.
    pub softness: f32,
    /// How far the taps are turned (radians): the pixel's noise.
    pub turn: f32,
    /// How the receiver's own surface changes depth a texel across the map, each way
    /// ([`receiver_slope`]): each tap compares against the receiver's plane where it reads.
    pub slope: [f32; 2],
}

/// How the depth of the surface through `point` whose unit normal is `normal` changes a texel
/// across the map of `view` (column, row): a receiver-plane depth bias, so a wide filter on a
/// lit surface sloping away from the light does not read its own nearer texels as a shadow.
/// Found from where two steps along the surface, and one away from the light, land in the map
/// (no screen derivatives, so the two painters agree); held to the depth change of a surface
/// four times as steep as the light's own direction (the slope bias's cap), and 0 for a surface
/// edge-on to the light. The shader's `receiver_slope` is the same.
pub fn receiver_slope(
    view: &ShadowView,
    point: [f32; 3],
    normal: [f32; 3],
    to_light: [f32; 3],
) -> [f32; 2] {
    let Some((origin, depth)) = project_raw(view, point) else {
        return [0.0; 2];
    };
    let step = view.texel * transform(&view.view_proj, point, 1.0)[3].max(1e-6);
    let along = |direction: [f32; 3]| {
        let moved = [0, 1, 2].map(|axis| point[axis] + direction[axis] * step);
        project_raw(view, moved)
            .map(|(at, moved_depth)| [at[0] - origin[0], at[1] - origin[1], moved_depth - depth])
    };
    let (tangent, bitangent) = basis(normal);
    let (Some(first), Some(second), Some(away)) = (
        along(tangent),
        along(bitangent),
        along([-to_light[0], -to_light[1], -to_light[2]]),
    ) else {
        return [0.0; 2];
    };
    let determinant = first[0] * second[1] - first[1] * second[0];
    if determinant.abs() < 1e-3 {
        return [0.0; 2];
    }
    let cap = 4.0 * away[2].abs();
    [
        ((first[2] * second[1] - second[2] * first[1]) / determinant).clamp(-cap, cap),
        ((second[2] * first[0] - first[2] * second[0]) / determinant).clamp(-cap, cap),
    ]
}

/// A soft shadow (ADR-793): how much of a light a receiver at `point` gets past the depth map
/// of `view`, filtered over a disc of [`SOFT_TAPS`] bilinear taps as `filter` says, each
/// compared against the receiver's plane where it reads.
///
/// With no size the disc's radius is `softness` texels. With a size it is the penumbra the
/// light casts there (percentage-closer soft shadows): the taps first look for blockers within
/// `softness` texels, and the penumbra's half-width is the size times the gap from the
/// blockers' mean distance to the receiver's, over the blockers' distance for a light at a
/// point — so a contact shadow stays crisp and a far one softens — measured in texels where
/// the receiver stands, held to [`MIN_SOFTNESS`] … `softness`. A receiver no blocker covers is
/// lit. The shader's `soft_shadow` is the same.
pub fn soft_shadow(
    view: &ShadowView,
    layer_side: u32,
    depth_map: &[f32],
    point: [f32; 3],
    filter: &SoftFilter,
) -> f32 {
    let Some((at, depth)) = project_into(view, point) else {
        return 1.0;
    };
    let softness = filter.softness.max(MIN_SOFTNESS);
    // the receiver's depth `offset` texels across the map, on its own plane
    let plane =
        |offset: [f32; 2]| depth + filter.slope[0] * offset[0] + filter.slope[1] * offset[1];
    let mut radius = softness;
    if filter.size > 0.0 {
        let (mut blockers, mut found) = (0.0f32, 0u32);
        for index in 0..SOFT_TAPS {
            let offset = disc_tap(index, filter.turn).map(|axis| axis * softness);
            let stored = stored_depth(
                view,
                layer_side,
                depth_map,
                (at[0] + offset[0]).floor() as i64,
                (at[1] + offset[1]).floor() as i64,
            );
            if stored < plane(offset) {
                blockers += view.distance(stored);
                found += 1;
            }
        }
        if found == 0 {
            return 1.0;
        }
        let blocker = blockers / found as f32;
        let receiver = view.distance(depth);
        let gap = (receiver - blocker).max(0.0);
        let (penumbra, texel) = if view.perspective {
            (filter.size * gap / blocker.max(1e-4), view.texel * receiver)
        } else {
            (filter.size * gap, view.texel)
        };
        radius = (penumbra * 0.5 / texel.max(1e-9)).clamp(MIN_SOFTNESS, softness);
    }
    let mut lit = 0.0;
    for index in 0..SOFT_TAPS {
        let offset = disc_tap(index, filter.turn).map(|axis| axis * radius);
        let tap = [at[0] - 0.5 + offset[0], at[1] - 0.5 + offset[1]];
        lit += bilinear_tap(view, layer_side, depth_map, tap, plane(offset));
    }
    lit / SOFT_TAPS as f32
}

/// How much of light `index` reaches `world`, whose surface's unit normal is `normal`, past
/// its shadow — 1 for a light without one. The receiver is moved toward the light by the
/// shadow's `bias` texels (times one plus the tangent of the light's angle to the surface,
/// held at 4) and along its normal by `normal_bias` texels, both measured where it stands, so
/// a lit surface does not shadow itself (acne) at any angle and a shadow starts where its
/// caster touches the ground. Then it is filtered 3×3 ([`sample_shadow`]), or soft
/// ([`soft_shadow`], turned by `turn` radians) when the light has a `size` or its shadow a
/// `softness`. The shader's `shadow_visibility` is the same.
pub fn shadow_visibility(
    scene: &Scene,
    plan: &ShadowPlan,
    maps: &[Vec<f32>],
    index: usize,
    world: [f32; 3],
    normal: [f32; 3],
    turn: f32,
) -> f32 {
    let (first, count) = plan.lights.get(index).copied().unwrap_or((0, 0));
    let (Some(light), true) = (scene.lights.get(index), count > 0) else {
        return 1.0;
    };
    let Some(shadow) = &light.shadow else {
        return 1.0;
    };
    let face = if count == 6 {
        cube_face(subtract(world, light.position))
    } else {
        0
    };
    let at = first as usize + face;
    let (Some(view), Some(depth_map)) = (plan.views.get(at), maps.get(at)) else {
        return 1.0;
    };
    let texel = view.texel * transform(&view.view_proj, world, 1.0)[3].max(0.0);
    let (to_light, _) = light_arrival(light, world);
    let facing = dot(normal, to_light).abs().clamp(1e-3, 1.0);
    let slope = ((1.0 - facing * facing).max(0.0).sqrt() / facing).min(4.0);
    let toward = shadow.bias * texel * (1.0 + slope);
    let along = shadow.normal_bias * texel;
    let biased = [0, 1, 2].map(|axis| world[axis] + normal[axis] * along + to_light[axis] * toward);
    match shadow_softness(light, texel) {
        Some(softness) => soft_shadow(
            view,
            plan.layer_side,
            depth_map,
            biased,
            &SoftFilter {
                size: light.size,
                softness,
                turn,
                slope: receiver_slope(view, biased, normal, to_light),
            },
        ),
        None => sample_shadow(view, plan.layer_side, depth_map, biased),
    }
}

/// The radius a shadowed light's soft filter reaches, in texels where the receiver stands
/// (`texel` metres a texel there): its shadow's `softness` (metres) over that, held to 1 …
/// [`MAX_SOFT_SHADOW_TEXELS`], or [`DEFAULT_SOFTNESS`] texels for a light with a size and no
/// softness — none (the 3×3 filter) for a light with neither, or no shadow. The shader's
/// `shadow_softness` is the same.
pub fn shadow_softness(light: &Light, texel: f32) -> Option<f32> {
    let shadow = light.shadow.as_ref()?;
    if shadow.softness > 0.0 && texel > 0.0 {
        Some((shadow.softness / texel).clamp(1.0, MAX_SOFT_SHADOW_TEXELS))
    } else if light.size > 0.0 {
        Some(DEFAULT_SOFTNESS)
    } else {
        None
    }
}

/// The distance along the view of a point at clip depth `depth` (0 … 1) in the view
/// `view_proj` projects: its clip w, for a perspective view (whose clip depth is `a + b / w`);
/// for an orthographic one (w is 1 everywhere) the depth over its row's length, a distance up
/// to a constant — all the ambient occlusion compares. The shader's `view_distance` is the
/// same.
pub fn view_distance(view_proj: &[f32; 16], depth: f32) -> f32 {
    let depth_row = [view_proj[2], view_proj[6], view_proj[10], view_proj[14]];
    let w_row = [view_proj[3], view_proj[7], view_proj[11], view_proj[15]];
    let w_length = dot(
        [w_row[0], w_row[1], w_row[2]],
        [w_row[0], w_row[1], w_row[2]],
    );
    if w_length < 1e-12 {
        let depth_length = dot(
            [depth_row[0], depth_row[1], depth_row[2]],
            [depth_row[0], depth_row[1], depth_row[2]],
        )
        .sqrt();
        return depth / depth_length.max(1e-12);
    }
    let slope = dot(
        [depth_row[0], depth_row[1], depth_row[2]],
        [w_row[0], w_row[1], w_row[2]],
    ) / w_length;
    let offset = depth_row[3] - slope * w_row[3];
    let gap = depth - slope;
    if gap.abs() < 1e-12 {
        return f32::MAX;
    }
    offset / gap
}

/// Tap `index` of the ambient occlusion, in the frame of the surface (x and y across it, z
/// along its normal), as a share of the radius: a golden-angle spiral over the hemisphere,
/// denser toward the normal (cosine-weighted, as the ambient light arrives), the taps reaching
/// further as they go round, from a fifth of the radius to all of it. The shader's
/// `occlusion_tap` is the same.
pub fn occlusion_tap(index: usize) -> [f32; 3] {
    let share = (index as f32 + 0.5) / OCCLUSION_TAPS as f32;
    let angle = index as f32 * 2.399_963;
    let across = share.sqrt();
    let reach = 0.2 + 0.8 * share * share;
    [
        angle.cos() * across * reach,
        angle.sin() * across * reach,
        (1.0 - share).sqrt() * reach,
    ]
}

/// How much of the ambient light reaches `world`, whose surface's unit normal is `normal`:
/// 1 without ambient occlusion. Each tap of the hemisphere the surface faces is a point
/// `radius` round it at most; a tap behind what the camera's depth view (`plan.occlusion`)
/// holds there is enclosed, counted fully when that surface is within the radius of the
/// point, less the further it is in front (a near object far in front of a wall does not
/// darken the wall). The shader's `ambient_occlusion` is the same.
pub fn ambient_occlusion(
    scene: &Scene,
    plan: &ShadowPlan,
    maps: &[Vec<f32>],
    world: [f32; 3],
    normal: [f32; 3],
) -> f32 {
    let (Some(settings), Some(at)) = (scene.ambient_occlusion, plan.occlusion) else {
        return 1.0;
    };
    let (Some(view), Some(depth_map)) = (plan.views.get(at as usize), maps.get(at as usize)) else {
        return 1.0;
    };
    let radius = settings.radius;
    let clip = transform(&view.view_proj, world, 1.0);
    if radius <= 0.0 || clip[3] <= 1e-6 {
        return 1.0;
    }
    let distance = view_distance(&view.view_proj, clip[2] / clip[3]);
    let helper = if normal[1].abs() < 0.99 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let across = normalize(cross(helper, normal));
    let along = cross(normal, across);
    let side = view.resolution as usize;
    let mut occluded = 0.0;
    for index in 0..OCCLUSION_TAPS {
        let tap = occlusion_tap(index);
        let point = [0, 1, 2].map(|axis| {
            world[axis]
                + (across[axis] * tap[0] + along[axis] * tap[1] + normal[axis] * tap[2]) * radius
        });
        let clip = transform(&view.view_proj, point, 1.0);
        if clip[3] <= 1e-6 {
            continue;
        }
        let across_unit = clip[0] / clip[3] * 0.5 + 0.5;
        let down_unit = 0.5 - clip[1] / clip[3] * 0.5;
        if !(0.0..1.0).contains(&across_unit) || !(0.0..1.0).contains(&down_unit) {
            continue;
        }
        let column = ((across_unit * side as f32) as usize).min(side - 1);
        let row = ((down_unit * side as f32) as usize).min(side - 1);
        let stored = view_distance(&view.view_proj, depth_map[row * side + column]);
        let reached = view_distance(&view.view_proj, clip[2] / clip[3]);
        if stored < reached - radius * 0.05 {
            occluded += (radius / (distance - stored).abs().max(1e-6)).min(1.0);
        }
    }
    (1.0 - settings.strength * occluded / OCCLUSION_TAPS as f32).clamp(0.0, 1.0)
}

/// The widest a soft shadow's filter reaches for a `softness` in metres, in texels where the
/// receiver stands: as wide as a sized light's default reach ([`DEFAULT_SOFTNESS`]), so a
/// softness wider than the map can hold is clamped rather than breaking its taps apart.
pub const MAX_SOFT_SHADOW_TEXELS: f32 = DEFAULT_SOFTNESS;

// --- the CPU painter's rasteriser ---------------------------------------------------------------

/// A texture as the rasteriser samples it: straight-alpha RGBA bytes.
pub struct TextureRef<'a> {
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
    /// Its mip chain ([`mip_chain`]): empty for a texture sampled at its own size only.
    pub levels: &'a [MipLevel],
}

/// The alpha a `:coverage` texture's mip levels keep the share of texels above: the middle of a
/// `:hair` surface's coverage at its usual cutoff of a half.
pub const COVERAGE_CUTOFF: f32 = 0.5;

/// One level of a texture's mip chain below its own size: half the size of the level above
/// (rounded down, at least 1), straight-alpha RGBA bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct MipLevel {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// The mip chain of a smooth texture (`w`×`h` straight-alpha `rgba`), every level below its own
/// down to 1×1: each texel the average of the (up to) four under it, weighted by their alpha —
/// premultiplied, so a transparent texel's colour does not bleed into a strand's edge — so a
/// texture seen small (fine hair, far away) reads as its average, not as the few texels that
/// happen to land on a pixel (aliasing: crimp and glitter). Alpha averages too: what a
/// `:hair` surface's coverage reads. With a `coverage` cutoff (a `:coverage` texture, at
/// [`COVERAGE_CUTOFF`]) each level's alpha is scaled so as many of its texels reach the cutoff,
/// as a share, as at the texture's own size (Castaño's coverage-preserving mips): fine strands
/// keep their density with distance instead of averaging into a faint, uniform sheet. Both
/// painters sample these levels (the GPU target uploads them; ADR-786).
pub fn mip_chain(rgba: &[u8], w: u32, h: u32, coverage: Option<f32>) -> Vec<MipLevel> {
    let mut levels: Vec<MipLevel> = Vec::new();
    let (mut width, mut height) = (w as usize, h as usize);
    if rgba.len() < width * height * 4 {
        return levels;
    }
    // the share of texels at or above the cutoff at the texture's own size, which every level
    // keeps (a coverage texture's)
    let covered_share = |bytes: &[u8], cutoff: f32, scale: f32| {
        let texels = bytes.len() / 4;
        let covered = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|texel| (f32::from(texel[3]) / 255.0 * scale).min(1.0) >= cutoff)
            .count();
        covered as f32 / texels.max(1) as f32
    };
    let target = coverage.map(|cutoff| covered_share(rgba, cutoff, 1.0));
    // each level averages the one above as it was averaged, not as it was rescaled
    let mut above: Vec<u8> = rgba.to_vec();
    while width > 1 || height > 1 {
        let (next_width, next_height) = ((width / 2).max(1), (height / 2).max(1));
        let mut out = vec![0u8; next_width * next_height * 4];
        for row in 0..next_height {
            for column in 0..next_width {
                let mut sum = [0.0f32; 4];
                for (step_row, step_column) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let source_row = (row * 2 + step_row).min(height - 1);
                    let source_column = (column * 2 + step_column).min(width - 1);
                    let start = (source_row * width + source_column) * 4;
                    let alpha = f32::from(above[start + 3]) / 255.0;
                    for channel in 0..3 {
                        sum[channel] += f32::from(above[start + channel]) * alpha;
                    }
                    sum[3] += alpha;
                }
                let start = (row * next_width + column) * 4;
                if sum[3] > 1e-6 {
                    for channel in 0..3 {
                        out[start + channel] =
                            (sum[channel] / sum[3]).round().clamp(0.0, 255.0) as u8;
                    }
                }
                out[start + 3] = (sum[3] / 4.0 * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        let mut kept = out.clone();
        if let (Some(cutoff), Some(target)) = (coverage, target) {
            // the alpha scale that keeps the covered share (Castaño): found by halving, the
            // share growing with the scale
            let (mut low, mut high) = (0.0f32, 8.0f32);
            for _ in 0..16 {
                let middle = 0.5 * (low + high);
                if covered_share(&out, cutoff, middle) < target {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            // the bracket's upper end: never less covered than the texture itself
            let scale = high;
            for texel in kept.as_chunks_mut::<4>().0 {
                texel[3] = (f32::from(texel[3]) * scale).round().clamp(0.0, 255.0) as u8;
            }
        }
        levels.push(MipLevel {
            w: next_width as u32,
            h: next_height as u32,
            rgba: kept,
        });
        above = out;
        (width, height) = (next_width, next_height);
    }
    levels
}

/// A texel bilinearly filtered at `(u, v)` (repeating), each channel 0 … 1, still encoded.
fn sample(texture: &TextureRef, u: f32, v: f32) -> [f32; 4] {
    if texture.w == 0 || texture.h == 0 || texture.rgba.len() < (texture.w * texture.h * 4) as usize
    {
        return [1.0; 4];
    }
    let (width, height) = (texture.w as i64, texture.h as i64);
    let across = u.rem_euclid(1.0) * width as f32 - 0.5;
    let down = v.rem_euclid(1.0) * height as f32 - 0.5;
    let (left, top) = (across.floor(), down.floor());
    let (fraction_x, fraction_y) = (across - left, down - top);
    let texel = |column: i64, row: i64| {
        let column = column.rem_euclid(width) as usize;
        let row = row.rem_euclid(height) as usize;
        let start = (row * texture.w as usize + column) * 4;
        let pixel = &texture.rgba[start..start + 4];
        let alpha = pixel[3] as f32 / 255.0;
        // filtered premultiplied, so a transparent texel's colour does not bleed in
        [
            pixel[0] as f32 / 255.0 * alpha,
            pixel[1] as f32 / 255.0 * alpha,
            pixel[2] as f32 / 255.0 * alpha,
            alpha,
        ]
    };
    let (left, top) = (left as i64, top as i64);
    let (top_left, top_right) = (texel(left, top), texel(left + 1, top));
    let (bottom_left, bottom_right) = (texel(left, top + 1), texel(left + 1, top + 1));
    let mut out = [0.0f32; 4];
    for channel in 0..4 {
        let upper = top_left[channel] + (top_right[channel] - top_left[channel]) * fraction_x;
        let lower =
            bottom_left[channel] + (bottom_right[channel] - bottom_left[channel]) * fraction_x;
        out[channel] = upper + (lower - upper) * fraction_y;
    }
    let coverage = out[3];
    if coverage > 1e-6 {
        for value in out.iter_mut().take(3) {
            *value /= coverage;
        }
    }
    out
}

/// A texel trilinearly filtered at `(u, v)`: bilinear in the two levels of `texture`'s mip
/// chain about `lod` (0 its own size, 1 the first level below, …), mixed premultiplied — as
/// the GPU target's mesh sampler filters. Straight alpha out, still encoded.
fn sample_lod(texture: &TextureRef, u: f32, v: f32, lod: f32) -> [f32; 4] {
    let lod = lod.clamp(0.0, texture.levels.len() as f32);
    let level = |index: usize| {
        let texel = if index == 0 {
            sample(texture, u, v)
        } else {
            let below = &texture.levels[index - 1];
            sample(
                &TextureRef {
                    w: below.w,
                    h: below.h,
                    rgba: &below.rgba,
                    levels: &[],
                },
                u,
                v,
            )
        };
        [
            texel[0] * texel[3],
            texel[1] * texel[3],
            texel[2] * texel[3],
            texel[3],
        ]
    };
    let lower = lod.floor() as usize;
    let fraction = lod - lower as f32;
    let near = level(lower);
    let mixed = if fraction > 0.0 && lower < texture.levels.len() {
        let far = level(lower + 1);
        [0, 1, 2, 3].map(|channel| near[channel] + (far[channel] - near[channel]) * fraction)
    } else {
        near
    };
    if mixed[3] > 1e-6 {
        [
            mixed[0] / mixed[3],
            mixed[1] / mixed[3],
            mixed[2] / mixed[3],
            mixed[3],
        ]
    } else {
        [0.0, 0.0, 0.0, 0.0]
    }
}

/// The mip level a triangle samples `texture` at, the texture repeated `repeats` times across
/// the UV square (`[1, 1]` but for a detail tile, which is read at its own scaled coordinates):
/// half the base-2 logarithm of how many of its texels fall on one of the picture's pixels
/// (`supersample` canvas pixels a side), 0 when it is magnified. One level for the whole
/// triangle (the GPU target's sampler picks one a 2×2 block of pixels; on a mesh's small
/// triangles they agree).
fn triangle_lod(
    corners: [&ClipVertex; 3],
    texture: &TextureRef,
    repeats: [f32; 2],
    width: usize,
    height: usize,
    supersample: f32,
) -> f32 {
    if texture.levels.is_empty() {
        return 0.0;
    }
    let pixel = |vertex: &ClipVertex| {
        let inverse_w = 1.0 / vertex.clip[3];
        [
            (vertex.clip[0] * inverse_w * 0.5 + 0.5) * width as f32,
            (0.5 - vertex.clip[1] * inverse_w * 0.5) * height as f32,
        ]
    };
    let texel = |vertex: &ClipVertex| {
        [
            vertex.uv[0] * repeats[0] * texture.w as f32,
            vertex.uv[1] * repeats[1] * texture.h as f32,
        ]
    };
    let area = |points: [[f32; 2]; 3]| {
        ((points[1][0] - points[0][0]) * (points[2][1] - points[0][1])
            - (points[2][0] - points[0][0]) * (points[1][1] - points[0][1]))
            .abs()
            * 0.5
    };
    let screen = area(corners.map(pixel)) / (supersample * supersample);
    let texels = area(corners.map(texel));
    if screen <= 1e-9 || texels <= 1e-9 {
        return 0.0;
    }
    (0.5 * (texels / screen).log2()).max(0.0)
}

fn transform(matrix: &[f32; 16], point: [f32; 3], w: f32) -> [f32; 4] {
    [
        matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12] * w,
        matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13] * w,
        matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14] * w,
        matrix[3] * point[0] + matrix[7] * point[1] + matrix[11] * point[2] + matrix[15] * w,
    ]
}

fn multiply(left: &[f32; 16], right: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = (0..4)
                .map(|term| left[term * 4 + row] * right[column * 4 + term])
                .sum();
        }
    }
    out
}

/// The skinning matrix of vertex `index` — the weighted sum of its joints' palette matrices,
/// a joint past the palette's end skipped — or `None` for a rigid draw or a vertex no joint
/// of the palette moves. The shader skins the same way (`vs_mesh`).
pub fn skin_matrix(mesh: &Mesh, palette: Option<&[f32]>, index: usize) -> Option<[f32; 16]> {
    let palette = palette?;
    if mesh.joints.is_empty() {
        return None;
    }
    let joints = mesh.joints[index];
    let weights = mesh.weights[index];
    let mut out = [0.0f32; 16];
    let mut any = false;
    for (joint, weight) in joints.iter().zip(weights) {
        let start = *joint as usize * 16;
        if weight <= 0.0 || start + 16 > palette.len() {
            continue;
        }
        any = true;
        for (slot, value) in out.iter_mut().zip(&palette[start..start + 16]) {
            *slot += weight * value;
        }
    }
    any.then_some(out)
}

/// A vertex ready to rasterise: its clip position and what the fragment interpolates.
#[derive(Clone, Copy)]
struct ClipVertex {
    clip: [f32; 4],
    world: [f32; 3],
    normal: [f32; 3],
    tangent: [f32; 4],
    uv: [f32; 2],
    /// How sharply the mesh curves here ([`Mesh::curvature`]).
    curvature: f32,
}

fn lerp_vertex(from: &ClipVertex, to: &ClipVertex, amount: f32) -> ClipVertex {
    let mix = |start: f32, end: f32| start + (end - start) * amount;
    let mix_n = |start: &[f32], end: &[f32], out: &mut [f32]| {
        for ((slot, begin), finish) in out.iter_mut().zip(start).zip(end) {
            *slot = mix(*begin, *finish);
        }
    };
    let mut vertex = *from;
    mix_n(&from.clip, &to.clip, &mut vertex.clip);
    mix_n(&from.world, &to.world, &mut vertex.world);
    mix_n(&from.normal, &to.normal, &mut vertex.normal);
    mix_n(
        &from.tangent[..3],
        &to.tangent[..3],
        &mut vertex.tangent[..3],
    );
    mix_n(&from.uv, &to.uv, &mut vertex.uv);
    vertex.curvature = mix(from.curvature, to.curvature);
    vertex
}

/// The smallest clip `w` a vertex may have: the near plane the rasteriser clips at, as the GPU
/// clips at depth 0.
const NEAR_W: f32 = 1e-5;

/// `triangle` clipped to the half-space in front of the camera (`w ≥ NEAR_W` and depth ≥ 0):
/// none, or a convex polygon of three or four vertices — so a floor the camera stands on, or a
/// room it is inside, keeps the part in front of it, as on the GPU.
fn clip_near(triangle: [ClipVertex; 3]) -> Vec<ClipVertex> {
    // the distance inside each plane: w − NEAR_W, and z (clip depth 0 … w)
    let planes: [fn(&ClipVertex) -> f32; 2] =
        [|vertex| vertex.clip[3] - NEAR_W, |vertex| vertex.clip[2]];
    let mut polygon: Vec<ClipVertex> = triangle.to_vec();
    for inside in planes {
        if polygon.iter().all(|vertex| inside(vertex) >= 0.0) {
            continue;
        }
        let mut clipped = Vec::with_capacity(polygon.len() + 1);
        for (position, current) in polygon.iter().enumerate() {
            let next = &polygon[(position + 1) % polygon.len()];
            let (here, there) = (inside(current), inside(next));
            if here >= 0.0 {
                clipped.push(*current);
            }
            if (here >= 0.0) != (there >= 0.0) {
                clipped.push(lerp_vertex(current, next, here / (here - there)));
            }
        }
        polygon = clipped;
        if polygon.len() < 3 {
            return Vec::new();
        }
    }
    polygon
}

/// A scene's colour and depth as the rasteriser fills them, at the supersampled size.
struct Canvas {
    width: usize,
    height: usize,
    /// Premultiplied, sRGB-encoded — the space the GPU target blends in.
    color: Vec<[f32; 4]>,
    depth: Vec<f32>,
}

/// One draw's state the fragments of its triangles read.
struct DrawContext<'a> {
    scene: &'a Scene,
    material: &'a Material,
    base_texture: Option<TextureRef<'a>>,
    normal_texture: Option<TextureRef<'a>>,
    occlusion_texture: Option<TextureRef<'a>>,
    /// The material's detail tile (ADR-797), for its cavity; none for a hair surface.
    detail_texture: Option<TextureRef<'a>>,
    /// Whether the detail tile's normal tilts the surface's: only on a mesh with tangents.
    detail_tilts: bool,
    /// How many canvas pixels a side make one of the picture's: a soft shadow's noise is the
    /// picture pixel's, as the GPU's is.
    supersample: usize,
    shadows: &'a ShadowPlan,
    shadow_maps: &'a [Vec<f32>],
}

/// The pixels of a `width`×`height` picture whose centres `corners` (already clipped in front
/// of the view) cover, each handed to `pixel` with its column, row, screen-space barycentric
/// weights and depth (0 … 1; a pixel past either end of the depth range is left out). A
/// triangle facing away (clockwise in normalised device coordinates) is skipped unless
/// `keep_back`; `pixel` is told which way it faces.
fn scan_triangle(
    width: usize,
    height: usize,
    corners: [&ClipVertex; 3],
    keep_back: bool,
    mut pixel: impl FnMut(usize, usize, [f32; 3], f32, bool),
) {
    let to_device = |vertex: &ClipVertex| {
        let inverse_w = 1.0 / vertex.clip[3];
        [
            vertex.clip[0] * inverse_w,
            vertex.clip[1] * inverse_w,
            vertex.clip[2] * inverse_w,
        ]
    };
    let device = [
        to_device(corners[0]),
        to_device(corners[1]),
        to_device(corners[2]),
    ];
    // counter-clockwise in normalised device coordinates faces the viewer (glTF's winding, and
    // the GPU pipeline's front face)
    let area = (device[1][0] - device[0][0]) * (device[2][1] - device[0][1])
        - (device[2][0] - device[0][0]) * (device[1][1] - device[0][1]);
    let front = area > 0.0;
    if area == 0.0 || !area.is_finite() || (!front && !keep_back) {
        return;
    }
    // screen space in f64: a triangle clipped just in front of the camera reaches hundreds of
    // thousands of pixels off the picture, where f32 edge functions cancel to noise
    let (width_f, height_f) = (width as f64, height as f64);
    let to_screen = |point: [f32; 3]| {
        [
            (f64::from(point[0]) * 0.5 + 0.5) * width_f,
            (0.5 - f64::from(point[1]) * 0.5) * height_f,
            f64::from(point[2]),
        ]
    };
    let screen = [
        to_screen(device[0]),
        to_screen(device[1]),
        to_screen(device[2]),
    ];
    let span = |axis: usize, limit: f64| {
        let low = screen
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as usize;
        let high = screen
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(limit) as usize;
        (low, high)
    };
    let (min_x, max_x) = span(0, width_f);
    let (min_y, max_y) = span(1, height_f);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let edge = |from: [f64; 3], to: [f64; 3], x: f64, y: f64| {
        (to[0] - from[0]) * (y - from[1]) - (to[1] - from[1]) * (x - from[0])
    };
    let total = edge(screen[0], screen[1], screen[2][0], screen[2][1]);
    if total.abs() < 1e-12 {
        return;
    }
    for row in min_y..max_y {
        for column in min_x..max_x {
            let (x, y) = (column as f64 + 0.5, row as f64 + 0.5);
            let first = edge(screen[1], screen[2], x, y) / total;
            let second = edge(screen[2], screen[0], x, y) / total;
            let third = 1.0 - first - second;
            if first < 0.0 || second < 0.0 || third < 0.0 {
                continue;
            }
            let depth =
                (first * screen[0][2] + second * screen[1][2] + third * screen[2][2]) as f32;
            if !(0.0..=1.0).contains(&depth) {
                continue;
            }
            pixel(
                column,
                row,
                [first as f32, second as f32, third as f32],
                depth,
                front,
            );
        }
    }
}

/// Screen-space barycentric weights made perspective-correct for the attributes of
/// `corners`.
fn perspective_weights(corners: [&ClipVertex; 3], screen: [f32; 3]) -> [f32; 3] {
    let weights = [
        screen[0] / corners[0].clip[3],
        screen[1] / corners[1].clip[3],
        screen[2] / corners[2].clip[3],
    ];
    let sum = weights[0] + weights[1] + weights[2];
    [weights[0] / sum, weights[1] / sum, weights[2] / sum]
}

fn blend_uv(corners: [&ClipVertex; 3], weights: [f32; 3]) -> [f32; 2] {
    [
        corners[0].uv[0] * weights[0]
            + corners[1].uv[0] * weights[1]
            + corners[2].uv[0] * weights[2],
        corners[0].uv[1] * weights[0]
            + corners[1].uv[1] * weights[1]
            + corners[2].uv[1] * weights[2],
    ]
}

/// Rasterise one triangle (already clipped in front of the camera) into `canvas`.
fn fill_triangle(canvas: &mut Canvas, context: &DrawContext, corners: [&ClipVertex; 3]) {
    let material = context.material;
    let sign = if corners[0].tangent[3] < 0.0 {
        -1.0
    } else {
        1.0
    };
    let blend = material.surface == Surface::Blend;
    let scene = context.scene;
    let lights = scene.lights.len().min(MAX_LIGHTS);
    let (width, height) = (canvas.width, canvas.height);
    // the mip level each texture is read at, for the whole triangle
    let base_lod = context.base_texture.as_ref().map_or(0.0, |texture| {
        triangle_lod(
            corners,
            texture,
            [1.0, 1.0],
            width,
            height,
            context.supersample as f32,
        )
    });
    let normal_lod = context.normal_texture.as_ref().map_or(0.0, |texture| {
        triangle_lod(
            corners,
            texture,
            [1.0, 1.0],
            width,
            height,
            context.supersample as f32,
        )
    });
    // the detail tile's level follows its own, scaled coordinates: it fades into its average
    // (flat, and the tile's mean cavity) as its repeats shrink under a pixel
    let detail_lod = context.detail_texture.as_ref().map_or(0.0, |texture| {
        triangle_lod(
            corners,
            texture,
            material.detail_scale,
            width,
            height,
            context.supersample as f32,
        )
    });
    scan_triangle(
        width,
        height,
        corners,
        material.double_sided,
        |column, row, screen, depth, front| {
            let slot = row * canvas.width + column;
            if depth >= canvas.depth[slot] {
                return;
            }
            let weights = perspective_weights(corners, screen);
            let blend3 = |pick: fn(&ClipVertex) -> [f32; 3]| {
                let values = [pick(corners[0]), pick(corners[1]), pick(corners[2])];
                [0, 1, 2].map(|axis| {
                    values[0][axis] * weights[0]
                        + values[1][axis] * weights[1]
                        + values[2][axis] * weights[2]
                })
            };
            let uv = blend_uv(corners, weights);
            let texel = context.base_texture.as_ref().map_or([1.0; 4], |texture| {
                sample_lod(texture, uv[0], uv[1], base_lod)
            });
            let mut alpha = texel[3] * material.color[3];
            match material.surface {
                Surface::Cutout => {
                    if alpha < material.cutoff {
                        return;
                    }
                    alpha = 1.0;
                }
                Surface::Blend => {
                    if alpha <= 0.002 {
                        return;
                    }
                }
                // coverage, past half the cutoff: drawn at this sample when it reaches its step of
                // the 2×2 dither, then opaque (the GPU target's alpha-to-coverage)
                Surface::Hair | Surface::Strands => {
                    let mut coverage = ((alpha - 0.5 * material.cutoff)
                        / material.cutoff.max(1e-6))
                    .clamp(0.0, 1.0);
                    // a strand thinner than a pixel covers only its share of one, at samples
                    // its own position picks (`strand_sample_step`): strands overlapping in a
                    // pixel fill different samples and add up, where one dither would have them
                    // all cover the same few
                    let step = if material.surface == Surface::Strands {
                        coverage *= corners[0].tangent[3] * weights[0]
                            + corners[1].tangent[3] * weights[1]
                            + corners[2].tangent[3] * weights[2];
                        strand_sample_step(
                            blend3(|vertex| vertex.world),
                            (row % 2) * 2 + column % 2,
                        )
                    } else {
                        HAIR_COVERAGE_STEPS[row % 2][column % 2]
                    };
                    if coverage <= step {
                        return;
                    }
                    alpha = 1.0;
                }
                Surface::Skin | Surface::Standard => alpha = 1.0,
            }
            let base = [
                srgb_to_linear(texel[0]) * material.color[0],
                srgb_to_linear(texel[1]) * material.color[1],
                srgb_to_linear(texel[2]) * material.color[2],
            ];
            let world = blend3(|vertex| vertex.world);
            let mut normal = normalize(blend3(|vertex| vertex.normal));
            if !front {
                normal = [-normal[0], -normal[1], -normal[2]];
            }
            // the shadow lookups move the receiver along the surface's own normal, not the
            // normal map's
            let surface_normal = normal;
            let mapped = context.normal_texture.as_ref().map(|map| {
                let texel = sample_lod(map, uv[0], uv[1], normal_lod);
                [
                    texel[0] * 2.0 - 1.0,
                    texel[1] * 2.0 - 1.0,
                    texel[2] * 2.0 - 1.0,
                ]
            });
            // the detail tile (ADR-797), read at its own repeats: its normal laid over the
            // normal map's (or the bare surface's) unless it is flat there, its cavity kept for
            // the specular
            let detail = context.detail_texture.as_ref().map(|tile| {
                sample_lod(
                    tile,
                    uv[0] * material.detail_scale[0],
                    uv[1] * material.detail_scale[1],
                    detail_lod,
                )
            });
            let tilt = detail
                .filter(|_| context.detail_tilts)
                .map(detail_tilt)
                .filter(|tilt| tilt[0] != 0.0 || tilt[1] != 0.0);
            let local = match tilt {
                Some(tilt) => Some(detail_normal(
                    mapped.unwrap_or([0.0, 0.0, 1.0]),
                    tilt,
                    material.detail_strength,
                )),
                None => mapped,
            };
            let specular_kept = detail.map_or(1.0, |texel| {
                detail_specular(texel[2], material.detail_cavity)
            });
            if let Some(local) = local {
                let tangent = normalize(blend3(|vertex| {
                    [vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]]
                }));
                let bitangent = cross(normal, tangent);
                normal = normalize([
                    tangent[0] * local[0] + sign * bitangent[0] * local[1] + normal[0] * local[2],
                    tangent[1] * local[0] + sign * bitangent[1] * local[1] + normal[1] * local[2],
                    tangent[2] * local[0] + sign * bitangent[2] * local[1] + normal[2] * local[2],
                ]);
            }
            let occlusion = context
                .occlusion_texture
                .as_ref()
                .map_or(1.0, |texture| sample(texture, uv[0], uv[1])[0]);
            let turn = std::f32::consts::TAU
                * pixel_noise(
                    (column / context.supersample) as f32,
                    (row / context.supersample) as f32,
                );
            let mut visibility = [1.0f32; MAX_LIGHTS];
            for (index, seen) in visibility.iter_mut().enumerate().take(lights) {
                if context.shadows.lights[index].1 > 0 {
                    *seen = shadow_visibility(
                        scene,
                        context.shadows,
                        context.shadow_maps,
                        index,
                        world,
                        surface_normal,
                        turn,
                    );
                }
            }
            // a hair's strand: the bitangent, as its UV v runs, or a strand's own direction
            let strand = if material.surface == Surface::Strands {
                normalize(blend3(|vertex| {
                    [vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]]
                }))
            } else if material.surface == Surface::Hair {
                let tangent = normalize(blend3(|vertex| {
                    [vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]]
                }));
                let bitangent = cross(surface_normal, tangent);
                normalize([
                    bitangent[0] * sign,
                    bitangent[1] * sign,
                    bitangent[2] * sign,
                ])
            } else {
                [0.0; 3]
            };
            // the ambient's occlusion: its map's, times the scene's screen-space ambient occlusion
            let occlusion = occlusion
                * ambient_occlusion(
                    scene,
                    context.shadows,
                    context.shadow_maps,
                    world,
                    surface_normal,
                );
            let eye = scene.eye;
            let view = normalize([eye[0] - world[0], eye[1] - world[1], eye[2] - world[2]]);
            let lit = tone(
                shade(
                    scene,
                    material,
                    base,
                    normal,
                    view,
                    world,
                    occlusion,
                    &visibility,
                    strand,
                    corners[0].curvature * weights[0]
                        + corners[1].curvature * weights[1]
                        + corners[2].curvature * weights[2],
                    specular_kept,
                ),
                scene.exposure,
            );
            let under = canvas.color[slot];
            canvas.color[slot] = [
                lit[0] * alpha + under[0] * (1.0 - alpha),
                lit[1] * alpha + under[1] * (1.0 - alpha),
                lit[2] * alpha + under[2] * (1.0 - alpha),
                alpha + under[3] * (1.0 - alpha),
            ];
            if !blend {
                canvas.depth[slot] = depth;
            }
        },
    );
}

/// The coverage a `:strands` fragment at `world` needs to be drawn at sample `sample` (0 … 3:
/// the GPU target's samples, the CPU painter's 2×2 sub-samples row by row): a hash of where it
/// is and which sample, 0 … 1 (stochastic coverage). Overlapping strands thinner than a pixel
/// then cover different samples and add up as hair does, where a fixed dither would have them
/// all cover the same few. The shader's `strand_sample_step` is the same.
pub fn strand_sample_step(world: [f32; 3], sample: usize) -> f32 {
    let seed = (world[0] * 12.9898 + world[1] * 78.233 + world[2] * 37.719) * 1000.0
        + sample as f32 * 7.31;
    (seed.sin() * 43_758.547).rem_euclid(1.0)
}

/// A `:strands` ribbon's vertex: the strand's point `centre` (world) on the side `side` (−1 or 1)
/// of a ribbon `width` metres wide turned to face the camera about the strand's unit
/// `direction`. Never under one of the picture's pixels wide where the point stands: a thinner
/// strand is drawn a pixel wide, its coverage the share it fills. Answers the vertex, its normal
/// (a cylinder's, turned to that side) and the coverage. The shader's `strand_vertex` is the
/// same.
pub fn strand_vertex(
    scene: &Scene,
    centre: [f32; 3],
    direction: [f32; 3],
    side: f32,
    width: f32,
) -> ([f32; 3], [f32; 3], f32) {
    let view = normalize(subtract(scene.eye, centre));
    let across = normalize(cross(direction, view));
    let facing = normalize(cross(across, direction));
    // the metres a picture pixel spans across the strand here: where a centimetre lands
    let (pixel_width, pixel_height) = scene.pixel_size();
    let near = transform(&scene.view_proj, centre, 1.0);
    let far = transform(
        &scene.view_proj,
        [0, 1, 2].map(|axis| centre[axis] + across[axis] * 0.01),
        1.0,
    );
    let pixel = if near[3] > 1e-6 && far[3] > 1e-6 {
        let x = (far[0] / far[3] - near[0] / near[3]) * pixel_width as f32 * 0.5;
        let y = (far[1] / far[3] - near[1] / near[3]) * pixel_height as f32 * 0.5;
        let pixels = (x * x + y * y).sqrt();
        if pixels > 1e-6 { 0.01 / pixels } else { width }
    } else {
        width
    };
    let drawn = width.max(pixel);
    let coverage = if drawn > 0.0 {
        (width / drawn).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let half = drawn * 0.5 * side;
    let vertex = [0, 1, 2].map(|axis| centre[axis] + across[axis] * half);
    let normal = normalize([0, 1, 2].map(|axis| across[axis] * side + facing[axis]));
    (vertex, normal, coverage)
}

/// The vertices of `mesh` drawn by `draw`: morphed, skinned, moved by the model matrix and seen
/// through the scene's camera.
fn pose_vertices(scene: &Scene, mesh: &Mesh, draw: &Draw) -> Vec<ClipVertex> {
    let palette = scene.palette_of(draw);
    // morphed only where its targets move it (sparse: a face target is a few thousand vertices
    // of a body's seventy thousand)
    let mut positions = mesh.positions.clone();
    let mut normals = mesh.normals.clone();
    add_morphs(&mesh.morphs, &draw.morphs, &mut positions, &mut normals);
    (0..mesh.positions.len())
        .map(|index| {
            let position = positions[index];
            let normal = normals[index];
            let tangent = mesh
                .tangents
                .as_ref()
                .map_or([1.0, 0.0, 0.0, 1.0], |tangents| tangents[index]);
            let matrix = match skin_matrix(mesh, palette, index) {
                Some(skin) => multiply(&draw.model, &skin),
                None => draw.model,
            };
            let world = transform(&matrix, position, 1.0);
            let turned_normal = transform(&matrix, normal, 0.0);
            let turned_tangent = transform(&matrix, [tangent[0], tangent[1], tangent[2]], 0.0);
            if draw.material.surface == Surface::Strands {
                // a ribbon turned to face the camera: its tangent the strand's direction (the
                // mesh's normal), its fourth the coverage
                let direction = normalize([turned_normal[0], turned_normal[1], turned_normal[2]]);
                let [root, tip] = draw.material.strand_width;
                let uv = mesh.uvs[index];
                let width = tip + (root - tip) * uv[1].clamp(0.0, 1.0);
                let centre = [world[0], world[1], world[2]];
                let (vertex, ribbon_normal, coverage) =
                    strand_vertex(scene, centre, direction, uv[0].signum(), width);
                return ClipVertex {
                    clip: transform(&scene.view_proj, vertex, 1.0),
                    world: vertex,
                    normal: ribbon_normal,
                    tangent: [direction[0], direction[1], direction[2], coverage],
                    uv,
                    curvature: 0.0,
                };
            }
            ClipVertex {
                clip: transform(&scene.view_proj, [world[0], world[1], world[2]], 1.0),
                world: [world[0], world[1], world[2]],
                normal: normalize([turned_normal[0], turned_normal[1], turned_normal[2]]),
                tangent: {
                    let unit = normalize([turned_tangent[0], turned_tangent[1], turned_tangent[2]]);
                    [unit[0], unit[1], unit[2], tangent[3]]
                },
                uv: mesh.uvs[index],
                curvature: mesh.curvature[index],
            }
        })
        .collect()
}

/// A draw posed for this frame: its mesh and its vertices, as the camera sees them.
struct Posed<'a> {
    draw: &'a Draw,
    mesh: Arc<Mesh>,
    vertices: Vec<ClipVertex>,
}

/// The convex polygons `mesh`'s triangles leave in front of the view their `vertices` are seen
/// through, each as a fan of triangles handed to `triangle`.
fn each_clipped_triangle(
    mesh: &Mesh,
    vertices: &[ClipVertex],
    mut triangle: impl FnMut([&ClipVertex; 3]),
) {
    for indices in mesh.indices.as_chunks::<3>().0 {
        let corners = [
            vertices[indices[0] as usize],
            vertices[indices[1] as usize],
            vertices[indices[2] as usize],
        ];
        let polygon = clip_near(corners);
        // a convex polygon, as a fan from its first vertex
        for fan in 1..polygon.len().saturating_sub(1) {
            triangle([&polygon[0], &polygon[fan], &polygon[fan + 1]]);
        }
    }
}

/// The depth map of shadow `view`: every draw of `posed` that casts a shadow, rasterised as
/// the light sees it, both faces, a `:cutout`, `:blend` or `:hair` surface only where its alpha
/// reaches its `cutoff` — `resolution`² depths, row 0 the top, 1 where nothing is.
fn render_shadow_map<'t>(
    view: &ShadowView,
    posed: &[Posed],
    textures: &dyn Fn(u32) -> Option<TextureRef<'t>>,
) -> Vec<f32> {
    let side = view.resolution as usize;
    let mut depth_map = vec![1.0f32; side * side];
    for item in posed {
        let material = &item.draw.material;
        if !material.cast_shadow {
            continue;
        }
        let alpha_tested = matches!(
            material.surface,
            Surface::Cutout | Surface::Blend | Surface::Hair | Surface::Strands
        );
        let base_texture = if alpha_tested {
            material.base.and_then(textures)
        } else {
            None
        };
        if alpha_tested && base_texture.is_none() && material.color[3] < material.cutoff {
            continue;
        }
        let seen: Vec<ClipVertex> = item
            .vertices
            .iter()
            .map(|vertex| ClipVertex {
                clip: transform(&view.view_proj, vertex.world, 1.0),
                ..*vertex
            })
            .collect();
        each_clipped_triangle(&item.mesh, &seen, |corners| {
            scan_triangle(
                side,
                side,
                corners,
                true,
                |column, row, screen, depth, _| {
                    let slot = row * side + column;
                    if depth >= depth_map[slot] {
                        return;
                    }
                    if let Some(texture) = base_texture.as_ref() {
                        let uv = blend_uv(corners, perspective_weights(corners, screen));
                        if sample(texture, uv[0], uv[1])[3] * material.color[3] < material.cutoff {
                            return;
                        }
                    }
                    depth_map[slot] = depth;
                },
            );
        });
    }
    depth_map
}

/// How many times finer than the picture the CPU painter rasterises a scene of at most
/// `pixels` pixels and averages down: the anti-aliasing the GPU's 4× multisampling gives, for
/// a scene small enough that four samples a pixel stay cheap; one sample for a bigger one, so
/// a full-screen scene costs 20 bytes a pixel, not 80.
fn supersample_for(pixels: usize) -> usize {
    if pixels <= 1 << 20 { 2 } else { 1 }
}

/// `scene` rasterised in software: its picture, `w`×`h` straight-alpha RGBA bytes (sRGB),
/// what the CPU painter draws where the op stands. `meshes` and `textures` look up handles;
/// a draw naming a mesh that is not there draws nothing, a missing texture samples white.
/// The scene's shadow maps are rasterised first, from the same posed vertices (ADR-788).
pub fn rasterize<'t>(
    scene: &Scene,
    meshes: &dyn Fn(u32) -> Option<Arc<Mesh>>,
    textures: &dyn Fn(u32) -> Option<TextureRef<'t>>,
) -> (u32, u32, Vec<u8>) {
    let (out_width, out_height) = scene.pixel_size();
    let supersample = supersample_for(out_width as usize * out_height as usize);
    let (width, height) = (
        out_width as usize * supersample,
        out_height as usize * supersample,
    );
    let background = scene.background;
    let clear = [
        background[0].clamp(0.0, 1.0) * background[3],
        background[1].clamp(0.0, 1.0) * background[3],
        background[2].clamp(0.0, 1.0) * background[3],
        background[3].clamp(0.0, 1.0),
    ];
    let mut canvas = Canvas {
        width,
        height,
        color: vec![clear; width * height],
        depth: vec![1.0f32; width * height],
    };
    let posed: Vec<Posed> = scene
        .draw_order()
        .filter_map(|draw| {
            let mesh = meshes(draw.mesh)?;
            let vertices = pose_vertices(scene, &mesh, draw);
            Some(Posed {
                draw,
                mesh,
                vertices,
            })
        })
        .collect();
    let world = posed.iter().fold(None, |bounds, item| {
        union(bounds, draw_bounds(scene, item.draw, &item.mesh.bounds))
    });
    let shadows = shadow_plan(scene, world);
    let shadow_maps: Vec<Vec<f32>> = shadows
        .views
        .iter()
        .map(|view| render_shadow_map(view, &posed, textures))
        .collect();
    for item in &posed {
        let material = &item.draw.material;
        let context = DrawContext {
            scene,
            material,
            base_texture: material.base.and_then(textures),
            normal_texture: if item.mesh.tangents.is_some() {
                material.normal.and_then(textures)
            } else {
                None
            },
            occlusion_texture: material.occlusion.and_then(textures),
            detail_texture: material.detail_texture().and_then(textures),
            detail_tilts: item.mesh.tangents.is_some(),
            supersample,
            shadows: &shadows,
            shadow_maps: &shadow_maps,
        };
        each_clipped_triangle(&item.mesh, &item.vertices, |corners| {
            fill_triangle(&mut canvas, &context, corners)
        });
    }
    // averaged down to the picture's pixels, premultiplied, then straight alpha
    let mut rgba = vec![0u8; out_width as usize * out_height as usize * 4];
    let samples = (supersample * supersample) as f32;
    for out_row in 0..out_height as usize {
        for out_column in 0..out_width as usize {
            let mut sum = [0.0f32; 4];
            for sub_row in 0..supersample {
                for sub_column in 0..supersample {
                    let pixel = canvas.color[(out_row * supersample + sub_row) * width
                        + out_column * supersample
                        + sub_column];
                    for channel in 0..4 {
                        sum[channel] += pixel[channel];
                    }
                }
            }
            let alpha = sum[3] / samples;
            let start = (out_row * out_width as usize + out_column) * 4;
            if alpha > 1e-6 {
                for channel in 0..3 {
                    rgba[start + channel] =
                        ((sum[channel] / samples / alpha).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            rgba[start + 3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    (out_width, out_height, rgba)
}

/// The meshes a window keeps, by handle — shared by both painters (the CPU one rasterises
/// from them; a GPU window keeps them too, for `gui/capture`, which paints in software).
#[derive(Default)]
pub struct MeshBook {
    meshes: HashMap<u32, Arc<Mesh>>,
}

impl MeshBook {
    pub fn insert(&mut self, handle: u32, mesh: Arc<Mesh>) {
        self.meshes.insert(handle, mesh);
    }
    pub fn remove(&mut self, handle: u32) {
        self.meshes.remove(&handle);
    }
    pub fn get(&self, handle: u32) -> Option<Arc<Mesh>> {
        self.meshes.get(&handle).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_of(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn identity() -> [f32; 16] {
        [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    }

    /// A triangle facing +z, covering the middle of clip space.
    fn triangle() -> Mesh {
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&[-0.8, -0.8, 0.5, 0.8, -0.8, 0.5, 0.0, 0.8, 0.5]),
            normals: bytes_of(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
            indices: [0u16, 1, 2]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        })
        .expect("a triangle")
    }

    fn scene(draw: Draw) -> Scene {
        Scene {
            x: 0.0,
            y: 0.0,
            w: 16.0,
            h: 16.0,
            view_proj: identity(),
            eye: [0.0, 0.0, -10.0],
            lights: vec![Light::directional([0.0, 0.0, -1.0], [1.0, 1.0, 1.0])],
            sky: [0.1, 0.1, 0.1],
            ground: [0.1, 0.1, 0.1],
            ambient_sh: None,
            exposure: 1.0,
            ambient_occlusion: None,
            background: [0.0, 0.0, 0.0, 0.0],
            palettes: vec![],
            draws: vec![draw],
        }
    }

    fn draw() -> Draw {
        Draw {
            mesh: 1,
            model: identity(),
            palette: None,
            morphs: vec![],
            material: Material::default(),
        }
    }

    fn alpha_at(rgba: &[u8], column: usize, row: usize) -> u8 {
        rgba[(row * 16 + column) * 4 + 3]
    }

    #[test]
    fn bad_parts_are_refused() {
        let ragged = MeshParts {
            positions: vec![0; 11],
            ..MeshParts::default()
        };
        assert!(Mesh::from_parts(ragged).is_err());
        let out_of_range = MeshParts {
            positions: bytes_of(&[0.0; 9]),
            indices: [0u16, 1, 3]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        };
        assert!(
            Mesh::from_parts(out_of_range)
                .unwrap_err()
                .contains("names no vertex")
        );
        // enough targets of a big mesh to pass the GPU's storage limit: refused before a byte
        // of them is read
        let vertices = 3 << 15;
        let too_many_morphs = MeshParts {
            positions: vec![0; vertices * 12],
            morphs: vec![(Vec::new(), Vec::new()); MAX_MORPH_BYTES / (vertices * 24) + 1],
            ..MeshParts::default()
        };
        assert!(
            Mesh::from_parts(too_many_morphs)
                .unwrap_err()
                .contains("morph targets")
        );
    }

    #[test]
    fn a_mesh_without_indices_is_a_triangle_list() {
        let listed = Mesh::from_parts(MeshParts {
            positions: bytes_of(&[0.0; 9]),
            ..MeshParts::default()
        })
        .expect("three vertices are one triangle");
        assert_eq!(listed.indices, vec![0, 1, 2]);
        let ragged = MeshParts {
            positions: bytes_of(&[0.0; 6]),
            ..MeshParts::default()
        };
        assert!(
            Mesh::from_parts(ragged).is_err(),
            "two vertices make no triangle"
        );
    }

    #[test]
    fn a_facing_triangle_covers_the_middle_and_leaves_the_corners() {
        let mesh = Arc::new(triangle());
        let (width, height, rgba) = rasterize(&scene(draw()), &|_| Some(mesh.clone()), &|_| None);
        assert_eq!((width, height), (16, 16));
        assert_eq!(alpha_at(&rgba, 8, 9), 255, "the middle is covered");
        assert_eq!(alpha_at(&rgba, 0, 0), 0, "a corner is the background");
    }

    #[test]
    fn a_back_face_is_culled_unless_double_sided() {
        let mesh = Arc::new(triangle());
        let mut turned = draw();
        // mirrored in x: the winding reverses, so the triangle faces away
        turned.model[0] = -1.0;
        let (_, _, rgba) = rasterize(&scene(turned.clone()), &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(alpha_at(&rgba, 8, 9), 0, "a back face draws nothing");
        turned.material.double_sided = true;
        let (_, _, rgba) = rasterize(&scene(turned), &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(alpha_at(&rgba, 8, 9), 255, "double-sided, it draws");
    }

    #[test]
    fn a_triangle_crossing_the_camera_keeps_the_part_in_front() {
        // a floor below the camera running behind it: one corner behind (w < 0 through a matrix
        // that copies z into w). The GPU clips it and draws the floor's near part across the
        // bottom of the picture, and so must the rasteriser — which once dropped it whole
        let mesh = Arc::new(
            Mesh::from_parts(MeshParts {
                positions: bytes_of(&[-0.8, -0.25, 0.5, 0.8, -0.25, 0.5, 0.0, -0.25, -0.5]),
                ..MeshParts::default()
            })
            .unwrap(),
        );
        let mut looking = scene(Draw {
            material: Material {
                double_sided: true,
                ..Material::default()
            },
            ..draw()
        });
        // clip w = z, clip z = z / 2: depth 0.5 everywhere in front of the camera
        looking.view_proj = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let (_, _, rgba) = rasterize(&looking, &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(
            alpha_at(&rgba, 8, 15),
            255,
            "the floor in front of the camera is drawn"
        );
        assert_eq!(alpha_at(&rgba, 8, 2), 0, "and nothing above the horizon");
    }

    #[test]
    fn a_palette_moves_a_skinned_vertex_and_a_joint_past_it_is_skipped() {
        let mut parts = MeshParts {
            positions: bytes_of(&[-0.8, -0.8, 0.5, 0.8, -0.8, 0.5, 0.0, 0.8, 0.5]),
            ..MeshParts::default()
        };
        parts.joints = vec![0; 12];
        parts.weights = bytes_of(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        let mesh = Arc::new(Mesh::from_parts(parts).unwrap());
        let mut moved = identity();
        moved[12] = 5.0; // joint 0 translated far right: the triangle leaves the picture
        let mut skinned = scene(Draw {
            palette: Some(0),
            ..draw()
        });
        skinned.palettes = vec![Arc::from(&moved[..])];
        let (_, _, rgba) = rasterize(&skinned, &|_| Some(mesh.clone()), &|_| None);
        assert!(
            rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0),
            "skinned off the picture"
        );
        // a palette with no joint draws the mesh rigid, where it stands
        skinned.palettes = vec![Arc::from(&moved[..8])];
        let (_, _, rgba) = rasterize(&skinned, &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(
            alpha_at(&rgba, 8, 9),
            255,
            "a palette too short to hold a joint is rigid"
        );
    }

    /// `material` lit by one white light whose direction meets `normal` at the cosine
    /// `facing`, nothing else: its diffuse light on a white surface curving `curvature` per
    /// mesh unit.
    fn diffuse_alone(material: &Material, facing: f32, curvature: f32) -> [f32; 3] {
        let mut lit = scene(draw());
        lit.sky = [0.0; 3];
        lit.ground = [0.0; 3];
        let across = (1.0 - facing * facing).max(0.0).sqrt();
        // the light travels against (across, 0, facing); the normal is +z
        lit.lights = vec![Light::directional([-across, 0.0, -facing], [1.0; 3])];
        let quiet = Material {
            specular: 0.0,
            ..material.clone()
        };
        shade(
            &lit,
            &quiet,
            [1.0; 3],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0; 3],
            1.0,
            &[],
            [0.0; 3],
            curvature,
            1.0,
        )
    }

    fn skin() -> Material {
        Material {
            surface: Surface::Skin,
            ..Material::default()
        }
    }

    #[test]
    fn flat_skin_is_lambert_and_white() {
        // ADR-794: the profile's weights sum to 1 a channel and a flat surface has no ring to
        // blur round, so the table's first row is Lambert itself, the same in every channel
        let table = skin_table();
        assert_eq!(table.len(), SKIN_TABLE_WIDTH * SKIN_TABLE_HEIGHT);
        for (column, texel) in table[..SKIN_TABLE_WIDTH].iter().enumerate() {
            let cosine = column as f32 / (SKIN_TABLE_WIDTH - 1) as f32 * 2.0 - 1.0;
            assert_eq!(*texel, [cosine.max(0.0); 3], "column {column}");
        }
        // read between the columns it is still Lambert: its one corner is a column
        for step in 0..=400 {
            let facing = step as f32 / 200.0 - 1.0;
            let read = skin_diffuse(table, facing, 0.0);
            for channel in read {
                assert!(
                    (channel - facing.max(0.0)).abs() < 1e-6,
                    "facing {facing}: {read:?}"
                );
            }
            // and through the shading: a flat skin, or a curved one with no scattering, is a
            // standard surface's diffuse
            let plain = diffuse_alone(&Material::default(), facing, 0.0);
            let flat = diffuse_alone(&skin(), facing, 0.0);
            let unscattered = diffuse_alone(
                &Material {
                    scatter: Some(0.0),
                    ..skin()
                },
                facing,
                400.0,
            );
            for channel in 0..3 {
                assert!((flat[channel] - plain[channel]).abs() < 1e-6, "{flat:?}");
                assert!((unscattered[channel] - plain[channel]).abs() < 1e-6);
            }
        }
        // the first curved row (a radius near a metre) is the integral itself, and still
        // Lambert and white to a thousandth: the profile, normalised, only moves light
        let nearly_flat = skin_table_curvature(1);
        for step in 0..=400 {
            let facing = step as f32 / 200.0 - 1.0;
            for channel in skin_diffuse(table, facing, nearly_flat) {
                assert!((channel - facing.max(0.0)).abs() < 1e-3, "facing {facing}");
            }
        }
        // a surface sharply curved does differ: the lookup is not Lambert whatever it is given
        assert!(skin_diffuse(table, 0.0, 0.4)[0] > 0.05);
    }

    /// `light(angle)` summed over the angles 0 … π from the light, each weighted by
    /// `measure(angle)`.
    fn over_angles(light: impl Fn(f32) -> f32, measure: impl Fn(f32) -> f32) -> f32 {
        let steps = 4000;
        (0..steps)
            .map(|index| {
                let angle = (index as f32 + 0.5) / steps as f32 * std::f32::consts::PI;
                light(angle) * measure(angle) * std::f32::consts::PI / steps as f32
            })
            .sum()
    }

    #[test]
    fn skin_scattering_moves_light_and_adds_none() {
        // the table blurs Lambert round a ring, so round the ring the light is Lambert's
        // exactly (∫ max(cos, 0) over 0 … π is 1), each channel, whatever the curvature; over
        // a sphere (each angle weighted by its sine; Lambert's is ½) it is Lambert's to within
        // the little the ring's blur is not a sphere's: 1 % on a nose (a 10 mm radius), under
        // 10 % at a 2.5 mm radius
        let table = skin_table();
        for (curvature, sphere_slack) in [(0.0125f32, 0.002), (0.1, 0.01), (0.4, 0.1)] {
            for channel in 0..3 {
                let read = |angle: f32| skin_diffuse(table, angle.cos(), curvature)[channel];
                let ring = over_angles(read, |_| 1.0);
                assert!(
                    (ring - 1.0).abs() < 2e-3,
                    "curvature {curvature} channel {channel}: {ring} round the ring"
                );
                let sphere = over_angles(read, f32::sin);
                assert!(
                    (sphere - 0.5).abs() < 0.5 * sphere_slack,
                    "curvature {curvature} channel {channel}: {sphere} over the sphere"
                );
            }
        }
        // through the shading too, on a mesh in metres: a finger's 8 mm radius is 125 a metre
        for channel in 0..3 {
            let ring = over_angles(
                |angle| diffuse_alone(&skin(), angle.cos(), 125.0)[channel],
                |_| 1.0,
            );
            assert!((ring - 1.0).abs() < 2e-3, "channel {channel}: {ring}");
        }
    }

    /// How far past the terminator (as the cosine to the light, 0 … 1 behind it) `channel` of
    /// a skin surface curving `curvature` per metre still gives back over 1 % of the light.
    fn band(curvature: f32, channel: usize) -> f32 {
        (0..1000)
            .map(|step| step as f32 / 1000.0)
            .filter(|behind| diffuse_alone(&skin(), -behind, curvature)[channel] > 0.01)
            .fold(0.0, f32::max)
    }

    #[test]
    fn red_reaches_past_the_terminator_furthest_and_the_band_narrows_as_the_surface_flattens() {
        // a finger (8 mm radius: 125 a metre): red light travels furthest under skin, so the
        // terminator's band is red, then green, then blue
        let finger = diffuse_alone(&skin(), -0.1, 125.0);
        assert!(
            finger[0] > 0.015 && finger[0] > 10.0 * finger[1] && finger[1] > finger[2],
            "a finger just past the terminator: {finger:?}"
        );
        assert!(band(125.0, 0) > 0.15 && band(125.0, 0) > band(125.0, 2) + 0.15);
        // the band narrows as the curvature falls: an ear's rim (2.5 mm), a finger, a nose
        // (10 mm), a thigh (80 mm), where nothing shows past the terminator at all — the fault
        // the wrapped diffuse had, which reddened a knee as much as a nostril
        let bands = [400.0, 125.0, 100.0, 12.5].map(|curvature| band(curvature, 0));
        assert!(
            bands.windows(2).all(|pair| pair[0] > pair[1]),
            "narrower as it flattens: {bands:?}"
        );
        assert_eq!(bands[3], 0.0, "a thigh has no band");
        let thigh = diffuse_alone(&skin(), 0.0, 12.5);
        assert!(thigh[0] < 0.006, "a thigh at the terminator: {thigh:?}");
        // on the lit side the red it moved is missing: light is moved, not added
        let lit = diffuse_alone(&skin(), 1.0, 400.0);
        assert!(
            lit[0] < 0.95 && lit[2] > 0.99,
            "an ear's rim head on: {lit:?}"
        );
        // `:scatter` scales the curvature: twice the scattering on a nose is a 5 mm radius
        let doubled = Material {
            scatter: Some(2.0),
            ..skin()
        };
        assert_eq!(
            diffuse_alone(&doubled, -0.05, 100.0),
            diffuse_alone(&skin(), -0.05, 200.0)
        );
    }

    #[test]
    fn a_mesh_knows_how_sharply_it_curves() {
        // a sphere curves 1 over its radius everywhere; a flat sheet not at all
        let unit = sphere(12, 24);
        assert_eq!(unit.curvature.len(), unit.positions.len());
        for curvature in &unit.curvature {
            assert!((curvature - 1.0).abs() < 1e-3, "a unit sphere: {curvature}");
        }
        let half: Vec<[f32; 3]> = unit
            .positions
            .iter()
            .map(|position| position.map(|coordinate| coordinate * 0.5))
            .collect();
        for curvature in vertex_curvature(&half, &unit.normals, &unit.indices) {
            assert!(
                (curvature - 2.0).abs() < 2e-3,
                "half the radius: {curvature}"
            );
        }
        assert_eq!(triangle().curvature, vec![0.0; 3]);
        // a strip along x that bends more and more: flat from 0 to 1, then its normal turns
        let tilt = [0.0f32, 0.0, 0.1, 0.4];
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        for (column, angle) in tilt.iter().enumerate() {
            for row in 0..2 {
                positions.push([column as f32, row as f32, 0.0]);
                normals.push([angle.sin(), 0.0, angle.cos()]);
            }
        }
        let mut indices = Vec::new();
        for column in 0..3u32 {
            let corner = column * 2;
            indices.extend([
                corner,
                corner + 2,
                corner + 3,
                corner,
                corner + 3,
                corner + 1,
            ]);
        }
        let welded = vertex_curvature(&positions, &normals, &indices);
        assert!(
            welded[0] < welded[2] && welded[2] < welded[4] && welded[4] < welded[6],
            "sharper along the strip: {welded:?}"
        );
        // the same strip with every triangle's corners its own vertices (a seam along every
        // edge): vertices at one position are one point of the surface, with one curvature
        let soup_positions: Vec<[f32; 3]> = indices
            .iter()
            .map(|index| positions[*index as usize])
            .collect();
        let soup_normals: Vec<[f32; 3]> = indices
            .iter()
            .map(|index| normals[*index as usize])
            .collect();
        let soup_indices: Vec<u32> = (0..indices.len() as u32).collect();
        let soup = vertex_curvature(&soup_positions, &soup_normals, &soup_indices);
        for (corner, index) in indices.iter().enumerate() {
            assert!(
                (soup[corner] - welded[*index as usize]).abs() < 1e-6,
                "corner {corner}: {} against {}",
                soup[corner],
                welded[*index as usize]
            );
        }
    }

    #[test]
    fn a_painted_skin_sphere_scatters_by_its_own_curvature() {
        // a unit sphere lit from −x, seen from −z: past the terminator (x > 0 on its face) a
        // skin a metre in radius is as dark as a standard surface — no band — while the same
        // sphere scattering 400 times as far (a 2.5 mm radius's skin) shows a red one, read
        // from the curvature the mesh found for itself
        let painted = |material: Material| {
            let mut lit = scene(Draw {
                material: Material {
                    specular: 0.0,
                    ..material
                },
                ..draw()
            });
            lit.w = 128.0;
            lit.h = 128.0;
            lit.view_proj = from_rows([
                [1.0 / 1.5, 0.0, 0.0, 0.0],
                [0.0, 1.0 / 1.5, 0.0, 0.0],
                [0.0, 0.0, 0.25, 0.5],
                [0.0, 0.0, 0.0, 1.0],
            ]);
            lit.sky = [0.0; 3];
            lit.ground = [0.0; 3];
            lit.lights = vec![Light::directional([1.0, 0.0, 0.0], [2.0; 3])];
            paint(&lit, &HashMap::from([(1, Arc::new(sphere(48, 96)))]))
        };
        // the middle row, a quarter of the way past the terminator (world x 0.25)
        let at = (64 * 128 + (64.0 + 0.25 / 1.5 * 64.0) as usize) * 4;
        let pixel = |rgba: &[u8]| [rgba[at], rgba[at + 1], rgba[at + 2]];
        let standard = pixel(&painted(Material::default()));
        assert_eq!(standard, [0, 0, 0], "Lambert is dark past the terminator");
        assert_eq!(pixel(&painted(skin())), standard, "and so is a 1 m skin");
        let small = pixel(&painted(Material {
            scatter: Some(400.0),
            ..skin()
        }));
        assert!(
            small[0] > 40 && small[0] > small[1] + 30 && small[1] >= small[2],
            "a red band: {small:?}"
        );
    }

    #[test]
    fn a_mip_level_averages_by_coverage_and_lets_no_transparent_colour_in() {
        // red opaque, a transparent green, blue opaque, white half covered
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 0, //
            0, 0, 255, 255, 255, 255, 255, 127,
        ];
        let levels = mip_chain(&rgba, 2, 2, None);
        assert_eq!(levels.len(), 1, "2×2 down to 1×1");
        let texel = &levels[0].rgba;
        // the colour weighed by alpha (1, 0, 1, about 0.5 of 2.5): red and blue 1.5 / 2.5, green
        // 0.5 / 2.5 (none from the transparent texel); alpha the plain average
        assert_eq!(texel.as_slice(), &[153, 51, 153, 159]);
        assert_eq!(
            mip_chain(&[0; 4 * 5 * 3], 5, 3, None).len(),
            2,
            "5×3 → 2×1 → 1×1"
        );
    }

    #[test]
    fn coverage_mips_keep_fine_strands_as_covered_as_up_close() {
        // a 16×16 texture of 1-texel strands, every fourth column opaque: a quarter of it
        // covered. Two levels down each texel averages four columns, a quarter alpha: under a
        // half everywhere, so plain mips lose every strand; coverage mips scale it back up.
        let rgba: Vec<u8> = (0..16 * 16)
            .flat_map(|index| {
                let alpha = if index % 16 % 4 == 0 { 255 } else { 0 };
                [200, 180, 120, alpha]
            })
            .collect();
        let plain = mip_chain(&rgba, 16, 16, None);
        let kept = mip_chain(&rgba, 16, 16, Some(COVERAGE_CUTOFF));
        let alpha = |levels: &[MipLevel]| levels[1].rgba[3];
        assert_eq!(alpha(&plain), 64, "plain mips: a quarter alpha");
        assert!(
            alpha(&kept) >= 120,
            "coverage mips bring it back to about the cutoff: {}",
            alpha(&kept)
        );
        assert_eq!(
            plain[1]
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|texel| texel[0])
                .collect::<Vec<_>>(),
            kept[1]
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|texel| texel[0])
                .collect::<Vec<_>>(),
            "colour untouched"
        );
    }

    #[test]
    fn a_fine_texture_seen_small_reads_as_its_average() {
        // a 1-texel black and white checker, 61 texels across a quad drawn over a 16-pixel
        // picture: nearly four texels to a pixel each way, read from its mip levels 1 and 2,
        // where every texel is the checker's grey (without them it would alias into bands)
        let side = 61usize;
        let rgba: Vec<u8> = (0..side * side)
            .flat_map(|index| {
                let value = if (index / side + index % side).is_multiple_of(2) {
                    255
                } else {
                    0
                };
                [value, value, value, 255]
            })
            .collect();
        let levels = mip_chain(&rgba, side as u32, side as u32, None);
        let mut parts = MeshParts {
            positions: bytes_of(&[
                -1.0, -1.0, 0.5, 1.0, -1.0, 0.5, 1.0, 1.0, 0.5, -1.0, 1.0, 0.5,
            ]),
            uvs: bytes_of(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]),
            ..MeshParts::default()
        };
        parts.indices = [0u16, 1, 2, 0, 2, 3]
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        let mesh = Arc::new(Mesh::from_parts(parts).unwrap());
        let mut seen = scene(Draw {
            material: Material {
                base: Some(3),
                double_sided: true,
                ..Material::default()
            },
            ..draw()
        });
        seen.lights = Vec::new();
        seen.sky = [1.0; 3];
        seen.ground = [1.0; 3];
        let picture = rasterize(&seen, &|_| Some(mesh.clone()), &|handle| {
            (handle == 3).then_some(TextureRef {
                w: side as u32,
                h: side as u32,
                rgba: &rgba,
                levels: &levels,
            })
        })
        .2;
        let reds = |picture: &[u8]| -> Vec<i32> {
            (2..14)
                .map(|column| i32::from(picture[(8 * 16 + column) * 4]))
                .collect()
        };
        let row = reds(&picture);
        let spread = row.iter().max().unwrap() - row.iter().min().unwrap();
        assert!(spread <= 2, "an even grey across the picture: {row:?}");
        assert!(
            row.iter().all(|red| (60..200).contains(red)),
            "grey, neither the checker's black nor its white: {row:?}"
        );
    }

    #[test]
    fn strand_samples_are_spread_evenly_and_differ_from_point_to_point() {
        // over many points a strand of half coverage takes about half the samples, and two
        // strands at different points take different ones (so overlapping strands add up)
        let points: Vec<[f32; 3]> = (0..4000)
            .map(|index| {
                let index = index as f32;
                [index * 0.000_37, 1.5 + index * 0.000_11, index * 0.000_23]
            })
            .collect();
        let taken = points
            .iter()
            .flat_map(|point| (0..4).map(|sample| strand_sample_step(*point, sample)))
            .filter(|step| *step < 0.5)
            .count() as f32
            / (points.len() * 4) as f32;
        assert!(
            (taken - 0.5).abs() < 0.04,
            "half the samples at half coverage: {taken}"
        );
        let differ = points
            .windows(2)
            .filter(|pair| strand_sample_step(pair[0], 0) != strand_sample_step(pair[1], 0))
            .count();
        assert!(
            differ > points.len() * 9 / 10,
            "neighbouring points pick their own"
        );
    }

    #[test]
    fn a_strand_faces_the_camera_and_is_never_under_a_pixel_wide() {
        // seen from z = −10 through the identity camera onto 16 pixels: a pixel is 2/16 = 0.125 wide
        let seen = scene(draw());
        let up = [0.0, 1.0, 0.0];
        let wide = strand_vertex(&seen, [0.0, 0.0, 0.5], up, 1.0, 0.5);
        assert!(
            (wide.0[0] + 0.25).abs() < 1e-5,
            "half its width to the side: {:?}",
            wide.0
        );
        assert_eq!(wide.2, 1.0, "wider than a pixel: fully covered");
        // turned to face the camera: its normal across the ribbon, toward the eye
        assert!(wide.1[2] < 0.0 && wide.1[0] < 0.0, "{:?}", wide.1);
        let thin = strand_vertex(&seen, [0.0, 0.0, 0.5], up, -1.0, 0.025);
        assert!(
            (thin.0[0] - 0.0625).abs() < 1e-5,
            "drawn a pixel wide: {:?}",
            thin.0
        );
        assert!(
            (thin.2 - 0.2).abs() < 1e-4,
            "covering its share of it: {}",
            thin.2
        );
    }

    #[test]
    fn hair_shines_across_its_strands_and_not_along_them() {
        let lit = scene(draw());
        let hair = Material {
            surface: Surface::Hair,
            roughness: 0.3,
            specular: 1.0,
            ..Material::default()
        };
        // lit and seen straight on (half vector +z): a strand across it (along x) shines, one
        // pointing along it does not
        let normal = [0.0, 0.0, 1.0];
        let shine = |strand: [f32; 3]| {
            shade(
                &lit,
                &hair,
                [0.2; 3],
                normal,
                normal,
                [0.0; 3],
                1.0,
                &[],
                strand,
                0.0,
                1.0,
            )[0]
        };
        let across = shine([1.0, 0.0, 0.0]);
        let along = shine(normalize([0.0, 0.08, 1.0]));
        assert!(
            across > along + 0.3,
            "across its strands {across}, along them {along}"
        );
    }

    #[test]
    fn hair_covers_as_much_of_a_pixel_as_its_alpha() {
        // a hair card whose alpha is the cutoff: coverage one half, so two of each pixel's
        // 2×2 sub-samples are drawn and the picture is half covered
        let mut half = scene(Draw {
            material: Material {
                surface: Surface::Hair,
                cutoff: 0.5,
                color: [1.0, 1.0, 1.0, 0.5],
                ..Material::default()
            },
            ..draw()
        });
        half.background = [0.0; 4];
        let mesh = Arc::new(triangle());
        let (_, _, rgba) = rasterize(&half, &|_| Some(mesh.clone()), &|_| None);
        let alpha = alpha_at(&rgba, 8, 9);
        assert!(
            (alpha as i32 - 128).abs() <= 1,
            "half the samples covered: alpha {alpha}"
        );
        // past one and a half cutoffs, all of it
        half.draws[0].material.color[3] = 0.8;
        let (_, _, rgba) = rasterize(&half, &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(alpha_at(&rgba, 8, 9), 255, "fully covered");
    }

    #[test]
    fn a_zero_light_direction_shades_as_the_shader_does() {
        // the shader's `safe_normalize` turns a zero vector into +z, as `normalize` here does
        let mut zero = scene(draw());
        zero.lights[0].direction = [0.0, 0.0, 0.0];
        let lit = shade(
            &zero,
            &Material::default(),
            [1.0; 3],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0; 3],
            1.0,
            &[],
            [0.0; 3],
            0.0,
            1.0,
        );
        assert!(lit.iter().all(|channel| channel.is_finite()));
    }

    // --- lights and shadows (ADR-788) -----------------------------------------------------------

    /// A light of colour 1 on a white surface facing it, through `shade` with nothing else
    /// lighting it: what arrives.
    fn arriving(light: Light, world: [f32; 3], normal: [f32; 3]) -> f32 {
        let mut lit = scene(draw());
        lit.lights = vec![light];
        lit.sky = [0.0; 3];
        lit.ground = [0.0; 3];
        let material = Material {
            specular: 0.0,
            ..Material::default()
        };
        shade(
            &lit,
            &material,
            [1.0; 3],
            normal,
            normal,
            world,
            1.0,
            &[],
            [0.0; 3],
            0.0,
            1.0,
        )[0]
    }

    #[test]
    fn a_point_light_falls_off_with_the_square_of_the_distance_and_ends_at_its_range() {
        let up = [0.0, 1.0, 0.0];
        let unbounded = Light::point([0.0, 0.0, 0.0], [1.0; 3], 0.0);
        // `:color` is the light 1 m away; at half a metre four times it, at 2 m a quarter
        let at = |distance: f32| arriving(unbounded.clone(), [0.0, -distance, 0.0], up);
        assert!((at(1.0) - 1.0).abs() < 1e-5, "1 m: {}", at(1.0));
        assert!((at(0.5) - 4.0).abs() < 1e-4, "0.5 m: {}", at(0.5));
        assert!((at(2.0) - 0.25).abs() < 1e-5, "2 m: {}", at(2.0));
        // a range windows it: nearly the inverse square close in, nothing at or past the range
        let ranged = Light::point([0.0, 0.0, 0.0], [1.0; 3], 4.0);
        let near = arriving(ranged.clone(), [0.0, -0.5, 0.0], up);
        assert!(
            (near - 4.0).abs() < 0.01,
            "close in, the window is ~1: {near}"
        );
        let toward_the_end = arriving(ranged.clone(), [0.0, -3.9, 0.0], up);
        assert!(toward_the_end > 0.0 && toward_the_end < 0.25 / 3.9 / 3.9);
        assert_eq!(arriving(ranged.clone(), [0.0, -4.0, 0.0], up), 0.0);
        assert_eq!(arriving(ranged, [0.0, -9.0, 0.0], up), 0.0);
        // a surface on the bulb is bright, not infinite
        assert!(at(0.0).is_finite());
    }

    #[test]
    fn a_spot_lights_its_inner_cone_fully_and_nothing_past_its_outer() {
        // pointing straight down from 1 m up; inner 0.3 rad, outer 0.5 rad
        let spot = Light::spot([0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [1.0; 3], 0.0, [0.3, 0.5]);
        let point = Light::point([0.0, 1.0, 0.0], [1.0; 3], 0.0);
        let at_angle = |light: &Light, angle: f32| {
            // a point on the unit sphere about the light, its surface facing the light
            let offset = [angle.sin(), -angle.cos(), 0.0];
            let world = [offset[0], 1.0 + offset[1], 0.0];
            arriving(light.clone(), world, [-offset[0], -offset[1], 0.0])
        };
        for inside in [0.0, 0.2, 0.299] {
            let full = at_angle(&point, inside);
            assert!(
                (at_angle(&spot, inside) - full).abs() < 1e-4,
                "inside the inner cone ({inside}) a spot is its point light"
            );
        }
        let between = at_angle(&spot, 0.4);
        assert!(
            between > 0.05 && between < 0.95,
            "between the cones: {between}"
        );
        assert!(
            at_angle(&spot, 0.35) > at_angle(&spot, 0.45),
            "falling outward"
        );
        for outside in [0.5, 0.6, 1.5, 3.0] {
            assert_eq!(
                at_angle(&spot, outside),
                0.0,
                "past the outer cone ({outside})"
            );
        }
    }

    /// A quad of `corners` (counter-clockwise seen from where its normal points), its UVs
    /// running 0 … 1 from the first corner.
    fn quad(corners: [[f32; 3]; 4]) -> Mesh {
        let normal = normalize(cross(
            subtract(corners[1], corners[0]),
            subtract(corners[2], corners[0]),
        ));
        let positions: Vec<f32> = corners.iter().flatten().copied().collect();
        let normals: Vec<f32> = (0..4).flat_map(|_| normal).collect();
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&positions),
            normals: bytes_of(&normals),
            uvs: bytes_of(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]),
            indices: [0u16, 1, 2, 0, 2, 3]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        })
        .expect("a quad")
    }

    /// An 8 m floor at y = 0 and a 1 m square card held 1 m above its middle (mesh 2), seen
    /// straight down by an orthographic camera onto a 128-pixel picture (16 px a metre: world x
    /// is column `64 + 16x`, world z row `64 + 16z`), lit by `light` alone.
    fn floor_and_card(light: Light) -> (Scene, HashMap<u32, Arc<Mesh>>) {
        let floor = quad([
            [-4.0, 0.0, 4.0],
            [4.0, 0.0, 4.0],
            [4.0, 0.0, -4.0],
            [-4.0, 0.0, -4.0],
        ]);
        let card = quad([
            [-0.5, 1.0, 0.5],
            [0.5, 1.0, 0.5],
            [0.5, 1.0, -0.5],
            [-0.5, 1.0, -0.5],
        ]);
        let solid = Material {
            double_sided: true,
            specular: 0.0,
            ..Material::default()
        };
        let mut seen = scene(Draw {
            material: solid.clone(),
            ..draw()
        });
        seen.draws.push(Draw {
            mesh: 2,
            material: solid,
            ..draw()
        });
        seen.w = 128.0;
        seen.h = 128.0;
        // x → clip x / 4, z → clip −y / 4 (z grows down the picture), height → depth
        seen.view_proj = from_rows([
            [0.25, 0.0, 0.0, 0.0],
            [0.0, 0.0, -0.25, 0.0],
            [0.0, -1.0 / 6.0, 0.0, 0.5],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        seen.eye = [0.0, 10.0, 0.0];
        seen.lights = vec![light];
        seen.sky = [0.05; 3];
        seen.ground = [0.05; 3];
        let meshes = HashMap::from([(1, Arc::new(floor)), (2, Arc::new(card))]);
        (seen, meshes)
    }

    fn paint(scene: &Scene, meshes: &HashMap<u32, Arc<Mesh>>) -> Vec<u8> {
        rasterize(scene, &|handle| meshes.get(&handle).cloned(), &|_| None).2
    }

    fn red_at(rgba: &[u8], world_x: f32, world_z: f32) -> u8 {
        let column = (64.0 + 16.0 * world_x) as usize;
        let row = (64.0 + 16.0 * world_z) as usize;
        rgba[(row * 128 + column) * 4]
    }

    fn shadowed(mut light: Light) -> Light {
        light.shadow = Some(Shadow::default());
        light
    }

    /// The light 45° from straight down, travelling toward −x: the card's shadow falls 1 m
    /// toward −x of it, on x −1.5 … −0.5.
    fn slanted() -> Light {
        Light::directional([-1.0, -1.0, 0.0], [1.0; 3])
    }

    #[test]
    fn a_card_shadows_the_floor_under_a_directional_light() {
        let (mut lit, meshes) = floor_and_card(shadowed(slanted()));
        let with_shadow = paint(&lit, &meshes);
        lit.lights[0].shadow = None;
        let without = paint(&lit, &meshes);
        let (dark, bright) = (red_at(&with_shadow, -1.0, 0.0), red_at(&without, -1.0, 0.0));
        assert!(
            u16::from(dark) + 60 < u16::from(bright),
            "the floor in the card's shadow is dark: {dark} against {bright} unshadowed"
        );
        assert_eq!(
            red_at(&with_shadow, 2.0, 2.0),
            red_at(&without, 2.0, 2.0),
            "the floor away from it is lit"
        );
    }

    #[test]
    fn a_soft_shadow_grades_its_edge_and_keeps_its_middle_dark_without_acne() {
        // the card's shadow on x −1.5 … −0.5, its edges filtered over 0.3 m
        let (mut scene, meshes) = floor_and_card(shadowed(slanted()));
        let hard = paint(&scene, &meshes);
        scene.lights[0].shadow.as_mut().expect("shadowed").softness = 0.3;
        let soft = paint(&scene, &meshes);
        scene.lights[0].shadow = None;
        let without = paint(&scene, &meshes);
        let red = |picture: &[u8], x: f32| i32::from(red_at(picture, x, 0.0));
        // just inside its edge a soft shadow is lighter, just outside it darker, than a hard one
        assert!(
            red(&soft, -1.35) > red(&hard, -1.35) + 10,
            "inside the edge: soft {} against hard {}",
            red(&soft, -1.35),
            red(&hard, -1.35)
        );
        assert!(
            red(&soft, -1.65) + 5 < red(&hard, -1.65),
            "outside the edge: soft {} against hard {}",
            red(&soft, -1.65),
            red(&hard, -1.65)
        );
        // still a shadow in its middle, and the floor well away from it lit as without one
        assert!(
            red(&soft, -1.0) + 40 < red(&without, -1.0),
            "its middle stays dark"
        );
        // the edge filtered over 0.3 m as the light sees it lies across 0.3 × √2 of floor at 45°,
        // and the slope bias (0.3 m toward the light) carries it ~0.2 m further
        no_acne(&soft, &without, -1.5..-0.5, 0.75);
    }

    #[test]
    fn ambient_occlusion_darkens_the_floor_beside_a_low_card_and_nothing_open() {
        // the floor lit by its ambient alone, a 1 m card lying 10 cm over its middle
        // (most taps of a 0.3 m hemisphere rise above a card that low, and the tone curve presses
        // the difference at this brightness: the floor beside it darkens by about 9 of 255)
        let (mut scene, mut meshes) = floor_and_card(slanted());
        meshes.insert(
            2,
            Arc::new(quad([
                [-0.5, 0.1, 0.5],
                [0.5, 0.1, 0.5],
                [0.5, 0.1, -0.5],
                [-0.5, 0.1, -0.5],
            ])),
        );
        scene.lights = Vec::new();
        scene.sky = [0.5; 3];
        scene.ground = [0.5; 3];
        let without = paint(&scene, &meshes);
        scene.ambient_occlusion = Some(AmbientOcclusion {
            radius: 0.3,
            strength: 1.0,
        });
        let with = paint(&scene, &meshes);
        let red = |picture: &[u8], x: f32| i32::from(red_at(picture, x, 0.0));
        assert!(
            red(&with, -0.55) + 5 < red(&without, -0.55),
            "the floor tucked beside the card's edge darkens: {} against {}",
            red(&with, -0.55),
            red(&without, -0.55)
        );
        assert!(
            red(&with, -0.55) < red(&with, -0.75),
            "more the closer it is to the edge"
        );
        // open floor, a radius and more from the card, is the same as without: a flat surface
        // does not occlude itself
        for x in [-2.5f32, -1.5, 2.0] {
            assert_eq!(red(&with, x), red(&without, x), "open floor at x {x}");
        }
        // the card's own top, open to the sky, is untouched too
        assert_eq!(red(&with, 0.0), red(&without, 0.0), "the card's top");
    }

    /// Every floor pixel of `with_shadow` and `without` outside the card and its shadow (and a
    /// band about them, `margin` metres, for the filter's softening) the same: no acne.
    fn no_acne(with_shadow: &[u8], without: &[u8], shadow_x: std::ops::Range<f32>, margin: f32) {
        let mut worst = 0;
        for row in 0..128usize {
            for column in 0..128usize {
                let (world_x, world_z) = (
                    (column as f32 + 0.5 - 64.0) / 16.0,
                    (row as f32 + 0.5 - 64.0) / 16.0,
                );
                let near_card = world_x > shadow_x.start.min(-0.5) - margin
                    && world_x < shadow_x.end.max(0.5) + margin
                    && world_z.abs() < 0.5 + margin;
                if near_card {
                    continue;
                }
                let at = (row * 128 + column) * 4;
                let difference = (i32::from(with_shadow[at]) - i32::from(without[at])).abs();
                worst = worst.max(difference);
            }
        }
        assert_eq!(
            worst, 0,
            "a lit floor pixel darkened by its own shadow (acne)"
        );
    }

    #[test]
    fn a_lit_floor_does_not_shadow_itself_at_any_angle_of_light() {
        // from straight down to 80° off: the receiver's bias grows with the slope and its
        // normal offset, so no lit texel falls behind its own depth
        for degrees in [0.0f32, 30.0, 60.0, 80.0] {
            let angle = degrees.to_radians();
            let light = Light::directional([-angle.sin(), -angle.cos(), 0.0], [1.0; 3]);
            let (mut lit, meshes) = floor_and_card(shadowed(light));
            let with_shadow = paint(&lit, &meshes);
            lit.lights[0].shadow = None;
            let without = paint(&lit, &meshes);
            let reach = angle.tan();
            no_acne(&with_shadow, &without, -0.5 - reach..0.5 - reach, 0.25);
        }
    }

    #[test]
    fn a_spot_and_a_point_light_shadow_the_floor_without_acne() {
        // 2 m up over the floor's middle: the card at 1 m casts a shadow twice its size, x and z
        // −1 … 1, seen past the card's own −0.5 … 0.5
        let spot = shadowed(Light::spot(
            [0.0, 2.0, 0.0],
            [0.0, -1.0, 0.0],
            [8.0; 3],
            0.0,
            [1.2, 1.4],
        ));
        let point = shadowed(Light::point([0.0, 2.0, 0.0], [8.0; 3], 0.0));
        for light in [spot, point] {
            let (mut lit, meshes) = floor_and_card(light);
            let with_shadow = paint(&lit, &meshes);
            lit.lights[0].shadow = None;
            let without = paint(&lit, &meshes);
            let (dark, bright) = (red_at(&with_shadow, 0.75, 0.0), red_at(&without, 0.75, 0.0));
            assert!(
                u16::from(dark) + 60 < u16::from(bright),
                "{:?}: shadowed {dark} against {bright}",
                lit.lights[0].kind
            );
            // the point light's floor runs across five of its cube's faces, seams included
            no_acne(&with_shadow, &without, -1.0..1.0, 0.6);
        }
    }

    #[test]
    fn a_shadow_starts_where_its_caster_stands() {
        // a 1 m wall standing on the floor along z at x = 0, the light from +x at 45°: its
        // shadow runs from the wall's foot to x = −1 — and must reach the foot (no
        // peter-panning) within a few centimetres
        let (mut lit, mut meshes) = floor_and_card(shadowed(slanted()));
        meshes.insert(
            2,
            Arc::new(quad([
                [0.0, 0.0, 1.0],
                [0.0, 0.0, -1.0],
                [0.0, 1.0, -1.0],
                [0.0, 1.0, 1.0],
            ])),
        );
        let with_shadow = paint(&lit, &meshes);
        lit.lights[0].shadow = None;
        let without = paint(&lit, &meshes);
        // the pixel whose centre is 9 cm from the foot, and one well inside
        for world_x in [-0.09, -0.5] {
            let (dark, bright) = (
                red_at(&with_shadow, world_x, 0.0),
                red_at(&without, world_x, 0.0),
            );
            assert!(
                u16::from(dark) + 60 < u16::from(bright),
                "{world_x} m from the foot: {dark} against {bright}"
            );
        }
        assert_eq!(
            red_at(&with_shadow, 0.1, 0.0),
            red_at(&without, 0.1, 0.0),
            "the lit side of the wall"
        );
    }

    #[test]
    fn a_skinned_caster_shadows_where_its_pose_puts_it() {
        // the card skinned to joint 0, which the palette carries 2 m toward +x and back 1 m in
        // z: its shadow moves with it, to x 0.5 … 1.5, z 0.5 … 1.5
        let (mut lit, mut meshes) = floor_and_card(shadowed(slanted()));
        let card = &meshes[&2];
        let mut parts = MeshParts {
            positions: bytes_of(&card.positions.iter().flatten().copied().collect::<Vec<_>>()),
            normals: bytes_of(&card.normals.iter().flatten().copied().collect::<Vec<_>>()),
            indices: card
                .indices
                .iter()
                .flat_map(|index| (*index as u16).to_le_bytes())
                .collect(),
            ..MeshParts::default()
        };
        parts.joints = vec![0; 16];
        parts.weights = bytes_of(&[1.0, 0.0, 0.0, 0.0].repeat(4));
        meshes.insert(2, Arc::new(Mesh::from_parts(parts).unwrap()));
        let mut moved = identity();
        moved[12] = 2.0;
        moved[14] = 1.0;
        lit.palettes = vec![Arc::from(&moved[..])];
        lit.draws[1].palette = Some(0);
        let with_shadow = paint(&lit, &meshes);
        lit.lights[0].shadow = None;
        let without = paint(&lit, &meshes);
        let (dark, bright) = (red_at(&with_shadow, 1.0, 1.0), red_at(&without, 1.0, 1.0));
        assert!(
            u16::from(dark) + 60 < u16::from(bright),
            "under the posed card: {dark} against {bright}"
        );
        assert_eq!(
            red_at(&with_shadow, -1.0, 0.0),
            red_at(&without, -1.0, 0.0),
            "nothing where the card's bind pose would have cast"
        );
    }

    #[test]
    fn a_draw_that_casts_no_shadow_leaves_the_floor_lit() {
        let (mut lit, meshes) = floor_and_card(shadowed(slanted()));
        lit.draws[1].material.cast_shadow = false;
        let with_shadow = paint(&lit, &meshes);
        lit.lights[0].shadow = None;
        let without = paint(&lit, &meshes);
        assert_eq!(with_shadow, without, ":cast-shadow false casts nothing");
    }

    #[test]
    fn a_cutout_or_blended_card_casts_only_where_its_alpha_reaches_its_cutoff() {
        // a texture opaque on its left half (u < 0.5) and clear on its right: the card's
        // shadow is its left half's (x −1.5 … −1), not its right's (x −1 … −0.5)
        let texture = [
            255u8, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 255, 255, 255, 0,
        ];
        for surface in [Surface::Cutout, Surface::Blend] {
            let (mut lit, meshes) = floor_and_card(shadowed(slanted()));
            lit.draws[1].material.surface = surface;
            lit.draws[1].material.base = Some(7);
            let textures = |handle: u32| {
                (handle == 7).then_some(TextureRef {
                    w: 4,
                    h: 1,
                    rgba: &texture,
                    levels: &[],
                })
            };
            let paint_with = |scene: &Scene| {
                rasterize(scene, &|handle| meshes.get(&handle).cloned(), &textures).2
            };
            let with_shadow = paint_with(&lit);
            lit.lights[0].shadow = None;
            let without = paint_with(&lit);
            let (dark, bright) = (red_at(&with_shadow, -1.3, 0.0), red_at(&without, -1.3, 0.0));
            assert!(
                u16::from(dark) + 60 < u16::from(bright),
                "{surface:?}: the opaque half casts"
            );
            assert_eq!(
                red_at(&with_shadow, -0.7, 0.0),
                red_at(&without, -0.7, 0.0),
                "{surface:?}: the clear half does not"
            );
        }
    }

    #[test]
    fn a_fitted_directional_shadow_covers_every_posed_vertex() {
        // the bounds both painters fit a directional map to hold the skinned and morphed
        // vertices, however the palette turns them
        let mut parts = MeshParts {
            positions: bytes_of(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            morphs: vec![(
                bytes_of(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 3.0]),
                Vec::new(),
            )],
            ..MeshParts::default()
        };
        parts.joints = vec![0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0];
        parts.weights = bytes_of(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0]);
        let mesh = Mesh::from_parts(parts).unwrap();
        let mut turned = identity();
        // joint 1 turned a quarter about y and lifted
        turned[0] = 0.0;
        turned[2] = -1.0;
        turned[8] = 1.0;
        turned[10] = 0.0;
        turned[13] = 2.0;
        let mut posed = scene(Draw {
            palette: Some(0),
            morphs: vec![(0, 0.5)],
            ..draw()
        });
        posed.palettes = vec![Arc::from([identity(), turned].concat().as_slice())];
        let bounds = draw_bounds(&posed, &posed.draws[0], &mesh.bounds).unwrap();
        for vertex in pose_vertices(&posed, &mesh, &posed.draws[0]) {
            for axis in 0..3 {
                assert!(
                    vertex.world[axis] >= bounds[0][axis] - 1e-5
                        && vertex.world[axis] <= bounds[1][axis] + 1e-5,
                    "{:?} outside {bounds:?}",
                    vertex.world
                );
            }
        }
    }

    #[test]
    fn every_morph_a_draw_names_is_blended() {
        // a face blends a smile, a blink and speech at once: twenty targets, none dropped
        let count = 20u32;
        let parts = MeshParts {
            positions: bytes_of(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            morphs: (0..count)
                .map(|_| {
                    (
                        bytes_of(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.01, 0.0]),
                        Vec::new(),
                    )
                })
                .collect(),
            ..MeshParts::default()
        };
        let mesh = Mesh::from_parts(parts).unwrap();
        let morphed = scene(Draw {
            morphs: (0..count).map(|target| (target, 1.0)).collect(),
            ..draw()
        });
        let vertices = pose_vertices(&morphed, &mesh, &morphed.draws[0]);
        assert!(
            (vertices[2].world[1] - (1.0 + 0.01 * count as f32)).abs() < 1e-5,
            "every target moved it: {:?}",
            vertices[2].world
        );
    }

    // --- soft shadows, SH ambient, Fresnel and lobes, occlusion (ADR-793) -----------------------

    /// How many pixels of `rgba` along the floor line at `world_x`, z `from` … `to`, are
    /// between the shadow's dark and the lit floor's bright (8 levels in from each): the
    /// penumbra's width there, in pixels (16 a metre).
    fn penumbra_across_z(rgba: &[u8], world_x: f32, from: f32, to: f32) -> usize {
        let (dark, bright) = (red_at(rgba, world_x, 0.0), red_at(rgba, world_x, 2.0));
        assert!(
            u16::from(dark) + 60 < u16::from(bright),
            "x {world_x}: a shadow ({dark}) to measure against the lit floor ({bright})"
        );
        let rows = ((to - from) * 16.0) as usize;
        (0..rows)
            .filter(|step| {
                let red = red_at(rgba, world_x, from + *step as f32 / 16.0);
                red > dark.saturating_add(8) && red < bright.saturating_sub(8)
            })
            .count()
    }

    /// The same along the floor line at `world_z`, x `from` … `to`.
    fn penumbra_across_x(rgba: &[u8], world_z: f32, from: f32, to: f32) -> usize {
        let (dark, bright) = (red_at(rgba, -1.0, world_z), red_at(rgba, -2.5, world_z));
        assert!(u16::from(dark) + 60 < u16::from(bright));
        let columns = ((to - from) * 16.0) as usize;
        (0..columns)
            .filter(|step| {
                let red = red_at(rgba, from + *step as f32 / 16.0, world_z);
                red > dark.saturating_add(8) && red < bright.saturating_sub(8)
            })
            .count()
    }

    #[test]
    fn a_shadows_softness_widens_its_edge() {
        // the card's shadow ends at x −1.5; the 3×3 filter's edge is a pixel or two, a softness
        // of 21 cm (16 texels here) several
        let (mut lit, meshes) = floor_and_card(shadowed(slanted()));
        let hard = penumbra_across_x(&paint(&lit, &meshes), 0.0, -2.0, -1.0);
        lit.lights[0].shadow.as_mut().unwrap().softness = 0.21; // 16 texels here
        let soft = penumbra_across_x(&paint(&lit, &meshes), 0.0, -2.0, -1.0);
        assert!(hard <= 2, "the 3×3 edge: {hard} px");
        assert!(soft >= hard + 3, "a soft edge: {soft} px against {hard}");
    }

    #[test]
    fn a_soft_shadow_leaves_a_lit_floor_lit_at_any_angle_of_light() {
        // a 16-texel filter reaches far past what the receiver bias clears; each tap compares
        // against the floor's own plane where it reads (`receiver_slope`), so the lit floor
        // reads no shadow of itself — with a fixed radius and with a sized light alike
        for size in [0.0, 0.2] {
            for degrees in [0.0f32, 30.0, 60.0] {
                let angle = degrees.to_radians();
                let mut light = shadowed(Light::directional(
                    [-angle.sin(), -angle.cos(), 0.0],
                    [1.0; 3],
                ));
                light.size = size;
                light.shadow.as_mut().unwrap().softness = 0.21; // 16 texels here
                let (mut lit, meshes) = floor_and_card(light);
                let with_shadow = paint(&lit, &meshes);
                lit.lights[0].shadow = None;
                let without = paint(&lit, &meshes);
                let reach = angle.tan();
                no_acne(&with_shadow, &without, -0.5 - reach..0.5 - reach, 0.6);
            }
        }
    }

    #[test]
    fn a_sized_light_keeps_a_contact_shadow_crisp_and_softens_a_far_one() {
        // a 2 m wall standing on the floor along z (x = 0, z −1 … 1), the light from +x at 45°
        // with an apparent diameter of 0.4 rad, searching 24 texels (about 31 cm): the shadow's
        // side edge at z = 1 is cast by the wall's foot at x = −0.1 (about 0.4 × 0.14 m = 6 cm
        // wide, the blocker search averaging in some of the wall above: crisp) and by its upper
        // part at x = −1.3 (about 0.4 × 1.84 m = 74 cm, held to the 24 texels' radius: some 10 px)
        // — and the shadow's far end, cast by the top at x = −2, is away from both
        let mut light = shadowed(slanted());
        light.size = 0.4;
        light.shadow.as_mut().unwrap().softness = 0.315; // 24 texels here
        let (lit, mut meshes) = floor_and_card(light);
        meshes.insert(
            2,
            Arc::new(quad([
                [0.0, 0.0, 1.0],
                [0.0, 0.0, -1.0],
                [0.0, 2.0, -1.0],
                [0.0, 2.0, 1.0],
            ])),
        );
        let rgba = paint(&lit, &meshes);
        let near = penumbra_across_z(&rgba, -0.1, 0.4, 1.6);
        let far = penumbra_across_z(&rgba, -1.3, 0.4, 1.6);
        assert!(near <= 3, "by the wall's foot the edge is crisp: {near} px");
        assert!(
            far >= near + 4,
            "far from it, soft: {far} px against {near}"
        );
        // and the floor no blocker covers is lit exactly as without a shadow
        let mut unshadowed = lit.clone();
        unshadowed.lights[0].shadow = None;
        let without = paint(&unshadowed, &meshes);
        assert_eq!(red_at(&rgba, 2.0, 2.0), red_at(&without, 2.0, 2.0));
    }

    #[test]
    fn a_uniform_sh_ambient_lights_as_the_same_sky_and_ground() {
        let radiance = [0.2f32, 0.3, 0.4];
        let mut coefficients = [[0.0f32; 3]; 9];
        coefficients[0] = radiance.map(|channel| channel * 2.0 * std::f32::consts::PI.sqrt());
        let mut sky = scene(draw());
        sky.lights.clear();
        sky.sky = radiance;
        sky.ground = radiance;
        let mut sh = sky.clone();
        sh.sky = [9.0; 3];
        sh.ground = [9.0; 3];
        sh.ambient_sh = Some(coefficients);
        let material = Material::default();
        for normal in [[0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.6, 0.0, 0.8]] {
            let by_sky = shade(
                &sky,
                &material,
                [1.0; 3],
                normal,
                normal,
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                0.0,
                1.0,
            );
            let by_sh = shade(
                &sh,
                &material,
                [1.0; 3],
                normal,
                normal,
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                0.0,
                1.0,
            );
            for channel in 0..3 {
                assert!(
                    (by_sky[channel] - by_sh[channel]).abs() < 1e-4,
                    "{normal:?}: {by_sh:?} against {by_sky:?}"
                );
            }
        }
        // light from above (L1−1, the y term) brightens what faces up and not what faces down,
        // and an irradiance below 0 is held at 0
        let mut above = [[0.0f32; 3]; 9];
        above[0] = [1.0; 3];
        above[1] = [1.0; 3];
        let (up, down) = (
            sh_irradiance(&above, [0.0, 1.0, 0.0]),
            sh_irradiance(&above, [0.0, -1.0, 0.0]),
        );
        assert!(up[0] > 0.5 && down[0] == 0.0, "up {up:?}, down {down:?}");
    }

    /// A white light straight on a black surface seen along its normal, nothing else: its
    /// specular alone.
    fn specular_head_on(material: &Material) -> f32 {
        let mut lit = scene(draw());
        lit.sky = [0.0; 3];
        lit.ground = [0.0; 3];
        lit.lights = vec![Light::directional([0.0, 0.0, -1.0], [1.0; 3])];
        let normal = [0.0, 0.0, 1.0];
        shade(
            &lit,
            material,
            [0.0; 3],
            normal,
            normal,
            [0.0; 3],
            1.0,
            &[],
            [0.0; 3],
            0.0,
            1.0,
        )[0]
    }

    #[test]
    fn specular_is_f0_at_normal_incidence_on_the_diffuse_scale_and_grows_at_grazing() {
        // head on, Schlick's Fresnel is F0 (:specular × 0.08) and the normalised Blinn-Phong
        // (n + 8)/8: the 1/π of both BRDFs folded into the light alike
        let plain = Material {
            roughness: 0.5,
            ..Material::default()
        };
        let expected = 0.5 * SPECULAR_TO_F0 * (shininess(0.5) + 8.0) / 8.0;
        let measured = specular_head_on(&plain);
        assert!(
            (measured - expected).abs() < 1e-3 * expected,
            "{measured} against {expected}"
        );
        // at a grazing view the reflectance climbs toward 1; with :specular 0 it is nothing
        assert!(fresnel(0.04, 0.05) > 0.5 && fresnel(0.04, 1.0) == 0.04);
        assert_eq!(fresnel(0.0, 0.0), 0.0);
        let none = Material {
            specular: 0.0,
            ..plain.clone()
        };
        assert_eq!(specular_head_on(&none), 0.0);
        // skin's two lobes by default, UE4's: 0.85 of a sharper one, 0.15 of a broader one
        let skin = Material {
            surface: Surface::Skin,
            roughness: 0.5,
            specular: 0.35,
            ..Material::default()
        };
        assert_eq!(skin.lobes(), SKIN_LOBES);
        let lobe = |roughness: f32| (shininess(roughness) + 8.0) / 8.0;
        let two = 0.35 * SPECULAR_TO_F0 * (0.85 * lobe(0.375) + 0.15 * lobe(0.65));
        assert!((specular_head_on(&skin) - two).abs() < 1e-3 * two);
        let one = Material {
            lobes: Some([1.0, 1.0, 1.0]),
            ..skin
        };
        let single = 0.35 * SPECULAR_TO_F0 * lobe(0.5);
        assert!((specular_head_on(&one) - single).abs() < 1e-3 * single);
    }

    #[test]
    fn an_occlusion_map_darkens_the_ambient_and_not_the_lights() {
        // no light: the triangle is its ambient, which a black occlusion map takes away; lit
        // head on, the light stays
        let mesh = Arc::new(triangle());
        let black = [0u8, 0, 0, 255];
        let textures = |handle: u32| {
            (handle == 9).then_some(TextureRef {
                w: 1,
                h: 1,
                rgba: &black,
                levels: &[],
            })
        };
        let red_middle = |scene: &Scene| {
            rasterize(scene, &|_| Some(mesh.clone()), &textures).2[(9 * 16 + 8) * 4]
        };
        let occluded = Draw {
            material: Material {
                occlusion: Some(9),
                double_sided: true,
                ..Material::default()
            },
            ..draw()
        };
        let mut dark = scene(occluded.clone());
        dark.lights.clear();
        let mut open = dark.clone();
        open.draws[0].material.occlusion = None;
        assert!(red_middle(&open) > 40, "the ambient alone lights it");
        assert_eq!(red_middle(&dark), 0, "occluded, it is black");
        // `scene`'s own light, travelling −z onto the triangle's face
        let lit = scene(occluded);
        assert!(red_middle(&lit) > 100, "a light still lights it");
    }

    /// A unit sphere about the origin, `rings` × `segments`, its normals its positions, each
    /// triangle wound counter-clockwise seen from outside in the painters' convention (the
    /// one `triangle` faces the camera with).
    fn sphere(rings: usize, segments: usize) -> Mesh {
        let point = |ring: usize, segment: usize| {
            let polar = std::f32::consts::PI * ring as f32 / rings as f32;
            let around = std::f32::consts::TAU * segment as f32 / segments as f32;
            [
                polar.sin() * around.cos(),
                polar.cos(),
                polar.sin() * around.sin(),
            ]
        };
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut indices: Vec<u16> = Vec::new();
        for ring in 0..=rings {
            for segment in 0..=segments {
                positions.push(point(ring, segment));
            }
        }
        let at = |ring: usize, segment: usize| (ring * (segments + 1) + segment) as u16;
        for ring in 0..rings {
            for segment in 0..segments {
                let corners = [
                    at(ring, segment),
                    at(ring + 1, segment),
                    at(ring + 1, segment + 1),
                    at(ring, segment + 1),
                ];
                for triangle in [
                    [corners[0], corners[1], corners[2]],
                    [corners[0], corners[2], corners[3]],
                ] {
                    let [first, second, third] = triangle.map(|index| positions[index as usize]);
                    let turn = cross(subtract(second, first), subtract(third, first));
                    let middle = [0, 1, 2].map(|axis| first[axis] + second[axis] + third[axis]);
                    // facing the camera means counter-clockwise on screen, a turn against the
                    // outward normal here (`triangle`'s normal is +z, seen from −z)
                    if dot(turn, middle) > 0.0 {
                        indices.extend([triangle[0], triangle[2], triangle[1]]);
                    } else {
                        indices.extend(triangle);
                    }
                }
            }
        }
        let flat: Vec<f32> = positions.iter().flatten().copied().collect();
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&flat),
            normals: bytes_of(&flat),
            indices: indices
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        })
        .expect("a sphere")
    }

    #[test]
    fn a_shadow_does_not_cut_the_skins_wrap_light_past_the_terminator() {
        // a skin sphere lit from −x by a shadowed light, seen from −z: past the terminator
        // (x > 0 on its face) the wrap light is light under the skin, and the sphere's own
        // shadow there must not cut it off in a hard line — every pixel there paints as it does
        // with the shadow off; on the lit side the shadow still applies (nothing casts on it)
        let mut light = shadowed(Light::directional([1.0, 0.0, 0.0], [2.0; 3]));
        light.shadow.as_mut().unwrap().resolution = 512;
        let skin = Material {
            surface: Surface::Skin,
            // a 1 m sphere scattering as a 6.7 mm one does: a band to compare
            scatter: Some(150.0),
            specular: 0.0,
            ..Material::default()
        };
        let mut lit = scene(Draw {
            material: skin,
            ..draw()
        });
        lit.w = 128.0;
        lit.h = 128.0;
        // x, y → clip / 1.5; depth (z + 2) / 4, the camera on −z looking along +z
        lit.view_proj = from_rows([
            [1.0 / 1.5, 0.0, 0.0, 0.0],
            [0.0, 1.0 / 1.5, 0.0, 0.0],
            [0.0, 0.0, 0.25, 0.5],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        lit.eye = [0.0, 0.0, -10.0];
        lit.sky = [0.0; 3];
        lit.ground = [0.0; 3];
        lit.lights = vec![light];
        let meshes = HashMap::from([(1, Arc::new(sphere(48, 96)))]);
        let with_shadow = paint(&lit, &meshes);
        let mut unshadowed = lit.clone();
        unshadowed.lights[0].shadow = None;
        let without = paint(&unshadowed, &meshes);
        // the picture's column for world x (64 px a 1.5 m, centre 64)
        let column = |world_x: f32| (64.0 + world_x / 1.5 * 64.0) as usize;
        let mut band = 0;
        for row in 40..88usize {
            let world_y = (64.0 - row as f32 - 0.5) / 64.0 * 1.5;
            for world_x in (1..20).map(|step| 0.04 * step as f32) {
                // inside the disc, a little past the terminator and clear of the rim
                if world_x * world_x + world_y * world_y > 0.8 {
                    continue;
                }
                let at = (row * 128 + column(world_x)) * 4;
                assert_eq!(
                    with_shadow[at], without[at],
                    "past the terminator at x {world_x}, y {world_y}: the shadow cut the wrap"
                );
                band += usize::from(without[at] > 10);
            }
        }
        assert!(band > 50, "the wrap band is lit to compare: {band} pixels");
    }

    // --- the detail tile (ADR-797) --------------------------------------------------------------

    /// A quad filling the picture at depth 0.5, facing the viewer at −z: `normal` at every
    /// vertex, its tangent +x (so its bitangent is −y) when `tangents`, its UVs 0 … 1 from
    /// the bottom left. Picture column `c` is u `(c + 0.5) / 16`, row `r` is v
    /// `1 − (r + 0.5) / 16`.
    fn facing_quad(normal: [f32; 3], tangents: bool) -> Arc<Mesh> {
        let normals: Vec<f32> = (0..4).flat_map(|_| normal).collect();
        let tangent: Vec<f32> = (0..4).flat_map(|_| [1.0, 0.0, 0.0, 1.0]).collect();
        Arc::new(
            Mesh::from_parts(MeshParts {
                positions: bytes_of(&[
                    -1.0, -1.0, 0.5, 1.0, -1.0, 0.5, 1.0, 1.0, 0.5, -1.0, 1.0, 0.5,
                ]),
                normals: bytes_of(&normals),
                uvs: bytes_of(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]),
                tangents: if tangents {
                    bytes_of(&tangent)
                } else {
                    Vec::new()
                },
                indices: [0u16, 1, 2, 0, 2, 3]
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect(),
                ..MeshParts::default()
            })
            .expect("a quad"),
        )
    }

    /// `material` on `mesh`, lit head on by one white light and a faint ambient, seen from −z.
    fn detail_scene(material: Material) -> Scene {
        let mut seen = scene(Draw {
            material: Material {
                double_sided: true,
                ..material
            },
            ..draw()
        });
        seen.lights = vec![Light::directional([0.0, 0.0, 1.0], [1.0; 3])];
        seen
    }

    /// The detail tile's handle in these tests, and a normal map's.
    const TILE: u32 = 21;
    const NORMAL_MAP: u32 = 22;

    /// `seen` painted, texture [`TILE`] being `tile` and [`NORMAL_MAP`] a 2×2 map of four
    /// different normals.
    fn paint_detailed(seen: &Scene, mesh: &Arc<Mesh>, tile: &TextureRef) -> Vec<u8> {
        let bumps: [u8; 16] = [
            150, 128, 250, 255, 128, 160, 248, 255, 100, 128, 247, 255, 128, 110, 252, 255,
        ];
        rasterize(seen, &|_| Some(mesh.clone()), &|handle| match handle {
            TILE => Some(TextureRef {
                w: tile.w,
                h: tile.h,
                rgba: tile.rgba,
                levels: tile.levels,
            }),
            NORMAL_MAP => Some(TextureRef {
                w: 2,
                h: 2,
                rgba: &bumps,
                levels: &[],
            }),
            _ => None,
        })
        .2
    }

    /// A tile of one texel.
    fn one_texel(rgba: &[u8; 4]) -> TextureRef<'_> {
        TextureRef {
            w: 1,
            h: 1,
            rgba,
            levels: &[],
        }
    }

    /// The most any byte of two pictures differs by.
    fn furthest_apart(left: &[u8], right: &[u8]) -> i32 {
        assert_eq!(left.len(), right.len());
        left.iter()
            .zip(right)
            .map(|(one, other)| (i32::from(*one) - i32::from(*other)).abs())
            .max()
            .unwrap_or(0)
    }

    /// How far two pictures are apart in all, a byte at a time.
    fn apart_in_all(left: &[u8], right: &[u8]) -> i64 {
        left.iter()
            .zip(right)
            .map(|(one, other)| i64::from((i32::from(*one) - i32::from(*other)).abs()))
            .sum()
    }

    #[test]
    fn a_flat_detail_tile_changes_nothing_at_all() {
        // the byte 128 is no tilt and 255 an open surface, exactly: read through the sampler
        // it is `DETAIL_FLAT` to the bit, so the tilt is 0 and the specular is kept whole
        let flat = [128u8, 128, 255, 255];
        let texel = sample_lod(&one_texel(&flat), 0.3, 0.7, 0.0);
        assert_eq!(detail_tilt(texel), [0.0, 0.0]);
        assert_eq!(detail_specular(texel[2], 1.0), 1.0);
        // a tile that does not tilt gives the surface's own normal back, bit for bit
        let bumped = [0.31f32, -0.12, 0.943];
        assert_eq!(detail_normal(bumped, [0.0, 0.0], 3.0), bumped);
        // so a picture is the same bytes with it as without: over a normal map, over a bare
        // surface, and on a mesh without tangents; whatever its scale and strength
        let detailed = |material: Material| Material {
            detail: Some(TILE),
            detail_scale: [7.0, 3.0],
            detail_strength: 3.0,
            ..material
        };
        let glossy = Material {
            surface: Surface::Skin,
            color: [0.4, 0.3, 0.25, 1.0],
            roughness: 0.3,
            rim: 0.5,
            ..Material::default()
        };
        let mapped = Material {
            normal: Some(NORMAL_MAP),
            ..glossy.clone()
        };
        for (material, tangents) in [(mapped, true), (glossy.clone(), true), (glossy, false)] {
            let mesh = facing_quad([0.0, 0.0, -1.0], tangents);
            let tile = one_texel(&flat);
            let without = paint_detailed(&detail_scene(material.clone()), &mesh, &tile);
            let with = paint_detailed(&detail_scene(detailed(material)), &mesh, &tile);
            assert!(without.iter().any(|byte| *byte != 0), "something is drawn");
            assert_eq!(with, without, "tangents: {tangents}");
        }
    }

    #[test]
    fn a_detail_normal_adds_its_slope_to_the_surfaces_scaled_by_its_strength() {
        // over a flat surface it is the detail itself, its angle θ becoming atan(s·tan θ)
        let angle = 0.3f32;
        for strength in [0.0f32, 0.5, 1.0, 2.0, 4.0] {
            let tilted = normalize(detail_normal([0.0, 0.0, 1.0], [angle.sin(), 0.0], strength));
            let expected = (strength * angle.tan()).atan();
            let measured = tilted[0].atan2(tilted[2]);
            assert!(
                (measured - expected).abs() < 1e-5,
                "strength {strength}: {measured} against {expected}"
            );
            assert_eq!(tilted[1], 0.0);
        }
        // over a tilted surface the slopes add: 0.2 across and 0.1 up, plus the detail's
        let surface = normalize([0.2, 0.1, 1.0]);
        let detail = normalize([-0.3, 0.4, 1.0]);
        let both = detail_normal(surface, [detail[0], detail[1]], 1.0);
        let slopes = [both[0] / both[2], both[1] / both[2]];
        assert!(
            (slopes[0] + 0.1).abs() < 1e-5 && (slopes[1] - 0.5).abs() < 1e-5,
            "{slopes:?}"
        );
        // a texel past the unit circle (the bytes allow it) is a normal lying flat, not a NaN
        let flat_out = detail_normal([0.0, 0.0, 1.0], [-1.004, 0.3], 1.0);
        assert!(flat_out.iter().all(|part| part.is_finite()) && flat_out[2] == 0.0);
    }

    #[test]
    fn a_tilted_detail_tile_shades_as_a_mesh_tilted_by_the_same_angle() {
        // one texel tilting 64 bytes along the tangent: sin θ = 2 · 64 / 255. The quad faces
        // −z with its tangent +x, so its shaded normal is (sin θ', 0, −cos θ'), θ' the angle
        // the strength makes of θ; a quad without the tile but with that normal at its
        // vertices is the same picture
        let tilting = [192u8, 128, 255, 255];
        let sine = 2.0 * 64.0 / 255.0f32;
        let angle = sine.asin();
        // lit from 45° toward +x, so the light it takes grows with the tilt at once
        let lit = |material: Material| {
            let mut seen = detail_scene(Material {
                color: [0.25, 0.25, 0.25, 1.0],
                ..material
            });
            seen.lights = vec![Light::directional([-1.0, 0.0, 1.0], [1.0; 3])];
            seen
        };
        let plain = paint_detailed(
            &lit(Material::default()),
            &facing_quad([0.0, 0.0, -1.0], true),
            &one_texel(&tilting),
        );
        for strength in [1.0f32, 2.0, 0.5] {
            let expected = (strength * angle.tan()).atan();
            let detailed = paint_detailed(
                &lit(Material {
                    detail: Some(TILE),
                    detail_scale: [5.0, 5.0],
                    detail_strength: strength,
                    ..Material::default()
                }),
                &facing_quad([0.0, 0.0, -1.0], true),
                &one_texel(&tilting),
            );
            let tilted = paint_detailed(
                &lit(Material::default()),
                &facing_quad([expected.sin(), 0.0, -expected.cos()], false),
                &one_texel(&tilting),
            );
            assert!(
                furthest_apart(&detailed, &tilted) <= 1,
                "strength {strength}: {} apart",
                furthest_apart(&detailed, &tilted)
            );
            assert!(
                furthest_apart(&detailed, &plain) > 4,
                "strength {strength}: the tilt shows"
            );
        }
        // without tangents there is no frame to tilt in: the tile's normal is skipped
        let bare = facing_quad([0.0, 0.0, -1.0], false);
        let skipped = paint_detailed(
            &lit(Material {
                detail: Some(TILE),
                ..Material::default()
            }),
            &bare,
            &one_texel(&tilting),
        );
        assert_eq!(skipped, plain);
    }

    #[test]
    fn a_cavity_takes_the_specular_and_the_rim_and_leaves_the_diffuse() {
        // by the number: an open texel keeps it all, a closed one none, `:detail-cavity`
        // saying how much of that is taken
        assert_eq!(detail_specular(1.0, 1.0), 1.0);
        assert_eq!(detail_specular(0.0, 1.0), 0.0);
        assert_eq!(detail_specular(0.0, 0.0), 1.0);
        assert!((detail_specular(0.25, 0.5) - 0.625).abs() < 1e-6);
        // in `shade`: nothing kept is the same light as a material that reflects nothing
        let shiny = Material {
            color: [0.2, 0.2, 0.2, 1.0],
            roughness: 0.4,
            rim: 0.5,
            ..Material::default()
        };
        let matte = Material {
            specular: 0.0,
            rim: 0.0,
            ..shiny.clone()
        };
        let lit = detail_scene(shiny.clone());
        let normal = [0.0, 0.0, -1.0];
        let view = normalize([0.3, 0.0, -1.0]);
        let shaded = |material: &Material, kept: f32| {
            shade(
                &lit,
                material,
                [0.2; 3],
                normal,
                view,
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                0.0,
                kept,
            )
        };
        assert_eq!(shaded(&shiny, 0.0), shaded(&matte, 1.0));
        assert!(shaded(&shiny, 1.0)[0] > shaded(&shiny, 0.5)[0]);
        assert!(shaded(&shiny, 0.5)[0] > shaded(&shiny, 0.0)[0]);
        // painted, with or without tangents: a tile of closed pores is the matte material's
        // picture exactly (its diffuse untouched), and with `:detail-cavity` 0 the shiny one's
        let closed = [128u8, 128, 0, 255];
        for tangents in [true, false] {
            let mesh = facing_quad(normal, tangents);
            let paint = |material: Material| {
                paint_detailed(&detail_scene(material), &mesh, &one_texel(&closed))
            };
            let pored = paint(Material {
                detail: Some(TILE),
                ..shiny.clone()
            });
            let ignored = paint(Material {
                detail: Some(TILE),
                detail_cavity: 0.0,
                ..shiny.clone()
            });
            assert_eq!(pored, paint(matte.clone()), "tangents: {tangents}");
            assert_eq!(ignored, paint(shiny.clone()), "tangents: {tangents}");
            assert!(furthest_apart(&pored, &ignored) > 4, "the specular shows");
        }
    }

    #[test]
    fn a_detail_scale_repeats_the_tile() {
        // a 2×2 tile, flat at (0, 0) and (1, 1) and tilting hard at the other two; a tilted
        // texel takes less of the head-on light, so it is darker
        let flat = [128u8, 128, 255, 255];
        let steep = [238u8, 128, 255, 255];
        let rgba: Vec<u8> = [flat, steep, steep, flat].concat();
        let tile = TextureRef {
            w: 2,
            h: 2,
            rgba: &rgba,
            levels: &[],
        };
        let mesh = facing_quad([0.0, 0.0, -1.0], true);
        let painted = |scale: [f32; 2]| {
            paint_detailed(
                &detail_scene(Material {
                    color: [0.25, 0.25, 0.25, 1.0],
                    specular: 0.0,
                    detail: Some(TILE),
                    detail_scale: scale,
                    ..Material::default()
                }),
                &mesh,
                &tile,
            )
        };
        let red =
            |picture: &[u8], column: usize, row: usize| i32::from(picture[(row * 16 + column) * 4]);
        // what is flat is this much brighter than what tilts
        let brighter = |picture: &[u8], flat_at: (usize, usize), steep_at: (usize, usize)| {
            let gap = red(picture, flat_at.0, flat_at.1) - red(picture, steep_at.0, steep_at.1);
            assert!(gap > 8, "{flat_at:?} over {steep_at:?}: {gap}");
        };
        // once across: the picture's bottom left and top right quarters are the flat texels
        // (row 12 is v 0.22, row 4 is v 0.72)
        let once = painted([1.0, 1.0]);
        brighter(&once, (3, 12), (11, 12));
        brighter(&once, (11, 4), (3, 4));
        // twice along u only: four columns of texels across, still two rows
        let twice_across = painted([2.0, 1.0]);
        brighter(&twice_across, (1, 12), (5, 12));
        brighter(&twice_across, (9, 12), (13, 12));
        brighter(&twice_across, (5, 4), (1, 4));
        // twice each way: the tile's four texels in each quarter of the picture (row 14 is v
        // 0.09, row 10 is v 0.34)
        let twice = painted([2.0, 2.0]);
        brighter(&twice, (1, 14), (5, 14));
        brighter(&twice, (9, 14), (13, 14));
        brighter(&twice, (5, 10), (1, 10));
        brighter(&twice, (9, 6), (9, 10));
    }

    #[test]
    fn a_detail_tile_is_read_at_the_level_its_own_repeats_ask_for() {
        // a 64-texel texture over a quad 16 pixels wide is four texels a pixel: level 2; every
        // doubling of its repeats along one axis is half a level more, along both a whole one
        let blank = vec![255u8; 64 * 64 * 4];
        let levels = mip_chain(&blank, 64, 64, None);
        let texture = TextureRef {
            w: 64,
            h: 64,
            rgba: &blank,
            levels: &levels,
        };
        let corner = |x: f32, y: f32, u: f32, v: f32| ClipVertex {
            clip: [x, y, 0.5, 1.0],
            world: [x, y, 0.5],
            normal: [0.0, 0.0, -1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [u, v],
            curvature: 0.0,
        };
        let corners = [
            corner(-1.0, -1.0, 0.0, 0.0),
            corner(1.0, -1.0, 1.0, 0.0),
            corner(1.0, 1.0, 1.0, 1.0),
        ];
        let level = |repeats: [f32; 2]| {
            triangle_lod(
                [&corners[0], &corners[1], &corners[2]],
                &texture,
                repeats,
                16,
                16,
                1.0,
            )
        };
        assert!(
            (level([1.0, 1.0]) - 2.0).abs() < 1e-4,
            "{}",
            level([1.0, 1.0])
        );
        assert!(
            (level([4.0, 4.0]) - 4.0).abs() < 1e-4,
            "{}",
            level([4.0, 4.0])
        );
        assert!(
            (level([4.0, 1.0]) - 3.0).abs() < 1e-4,
            "{}",
            level([4.0, 1.0])
        );
        assert_eq!(level([0.125, 0.125]), 0.0, "magnified: its own size");
    }

    #[test]
    fn a_detail_tile_far_away_fades_into_the_surface_without_it() {
        // an 8×8 tile whose left half tilts one way and its right half the other, as much,
        // half its pores closed: its mip chain averages it down to one texel that does not
        // tilt (the byte 128 exactly, not renormalised, not treated as colour) and is half open
        let side = 8usize;
        let rgba: Vec<u8> = (0..side * side)
            .flat_map(|index| {
                let leftward = index % side < side / 2;
                [
                    if leftward { 228 } else { 28 },
                    128,
                    if leftward { 0 } else { 255 },
                    255,
                ]
            })
            .collect();
        let levels = mip_chain(&rgba, side as u32, side as u32, None);
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[2].rgba, vec![128, 128, 128, 255]);
        assert_eq!(
            &levels[0].rgba[..4],
            &[228, 128, 0, 255],
            "a level is plain averages"
        );
        let tile = TextureRef {
            w: side as u32,
            h: side as u32,
            rgba: &rgba,
            levels: &levels,
        };
        let mesh = facing_quad([0.0, 0.0, -1.0], true);
        let shiny = Material {
            color: [0.25, 0.25, 0.25, 1.0],
            roughness: 0.4,
            ..Material::default()
        };
        let painted = |repeats: f32, cavity: f32| {
            paint_detailed(
                &detail_scene(Material {
                    detail: Some(TILE),
                    detail_scale: [repeats; 2],
                    detail_cavity: cavity,
                    ..shiny.clone()
                }),
                &mesh,
                &tile,
            )
        };
        let without = paint_detailed(&detail_scene(shiny.clone()), &mesh, &tile);
        // the quad is 16 pixels wide: `repeats` of the 8-texel tile are `repeats / 2` texels a
        // pixel, level log2(repeats / 2). Up close (level 0) the tile shows; between its last
        // two levels (2.5) it shows less; from its last level (3) on it is gone, bit for bit
        let near = apart_in_all(&painted(1.0, 0.0), &without);
        let fading = apart_in_all(&painted(2.0 * 2.5f32.exp2(), 0.0), &without);
        assert!(near > fading && fading > 0, "near {near}, fading {fading}");
        assert!(furthest_apart(&painted(16.0, 0.0), &without) <= 1);
        assert_eq!(painted(64.0, 0.0), without);
        // its cavity fades to the tile's mean, not to nothing: a surface of pores reflects
        // less than one without at any distance
        let half_open = [128u8, 128, 128, 255];
        let mean = paint_detailed(
            &detail_scene(Material {
                detail: Some(TILE),
                ..shiny.clone()
            }),
            &mesh,
            &one_texel(&half_open),
        );
        assert_eq!(painted(64.0, 1.0), mean);
        assert!(furthest_apart(&mean, &without) > 2, "the mean cavity shows");
    }

    #[test]
    fn hair_reads_no_detail_tile() {
        // a `:hair` or `:strands` surface's tangents are its strands, and its light its own
        for surface in [Surface::Hair, Surface::Strands] {
            let material = Material {
                surface,
                detail: Some(TILE),
                ..Material::default()
            };
            assert_eq!(material.detail_texture(), None);
        }
        for surface in [
            Surface::Skin,
            Surface::Standard,
            Surface::Cutout,
            Surface::Blend,
        ] {
            let material = Material {
                surface,
                detail: Some(TILE),
                ..Material::default()
            };
            assert_eq!(material.detail_texture(), Some(TILE));
        }
        assert_eq!(Material::default().detail_texture(), None);
    }

    /// What morphing costs at a real figure's size (womanblend's body: 70,737 vertices, 37 face
    /// targets each moving about 2,500 of them, scattered through the mesh as hers are), so a
    /// growing face or a new figure can be checked against it:
    /// `cargo test --release -p brood --features gui --lib -- --ignored morph_cost --nocapture`.
    /// BLEND is what both painters do a frame (the GPU target then uploads the offsets and reads
    /// one pair a vertex); POSE is the CPU painter's whole vertex pass (morph, skin, transform).
    #[test]
    #[ignore = "a timing, run by hand"]
    fn morph_cost_at_her_size() {
        let vertex_count = 70_737usize;
        let moved_each = 2_500usize;
        let positions: Vec<f32> = (0..vertex_count * 3)
            .map(|index| index as f32 * 1e-5)
            .collect();
        let parts = MeshParts {
            positions: bytes_of(&positions),
            morphs: (0..37usize)
                .map(|target| {
                    // a scattered set of vertices: every (vertex_count / moved_each)th one, from
                    // an offset of its own
                    let mut offsets = vec![0.0f32; vertex_count * 3];
                    let stride = vertex_count / moved_each;
                    for step in 0..moved_each {
                        let vertex = (target * 7 + step * stride) % vertex_count;
                        offsets[vertex * 3 + 1] = 0.001;
                    }
                    (bytes_of(&offsets), bytes_of(&offsets))
                })
                .collect(),
            ..MeshParts::default()
        };
        let mesh = Mesh::from_parts(parts).unwrap();
        let rounds = 50;
        let per_round =
            |start: std::time::Instant| start.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        for count in [0u32, 8, 16, 24, 37] {
            let morphs: Vec<(u32, f32)> = (0..count).map(|target| (target, 0.5)).collect();
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                let mut offsets = vec![[0.0f32; 3]; vertex_count];
                let mut turns = vec![[0.0f32; 3]; vertex_count];
                add_morphs(&mesh.morphs, &morphs, &mut offsets, &mut turns);
                std::hint::black_box((offsets, turns));
            }
            let blend = per_round(start);
            let morphed = scene(Draw { morphs, ..draw() });
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                std::hint::black_box(pose_vertices(&morphed, &mesh, &morphed.draws[0]));
            }
            eprintln!(
                "MORPH COST {count} targets: BLEND {blend:.3} ms, POSE {:.2} ms",
                per_round(start)
            );
        }
    }
}
