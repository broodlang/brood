//! A hair lock's sub-steps, natively (ADR-821): b3d-hair's locks read from Brood values into
//! `host::hair`, stepped, and handed back. What a step means is b3d-hair's; this file only
//! carries numbers across.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, kw};
use crate::error::{LispError, LispResult};
use crate::host::hair::{
    Constants, Link, LinkConstants, Lock, Settling, Shape, Vector, linked, moved,
};

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
    primitives.def(
        "%hair-advance",
        Arity::exact(10),
        Sig::new(
            vec![
                vec_ty,
                vec_ty,
                vec_ty,
                crate::types::Ty::of_tags(&[Tag::Vector, Tag::Nil]),
                num,
                crate::types::Ty::of_tags(&[Tag::Bool, Tag::Nil]),
                vec_ty,
                crate::types::Ty::of_tags(&[Tag::Vector, Tag::Nil]),
                map_ty,
                map_ty,
            ],
            vec_ty,
        ),
        &[
            "locks",
            "worlds",
            "base",
            "twist",
            "dt",
            "moved-shapes",
            "shapes",
            "shapes-before",
            "constants",
            "plan-constants",
        ],
        "Every lock of `locks` (b3d-hair's state locks, each with its :root-parent: the root joint's parent, nil for the skeleton's base) planned as b3d-hair's `moved-plan` plans it, from the frame's `worlds` (16 numbers a joint), `base`, the head's `twist` ([angle [x y z]] or nil), `dt` and whether the colliders `moved-shapes`, then its sub-steps taken against `shapes` (blended from `shapes-before`) as `%hair-moved` takes them, with b3d-hair's settings in `constants` and `plan-constants` (:rate :most-steps :drift :clearance): a vector of `[points previous now step fresh nears bounce]` a lock (its curls' stretch planned too, from `plan-constants`' :bounce :stretch :bounce-most). b3d-hair's plan and sub-steps for a whole frame natively, in one call (ADR-821 amended).",
        hair_advance,
    );
    primitives.def(
        "%hair-turned",
        Arity::exact(4),
        Sig::new(vec![vec_ty, vec_ty, vec_ty, vec_ty], vec_ty),
        &["locks", "pose", "worlds", "base"],
        "Every lock of `locks` (maps of :joints :points :bounce :root-parent) with its joints turned, root first, to point along its points, as b3d-hair's `turned-lock` turns them: from the `pose`'s local transforms (maps of :rotation :translation :scale a joint), the frame's `worlds` (16 numbers a joint) and the skeleton's `base`. A vector a lock of `[rotation translation world]` a joint: its new local rotation and translation and its world matrix (ADR-821 amended, stage 2).",
        hair_turned,
    );
    primitives.def(
        "%hair-linked",
        Arity::exact(5),
        Sig::new(vec![vec_ty, vec_ty, vec_ty, map_ty, map_ty], vec_ty),
        &["locks", "links", "shapes", "constants", "link-constants"],
        "Every link of `links` (`[a b rests]`, b3d-hair's `links-of`) between the locks of `locks` (maps of :points :previous :lengths :margins :limits :thickness :nears, :lengths as the curls stretch them now) brought back within its bounds as b3d-hair's `cohered` does (`link-constants`: :together :apart :laid [least most] :give), then each lock the links moved settled again against her body's `shapes` as its `settled-again` settles it (b3d-hair's settings in `constants`): a vector a lock, nil where its points are as they were, else `[points previous]` (ADR-821 amended, stage 3).",
        hair_linked,
    );
}

/// `(%hair-turned locks pose worlds base)`
fn hair_turned(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::host::hair::{Local, turned_lock};
    let who = "hair/turned";
    let fail = |reason: String| LispError::runtime(format!("{who}: {reason}"));
    let triple = |value: Value, what: &str| -> Result<Vector, String> {
        match numbers(heap, value, what)?.as_slice() {
            [x, y, z] => Ok([*x, *y, *z]),
            _ => Err(format!("{what} is not [x y z]")),
        }
    };
    let pose = items(heap, arg(args, 1), "the pose")
        .map_err(fail)?
        .into_iter()
        .map(|local| -> Result<Local, String> {
            let rotation =
                match numbers(heap, field(heap, local, "rotation"), ":rotation")?.as_slice() {
                    [x, y, z, w] => [*x, *y, *z, *w],
                    _ => return Err(":rotation is not [x y z w]".into()),
                };
            Ok(Local {
                rotation,
                translation: triple(field(heap, local, "translation"), ":translation")?,
                scale: triple(field(heap, local, "scale"), ":scale")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(fail)?;
    let worlds = items(heap, arg(args, 2), "the worlds")
        .map_err(fail)?
        .into_iter()
        .map(|world| matrix(heap, world, "a world matrix"))
        .collect::<Result<Vec<_>, String>>()
        .map_err(fail)?;
    let base = matrix(heap, arg(args, 3), "the base").map_err(fail)?;
    let locks = items(heap, arg(args, 0), "the locks").map_err(fail)?;
    let mut out = Vec::with_capacity(locks.len());
    for lock in locks {
        let joints: Vec<usize> = numbers(heap, field(heap, lock, "joints"), ":joints")
            .map_err(fail)?
            .into_iter()
            .map(|joint| joint as usize)
            .collect();
        let points = vectors(heap, field(heap, lock, "points"), ":points").map_err(fail)?;
        let stretch = match bounce_of(heap, field(heap, lock, "bounce")).map_err(fail)? {
            Some((b, _, _)) => 1.0 + b,
            None => 1.0,
        };
        let above = match field(heap, lock, "root-parent") {
            Value::Nil => base,
            value => {
                let parent = number(heap, value, ":root-parent").map_err(fail)? as usize;
                *worlds
                    .get(parent)
                    .ok_or_else(|| fail(format!("root parent {parent} past the worlds")))?
            }
        };
        if points.len() != joints.len() + 1 || joints.iter().any(|&joint| joint >= pose.len()) {
            return Err(fail("a lock's points or joints do not fit its pose".into()));
        }
        let turned = turned_lock(&joints, &points, stretch, &pose, &above);
        let mut joints_out = Vec::with_capacity(turned.len());
        for (rotation, place, world) in turned {
            let rotation = heap.alloc_vector(rotation.iter().map(|&n| Value::float(n)).collect());
            let place = heap.alloc_vector(place.iter().map(|&n| Value::float(n)).collect());
            let world = heap.alloc_vector(world.iter().map(|&n| Value::float(n)).collect());
            joints_out.push(heap.alloc_vector(vec![rotation, place, world]));
        }
        out.push(heap.alloc_vector(joints_out));
    }
    Ok(heap.alloc_vector(out))
}

fn matrix(heap: &Heap, value: Value, what: &str) -> Result<crate::host::hair::Matrix, String> {
    let numbers = numbers(heap, value, what)?;
    numbers
        .try_into()
        .map_err(|numbers: Vec<f64>| format!("{what} has {} numbers, not 16", numbers.len()))
}

fn held_of(heap: &Heap, map: Value, count: usize) -> Result<crate::host::hair::Held, String> {
    let place_lists = |value: Value, what: &str| -> Result<Vec<Vec<usize>>, String> {
        items(heap, value, what)?
            .into_iter()
            .map(|places| {
                numbers(heap, places, what)?
                    .into_iter()
                    .map(|place| {
                        let place = place as usize;
                        if place < count {
                            Ok(place)
                        } else {
                            Err(format!("{what} names shape {place} of {count}"))
                        }
                    })
                    .collect()
            })
            .collect()
    };
    let optional_vectors = |value: Value, what: &str| -> Result<Option<Vec<Vector>>, String> {
        if matches!(value, Value::Nil) {
            Ok(None)
        } else {
            vectors(heap, value, what).map(Some)
        }
    };
    let up = match numbers(heap, field(heap, map, "up"), ":up")?.as_slice() {
        [] => [0.0, 1.0, 0.0],
        [x, y, z] => [*x, *y, *z],
        _ => return Err(":up is not [x y z]".into()),
    };
    let root_parent = match field(heap, map, "root-parent") {
        Value::Nil => None,
        value => Some(number(heap, value, ":root-parent")? as usize),
    };
    let last_step = match field(heap, map, "step") {
        Value::Nil => None,
        value => Some(number(heap, value, ":step")?),
    };
    Ok(crate::host::hair::Held {
        points: vectors(heap, field(heap, map, "points"), ":points")?,
        previous: vectors(heap, field(heap, map, "previous"), ":previous")?,
        last_step,
        posed: optional_vectors(field(heap, map, "posed"), ":posed")?,
        preload: vectors(heap, field(heap, map, "preload"), ":preload")?,
        up,
        nears: place_lists(field(heap, map, "nears"), ":nears")?,
        nears_at: optional_vectors(field(heap, map, "nears-at"), ":nears-at")?,
        joints: numbers(heap, field(heap, map, "joints"), ":joints")?
            .into_iter()
            .map(|joint| joint as usize)
            .collect(),
        root_parent,
        stiffness: numbers(heap, field(heap, map, "stiffness"), ":stiffness")?,
        follows: optional_numbers(heap, field(heap, map, "follows"), ":follows")?,
        damping: numbers(heap, field(heap, map, "damping"), ":damping")?,
        holds: numbers(heap, field(heap, map, "holds"), ":holds")?,
        lengths: numbers(heap, field(heap, map, "lengths"), ":lengths")?,
        margins: numbers(heap, field(heap, map, "margins"), ":margins")?,
        limits: numbers(heap, field(heap, map, "limits"), ":limits")?,
        thickness: optional_numbers(heap, field(heap, map, "thickness"), ":thickness")?,
        bounce: bounce_of(heap, field(heap, map, "bounce"))?,
    })
}

/// A lock's `:bounce`, `[b speed root-velocity]` (the velocity nil on the first frame), or nil.
fn bounce_of(heap: &Heap, value: Value) -> Result<Option<crate::host::hair::Bounce>, String> {
    if matches!(value, Value::Nil) {
        return Ok(None);
    }
    let parts = items(heap, value, ":bounce")?;
    match parts.as_slice() {
        [b, speed, velocity] => {
            let velocity = match velocity {
                Value::Nil => None,
                value => match numbers(heap, *value, ":bounce's velocity")?.as_slice() {
                    [x, y, z] => Some([*x, *y, *z]),
                    _ => return Err(":bounce's velocity is not [x y z]".into()),
                },
            };
            Ok(Some((
                number(heap, *b, ":bounce")?,
                number(heap, *speed, ":bounce")?,
                velocity,
            )))
        }
        _ => Err(":bounce is not [b speed velocity]".into()),
    }
}

/// `(%hair-advance locks worlds base twist dt moved-shapes shapes shapes-before constants plan-constants)`
fn hair_advance(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::host::hair::{Frame, PlanConstants, plan};
    let who = "hair/advance";
    let fail = |reason: String| LispError::runtime(format!("{who}: {reason}"));
    let shapes_now = shapes(heap, arg(args, 6), "the shapes").map_err(fail)?;
    let before = match arg(args, 7) {
        Value::Nil => None,
        value => Some(shapes(heap, value, "the shapes before").map_err(fail)?),
    };
    let worlds = items(heap, arg(args, 1), "the worlds")
        .map_err(fail)?
        .into_iter()
        .map(|world| matrix(heap, world, "a world matrix"))
        .collect::<Result<Vec<_>, String>>()
        .map_err(fail)?;
    let base = matrix(heap, arg(args, 2), "the base").map_err(fail)?;
    let twist = match arg(args, 3) {
        Value::Nil => None,
        value => {
            let parts = items(heap, value, "the twist").map_err(fail)?;
            match parts.as_slice() {
                [angle, axis] => {
                    let angle = number(heap, *angle, "the twist's angle").map_err(fail)?;
                    let axis = match numbers(heap, *axis, "the twist's axis")
                        .map_err(fail)?
                        .as_slice()
                    {
                        [x, y, z] => [*x, *y, *z],
                        _ => return Err(fail("the twist's axis is not [x y z]".into())),
                    };
                    Some((angle, axis))
                }
                _ => return Err(fail("the twist is not [angle axis]".into())),
            }
        }
    };
    let dt = number(heap, arg(args, 4), "dt").map_err(fail)?;
    let moved_shapes = !matches!(arg(args, 5), Value::Nil | Value::Bool(false));
    let constants = constants_of(heap, arg(args, 8)).map_err(fail)?;
    let settings = arg(args, 9);
    let setting = |name: &str| number(heap, field(heap, settings, name), name);
    let plan_constants = PlanConstants {
        rate: setting("rate").map_err(fail)?,
        most_steps: setting("most-steps").map_err(fail)? as usize,
        drift: setting("drift").map_err(fail)?,
        clearance: setting("clearance").map_err(fail)?,
        bounce: match field(heap, settings, "bounce") {
            Value::Nil => None,
            value => match numbers(heap, value, ":bounce").map_err(fail)?.as_slice() {
                [gain, hertz, damping] => Some([*gain, *hertz, *damping]),
                _ => return Err(fail(":bounce is not [gain hertz damping]".into())),
            },
        },
        stretch: setting("stretch").map_err(fail)?,
        bounce_most: setting("bounce-most").map_err(fail)?,
    };
    let held = items(heap, arg(args, 0), "the locks")
        .map_err(fail)?
        .into_iter()
        .map(|map| held_of(heap, map, shapes_now.len()))
        .collect::<Result<Vec<_>, String>>()
        .map_err(fail)?;
    let frame = Frame {
        worlds: &worlds,
        base,
        twist,
        dt,
        moved_shapes,
        shapes: &shapes_now,
    };
    let mut out = Vec::with_capacity(held.len());
    for lock in &held {
        if lock
            .joints
            .iter()
            .chain(lock.root_parent.iter())
            .any(|&joint| joint >= worlds.len())
        {
            return Err(fail(format!(
                "a lock names a joint past the {} worlds",
                worlds.len()
            )));
        }
        let planned = plan(lock, &frame, &plan_constants);
        let (points, previous) = moved(
            &planned.lock,
            planned.steps,
            planned.step,
            before.as_deref(),
            &shapes_now,
            &constants,
        );
        let mut chain = |points: &[Vector]| {
            let items: Vec<Value> = points
                .iter()
                .map(|&[x, y, z]| {
                    heap.alloc_vector(vec![Value::float(x), Value::float(y), Value::float(z)])
                })
                .collect();
            heap.alloc_vector(items)
        };
        let points = chain(&points);
        let previous = chain(&previous);
        let now = chain(&planned.lock.now);
        let nears: Vec<Value> = planned
            .lock
            .nears
            .iter()
            .map(|places| {
                let places: Vec<Value> = places
                    .iter()
                    .map(|&place| Value::Int(place as i64))
                    .collect();
                heap.alloc_vector(places)
            })
            .collect();
        let nears = heap.alloc_vector(nears);
        // its curls' stretch, `[b speed root-velocity]`, or nil
        let bounce = match planned.bounce {
            None => Value::Nil,
            Some((b, speed, velocity)) => {
                let velocity = match velocity {
                    None => Value::Nil,
                    Some([x, y, z]) => {
                        heap.alloc_vector(vec![Value::float(x), Value::float(y), Value::float(z)])
                    }
                };
                heap.alloc_vector(vec![Value::float(b), Value::float(speed), velocity])
            }
        };
        out.push(heap.alloc_vector(vec![
            points,
            previous,
            now,
            Value::float(planned.step),
            Value::Bool(planned.fresh),
            nears,
            bounce,
        ]));
    }
    Ok(heap.alloc_vector(out))
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
        past: get("past")?,
        ground: get("ground")?,
        clearance: get("clearance")?,
        passes: get("passes")? as usize,
    })
}

/// `(%hair-linked locks links shapes constants link-constants)`
fn hair_linked(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "hair/linked";
    let fail = |reason: String| LispError::runtime(format!("{who}: {reason}"));
    let shapes_now = shapes(heap, arg(args, 2), "the shapes").map_err(fail)?;
    let constants = constants_of(heap, arg(args, 3)).map_err(fail)?;
    let given = arg(args, 4);
    let get = |name: &str| number(heap, field(heap, given, name), &format!(":{name}"));
    let laid = match numbers(heap, field(heap, given, "laid"), ":laid")
        .map_err(fail)?
        .as_slice()
    {
        [least, most] => [*least, *most],
        _ => return Err(fail(":laid is not [least most]".to_string())),
    };
    let link_constants = LinkConstants {
        together: get("together").map_err(fail)?,
        apart: get("apart").map_err(fail)?,
        laid,
        give: get("give").map_err(fail)?,
    };
    let count = shapes_now.len();
    let locks = items(heap, arg(args, 0), "the locks")
        .map_err(fail)?
        .into_iter()
        .map(|map| {
            let nears = match field(heap, map, "nears") {
                Value::Nil => None,
                value => Some(
                    items(heap, value, ":nears")?
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
                        .collect::<Result<Vec<Vec<usize>>, String>>()?,
                ),
            };
            let lock = Settling {
                points: vectors(heap, field(heap, map, "points"), ":points")?,
                previous: vectors(heap, field(heap, map, "previous"), ":previous")?,
                lengths: numbers(heap, field(heap, map, "lengths"), ":lengths")?,
                margins: numbers(heap, field(heap, map, "margins"), ":margins")?,
                limits: numbers(heap, field(heap, map, "limits"), ":limits")?,
                thickness: optional_numbers(heap, field(heap, map, "thickness"), ":thickness")?,
                nears,
            };
            let points = lock.points.len();
            let bones = points.saturating_sub(1);
            if lock.previous.len() < points
                || lock.margins.len() < points
                || lock.limits.len() < points
                || lock.lengths.len() < bones
                || lock
                    .thickness
                    .as_ref()
                    .is_some_and(|thickness| thickness.len() < points)
                || lock.nears.as_ref().is_some_and(|nears| nears.len() < bones)
            {
                return Err(format!(
                    "a lock of {points} points is short of what it needs"
                ));
            }
            Ok(lock)
        })
        .collect::<Result<Vec<Settling>, String>>()
        .map_err(fail)?;
    let links = items(heap, arg(args, 1), "the links")
        .map_err(fail)?
        .into_iter()
        .map(|each| {
            let parts = items(heap, each, "a link")?;
            let [a, b, rests] = parts.as_slice() else {
                return Err("a link is not [a b rests]".to_string());
            };
            let a = number(heap, *a, "a link's a")? as usize;
            let b = number(heap, *b, "a link's b")? as usize;
            let rests = numbers(heap, *rests, "a link's rests")?;
            if a >= locks.len() || b >= locks.len() {
                return Err(format!("a link names lock {a} or {b} of {}", locks.len()));
            }
            if rests.len() > locks[a].points.len() || rests.len() > locks[b].points.len() {
                return Err("a link's rests are longer than its locks".to_string());
            }
            Ok(Link { a, b, rests })
        })
        .collect::<Result<Vec<Link>, String>>()
        .map_err(fail)?;
    let settled = linked(&locks, &links, &shapes_now, &constants, &link_constants);
    let mut out = Vec::with_capacity(settled.len());
    for each in settled {
        let Some((points, previous)) = each else {
            out.push(Value::Nil);
            continue;
        };
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
