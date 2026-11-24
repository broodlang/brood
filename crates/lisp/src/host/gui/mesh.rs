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
//! wrapped diffuse with a scatter tint at the terminator for skin, Blinn-Phong specular
//! normalised by its exponent, a fresnel rim, a hemispherical ambient, a tangent-space
//! normal map, alpha cut out (lashes, brows) or blended (hair), ACES tone mapping and sRGB.
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
/// The distance under which a point or spot light's inverse-square falloff stops growing (a
/// surface touching the bulb is not infinitely bright).
pub const NEAR_LIGHT: f32 = 0.01;
/// The texels each cube face of a point light's shadow reaches past its 90°, so the filter of
/// a receiver on a face's edge stays on that face.
const CUBE_MARGIN: f32 = 3.0;
/// The largest side of a scene's picture, in pixels.
pub const MAX_SCENE_SIDE: u32 = 4096;
/// The most vertices one mesh may have: 80 bytes each on the GPU target stays under the
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
    /// Where its vertices can be, for fitting a shadow map to a scene without posing them.
    pub bounds: MeshBounds,
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
        Ok(Mesh {
            positions,
            normals,
            uvs,
            tangents,
            joints,
            weights,
            indices,
            morphs,
            bounds,
        })
    }
}

/// How a surface is shaded and blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Wrapped diffuse with the scatter colour at the terminator (light under the skin).
    Skin,
    /// Plain diffuse.
    Standard,
    /// Standard, with texels under the material's `cutoff` alpha not drawn (lashes, brows).
    Cutout,
    /// Standard, blended over what is behind it by its alpha, after every other surface and
    /// without writing depth (hair cards).
    Blend,
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
    /// Linear RGBA factor on the base colour.
    pub color: [f32; 4],
    /// 0 (mirror) … 1 (chalk).
    pub roughness: f32,
    /// The specular reflection's strength.
    pub specular: f32,
    /// How far diffuse light wraps past the terminator, 0 … 1 (skin).
    pub wrap: f32,
    /// The colour the wrapped light takes (skin's red where light goes under it).
    pub scatter: [f32; 3],
    /// The fresnel rim's strength, lit by the sky.
    pub rim: f32,
    /// The alpha under which a cutout texel is not drawn.
    pub cutoff: f32,
    /// Draw back faces too (their normals turned to face the viewer).
    pub double_sided: bool,
    /// Whether it casts a shadow from the scene's shadowed lights (ADR-788). A `:cutout` or
    /// `:blend` surface casts only where its alpha reaches `cutoff`.
    pub cast_shadow: bool,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            surface: Surface::Standard,
            base: None,
            normal: None,
            color: [1.0, 1.0, 1.0, 1.0],
            roughness: 0.6,
            specular: 0.3,
            wrap: 0.0,
            scatter: [0.0, 0.0, 0.0],
            rim: 0.0,
            cutoff: 0.5,
            double_sided: false,
            cast_shadow: true,
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
    /// How soft the shadow's edge is: the radius, in metres at the receiver, over which it is
    /// filtered (a Poisson disc of `SOFT_SHADOW_TAPS` bilinear taps, turned per point). 0 keeps
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

/// A light (ADR-786, ADR-788). `direction` is the way its light TRAVELS (from the light into
/// the scene) for a directional or spot light; `position` is where a point or spot light is;
/// `color` is linear, intensity included — for a point or spot light the light arriving 1 m
/// away, falling off with the inverse square of the distance, windowed to nothing at `range`
/// when it is above 0. A spot's `cone` is its inner and outer half-angle (radians): full
/// inside the inner, nothing past the outer, a smoothstep between.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    pub direction: [f32; 3],
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub range: f32,
    pub cone: [f32; 2],
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

/// The light leaving a surface point toward the eye, linear and before exposure: `base` its
/// linear colour, `normal` its unit normal facing the viewer's side, `view` the unit
/// direction from it to the eye, `world` where it is, and `visibility` how much of each
/// light (by index) its shadow lets through — 1 for a light it holds no entry for.
pub fn shade(
    scene: &Scene,
    material: &Material,
    base: [f32; 3],
    normal: [f32; 3],
    view: [f32; 3],
    world: [f32; 3],
    visibility: &[f32],
) -> [f32; 3] {
    let up = normal[1] * 0.5 + 0.5;
    let mut out = [0.0f32; 3];
    for channel in 0..3 {
        out[channel] = base[channel]
            * (scene.ground[channel] + (scene.sky[channel] - scene.ground[channel]) * up);
    }
    let exponent = shininess(material.roughness);
    let normalisation = (exponent + 8.0) / (8.0 * std::f32::consts::PI);
    for (index, light) in scene.lights.iter().take(MAX_LIGHTS).enumerate() {
        let (to_light, arriving) = light_arrival(light, world);
        let arriving = arriving * visibility.get(index).copied().unwrap_or(1.0);
        let facing = dot(normal, to_light);
        let lambert = facing.max(0.0);
        let half = normalize([
            to_light[0] + view[0],
            to_light[1] + view[1],
            to_light[2] + view[2],
        ]);
        let specular =
            material.specular * normalisation * dot(normal, half).max(0.0).powf(exponent) * lambert;
        for channel in 0..3 {
            let diffuse = if material.surface == Surface::Skin {
                let wrapped = ((facing + material.wrap) / (1.0 + material.wrap)).clamp(0.0, 1.0);
                lambert + material.scatter[channel] * (wrapped - lambert)
            } else {
                lambert
            };
            out[channel] += light.color[channel] * arriving * (base[channel] * diffuse + specular);
        }
    }
    let rim = material.rim * (1.0 - dot(normal, view).max(0.0)).powi(3);
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
    plan
}

/// How much of a light a receiver at `point` gets past the depth map of `view` (`depth_map`,
/// its `resolution`² texels, row 0 the top): a 3×3 percentage-closer filter of bilinear taps,
/// read as the weights of a 4×4 block of plain comparisons — 1 lit … 0 in shadow. A point
/// outside the map, or past its depth, is lit. Texel reads off the map's left or top edge read
/// the edge; off its right or bottom edge they read the GPU target's layer of `layer_side`
/// texels, cleared to the far plane where the view did not reach. The shader's
/// `shadow_sample` is the same, a comparison gather per 2×2 of the block.
pub fn sample_shadow(
    view: &ShadowView,
    layer_side: u32,
    depth_map: &[f32],
    point: [f32; 3],
) -> f32 {
    let clip = transform(&view.view_proj, point, 1.0);
    if clip[3] <= 1e-6 || !clip[3].is_finite() {
        return 1.0;
    }
    let depth = clip[2] / clip[3];
    let across_unit = clip[0] / clip[3] * 0.5 + 0.5;
    let down_unit = 0.5 - clip[1] / clip[3] * 0.5;
    let unit = 0.0..=1.0;
    if !unit.contains(&depth) || !unit.contains(&across_unit) || !unit.contains(&down_unit) {
        return 1.0;
    }
    let side = view.resolution as i64;
    let last = i64::from(layer_side.max(1)) - 1;
    let across = across_unit * view.resolution as f32 - 0.5;
    let down = down_unit * view.resolution as f32 - 0.5;
    let (column, row) = (across.floor(), down.floor());
    let (fraction_x, fraction_y) = (across - column, down - row);
    let weights_x = [1.0 - fraction_x, 1.0, 1.0, fraction_x];
    let weights_y = [1.0 - fraction_y, 1.0, 1.0, fraction_y];
    let (column, row) = (column as i64 - 1, row as i64 - 1);
    let mut lit = 0.0;
    for (step_y, weight_y) in weights_y.iter().enumerate() {
        let read_row = (row + step_y as i64).clamp(0, last);
        for (step_x, weight_x) in weights_x.iter().enumerate() {
            let read_column = (column + step_x as i64).clamp(0, last);
            let stored = if read_column >= side || read_row >= side {
                1.0
            } else {
                depth_map[(read_row * side + read_column) as usize]
            };
            if depth <= stored {
                lit += weight_x * weight_y;
            }
        }
    }
    lit / 9.0
}

/// How much of light `index` reaches `world`, whose surface's unit normal is `normal`, past
/// its shadow — 1 for a light without one. The receiver is moved toward the light by the
/// shadow's `bias` texels (times one plus the tangent of the light's angle to the surface,
/// held at 4) and along its normal by `normal_bias` texels, both measured where it stands, so
/// a lit surface does not shadow itself (acne) at any angle and a shadow starts where its
/// caster touches the ground. The shader's `shadow_visibility` is the same.
pub fn shadow_visibility(
    scene: &Scene,
    plan: &ShadowPlan,
    maps: &[Vec<f32>],
    index: usize,
    world: [f32; 3],
    normal: [f32; 3],
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
    // a soft edge reads the map further from the receiver, where a sloped surface lies deeper:
    // moved toward the light by the filter's reach down that slope too
    let toward = shadow.bias * texel * (1.0 + slope) + shadow.softness * slope.min(2.0);
    let along = shadow.normal_bias * texel;
    let biased = [0, 1, 2].map(|axis| world[axis] + normal[axis] * along + to_light[axis] * toward);
    if shadow.softness > 0.0 && texel > 0.0 {
        let reach = (shadow.softness / texel).clamp(1.0, MAX_SOFT_SHADOW_TEXELS);
        sample_shadow_soft(
            view,
            plan.layer_side,
            depth_map,
            biased,
            reach,
            soft_turn(world),
        )
    } else {
        sample_shadow(view, plan.layer_side, depth_map, biased)
    }
}

/// The taps a soft shadow reads, each a bilinear comparison.
pub const SOFT_SHADOW_TAPS: usize = 16;
/// The widest a soft shadow's filter reaches, in texels: past this its taps fall so far apart
/// that the edge breaks into rings, so a softness wider than the map can hold is clamped.
pub const MAX_SOFT_SHADOW_TEXELS: f32 = 24.0;

/// A Poisson disc: `SOFT_SHADOW_TAPS` points none nearer another than about 0.3, spread evenly
/// out to 1.234 from the centre (the shader's `POISSON` is the same); read scaled by
/// `POISSON_INTO_UNIT` so every tap lies within the filter's reach, which the slope bias covers.
/// Brings the disc's farthest point (1.234 out) inside the unit circle.
const POISSON_INTO_UNIT: f32 = 0.81;

const POISSON: [[f32; 2]; SOFT_SHADOW_TAPS] = [
    [-0.942_016, -0.399_062],
    [0.945_586, -0.768_907],
    [-0.094_184, -0.929_389],
    [0.344_959, 0.293_878],
    [-0.915_886, 0.457_714],
    [-0.815_442, -0.879_125],
    [-0.382_775, 0.276_768],
    [0.974_844, 0.756_484],
    [0.443_233, -0.975_116],
    [0.537_430, -0.473_734],
    [-0.264_969, -0.418_930],
    [0.791_975, 0.190_909],
    [-0.241_888, 0.997_065],
    [-0.814_100, 0.914_376],
    [0.199_841, 0.786_414],
    [0.143_832, -0.141_008],
];

/// How far the disc is turned at `world`, in radians: a hash of where the point is, so the
/// taps' pattern differs from point to point and a soft edge reads as fine grain, not as rings
/// or bands. The shader's `soft_turn` is the same.
fn soft_turn(world: [f32; 3]) -> f32 {
    let seed = (world[0] * 12.9898 + world[1] * 78.233 + world[2] * 37.719).sin() * 43_758.547;
    (seed - seed.floor()) * std::f32::consts::TAU
}

/// `sample_shadow` with a soft edge: the map read at `SOFT_SHADOW_TAPS` points of a Poisson disc
/// `reach` texels wide about the receiver, turned by `turn`, each a bilinear comparison — 1 lit
/// … 0 in shadow. The shader's `shadow_sample_soft` is the same, a comparison gather a tap.
pub fn sample_shadow_soft(
    view: &ShadowView,
    layer_side: u32,
    depth_map: &[f32],
    point: [f32; 3],
    reach: f32,
    turn: f32,
) -> f32 {
    let clip = transform(&view.view_proj, point, 1.0);
    if clip[3] <= 1e-6 || !clip[3].is_finite() {
        return 1.0;
    }
    let depth = clip[2] / clip[3];
    let across_unit = clip[0] / clip[3] * 0.5 + 0.5;
    let down_unit = 0.5 - clip[1] / clip[3] * 0.5;
    let unit = 0.0..=1.0;
    if !unit.contains(&depth) || !unit.contains(&across_unit) || !unit.contains(&down_unit) {
        return 1.0;
    }
    let side = view.resolution as i64;
    let last = i64::from(layer_side.max(1)) - 1;
    let stored = |column: i64, row: i64| {
        let (column, row) = (column.clamp(0, last), row.clamp(0, last));
        if column >= side || row >= side {
            1.0
        } else {
            depth_map[(row * side + column) as usize]
        }
    };
    let lit_at = |column: i64, row: i64| {
        if depth <= stored(column, row) {
            1.0
        } else {
            0.0
        }
    };
    let (cosine, sine) = (turn.cos(), turn.sin());
    let centre = (
        across_unit * view.resolution as f32 - 0.5,
        down_unit * view.resolution as f32 - 0.5,
    );
    let mut lit = 0.0;
    for [x, y] in POISSON {
        let (x, y) = (x * POISSON_INTO_UNIT, y * POISSON_INTO_UNIT);
        let across = centre.0 + reach * (x * cosine - y * sine);
        let down = centre.1 + reach * (x * sine + y * cosine);
        let (column, row) = (across.floor(), down.floor());
        let (fraction_x, fraction_y) = (across - column, down - row);
        let (column, row) = (column as i64, row as i64);
        lit += (1.0 - fraction_x) * (1.0 - fraction_y) * lit_at(column, row)
            + fraction_x * (1.0 - fraction_y) * lit_at(column + 1, row)
            + (1.0 - fraction_x) * fraction_y * lit_at(column, row + 1)
            + fraction_x * fraction_y * lit_at(column + 1, row + 1);
    }
    lit / SOFT_SHADOW_TAPS as f32
}

// --- the CPU painter's rasteriser ---------------------------------------------------------------

/// A texture as the rasteriser samples it: straight-alpha RGBA bytes.
pub struct TextureRef<'a> {
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
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
            let texel = context
                .base_texture
                .as_ref()
                .map_or([1.0; 4], |texture| sample(texture, uv[0], uv[1]));
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
            if let Some(map) = context.normal_texture.as_ref() {
                let tangent = normalize(blend3(|vertex| {
                    [vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]]
                }));
                let bitangent = cross(normal, tangent);
                let texel = sample(map, uv[0], uv[1]);
                let local = [
                    texel[0] * 2.0 - 1.0,
                    texel[1] * 2.0 - 1.0,
                    texel[2] * 2.0 - 1.0,
                ];
                normal = normalize([
                    tangent[0] * local[0] + sign * bitangent[0] * local[1] + normal[0] * local[2],
                    tangent[1] * local[0] + sign * bitangent[1] * local[1] + normal[1] * local[2],
                    tangent[2] * local[0] + sign * bitangent[2] * local[1] + normal[2] * local[2],
                ]);
            }
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
                    );
                }
            }
            let eye = scene.eye;
            let view = normalize([eye[0] - world[0], eye[1] - world[1], eye[2] - world[2]]);
            let lit = tone(
                shade(scene, material, base, normal, view, world, &visibility),
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
            ClipVertex {
                clip: transform(&scene.view_proj, [world[0], world[1], world[2]], 1.0),
                world: [world[0], world[1], world[2]],
                normal: normalize([turned_normal[0], turned_normal[1], turned_normal[2]]),
                tangent: {
                    let unit = normalize([turned_tangent[0], turned_tangent[1], turned_tangent[2]]);
                    [unit[0], unit[1], unit[2], tangent[3]]
                },
                uv: mesh.uvs[index],
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
/// the light sees it, both faces, a `:cutout` or `:blend` surface only where its alpha reaches
/// its `cutoff` — `resolution`² depths, row 0 the top, 1 where nothing is.
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
        let alpha_tested = matches!(material.surface, Surface::Cutout | Surface::Blend);
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
            exposure: 1.0,
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

    #[test]
    fn skin_wraps_light_past_the_terminator() {
        let scene = scene(draw());
        let skin = Material {
            surface: Surface::Skin,
            wrap: 0.5,
            scatter: [1.0, 0.3, 0.2],
            ..Material::default()
        };
        // lit edge-on: plain diffuse gets nothing from the light, skin gets red
        let normal = [1.0, 0.0, 0.0];
        let plain = shade(
            &scene,
            &Material::default(),
            [1.0; 3],
            normal,
            [0.0, 0.0, -1.0],
            [0.0; 3],
            &[],
        );
        let wrapped = shade(
            &scene,
            &skin,
            [1.0; 3],
            normal,
            [0.0, 0.0, -1.0],
            [0.0; 3],
            &[],
        );
        assert!(wrapped[0] > plain[0] + 0.2, "red wraps round");
        assert!(
            wrapped[0] - plain[0] > wrapped[2] - plain[2],
            "more red than blue"
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
            &[],
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
        shade(&lit, &material, [1.0; 3], normal, normal, world, &[])[0]
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
