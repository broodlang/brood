//! Signed-distance scenes evaluated natively (ADR-787): a model lowered to flat shape records
//! by the program that defines it (b2d's `b2d-sculpt`), validated here into a [`Scene`] and
//! evaluated per point (distance, region, material) or marched into a G-buffer ([`bake`]).
//!
//! MECHANISM only. What the shapes, joins and rules MEAN is defined in Brood, by the program
//! that lowers to these records; this is a cache of that definition written out in Rust, held to
//! it by a contract test on the Brood side. Every operation is the Brood source's, in the same
//! order and with the same f64 arithmetic (`min`/`max`/`abs`/`sqrt` as Brood's own behave, no
//! fused multiply-add), so a native answer is bit for bit the Brood one and a bake does not
//! depend on the number of threads.
//!
//! The record format, a scene as Brood hands it over (`std/sdf.blsp` documents it for callers):
//!
//! - `records`: [`RECORD_WORDS`] numbers (or nil, "absent") a part, in pre-order: a group's
//!   record is followed by its parts' records, and its `size` counts them all and itself.
//! - `points`: the outlines' polygons, `x y` pairs end to end.
//! - `fields`: the distance grids a `:grid` part reads, each a body grid, an optional finer
//!   head grid, and per label byte its region index and whether it is kept.
//! - `materials`: [`MATERIAL_WORDS`] numbers a material; `fallback` the one a point gets where
//!   no part names one; `bounds` the box the model lies in.
//!
//! Validation refuses a scene whole with a message naming the record: an unknown kind or op, a
//! group whose size does not add up, nesting past [`MAX_DEPTH`], more than [`MAX_RECORDS`]
//! records, a non-finite number, an index out of range, a grid smaller than its dimensions say.
//! Nothing a scene says can make an evaluation loop longer than its constant step counts.

mod bake;
mod evaluate;
#[cfg(test)]
mod tests;

pub use bake::{BakeRequest, Baked, Light, Subsampling, bake};

/// Numbers a part record holds.
pub const RECORD_WORDS: usize = 27;
/// Numbers a material record holds.
pub const MATERIAL_WORDS: usize = 10;
/// The most records a scene may hold.
pub const MAX_RECORDS: usize = 65_536;
/// The deepest groups may nest.
pub const MAX_DEPTH: usize = 32;
/// Numbers a grid's `numbers` hold: origin x y z, spacing, its inverse, nx ny nz, quantum,
/// far corner x y z, far steps, half-diagonal steps.
pub const GRID_NUMBERS: usize = 14;

// Word offsets within a part record.
const KIND: usize = 0;
const OP: usize = 1;
const SIZE: usize = 2;
const BLEND: usize = 3;
const AMOUNT: usize = 4;
const SHELL: usize = 5;
const BOUND: usize = 6; // x y z radius
const MATERIAL: usize = 10;
const REGION: usize = 11;
const PARAMS: usize = 12; // eleven shape parameters
const TINT_COLOUR: usize = 23; // r g b
const TINT_ON: usize = 26;

/// A scene as it crosses from Brood: numbers and nils, not yet checked.
#[derive(Clone, Default)]
pub struct SceneSource {
    pub records: Vec<Option<f64>>,
    pub points: Vec<f64>,
    pub fields: Vec<FieldSource>,
    pub materials: Vec<Option<f64>>,
    pub fallback: i64,
    pub bounds: Vec<f64>,
}

/// A distance field as it crosses from Brood.
#[derive(Clone)]
pub struct FieldSource {
    pub body: GridSource,
    pub head: Option<GridSource>,
    /// Per label byte: the region index it names, or none.
    pub regions: Vec<Option<i64>>,
    /// Per label byte: whether it is kept (none: every one is).
    pub keep: Option<Vec<bool>>,
}

/// One grid of a field: int16 little-endian distances in steps of the quantum, a label byte a
/// voxel, voxel (i j k) at `i + nx (k + nz j)`.
#[derive(Clone)]
pub struct GridSource {
    pub distances: std::sync::Arc<crate::core::blob::SharedBlob>,
    pub labels: std::sync::Arc<crate::core::blob::SharedBlob>,
    pub numbers: Vec<f64>,
}

/// A validated scene, ready to evaluate. Immutable and `Sync`: a bake's threads share it.
#[derive(Debug)]
pub struct Scene {
    parts: Vec<Part>,
    tree: Tree,
    tints: Vec<Part>,
    materials: Vec<Material>,
    fallback: usize,
    bounds: [f64; 6],
    points: Vec<[f64; 2]>,
    fields: Vec<Field>,
}

/// The shape a part is.
#[derive(Clone, Debug)]
enum Shape {
    Sphere {
        centre: [f64; 3],
        radius: f64,
    },
    Ellipsoid {
        centre: [f64; 3],
        radii: [f64; 3],
    },
    Limb {
        from: [f64; 3],
        to: [f64; 3],
        radii: [f64; 2],
    },
    Box {
        centre: [f64; 3],
        half: [f64; 3],
        rounding: f64,
    },
    Outline {
        first: usize,
        count: usize,
    },
    Plane {
        at: [f64; 3],
        normal: [f64; 3],
    },
    Grid {
        field: usize,
    },
    Group {
        parts: Vec<Part>,
        tree: Tree,
    },
}

/// How a part joins what comes before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Union,
    Carve,
    Intersect,
    Swell,
    Coat,
    Tint,
}

#[derive(Clone, Debug)]
struct Part {
    shape: Shape,
    op: Op,
    blend: Option<f64>,
    amount: Option<f64>,
    shell: Option<f64>,
    bound: Option<[f64; 4]>,
    material: Option<usize>,
    region: Option<usize>,
    tint: Option<Tint>,
    /// How far beyond its bound the part can still change a distance: its blend and amount.
    reach: f64,
}

#[derive(Clone, Copy, Debug)]
struct Tint {
    colour: [f64; 3],
    on: Option<i64>,
}

/// The order-keeping bounding tree over a run of parts (`b2d-sculpt`'s `run-tree`): runs of up
/// to three parts stepped in turn, longer runs halved under one sphere round all of them.
#[derive(Clone, Debug)]
enum Tree {
    Steps {
        start: usize,
        end: usize,
    },
    Split {
        first: Box<Tree>,
        second: Box<Tree>,
        cull: Option<Cull>,
    },
}

#[derive(Clone, Copy, Debug)]
struct Cull {
    centre: [f64; 3],
    radius: f64,
    most_reach: f64,
    has_union: bool,
    has_carve: bool,
}

/// A material as the G-buffer needs it, resolved and indexed by the lowering.
#[derive(Clone, Copy, Debug)]
pub struct Material {
    /// Its index among the model's materials, the G-buffer's material byte.
    pub index: u8,
    /// Its name, as a number the lowering chose (tints match it); none for no name.
    pub name: Option<i64>,
    pub albedo: [f64; 3],
    /// Its material's own albedo, which a tinted albedo is kept as a ratio of; none to use
    /// the albedo itself.
    pub base: Option<[f64; 3]>,
    pub alpha: f64,
    /// Skin's scattering radius: the wide normal's sampling distance; none for no scattering.
    pub scatter: Option<f64>,
}

#[derive(Debug)]
struct Field {
    body: Grid,
    head: Option<Grid>,
    regions: Vec<Option<usize>>,
    keep: Option<Vec<bool>>,
}

struct Grid {
    distances: Vec<i16>,
    labels: std::sync::Arc<crate::core::blob::SharedBlob>,
    origin: [f64; 3],
    spacing: f64,
    inverse: f64,
    dimensions: [i64; 3],
    quantum: f64,
    far: [f64; 3],
    far_steps: i64,
    half_diagonal_steps: i64,
}

/// What a point of a scene is: its distance, the region its surface belongs to, the material
/// index (into the scene's materials) and that material's albedo with the colour zones in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Probe {
    pub distance: f64,
    pub region: Option<usize>,
    pub material: usize,
    pub albedo: [f64; 3],
}

fn finite(value: f64, what: &str) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{what} is not a finite number"))
    }
}

fn whole_number(value: f64, what: &str) -> Result<i64, String> {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9.0e15 {
        Ok(value as i64)
    } else {
        Err(format!("{what} is {value}, not a whole number"))
    }
}

fn index_below(value: f64, limit: usize, what: &str) -> Result<usize, String> {
    let number = whole_number(value, what)?;
    if number < 0 || number as usize >= limit {
        Err(format!(
            "{what} is {number}, outside 0 … {}",
            limit as i64 - 1
        ))
    } else {
        Ok(number as usize)
    }
}

impl Scene {
    /// Check `source` and build the scene, or say what is wrong with it.
    pub fn from_source(source: &SceneSource) -> Result<Scene, String> {
        if !source.records.len().is_multiple_of(RECORD_WORDS) {
            return Err(format!(
                "the records hold {} numbers, not a multiple of {RECORD_WORDS}",
                source.records.len()
            ));
        }
        let count = source.records.len() / RECORD_WORDS;
        if count > MAX_RECORDS {
            return Err(format!("{count} records, more than {MAX_RECORDS}"));
        }
        if !source.points.len().is_multiple_of(2) {
            return Err("the outline points are not x y pairs".to_string());
        }
        let points: Vec<[f64; 2]> = source
            .points
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .map(|(index, pair)| {
                Ok([
                    finite(pair[0], &format!("outline point {index}"))?,
                    finite(pair[1], &format!("outline point {index}"))?,
                ])
            })
            .collect::<Result<_, String>>()?;
        if !source.materials.len().is_multiple_of(MATERIAL_WORDS) || source.materials.is_empty() {
            return Err(format!(
                "the materials hold {} numbers, not a positive multiple of {MATERIAL_WORDS}",
                source.materials.len()
            ));
        }
        let materials = source
            .materials
            .as_chunks::<MATERIAL_WORDS>()
            .0
            .iter()
            .enumerate()
            .map(|(index, words)| material_of(index, words))
            .collect::<Result<Vec<_>, String>>()?;
        if source.fallback < 0 || source.fallback as usize >= materials.len() {
            return Err(format!(
                "the fallback material {} is not one of the materials",
                source.fallback
            ));
        }
        if source.bounds.len() != 6 {
            return Err(format!(
                "the bounds hold {} numbers, not 6",
                source.bounds.len()
            ));
        }
        let mut bounds = [0.0; 6];
        for (slot, value) in bounds.iter_mut().zip(&source.bounds) {
            *slot = finite(*value, "a bound")?;
        }
        let fields = source
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| field_of(index, field))
            .collect::<Result<Vec<_>, String>>()?;
        let context = Context {
            records: &source.records,
            points: points.len(),
            fields: fields.len(),
            materials: materials.len(),
        };
        let mut parts = Vec::new();
        let mut at = 0;
        while at < count {
            let (part, next) = context.part(at, 0)?;
            parts.push(part);
            at = next;
        }
        let tree = run_tree(&parts, 0, parts.len());
        let mut tints = Vec::new();
        collect_tints(&parts, &mut tints);
        Ok(Scene {
            parts,
            tree,
            tints,
            materials,
            fallback: source.fallback as usize,
            bounds,
            points,
            fields,
        })
    }

    /// The box the model lies in: min x y z, max x y z.
    pub fn bounds(&self) -> [f64; 6] {
        self.bounds
    }

    /// The scene's materials, in the order the lowering gave them.
    pub fn materials(&self) -> &[Material] {
        &self.materials
    }

    /// Distance, region, material and tinted albedo at a point.
    pub fn probe(&self, x: f64, y: f64, z: f64) -> Probe {
        let material = self.material_at(x, y, z);
        Probe {
            distance: self.distance(x, y, z),
            region: self.region(x, y, z),
            material,
            albedo: self.tinted(material, x, y, z),
        }
    }
}

fn material_of(index: usize, words: &[Option<f64>]) -> Result<Material, String> {
    let what = |slot: &str| format!("material {index}'s {slot}");
    let number = |slot: usize, name: &str| -> Result<f64, String> {
        match words[slot] {
            Some(value) => finite(value, &what(name)),
            None => Err(format!("{} is missing", what(name))),
        }
    };
    let byte = whole_number(number(0, "index")?, &what("index"))?;
    if !(0..=255).contains(&byte) {
        return Err(format!("{} is {byte}, not a byte", what("index")));
    }
    let name = match words[1] {
        Some(value) => Some(whole_number(value, &what("name"))?),
        None => None,
    };
    let albedo = [
        number(2, "albedo")?,
        number(3, "albedo")?,
        number(4, "albedo")?,
    ];
    let base = match (words[5], words[6], words[7]) {
        (None, None, None) => None,
        _ => Some([
            number(5, "base albedo")?,
            number(6, "base albedo")?,
            number(7, "base albedo")?,
        ]),
    };
    let alpha = number(8, "alpha")?;
    let scatter = match words[9] {
        Some(value) => Some(finite(value, &what("scatter"))?),
        None => None,
    };
    Ok(Material {
        index: byte as u8,
        name,
        albedo,
        base,
        alpha,
        scatter,
    })
}

fn grid_of(field: usize, which: &str, source: &GridSource) -> Result<Grid, String> {
    let what = format!("field {field}'s {which} grid");
    if source.numbers.len() != GRID_NUMBERS {
        return Err(format!(
            "{what} has {} numbers, not {GRID_NUMBERS}",
            source.numbers.len()
        ));
    }
    let mut numbers = [0.0; GRID_NUMBERS];
    for (slot, value) in numbers.iter_mut().zip(&source.numbers) {
        *slot = finite(*value, &what)?;
    }
    let dimension = |slot: usize| whole_number(numbers[slot], &format!("{what}'s dimension"));
    let dimensions = [dimension(5)?, dimension(6)?, dimension(7)?];
    if dimensions.iter().any(|&size| !(3..=65_536).contains(&size)) {
        return Err(format!(
            "{what}'s dimensions {dimensions:?} are not each 3 … 65536"
        ));
    }
    let voxels = dimensions[0] as usize * dimensions[1] as usize * dimensions[2] as usize;
    let bytes = source.distances.as_bytes();
    if bytes.len() < 2 * voxels {
        return Err(format!(
            "{what} has {} distance bytes, fewer than its {voxels} voxels need",
            bytes.len()
        ));
    }
    if source.labels.as_bytes().len() < voxels {
        return Err(format!(
            "{what} has {} label bytes, fewer than its {voxels} voxels",
            source.labels.len()
        ));
    }
    if numbers[3] <= 0.0 || numbers[4] <= 0.0 {
        return Err(format!("{what}'s spacing is not positive"));
    }
    let distances = bytes[..2 * voxels]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    Ok(Grid {
        distances,
        labels: source.labels.clone(),
        origin: [numbers[0], numbers[1], numbers[2]],
        spacing: numbers[3],
        inverse: numbers[4],
        dimensions,
        quantum: numbers[8],
        far: [numbers[9], numbers[10], numbers[11]],
        far_steps: whole_number(numbers[12], &format!("{what}'s far steps"))?,
        half_diagonal_steps: whole_number(numbers[13], &format!("{what}'s half-diagonal steps"))?,
    })
}

fn field_of(index: usize, source: &FieldSource) -> Result<Field, String> {
    let regions = source
        .regions
        .iter()
        .map(|region| match region {
            Some(value) if *value < 0 => Err(format!("field {index} names region {value}")),
            Some(value) => Ok(Some(*value as usize)),
            None => Ok(None),
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Field {
        body: grid_of(index, "body", &source.body)?,
        head: match &source.head {
            Some(head) => Some(grid_of(index, "head", head)?),
            None => None,
        },
        regions,
        keep: source.keep.clone(),
    })
}

/// What a record is checked against.
struct Context<'a> {
    records: &'a [Option<f64>],
    points: usize,
    fields: usize,
    materials: usize,
}

impl Context<'_> {
    /// The part whose record is number `at`, and the number of the record after it (and its
    /// parts', for a group).
    fn part(&self, at: usize, depth: usize) -> Result<(Part, usize), String> {
        if depth > MAX_DEPTH {
            return Err(format!("record {at}: groups nest deeper than {MAX_DEPTH}"));
        }
        let words = &self.records[at * RECORD_WORDS..(at + 1) * RECORD_WORDS];
        let what = |slot: &str| format!("record {at}'s {slot}");
        let optional = |slot: usize, name: &str| -> Result<Option<f64>, String> {
            match words[slot] {
                Some(value) => Ok(Some(finite(value, &what(name))?)),
                None => Ok(None),
            }
        };
        let number = |slot: usize, name: &str| -> Result<f64, String> {
            optional(slot, name)?.ok_or_else(|| format!("{} is missing", what(name)))
        };
        let kind = whole_number(number(KIND, "kind")?, &what("kind"))?;
        let op = match whole_number(number(OP, "op")?, &what("op"))? {
            0 => Op::Union,
            1 => Op::Carve,
            2 => Op::Intersect,
            3 => Op::Swell,
            4 => Op::Coat,
            5 => Op::Tint,
            other => return Err(format!("record {at}: unknown op {other}")),
        };
        let size = whole_number(number(SIZE, "size")?, &what("size"))?;
        let total = self.records.len() / RECORD_WORDS;
        if size < 1 || at as i64 + size > total as i64 {
            return Err(format!("record {at}: size {size} runs past the records"));
        }
        let blend = optional(BLEND, "blend")?;
        let amount = optional(AMOUNT, "amount")?;
        let shell = optional(SHELL, "shell")?;
        let bound = match (
            words[BOUND],
            words[BOUND + 1],
            words[BOUND + 2],
            words[BOUND + 3],
        ) {
            (None, None, None, None) => None,
            _ => Some([
                number(BOUND, "bound")?,
                number(BOUND + 1, "bound")?,
                number(BOUND + 2, "bound")?,
                number(BOUND + 3, "bound")?,
            ]),
        };
        let material = match words[MATERIAL] {
            Some(value) => Some(index_below(value, self.materials, &what("material"))?),
            None => None,
        };
        let region = match words[REGION] {
            Some(value) => Some(index_below(value, usize::MAX, &what("region"))?),
            None => None,
        };
        let param = |offset: usize| number(PARAMS + offset, "shape parameter");
        let point = |offset: usize| -> Result<[f64; 3], String> {
            Ok([param(offset)?, param(offset + 1)?, param(offset + 2)?])
        };
        let mut next = at + 1;
        let shape = match kind {
            0 => {
                let end = at + size as usize;
                let mut parts = Vec::new();
                while next < end {
                    let (part, after) = self.part(next, depth + 1)?;
                    parts.push(part);
                    next = after;
                }
                if next != end {
                    return Err(format!(
                        "record {at}: the group's size {size} splits a part"
                    ));
                }
                let tree = run_tree(&parts, 0, parts.len());
                Shape::Group { parts, tree }
            }
            1 => Shape::Sphere {
                centre: point(0)?,
                radius: param(3)?,
            },
            2 => Shape::Ellipsoid {
                centre: point(0)?,
                radii: point(3)?,
            },
            3 => Shape::Limb {
                from: point(0)?,
                to: point(3)?,
                radii: [param(6)?, param(7)?],
            },
            4 => Shape::Box {
                centre: point(0)?,
                half: point(3)?,
                rounding: param(6)?,
            },
            5 => {
                let first = whole_number(param(0)?, &what("outline start"))?;
                let count = whole_number(param(1)?, &what("outline count"))?;
                if first < 0 || count < 1 || (first + count) as usize > self.points {
                    return Err(format!(
                        "record {at}: outline points {first} … {} are not all there",
                        first + count - 1
                    ));
                }
                Shape::Outline {
                    first: first as usize,
                    count: count as usize,
                }
            }
            6 => Shape::Plane {
                at: point(0)?,
                normal: point(3)?,
            },
            7 => Shape::Grid {
                field: index_below(param(0)?, self.fields, &what("field"))?,
            },
            other => return Err(format!("record {at}: unknown kind {other}")),
        };
        if kind != 0 && size != 1 {
            return Err(format!("record {at}: a shape's size is 1, not {size}"));
        }
        if matches!(op, Op::Swell | Op::Coat | Op::Tint) && amount.is_none() {
            return Err(format!(
                "record {at}: a swell, coat or tint needs an amount"
            ));
        }
        let tint = if op == Op::Tint {
            let colour = [
                number(TINT_COLOUR, "tint colour")?,
                number(TINT_COLOUR + 1, "tint colour")?,
                number(TINT_COLOUR + 2, "tint colour")?,
            ];
            let on = match words[TINT_ON] {
                Some(value) => Some(whole_number(value, &what("tint material"))?),
                None => None,
            };
            Some(Tint { colour, on })
        } else {
            None
        };
        let reach = evaluate::reach_of(blend, amount);
        Ok((
            Part {
                shape,
                op,
                blend,
                amount,
                shell,
                bound,
                material,
                region,
                tint,
                reach,
            },
            next,
        ))
    }
}

fn collect_tints(parts: &[Part], into: &mut Vec<Part>) {
    // `b2d-sculpt`'s `tints-in`: every tint part in order, groups' included
    for part in parts {
        match &part.shape {
            Shape::Group { parts, .. } => collect_tints(parts, into),
            _ if part.op == Op::Tint => into.push(part.clone()),
            _ => {}
        }
    }
}

/// `run-tree` over `parts[start..end]`: runs of three or fewer stepped in turn, longer ones
/// halved, each half culled by the sphere round all its parts when every one has a bound and
/// none intersects.
fn run_tree(parts: &[Part], start: usize, end: usize) -> Tree {
    let total = end - start;
    if total <= 3 {
        return Tree::Steps { start, end };
    }
    let half = start + total / 2;
    let first = Box::new(run_tree(parts, start, half));
    let second = Box::new(run_tree(parts, half, end));
    let run = &parts[start..end];
    let unbounded = run
        .iter()
        .any(|part| part.bound.is_none() || part.op == Op::Intersect);
    let cull = if unbounded {
        None
    } else {
        let bounds: Vec<[f64; 4]> = run.iter().filter_map(|part| part.bound).collect();
        let (centre, radius) = evaluate::enclosing(&bounds);
        Some(Cull {
            centre,
            radius,
            most_reach: run
                .iter()
                .fold(0.0, |most, part| evaluate::brood_max(most, part.reach)),
            has_union: run.iter().any(|part| part.op == Op::Union),
            has_carve: run.iter().any(|part| part.op == Op::Carve),
        })
    };
    Tree::Split {
        first,
        second,
        cull,
    }
}

impl std::fmt::Debug for Grid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Grid")
            .field("origin", &self.origin)
            .field("spacing", &self.spacing)
            .field("dimensions", &self.dimensions)
            .finish_non_exhaustive()
    }
}
