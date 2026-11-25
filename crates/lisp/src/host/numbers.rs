//! The shapes numbers take at a native boundary — a WASM component's
//! `list<float64|float32|s32|u32>` parameter (ADR-806) and a native kernel's input (ADR-807) —
//! and the shape a result comes back in.
//!
//! In: a vector or proper list whose items are numbers, or are themselves vectors or lists of
//! numbers, flattened one level in order; or a map `{:records rs :keys ks}`, each record (a map
//! or a `defrecord` value) in `rs` contributing the value under each key in `ks`, in order, a
//! number or a vector of numbers. Out: the flat floats, or `count` vectors of `size`.
//!
//! These shapes exist because the Brood around a fast crossing was the cost (ADR-810): b3d's
//! 176-joint pose is a vector of `{:translation [3] :rotation [4] :scale [3]}` maps, and
//! gathering it into one list in Brood took ~260 µs and splitting the 2 816 floats back into
//! matrices ~100 µs, against a 28 µs kernel. Both now happen here.

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
/// or list replaced by that collection's items (one level); or, for `{:records … :keys …}`,
/// each record's values under the keys, in order, each flattened the same one level. Whether
/// each item is a number is the caller's check, made in its own element type.
pub(crate) fn flat_items(heap: &Heap, who: &str, v: Value) -> Result<Vec<Value>, LispError> {
    if let Value::Map(id) = v {
        return record_items(heap, who, id);
    }
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
        push_flat(heap, who, item, &mut out)?;
    }
    Ok(out)
}

/// `item` onto `out`: a vector's or list's items, else the item itself.
fn push_flat(heap: &Heap, who: &str, item: Value, out: &mut Vec<Value>) -> Result<(), LispError> {
    match item {
        Value::Vector(_) | Value::Pair(_) => push_items(heap, who, item, out),
        number => {
            out.push(number);
            Ok(())
        }
    }
}

/// `{:records rs :keys ks}`: each record's value under each key, in order, flattened one level.
/// A record missing a key is an error naming the record's position and the key — a kernel fed
/// a short array reads garbage, so the boundary refuses it.
fn record_items(
    heap: &Heap,
    who: &str,
    spec: crate::core::value::MapId,
) -> Result<Vec<Value>, LispError> {
    let field = |name: &str| heap.map_get(spec, Value::Keyword(crate::core::value::intern(name)));
    let (Some(records), Some(keys)) = (field("records"), field("keys")) else {
        return Err(LispError::runtime(format!(
            "{who}: a map argument is {{:records rs :keys ks}} — the records, and the keys whose \
             values each contributes in order"
        )));
    };
    let mut rows = Vec::new();
    push_items(heap, who, records, &mut rows)?;
    let mut names = Vec::new();
    push_items(heap, who, keys, &mut names)?;
    let mut out = Vec::with_capacity(rows.len() * names.len() * 4);
    for (index, row) in rows.iter().enumerate() {
        let fields = match *row {
            Value::Map(id) => id,
            Value::Record { fields, .. } => fields,
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    who,
                    "a map or record in :records",
                    other,
                ));
            }
        };
        for &name in &names {
            let Some(value) = heap.map_get(fields, name) else {
                return Err(LispError::runtime(format!(
                    "{who}: record {index} of :records has no {}",
                    crate::syntax::printer::print(heap, name)
                )));
            };
            push_flat(heap, who, value, &mut out)?;
        }
    }
    Ok(out)
}

/// A result's requested shape: an int `n` (that many floats, flat), or `[count size]` (`count`
/// vectors of `size` floats). The total, and the row size when there is one.
#[cfg(feature = "native-kernels")]
pub(crate) fn out_shape(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<(usize, Option<usize>), LispError> {
    let count = |n: Value| match n {
        Value::Int(n) if n >= 0 => Ok(n as usize),
        other => Err(LispError::wrong_type(
            heap,
            who,
            "a count that is not negative",
            other,
        )),
    };
    match v {
        Value::Vector(id) => match heap.vector(id) {
            &[rows, size] => {
                let (rows, size) = (count(rows)?, count(size)?);
                if size == 0 {
                    return Err(LispError::runtime(format!("{who}: a row size of 0")));
                }
                let total = rows.checked_mul(size).ok_or_else(|| {
                    LispError::runtime(format!("{who}: {rows} rows of {size} overflow"))
                })?;
                Ok((total, Some(size)))
            }
            _ => Err(LispError::wrong_type(
                heap,
                who,
                "an int, or [count size]",
                v,
            )),
        },
        other => Ok((count(other)?, None)),
    }
}

/// `floats` as a Brood value: one flat vector of floats (`size` `None`), or vectors of `size`
/// floats each (`floats.len()` a multiple of it: the caller sized the output so).
#[cfg(feature = "native-kernels")]
pub(crate) fn shaped_floats(heap: &mut Heap, floats: Vec<f64>, size: Option<usize>) -> Value {
    let Some(size) = size else {
        return heap.alloc_vector(floats.into_iter().map(Value::Float).collect());
    };
    let rows: Vec<Vec<Value>> = floats
        .chunks(size)
        .map(|row| row.iter().map(|&f| Value::Float(f)).collect())
        .collect();
    let rows = rows.into_iter().map(|row| heap.alloc_vector(row)).collect();
    heap.alloc_vector(rows)
}

/// A vector result as vectors of `size` items each — `wasm/call-shaped`'s answer. Its length
/// must be a multiple of `size`, or the shape is the caller's mistake and an error.
#[cfg(feature = "wasm")]
pub(crate) fn chunked(
    heap: &mut Heap,
    who: &str,
    v: Value,
    size: usize,
) -> crate::error::LispResult {
    let Value::Vector(id) = v else {
        return Err(LispError::wrong_type(
            heap,
            who,
            "a list result to shape",
            v,
        ));
    };
    let items = heap.vector(id).to_vec();
    if size == 0 || !items.len().is_multiple_of(size) {
        return Err(LispError::runtime(format!(
            "{who}: a result of {} items does not split into rows of {size}",
            items.len()
        )));
    }
    let rows = items
        .chunks(size)
        .map(|row| row.to_vec())
        .collect::<Vec<_>>();
    let rows = rows.into_iter().map(|row| heap.alloc_vector(row)).collect();
    Ok(heap.alloc_vector(rows))
}
