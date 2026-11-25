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
/// A hair's own shadow ([`hair_shadow`]): the reads of the depth map it takes, the radius of
/// their disc in texels, the share of light lost a metre of hair (1/e in 2.5 cm), and how far
/// the light that passes takes the hair's colour ([`hair_light`]).
pub const HAIR_SHADOW_TAPS: u32 = 16;
pub const HAIR_SHADOW_TEXELS: f32 = 6.0;
pub const HAIR_ABSORPTION: f32 = 40.0;
pub const HAIR_TINT: f32 = 0.45;
/// A material's `:specular` times this is its reflectance at normal incidence, F0 (ADR-793):
/// UE4's mapping, so the default 0.5 is 4 %, a common dielectric's, and skin's 2.8 % is 0.35.
pub const SPECULAR_TO_F0: f32 = 0.08;
/// The most shapes a scene's glossy surfaces show reflected (ADR-795): its shaped lights and its
/// `:emitters` together.
pub const MAX_EMITTERS: usize = 8;
/// The reflectance at normal incidence of a material's `:coat` (ADR-795): water's, 2 %.
pub const COAT_F0: f32 = 0.02;
/// The roughness under which a specular lobe is no sharper (a mirror's): the exponent there,
/// `2 / roughness⁴ − 2`, is the most any lobe has.
pub const MIN_ROUGHNESS: f32 = 0.04;
/// The specular exponent a light without a `:shape` reaches on any surface; past it (a
/// roughness under about 0.149) its highlight sharpens only as far as a pixel can show on a
/// surface curved as the mesh is there ([`highlight_exponent`], ADR-795).
pub const RESOLVED_EXPONENT: f32 = 4096.0;
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
/// The least of its width a `:strands` ribbon keeps when the strand runs straight at the eye
/// ([`strand_vertex`]).
pub const STRAND_END_ON: f32 = 0.3;
/// The share of a pixel a `:strands` ribbon covers at or above which it counts as whole: its
/// texture's alpha is then dithered in [`HAIR_COVERAGE_STEPS`], not at samples its position picks.
pub const STRAND_WHOLE: f32 = 0.999;
///
/// Eight steps, as the GPU painter's eight samples a pixel take them (ADR-799; four on an
/// adapter that cannot multisample 8×, the sets' every other step): the CPU painter has four
/// sub-samples a pixel, so neighbouring pixels take alternate halves of the eight
/// (`[parity][row][column]`), the same coverage on average and nine levels over two pixels
/// where one pixel's four held five.
pub const HAIR_COVERAGE_STEPS: [[[f32; 2]; 2]; 2] = [
    [[0.0625, 0.5625], [0.8125, 0.3125]],
    [[0.1875, 0.6875], [0.9375, 0.4375]],
];

/// The reach of the ambient occlusion, in metres, when `:ambient-occlusion` names none: the
/// size of the folds it darkens (a mouth's corner, a crease beside the nose).
pub const DEFAULT_OCCLUSION_RADIUS: f32 = 0.03;
/// The most vertices one mesh may have: 84 bytes each on the GPU target stays under the
/// smallest buffer every device allows (256 MiB).
pub const MAX_VERTICES: usize = 3 << 20;
/// The most bytes of morph targets one mesh may carry as uploaded: 24 bytes a vertex a target
/// (position and normal offsets, every vertex, as a glTF buffer holds them), before they are
/// kept sparse. Neither painter keeps them dense: the GPU target uploads one blended offset a
/// vertex a frame. The cap bounds what one `gui/mesh` call may make the host unpack: a guard
/// against a bad file, not a cost kept (the dense input lives only while the call sorts it into
/// sparse targets). 256 MiB: a figure of 70,000 vertices has room for some 150 targets (at 128
/// MiB its 79 were all spoken for). Sparse input (indices and offsets a target) is the real
/// cure and would make the cap count what is kept.
pub const MAX_MORPH_BYTES: usize = 256 << 20;
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
    /// How soft its flesh is at each vertex, 0 (bone) … 1 (soft), for the ring a contact raises
    /// (ADR-805); empty: none (a contact still lays its skin flat, with no ring).
    pub softness: Vec<f32>,
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
    /// How soft its flesh is a vertex, f32 0 (bone) … 1 (soft), for the ring round a contact
    /// (ADR-805); empty: none.
    pub softness: Vec<u8>,
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
        let softness = if parts.softness.is_empty() {
            Vec::new()
        } else {
            floats::<1>("softness", &parts.softness, count)?
                .into_iter()
                .map(|[value]| value.clamp(0.0, 1.0))
                .collect()
        };
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
            softness,
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
    /// pixel, all of it at one and a half `cutoff`s — that many of the GPU target's 8 samples
    /// (4 where the adapter has no 8×; its sample mask, ADR-799) or the CPU painter's 2×2
    /// sub-samples (alternate halves of the eight steps in neighbouring pixels), each a step of
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
    /// An eye (ADR-796): an eyeball's mesh, opaque, its texels under the material's `cutoff`
    /// not drawn (a clear shell over it), what is under its cornea found by optics in the eye's
    /// own frame ([`Eye`], [`eye_seen`]) — the iris read where the sight line, bent at the
    /// cornea, meets the iris's plane; the pupil drawn there; its highlights the cornea's.
    /// Without an [`Eye`] it is a `:cutout`.
    Eye,
}

/// How many eyes one `:eye` material may place: a pair in one mesh.
pub const MAX_EYES: usize = 2;
/// The refractive index an eye bends light by unless its material says: the cornea's.
pub const DEFAULT_EYE_IOR: f32 = 1.376;
/// An eye's sizes unless its material says, metres: a grown person's.
pub const DEFAULT_EYE_RADIUS: f32 = 0.012;
pub const DEFAULT_IRIS_RADIUS: f32 = 0.0059;
pub const DEFAULT_CORNEA_RADIUS: f32 = 0.0078;
pub const DEFAULT_IRIS_DEPTH: f32 = 0.0033;
/// The pupil's radius, a share of the iris's, unless the material says.
pub const DEFAULT_PUPIL: f32 = 0.35;
/// The slope of the bowl an iris is lit as, at its edge, unless the material says: about 19°.
pub const DEFAULT_IRIS_CONCAVITY: f32 = 0.35;
/// The steepest bowl a material may ask for.
pub const MAX_IRIS_CONCAVITY: f32 = 4.0;
/// How soft the pupil's edge is: it fades over this share of its radius either side.
pub const PUPIL_EDGE: f32 = 0.08;
/// Where, as a share of the iris's radius from the eye's axis, the cornea's own normal begins
/// to give way to the mesh's, which it is at the limbus (1).
pub const CORNEA_EDGE: f32 = 0.85;

/// One eye of an `:eye` material, in its mesh's space at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeAt {
    /// The eyeball's centre.
    pub centre: [f32; 3],
    /// The unit direction it looks along, out through the pupil.
    pub axis: [f32; 3],
    /// Where the middle of its iris is in the base texture.
    pub iris_uv: [f32; 2],
}

/// An `:eye` surface's optics (ADR-796): where its eyes are in the mesh at rest, and their
/// shape. The vertices carry the frame into the world ([`eye_frame`]: each vertex's own
/// skinning matrix moves its eye's centre, axis and up), so an eye that turns with a bone
/// needs nothing told to it each frame. Lengths are in the mesh's units.
///
/// The model: the eyeball a sphere of `radius` about the centre; the limbus the circle of
/// `iris_radius` on it about the axis; the cornea a sphere of `cornea_radius` through the
/// limbus, bulging out along the axis; the iris a flat disc of `iris_radius` across the axis,
/// `iris_depth` under the cornea's top. The base texture holds the iris as a disc: its middle
/// at the eye's `iris_uv`, its edge `iris_uv_radius` away, u running to the right of a viewer
/// the eye looks at whose up is `up`, v running down.
#[derive(Clone, Debug, PartialEq)]
pub struct Eye {
    /// One or two eyes ([`MAX_EYES`]).
    pub eyes: Vec<EyeAt>,
    /// The direction that is up in the iris's texture, in the mesh at rest.
    pub up: [f32; 3],
    pub radius: f32,
    pub iris_radius: f32,
    pub cornea_radius: f32,
    pub iris_depth: f32,
    /// The refractive index the cornea bends the sight line by (1: not at all).
    pub ior: f32,
    pub iris_uv_radius: f32,
    /// The pupil's radius as a share of the iris's (0: none drawn).
    pub pupil: f32,
    /// How steeply the iris, lit as a shallow bowl, slopes up at its edge: the tangent of the
    /// angle its normal leans toward the axis there (0: lit flat).
    pub concavity: f32,
}

impl Eye {
    /// The eye a vertex at `rest` (the mesh's space, morphed, not yet skinned) belongs to: the
    /// one whose centre is nearest. The shader's `eye_frame` picks the same.
    pub fn nearest(&self, rest: [f32; 3]) -> usize {
        let apart = |eye: &EyeAt| {
            let offset = subtract(rest, eye.centre);
            dot(offset, offset)
        };
        match (self.eyes.first(), self.eyes.get(1)) {
            (Some(first), Some(second)) if apart(second) < apart(first) => 1,
            _ => 0,
        }
    }
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
    /// the specular (the lights' and the surroundings') and nothing else ([`detail_specular`]). Not read by a `:hair`
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
    /// `[strength roughness]`: a second, clear layer over the surface (ADR-795), a dielectric
    /// of [`COAT_F0`] reflecting the lights, the emitters and the surroundings at its own
    /// roughness, `strength` (0 … 1) of it: saliva on lips and teeth, the film over an eye.
    /// None: no coat. Not read by a `:hair` or `:strands` surface.
    pub coat: Option<[f32; 2]>,
    /// An `:eye` surface's optics (ADR-796). Read by no other surface.
    pub eye: Option<Eye>,
    /// The alpha under which a cutout texel is not drawn.
    pub cutoff: f32,
    /// Draw back faces too (their normals turned to face the viewer).
    pub double_sided: bool,
    /// Whether it casts a shadow from the scene's shadowed lights (ADR-788). A `:cutout` or
    /// `:blend` surface casts only where its alpha reaches `cutoff`.
    pub cast_shadow: bool,
    /// A `:strands` surface's width at the root and at the tip, metres ([`DEFAULT_STRAND_WIDTH`]).
    pub strand_width: [f32; 2],
    /// A `:hair` or `:strands` surface's two highlights' shifts along its normal, the first
    /// and the second (`:shift`; [`HAIR_SHIFT_PRIMARY`], [`HAIR_SHIFT_SECONDARY`]: ADR-799).
    pub hair_shift: [f32; 2],
    /// The second highlight's colour, linear RGB (`:sheen`); nil: the hair's own base colour.
    pub sheen: Option<[f32; 3]>,
    /// Its skin pressed by the scene's contacts (`:deform`, ADR-805).
    pub deform: bool,
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
            coat: None,
            eye: None,
            cutoff: 0.5,
            double_sided: false,
            cast_shadow: true,
            strand_width: DEFAULT_STRAND_WIDTH,
            hair_shift: [HAIR_SHIFT_PRIMARY, HAIR_SHIFT_SECONDARY],
            sheen: None,
            deform: false,
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
            Surface::Skin | Surface::Standard | Surface::Cutout | Surface::Blend | Surface::Eye => {
                self.detail
            }
        }
    }

    /// Its eyes' optics (ADR-796): its `:eye`, on an `:eye` surface only. Both painters ask
    /// here, so no other surface pays for a key it does not read.
    pub fn eye_optics(&self) -> Option<&Eye> {
        match self.surface {
            Surface::Eye => self.eye.as_ref().filter(|eye| !eye.eyes.is_empty()),
            _ => None,
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
    /// The shape glossy surfaces show it as (ADR-795); none is a point, whose highlight is the
    /// Blinn-Phong lobe.
    pub shape: Option<LightShape>,
}

/// What an emitter looks like in a reflection (ADR-795).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmitterShape {
    /// A flat disc facing its direction.
    Disc,
    /// A flat rectangle facing its direction: a softbox, a window.
    Rect,
    /// A ball: a bulb, a globe.
    Sphere,
}

/// A light's `:shape` (ADR-795). A disc's or a sphere's diameter is the light's `size` (a
/// directional light's its `:angle`); a rectangle is `extent` (width and height, metres) with
/// its height along `up` as nearly as its facing, the light's direction, allows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightShape {
    pub shape: EmitterShape,
    pub extent: [f32; 2],
    pub up: [f32; 3],
}

/// One of a scene's `:emitters` (ADR-795): a shape seen only in reflections, lighting nothing
/// and casting nothing (a window, a bounce card). `direction` is the way its light travels
/// (the way a disc or rectangle faces); `extent` a rectangle's width and height, or a disc's
/// or sphere's diameter in its first; `radiance` linear, on the scale of the ambient light's.
#[derive(Clone, Debug, PartialEq)]
pub struct Emitter {
    pub shape: EmitterShape,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub up: [f32; 3],
    pub extent: [f32; 2],
    pub radiance: [f32; 3],
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
            shape: None,
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
    /// Shapes seen only in reflections (`:emitters`, ADR-795).
    pub emitters: Vec<Emitter>,
    /// Scales the light before tone mapping.
    pub exposure: f32,
    /// What the rect shows where no mesh is: straight RGBA 0 … 1, transparent by default.
    pub background: [f32; 4],
    /// Joint matrices (column-major, 16 floats each), one list per skeleton.
    pub palettes: Vec<Arc<[f32]>>,
    pub draws: Vec<Draw>,
    /// What presses on the skin of its `:deform` draws this frame (ADR-805), in world space, at
    /// most [`MAX_CONTACTS`]: skin passed into one is laid onto its surface, and soft flesh round
    /// it pushed out in a ring ([`deform_by_contacts`]).
    pub contacts: Vec<Contact>,
    /// How far each contact drags the skin it presses along its surface this frame (metres; the
    /// friction that holds skin to glass, kept by the game): one a contact, or none (ADR-805).
    pub contact_drags: Vec<[f32; 3]>,
    /// Each contact's own limb: up to four joints (the mesh's skin joint indices; `u16::MAX` none)
    /// whose skin it never presses, a vertex skipped where its weights on them pass a half: a
    /// thigh's capsule presses the other thigh, not its own (ADR-805). One a contact, or none.
    pub contact_skips: Vec<[u16; 4]>,
}

/// The most contacts a scene presses its skin with (ADR-805).
pub const MAX_CONTACTS: usize = 16;
/// How far round a contact soft flesh rises, at the least (metres); the deeper the press, the
/// wider: [`RING_WIDTH`] + [`RING_SPREAD`] × its depth (ADR-805).
pub const RING_WIDTH: f32 = 0.012;
pub const RING_SPREAD: f32 = 2.0;
/// How high the ring rises at its crest, for flesh of softness 1, as a share of the press's
/// deepest depth: about the dent's volume spread round it (ADR-805).
pub const RING_LIFT: f32 = 0.35;
/// How wide the eased edge of a contact's patch is (metres): `EDGE_BAND` + `EDGE_SPREAD` × the press's
/// depth, over which skin meets the surface tangentially and its normal turns (ADR-805).
pub const EDGE_BAND: f32 = 0.004;
pub const EDGE_SPREAD: f32 = 0.5;
/// How far a firm part sits behind a contact's surface (a nipple sunk into the softer breast
/// round it), as a share of the press's depth, for softness 0, none from `FIRM_SOFTNESS` up.
pub const FIRM_INSET: f32 = 0.25;
pub const FIRM_SOFTNESS: f32 = 0.5;
/// The farthest skin of softness 1 is laid toward a contact's surface (metres); softer skin less
/// in proportion. Pressed further (a pose that pushes a face deep through glass), skin passes the
/// surface rather than being flattened away from what lies under it (an eye, the mouth) (ADR-805).
pub const MAX_SQUASH: f32 = 0.02;

/// A shape pressing on skin (ADR-805), in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Contact {
    /// The half-space `normal · p < offset` is inside (a floor, a pane, a wall); `normal` unit.
    Plane { normal: [f32; 3], offset: f32 },
    /// A ball.
    Sphere { centre: [f32; 3], radius: f32 },
    /// A segment `start`–`end` grown by `radius` (a finger's bone, a forearm, a seat's edge).
    Capsule {
        start: [f32; 3],
        end: [f32; 3],
        radius: f32,
    },
}

impl Contact {
    /// How far `point` lies outside the shape (negative inside), and the unit direction out of
    /// it there: its surface's normal at the nearest point.
    pub fn signed_distance(&self, point: [f32; 3]) -> (f32, [f32; 3]) {
        let away = |from: [f32; 3], radius: f32| {
            let offset = [0, 1, 2].map(|axis| point[axis] - from[axis]);
            let length = dot(offset, offset).sqrt();
            let direction = if length > 1e-9 {
                offset.map(|value| value / length)
            } else {
                [0.0, 1.0, 0.0]
            };
            (length - radius, direction)
        };
        match *self {
            Contact::Plane { normal, offset } => (dot(normal, point) - offset, normal),
            Contact::Sphere { centre, radius } => away(centre, radius),
            Contact::Capsule { start, end, radius } => {
                let along = [0, 1, 2].map(|axis| end[axis] - start[axis]);
                let length_squared = dot(along, along);
                let share = if length_squared > 1e-12 {
                    (dot([0, 1, 2].map(|axis| point[axis] - start[axis]), along) / length_squared)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };
                away(
                    [0, 1, 2].map(|axis| start[axis] + along[axis] * share),
                    radius,
                )
            }
        }
    }
}

/// How deep each contact presses into any of `points` (metres, 0 where it touches none): what
/// sizes the ring round it ([`deform_by_contacts`]). The scene's every `:deform` draw's posed
/// points together, so meshes that meet (a body and its face) rise alike at their seam.
pub fn contact_depths<'p>(
    contacts: &[Contact],
    points: impl IntoIterator<Item = &'p [f32; 3]>,
) -> Vec<f32> {
    let mut depths = vec![0.0f32; contacts.len().min(MAX_CONTACTS)];
    for point in points {
        for (depth, contact) in depths.iter_mut().zip(contacts) {
            let (distance, _) = contact.signed_distance(*point);
            *depth = depth.max(-distance);
        }
    }
    depths
}

/// A posed skin point and its unit normal, pressed by `contacts` whose deepest presses are
/// `depths` (ADR-805): inside one, laid onto its surface, its normal turning to face into it (out
/// of the body: the surface's normal reversed);
/// just outside, flesh of `softness` (0 bone … 1 soft) lifted along its normal in a ring as wide
/// as the press is deep, its normal tilted with the ring's slope. The shader's
/// `deform_by_contacts` is the same.
pub fn deform_by_contacts(
    contacts: &[Contact],
    depths: &[f32],
    drags: &[[f32; 3]],
    point: [f32; 3],
    normal: [f32; 3],
    softness: f32,
    skipped: u16,
) -> ([f32; 3], [f32; 3]) {
    // bone, or the limb that presses (softness 0): never pressed
    if softness <= 0.0 {
        return (point, normal);
    }
    let mut point = point;
    let mut normal = normal;
    for (slot, (contact, &depth)) in contacts.iter().zip(depths).enumerate() {
        // (a contact whose own limb this skin is presses it not: `skipped_by`)
        if skipped & (1 << slot) != 0 {
            continue;
        }
        let (distance, out) = contact.signed_distance(point);
        // the patch's edge eased over `band`: the distance's smooth maximum with 0, so skin meets
        // the surface tangentially instead of creasing where the patch ends
        let band = EDGE_BAND + EDGE_SPREAD * depth;
        let eased = smooth_outside(distance, band);
        // how far into the patch, 0 outside the band … 1 inside it
        let into = 1.0 - smoothstep(-band, band, distance);
        // a firm part (a nipple) sits a little behind the surface, sunk into the softer flesh round it
        let inset = FIRM_INSET * depth * (1.0 - softness / FIRM_SOFTNESS).clamp(0.0, 1.0) * into;
        // at most `MAX_SQUASH` × softness: tissue gives only so far (a lid, bone, barely); pushed
        // further, the skin passes the surface with the rest of her rather than opening holes
        let moved = (eased - distance - inset).min(MAX_SQUASH * softness);
        // the ring: nothing at the patch's edge, rising to its crest a little way out, falling
        // away; as wide as the press is deep, wider and higher for soft flesh
        let (mut lift, mut slope) = (0.0, 0.0);
        if depth > 0.0 && distance > 0.0 {
            let width = (RING_WIDTH + RING_SPREAD * depth) * (0.5 + softness);
            let reach = distance / width;
            if reach < 3.0 {
                let crest = RING_LIFT * softness * depth;
                let fall = (0.5 - reach * reach).exp() * std::f32::consts::SQRT_2;
                lift = crest * reach * fall;
                slope = crest / width * fall * (1.0 - 2.0 * reach * reach);
            }
        }
        let across = dot(out, normal);
        let sideways = [0, 1, 2].map(|axis| out[axis] - normal[axis] * across);
        point = [0, 1, 2].map(|axis| point[axis] + out[axis] * moved + normal[axis] * lift);
        // dragged along the surface with the patch (its friction: `:drag`), wholly inside it, less
        // and less through its edge and its ring, so the skin round it stretches with it
        if let Some(drag) = drags.get(slot) {
            let reach = (RING_WIDTH + RING_SPREAD * depth) * (0.5 + softness);
            let outside = distance.max(0.0) / reach;
            let held = into + (1.0 - into) * (-outside * outside).exp();
            point = [0, 1, 2].map(|axis| point[axis] + drag[axis] * held);
        }
        // its normal turns to face into the contact (out of the body) across the band, and leans
        // with the ring's slope: toward the contact where the ring rises, away where it falls
        let turned =
            normalize([0, 1, 2].map(|axis| normal[axis] + (-out[axis] - normal[axis]) * into));
        normal = if dot(sideways, sideways) > 1e-8 {
            let away = normalize(sideways);
            normalize([0, 1, 2].map(|axis| turned[axis] - away[axis] * slope))
        } else {
            turned
        };
    }
    (point, normal)
}

/// `distance`'s smooth maximum with 0 over `band` (ADR-805): 0 deep inside, `distance` well
/// outside, and between them a curve meeting both with their slope.
pub fn smooth_outside(distance: f32, band: f32) -> f32 {
    let share = ((band - distance.abs()) / band.max(1e-9)).max(0.0);
    distance.max(0.0) + share * share * band * 0.25
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
/// `turn` is how far the reflection turns across half a pixel there ([`highlight_exponent`]).
pub fn specular_lobes(material: &Material, normal_dot_half: f32, turn: f32) -> f32 {
    let [rough_a, rough_b, mix] = material.lobes();
    let first = specular_lobe(material.roughness * rough_a, normal_dot_half, turn);
    if mix >= 1.0 {
        first
    } else {
        mix * first
            + (1.0 - mix) * specular_lobe(material.roughness * rough_b, normal_dot_half, turn)
    }
}

/// One Blinn-Phong lobe of `roughness`, normalised to `(n + 8) / 8` ([`specular_lobes`]).
pub fn specular_lobe(roughness: f32, normal_dot_half: f32, turn: f32) -> f32 {
    let exponent = highlight_exponent(roughness, turn);
    (exponent + 8.0) / 8.0 * normal_dot_half.max(0.0).powf(exponent)
}

/// The specular exponent of a lobe of `roughness`, `2 / roughness⁴ − 2`, at least 1 and the
/// roughness held to [`MIN_ROUGHNESS`] … 1 (so at most 781 248): what a reflection of the
/// surroundings is as sharp as (ADR-795). [`shininess`] with no ceiling of 4096.
pub fn lobe_exponent(roughness: f32) -> f32 {
    let clamped = roughness.clamp(MIN_ROUGHNESS, 1.0);
    (2.0 / (clamped * clamped * clamped * clamped) - 2.0).max(1.0)
}

/// The exponent of the highlight a light without a `:shape` makes on a lobe of `roughness`
/// (ADR-795): [`lobe_exponent`]'s, which up to [`RESOLVED_EXPONENT`] (a roughness of about
/// 0.149 and rougher) is what it always was. A sharper lobe is as sharp as its roughness asks
/// on a flat surface, and on a curved one only as sharp as a pixel resolves, never under
/// `RESOLVED_EXPONENT`: `turn` is how far (radians) the reflection turns across half a
/// picture pixel there, twice the mesh's curvature times the metres the pixel spans, and a
/// lobe of solid angle `8π / (n + 8)` narrower than `(2 turn)²` would be a point lighting one
/// pixel in one frame and none in the next.
pub fn highlight_exponent(roughness: f32, turn: f32) -> f32 {
    let exponent = lobe_exponent(roughness);
    if exponent <= RESOLVED_EXPONENT || turn <= 0.0 {
        return exponent;
    }
    let resolved = 2.0 * std::f32::consts::PI / (turn * turn) - 8.0;
    exponent.min(resolved.max(RESOLVED_EXPONENT))
}

/// The half-width, per metre travelled, of the square a specular lobe of `exponent` covers
/// where its reflection lands (ADR-795): the lobe taken as a box of the solid angle
/// `Ω = 8π / (n + 8)` — the one whose height is the Blinn-Phong lobe's peak `(n + 8) / 8π` —
/// so `√Ω / 2`, and never under `turn` ([`highlight_exponent`]), the only anti-aliasing a
/// sharp reflected edge gets on a curved surface.
pub fn lobe_spread(exponent: f32, turn: f32) -> f32 {
    ((8.0 * std::f32::consts::PI / (exponent + 8.0)).sqrt() * 0.5).max(turn)
}

/// The metres one of the picture's pixels spans at `world`, seen along `view` (the unit
/// direction to the eye): where a centimetre across the line of sight lands, as
/// [`strand_vertex`] measures a strand's pixel. 0 where it cannot be told (behind the eye).
pub fn pixel_span(scene: &Scene, world: [f32; 3], view: [f32; 3]) -> f32 {
    let (across, _) = basis(view);
    let (pixel_width, pixel_height) = scene.pixel_size();
    let near = transform(&scene.view_proj, world, 1.0);
    let far = transform(
        &scene.view_proj,
        [0, 1, 2].map(|axis| world[axis] + across[axis] * 0.01),
        1.0,
    );
    if near[3] > 1e-6 && far[3] > 1e-6 {
        let x = (far[0] / far[3] - near[0] / near[3]) * pixel_width as f32 * 0.5;
        let y = (far[1] / far[3] - near[1] / near[3]) * pixel_height as f32 * 0.5;
        let pixels = (x * x + y * y).sqrt();
        if pixels > 1e-6 {
            return 0.01 / pixels;
        }
    }
    0.0
}

/// The radiance arriving along the unit `direction` from an environment whose radiance has
/// the SH coefficients `coefficients` ([`sh_irradiance`]'s convention), blurred over a
/// specular lobe of `exponent` (ADR-795): band l scaled by `exp(−2 l² / n)` — Ramamoorthi and
/// Hanrahan's `exp(−l² / 2s)` for a Phong lobe of exponent s about the reflection, a
/// Blinn-Phong lobe of n being one of n / 4 — and held at 0. A uniform radiance comes back
/// as itself at any exponent.
pub fn sh_radiance(coefficients: &[[f32; 3]; 9], direction: [f32; 3], exponent: f32) -> [f32; 3] {
    let [x, y, z] = direction;
    let first = (-2.0 / exponent).exp();
    let second = (-8.0 / exponent).exp();
    let weights = [
        0.282_095,
        0.488_603 * y * first,
        0.488_603 * z * first,
        0.488_603 * x * first,
        1.092_548 * x * y * second,
        1.092_548 * y * z * second,
        0.315_392 * (3.0 * z * z - 1.0) * second,
        1.092_548 * x * z * second,
        0.546_274 * (x * x - y * y) * second,
    ];
    [0, 1, 2].map(|channel| {
        let radiance: f32 = weights
            .iter()
            .zip(coefficients)
            .map(|(weight, coefficient)| weight * coefficient[channel])
            .sum();
        radiance.max(0.0)
    })
}

/// The surroundings' radiance a specular lobe of `exponent` about the unit `direction`
/// reflects (ADR-795): [`sh_radiance`] of the scene's `ambient_sh` when it has one, else
/// `ground` blended to `sky` by how far the direction points up, that blend drawn toward the
/// middle as band 1 is (`exp(−2 / n)`).
pub fn ambient_radiance(scene: &Scene, direction: [f32; 3], exponent: f32) -> [f32; 3] {
    match &scene.ambient_sh {
        Some(coefficients) => sh_radiance(coefficients, direction, exponent),
        None => {
            let up = direction[1] * (-2.0 / exponent).exp() * 0.5 + 0.5;
            [0, 1, 2].map(|channel| {
                scene.ground[channel] + (scene.sky[channel] - scene.ground[channel]) * up
            })
        }
    }
}

/// Karis's closed-form fit of the split-sum environment BRDF (`EnvBRDFApprox`, "Physically
/// Based Shading on Mobile", 2014): `[scale, bias]`, a surface of `roughness` seen at the
/// cosine `normal_dot_view` reflecting `F0 × scale + F90 × bias` of a uniform surrounding. No
/// table: both painters evaluate it. `scale + bias` is `1 − 0.55 × roughness`, never above 1.
pub fn environment_brdf(roughness: f32, normal_dot_view: f32) -> [f32; 2] {
    let rough = roughness.clamp(0.0, 1.0);
    let curve = [
        1.0 - rough,
        0.0425 - 0.0275 * rough,
        1.04 - 0.572 * rough,
        0.022 * rough - 0.04,
    ];
    let grazing = (curve[0] * curve[0]).min((-9.28 * normal_dot_view).exp2()) * curve[0] + curve[1];
    // the fit strays a few hundredths outside 0 … 1 at its ends (a mirror at a grazing angle,
    // chalk head on): held, so nothing reflects more than arrives or less than nothing
    let scale = (curve[2] - 1.04 * grazing).max(0.0);
    [scale, (curve[3] + 1.04 * grazing).clamp(0.0, 1.0 - scale)]
}

/// How much of a reflection reaches a point whose ambient light is `occlusion` (its map's
/// times the scene's [`ambient_occlusion`]; 1 open) — Lagarde's specular occlusion ("Moving
/// Frostbite to PBR", 2014): `clamp((n·v + ao)^(2^(−16 r − 1)) − 1 + ao)`. Exactly 1 for an
/// open point and 0 for a fully occluded one; between, a smooth surface seen head on keeps
/// more of its reflection than its diffuse keeps of the ambient.
pub fn specular_occlusion(normal_dot_view: f32, occlusion: f32, roughness: f32) -> f32 {
    let power = (-16.0 * roughness.clamp(0.0, 1.0) - 1.0).exp2();
    ((normal_dot_view + occlusion).max(0.0).powf(power) - 1.0 + occlusion).clamp(0.0, 1.0)
}

/// How much of a reflection along the unit `reflected` is kept where a normal map (or a
/// smoothed normal) has turned it under the surface whose own unit normal is
/// `surface_normal`: `clamp(1 + r·n)²`, 1 for any reflection leaving the surface.
pub fn horizon(reflected: [f32; 3], surface_normal: [f32; 3]) -> f32 {
    let above = (1.0 + dot(reflected, surface_normal)).clamp(0.0, 1.0);
    above * above
}

/// How an [`emitter_plan`]'s shape is tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlannedShape {
    Disc,
    Rect,
    Sphere,
    /// A directional light's disc, at no distance: `half[0]` the tangent of half its angle.
    Distant,
}

impl PlannedShape {
    /// The number the scene uniform carries for it (0 is no emitter).
    pub fn code(self) -> u32 {
        match self {
            PlannedShape::Disc => 1,
            PlannedShape::Rect => 2,
            PlannedShape::Sphere => 3,
            PlannedShape::Distant => 4,
        }
    }
}

/// One shape a scene's glossy surfaces show this frame (ADR-795).
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedEmitter {
    pub shape: PlannedShape,
    pub centre: [f32; 3],
    /// The unit direction its light travels: the way a disc or rectangle faces.
    pub facing: [f32; 3],
    /// A rectangle's unit width and height directions, square to `facing`.
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// A rectangle's half width and half height; a disc's or sphere's radius in the first.
    pub half: [f32; 2],
    /// Linear radiance: a shaped light's is `π × color / area`, the radiance that brings the
    /// light its `:color` does, so its image and its diffuse light are on one scale.
    pub radiance: [f32; 3],
    /// The light it is the shape of, whose shadow, cone and range window it keeps; none for
    /// one of the scene's `:emitters`.
    pub light: Option<usize>,
}

/// The shapes a scene's glossy surfaces show this frame, planned once for both painters as
/// [`shadow_plan`] plans the shadow maps: every shaped light, then the scene's `:emitters`,
/// [`MAX_EMITTERS`] at most.
#[derive(Clone, Debug, PartialEq)]
pub struct EmitterPlan {
    pub emitters: Vec<PlannedEmitter>,
    /// Per light, by index: whether it is in `emitters`, its Blinn-Phong highlight then
    /// skipped so nothing is counted twice.
    pub shaped: [bool; MAX_LIGHTS],
}

/// A rectangle's width and height directions: square to the unit `facing`, the height as
/// near `up` as that allows (any side when `up` runs along `facing`).
fn emitter_frame(facing: [f32; 3], up: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let across = cross(up, facing);
    let right = if dot(across, across) > 1e-12 {
        normalize(across)
    } else {
        basis(facing).0
    };
    (right, cross(facing, right))
}

/// `scene`'s [`EmitterPlan`]. A shaped light whose size is nothing (a `:size` of 0) is left a
/// point, with its Blinn-Phong highlight.
pub fn emitter_plan(scene: &Scene) -> EmitterPlan {
    let mut plan = EmitterPlan {
        emitters: Vec::new(),
        shaped: [false; MAX_LIGHTS],
    };
    for (index, light) in scene.lights.iter().take(MAX_LIGHTS).enumerate() {
        let Some(shape) = light.shape else {
            continue;
        };
        let (planned, half) = match (light.kind, shape.shape) {
            (LightKind::Directional, _) => (
                PlannedShape::Distant,
                [(light.size * 0.5).min(1.5).tan(), 0.0],
            ),
            (_, EmitterShape::Rect) => (
                PlannedShape::Rect,
                [shape.extent[0] * 0.5, shape.extent[1] * 0.5],
            ),
            (_, EmitterShape::Disc) => (PlannedShape::Disc, [light.size * 0.5, 0.0]),
            (_, EmitterShape::Sphere) => (PlannedShape::Sphere, [light.size * 0.5, 0.0]),
        };
        let area = if planned == PlannedShape::Rect {
            4.0 * half[0] * half[1]
        } else {
            std::f32::consts::PI * half[0] * half[0]
        };
        if area.is_nan() || area <= 0.0 || plan.emitters.len() == MAX_EMITTERS {
            continue;
        }
        let facing = normalize(light.direction);
        let (right, up) = emitter_frame(facing, shape.up);
        plan.shaped[index] = true;
        plan.emitters.push(PlannedEmitter {
            shape: planned,
            centre: light.position,
            facing,
            right,
            up,
            half,
            radiance: light
                .color
                .map(|channel| std::f32::consts::PI * channel / area),
            light: Some(index),
        });
    }
    for emitter in &scene.emitters {
        if plan.emitters.len() == MAX_EMITTERS {
            break;
        }
        let facing = normalize(emitter.direction);
        let (right, up) = emitter_frame(facing, emitter.up);
        plan.emitters.push(PlannedEmitter {
            shape: match emitter.shape {
                EmitterShape::Disc => PlannedShape::Disc,
                EmitterShape::Rect => PlannedShape::Rect,
                EmitterShape::Sphere => PlannedShape::Sphere,
            },
            centre: emitter.position,
            facing,
            right,
            up,
            half: [emitter.extent[0] * 0.5, emitter.extent[1] * 0.5],
            radiance: emitter.radiance,
            light: None,
        });
    }
    plan
}

/// How much of the segment `offset ± reach` lies within `± half`: 0 … 1.
pub fn overlap(offset: f32, half: f32, reach: f32) -> f32 {
    (((offset + reach).min(half) - (offset - reach).max(-half)) / (2.0 * reach)).clamp(0.0, 1.0)
}

/// How much of a specular lobe leaving `world` along the unit `reflected` lands on `emitter`
/// (ADR-795), 0 … 1; `spread` is the lobe's [`lobe_spread`]. The reflection is followed to the
/// emitter's plane — a rectangle's or disc's own, a sphere's the one through its centre facing
/// the point, a distant disc's a metre away — which it must reach from the lit side; there the
/// lobe covers a square of half-width `distance × spread / √cos`, the cosine the one the
/// reflection meets the plane at (so a small emitter seen aslant covers as little of the lobe
/// as its solid angle is). A rectangle's coverage is the square's overlap with it along its
/// width times along its height; a disc's or sphere's falls from 1 to 0 as the square leaves
/// it, times `min(1, π a² / 4 reach²)`, the share of the square it can fill. A mirror's lobe
/// inside the shape is covered exactly 1; a shape shrunk to a point covers `area / (distance²
/// Ω)` of it, which times its radiance is the Blinn-Phong highlight's peak.
pub fn emitter_coverage(
    emitter: &PlannedEmitter,
    world: [f32; 3],
    reflected: [f32; 3],
    spread: f32,
) -> f32 {
    let (centre, facing) = match emitter.shape {
        PlannedShape::Sphere => (emitter.centre, normalize(subtract(world, emitter.centre))),
        PlannedShape::Distant => (subtract(world, emitter.facing), emitter.facing),
        PlannedShape::Disc | PlannedShape::Rect => (emitter.centre, emitter.facing),
    };
    let toward = -dot(reflected, facing);
    let height = dot(subtract(world, centre), facing);
    if toward.is_nan() || toward <= 1e-4 || height.is_nan() || height <= 0.0 {
        return 0.0;
    }
    let distance = height / toward;
    let reach = (distance * spread / toward.sqrt()).max(1e-9);
    let hit = [0, 1, 2].map(|axis| world[axis] + reflected[axis] * distance - centre[axis]);
    if emitter.shape == PlannedShape::Rect {
        return overlap(dot(hit, emitter.right), emitter.half[0], reach)
            * overlap(dot(hit, emitter.up), emitter.half[1], reach);
    }
    let radius = emitter.half[0];
    let edge =
        ((radius + reach - dot(hit, hit).sqrt()) / (2.0 * radius.min(reach))).clamp(0.0, 1.0);
    edge * (std::f32::consts::PI * radius * radius / (4.0 * reach * reach)).min(1.0)
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

/// An eye's frame in the world, as a vertex of an `:eye` surface carries it to its fragments
/// (ADR-796): the eye's centre, its axis and its texture's up — neither made unit length, the
/// axis's length being how much the model is scaled — and which eye of the material it is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EyeFrame {
    pub centre: [f32; 3],
    pub axis: [f32; 3],
    pub up: [f32; 3],
    pub index: f32,
}

/// The frame of the eye a vertex at `rest` (the mesh's space, morphed) belongs to
/// ([`Eye::nearest`]), moved by `matrix`, the one that skins and places that vertex. The
/// shader's `eye_frame` is the same.
pub fn eye_frame(eye: &Eye, matrix: &[f32; 16], rest: [f32; 3]) -> EyeFrame {
    let index = eye.nearest(rest);
    let Some(at) = eye.eyes.get(index) else {
        return EyeFrame::default();
    };
    let centre = transform(matrix, at.centre, 1.0);
    let axis = transform(matrix, at.axis, 0.0);
    let up = transform(matrix, eye.up, 0.0);
    EyeFrame {
        centre: [centre[0], centre[1], centre[2]],
        axis: [axis[0], axis[1], axis[2]],
        up: [up[0], up[1], up[2]],
        index: index as f32,
    }
}

/// What a sight line meets in an eye ([`eye_seen`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeSeen {
    /// Whether it passed through the cornea. When not, the rest is the surface's own.
    pub through: bool,
    /// Where the base texture is read: under the cornea, where the bent line meets the iris.
    pub uv: [f32; 2],
    /// How much of the pupil is there, 0 … 1.
    pub pupil: f32,
    /// The normal the highlights are reflected about: the cornea's, the eyeball's own at the
    /// limbus and over the white (ADR-804).
    pub normal: [f32; 3],
    /// How much of that normal is the cornea's, 0 … 1.
    pub cornea: f32,
    /// How sharply the reflecting surface curves there.
    pub curvature: f32,
    /// The normal the light falls on under the cornea: the iris's, lit as a shallow bowl — the
    /// eye's axis leaning toward itself by the material's `concavity` times the distance from
    /// it (as a share of the iris's radius, 1 at most). The mesh's when not `through`.
    pub iris_normal: [f32; 3],
    /// The eye's unit axis, about which the light is bent on its way in ([`light_under_cornea`]).
    pub axis: [f32; 3],
}

/// What bends the light on its way to a surface under a cornea ([`shade`]): the eye's unit
/// `axis`, the normal of its front, and `ratio`, 1 over its refractive index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnderCornea {
    pub axis: [f32; 3],
    pub ratio: f32,
}

/// The light reaching a surface under a cornea (ADR-796): `to_light` the unit direction to
/// the light outside. Answers the unit direction it comes from inside, bent toward the eye's
/// axis by Snell's law at the eye's front, and the share of it that gets in: none from behind
/// the front, the rest less what the front reflects (Schlick, the reflectance at normal
/// incidence that of the index). So a light from the side still lights the iris, from 46° off
/// the axis at most, where on a bare disc it would graze it. The shader's is the same.
pub fn light_under_cornea(under: &UnderCornea, to_light: [f32; 3]) -> ([f32; 3], f32) {
    let cosine = dot(under.axis, to_light);
    if cosine <= 0.0 {
        return (to_light, 0.0);
    }
    let ratio = under.ratio;
    let inside = (1.0 - ratio * ratio * (1.0 - cosine * cosine)).max(0.0);
    let bend = inside.sqrt() - ratio * cosine;
    let bent = [0, 1, 2].map(|index| ratio * to_light[index] + bend * under.axis[index]);
    let head_on = (1.0 - ratio) / (1.0 + ratio);
    (bent, 1.0 - fresnel(head_on * head_on, cosine))
}

/// What the sight line through a fragment of an `:eye` surface meets (ADR-796): `world` the
/// fragment, `view` the unit direction from it to the viewer, `uv`, `normal` and `curvature`
/// the mesh's there, `frame` its eye's. The mesh only says which pixels the eye covers: the
/// line through the fragment is met with the cornea's sphere on the viewer's side, bent there
/// by Snell's law (`ior`), and followed to the iris's plane, and the texture is read at that
/// point's place on the iris's disc. A line that misses the cornea's cap (the white) keeps the
/// mesh's uv and the light falls on the mesh's normal, but it reflects about the eyeball's own
/// normal (ADR-804). The shader's `eye_seen` is the same.
pub fn eye_seen(
    eye: &Eye,
    frame: &EyeFrame,
    world: [f32; 3],
    view: [f32; 3],
    uv: [f32; 2],
    normal: [f32; 3],
    curvature: f32,
) -> EyeSeen {
    let missed = EyeSeen {
        through: false,
        uv,
        pupil: 0.0,
        normal,
        cornea: 0.0,
        curvature,
        iris_normal: normal,
        axis: [0.0; 3],
    };
    let scale = dot(frame.axis, frame.axis).sqrt();
    let Some(at) = eye.eyes.get((frame.index + 0.5) as usize) else {
        return missed;
    };
    if scale <= 1e-12 {
        return missed;
    }
    let axis = [0, 1, 2].map(|index| frame.axis[index] / scale);
    // the white reflects as the ball it is (ADR-804): its own normal, not the mesh's, which
    // around the limbus may lean any way the mesh was modelled (a bowl for a painted iris)
    let ball = normalize([0, 1, 2].map(|index| world[index] - frame.centre[index]));
    let missed = EyeSeen {
        normal: ball,
        ..missed
    };
    let iris_radius = eye.iris_radius * scale;
    let cornea_radius = eye.cornea_radius * scale;
    let ball_radius = eye.radius * scale;
    // heights along the axis from the eye's centre: the limbus's plane, the cornea's centre,
    // the iris's plane
    let limbus_height = (ball_radius * ball_radius - iris_radius * iris_radius)
        .max(0.0)
        .sqrt();
    let cornea_height = limbus_height
        - (cornea_radius * cornea_radius - iris_radius * iris_radius)
            .max(0.0)
            .sqrt();
    let iris_height = cornea_height + cornea_radius - eye.iris_depth * scale;
    // the fragment from the cornea's centre, and where the line through it toward the viewer
    // leaves the cornea's sphere: where the sight line came in
    let from_cornea =
        [0, 1, 2].map(|index| world[index] - frame.centre[index] - axis[index] * cornea_height);
    let along = dot(from_cornea, view);
    let discriminant =
        along * along - (dot(from_cornea, from_cornea) - cornea_radius * cornea_radius);
    if discriminant < 0.0 {
        return missed;
    }
    let travel = discriminant.sqrt() - along;
    if travel < 0.0 {
        return missed;
    }
    let entry = [0, 1, 2].map(|index| from_cornea[index] + view[index] * travel);
    let entry_along = dot(entry, axis);
    if entry_along + cornea_height < limbus_height {
        return missed;
    }
    let cornea_normal = [0, 1, 2].map(|index| entry[index] / cornea_radius);
    // Snell: the line going in is −view, the index 1 outside
    let cosine = dot(cornea_normal, view);
    let ratio = 1.0 / eye.ior.max(1.0);
    let inside = (1.0 - ratio * ratio * (1.0 - cosine * cosine)).max(0.0);
    let bend = ratio * cosine - inside.sqrt();
    let bent = [0, 1, 2].map(|index| bend * cornea_normal[index] - ratio * view[index]);
    let sinking = dot(bent, axis);
    if sinking > -1e-6 {
        return missed;
    }
    let reach = (iris_height - entry_along - cornea_height) / sinking;
    // where it meets the iris's plane, from the eye's centre, in the texture's directions
    let hit =
        [0, 1, 2].map(|index| entry[index] + axis[index] * cornea_height + bent[index] * reach);
    let lean = dot(frame.up, axis);
    let up = normalize([0, 1, 2].map(|index| frame.up[index] - axis[index] * lean));
    let right = cross(up, axis);
    let across = dot(hit, right) / iris_radius;
    let above = dot(hit, up) / iris_radius;
    let rim = (across * across + above * above).sqrt();
    let pupil = if eye.pupil > 0.0 {
        1.0 - smoothstep(
            eye.pupil * (1.0 - PUPIL_EDGE),
            eye.pupil * (1.0 + PUPIL_EDGE),
            rim,
        )
    } else {
        0.0
    };
    // the cornea's normal, giving way to the mesh's toward the limbus
    let entry_out = [0, 1, 2].map(|index| entry[index] - axis[index] * entry_along);
    let entry_rim = dot(entry_out, entry_out).sqrt() / iris_radius;
    let cornea = 1.0 - smoothstep(CORNEA_EDGE, 1.0, entry_rim);
    // the iris as a shallow bowl: its normal leans toward the axis, more the further out
    let slope = eye.concavity / rim.max(1.0);
    let iris_normal = normalize(
        [0, 1, 2].map(|index| axis[index] - slope * (right[index] * across + up[index] * above)),
    );
    EyeSeen {
        through: true,
        uv: [
            at.iris_uv[0] + across * eye.iris_uv_radius,
            at.iris_uv[1] - above * eye.iris_uv_radius,
        ],
        pupil,
        // the cornea's normal, giving way to the ball's toward the limbus (ADR-804)
        normal: normalize(
            [0, 1, 2].map(|index| ball[index] + (cornea_normal[index] - ball[index]) * cornea),
        ),
        cornea,
        curvature: curvature + (1.0 / cornea_radius - curvature) * cornea,
        iris_normal,
        axis,
    }
}

/// The light leaving a surface point toward the eye, linear and before exposure: `base` its
/// linear colour, `normal` its unit normal facing the viewer's side, `view` the unit
/// direction from it to the eye, `world` where it is, `occlusion` how much of the ambient
/// reaches it (its occlusion map's times the scene's [`ambient_occlusion`], 1 for all of it),
/// `visibility` how much of each light (by index) its shadow lets through — 1 for a light it
/// holds no entry for — `strand` a `:hair` surface's unit strand direction (unused by the
/// others), `curvature` how sharply the mesh curves there, 1 over its radius in the mesh's
/// units ([`Mesh::curvature`]: a `:skin` surface's scattering, and how sharp a reflection a
/// pixel resolves), `specular_kept` how much of the specular its detail tile's cavity leaves
/// there ([`detail_specular`]; 1 without one, and not read by a `:hair` or `:strands`
/// surface's highlights), `surface_normal` the surface's own unit normal, no normal map's (a
/// reflection under it is faded out), `emitters` the scene's [`emitter_plan`], and
/// `diffuse_normal` the unit normal the diffuse and the ambient light fall on: `normal` itself
/// for every surface but an `:eye` under its cornea (ADR-796), whose highlights are the
/// cornea's (`normal`) while the light falls on what is under it — bent on its way in and
/// lessened by what the eye's front reflects when `under_cornea` says so
/// ([`light_under_cornea`]; none for every other surface).
///
/// Every surface but `:hair` and `:strands` also reflects its surroundings (ADR-795), a lobe
/// of its `:lobes` at a time and then its `:coat`: the ambient light's radiance along the
/// reflection, blurred as the lobe is rough ([`ambient_radiance`]) and scaled by
/// [`environment_brdf`]; and each planned emitter's radiance times the share of the lobe it
/// covers ([`emitter_coverage`]) and Schlick's Fresnel at the normal — for a shaped light
/// times its shadow, cone and range window too, its Blinn-Phong highlight skipped. Both are
/// scaled by [`specular_occlusion`] and [`horizon`].
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
    surface_normal: [f32; 3],
    emitters: &EmitterPlan,
    diffuse_normal: [f32; 3],
    under_cornea: Option<&UnderCornea>,
) -> [f32; 3] {
    let ambient = ambient(scene, diffuse_normal);
    let skin = (material.surface == Surface::Skin)
        .then(|| (skin_table(), curvature * material.scatter_scale()));
    let mut out = [0, 1, 2].map(|channel| base[channel] * ambient[channel] * occlusion);
    let f0 = material.f0();
    // a `:hair` surface's highlights: as sharp as its roughness asks
    let exponent = shininess(material.roughness);
    let reflects = !matches!(material.surface, Surface::Hair | Surface::Strands);
    // how far the reflection turns across half a pixel: twice the curvature, times the pixel
    let reflection_turn = if reflects && curvature > 0.0 {
        2.0 * curvature * pixel_span(scene, world, view)
    } else {
        0.0
    };
    let coat = material.coat.filter(|coat| reflects && coat[0] > 0.0);
    // per light: how much of its shape's image is left by its shadow, cone and range window
    let mut shaping = [0.0f32; MAX_LIGHTS];
    for (index, light) in scene.lights.iter().take(MAX_LIGHTS).enumerate() {
        let (to_light, arriving) = light_arrival(light, world);
        let seen = visibility.get(index).copied().unwrap_or(1.0);
        // under a cornea the light is bent on its way in, and only some of it gets in (ADR-796)
        let (falling, let_in) = match under_cornea {
            Some(under) => light_under_cornea(under, to_light),
            None => (to_light, 1.0),
        };
        let facing = dot(diffuse_normal, falling);
        // the light past the terminator is light under the skin, which the body's own shadow
        // there would cut off in a hard line: the shadow fades out of what the scattering
        // moved toward the terminator (ADR-793)
        let scattered_seen = 1.0 + (seen - 1.0) * smoothstep(0.0, 0.3, facing);
        // (under a cornea, the share of the light that got in)
        let lambert = match under_cornea {
            Some(_) => facing.max(0.0) * let_in,
            None => facing.max(0.0),
        };
        // the highlights' own: the reflecting normal's (the same number but under a cornea)
        let mirror_lambert = dot(normal, to_light).max(0.0);
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
            let primary = highlight(material.hair_shift[0], exponent);
            let secondary = highlight(material.hair_shift[1], exponent * HAIR_SECONDARY_SHARPNESS);
            // the second highlight's colour: the hair's own, or the material's `:sheen`
            let sheen = material.sheen.unwrap_or(base);
            let lit_side = (facing + 0.25).clamp(0.0, 1.0);
            let diffuse = (0.25 + 0.75 * facing).clamp(0.0, 1.0);
            // what the hair before it let through, in the hair's colour
            let passed = hair_light(seen, base);
            for channel in 0..3 {
                let shine = material.specular * lit_side * (primary + secondary * sheen[channel]);
                out[channel] += light.color[channel]
                    * arriving
                    * passed[channel]
                    * (base[channel] * diffuse + shine);
            }
            continue;
        }
        // a pore reflects less than the open surface round it (ADR-797); the diffuse is not
        // touched
        // a shaped light's highlight is its shape's image, below: its inverse square is the
        // image's size, so only its cone and range window are kept
        let shaped = emitters.shaped[index];
        if shaped {
            let spread_out = match light.kind {
                LightKind::Directional => 1.0,
                LightKind::Point | LightKind::Spot => {
                    let toward = subtract(light.position, world);
                    dot(toward, toward).max(NEAR_LIGHT * NEAR_LIGHT)
                }
            };
            shaping[index] = seen * arriving * spread_out;
        }
        let specular = if shaped {
            0.0
        } else {
            let highlight = fresnel(f0, dot(view, half))
                * specular_lobes(material, dot(normal, half), reflection_turn)
                * mirror_lambert
                * specular_kept;
            match coat {
                Some([strength, roughness]) => {
                    highlight
                        + strength
                            * fresnel(COAT_F0, dot(view, half))
                            * specular_lobe(roughness, dot(normal, half), reflection_turn)
                            * mirror_lambert
                }
                None => highlight,
            }
        };
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
    if !reflects {
        return out;
    }
    // the surroundings, reflected (ADR-795): a layer at a time, each a weight, a roughness and
    // a reflectance at normal incidence — the material's lobes (a pore reflecting less,
    // ADR-797), then its coat
    let facing_view = dot(normal, view);
    let normal_dot_view = facing_view.clamp(0.0, 1.0);
    let reflected = [0, 1, 2].map(|axis| 2.0 * facing_view * normal[axis] - view[axis]);
    let kept = horizon(reflected, surface_normal);
    let [rough_a, rough_b, mix] = material.lobes();
    let [coat_strength, coat_roughness] = coat.unwrap_or([0.0, 0.0]);
    let layers = [
        (mix * specular_kept, material.roughness * rough_a, f0),
        (
            (1.0 - mix) * specular_kept,
            material.roughness * rough_b,
            f0,
        ),
        (coat_strength, coat_roughness, COAT_F0),
    ];
    for (weight, roughness, reflectance) in layers {
        if weight <= 0.0 {
            continue;
        }
        let exponent = lobe_exponent(roughness);
        let open = weight * kept * specular_occlusion(normal_dot_view, occlusion, roughness);
        let [scale, bias] = environment_brdf(roughness, normal_dot_view);
        let around = ambient_radiance(scene, reflected, exponent);
        let surrounding = open * (reflectance * scale + (50.0 * reflectance).min(1.0) * bias);
        for channel in 0..3 {
            out[channel] += around[channel] * surrounding;
        }
        let mirrored = open * fresnel(reflectance, normal_dot_view);
        let spread = lobe_spread(exponent, reflection_turn);
        for emitter in &emitters.emitters {
            let left = emitter.light.map_or(1.0, |index| shaping[index]);
            if left <= 0.0 {
                continue;
            }
            let covered = emitter_coverage(emitter, world, reflected, spread);
            for channel in 0..3 {
                out[channel] += emitter.radiance[channel] * (mirrored * left * covered);
            }
        }
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
    /// Where its map is kept (ADR-800), set by [`shadow_plan`].
    pub map: ShadowMapSlot,
}

/// Where a shadow view's map is kept (ADR-800): which of the scene's two map arrays (0, the
/// largest views'; 1, every smaller view's), its layer in that array, and that array's side.
/// The view renders into the top-left `resolution`² of its layer; reads past that up to the
/// array's side find the layer cleared to the far plane.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowMapSlot {
    pub array: u32,
    pub layer: u32,
    pub side: u32,
}

/// The two map arrays a plan's views are kept in (ADR-800): array 0 at the largest view's
/// resolution, holding every view of that resolution; array 1 at the largest of the others',
/// holding the rest (side 0 and no layers when there are none). Two size classes, not an
/// atlas: one big map and smaller others is the case that needed it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowArrays {
    pub sides: [u32; 2],
    pub layers: [u32; 2],
}

impl ShadowArrays {
    /// The arrays views of these resolutions need.
    pub fn of(resolutions: &[u32]) -> ShadowArrays {
        let large = resolutions.iter().copied().max().unwrap_or(0);
        let small = resolutions
            .iter()
            .copied()
            .filter(|resolution| *resolution < large)
            .max()
            .unwrap_or(0);
        let in_large = resolutions
            .iter()
            .filter(|resolution| **resolution == large)
            .count() as u32;
        ShadowArrays {
            sides: [large, small],
            layers: [in_large, resolutions.len() as u32 - in_large],
        }
    }

    /// Which array a view of `resolution` is kept in.
    pub fn array_of(&self, resolution: u32) -> usize {
        usize::from(resolution < self.sides[0])
    }

    /// The texels both arrays hold together: what [`MAX_SHADOW_TEXELS`] limits.
    pub fn texels(&self) -> u64 {
        (0..2)
            .map(|array| {
                u64::from(self.layers[array])
                    * u64::from(self.sides[array])
                    * u64::from(self.sides[array])
            })
            .sum()
    }
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
    /// The two map arrays the views are kept in (ADR-800); each view's `map` says where.
    pub arrays: ShadowArrays,
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
                map: ShadowMapSlot::default(),
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
                map: ShadowMapSlot::default(),
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
                    map: ShadowMapSlot::default(),
                })
                .collect()
        }
    }
}

/// The shadow maps `scene` renders this frame, its things lying in `world` (the union of its
/// draws' [`draw_bounds`]): the first [`MAX_SHADOWS`] shadowed lights' views, within
/// [`MAX_SHADOW_TEXELS`] counted as the two map arrays hold them ([`ShadowArrays::texels`],
/// ADR-800; the spec parser refuses a scene asking for more). A light whose views would pass
/// the limit is skipped.
pub fn shadow_plan(scene: &Scene, world: Option<Bounds>) -> ShadowPlan {
    let mut plan = ShadowPlan {
        views: Vec::new(),
        lights: [(0, 0); MAX_LIGHTS],
        arrays: ShadowArrays::default(),
        occlusion: None,
    };
    // the texels the plan's views and `more` would need together
    let texels_with = |views: &[ShadowView], more: &[u32]| {
        let resolutions: Vec<u32> = views
            .iter()
            .map(|view| view.resolution)
            .chain(more.iter().copied())
            .collect();
        ShadowArrays::of(&resolutions).texels()
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
        let resolutions: Vec<u32> = views.iter().map(|view| view.resolution).collect();
        if texels_with(&plan.views, &resolutions) > MAX_SHADOW_TEXELS {
            continue;
        }
        shadowed += 1;
        plan.lights[index] = (plan.views.len() as u32, views.len() as u32);
        plan.views.extend(views);
    }
    // the ambient occlusion's view: the camera's own, square (its depth is read by where a
    // point falls in the picture, not texel for pixel), as fine as the picture up to a limit
    if scene.ambient_occlusion.is_some() && plan.views.len() < MAX_SHADOW_VIEWS {
        let (width, height) = scene.pixel_size();
        let side = width.max(height).clamp(MIN_SHADOW_SIDE, MAX_OCCLUSION_SIDE);
        if texels_with(&plan.views, &[side]) <= MAX_SHADOW_TEXELS {
            plan.occlusion = Some(plan.views.len() as u32);
            plan.views.push(ShadowView {
                view_proj: scene.view_proj,
                resolution: side,
                texel: 0.0,
                // no penumbra is read from it: the occlusion turns its depths into distances
                // from the camera's own matrix (`view_distance`)
                depth_range: [0.0, 1.0],
                perspective: false,
                map: ShadowMapSlot::default(),
            });
        }
    }
    // each view to its array, in order: a view at the largest resolution in array 0, any
    // smaller one in array 1
    let resolutions: Vec<u32> = plan.views.iter().map(|view| view.resolution).collect();
    plan.arrays = ShadowArrays::of(&resolutions);
    let mut next = [0u32; 2];
    for view in &mut plan.views {
        let array = plan.arrays.array_of(view.resolution);
        view.map = ShadowMapSlot {
            array: array as u32,
            layer: next[array],
            side: plan.arrays.sides[array],
        };
        next[array] += 1;
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
/// edge reads the edge; off its right or bottom edge it reads the GPU target's layer, its
/// map array's `map.side` texels (ADR-800), cleared to the far plane (1) where the view did
/// not reach — as the GPU's clamped reads of that layer do.
fn stored_depth(view: &ShadowView, depth_map: &[f32], column: i64, row: i64) -> f32 {
    let side = view.resolution as i64;
    // a view no plan placed is its own map
    let last = i64::from(view.map.side.max(view.resolution).max(1)) - 1;
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
fn bilinear_tap(view: &ShadowView, depth_map: &[f32], [across, down]: [f32; 2], depth: f32) -> f32 {
    let (column, row) = (across.floor(), down.floor());
    let (fraction_x, fraction_y) = (across - column, down - row);
    let (column, row) = (column as i64, row as i64);
    let lit = |step_x: i64, step_y: i64| {
        let stored = stored_depth(view, depth_map, column + step_x, row + step_y);
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
pub fn sample_shadow(view: &ShadowView, depth_map: &[f32], point: [f32; 3]) -> f32 {
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
            let stored = stored_depth(view, depth_map, column + step_x as i64, row + step_y as i64);
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
        lit += bilinear_tap(view, depth_map, tap, plane(offset));
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
            depth_map,
            biased,
            &SoftFilter {
                size: light.size,
                softness,
                turn,
                slope: receiver_slope(view, biased, normal, to_light),
            },
        ),
        None => sample_shadow(view, depth_map, biased),
    }
}

/// How much of light `index` reaches a hair at `world` through the hair before it (ADR-793
/// follow-up): hair is no opaque wall, so a hair is not in or out of shadow but lit by what
/// is left after the light's way through the hair in front. That way is how far `world` lies
/// behind the shadow map's first surface, averaged over [`HAIR_SHADOW_TAPS`] taps of a disc
/// [`HAIR_SHADOW_TEXELS`] texels round it (the hair's own strands in the map are a dotted
/// surface: one tap would be noise), and the light left is `e^(−HAIR_ABSORPTION × way)`: all of
/// it on the hair's lit surface, falling away smoothly into the hair, nearly none behind her
/// head. After Yuksel & Keyser's deep opacity maps (2008) in what it gives, by Rünz's (2012)
/// depth difference in how: no second pass, no layers, the one map every light already has.
/// The shader's `hair_shadow` is the same.
pub fn hair_shadow(
    scene: &Scene,
    plan: &ShadowPlan,
    maps: &[Vec<f32>],
    index: usize,
    world: [f32; 3],
    turn: f32,
) -> f32 {
    let (first, count) = plan.lights.get(index).copied().unwrap_or((0, 0));
    let (Some(light), true) = (scene.lights.get(index), count > 0) else {
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
    let Some((texel, depth)) = project_into(view, world) else {
        return 1.0;
    };
    let receiver = view.distance(depth);
    let mut way = 0.0f32;
    for tap in 0..HAIR_SHADOW_TAPS {
        let offset = disc_tap(tap * (SOFT_TAPS / HAIR_SHADOW_TAPS), turn)
            .map(|axis| axis * HAIR_SHADOW_TEXELS);
        let stored = stored_depth(
            view,
            depth_map,
            (texel[0] + offset[0]).floor() as i64,
            (texel[1] + offset[1]).floor() as i64,
        );
        way += (receiver - view.distance(stored)).max(0.0);
    }
    (-HAIR_ABSORPTION * way / HAIR_SHADOW_TAPS as f32).exp()
}

/// The light left of `seen` (a hair's [`hair_shadow`]) channel by channel: what passes through
/// hair takes the hair's colour, so the more of `base` a channel has the less of it is lost
/// ([`HAIR_TINT`] of the way to none). Hair in its own shade is a deeper copper or gold, not
/// grey: the forward scattering of Zinke et al.'s dual scattering (2008), without its tables.
pub fn hair_light(seen: f32, base: [f32; 3]) -> [f32; 3] {
    let most = base[0].max(base[1]).max(base[2]).max(1e-3);
    base.map(|channel| seen.max(0.0).powf(1.0 - HAIR_TINT * channel / most))
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
    /// Its eye's frame, on an `:eye` surface ([`eye_frame`]); zeros on any other.
    eye: EyeFrame,
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
    mix_n(&from.eye.centre, &to.eye.centre, &mut vertex.eye.centre);
    mix_n(&from.eye.axis, &to.eye.axis, &mut vertex.eye.axis);
    mix_n(&from.eye.up, &to.eye.up, &mut vertex.eye.up);
    vertex.eye.index = mix(from.eye.index, to.eye.index);
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
    /// The shapes glossy surfaces show (ADR-795).
    emitters: &'a EmitterPlan,
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
                // (an eye's clear shell is cut out the same way: ADR-796)
                Surface::Cutout | Surface::Eye => {
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
                    // … a strand as wide as a pixel or wider (a wisp: a ribbon with a texture
                    // of hairs) has no such share to add up: its texture's alpha is dithered
                    // as a card's is, in fixed steps, for clean edges with no speckle
                    let share = if material.surface == Surface::Strands {
                        corners[0].tangent[3] * weights[0]
                            + corners[1].tangent[3] * weights[1]
                            + corners[2].tangent[3] * weights[2]
                    } else {
                        1.0
                    };
                    let step = if share < STRAND_WHOLE {
                        coverage *= share;
                        strand_sample_step(
                            blend3(|vertex| vertex.world),
                            (row % 2) * 2 + column % 2,
                        )
                    } else {
                        HAIR_COVERAGE_STEPS[(row / 2 + column / 2) % 2][row % 2][column % 2]
                    };
                    if coverage <= step {
                        return;
                    }
                    alpha = 1.0;
                }
                Surface::Skin | Surface::Standard => alpha = 1.0,
            }
            let mut base = [
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
            let eye = scene.eye;
            let view = normalize([eye[0] - world[0], eye[1] - world[1], eye[2] - world[2]]);
            let mut curvature = corners[0].curvature * weights[0]
                + corners[1].curvature * weights[1]
                + corners[2].curvature * weights[2];
            // an eye (ADR-796): the mesh says only which pixels it covers. Under the cornea the
            // texture is read where the bent sight line meets the iris, the pupil is drawn
            // there, and the highlights are the cornea's; the light, bent on its way in,
            // falls on the iris as a shallow bowl
            let mut diffuse_normal = normal;
            let mut under_cornea = None;
            let mut mirror_surface = surface_normal;
            if let Some(optics) = material.eye_optics() {
                let frame = EyeFrame {
                    centre: blend3(|vertex| vertex.eye.centre),
                    axis: blend3(|vertex| vertex.eye.axis),
                    up: blend3(|vertex| vertex.eye.up),
                    index: corners[0].eye.index * weights[0]
                        + corners[1].eye.index * weights[1]
                        + corners[2].eye.index * weights[2],
                };
                let seen = eye_seen(optics, &frame, world, view, uv, normal, curvature);
                if seen.through {
                    let under = context.base_texture.as_ref().map_or([1.0; 4], |texture| {
                        sample_lod(texture, seen.uv[0], seen.uv[1], base_lod)
                    });
                    let kept = 1.0 - seen.pupil;
                    base = [
                        srgb_to_linear(under[0]) * material.color[0] * kept,
                        srgb_to_linear(under[1]) * material.color[1] * kept,
                        srgb_to_linear(under[2]) * material.color[2] * kept,
                    ];
                    normal = seen.normal;
                    mirror_surface = seen.normal;
                    diffuse_normal = seen.iris_normal;
                    under_cornea = Some(UnderCornea {
                        axis: seen.axis,
                        ratio: 1.0 / optics.ior.max(1.0),
                    });
                    curvature = seen.curvature;
                } else {
                    // the white: reflecting as the ball; the light still falls on the mesh's
                    // normal (ADR-804)
                    normal = seen.normal;
                    mirror_surface = seen.normal;
                }
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
                if context.shadows.lights[index].1 == 0 {
                    continue;
                }
                if matches!(material.surface, Surface::Hair | Surface::Strands) {
                    // hair shades hair by how much of it the light crossed, not in or out
                    *seen = hair_shadow(
                        scene,
                        context.shadows,
                        context.shadow_maps,
                        index,
                        world,
                        turn,
                    );
                } else {
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
                    curvature,
                    specular_kept,
                    mirror_surface,
                    context.emitters,
                    diffuse_normal,
                    under_cornea.as_ref(),
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
/// same. Seen end-on the ribbon narrows, by the sine of the angle between the strand and the
/// line of sight but never under [`STRAND_END_ON`] of its width: a lock pointing at the eye
/// shows its cross-section, and a full-width ribbon there is a plate whose side spins with every
/// bend of the strand (wide strands, wisps of hair, broke into shingles where they ran at the
/// camera).
pub fn strand_vertex(
    scene: &Scene,
    centre: [f32; 3],
    direction: [f32; 3],
    side: f32,
    width: f32,
) -> ([f32; 3], [f32; 3], f32) {
    let view = normalize(subtract(scene.eye, centre));
    let along = dot(direction, view);
    let width = width * (1.0 - along * along).max(0.0).sqrt().max(STRAND_END_ON);
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
    let eye = draw.material.eye_optics();
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
                    eye: EyeFrame::default(),
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
                // an eye's frame goes with its vertices, moved as they are (ADR-796)
                eye: eye.map_or_else(EyeFrame::default, |eye| eye_frame(eye, &matrix, position)),
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

/// Whether `draw`'s skin is pressed by the scene's contacts (ADR-805): a `:deform` material
/// that is skin, not strands or an eye (whose optics need its mesh as it is).
pub fn pressed(draw: &Draw) -> bool {
    draw.material.deform
        && !matches!(
            draw.material.surface,
            Surface::Strands | Surface::Hair | Surface::Eye
        )
}

/// How soft `mesh`'s flesh is at vertex `index` (ADR-805): its `:softness`, or wholly soft when it
/// has none; 0 is bone, or a limb that presses, which is never pressed.
pub fn softness_at(mesh: &Mesh, index: usize) -> f32 {
    if mesh.softness.is_empty() {
        1.0
    } else {
        mesh.softness.get(index).copied().unwrap_or(0.0)
    }
}

/// Which of `skips` (each contact's own limb's joints) vertex `index` of `mesh` belongs to, as
/// bits by contact: its skin weights on that contact's joints pass a half (ADR-805).
pub fn skipped_by(mesh: &Mesh, index: usize, skips: &[[u16; 4]]) -> u16 {
    let (Some(joints), Some(weights)) = (mesh.joints.get(index), mesh.weights.get(index)) else {
        return 0;
    };
    let mut bits = 0u16;
    for (slot, skip) in skips.iter().enumerate().take(MAX_CONTACTS) {
        let share: f32 = joints
            .iter()
            .zip(weights)
            .filter(|(joint, _)| skip.contains(joint))
            .map(|(_, weight)| *weight)
            .sum();
        if share > 0.5 {
            bits |= 1 << slot;
        }
    }
    bits
}

/// The scene's `:deform` draws' posed vertices pressed by its contacts (ADR-805): each
/// contact's deepest press found over them all, then each vertex laid onto what it passed into
/// and lifted in a ring round it ([`deform_by_contacts`]), its tangent kept square to its new
/// normal and its clip position moved with it. Nothing without contacts.
fn press_posed(scene: &Scene, posed: &mut [Posed]) {
    if scene.contacts.is_empty() {
        return;
    }
    let contacts = &scene.contacts[..scene.contacts.len().min(MAX_CONTACTS)];
    // each contact's deepest press, over the soft skin it does press (not its own limb's)
    let mut depths = vec![0.0f32; contacts.len()];
    for item in posed.iter().filter(|item| pressed(item.draw)) {
        for (index, vertex) in item.vertices.iter().enumerate() {
            if softness_at(&item.mesh, index) <= 0.0 {
                continue;
            }
            let skipped = skipped_by(&item.mesh, index, &scene.contact_skips);
            for (slot, (depth, contact)) in depths.iter_mut().zip(contacts).enumerate() {
                if skipped & (1 << slot) == 0 {
                    *depth = depth.max(-contact.signed_distance(vertex.world).0);
                }
            }
        }
    }
    for item in posed.iter_mut().filter(|item| pressed(item.draw)) {
        for (index, vertex) in item.vertices.iter_mut().enumerate() {
            let softness = softness_at(&item.mesh, index);
            let (world, normal) = deform_by_contacts(
                contacts,
                &depths,
                &scene.contact_drags,
                vertex.world,
                vertex.normal,
                softness,
                skipped_by(&item.mesh, index, &scene.contact_skips),
            );
            let tangent = [vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]];
            let square = normalize(
                [0, 1, 2].map(|axis| tangent[axis] - normal[axis] * dot(normal, tangent)),
            );
            vertex.world = world;
            vertex.normal = normal;
            vertex.tangent = [square[0], square[1], square[2], vertex.tangent[3]];
            vertex.clip = transform(&scene.view_proj, world, 1.0);
        }
    }
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
/// the light sees it, both faces, a `:cutout`, `:blend`, `:hair` or `:eye` surface only where its alpha
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
            Surface::Cutout | Surface::Blend | Surface::Hair | Surface::Strands | Surface::Eye
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
    let mut posed: Vec<Posed> = scene
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
    press_posed(scene, &mut posed);
    let world = posed.iter().fold(None, |bounds, item| {
        union(bounds, draw_bounds(scene, item.draw, &item.mesh.bounds))
    });
    let shadows = shadow_plan(scene, world);
    let emitters = emitter_plan(scene);
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
            emitters: &emitters,
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
            emitters: Vec::new(),
            background: [0.0, 0.0, 0.0, 0.0],
            palettes: vec![],
            contacts: Vec::new(),
            contact_drags: Vec::new(),
            contact_skips: Vec::new(),
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
            [0.0, 0.0, 1.0],
            &emitter_plan(&lit),
            [0.0, 0.0, 1.0],
            None,
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
    fn hair_is_shaded_by_how_much_hair_the_light_crossed() {
        // a floor of hair under a card a metre up, the light 45° from straight down. In the
        // card's shadow the light has a long way behind the map's first surface: dark. Away
        // from it the floor IS the map's first surface, next to no way at all: lit nearly as with no
        // shadow, and with no bias to keep it from shading itself. (The grading between, over
        // centimetres of hair, is finer than this picture's 6 cm pixels: `hair_light` and the
        // exponential below stand for it.)
        let hair_floor = |shadow: bool| {
            let light = if shadow {
                shadowed(slanted())
            } else {
                slanted()
            };
            let (mut seen, meshes) = floor_and_card(light);
            seen.draws[0].material.surface = Surface::Hair;
            paint(&seen, &meshes)
        };
        let (unshadowed, shaded) = (hair_floor(false), hair_floor(true));
        let (dark, lit) = (red_at(&shaded, -1.0, 0.0), red_at(&unshadowed, -1.0, 0.0));
        assert!(
            u16::from(dark) + 60 < u16::from(lit),
            "{dark} against {lit}"
        );
        // (a floor aslant to the light lies a little behind its own nearer texels in the taps' disc:
        // it keeps nine tenths and more)
        let (away, open) = (red_at(&shaded, 2.0, 2.0), red_at(&unshadowed, 2.0, 2.0));
        assert!(
            away <= open && u16::from(away) + 16 >= u16::from(open),
            "{away} against {open}"
        );
        // 2.5 cm of hair leaves 1/e of the light
        assert!(((-HAIR_ABSORPTION * 0.025).exp() - (-1.0f32).exp()).abs() < 1e-6);
        // and what passes takes the hair's colour: copper keeps its red longest, and in full
        // light nothing is lost
        let passed = hair_light(0.3, [0.8, 0.4, 0.1]);
        assert!(passed[0] > passed[1] && passed[1] > passed[2], "{passed:?}");
        assert!(passed[0] < 1.0 && passed[2] > 0.3 - 1e-6, "{passed:?}");
        assert_eq!(hair_light(1.0, [0.8, 0.4, 0.1]), [1.0; 3]);
    }

    #[test]
    fn a_strand_seen_end_on_narrows_but_keeps_a_share_of_its_width() {
        // seen from z = −10: a strand across the view keeps its width; one leaning 60° toward
        // the eye keeps half (the sine of the 30° left between it and the line of sight); one
        // 10° off the line of sight keeps STRAND_END_ON of it, not the 0.17 its sine would leave
        let seen = scene(draw());
        let half_width = |direction: [f32; 3]| {
            strand_vertex(&seen, [0.0, 0.0, 0.5], normalize(direction), 1.0, 0.5).0[0].abs()
        };
        assert!((half_width([0.0, 1.0, 0.0]) - 0.25).abs() < 1e-5);
        let leaning = half_width([0.0, 0.5, -0.866_025_4]);
        assert!((leaning - 0.125).abs() < 1e-3, "half its width: {leaning}");
        let end_on = half_width([0.0, 0.173_648_2, -0.984_807_7]);
        assert!(
            (end_on - 0.25 * STRAND_END_ON).abs() < 1e-3,
            "its least share: {end_on}"
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
                normal,
                &emitter_plan(&lit),
                normal,
                None,
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
    fn hair_coverage_takes_eight_steps_over_two_neighbouring_pixels() {
        // (ADR-799) every step k/8 + 1/16 once, half of them a pixel's 2×2 sub-samples and the
        // other half its neighbour's: a coverage between two of the old four steps now covers
        // a share of samples between their two shares, on average over the pair
        let mut steps: Vec<f32> = HAIR_COVERAGE_STEPS
            .iter()
            .flatten()
            .flatten()
            .copied()
            .collect();
        steps.sort_by(f32::total_cmp);
        for (k, step) in steps.iter().enumerate() {
            assert!(
                (step - (k as f32 + 0.5) / 8.0).abs() < 1e-6,
                "step {k}: {step}"
            );
        }
        let covered = |coverage: f32, parity: usize| {
            HAIR_COVERAGE_STEPS[parity]
                .iter()
                .flatten()
                .filter(|step| coverage > **step)
                .count()
        };
        // a coverage of 0.3: 2 of 8 over the pair (the four steps covered 1 of 4 or 2 of 4)
        assert_eq!(covered(0.3, 0) + covered(0.3, 1), 2);
        // a half: half of each pixel, as the GPU's samples (the parity test's case)
        assert_eq!(covered(0.5, 0), 2);
        assert_eq!(covered(0.5, 1), 2);
    }

    #[test]
    fn a_hair_surface_says_its_highlights_shifts_and_its_second_highlights_colour() {
        // (ADR-799) `:shift` moves where the highlights fall, `:sheen` colours the second one;
        // unsaid, both are what they were
        let lit = scene(draw());
        let hair = Material {
            surface: Surface::Hair,
            roughness: 0.3,
            specular: 1.0,
            ..Material::default()
        };
        assert_eq!(hair.hair_shift, [HAIR_SHIFT_PRIMARY, HAIR_SHIFT_SECONDARY]);
        assert_eq!(hair.sheen, None);
        let normal = [0.0, 0.0, 1.0];
        let shine = |material: &Material| {
            shade(
                &lit,
                material,
                [0.2, 0.1, 0.05],
                normal,
                normal,
                [0.0; 3],
                1.0,
                &[],
                [1.0, 0.0, 0.0],
                0.0,
                1.0,
                normal,
                &emitter_plan(&lit),
                normal,
                None,
            )
        };
        let plain = shine(&hair);
        let shifted = shine(&Material {
            hair_shift: [0.4, 0.6],
            ..hair.clone()
        });
        assert!(
            (plain[0] - shifted[0]).abs() > 1e-3,
            "a shift moves the highlight: {plain:?} {shifted:?}"
        );
        let blue = shine(&Material {
            sheen: Some([0.0, 0.0, 1.0]),
            ..hair.clone()
        });
        assert!(
            blue[2] - blue[0] > plain[2] - plain[0],
            "a blue sheen is bluer: {plain:?} {blue:?}"
        );
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
            [0.0, 0.0, 1.0],
            &emitter_plan(&zero),
            [0.0, 0.0, 1.0],
            None,
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
            normal,
            &emitter_plan(&lit),
            normal,
            None,
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

    /// A shadowed light whose map is `resolution` texels a side.
    fn shadowed_at(mut light: Light, resolution: u32) -> Light {
        light.shadow = Some(Shadow {
            resolution,
            ..Shadow::default()
        });
        light
    }

    fn key_spot() -> Light {
        Light::spot([0.0, 3.0, 0.0], [0.0, -1.0, 0.0], [1.0; 3], 8.0, [0.3, 0.6])
    }

    #[test]
    fn a_big_key_map_and_a_point_lights_small_cube_plan_in_two_arrays() {
        // ADR-800: 4096² + 6 × 1024² is within the limit counted as the two arrays hold them
        // (7 × 4096² was not)
        let (mut scene, _) = floor_and_card(slanted());
        scene.lights = vec![
            shadowed_at(key_spot(), 4096),
            shadowed_at(Light::point([1.0, 2.0, 1.0], [1.0; 3], 8.0), 1024),
        ];
        let world = Some([[-4.0, 0.0, -4.0], [4.0, 1.0, 4.0]]);
        let plan = shadow_plan(&scene, world);
        assert_eq!(plan.views.len(), 7);
        assert_eq!((plan.lights[0], plan.lights[1]), ((0, 1), (1, 6)));
        assert_eq!(
            plan.arrays,
            ShadowArrays {
                sides: [4096, 1024],
                layers: [1, 6]
            }
        );
        assert!(plan.arrays.texels() <= MAX_SHADOW_TEXELS);
        assert_eq!(
            plan.views[0].map,
            ShadowMapSlot {
                array: 0,
                layer: 0,
                side: 4096
            }
        );
        for (face, view) in plan.views[1..].iter().enumerate() {
            assert_eq!(
                view.map,
                ShadowMapSlot {
                    array: 1,
                    layer: face as u32,
                    side: 1024
                }
            );
        }
        // every view of one size: all in array 0, array 1 empty
        scene.lights = vec![shadowed_at(key_spot(), 1024)];
        let alone = shadow_plan(&scene, world);
        assert_eq!(
            alone.arrays,
            ShadowArrays {
                sides: [1024, 0],
                layers: [1, 0]
            }
        );
    }

    #[test]
    fn a_light_past_the_summed_limit_is_cut_and_a_smaller_one_after_it_kept() {
        // 4096² + 6 × 2048² ≈ 41.9 M texels: over; 4096² + 6 × 1024² ≈ 23 M: within
        let (mut scene, _) = floor_and_card(slanted());
        scene.lights = vec![
            shadowed_at(key_spot(), 4096),
            shadowed_at(Light::point([1.0, 2.0, 1.0], [1.0; 3], 8.0), 2048),
            shadowed_at(Light::point([-1.0, 2.0, 1.0], [1.0; 3], 8.0), 1024),
        ];
        let plan = shadow_plan(&scene, Some([[-4.0, 0.0, -4.0], [4.0, 1.0, 4.0]]));
        assert_eq!(plan.lights[1], (0, 0), "the 2048 cube is cut");
        assert_eq!(plan.lights[2], (1, 6), "the 1024 cube after it is kept");
        assert_eq!(plan.views.len(), 7);
        assert_eq!(plan.arrays.sides, [4096, 1024]);
        assert!(plan.arrays.texels() <= MAX_SHADOW_TEXELS);
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
    fn a_key_spot_with_a_bigger_map_still_shadows_beside_a_point_cube_and_occlusion() {
        // (ADR-800) a game's light set: a key spot whose map is bigger than the rest, a point
        // lamp's cube of smaller maps, the camera's occlusion view: the key's shadow under the
        // card must stay as dark as the key alone casts it, whatever else shares the arrays
        let key = |resolution: u32| {
            let mut light = shadowed(Light::spot(
                [0.0, 2.0, 0.0],
                [0.0, -1.0, 0.0],
                [8.0; 3],
                0.0,
                [1.2, 1.4],
            ));
            light.shadow.as_mut().unwrap().resolution = resolution;
            // (as the game's key: a softbox's size and a softness: the soft, contact-hardening
            // read, `soft_shadow`)
            light.shadow.as_mut().unwrap().softness = 0.12;
            light.shadow.as_mut().unwrap().normal_bias = 2.0;
            light.size = 1.3;
            light
        };
        let lamp = {
            let mut light = shadowed(Light::point([3.5, 1.5, 3.5], [0.0; 3], 0.0));
            light.shadow.as_mut().unwrap().resolution = 1024;
            light.shadow.as_mut().unwrap().softness = 0.03;
            light
        };
        let (alone, meshes) = floor_and_card(key(2048));
        let reference = red_at(&paint(&alone, &meshes), 0.75, 0.0);
        let mut unlit = alone.clone();
        unlit.lights[0].shadow = None;
        let bright = red_at(&paint(&unlit, &meshes), 0.75, 0.0);
        assert!(
            u16::from(reference) + 60 < u16::from(bright),
            "the key alone: {reference} against {bright}"
        );
        let mut with_lamp = alone.clone();
        with_lamp.lights.push(lamp.clone());
        let beside_lamp = red_at(&paint(&with_lamp, &meshes), 0.75, 0.0);
        assert!(
            (i16::from(beside_lamp) - i16::from(reference)).abs() <= 3,
            "beside a point cube: {beside_lamp} against {reference}"
        );
        let mut with_occlusion = with_lamp.clone();
        with_occlusion.ambient_occlusion = Some(AmbientOcclusion {
            radius: 0.015,
            strength: 0.6,
        });
        let beside_both = red_at(&paint(&with_occlusion, &meshes), 0.75, 0.0);
        assert!(
            u16::from(beside_both) + 60 < u16::from(bright),
            "beside a point cube and occlusion: {beside_both} against {bright} lit"
        );
        // … and with the occlusion view BIGGER than the lamp's maps (the picture's size, as a
        // game's picture is bigger than its lamp's 1024): the lamp's maps then sit in slots of
        // the occlusion view's size
        let mut small_lamp = with_occlusion.clone();
        small_lamp.lights[1].shadow.as_mut().unwrap().resolution = 64;
        let beside_small = red_at(&paint(&small_lamp, &meshes), 0.75, 0.0);
        assert!(
            u16::from(beside_small) + 60 < u16::from(bright),
            "beside a lamp's maps smaller than the occlusion view: {beside_small} against {bright} lit"
        );
        // and the POINT lamp's own shadow, its cube's maps in the smaller array, beside a
        // dark key with the bigger map: as dark as the lamp alone casts it
        let lit_lamp = {
            let mut light = shadowed(Light::point([0.0, 2.0, 0.0], [8.0; 3], 0.0));
            light.shadow.as_mut().unwrap().resolution = 1024;
            light.shadow.as_mut().unwrap().softness = 0.03;
            light
        };
        let (lamp_alone, meshes) = floor_and_card(lit_lamp);
        let lamp_reference = red_at(&paint(&lamp_alone, &meshes), 0.75, 0.0);
        let mut dark_key = key(2048);
        dark_key.color = [0.0; 3];
        dark_key.position = [3.5, 2.0, 3.5];
        let mut lamp_beside_key = lamp_alone.clone();
        lamp_beside_key.lights.push(dark_key);
        lamp_beside_key.ambient_occlusion = Some(AmbientOcclusion {
            radius: 0.015,
            strength: 0.6,
        });
        let lamp_shadow = red_at(&paint(&lamp_beside_key, &meshes), 0.75, 0.0);
        assert!(
            (i16::from(lamp_shadow) - i16::from(lamp_reference)).abs() <= 3,
            "the lamp's shadow beside a bigger key map: {lamp_shadow} against {lamp_reference}"
        );
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
                normal,
                &emitter_plan(&sky),
                normal,
                None,
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
                normal,
                &emitter_plan(&sh),
                normal,
                None,
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
            normal,
            &emitter_plan(&lit),
            normal,
            None,
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
    fn a_cavity_takes_the_specular_and_leaves_the_diffuse() {
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
            ..Material::default()
        };
        let matte = Material {
            specular: 0.0,
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
                normal,
                &emitter_plan(&lit),
                normal,
                None,
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
            eye: EyeFrame::default(),
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

    // --- the surroundings reflected (ADR-795) ---------------------------------------------------

    /// A scene with no light at all.
    fn dark() -> Scene {
        let mut lit = scene(draw());
        lit.lights.clear();
        lit.sky = [0.0; 3];
        lit.ground = [0.0; 3];
        lit
    }

    /// What a black surface at the origin facing +z sends toward `view` in `lit`: its
    /// reflections alone, `open` its ambient occlusion, `visibility` its lights' shadows.
    fn reflecting(
        lit: &Scene,
        material: &Material,
        view: [f32; 3],
        open: f32,
        visibility: &[f32],
    ) -> [f32; 3] {
        let normal = [0.0, 0.0, 1.0];
        shade(
            lit,
            material,
            [0.0; 3],
            normal,
            normalize(view),
            [0.0; 3],
            open,
            visibility,
            [0.0; 3],
            0.0,
            1.0,
            normal,
            &emitter_plan(lit),
            normal,
            None,
        )
    }

    /// The view from which that surface's reflection lands at (x, y) on the plane 2 m above it.
    fn landing(x: f32, y: f32) -> [f32; 3] {
        [-x, -y, 2.0]
    }

    /// A white spot of `size` 2 m above the origin, shining straight down on it.
    fn overhead(size: f32) -> Light {
        Light {
            size,
            ..Light::spot([0.0, 0.0, 2.0], [0.0, 0.0, -1.0], [1.0; 3], 0.0, [0.5, 0.8])
        }
    }

    fn shaped(light: Light, shape: EmitterShape, extent: [f32; 2]) -> Light {
        Light {
            shape: Some(LightShape {
                shape,
                extent,
                up: [0.0, 1.0, 0.0],
            }),
            ..light
        }
    }

    fn near(measured: f32, expected: f32) {
        assert!(
            (measured - expected).abs() <= 1e-3 * expected.abs().max(1e-5),
            "{measured} against {expected}"
        );
    }

    #[test]
    fn a_shaped_light_shrunk_to_a_point_is_the_unshaped_highlight() {
        // the lobe as a box of solid angle 8π / (n + 8), a shape of radiance π·color / area:
        // a shape far smaller than the lobe gives the Blinn-Phong peak, (n + 8) / 8 × F0 × the
        // light arriving (a quarter, 2 m away), whatever the shape and the roughness
        let head_on = [0.0, 0.0, 1.0];
        for roughness in [0.2f32, 0.35, 0.5, 0.8] {
            let material = Material {
                roughness,
                ..Material::default()
            };
            let mut lit = dark();
            lit.lights = vec![overhead(0.0)];
            assert!(!emitter_plan(&lit).shaped[0]);
            let point = reflecting(&lit, &material, head_on, 1.0, &[])[0];
            near(
                point,
                material.f0() * (shininess(roughness) + 8.0) / 8.0 / 4.0,
            );
            for shape in [EmitterShape::Disc, EmitterShape::Sphere, EmitterShape::Rect] {
                lit.lights = vec![shaped(overhead(1e-3), shape, [1e-3, 2e-3])];
                assert!(emitter_plan(&lit).shaped[0]);
                near(reflecting(&lit, &material, head_on, 1.0, &[])[0], point);
            }
            // a directional light's disc, as wide as its angle
            let sun = Light::directional([0.0, 0.0, -1.0], [1.0; 3]);
            lit.lights = vec![sun.clone()];
            let far_point = reflecting(&lit, &material, head_on, 1.0, &[])[0];
            near(far_point, point * 4.0);
            lit.lights = vec![shaped(
                Light { size: 1e-3, ..sun },
                EmitterShape::Disc,
                [0.0; 2],
            )];
            near(reflecting(&lit, &material, head_on, 1.0, &[])[0], far_point);
        }
        // a shape with no size stays a point, its highlight the lobe's
        let mut lit = dark();
        lit.lights = vec![shaped(overhead(0.0), EmitterShape::Disc, [0.0; 2])];
        let plan = emitter_plan(&lit);
        assert!(!plan.shaped[0] && plan.emitters.is_empty());
    }

    #[test]
    fn a_mirror_shows_a_rectangle_at_its_radiance_keeps_its_aspect_and_shows_nothing_beside_it() {
        let mirror = Material {
            roughness: 0.0,
            ..Material::default()
        };
        let color = [0.5f32, 0.4, 0.3];
        let softbox = |up: [f32; 3]| {
            let mut light = shaped(
                Light {
                    color,
                    ..overhead(0.0)
                },
                EmitterShape::Rect,
                [1.0, 0.5],
            );
            light.shape.as_mut().unwrap().up = up;
            light
        };
        // π·color / area: a 1 m × 0.5 m box
        let radiance = color.map(|channel| std::f32::consts::PI * channel / 0.5);
        let mut lit = dark();
        lit.lights = vec![softbox([0.0, 1.0, 0.0])];
        assert_eq!(emitter_plan(&lit).emitters[0].radiance, radiance);
        // its middle, seen head on: exactly F0 × its radiance
        let middle = reflecting(&lit, &mirror, landing(0.0, 0.0), 1.0, &[]);
        for channel in 0..3 {
            near(middle[channel], 0.04 * radiance[channel]);
        }
        // … plus what the surroundings reflect, when there are any
        lit.sky = [0.2; 3];
        lit.ground = [0.2; 3];
        let around = |view: [f32; 3]| {
            let [scale, bias] = environment_brdf(0.0, normalize(view)[2]);
            0.2 * (0.04 * scale + bias)
        };
        let middle = reflecting(&lit, &mirror, landing(0.0, 0.0), 1.0, &[]);
        near(middle[0], 0.04 * radiance[0] + around(landing(0.0, 0.0)));
        // beside it, only the surroundings
        let beside = landing(0.9, 0.0);
        near(
            reflecting(&lit, &mirror, beside, 1.0, &[])[0],
            around(beside),
        );
        // 1 m wide and half a metre high: 0.4 m along its width is on it, 0.4 m up is not
        let (along, up) = (landing(0.4, 0.0), landing(0.0, 0.4));
        let on_it = |view: [f32; 3]| fresnel(0.04, normalize(view)[2]) * radiance[0] + around(view);
        near(reflecting(&lit, &mirror, along, 1.0, &[])[0], on_it(along));
        near(reflecting(&lit, &mirror, up, 1.0, &[])[0], around(up));
        // turned a quarter by its `:up`, the other way about
        lit.lights = vec![softbox([1.0, 0.0, 0.0])];
        near(reflecting(&lit, &mirror, along, 1.0, &[])[0], around(along));
        near(reflecting(&lit, &mirror, up, 1.0, &[])[0], on_it(up));
        // from behind it nothing: it shines one way
        lit.lights = vec![softbox([0.0, 1.0, 0.0])];
        lit.lights[0].position = [0.0, 0.0, -2.0];
        lit.lights[0].direction = [0.0, 0.0, 1.0];
        let behind = reflecting(&lit, &mirror, landing(0.0, 0.0), 1.0, &[])[0];
        near(behind, around(landing(0.0, 0.0)));
    }

    #[test]
    fn a_uniform_surrounding_reflects_by_the_environment_fit_and_never_brighter_than_itself() {
        // L × (F0 × scale + F90 × bias), F90 min(50 F0, 1): the same from a sky and ground and
        // from the same radiance as SH, at any roughness, angle and reflectance
        let level = 0.3f32;
        let mut by_sky = dark();
        by_sky.sky = [level; 3];
        by_sky.ground = [level; 3];
        let mut by_sh = dark();
        let mut coefficients = [[0.0f32; 3]; 9];
        coefficients[0] = [level * 2.0 * std::f32::consts::PI.sqrt(); 3];
        by_sh.ambient_sh = Some(coefficients);
        for roughness in [0.0f32, 0.1, 0.25, 0.5, 0.75, 1.0] {
            for specular in [0.0f32, 0.1, 0.25, 0.5, 2.0, 12.5] {
                for view in [
                    landing(0.0, 0.0),
                    landing(1.0, 0.5),
                    landing(8.0, 0.0),
                    landing(60.0, 0.0),
                ] {
                    let material = Material {
                        roughness,
                        specular,
                        ..Material::default()
                    };
                    let f0 = material.f0();
                    let [scale, bias] = environment_brdf(roughness, normalize(view)[2]);
                    assert!(scale >= 0.0 && bias >= 0.0 && scale + bias <= 1.0 + 1e-6);
                    let expected = level * (f0 * scale + (50.0 * f0).min(1.0) * bias);
                    let reflected = reflecting(&by_sky, &material, view, 1.0, &[])[0];
                    assert!(
                        (reflected - expected).abs() < 1e-6,
                        "{roughness} {specular} {view:?}: {reflected} against {expected}"
                    );
                    assert!(reflected <= level, "{reflected} is brighter than {level}");
                    let from_sh = reflecting(&by_sh, &material, view, 1.0, &[])[0];
                    assert!(
                        (from_sh - reflected).abs() < 1e-4,
                        "{from_sh} against {reflected}"
                    );
                    if specular == 0.0 {
                        assert_eq!(reflected, 0.0, ":specular 0 reflects nothing");
                    }
                }
            }
        }
        // the surroundings blurred as the lobe is rough: light from above only is seen sharply
        // by a mirror facing it or away from it, and as its mean by chalk
        let mut above = [[0.0f32; 3]; 9];
        above[0] = [1.0; 3];
        above[1] = [1.0; 3];
        let (up, down) = ([0.0, 1.0, 0.0], [0.0, -1.0, 0.0]);
        let sharp = lobe_exponent(0.0);
        assert!(sh_radiance(&above, up, sharp)[0] > 0.75);
        assert_eq!(sh_radiance(&above, down, sharp)[0], 0.0);
        let chalk = lobe_exponent(1.0);
        let (lit_side, far_side) = (
            sh_radiance(&above, up, chalk)[0],
            sh_radiance(&above, down, chalk)[0],
        );
        assert!(
            lit_side - far_side < 0.15 && far_side > 0.2,
            "{lit_side} {far_side}"
        );
        // and a sky over a ground likewise
        let mut split = dark();
        split.sky = [1.0; 3];
        assert!(ambient_radiance(&split, down, sharp)[0] < 1e-5);
        assert!((ambient_radiance(&split, down, chalk)[0] - 0.5).abs() < 0.07);
    }

    #[test]
    fn occlusion_a_shadow_and_no_specular_take_the_reflections() {
        let mirror = Material {
            roughness: 0.0,
            ..Material::default()
        };
        let mut lit = dark();
        lit.sky = [0.2; 3];
        lit.ground = [0.2; 3];
        let mut unlit = lit.clone();
        lit.lights = vec![shaped(overhead(0.0), EmitterShape::Rect, [1.0, 0.5])];
        let head_on = landing(0.0, 0.0);
        let open = reflecting(&lit, &mirror, head_on, 1.0, &[])[0];
        let surroundings = reflecting(&unlit, &mirror, head_on, 1.0, &[])[0];
        assert!(surroundings > 0.0 && open > 10.0 * surroundings);
        // a point no ambient light reaches reflects nothing, the shape neither
        assert_eq!(reflecting(&lit, &mirror, head_on, 0.0, &[]), [0.0; 3]);
        // half occluded, a mirror seen head on keeps more of its reflection than half, and
        // chalk half of it (Lagarde's)
        assert!(specular_occlusion(1.0, 0.5, 0.0) > 0.7);
        assert!((specular_occlusion(1.0, 0.5, 1.0) - 0.5).abs() < 1e-3);
        assert_eq!(specular_occlusion(0.3, 1.0, 0.5), 1.0);
        assert_eq!(specular_occlusion(0.3, 0.0, 0.5), 0.0);
        // in the light's shadow the shape is not reflected: the surroundings alone are left
        assert_eq!(
            reflecting(&lit, &mirror, head_on, 1.0, &[0.0])[0],
            surroundings
        );
        // … nor outside its cone (the surface moved out from under it would be; here the cone
        // is turned away)
        lit.lights[0].direction = [1.0, 0.0, 0.0];
        lit.lights[0].shape.as_mut().unwrap().shape = EmitterShape::Sphere;
        lit.lights[0].size = 0.5;
        assert_eq!(
            reflecting(&lit, &mirror, head_on, 1.0, &[])[0],
            surroundings
        );
        // :specular 0 reflects nothing of anything
        let matte = Material {
            specular: 0.0,
            ..mirror.clone()
        };
        lit.lights = vec![shaped(overhead(0.0), EmitterShape::Rect, [1.0, 0.5])];
        assert_eq!(reflecting(&lit, &matte, head_on, 1.0, &[]), [0.0; 3]);
        // one of the scene's `:emitters` is reflected and lights nothing
        unlit.emitters = vec![Emitter {
            shape: EmitterShape::Rect,
            position: [0.0, 0.0, 2.0],
            direction: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            extent: [1.0, 0.5],
            radiance: [3.0, 2.0, 1.0],
        }];
        let shown = reflecting(&unlit, &mirror, head_on, 1.0, &[]);
        near(shown[0] - surroundings, 0.04 * 3.0);
        near(shown[2] - surroundings, 0.04 * 1.0);
        let white = Material {
            specular: 0.0,
            ..Material::default()
        };
        let plan = emitter_plan(&unlit);
        let normal = [0.0, 0.0, 1.0];
        let diffuse = |seen: &Scene, plan: &EmitterPlan| {
            shade(
                seen,
                &white,
                [1.0; 3],
                normal,
                normal,
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                0.0,
                1.0,
                normal,
                plan,
                normal,
                None,
            )
        };
        let mut bare = unlit.clone();
        bare.emitters.clear();
        assert_eq!(diffuse(&unlit, &plan), diffuse(&bare, &emitter_plan(&bare)));
        // a surface tilted by its normal map until the reflection runs under the surface
        // itself reflects less by the horizon term, a reflection leaving the surface all
        let tilted = normalize([0.6, 0.0, 0.8]);
        let view = normalize([-0.6, 0.0, 0.8]);
        let facing_view = dot(tilted, view);
        let under = [0, 1, 2].map(|axis| 2.0 * facing_view * tilted[axis] - view[axis]);
        assert!(under[2] < 0.0);
        let kept = horizon(under, normal);
        assert!(kept > 0.2 && kept < 0.6, "{kept}");
        assert_eq!(horizon(under, tilted), 1.0);
        let mapped = |surface: [f32; 3]| {
            shade(
                &bare,
                &mirror,
                [0.0; 3],
                tilted,
                view,
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                0.0,
                1.0,
                surface,
                &emitter_plan(&bare),
                tilted,
                None,
            )[0]
        };
        near(mapped(normal), kept * mapped(tilted));
    }

    #[test]
    fn a_roughness_under_the_old_ceiling_means_something_and_nothing_steps() {
        // an eye at 0.12 and a tear line at 0.05 were one highlight (the exponent stopped at
        // 4096, a roughness of about 0.149); now each is its own
        let glossy = |roughness: f32| Material {
            roughness,
            ..Material::default()
        };
        let (eye, tear) = (
            specular_head_on(&glossy(0.12)),
            specular_head_on(&glossy(0.05)),
        );
        near(eye, 0.04 * (lobe_exponent(0.12) + 8.0) / 8.0);
        near(tear, 0.04 * (lobe_exponent(0.05) + 8.0) / 8.0);
        assert!(tear > 20.0 * eye, "{tear} against {eye}");
        assert!(lobe_exponent(0.12) > 2.0 * RESOLVED_EXPONENT);
        assert_eq!(lobe_exponent(0.0), lobe_exponent(MIN_ROUGHNESS));
        // from the old ceiling up nothing changed, on a flat surface or a curved one
        for step in 150..=1000 {
            let roughness = step as f32 / 1000.0;
            for turn in [0.0f32, 0.001, 0.05, 0.4] {
                assert_eq!(highlight_exponent(roughness, turn), shininess(roughness));
            }
        }
        // on a curved surface a sharper lobe is only as sharp as a pixel resolves, never
        // under the old ceiling: a reflection turning 0.05 rad across half a pixel
        let resolved = 2.0 * std::f32::consts::PI / (0.05 * 0.05) - 8.0;
        assert_eq!(
            highlight_exponent(0.0, 0.05),
            RESOLVED_EXPONENT.max(resolved)
        );
        assert_eq!(highlight_exponent(0.0, 0.01), resolved_at(0.01));
        assert_eq!(highlight_exponent(0.12, 1e-4), lobe_exponent(0.12));
        // and a sweep of the roughness has no step in it: the exponent falls by under an
        // eighth from one thousandth of roughness to the next, flat or curved, as do the
        // highlight's peak and the width a shape's image is blurred to
        for turn in [0.0f32, 0.004, 0.01, 0.05] {
            let mut last = highlight_exponent(0.0, turn);
            let mut last_spread = lobe_spread(lobe_exponent(0.0), turn);
            for step in 1..=1000 {
                let roughness = step as f32 / 1000.0;
                let exponent = highlight_exponent(roughness, turn);
                assert!(
                    exponent <= last && exponent > last * 0.875,
                    "{roughness} at {turn}: {exponent} after {last}"
                );
                let spread = lobe_spread(lobe_exponent(roughness), turn);
                assert!(
                    spread >= last_spread && spread >= turn && spread < last_spread * 1.07,
                    "{roughness} at {turn}: {spread} after {last_spread}"
                );
                (last, last_spread) = (exponent, spread);
            }
        }
    }

    /// The exponent whose lobe is as wide as a reflection turning `turn` across half a pixel.
    fn resolved_at(turn: f32) -> f32 {
        2.0 * std::f32::consts::PI / (turn * turn) - 8.0
    }

    #[test]
    fn a_reflection_crosses_a_shapes_edge_and_a_shape_shrinks_to_a_point_without_a_step() {
        // the reflection swept across a rectangle's and a disc's edge a millimetre at a time,
        // satin and mirror, the mirror on a flat surface and on one curved enough that the
        // reflection turns a hundredth of a radian across half a pixel: the image fades in over
        // the lobe's width, never by more than the millimetre's share of that width
        let plan_of = |shape: EmitterShape| {
            let mut lit = dark();
            lit.lights = vec![shaped(overhead(2.0), shape, [2.0, 2.0])];
            emitter_plan(&lit).emitters[0].clone()
        };
        for shape in [EmitterShape::Rect, EmitterShape::Disc, EmitterShape::Sphere] {
            let emitter = plan_of(shape);
            for (roughness, turn) in [(0.3f32, 0.0f32), (0.0, 0.0), (0.0, 0.01), (0.12, 0.05)] {
                let spread = lobe_spread(lobe_exponent(roughness), turn);
                let covered =
                    |x: f32| emitter_coverage(&emitter, [0.0; 3], normalize([x, 0.0, 2.0]), spread);
                assert_eq!(covered(0.0), 1.0, "{shape:?} {roughness}: its middle");
                assert_eq!(covered(2.2), 0.0, "{shape:?} {roughness}: well beside it");
                // the lobe's half-width where it lands, 2 m away: no narrower than this
                let most = 0.001 / (2.0 * 2.0 * spread) * 1.5;
                let mut last = 1.0f32;
                for step in 1..=2200 {
                    let here = covered(step as f32 * 0.001);
                    assert!(
                        (0.0..=1.0).contains(&here) && here <= last + 1e-4 && last - here <= most,
                        "{shape:?} {roughness} {turn} at {step} mm: {here} after {last}"
                    );
                    last = here;
                }
            }
        }
        // a shape shrinking by a twentieth at a time, from a metre to a tenth of a millimetre:
        // its image brightens as it shrinks inside the lobe, then holds at the point light's
        // highlight; never a jump
        for shape in [EmitterShape::Disc, EmitterShape::Sphere, EmitterShape::Rect] {
            let material = Material {
                roughness: 0.3,
                ..Material::default()
            };
            let mut lit = dark();
            lit.lights = vec![overhead(0.0)];
            let point = reflecting(&lit, &material, landing(0.0, 0.0), 1.0, &[])[0];
            let mut size = 1.0f32;
            let mut last = None;
            while size > 1e-4 {
                lit.lights = vec![shaped(overhead(size), shape, [size, size])];
                let here = reflecting(&lit, &material, landing(0.0, 0.0), 1.0, &[])[0];
                if let Some(before) = last {
                    assert!(
                        here >= before * 0.999 && here <= before * 1.12,
                        "{shape:?} at {size}: {here} after {before}"
                    );
                }
                assert!(
                    here <= point * 1.001,
                    "{shape:?} at {size}: {here} over {point}"
                );
                last = Some(here);
                size *= 0.95;
            }
            near(last.unwrap(), point);
        }
    }

    #[test]
    fn a_coat_adds_its_layer_and_nothing_without_it() {
        let dry = Material {
            roughness: 0.6,
            ..Material::default()
        };
        let wet = Material {
            coat: Some([1.0, 0.1]),
            ..dry.clone()
        };
        let none = Material {
            coat: Some([0.0, 0.1]),
            ..dry.clone()
        };
        let head_on = landing(0.0, 0.0);
        // under a point light: a second highlight at the coat's roughness and water's F0
        let mut lit = dark();
        lit.lights = vec![overhead(0.0)];
        let under = reflecting(&lit, &dry, head_on, 1.0, &[])[0];
        let coated = reflecting(&lit, &wet, head_on, 1.0, &[])[0];
        near(
            coated - under,
            COAT_F0 * (lobe_exponent(0.1) + 8.0) / 8.0 / 4.0,
        );
        assert_eq!(reflecting(&lit, &none, head_on, 1.0, &[])[0], under);
        // half a coat, half of it
        let half = Material {
            coat: Some([0.5, 0.1]),
            ..dry.clone()
        };
        near(
            reflecting(&lit, &half, head_on, 1.0, &[])[0] - under,
            (coated - under) * 0.5,
        );
        // in the surroundings: their reflection by the coat's own fit, over the surface's
        let mut around = dark();
        around.sky = [0.3; 3];
        around.ground = [0.3; 3];
        let [scale, bias] = environment_brdf(0.1, 1.0);
        let under = reflecting(&around, &dry, head_on, 1.0, &[])[0];
        near(
            reflecting(&around, &wet, head_on, 1.0, &[])[0] - under,
            0.3 * (COAT_F0 * scale + bias),
        );
        assert_eq!(reflecting(&around, &none, head_on, 1.0, &[])[0], under);
        // and a shape's image in it, sharp over the satin surface's blurred one
        around.lights = vec![shaped(overhead(0.0), EmitterShape::Rect, [1.0, 0.5])];
        let radiance = std::f32::consts::PI / 0.5;
        let under = reflecting(&around, &dry, head_on, 1.0, &[])[0];
        near(
            reflecting(&around, &wet, head_on, 1.0, &[])[0] - under,
            COAT_F0 * radiance + 0.3 * (COAT_F0 * scale + bias),
        );
    }

    #[test]
    fn hair_reflects_no_surroundings_and_has_no_coat() {
        // hair keeps its own highlights: no reflection of the ambient or of a shape, no coat
        for surface in [Surface::Hair, Surface::Strands] {
            let hair = Material {
                surface,
                roughness: 0.1,
                specular: 1.0,
                ..Material::default()
            };
            let coated = Material {
                coat: Some([1.0, 0.05]),
                ..hair.clone()
            };
            let mut lit = dark();
            lit.sky = [0.5; 3];
            lit.ground = [0.5; 3];
            assert_eq!(
                reflecting(&lit, &hair, landing(1.0, 0.0), 1.0, &[]),
                [0.0; 3]
            );
            lit.emitters = vec![Emitter {
                shape: EmitterShape::Sphere,
                position: [0.0, 0.0, 2.0],
                direction: [0.0, -1.0, 0.0],
                up: [0.0, 1.0, 0.0],
                extent: [1.0, 1.0],
                radiance: [5.0; 3],
            }];
            assert_eq!(
                reflecting(&lit, &hair, landing(0.0, 0.0), 1.0, &[]),
                [0.0; 3]
            );
            lit.lights = vec![overhead(0.0)];
            assert_eq!(
                reflecting(&lit, &coated, landing(0.2, 0.0), 1.0, &[]),
                reflecting(&lit, &hair, landing(0.2, 0.0), 1.0, &[])
            );
        }
    }

    #[test]
    fn the_emitters_are_planned_once_shaped_lights_first_eight_at_most() {
        let mut lit = dark();
        let window = Emitter {
            shape: EmitterShape::Rect,
            position: [1.0, 2.0, 3.0],
            direction: [0.0, 0.0, 2.0],
            up: [0.0, 1.0, 0.0],
            extent: [1.5, 1.3],
            radiance: [4.0, 5.0, 6.0],
        };
        lit.emitters = vec![window.clone(); MAX_EMITTERS];
        lit.lights = vec![
            overhead(0.0),
            shaped(
                Light {
                    color: [2.0, 1.0, 0.5],
                    ..overhead(0.5)
                },
                EmitterShape::Disc,
                [0.0; 2],
            ),
            shaped(
                Light {
                    size: 0.02,
                    ..Light::directional([0.0, -1.0, 0.0], [1.0; 3])
                },
                EmitterShape::Disc,
                [0.0; 2],
            ),
        ];
        let plan = emitter_plan(&lit);
        assert_eq!(plan.shaped[..4], [false, true, true, false]);
        assert_eq!(plan.emitters.len(), MAX_EMITTERS);
        let disc = &plan.emitters[0];
        assert_eq!((disc.shape, disc.light), (PlannedShape::Disc, Some(1)));
        assert_eq!(disc.half[0], 0.25);
        // π·color over its area, π r²: color / r²
        for channel in 0..3 {
            near(disc.radiance[channel], [2.0, 1.0, 0.5][channel] / 0.0625);
        }
        let sun = &plan.emitters[1];
        assert_eq!((sun.shape, sun.light), (PlannedShape::Distant, Some(2)));
        near(sun.half[0], 0.01);
        near(sun.radiance[0], 1.0 / (0.01f32.tan() * 0.01f32.tan()));
        // the scene's own after them: facing made unit, its width to the right of its facing
        // with its height up
        let planned = &plan.emitters[2];
        assert_eq!((planned.shape, planned.light), (PlannedShape::Rect, None));
        assert_eq!(planned.facing, [0.0, 0.0, 1.0]);
        assert_eq!(
            (planned.right, planned.up),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
        );
        assert_eq!(planned.half, [0.75, 0.65]);
        assert_eq!(planned.radiance, window.radiance);
        assert_eq!(
            [1, 2, 3, 4],
            [
                PlannedShape::Disc.code(),
                PlannedShape::Rect.code(),
                PlannedShape::Sphere.code(),
                PlannedShape::Distant.code()
            ]
        );
        // an `:up` along its facing still gives it a frame
        let (right, up) = emitter_frame([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        assert!((dot(right, right) - 1.0).abs() < 1e-6 && (dot(up, up) - 1.0).abs() < 1e-6);
        assert!(dot(right, up).abs() < 1e-6 && dot(right, [0.0, 1.0, 0.0]).abs() < 1e-6);
    }

    #[test]
    fn a_pixel_spans_what_the_camera_says_and_a_curved_mirror_blurs_by_it() {
        // `scene`'s camera is the identity over 16 pixels: clip −1 … 1 is 2 m across, so a
        // pixel is an eighth of a metre
        let seen = scene(draw());
        near(pixel_span(&seen, [0.0; 3], [0.0, 0.0, -1.0]), 0.125);
        // a mirror ball of 1 m radius there: its reflection turns a quarter of a radian across
        // half a pixel, so a shape's edge in it is blurred over a pixel and its point
        // highlight is no sharper than a pixel shows, where a flat mirror keeps both sharp
        let turn = 2.0 * 1.0 * 0.125;
        assert_eq!(lobe_spread(lobe_exponent(0.0), turn), turn);
        assert!(lobe_spread(lobe_exponent(0.0), 0.0) < 0.003);
        let mirror = Material {
            roughness: 0.0,
            ..Material::default()
        };
        let mut lit = scene(draw());
        lit.sky = [0.0; 3];
        lit.ground = [0.0; 3];
        lit.lights = vec![Light::directional([0.0, 0.0, 1.0], [1.0; 3])];
        let normal = [0.0, 0.0, -1.0];
        let highlight = |curvature: f32| {
            shade(
                &lit,
                &mirror,
                [0.0; 3],
                normal,
                normal,
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                curvature,
                1.0,
                normal,
                &emitter_plan(&lit),
                normal,
                None,
            )[0]
        };
        near(highlight(0.0), 0.04 * (lobe_exponent(0.0) + 8.0) / 8.0);
        near(highlight(1.0), 0.04 * (RESOLVED_EXPONENT + 8.0) / 8.0);
        near(highlight(0.04), 0.04 * (resolved_at(0.01) + 8.0) / 8.0);
    }

    // --- an eye (ADR-796) ---------------------------------------------------------------------

    /// A grown person's eye at the origin looking along +z, its iris a disc a quarter of the
    /// texture wide about the texture's middle.
    fn an_eye() -> Eye {
        Eye {
            eyes: vec![EyeAt {
                centre: [0.0; 3],
                axis: [0.0, 0.0, 1.0],
                iris_uv: [0.5, 0.5],
            }],
            up: [0.0, 1.0, 0.0],
            radius: DEFAULT_EYE_RADIUS,
            iris_radius: DEFAULT_IRIS_RADIUS,
            cornea_radius: DEFAULT_CORNEA_RADIUS,
            iris_depth: DEFAULT_IRIS_DEPTH,
            ior: DEFAULT_EYE_IOR,
            iris_uv_radius: 0.25,
            pupil: DEFAULT_PUPIL,
            concavity: DEFAULT_IRIS_CONCAVITY,
        }
    }

    /// That eye's frame, unmoved.
    fn at_rest() -> EyeFrame {
        EyeFrame {
            centre: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            index: 0.0,
        }
    }

    /// How far along its axis `eye`'s cornea's centre and its iris's plane are from its centre.
    fn heights(eye: &Eye) -> (f32, f32) {
        let limbus = (eye.radius.powi(2) - eye.iris_radius.powi(2)).sqrt();
        let cornea = limbus - (eye.cornea_radius.powi(2) - eye.iris_radius.powi(2)).sqrt();
        (cornea, cornea + eye.cornea_radius - eye.iris_depth)
    }

    /// What is seen at `world` from the direction `view`, the mesh there flat on to +y with
    /// the texture's corner.
    fn seen_at(eye: &Eye, frame: &EyeFrame, world: [f32; 3], view: [f32; 3]) -> EyeSeen {
        eye_seen(
            eye,
            frame,
            world,
            normalize(view),
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            7.0,
        )
    }

    #[test]
    fn a_sight_line_down_an_eyes_axis_is_not_bent_and_meets_the_middle_of_the_pupil() {
        let eye = an_eye();
        let (_, iris) = heights(&eye);
        let seen = seen_at(&eye, &at_rest(), [0.0, 0.0, iris], [0.0, 0.0, 1.0]);
        assert!(seen.through);
        assert_eq!(seen.uv, [0.5, 0.5]);
        assert_eq!(seen.pupil, 1.0);
        // the highlights are the cornea's there: its normal, its curve, none of the mesh's
        assert_eq!(seen.cornea, 1.0);
        near(seen.normal[2], 1.0);
        near(seen.curvature, 1.0 / DEFAULT_CORNEA_RADIUS);
        // and wherever along that line the mesh happens to be: it only says which pixel
        let deeper = seen_at(&eye, &at_rest(), [0.0, 0.0, iris - 0.003], [0.0, 0.0, 1.0]);
        assert_eq!(deeper.uv, [0.5, 0.5]);
    }

    #[test]
    fn the_cornea_bends_a_sight_line_as_snell_says_and_shows_the_iris_larger() {
        // a line parallel to the axis, 3 mm off it, meets the cornea where its normal leans
        // asin(x / R) from the axis, is bent to asin(sin θ / n) from that normal, and so
        // crosses to the iris's plane leaning θ − θ′ toward the axis
        let eye = an_eye();
        let (cornea, iris) = heights(&eye);
        let off_axis = 0.003f32;
        let leaning = (off_axis / eye.cornea_radius).asin();
        let bent = (leaning.sin() / eye.ior).asin();
        let entry = cornea + eye.cornea_radius * leaning.cos();
        let met = off_axis - (entry - iris) * (leaning - bent).tan();
        let seen = seen_at(&eye, &at_rest(), [off_axis, 0.0, iris], [0.0, 0.0, 1.0]);
        assert!(seen.through);
        near(seen.uv[0], 0.5 + met / eye.iris_radius * eye.iris_uv_radius);
        near(seen.uv[1], 0.5);
        // nearer the iris's middle than where the straight line would have met it: larger
        assert!(met < off_axis * 0.97, "{met}");
        // up in the world is up in the texture, whose v runs down
        let above = seen_at(&eye, &at_rest(), [0.0, off_axis, iris], [0.0, 0.0, 1.0]);
        near(
            above.uv[1],
            0.5 - met / eye.iris_radius * eye.iris_uv_radius,
        );
        near(above.uv[0], 0.5);
    }

    #[test]
    fn an_index_of_one_bends_nothing_and_from_the_side_the_iris_is_seen_nearer_the_viewer() {
        let eye = an_eye();
        let (_, iris) = heights(&eye);
        let aside = [0.6, 0.0, 0.8];
        // unbent, the line through a point of the iris's plane meets the iris there
        let plain = Eye {
            ior: 1.0,
            ..an_eye()
        };
        let seen = seen_at(&plain, &at_rest(), [0.002, 0.001, iris], aside);
        assert!(seen.through);
        near(seen.uv[0], 0.5 + 0.002 / eye.iris_radius * 0.25);
        near(seen.uv[1], 0.5 - 0.001 / eye.iris_radius * 0.25);
        // bent, the line through the iris's middle goes down more steeply and meets the iris
        // on the viewer's side of the middle: the middle itself is seen further from the
        // viewer than it lies, the iris floating up toward the cornea, not painted on a ball
        let middle = seen_at(&eye, &at_rest(), [0.0, 0.0, iris], aside);
        assert!(middle.through);
        assert!(middle.uv[0] > 0.5 + 0.01, "{:?}", middle.uv);
        near(middle.uv[1], 0.5);
    }

    #[test]
    fn the_white_of_an_eye_reflects_as_the_ball_and_is_lit_as_its_mesh() {
        // a point of the eyeball 60° round from the axis, seen along its own normal: the line
        // never meets the cornea. The mesh's normal there (`seen_at`'s, straight up) is not the
        // ball's: the white keeps the mesh's for the light and reflects about the ball's own
        // (ADR-804), so a mesh modelled any way round the limbus shows no mirror band
        let eye = an_eye();
        let on_the_white = [eye.radius * 0.866, 0.0, eye.radius * 0.5];
        let seen = seen_at(&eye, &at_rest(), on_the_white, on_the_white);
        assert!(!seen.through);
        assert_eq!(seen.uv, [0.0, 0.0]);
        assert_eq!(seen.cornea, 0.0);
        assert_eq!(seen.curvature, 7.0);
        assert_eq!(seen.iris_normal, [0.0, 1.0, 0.0]);
        let ball = normalize(on_the_white);
        for index in 0..3 {
            near(seen.normal[index], ball[index]);
        }
        // and a frame with no length (a vertex of another surface) is no eye at all
        let nowhere = EyeFrame::default();
        assert!(!seen_at(&eye, &nowhere, [0.0; 3], [0.0, 0.0, 1.0]).through);
    }

    #[test]
    fn the_pupil_is_a_soft_disc_its_share_of_the_iris_wide_and_none_at_zero() {
        let (_, iris) = heights(&an_eye());
        let open = Eye {
            ior: 1.0,
            pupil: 0.5,
            ..an_eye()
        };
        let pupil_at = |eye: &Eye, share: f32| {
            let world = [share * DEFAULT_IRIS_RADIUS, 0.0, iris];
            seen_at(eye, &at_rest(), world, [0.0, 0.0, 1.0]).pupil
        };
        assert_eq!(pupil_at(&open, 0.0), 1.0);
        assert_eq!(pupil_at(&open, 0.5 * (1.0 - PUPIL_EDGE) - 0.01), 1.0);
        near(pupil_at(&open, 0.5), 0.5);
        assert_eq!(pupil_at(&open, 0.5 * (1.0 + PUPIL_EDGE) + 0.01), 0.0);
        let none = Eye {
            pupil: 0.0,
            ..open.clone()
        };
        assert_eq!(pupil_at(&none, 0.0), 0.0);
    }

    #[test]
    fn the_corneas_normal_gives_way_to_the_balls_at_the_limbus() {
        let eye = Eye {
            ior: 1.0,
            ..an_eye()
        };
        let (_, iris) = heights(&eye);
        let cornea_at = |share: f32| {
            let world = [share * DEFAULT_IRIS_RADIUS, 0.0, iris];
            seen_at(&eye, &at_rest(), world, [0.0, 0.0, 1.0])
        };
        assert_eq!(cornea_at(CORNEA_EDGE - 0.02).cornea, 1.0);
        let edge = cornea_at(0.999);
        assert!(edge.through);
        assert!(edge.cornea < 1e-3, "{}", edge.cornea);
        // … where the normal is the eyeball's own (ADR-804; not the mesh's, straight up in
        // `seen_at`), and the curve the mesh's
        let ball = normalize([0.999 * DEFAULT_IRIS_RADIUS, 0.0, iris]);
        for index in 0..3 {
            assert!(
                (edge.normal[index] - ball[index]).abs() < 1e-3,
                "{:?}",
                edge.normal
            );
        }
        assert!((edge.curvature - 7.0).abs() < 0.2, "{}", edge.curvature);
        // past it the line misses the cap
        assert!(!cornea_at(1.01).through);
        let between = cornea_at(0.93).cornea;
        assert!(between > 0.1 && between < 0.9, "{between}");
    }

    #[test]
    fn an_eyes_frame_goes_with_its_vertices_and_a_scaled_model_scales_the_eye() {
        // two eyes; a matrix that doubles the model and moves it
        let pair = Eye {
            eyes: vec![
                EyeAt {
                    centre: [-0.03, 0.0, 0.0],
                    axis: [0.0, 0.0, 1.0],
                    iris_uv: [0.3, 0.7],
                },
                EyeAt {
                    centre: [0.03, 0.0, 0.0],
                    axis: [0.0, 0.0, 1.0],
                    iris_uv: [0.7, 0.3],
                },
            ],
            ior: 1.0,
            ..an_eye()
        };
        let mut matrix = identity();
        (matrix[0], matrix[5], matrix[10]) = (2.0, 2.0, 2.0);
        (matrix[12], matrix[13], matrix[14]) = (1.0, 2.0, 3.0);
        // a vertex belongs to the eye whose centre it is nearest at rest
        assert_eq!(eye_frame(&pair, &matrix, [-0.02, 0.01, 0.0]).index, 0.0);
        let frame = eye_frame(&pair, &matrix, [0.025, 0.01, 0.0]);
        assert_eq!(
            frame,
            EyeFrame {
                centre: [1.06, 2.0, 3.0],
                axis: [0.0, 0.0, 2.0],
                up: [0.0, 2.0, 0.0],
                index: 1.0,
            }
        );
        // the same point of the iris, twice as far from the moved centre, reads the same
        // texel: the second eye's
        let (_, iris) = heights(&pair);
        let local = [0.002, 0.001, iris];
        let world = [0, 1, 2].map(|axis| frame.centre[axis] + 2.0 * local[axis]);
        let seen = seen_at(&pair, &frame, world, [0.6, 0.0, 0.8]);
        assert!(seen.through);
        near(seen.uv[0], 0.7 + 0.002 / DEFAULT_IRIS_RADIUS * 0.25);
        near(seen.uv[1], 0.3 - 0.001 / DEFAULT_IRIS_RADIUS * 0.25);
        near(seen.curvature, 0.5 / DEFAULT_CORNEA_RADIUS);
        // one eye is every vertex's
        assert_eq!(an_eye().nearest([9.0, 9.0, 9.0]), 0);
    }

    #[test]
    fn the_highlights_follow_the_reflecting_normal_and_the_light_falls_on_the_other() {
        let mut lit = scene(draw());
        lit.lights = vec![Light::directional([0.0, 0.0, 1.0], [1.0; 3])];
        let toward = [0.0, 0.0, -1.0];
        let leaning = normalize([0.3, 0.0, -1.0]);
        let shaded = |material: &Material, base: f32, normal: [f32; 3], diffuse: [f32; 3]| {
            shade(
                &lit,
                material,
                [base; 3],
                normal,
                toward,
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                0.0,
                1.0,
                normal,
                &emitter_plan(&lit),
                diffuse,
                None,
            )
        };
        // no specular: only the normal the light falls on matters
        let matte = Material {
            specular: 0.0,
            ..Material::default()
        };
        assert_eq!(
            shaded(&matte, 1.0, toward, leaning),
            shaded(&matte, 1.0, leaning, leaning)
        );
        assert!(shaded(&matte, 1.0, toward, leaning)[0] < shaded(&matte, 1.0, toward, toward)[0]);
        // a black surface: only the reflecting normal matters
        let glossy = Material {
            roughness: 0.3,
            coat: Some([1.0, 0.1]),
            ..Material::default()
        };
        assert_eq!(
            shaded(&glossy, 0.0, toward, leaning),
            shaded(&glossy, 0.0, toward, toward)
        );
        assert!(shaded(&glossy, 0.0, toward, toward)[0] > shaded(&glossy, 0.0, leaning, toward)[0]);
    }

    /// A 64-texel texture for an eye: red, a blue ring 0.15 … 0.25 from its middle (the outer
    /// two fifths of an iris whose edge is 0.25 away), clear where u is more than 0.3 from the
    /// middle.
    fn eye_texture() -> Vec<u8> {
        let mut rgba = Vec::with_capacity(64 * 64 * 4);
        for row in 0..64 {
            for column in 0..64 {
                let u = (column as f32 + 0.5) / 64.0 - 0.5;
                let v = (row as f32 + 0.5) / 64.0 - 0.5;
                let from_middle = (u * u + v * v).sqrt();
                let ring = (0.15..0.25).contains(&from_middle);
                let alpha = if u.abs() > 0.3 { 0 } else { 255 };
                rgba.extend_from_slice(&[
                    if ring { 0 } else { 255 },
                    0,
                    if ring { 255 } else { 0 },
                    alpha,
                ]);
            }
        }
        rgba
    }

    /// A square ±0.8 across the middle of the picture at z = 0.5, its texture [`eye_texture`],
    /// drawn with `material`: an eye a hundred times life size behind it looks at the viewer,
    /// its iris in the square's plane.
    fn paint_eye(material: Material) -> Vec<u8> {
        let mesh = Arc::new(quad([
            [-0.8, -0.8, 0.5],
            [0.8, -0.8, 0.5],
            [0.8, 0.8, 0.5],
            [-0.8, 0.8, 0.5],
        ]));
        let mut seen = scene(Draw {
            material: Material {
                base: Some(5),
                double_sided: true,
                specular: 0.0,
                ..material
            },
            ..draw()
        });
        // the viewer on the side the square faces, the light coming from there
        seen.eye = [0.0, 0.0, 10.0];
        seen.lights = vec![Light::directional([0.0, 0.0, -1.0], [1.0; 3])];
        let rgba = eye_texture();
        let (_, _, painted) = rasterize(&seen, &|_| Some(mesh.clone()), &|handle| {
            (handle == 5).then_some(TextureRef {
                w: 64,
                h: 64,
                rgba: &rgba,
                levels: &[],
            })
        });
        painted
    }

    /// The eye [`paint_eye`]'s square stands for.
    fn a_big_eye() -> Eye {
        let big = Eye {
            radius: 1.2,
            iris_radius: 0.59,
            cornea_radius: 0.78,
            iris_depth: 0.33,
            ..an_eye()
        };
        let (_, iris) = heights(&big);
        Eye {
            eyes: vec![EyeAt {
                centre: [0.0, 0.0, 0.5 - iris],
                axis: [0.0, 0.0, 1.0],
                iris_uv: [0.5, 0.5],
            }],
            ..big
        }
    }

    #[test]
    fn a_painted_eye_reads_its_iris_through_the_cornea_draws_its_pupil_and_cuts_its_shell_out() {
        let pixel = |rgba: &[u8], column: usize, row: usize| {
            let at = (row * 16 + column) * 4;
            [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
        };
        let cutout = paint_eye(Material {
            surface: Surface::Cutout,
            ..Material::default()
        });
        let eye = paint_eye(Material {
            surface: Surface::Eye,
            eye: Some(a_big_eye()),
            ..Material::default()
        });
        // the middle: the pupil, black, where the texture itself is red
        assert_eq!(pixel(&eye, 8, 8), [0, 0, 0, 255]);
        assert!(pixel(&cutout, 8, 8)[0] > 100);
        // 0.44 from the middle, inside the cornea: the iris's outer ring, which the square's
        // own coordinates put elsewhere (0.27 from the texture's middle, red)
        for row in [4, 11] {
            let [red, _, blue, alpha] = pixel(&eye, 8, row);
            assert!(
                blue > 100 && red < 40 && alpha == 255,
                "row {row}: {red} {blue}"
            );
            assert!(pixel(&cutout, 8, row)[0] > 100 && pixel(&cutout, 8, row)[2] < 40);
        }
        // 0.69 from the middle, past the cornea: the white, the very bytes a cutout draws
        for row in [2, 13] {
            assert_eq!(pixel(&eye, 8, row), pixel(&cutout, 8, row));
            assert_eq!(pixel(&eye, 8, row)[3], 255);
        }
        // the clear texels are not drawn, in either
        for column in [2, 3, 12, 13] {
            assert_eq!(pixel(&eye, column, 8)[3], 0);
            assert_eq!(pixel(&cutout, column, 8)[3], 0);
        }
        // an `:eye` surface without optics is a cutout
        let bare = paint_eye(Material {
            surface: Surface::Eye,
            ..Material::default()
        });
        assert_eq!(bare, cutout);
    }

    #[test]
    fn a_contact_lays_skin_onto_its_surface_and_soft_flesh_rises_round_it() {
        // a floor at y = 0 pressed 5 mm into a soft slab facing it (its normal down)
        let floor = [Contact::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        }];
        let down = [0.0, -1.0, 0.0];
        let depths = contact_depths(&floor, [[0.0, -0.005, 0.0], [0.03, 0.002, 0.0]].iter());
        assert_eq!(depths, vec![0.005]);
        // inside: laid onto the floor (within its eased edge's fraction of a millimetre), facing
        // into it, out of the body: the floor's normal reversed
        let (laid, laid_normal) = deform_by_contacts(
            &floor,
            &depths,
            &[],
            [0.0, -0.005, 0.0],
            [0.6, -0.8, 0.0],
            1.0,
            0,
        );
        assert!(laid[1].abs() < 2e-4, "{laid:?}");
        assert!(laid_normal[1] < -0.99, "{laid_normal:?}");
        // a firm part (a nipple) sits a little behind the surface, sunk into the softer flesh
        let (firm, _) = deform_by_contacts(&floor, &depths, &[], [0.0, -0.005, 0.0], down, 0.1, 0);
        assert!(firm[1] < laid[1] - 0.0005, "{firm:?} against {laid:?}");
        // outside the eased edge, soft: lifted along its own normal (toward the floor, out of the
        // body), at most RING_LIFT × the depth at the crest; and nothing right at the edge
        let (ring, _) = deform_by_contacts(&floor, &depths, &[], [0.02, 0.012, 0.0], down, 1.0, 0);
        let lift = 0.012 - ring[1];
        assert!(lift > 0.0 && lift <= RING_LIFT * 0.005 + 1e-6, "{lift}");
        let edge = smooth_outside(1e-5, EDGE_BAND + EDGE_SPREAD * 0.005);
        assert!(
            edge > 0.0 && edge < 0.002,
            "the eased edge lifts skin off the surface only a little: {edge}"
        );
        // the same point as bone: no ring
        let (bone, _) = deform_by_contacts(&floor, &depths, &[], [0.02, 0.0005, 0.0], down, 0.0, 0);
        assert_eq!(bone, [0.02, 0.0005, 0.0]);
        // and bone inside the floor: never pressed (a limb that presses is softness 0 too)
        let (bone_inside, _) =
            deform_by_contacts(&floor, &depths, &[], [0.0, -0.005, 0.0], down, 0.0, 0);
        assert_eq!(bone_inside, [0.0, -0.005, 0.0]);
        // far off: untouched
        let (far, far_normal) =
            deform_by_contacts(&floor, &depths, &[], [0.0, 0.2, 0.0], down, 1.0, 0);
        assert_eq!((far, far_normal), ([0.0, 0.2, 0.0], down));
        // pushed far past the surface, firm skin gives only so far (MAX_SQUASH × softness) and passes
        // it, rather than being flattened away from what lies under it
        let deep_depths = contact_depths(&floor, [[0.0, -0.05, 0.0]].iter());
        let (deep, _) =
            deform_by_contacts(&floor, &deep_depths, &[], [0.0, -0.05, 0.0], down, 0.1, 0);
        assert!(deep[1] < -0.04, "{deep:?}");
        // dragged along the floor by its friction: inside, the whole drag; far off, none
        let drag = [[0.003, 0.0, 0.0]];
        let (held, _) =
            deform_by_contacts(&floor, &depths, &drag, [0.0, -0.005, 0.0], down, 1.0, 0);
        assert!((held[0] - 0.003).abs() < 1e-4, "{held:?}");
        let (free, _) = deform_by_contacts(&floor, &depths, &drag, [0.0, 0.2, 0.0], down, 1.0, 0);
        assert!(free[0].abs() < 1e-6, "{free:?}");
    }

    #[test]
    fn a_sphere_and_a_capsule_measure_from_their_surfaces() {
        let ball = Contact::Sphere {
            centre: [0.0, 0.0, 0.0],
            radius: 0.01,
        };
        let (distance, out) = ball.signed_distance([0.0, 0.0, 0.004]);
        assert!((distance + 0.006).abs() < 1e-6);
        assert_eq!(out, [0.0, 0.0, 1.0]);
        let finger = Contact::Capsule {
            start: [0.0, 0.0, 0.0],
            end: [0.05, 0.0, 0.0],
            radius: 0.008,
        };
        let (beside, out) = finger.signed_distance([0.025, 0.01, 0.0]);
        assert!((beside - 0.002).abs() < 1e-6);
        assert_eq!(out, [0.0, 1.0, 0.0]);
        // past its end, from the end's cap
        let (past, _) = finger.signed_distance([0.06, 0.0, 0.0]);
        assert!((past - 0.002).abs() < 1e-6);
    }

    #[test]
    fn a_contact_skips_the_skin_of_its_own_limb() {
        // three vertices: wholly on joint 3 (a thigh), split 60 / 40 between 3 and 5, wholly on 5
        let floats_bytes = |values: &[f32]| -> Vec<u8> {
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect()
        };
        let mesh = Mesh::from_parts(MeshParts {
            positions: floats_bytes(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            joints: vec![3, 0, 0, 0, 3, 5, 0, 0, 5, 0, 0, 0],
            weights: floats_bytes(&[1.0, 0.0, 0.0, 0.0, 0.6, 0.4, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]),
            ..MeshParts::default()
        })
        .expect("a skinned triangle");
        // contact 0 is joint 3's own limb, contact 1 nobody's
        let skips = [[3, u16::MAX, u16::MAX, u16::MAX], [u16::MAX; 4]];
        assert_eq!(skipped_by(&mesh, 0, &skips), 0b01);
        assert_eq!(skipped_by(&mesh, 1, &skips), 0b01, "60 % on its own limb");
        assert_eq!(skipped_by(&mesh, 2, &skips), 0);
        // skipped, a vertex inside the contact is left as it is
        let floor = [Contact::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        }];
        let (kept, _) = deform_by_contacts(
            &floor,
            &[0.005],
            &[],
            [0.0, -0.005, 0.0],
            [0.0, -1.0, 0.0],
            1.0,
            0b1,
        );
        assert_eq!(kept, [0.0, -0.005, 0.0]);
    }

    #[test]
    fn without_contacts_or_deform_nothing_is_pressed() {
        let mut draw = Draw {
            mesh: 1,
            model: identity(),
            palette: None,
            morphs: vec![],
            material: Material::default(),
        };
        assert!(!pressed(&draw));
        draw.material.deform = true;
        assert!(pressed(&draw));
        draw.material.surface = Surface::Eye;
        assert!(!pressed(&draw));
        assert!(contact_depths(&[], [[0.0f32; 3]].iter()).is_empty());
    }

    #[test]
    fn no_other_surface_reads_an_eye() {
        // the same bytes with the optics as without, whatever the surface
        for surface in [
            Surface::Skin,
            Surface::Standard,
            Surface::Cutout,
            Surface::Blend,
            Surface::Hair,
            Surface::Strands,
        ] {
            let plain = Material {
                surface,
                coat: Some([1.0, 0.1]),
                ..Material::default()
            };
            let with_optics = Material {
                eye: Some(a_big_eye()),
                ..plain.clone()
            };
            assert!(with_optics.eye_optics().is_none());
            assert_eq!(paint_eye(with_optics), paint_eye(plain), "{surface:?}");
        }
    }

    #[test]
    fn light_under_a_cornea_is_bent_toward_the_axis_as_snell_says_and_none_comes_from_behind() {
        let under = UnderCornea {
            axis: [0.0, 0.0, 1.0],
            ratio: 1.0 / DEFAULT_EYE_IOR,
        };
        // from 60° off the axis it arrives asin(sin 60° / n) off it, still a unit direction,
        // less what a surface of that index reflects there
        let outside = 60f32.to_radians();
        let inside = (outside.sin() / DEFAULT_EYE_IOR).asin();
        let (bent, let_in) = light_under_cornea(&under, [outside.sin(), 0.0, outside.cos()]);
        near(bent[0], inside.sin());
        near(bent[2], inside.cos());
        assert_eq!(bent[1], 0.0);
        let head_on = ((DEFAULT_EYE_IOR - 1.0) / (DEFAULT_EYE_IOR + 1.0)).powi(2);
        near(let_in, 1.0 - fresnel(head_on, outside.cos()));
        assert!(let_in > 0.9 && let_in < 0.98, "{let_in}");
        // down the axis: straight in, all but the 2.5 % reflected
        let (straight, most) = light_under_cornea(&under, [0.0, 0.0, 1.0]);
        near(straight[2], 1.0);
        near(most, 1.0 - head_on);
        // grazing the front: from the critical angle, and next to none of it
        let grazing = [0.9999f32, 0.0, (1.0 - 0.9999f32 * 0.9999).sqrt()];
        let (critical, little) = light_under_cornea(&under, grazing);
        near(critical[0], 0.9999 / DEFAULT_EYE_IOR);
        assert!(little < 0.1, "{little}");
        // from behind the front: none
        assert_eq!(light_under_cornea(&under, [0.6, 0.0, -0.8]).1, 0.0);
        // an index of 1 bends nothing and reflects nothing
        let plain = UnderCornea {
            axis: [0.0, 0.0, 1.0],
            ratio: 1.0,
        };
        let (unbent, all) = light_under_cornea(&plain, [0.6, 0.0, 0.8]);
        near(unbent[0], 0.6);
        near(unbent[2], 0.8);
        assert_eq!(all, 1.0);
    }

    #[test]
    fn the_iris_is_lit_as_a_bowl_the_side_away_from_a_side_light_the_brighter() {
        // an index of 1 for the sight line, so a point of the iris's plane is seen where it is
        let eye = Eye {
            ior: 1.0,
            pupil: 0.0,
            ..an_eye()
        };
        let (_, iris) = heights(&eye);
        let seen_across = |eye: &Eye, share: f32| {
            let world = [share * DEFAULT_IRIS_RADIUS, 0.0, iris];
            seen_at(eye, &at_rest(), world, [0.0, 0.0, 1.0])
        };
        // the bowl's normal: the axis in the middle, leaning toward it by the concavity times
        // the way out, and no further past the iris's edge
        assert_eq!(seen_across(&eye, 0.0).iris_normal, [0.0, 0.0, 1.0]);
        let half_way = seen_across(&eye, 0.5).iris_normal;
        near(-half_way[0] / half_way[2], 0.5 * DEFAULT_IRIS_CONCAVITY);
        let other_side = seen_across(&eye, -0.5).iris_normal;
        near(other_side[0] / other_side[2], 0.5 * DEFAULT_IRIS_CONCAVITY);
        assert_eq!(seen_across(&eye, 0.5).axis, [0.0, 0.0, 1.0]);
        let flat = Eye {
            concavity: 0.0,
            ..eye.clone()
        };
        assert_eq!(seen_across(&flat, 0.7).iris_normal, [0.0, 0.0, 1.0]);
        // a light 60° to the +x side of the axis, on a white iris with no highlights
        let mut lit = scene(draw());
        lit.sky = [0.0; 3];
        lit.ground = [0.0; 3];
        let outside = 60f32.to_radians();
        lit.lights = vec![Light::directional(
            [-outside.sin(), 0.0, -outside.cos()],
            [1.0; 3],
        )];
        let matte = Material {
            surface: Surface::Eye,
            specular: 0.0,
            ..Material::default()
        };
        let under = UnderCornea {
            axis: [0.0, 0.0, 1.0],
            ratio: 1.0 / DEFAULT_EYE_IOR,
        };
        let lit_at = |eye: &Eye, share: f32, under: Option<&UnderCornea>| {
            shade(
                &lit,
                &matte,
                [1.0; 3],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0; 3],
                1.0,
                &[],
                [0.0; 3],
                0.0,
                1.0,
                [0.0, 0.0, 1.0],
                &emitter_plan(&lit),
                seen_across(eye, share).iris_normal,
                under,
            )[0]
        };
        // the far side faces the light the bowl's slope more, the near side less
        let (near_side, middle, far_side) = (
            lit_at(&eye, 0.8, Some(&under)),
            lit_at(&eye, 0.0, Some(&under)),
            lit_at(&eye, -0.8, Some(&under)),
        );
        assert!(far_side > middle * 1.1 && middle > near_side * 1.1);
        // the middle: the cosine of the BENT light, times what got in
        let inside = (outside.sin() / DEFAULT_EYE_IOR).asin();
        let head_on = ((DEFAULT_EYE_IOR - 1.0) / (DEFAULT_EYE_IOR + 1.0)).powi(2);
        near(
            middle,
            inside.cos() * (1.0 - fresnel(head_on, outside.cos())),
        );
        // … brighter than the bare disc's cosine of 60°: a side light reaches under a cornea
        near(lit_at(&flat, 0.0, None), outside.cos());
        assert!(middle > 1.4 * outside.cos());
        // flat, it is lit evenly
        assert_eq!(
            lit_at(&flat, 0.8, Some(&under)),
            lit_at(&flat, -0.8, Some(&under))
        );
    }
}
