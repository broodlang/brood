//! A hair lock's sub-steps, natively (ADR-821): b3d-hair's locks read from Brood values into
//! `host::hair`, stepped, and handed back. What a step means is b3d-hair's; this file only
//! carries numbers across.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, kw};
use crate::error::{LispError, LispResult};
use crate::host::hair::{Constants, Lock, Shape, Vector, moved};

use super::args::arg;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::Sig;
    primitives.def(
        "%hair-moved",
        Arity::exact(6),
        Sig::new(
            vec![
                vec_ty,
                num,
                num,
                crate::types::Ty::of_tags(&[Tag::Vector, Tag::Nil]),
                vec_ty,
                map_ty,
            ],
            vec_ty,
        ),
        &["locks", "steps", "step", "shapes-before", "shapes", "constants"],
        "Every lock of `locks` (maps of :points :previous :before :now :carries :nears :stiffness :follows :damping :upright :holds :lengths :margins :limits :thickness, as b3d-hair's `moved-with` holds them) `steps` sub-steps of `step` seconds on against her body's `shapes` (blended from `shapes-before` across the sub-steps when given), with b3d-hair's settings in `constants`: a vector of `[points previous]` a lock, each a vector of [x y z]. The same arithmetic as b3d-hair's `sub-step`, natively and in one call.",
        hair_moved,
    );
}

fn field(heap: &Heap, map: Value, name: &str) -> Value {
    match map {
        Value::Map(id) => heap.map_get(id, kw(name)).unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

fn number(_heap: &Heap, value: Value, what: &str) -> Result<f64, String> {
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
        .map(|item| number(heap, item, what))
        .collect()
}

fn optional_numbers(heap: &Heap, value: Value, what: &str) -> Result<Option<Vec<f64>>, String> {
    if matches!(value, Value::Nil) {
        Ok(None)
    } else {
        numbers(heap, value, what).map(Some)
    }
}

fn vectors(heap: &Heap, value: Value, what: &str) -> Result<Vec<Vector>, String> {
    items(heap, value, what)?
        .into_iter()
        .map(|item| match numbers(heap, item, what)?.as_slice() {
            [x, y, z] => Ok([*x, *y, *z]),
            _ => Err(format!("{what} holds something that is not [x y z]")),
        })
        .collect()
}

fn shapes(heap: &Heap, value: Value, what: &str) -> Result<Vec<Shape>, String> {
    items(heap, value, what)?
        .into_iter()
        .map(|shape| {
            // (a pane's `:pane` mark, its one item that is not a number, as nothing)
            items(heap, shape, what).map(|numbers| {
                Shape(
                    numbers
                        .into_iter()
                        .map(|item| number(heap, item, what).unwrap_or(0.0))
                        .collect(),
                )
            })
        })
        .collect()
}

fn lock_of(heap: &Heap, map: Value, count: usize) -> Result<Lock, String> {
    let nears = items(heap, field(heap, map, "nears"), ":nears")?
        .into_iter()
        .map(|places| {
            numbers(heap, places, ":nears")?
                .into_iter()
                .map(|place| {
                    let place = place as usize;
                    if place < count {
                        Ok(place)
                    } else {
                        Err(format!(":nears names shape {place} of {count}"))
                    }
                })
                .collect()
        })
        .collect::<Result<Vec<Vec<usize>>, String>>()?;
    let lock = Lock {
        points: vectors(heap, field(heap, map, "points"), ":points")?,
        previous: vectors(heap, field(heap, map, "previous"), ":previous")?,
        before: vectors(heap, field(heap, map, "before"), ":before")?,
        now: vectors(heap, field(heap, map, "now"), ":now")?,
        carries: vectors(heap, field(heap, map, "carries"), ":carries")?,
        nears,
        stiffness: numbers(heap, field(heap, map, "stiffness"), ":stiffness")?,
        follows: optional_numbers(heap, field(heap, map, "follows"), ":follows")?,
        damping: numbers(heap, field(heap, map, "damping"), ":damping")?,
        upright: number(heap, field(heap, map, "upright"), ":upright")?,
        holds: numbers(heap, field(heap, map, "holds"), ":holds")?,
        lengths: numbers(heap, field(heap, map, "lengths"), ":lengths")?,
        margins: numbers(heap, field(heap, map, "margins"), ":margins")?,
        limits: numbers(heap, field(heap, map, "limits"), ":limits")?,
        thickness: optional_numbers(heap, field(heap, map, "thickness"), ":thickness")?,
    };
    let points = lock.points.len();
    let bones = points.saturating_sub(1);
    let short = |name: &str, have: usize, need: usize| {
        if have < need {
            Err(format!(
                "{name} has {have}, the lock's {points} points need {need}"
            ))
        } else {
            Ok(())
        }
    };
    short(":previous", lock.previous.len(), points)?;
    short(":before", lock.before.len(), points)?;
    short(":now", lock.now.len(), points)?;
    short(":carries", lock.carries.len(), bones)?;
    short(":nears", lock.nears.len(), bones)?;
    short(":stiffness", lock.stiffness.len(), bones)?;
    short(":damping", lock.damping.len(), bones)?;
    short(":lengths", lock.lengths.len(), bones)?;
    short(":margins", lock.margins.len(), points)?;
    short(":limits", lock.limits.len(), points)?;
    if let Some(follows) = &lock.follows {
        short(":follows", follows.len(), points)?;
    }
    if let Some(thickness) = &lock.thickness {
        short(":thickness", thickness.len(), points)?;
    }
    Ok(lock)
}

fn constants_of(heap: &Heap, map: Value) -> Result<Constants, String> {
    let get = |name: &str| number(heap, field(heap, map, name), &format!(":{name}"));
    let gravity = match numbers(heap, field(heap, map, "gravity"), ":gravity")?.as_slice() {
        [x, y, z] => [*x, *y, *z],
        _ => return Err(":gravity is not [x y z]".to_string()),
    };
    Ok(Constants {
        gravity,
        air_drag: get("air-drag")?,
        still: get("still")?,
        carried: get("carried")?,
        slide: get("slide")?,
        rests: get("rests")?,
        pressed: get("pressed")?,
        shove: get("shove")?,
        unfold_give: get("unfold-give")?,
        ground: get("ground")?,
        clearance: get("clearance")?,
        passes: get("passes")? as usize,
    })
}

/// `(%hair-moved locks steps step shapes-before shapes constants)`
fn hair_moved(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "hair/moved";
    let fail = |reason: String| LispError::runtime(format!("{who}: {reason}"));
    let shapes_now = shapes(heap, arg(args, 4), "the shapes").map_err(fail)?;
    let before = match arg(args, 3) {
        Value::Nil => None,
        value => Some(shapes(heap, value, "the shapes before").map_err(fail)?),
    };
    let steps = number(heap, arg(args, 1), "steps").map_err(fail)? as usize;
    let step = number(heap, arg(args, 2), "step").map_err(fail)?;
    let constants = constants_of(heap, arg(args, 5)).map_err(fail)?;
    let locks = items(heap, arg(args, 0), "the locks")
        .map_err(fail)?
        .into_iter()
        .map(|map| lock_of(heap, map, shapes_now.len()))
        .collect::<Result<Vec<Lock>, String>>()
        .map_err(fail)?;
    let mut out = Vec::with_capacity(locks.len());
    for lock in &locks {
        let (points, previous) = moved(
            lock,
            steps,
            step,
            before.as_deref(),
            &shapes_now,
            &constants,
        );
        let mut as_value = |chain: Vec<Vector>| {
            let items: Vec<Value> = chain
                .into_iter()
                .map(|[x, y, z]| {
                    heap.alloc_vector(vec![Value::float(x), Value::float(y), Value::float(z)])
                })
                .collect();
            heap.alloc_vector(items)
        };
        let points = as_value(points);
        let previous = as_value(previous);
        out.push(heap.alloc_vector(vec![points, previous]));
    }
    Ok(heap.alloc_vector(out))
}
