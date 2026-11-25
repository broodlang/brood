//! The one shape a numeric array argument takes at a native boundary — a WASM component's
//! `list<float64|float32|s32|u32>` parameter (ADR-806) and a native kernel's input (ADR-807):
//! a vector or proper list whose items are numbers, or are themselves vectors or lists of
//! numbers, flattened one level in order.
//!
//! The nesting is there because packing is the cost a fast crossing cannot remove: b3d's
//! 176-joint pose, built into one flat float list in Brood a value at a time, took ~510 µs,
//! more than the whole Brood pass it was meant to replace. Its matrices go in as they are, a
//! vector of sixteen-float vectors, and are copied out here.

use crate::core::heap::Heap;
use crate::core::value::Value;
use crate::error::LispError;

/// The items of `v`, a vector or proper list, appended to `out`.
fn push_items(heap: &Heap, who: &str, v: Value, out: &mut Vec<Value>) -> Result<(), LispError> {
    match v {
        Value::Nil => Ok(()),
        Value::Vector(id) => {
            out.extend_from_slice(heap.vector(id));
            Ok(())
        }
        Value::Pair(_) => {
            let mut cur = v;
            loop {
                match cur {
                    Value::Pair(id) => {
                        let cell = heap.pair(id);
                        out.push(cell.0);
                        cur = cell.1;
                    }
                    Value::Nil => return Ok(()),
                    other => {
                        return Err(LispError::wrong_type(heap, who, "proper list", other));
                    }
                }
            }
        }
        other => Err(LispError::wrong_type(heap, who, "vector or list", other)),
    }
}

/// The items of a numeric array argument `v`: its own items, with each item that is a vector
/// or list replaced by that collection's items (one level). Whether each item is a number is
/// the caller's check, made in its own element type.
pub(crate) fn flat_items(heap: &Heap, who: &str, v: Value) -> Result<Vec<Value>, LispError> {
    let mut top = Vec::new();
    push_items(heap, who, v, &mut top)?;
    if !top
        .iter()
        .any(|item| matches!(item, Value::Vector(_) | Value::Pair(_)))
    {
        return Ok(top);
    }
    let mut out = Vec::with_capacity(top.len() * 4);
    for item in top {
        match item {
            Value::Vector(_) | Value::Pair(_) => push_items(heap, who, item, &mut out)?,
            number => out.push(number),
        }
    }
    Ok(out)
}
