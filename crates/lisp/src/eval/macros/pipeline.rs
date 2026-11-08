//! Pipeline fusion and the counted range loop (ADR-360 §7): compile-time rewrites of
//! a range or vector pipeline into one loop.

use super::*;

// ===================== pipeline fusion + the counted range loop (ADR-360 §7) ==============
//
// `(-> (range n) (seq/lfilter p) (seq/lmap f) (reduce 0 +))` is what the pocket reference and
// the benchmarks write, and unrewritten it runs a transducer closure per stage, composed at
// runtime, called per element through a Rust→native gateway, each stage a fast-linked call
// into the next — two orders of magnitude slower than the same work in a `defn` loop. The
// two rewrites below take the shape the compiler can prove and turn it into that loop:
//
//   1. FUSION. A chain of `seq/lmap` / `seq/lfilter` / `seq/lreject` / `seq/lkeep` stages
//      under a `fold`/`reduce` becomes ONE literal over the base collection: each stage's
//      function is bound once (in the source's evaluation order) and called per element, or
//      — when it is a literal `(fn (p) E)` — substituted in place with `p` renamed to a fresh
//      gensym (`beta_reduce`), so no free variable of a later stage can be captured by an
//      earlier stage's parameter. A literal that rebinds its own parameter inside, or
//      quotes anything, is not substituted (it is bound and called instead).
//   2. THE COUNTED LOOP. `(fold (range …) INIT (fn (acc x) BODY…))` becomes a `letrec` loop
//      over the range's bounds (read once through `%range-bounds`), guarded by `range?` so
//      an empty range (nil) and any other value take the ordinary `fold`. A letrec loop is
//      a `SelfCall` loop the JIT runs native (KI-156) — the fold's per-element gateway is
//      gone, and the literal's body is the loop body.
//
// Both keep every evaluation in source order and evaluate each sub-form once; both decline
// the whole form when a local binder anywhere in it shadows a name they read.

/// The names the pipeline rewrites read by spelling.
const PIPELINE_HEADS: [&str; 21] = [
    "fold",
    "reduce",
    "range",
    "range?",
    "nth",
    ">=",
    "<=",
    ">",
    "+",
    "nil?",
    "seq/lmap",
    "seq/lfilter",
    "seq/lreject",
    "seq/lkeep",
    "vector?",
    "pair?",
    "count",
    "empty?",
    "first",
    "rest",
    "failure?",
];

/// [`PIPELINE_HEADS`] interned, once per process (symbols are process-global).
fn pipeline_names() -> &'static [value::Symbol] {
    static NAMES: std::sync::LazyLock<Vec<value::Symbol>> =
        std::sync::LazyLock::new(|| PIPELINE_HEADS.iter().map(|n| value::intern(n)).collect());
    &NAMES
}

/// Is `form` a call `(head …)` of one of `names` with exactly `argc` arguments? Returns the
/// items.
fn call_of(heap: &Heap, form: Value, names: &[&str], argc: usize) -> Option<Vec<Value>> {
    let items = heap.list_items(form)?;
    let ValueRef::Sym(h) = items.first()?.unpack() else {
        return None;
    };
    (names.iter().any(|n| value::symbol_is(h, n)) && items.len() == argc + 1).then_some(items)
}

/// A single-clause literal `(fn (p…) body…)` with plain params, no docstring and no
/// quasiquote: `(params, body forms)`.
fn plain_fn_literal(heap: &Heap, form: Value) -> Option<(Vec<value::Symbol>, Vec<Value>)> {
    let items = heap.list_items(form)?;
    if items.len() < 3
        || !matches!(items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::FN))
        || fn_is_arity_multi_clause(heap, &items)
    {
        return None;
    }
    let mut params = Vec::new();
    for &pv in &heap.list_or_vector_items(items[1])? {
        match pv.unpack() {
            ValueRef::Sym(s)
                if !value::symbol_is(s, kw::AMP) && !value::symbol_is(s, kw::AMP_OPTIONAL) =>
            {
                params.push(s)
            }
            _ => return None,
        }
    }
    let body: Vec<Value> = items[2..].to_vec();
    if body.len() > 1 && matches!(body[0].unpack(), ValueRef::Str(_)) {
        return None;
    }
    if body.iter().any(|&f| form_has_quasiquote(heap, f, 0)) {
        return None;
    }
    Some((params, body))
}

/// `form` with every free occurrence of `from` replaced by `to`. Only sound when nothing in
/// `form` rebinds `from` (the caller checks with `form_binds_any`) and nothing is quoted
/// (`quote` is skipped here; the caller has already declined a quasiquote).
fn substitute_symbol(heap: &mut Heap, form: Value, from: value::Symbol, to: Value) -> Value {
    match form.unpack() {
        ValueRef::Sym(s) if s == from => to,
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(form) else {
                return form;
            };
            if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack())
                && value::symbol_is(h, kw::QUOTE)
            {
                return form;
            }
            let out: Vec<Value> = items
                .iter()
                .map(|&it| substitute_symbol(heap, it, from, to))
                .collect();
            rebuild_list(heap, form, out)
        }
        // A literal evaluates its elements, so a parameter inside one is a use: `(fn (x) [x])`.
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out: Vec<Value> = items
                .into_iter()
                .map(|it| substitute_symbol(heap, it, from, to))
                .collect();
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let out: Vec<(Value, Value)> = entries
                .into_iter()
                .map(|(k, v)| {
                    (
                        substitute_symbol(heap, k, from, to),
                        substitute_symbol(heap, v, from, to),
                    )
                })
                .collect();
            heap.map_from_pairs(out)
        }
        ValueRef::Set(id) => {
            let elements = heap.set_elems(id);
            let out: Vec<Value> = elements
                .into_iter()
                .map(|it| substitute_symbol(heap, it, from, to))
                .collect();
            heap.set_from_elems(out)
        }
        _ => form,
    }
}

/// Beta-reduce a plain literal `(fn (p…) body…)` applied to `args` (fresh gensyms, so no
/// capture is possible): the body with each `p` replaced. `None` when the literal is not
/// plain, its arity differs, or its body rebinds a parameter — the caller then binds the
/// literal and calls it instead.
fn beta_reduce(heap: &mut Heap, fn_form: Value, args: &[Value]) -> Option<Value> {
    let (params, body) = plain_fn_literal(heap, fn_form)?;
    if params.len() != args.len() {
        return None;
    }
    if body
        .iter()
        .any(|&f| form_binds_any(heap, f, &params, 0) || form_has_quote(heap, f, 0))
    {
        return None;
    }
    let mut out = if body.len() == 1 {
        body[0]
    } else {
        let mut d = vec![value::sym(kw::DO)];
        d.extend(body);
        heap.list(d)
    };
    for (&p, &a) in params.iter().zip(args) {
        out = substitute_symbol(heap, out, p, a);
    }
    Some(out)
}

/// Does `form` contain a `(quote …)` anywhere? A substitution must not reach into data.
fn form_has_quote(heap: &Heap, form: Value, depth: usize) -> bool {
    if depth > 256 {
        return true;
    }
    let Some(items) = heap.list_items(form) else {
        return literal_items(heap, form)
            .is_some_and(|xs| xs.iter().any(|&x| form_has_quote(heap, x, depth + 1)));
    };
    if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack())
        && (value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE))
    {
        return true;
    }
    items.iter().any(|&it| form_has_quote(heap, it, depth + 1))
}

/// One `seq/l*` stage, in data-flow order.
enum Stage {
    Map(Value),
    Filter(Value),
    Reject(Value),
    Keep(Value),
}

/// Peel `(seq/lmap (seq/lfilter C P) F)` into `(C, [Filter(P), Map(F)])`.
fn peel_stages(heap: &Heap, form: Value) -> Option<(Value, Vec<Stage>)> {
    let mut stages = Vec::new();
    let mut cur = form;
    loop {
        let Some(items) = call_of(
            heap,
            cur,
            &["seq/lmap", "seq/lfilter", "seq/lreject", "seq/lkeep"],
            2,
        ) else {
            break;
        };
        let ValueRef::Sym(h) = items[0].unpack() else {
            break;
        };
        let f = items[2];
        stages.push(if value::symbol_is(h, "seq/lmap") {
            Stage::Map(f)
        } else if value::symbol_is(h, "seq/lfilter") {
            Stage::Filter(f)
        } else if value::symbol_is(h, "seq/lreject") {
            Stage::Reject(f)
        } else {
            Stage::Keep(f)
        });
        cur = items[1];
    }
    if stages.is_empty() {
        return None;
    }
    stages.reverse(); // innermost first = data order
    Some((cur, stages))
}

/// Is there a combination anywhere in `form`, outside quoted data, whose head symbol and
/// items satisfy `is_target`? The shared cheap pre-check of the fold rewrites. A form too
/// deep to walk answers no, which leaves it unrewritten.
pub(super) fn form_has_call(
    heap: &Heap,
    form: Value,
    depth: usize,
    is_target: &dyn Fn(value::Symbol, &[Value]) -> bool,
) -> bool {
    if depth > 256 {
        return false;
    }
    let Some(items) = heap.list_items(form) else {
        return false;
    };
    if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
        if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) {
            return false;
        }
        if is_target(h, &items) {
            return true;
        }
    }
    items
        .iter()
        .any(|&it| form_has_call(heap, it, depth + 1, is_target))
}

/// Is there a four-item `fold`/`reduce` call anywhere in `form`, outside quoted data? Every
/// source rewrite fires only under one — each pre-check below asks this and more — so a form
/// without one is answered for all three by a single walk.
pub(super) fn form_has_fold_call(heap: &Heap, form: Value) -> bool {
    form_has_call(heap, form, 0, &|head, items| {
        is_fold_head(head) && items.len() == 4
    })
}

/// Does `form` hold a stage chain under a `fold`/`reduce`? The cheap pre-check.
fn form_has_pipeline(heap: &Heap, form: Value) -> bool {
    form_has_call(heap, form, 0, &|head, items| {
        is_fold_head(head) && items.len() == 4 && peel_stages(heap, items[1]).is_some()
    })
}

/// Rewrite every `(fold|reduce <stage chain over C> INIT RF)` in `form` into
/// `(let (c C s1 S1 … init INIT rf RF) (fold c init (fn (acc e0) BODY)))` — see the section
/// comment. Declined for the whole form when a binder in it shadows a name read here.
pub(super) fn pipeline_fuse_rewrite(heap: &mut Heap, form: Value) -> Value {
    if !crate::diagnostics::debug_flags::linmap() {
        return form;
    }
    if !form_has_pipeline(heap, form) || form_binds_any(heap, form, pipeline_names(), 0) {
        return form;
    }
    pipeline_walk(heap, form, 0, &|heap, form, items| {
        pipeline_fuse_call(heap, form, items)
    })
}

/// The shared bottom-up walk of the pipeline and linmap fusions: children first (so nested
/// shapes are handled on their own), `quote`d data and `:generated` code left alone (a
/// rewritten fold's slow copy is generated), then `at` on the rebuilt call.
pub(super) fn pipeline_walk(
    heap: &mut Heap,
    form: Value,
    depth: usize,
    at: &dyn Fn(&mut Heap, Value, &[Value]) -> Option<Value>,
) -> Value {
    if depth > 256 {
        return form;
    }
    let items = match heap.list_items(form) {
        Some(v) if !v.is_empty() => v,
        _ => return form,
    };
    if let ValueRef::Sym(h) = items[0].unpack() {
        if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) {
            return form;
        }
        if value::symbol_is(h, "%lint-allow")
            && matches!(items.get(1), Some(&Value::Keyword(k)) if value::symbol_is(k, "generated"))
        {
            return form;
        }
    }
    let rebuilt: Vec<Value> = items
        .iter()
        .map(|&it| pipeline_walk(heap, it, depth + 1, at))
        .collect();
    let form = rebuild_list(heap, form, rebuilt.clone());
    at(heap, form, &rebuilt).unwrap_or(form)
}

fn pipeline_fuse_call(heap: &mut Heap, form: Value, items: &[Value]) -> Option<Value> {
    let ValueRef::Sym(h) = items[0].unpack() else {
        return None;
    };
    if !is_fold_head(h) || items.len() != 4 {
        return None;
    }
    let (base, stages) = peel_stages(heap, items[1])?;
    let init = items[2];
    let rf = items[3];
    // The element flowing through: a map/keep stage binds a new one, a filter/reject stage
    // passes the same one on.
    let acc = value::gensym("pipe-acc");
    let mut elems: Vec<Value> = Vec::with_capacity(stages.len() + 1);
    elems.push(value::gensym("pipe-e"));
    for st in &stages {
        let next = match st {
            Stage::Map(_) | Stage::Keep(_) => value::gensym("pipe-e"),
            Stage::Filter(_) | Stage::Reject(_) => *elems.last().expect("e0"),
        };
        elems.push(next);
    }
    // How each stage's function (and the reducer) enters the fused body — decided in the
    // source's evaluation order (base, each stage outward, init, reducer), binding once
    // what has to be evaluated:
    //   - a plain literal `fn` that rebinds none of its parameters and quotes nothing is
    //     SUBSTITUTED, its parameters renamed to the element gensyms (`beta_reduce`) — so no
    //     free variable of a later stage can be captured by an earlier stage's parameter;
    //   - a plain SYMBOL is called by that name (a global head keeps the inline-cache path
    //     and leaf splicing; a local is immutable, so it is the same value) — the one
    //     observable difference is a global redefined DURING the fold, which the source
    //     would not see and this does;
    //   - anything else is bound to a gensym and called.
    let mut binds: Vec<Value> = Vec::new();
    let c_val = value::gensym("pipe-coll");
    binds.push(c_val);
    binds.push(base);
    let mut uses: Vec<Value> = Vec::with_capacity(stages.len()); // the applied form per stage
    for (k, st) in stages.iter().enumerate() {
        let f = match st {
            Stage::Map(f) | Stage::Filter(f) | Stage::Reject(f) | Stage::Keep(f) => *f,
        };
        let e_in = elems[k];
        let applied = if let Some(b) = beta_reduce(heap, f, &[e_in]) {
            b
        } else if matches!(f.unpack(), ValueRef::Sym(_)) {
            heap.list(vec![f, e_in])
        } else {
            let v = value::gensym("pipe-stage");
            binds.push(v);
            binds.push(f);
            heap.list(vec![v, e_in])
        };
        uses.push(applied);
    }
    let init_val = value::gensym("pipe-init");
    binds.push(init_val);
    binds.push(init);
    let last = elems[stages.len()];
    let mut body = if let Some(b) = beta_reduce(heap, rf, &[acc, last]) {
        b
    } else if matches!(rf.unpack(), ValueRef::Sym(_)) {
        heap.list(vec![rf, acc, last])
    } else {
        let v = value::gensym("pipe-rf");
        binds.push(v);
        binds.push(rf);
        heap.list(vec![v, acc, last])
    };
    // The fused body, built inside-out: the reducer step on the final element, then each
    // stage wrapped around it from the last to the first.
    for (k, st) in stages.iter().enumerate().rev() {
        let e_out = elems[k + 1];
        let applied = uses[k];
        body = match st {
            Stage::Map(_) => {
                let bind = heap.list(vec![e_out, applied]);
                heap.list(vec![value::sym(kw::LET), bind, body])
            }
            Stage::Filter(_) => heap.list(vec![value::sym(kw::IF), applied, body, acc]),
            Stage::Reject(_) => heap.list(vec![value::sym(kw::IF), applied, acc, body]),
            Stage::Keep(_) => {
                let bind = heap.list(vec![e_out, applied]);
                let is_nil = heap.list(vec![value::sym("/nil?"), e_out]);
                let test = heap.list(vec![value::sym(kw::IF), is_nil, acc, body]);
                heap.list(vec![value::sym(kw::LET), bind, test])
            }
        };
    }
    let params = heap.list(vec![acc, elems[0]]);
    let lambda = heap.list(vec![value::sym(kw::FN), params, body]);
    // A range base takes the counted loop straight away (the base is already bound, so
    // `range_fold_rewrite` would not see the `(range …)` call).
    let ValueRef::Sym(acc_sym) = acc.unpack() else {
        return None;
    };
    let ValueRef::Sym(e0_sym) = elems[0].unpack() else {
        return None;
    };
    let call = if form_atoms(heap, body, 0) <= FOLD_LOOP_MAX_ATOMS {
        coll_or_fold(
            heap,
            form,
            c_val,
            init_val,
            lambda,
            acc_sym,
            e0_sym,
            &[body],
        )
    } else {
        rebuild_list(
            heap,
            form,
            vec![value::sym("fold"), c_val, init_val, lambda],
        )
    };
    let binds = heap.list(binds);
    Some(rebuild_list(
        heap,
        form,
        vec![value::sym(kw::LET), binds, call],
    ))
}

/// A head the counted loop replaces: `fold`, or `reduce`, whose three-argument form
/// `(reduce COLL INIT f)` is `(fold COLL INIT f)`. (Its two-argument form never has the
/// four items the callers require.)
pub(super) fn is_fold_head(h: value::Symbol) -> bool {
    value::symbol_is(h, "fold") || value::symbol_is(h, "reduce")
}

/// Rewrite every `(fold (range …) INIT (fn (acc x) BODY…))` in `form` — or the same
/// `reduce` — into a counted `letrec` loop over the range's bounds, with the ordinary
/// `fold` kept for a non-range (an empty range is nil). See the section comment.
pub(super) fn range_fold_rewrite(heap: &mut Heap, form: Value) -> Value {
    if !crate::diagnostics::debug_flags::linmap() {
        return form;
    }
    if !form_has_range_fold(heap, form) || form_binds_any(heap, form, pipeline_names(), 0) {
        return form;
    }
    pipeline_walk(heap, form, 0, &|heap, form, items| {
        range_fold_call(heap, form, items)
    })
}

fn form_has_range_fold(heap: &Heap, form: Value) -> bool {
    form_has_call(heap, form, 0, &|head, items| {
        is_fold_head(head)
            && items.len() == 4
            && plain_fn_literal(heap, items[3]).is_some_and(|(p, _)| p.len() == 2)
    })
}

/// The counted `letrec` loop over the range bound to `r`, folding `body` (which reads the
/// literal's own parameter names `acc_name`/`elem_name`) from `init`:
///
/// ```text
/// (let (B (%range-bounds r) LO (nth B 0) HI (nth B 1) STEP (nth B 2))
///   (letrec (L (fn (I A) (if (failure? A)
///                          A
///                          (if (if (> STEP 0) (>= I HI) (<= I HI))
///                            A
///                            (L (+ I STEP) (let (acc_name A elem_name I) body…))))))
///     (L LO init)))
/// ```
///
/// `%range-fold-span`'s loop, in Brood: `i < hi` / `i > hi` by the step's sign, `i + step` per
/// iteration (a `+` past i64 promotes and the compare then stops the loop, as the Rust
/// loop's checked add did). A `letrec` loop is a `SelfCall` loop the JIT runs native.
fn counted_range_loop(
    heap: &mut Heap,
    r: Value,
    init: Value,
    acc_name: value::Symbol,
    elem_name: value::Symbol,
    body: &[Value],
) -> Value {
    let bounds = value::gensym("rng-bounds");
    let lo = value::gensym("rng-lo");
    let hi = value::gensym("rng-hi");
    let step = value::gensym("rng-step");
    let loop_name = value::gensym("rng-loop");
    let i = value::gensym("rng-i");
    let acc = value::gensym("rng-acc");
    let up = heap.list(vec![value::sym("/>"), step, Value::int(0)]);
    let done_up = heap.list(vec![value::sym("/>="), i, hi]);
    let done_down = heap.list(vec![value::sym("/<="), i, hi]);
    let done = heap.list(vec![value::sym(kw::IF), up, done_up, done_down]);
    let next_i = heap.list(vec![value::sym("/+"), i, step]);
    let rebind = heap.list(vec![
        Value::symbol(acc_name),
        acc,
        Value::symbol(elem_name),
        i,
    ]);
    let mut step_body = vec![value::sym(kw::LET), rebind];
    // A copy of the literal's body: the fallback `fold` keeps the original (`copy_form`).
    for &form in body {
        let copied = copy_form(heap, form);
        step_body.push(copied);
    }
    let step_form = heap.list(step_body);
    let recur = heap.list(vec![loop_name, next_i, step_form]);
    let branch = stop_on_failure(heap, acc, done, recur);
    let loop_params = heap.list(vec![i, acc]);
    let loop_fn = heap.list(vec![value::sym(kw::FN), loop_params, branch]);
    let loop_binds = heap.list(vec![loop_name, loop_fn]);
    let start = heap.list(vec![loop_name, lo, init]);
    let letrec = heap.list(vec![value::sym(kw::LETREC), loop_binds, start]);
    let get_bounds = heap.list(vec![value::sym("%range-bounds"), r]);
    let nth0 = heap.list(vec![value::sym("/nth"), bounds, Value::int(0)]);
    let nth1 = heap.list(vec![value::sym("/nth"), bounds, Value::int(1)]);
    let nth2 = heap.list(vec![value::sym("/nth"), bounds, Value::int(2)]);
    let b_binds = heap.list(vec![bounds, get_bounds, lo, nth0, hi, nth1, step, nth2]);
    heap.list(vec![value::sym(kw::LET), b_binds, letrec])
}

/// `(if (failure? acc) acc (if done acc recur))`: each loop's step, which stops
/// once the accumulator is a failure (ADR-315), as `fold`'s own walks do.
fn stop_on_failure(heap: &mut Heap, acc: Value, done: Value, recur: Value) -> Value {
    // `failure?`, not `(= (type-of acc) :failure)`: the predicate lowers to one tag test
    // (`PrimOp1::TypeIs`), where the spelled-out compare reads the type as a keyword and
    // compares it — +2-4% on `nqueens`, whose `reduce` over a range is this loop.
    let failed = heap.list(vec![value::sym("/failure?"), acc]);
    let inner = heap.list(vec![value::sym(kw::IF), done, acc, recur]);
    heap.list(vec![value::sym(kw::IF), failed, acc, inner])
}

/// The counted loop over the vector bound to `r`: `(nth r i)` per element, `count` once.
fn counted_vector_loop(
    heap: &mut Heap,
    r: Value,
    init: Value,
    acc_name: value::Symbol,
    elem_name: value::Symbol,
    body: &[Value],
) -> Value {
    let n = value::gensym("vec-n");
    let loop_name = value::gensym("vec-loop");
    let i = value::gensym("vec-i");
    let acc = value::gensym("vec-acc");
    let done = heap.list(vec![value::sym("/>="), i, n]);
    let next_i = heap.list(vec![value::sym("/+"), i, Value::int(1)]);
    let elem = heap.list(vec![value::sym("/nth"), r, i]);
    let rebind = heap.list(vec![
        Value::symbol(acc_name),
        acc,
        Value::symbol(elem_name),
        elem,
    ]);
    let mut step_body = vec![value::sym(kw::LET), rebind];
    // A copy of the literal's body: the fallback `fold` keeps the original (`copy_form`).
    for &form in body {
        let copied = copy_form(heap, form);
        step_body.push(copied);
    }
    let step_form = heap.list(step_body);
    let recur = heap.list(vec![loop_name, next_i, step_form]);
    let branch = stop_on_failure(heap, acc, done, recur);
    let loop_params = heap.list(vec![i, acc]);
    let loop_fn = heap.list(vec![value::sym(kw::FN), loop_params, branch]);
    let loop_binds = heap.list(vec![loop_name, loop_fn]);
    let start = heap.list(vec![loop_name, Value::int(0), init]);
    let letrec = heap.list(vec![value::sym(kw::LETREC), loop_binds, start]);
    let count = heap.list(vec![value::sym("/count"), r]);
    let n_binds = heap.list(vec![n, count]);
    heap.list(vec![value::sym(kw::LET), n_binds, letrec])
}

/// The walk over the list bound to `r` — `%fold-loop` in place: a failure accumulator
/// stops the fold before the emptiness test (ADR-315), `first`/`rest` per element.
fn list_loop(
    heap: &mut Heap,
    r: Value,
    init: Value,
    acc_name: value::Symbol,
    elem_name: value::Symbol,
    body: &[Value],
) -> Value {
    let loop_name = value::gensym("list-loop");
    let xs = value::gensym("list-xs");
    let acc = value::gensym("list-acc");
    let done = heap.list(vec![value::sym("/empty?"), xs]);
    let next_xs = heap.list(vec![value::sym("/rest"), xs]);
    let elem = heap.list(vec![value::sym("/first"), xs]);
    let rebind = heap.list(vec![
        Value::symbol(acc_name),
        acc,
        Value::symbol(elem_name),
        elem,
    ]);
    let mut step_body = vec![value::sym(kw::LET), rebind];
    // A copy of the literal's body: the fallback `fold` keeps the original (`copy_form`).
    for &form in body {
        let copied = copy_form(heap, form);
        step_body.push(copied);
    }
    let step_form = heap.list(step_body);
    let recur = heap.list(vec![loop_name, next_xs, step_form]);
    let branch = stop_on_failure(heap, acc, done, recur);
    let loop_params = heap.list(vec![xs, acc]);
    let loop_fn = heap.list(vec![value::sym(kw::FN), loop_params, branch]);
    let loop_binds = heap.list(vec![loop_name, loop_fn]);
    let start = heap.list(vec![loop_name, r, init]);
    heap.list(vec![value::sym(kw::LETREC), loop_binds, start])
}

/// `(if (range? r) <range loop> (if (vector? r) <vector loop> (if (pair? r) <list loop>
/// (%lint-allow :generated (fold r init lambda)))))` — the three collection kinds `fold`
/// dispatches to a Rust loop with a per-element gateway, each as a `letrec` loop with the
/// literal's body in place; everything else (nil, a map, a set, a view, a string) takes
/// the ordinary `fold` with the literal as written.
#[allow(clippy::too_many_arguments)]
fn coll_or_fold(
    heap: &mut Heap,
    form: Value,
    r: Value,
    init: Value,
    lambda: Value,
    acc_name: value::Symbol,
    elem_name: value::Symbol,
    body: &[Value],
) -> Value {
    let ranged = counted_range_loop(heap, r, init, acc_name, elem_name, body);
    let slow = rebuild_list(heap, form, vec![value::sym("fold"), r, init, lambda]);
    let slow = heap.list(vec![
        value::sym("%lint-allow"),
        value::kw("generated"),
        slow,
    ]);
    // The vector and list loops are PARKED (`FOLD_LOOP_SMALL_ATOMS` is `None`): each is one more
    // copy of the body, and a copy costs ~1–3M instructions wherever the fold is compiled
    // (a closure template promoted, an arm compiled, the JIT's share) — including the std
    // functions a program materialises at boot, where a fold of a few elements pays the
    // copies and never the loop — the boot-time cost outweighed the loop win on a vector or
    // list fold. Set the constant to bring them back once a copy is cheaper.
    let atoms = body.iter().map(|&f| form_atoms(heap, f, 0)).sum::<usize>();
    let want_more = FOLD_LOOP_SMALL_ATOMS.is_some_and(|small| atoms <= small);
    let rest = if want_more {
        let vectored = counted_vector_loop(heap, r, init, acc_name, elem_name, body);
        let listed = list_loop(heap, r, init, acc_name, elem_name, body);
        let is_pair = heap.list(vec![value::sym("/pair?"), r]);
        let c3 = heap.list(vec![value::sym(kw::IF), is_pair, listed, slow]);
        let is_vec = heap.list(vec![value::sym("/vector?"), r]);
        heap.list(vec![value::sym(kw::IF), is_vec, vectored, c3])
    } else {
        slow
    };
    let is_range = heap.list(vec![value::sym("/range?"), r]);
    heap.list(vec![value::sym(kw::IF), is_range, ranged, rest])
}

/// A literal body this rewrite copies: bounded so a large body does not multiply the
/// compiled code for a fold it barely speeds up. Up to `MAX` gets the range loop (one
/// copy beside the fold); up to `SMALL`, when set, the vector and list loops too (two more).
const FOLD_LOOP_MAX_ATOMS: usize = 96;
const FOLD_LOOP_SMALL_ATOMS: Option<usize> = None; // parked — see `coll_or_fold`

fn form_atoms(heap: &Heap, form: Value, depth: usize) -> usize {
    if depth > 256 {
        return usize::MAX / 2;
    }
    match heap.list_items(form) {
        Some(items) => items
            .iter()
            .map(|&it| form_atoms(heap, it, depth + 1))
            .sum(),
        None => 1,
    }
}

fn range_fold_call(heap: &mut Heap, form: Value, items: &[Value]) -> Option<Value> {
    let ValueRef::Sym(h) = items[0].unpack() else {
        return None;
    };
    if !is_fold_head(h) || items.len() != 4 {
        return None;
    }
    let (params, body) = plain_fn_literal(heap, items[3])?;
    if params.len() != 2 {
        return None;
    }
    if body.iter().map(|&f| form_atoms(heap, f, 0)).sum::<usize>() > FOLD_LOOP_MAX_ATOMS {
        return None;
    }
    let r = value::gensym("coll");
    let init = value::gensym("coll-init");
    let choose = coll_or_fold(heap, form, r, init, items[3], params[0], params[1], &body);
    let outer = heap.list(vec![r, items[1], init, items[2]]);
    Some(rebuild_list(
        heap,
        form,
        vec![value::sym(kw::LET), outer, choose],
    ))
}
