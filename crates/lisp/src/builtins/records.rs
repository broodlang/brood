use super::args::{arg, expect_keyword};
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

/// Every primitive this file contributes: the kernel half of a record (ADR-542). A record is
/// its own `Value` kind, so Brood cannot construct or take one apart without a primitive —
/// the language has no way to make a new tag. `defrecord`, `record?`, `record-id` and
/// `fields` in the prelude are the policy over these.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::Sig;
    let record_ty = crate::types::Ty::of(Tag::Record);
    primitives.def(
        "%record",
        Arity::exact(2),
        Sig::new(vec![kw, map_ty.union(nil_ty)], record_ty.clone()),
        &["id", "fields"],
        "A record of identity id (a keyword, `:point` or `:geo/point`) over the field map fields. The primitive behind a `defrecord` constructor.",
        record_new,
    );
    primitives.def(
        "%record?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "Whether x is a record.",
        record_p,
    );
    primitives.def(
        "%record-id",
        Arity::exact(1),
        Sig::new(vec![any], kw.union(nil_ty)),
        &["x"],
        "The identity of record x as a keyword, else nil.",
        record_id,
    );
    primitives.def(
        "%record-fields",
        Arity::exact(1),
        Sig::new(vec![any], map_ty.union(nil_ty)),
        &["x"],
        "The fields of record x as a plain map, else nil.",
        record_fields,
    );
}

/// `(%record id fields)`: a record over `fields` (a map; `nil` is the empty one).
pub(super) fn record_new(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_keyword(heap, "%record", arg(args, 0))?;
    let fields = match arg(args, 1) {
        Value::Map(fields) => fields,
        Value::Nil => match heap.map_from_pairs(Vec::new()) {
            Value::Map(fields) => fields,
            _ => {
                return Err(LispError::runtime(
                    "%record: could not build an empty field map",
                ));
            }
        },
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%record",
                "map of fields",
                other,
            ));
        }
    };
    Ok(Value::record(id, fields))
}

/// `(%record? x)`.
pub(super) fn record_p(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Record { .. })))
}

/// `(%record-id x)`: the identity as a keyword, or `nil` for anything else.
pub(super) fn record_id(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(match arg(args, 0) {
        Value::Record { id, .. } => Value::Keyword(id),
        _ => Value::nil(),
    })
}

/// `(%record-fields x)`: the field map, or `nil` for anything else.
pub(super) fn record_fields(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(match arg(args, 0) {
        Value::Record { fields, .. } => Value::map(fields),
        _ => Value::nil(),
    })
}
