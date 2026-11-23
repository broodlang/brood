//! The signed-distance primitives (ADR-787): a lowered scene read from Brood values into
//! `host::sdf`, checked, probed at points or baked into a G-buffer. What a scene means, and the
//! lowering that makes one, are Brood (`std/sdf.blsp` for the format, the defining program for
//! the model); this file only carries numbers across.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, kw};
use crate::error::{LispError, LispResult};
use crate::host::sdf::{
    BakeRequest, FieldSource, GridSource, Light, Scene, SceneSource, Subsampling, bake,
};

use super::args::arg;
use super::bytes::bytes_to_value;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::Sig;
    primitives.def(
        "%sdf-check",
        Arity::exact(1),
        Sig::new(vec![map_ty], crate::types::Ty::of_tags(&[Tag::Str, Tag::Nil])),
        &["scene"],
        "Check a lowered signed-distance scene (the map `sdf/check` documents): nil when it is well formed, else a string naming the first record or field that is not. Never raises for a malformed scene.",
        sdf_check,
    );
    primitives.def(
        "%sdf-probe",
        Arity::exact(2),
        Sig::new(vec![map_ty, vec_ty], vec_ty),
        &["scene", "points"],
        "Evaluate a lowered signed-distance scene at `points`, a flat vector x y z x y z …: a flat vector of six values a point — the distance, the region index (nil for none), the material index (into the scene's :materials), and that material's albedo r g b with the colour zones worked in. Raises naming the record when the scene is malformed.",
        sdf_probe,
    );
    primitives.def(
        "%sdf-bake",
        Arity::exact(1),
        Sig::new(vec![map_ty], map_ty),
        &["request"],
        "March a lowered signed-distance scene into a G-buffer, on worker threads, the bytes the same whatever their number (the map `sdf/bake` documents): {:rgba bytes :height rows :subsamples n}. Raises naming the record when a scene is malformed.",
        sdf_bake,
    );
}

fn field(heap: &Heap, map: Value, name: &str) -> Value {
    match map {
        Value::Map(id) => heap.map_get(id, kw(name)).unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

fn number(value: Value, what: &str) -> Result<f64, String> {
    match value {
        Value::Int(integer) => Ok(integer as f64),
        Value::Float(float) => Ok(float),
        _ => Err(format!("{what} is not a number")),
    }
}

fn items(heap: &Heap, value: Value, what: &str) -> Result<Vec<Value>, String> {
    match value {
        Value::Vector(id) => Ok(heap.vector(id).to_vec()),
        Value::Nil => Ok(Vec::new()),
        _ => Err(format!("{what} is not a vector")),
    }
}

fn numbers(heap: &Heap, value: Value, what: &str) -> Result<Vec<f64>, String> {
    items(heap, value, what)?
        .into_iter()
        .map(|item| number(item, what))
        .collect()
}

fn optional_numbers(heap: &Heap, value: Value, what: &str) -> Result<Vec<Option<f64>>, String> {
    items(heap, value, what)?
        .into_iter()
        .map(|item| {
            if matches!(item, Value::Nil) {
                Ok(None)
            } else {
                number(item, what).map(Some)
            }
        })
        .collect()
}

fn blob(
    heap: &Heap,
    value: Value,
    what: &str,
) -> Result<std::sync::Arc<crate::core::blob::SharedBlob>, String> {
    match value {
        Value::Bytes(id) => Ok(heap.bytes(id).clone()),
        _ => Err(format!("{what} is not bytes")),
    }
}

fn grid_source(heap: &Heap, value: Value, what: &str) -> Result<GridSource, String> {
    Ok(GridSource {
        distances: blob(
            heap,
            field(heap, value, "distances"),
            &format!("{what}'s :distances"),
        )?,
        labels: blob(
            heap,
            field(heap, value, "labels"),
            &format!("{what}'s :labels"),
        )?,
        numbers: numbers(
            heap,
            field(heap, value, "numbers"),
            &format!("{what}'s :numbers"),
        )?,
    })
}

fn scene_source(heap: &Heap, scene: Value) -> Result<SceneSource, String> {
    if !matches!(scene, Value::Map(_)) {
        return Err("a scene is a map".to_string());
    }
    let fields = items(heap, field(heap, scene, "fields"), ":fields")?
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let what = format!("field {index}");
            let head = field(heap, entry, "head");
            let keep = field(heap, entry, "keep");
            Ok(FieldSource {
                body: grid_source(heap, field(heap, entry, "body"), &format!("{what}'s body"))?,
                head: if matches!(head, Value::Nil) {
                    None
                } else {
                    Some(grid_source(heap, head, &format!("{what}'s head"))?)
                },
                regions: items(
                    heap,
                    field(heap, entry, "regions"),
                    &format!("{what}'s :regions"),
                )?
                .into_iter()
                .map(|region| match region {
                    Value::Nil => Ok(None),
                    Value::Int(index) => Ok(Some(index)),
                    _ => Err(format!("{what}'s :regions holds a non-integer")),
                })
                .collect::<Result<_, String>>()?,
                keep: if matches!(keep, Value::Nil) {
                    None
                } else {
                    Some(
                        items(heap, keep, &format!("{what}'s :keep"))?
                            .into_iter()
                            .map(|flag| !matches!(flag, Value::Nil | Value::Bool(false)))
                            .collect(),
                    )
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let fallback = match field(heap, scene, "material") {
        Value::Int(index) => index,
        _ => return Err(":material is not a material index".to_string()),
    };
    Ok(SceneSource {
        records: optional_numbers(heap, field(heap, scene, "records"), ":records")?,
        points: numbers(heap, field(heap, scene, "points"), ":points")?,
        fields,
        materials: optional_numbers(heap, field(heap, scene, "materials"), ":materials")?,
        fallback,
        bounds: numbers(heap, field(heap, scene, "bounds"), ":bounds")?,
    })
}

fn scene_of(heap: &Heap, scene: Value) -> Result<Scene, String> {
    Scene::from_source(&scene_source(heap, scene)?)
}

/// `(%sdf-check scene)` — nil, or what is wrong with the scene.
fn sdf_check(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match scene_of(heap, arg(args, 0)) {
        Ok(_) => Ok(Value::Nil),
        Err(reason) => Ok(heap.alloc_string(&reason)),
    }
}

/// `(%sdf-probe scene points)` — six values a point.
fn sdf_probe(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "sdf/probe";
    let scene = scene_of(heap, arg(args, 0))
        .map_err(|reason| LispError::runtime(format!("{who}: {reason}")))?;
    let points = numbers(heap, arg(args, 1), "the points")
        .map_err(|reason| LispError::runtime(format!("{who}: {reason}")))?;
    if points.len() % 3 != 0 {
        return Err(LispError::runtime(format!(
            "{who}: the points hold {} numbers, not x y z triples",
            points.len()
        )));
    }
    let mut out = Vec::with_capacity(points.len() * 2);
    for point in points.as_chunks::<3>().0 {
        let probe = scene.probe(point[0], point[1], point[2]);
        out.push(Value::float(probe.distance));
        out.push(
            probe
                .region
                .map_or(Value::Nil, |region| Value::int(region as i64)),
        );
        out.push(Value::int(probe.material as i64));
        out.extend(probe.albedo.iter().map(|value| Value::float(*value)));
    }
    Ok(heap.alloc_vector(out))
}

fn vector3(value: &[f64], what: &str) -> Result<[f64; 3], String> {
    match value {
        [x, y, z] => Ok([*x, *y, *z]),
        _ => Err(format!("{what} is not [x y z]")),
    }
}

/// `(%sdf-bake request)` — the G-buffer's bytes.
fn sdf_bake(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "sdf/bake";
    let fail = |reason: String| LispError::runtime(format!("{who}: {reason}"));
    let request = arg(args, 0);
    let scene = scene_of(heap, field(heap, request, "scene")).map_err(fail)?;
    let castings = items(heap, field(heap, request, "castings"), ":castings")
        .map_err(fail)?
        .into_iter()
        .enumerate()
        .map(|(index, casting)| {
            scene_of(heap, casting).map_err(|reason| format!("casting {index}: {reason}"))
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(fail)?;
    let region_castings = items(
        heap,
        field(heap, request, "region-castings"),
        ":region-castings",
    )
    .map_err(fail)?
    .into_iter()
    .map(|entry| match entry {
        Value::Nil => Ok(None),
        Value::Int(index) if index >= 0 && (index as usize) < castings.len() => {
            Ok(Some(index as usize))
        }
        _ => Err(fail(
            ":region-castings holds something not a casting index".to_string(),
        )),
    })
    .collect::<Result<Vec<_>, LispError>>()?;
    let centre = vector3(
        &numbers(heap, field(heap, request, "centre"), ":centre").map_err(fail)?,
        ":centre",
    )
    .map_err(fail)?;
    let axes_values = items(heap, field(heap, request, "axes"), ":axes").map_err(fail)?;
    if axes_values.len() != 3 {
        return Err(fail(":axes is not [right down forward]".to_string()));
    }
    let mut axes = [[0.0; 3]; 3];
    for (slot, axis) in axes.iter_mut().zip(axes_values) {
        *slot = vector3(&numbers(heap, axis, "an axis").map_err(fail)?, "an axis").map_err(fail)?;
    }
    let at = numbers(heap, field(heap, request, "at"), ":at").map_err(fail)?;
    let [at_u, at_v] = at[..] else {
        return Err(fail(":at is not [u v]".to_string()));
    };
    let pixel = number(field(heap, request, "pixel"), ":pixel").map_err(fail)?;
    let count = |name: &str| match field(heap, request, name) {
        Value::Int(value) if (0..=1 << 16).contains(&value) => Ok(value as usize),
        _ => Err(fail(format!(":{name} is not a whole number 0 … 65536"))),
    };
    let (columns, rows) = (count("columns")?, count("rows")?);
    if columns * rows > 1 << 26 {
        return Err(fail(format!(
            "{columns} × {rows} pixels are more than a bake takes"
        )));
    }
    if !(pixel.is_finite() && pixel > 0.0) {
        return Err(fail(":pixel is not a positive number".to_string()));
    }
    let lights = items(heap, field(heap, request, "lights"), ":lights")
        .map_err(fail)?
        .into_iter()
        .map(|light| {
            let towards = vector3(
                &numbers(heap, field(heap, light, "towards"), "a light's :towards")?,
                "a light's :towards",
            )?;
            let softness = match field(heap, light, "softness") {
                Value::Nil => None,
                value => Some(number(value, "a light's :softness")?),
            };
            Ok(Light { towards, softness })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(fail)?;
    let antialias = field(heap, request, "antialias");
    let subsampling = if matches!(antialias, Value::Nil | Value::Bool(false)) {
        None
    } else {
        let always = numbers(
            heap,
            field(heap, antialias, "always"),
            "the antialias :always",
        )
        .map_err(fail)?
        .into_iter()
        .map(|index| index as u8)
        .collect();
        Some(Subsampling {
            depth_step: number(
                field(heap, antialias, "depth-step"),
                "the antialias :depth-step",
            )
            .map_err(fail)?,
            least_facing: number(
                field(heap, antialias, "least-facing"),
                "the antialias :least-facing",
            )
            .map_err(fail)?,
            always,
        })
    };
    let workers = match field(heap, request, "workers") {
        Value::Int(value) if value > 0 => value as usize,
        _ => std::thread::available_parallelism().map_or(1, |count| count.get()),
    };
    let baked = bake(&BakeRequest {
        scene: &scene,
        castings: region_castings
            .iter()
            .map(|entry| entry.map(|index| &castings[index]))
            .collect(),
        centre,
        axes,
        at: [at_u, at_v],
        pixel,
        columns,
        rows,
        lights,
        subsampling,
        workers,
    });
    let rgba = bytes_to_value(&baked.rgba, heap);
    Ok(heap.map_from_pairs(vec![
        (kw("rgba"), rgba),
        (kw("height"), Value::int(baked.height as i64)),
        (kw("subsamples"), Value::int(baked.subsamples as i64)),
    ]))
}
