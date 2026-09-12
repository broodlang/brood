//! Natives added for the editor toolkit (`std/editor/*`), registered LAST: registration
//! order feeds the intern table, and a primitive inserted mid-list reshuffles small-map
//! key order image-wide (see `syntax_scan::register`). Two live here:
//!
//! `%ui-harvest` — the per-frame pass under `editor/ui`'s memoised view fragments
//! (ADR-336). A `view` marks a fragment it memoised as `[:ui/memo key deps ops]` inside
//! the frame; the loop needs the frame WITHOUT the markers (what the frontend paints)
//! and a table `{key -> [deps ops]}` of them (what the next turn's `ui-memo` reads).
//! One walk of the frame produces both. It is the same shape as `%span-runs`: plain
//! data in, plain data out, and it runs on every frame — where a Brood-level walk of a
//! few hundred ops costs about a millisecond, more than the paint it exists to save.

use super::numeric::arg;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value, ValueRef};
use crate::error::{LispError, LispResult};

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    primitives.def(
        "%ui-harvest",
        Arity::exact(1),
        Sig::new(vec![any], vec_ty),
        &["frame"],
        "[frame' table]: frame (a vector or list of render ops) with every [:ui/memo key deps ops] marker replaced by its ops, and a map key -> [deps ops] of the markers. Recurses into a marker's ops and into a [:scroll-region frac ops] vector (its ops come back as a vector, as they went in). The pass behind editor/ui's memoised view fragments (ADR-336), native because it runs once per frame over every op.",
        ui_harvest,
    );
    // The text contrast exponent (`gui-text-contrast!`, ADR-337): partial glyph coverage
    // lifted where text is lighter than its ground — light-on-dark text under a
    // linear-light blend reads thin, and this is the knob every such renderer grows.
    primitives.def(
        "%gui-text-contrast!",
        Arity::exact(1),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Float])], nil_ty),
        &["gamma"],
        "Set the text contrast exponent γ (1.0 by default, clamped to 0.5..3.0): a monochrome glyph's partial coverage is lifted to cov^(1/γ) where the text is lighter than the pixel it lands on, so light-on-dark text — which a linear-light blend renders with thin stems — reads fuller without touching a glyph's interior, its exterior, dark-on-light text or colour emoji. 1.0 is the plain blend; 1.4–1.8 is the range other linear-light renderers ship. Applies to every open window and the default for ones opened later; a pure repaint. Needs --features gui. Returns nil.",
        gui_text_contrast,
    );
}

/// The ops of `seq` (a vector or list), or `None` for anything else.
fn ops_of(heap: &Heap, seq: Value) -> Option<Vec<Value>> {
    match seq.unpack() {
        ValueRef::Vector(id) => Some(heap.vector(id).to_vec()),
        ValueRef::Nil | ValueRef::Pair(_) => heap.list_to_vec(seq).ok(),
        _ => None,
    }
}

struct Tags {
    memo: value::Symbol,
    scroll_region: value::Symbol,
}

/// Walk `ops`, appending the flattened ops to `out` and every marker to `table`.
fn harvest_into(
    heap: &mut Heap,
    ops: Vec<Value>,
    tags: &Tags,
    out: &mut Vec<Value>,
    table: &mut Vec<(Value, Value)>,
) {
    for op in ops {
        let ValueRef::Vector(id) = op.unpack() else {
            out.push(op);
            continue;
        };
        let parts = heap.vector(id).to_vec();
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => {
                out.push(op);
                continue;
            }
        };
        if tag == tags.memo && parts.len() == 4 {
            let (key, deps, inner) = (parts[1], parts[2], parts[3]);
            let Some(inner_ops) = ops_of(heap, inner) else {
                continue; // a marker whose ops are not a sequence paints nothing
            };
            let mut flat = Vec::with_capacity(inner_ops.len());
            harvest_into(heap, inner_ops, tags, &mut flat, table);
            let flat_list = heap.list(flat.clone());
            let entry = heap.alloc_vector2(deps, flat_list);
            table.push((key, entry));
            out.extend(flat);
        } else if tag == tags.scroll_region && parts.len() == 3 {
            let (frac, inner) = (parts[1], parts[2]);
            let Some(inner_ops) = ops_of(heap, inner) else {
                out.push(op);
                continue;
            };
            let mut flat = Vec::with_capacity(inner_ops.len());
            harvest_into(heap, inner_ops, tags, &mut flat, table);
            let flat_vec = heap.alloc_vector(flat);
            let tag_v = Value::keyword(tags.scroll_region);
            let region = heap.alloc_vector(vec![tag_v, frac, flat_vec]);
            out.push(region);
        } else {
            out.push(op);
        }
    }
}

fn ui_harvest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let frame = arg(args, 0);
    let Some(ops) = ops_of(heap, frame) else {
        return Err(LispError::wrong_type(
            heap,
            "%ui-harvest",
            "vector or list (a frame)",
            frame,
        ));
    };
    let tags = Tags {
        memo: value::intern("ui/memo"),
        scroll_region: value::intern("scroll-region"),
    };
    let mut out = Vec::with_capacity(ops.len());
    let mut table = Vec::new();
    harvest_into(heap, ops, &tags, &mut out, &mut table);
    let flat = heap.alloc_vector(out);
    let map = heap.map_from_pairs(table);
    Ok(heap.alloc_vector2(flat, map))
}

/// `(%gui-text-contrast! gamma)` — see the registration. GUI only; returns nil.
fn gui_text_contrast(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let gamma = match arg(args, 0) {
        Value::Int(n) => n as f32,
        Value::Float(f) => f as f32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-text-contrast!",
                "a number (the contrast exponent, 1.0 = plain)",
                other,
            ))
        }
    };
    crate::host::gui::text_contrast(gamma).map_err(LispError::runtime)?;
    Ok(Value::nil())
}
