//! A file's declared type vocabulary: registering its `(sig …)` declarations (hand-written
//! and macro-emitted) under namespace-qualified names, installing the abilities, records,
//! aliases and operator domains a `sig` may name, and validating each hand-written
//! declaration against the definition it annotates.

use super::*;

/// Qualify a `(sig …)` declaration's target name to the file's namespace, the way
/// the resolve pass qualifies a def head — but only with the same *positive
/// evidence* the resolver requires: the file actually defines `ns/name` (recorded
/// as a file-global in pass 2). A name the file doesn't define (an imported/prelude
/// name) or an already-`ns/`-qualified name is left as written, so it still matches
/// the bare/qualified head the call resolves to.
pub(super) fn qualify_decl_name(ctx: &Ctx, file_ns: Option<&str>, name: Symbol) -> Symbol {
    match file_ns {
        Some(ns) => {
            let qualified = crate::eval::macros::qualify_name(ns, name);
            if ctx.is_file_global(qualified) {
                qualified
            } else {
                name
            }
        }
        None => name,
    }
}

/// Parse `form` as any of the four `(sig …)` declaration shapes (arrow, arrow with
/// type-variables, overload, or non-arrow value type) and record it in `ctx` under
/// the namespace-qualified name ([`qualify_decl_name`]). Shared by pass 2.5's two
/// sources — the un-expanded top-level `(sig …)` forms and the `(sig …)` forms
/// reconstructed from `%register-sig` in the expanded tree.
fn register_declared_sig(heap: &Heap, ctx: &mut Ctx, file_ns: Option<&str>, form: Value) {
    // The property keywords — `:pure`, `:total` — of any sig shape (ADR-351).
    if let Some((name, _, props)) = annot::sig_decl_head(heap, form)
        && !props.is_empty()
    {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_props(qn, &props);
    }
    if let Some((name, sig)) = annot::parse_sig_decl(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_sig(qn, sig);
        // `(T… -> _)` — the result is left to inference (ADR-420).
        if let Some((_, Some(type_form), _)) = annot::sig_decl_head(heap, form)
            && annot::arrow_ret_is_open(heap, type_form)
        {
            ctx.mark_open_ret(qn);
        }
    }
    if let Some((name, sv)) = annot::parse_sig_decl_with_vars(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_sig_with_vars(qn, sv);
    }
    // An overloaded sig — `(and (int -> int) (bool -> bool))` — has no single `Sig`,
    // so `parse_sig_decl` above yields nothing for it; record it separately (ADR-116).
    if let Some((name, sigs)) = annot::parse_sig_decl_overload(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_overload(qn, sigs);
    }
    // Non-arrow `(sig x T)` value-type declarations — consumed by the gradual-
    // assignment check on `(def x …)` (the first `GradualTy` consumer).
    if let Some((name, ty)) = annot::parse_value_sig_decl(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_value_ty(qn, ty);
    }
}

/// Do the two arities overlap — is there an argument count both admit? The
/// question a sig-vs-definition mismatch turns on: only a *disjoint* pair is
/// provably wrong (a multi-arm `defn` annotated with one arm's arrow overlaps,
/// and must stay silent).
fn arities_overlap(a: Arity, b: Arity) -> bool {
    let a_max = a.max.unwrap_or(usize::MAX);
    let b_max = b.max.unwrap_or(usize::MAX);
    a.min <= b_max && b.min <= a_max
}

/// Validate this file's hand-written `(sig …)` declarations (the declaration check) — see the
/// call site for why an unreadable annotation is worse than no annotation.
///
/// Reads the *un-expanded* forms, which is where a hand-written declaration is
/// legible; a macro-emitted sig (`defrecord`'s) is machine-built and deliberately
/// not second-guessed here.
pub(super) fn check_sig_declarations(
    heap: &Heap,
    forms: &[Value],
    file_ns: Option<&str>,
    ctx: &Ctx,
    out: &mut Vec<Finding>,
) {
    for &form in forms {
        // Typed parameters (ADR-420) are a declaration too: each type they write must read,
        // or the position silently widens to `any`.
        if let Some((name, types)) = crate::eval::macros::defn_param_types(heap, form) {
            let spelling = value::symbol_name(name);
            let pos = heap.form_pos_only(form);
            for (label, ty_form) in types {
                if let Some(problem) = annot::type_expr_problem(heap, ty_form) {
                    let what = if label == "->" {
                        "the result".to_string()
                    } else {
                        format!("parameter `{label}`")
                    };
                    out.push(Finding::new(
                        Lint::Declaration,
                        pos,
                        format!("defn {spelling}: {what}: {problem}"),
                    ));
                }
            }
            continue;
        }
        let Some((name, ty_form)) = annot::sig_decl_parts(heap, form) else {
            continue;
        };
        let spelling = value::symbol_name(name);
        let pos = heap.form_pos_only(form);
        // 1. Is the type-expression readable at all?
        if let Some(problem) = annot::type_expr_problem(heap, ty_form) {
            out.push(Finding::new(
                Lint::Declaration,
                pos,
                format!("sig {spelling}: {problem}"),
            ));
            continue; // the rest reads the parsed sig, which doesn't exist
        }
        let qualified = qualify_decl_name(ctx, file_ns, name);
        // 2. Does it annotate anything? A sig for a name this file never defines (and
        //    that isn't bound anywhere in the image) annotates nothing — usually a
        //    rename that moved the `defn` and left the declaration behind.
        if walk::is_unbound(heap, ctx, name) && walk::is_unbound(heap, ctx, qualified) {
            out.push(Finding::new(
                Lint::Declaration,
                pos,
                format!("sig {spelling}: nothing named `{spelling}` is defined here"),
            ));
            continue;
        }
        // 3. Does its arity agree with the definition's? Only a *disjoint* pair is
        //    provably wrong — see `arities_overlap`.
        if let (Some((_, sig)), Some(def_arity)) = (
            annot::parse_sig_decl(heap, form),
            ctx.file_arity(qualified).or_else(|| ctx.file_arity(name)),
        ) {
            let declared = walk::arity_of_sig(&sig);
            if !arities_overlap(declared, def_arity) {
                out.push(Finding::new(
                    Lint::Declaration,
                    pos,
                    format!(
                        "sig {spelling}: declares {} argument(s) but the definition takes {}",
                        sigs::arity_str(declared),
                        sigs::arity_str(def_arity),
                    ),
                ));
            }
        }
    }
}

/// Recover `(sig name type)` forms from the *expanded* tree. Each `(sig …)` — hand-
/// written or emitted by a macro like `defrecord` — lowers to `(%register-sig 'name
/// 'type)`; this walks into `(do …)` blocks (what those macros wrap their output in)
/// and, for each `%register-sig`, rebuilds the equivalent `(sig name type)` form so
/// [`register_declared_sig`] can parse it with the ordinary sig parsers. Building the
/// form needs `&mut Heap`; GC is blocked for the whole check, so the pushed handles
/// stay live.
fn collect_register_sig_forms(heap: &mut Heap, form: Value, out: &mut Vec<Value>) {
    // Recurses through nested `(do (do …))`; a deep-but-legal chain would blow
    // the native stack (a SIGSEGV `catch_unwind` can't catch — the sibling of
    // the walk.rs/recursion.rs hardening). Grow the stack in heap-backed
    // segments like the rest.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_register_sig_forms_inner(heap, form, out)
    })
}

fn collect_register_sig_forms_inner(heap: &mut Heap, form: Value, out: &mut Vec<Value>) {
    let Some(items) = heap.list_items(form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    // `(do …)` and `(%lint-allow :category …)` — what `check-allow` expands to — both wrap
    // definitions; a `(sig …)` inside a `check-allow` block must still declare its `defn`.
    if crate::core::value::symbol_is(head, kw::DO)
        || crate::core::value::symbol_is(head, "%lint-allow")
    {
        for &it in &items[1..] {
            collect_register_sig_forms(heap, it, out);
        }
        return;
    }
    if crate::core::value::symbol_is(head, "%register-sig") && items.len() == 3 {
        // items[1] = (quote name), items[2] = (quote type)
        if let (Some(name), Some(ty)) =
            (walk::unquote(heap, items[1]), walk::unquote(heap, items[2]))
        {
            let sig_head = crate::core::value::sym("sig");
            let rebuilt = heap.list(vec![sig_head, name, ty]);
            out.push(rebuilt);
        }
    }
}

/// The type vocabulary a `sig` may name, installed before any sig is parsed: sealed
/// abilities as types (ADR-181, so `(sig f (Shape -> …))` resolves to the union of its
/// members' shapes), which ids are records (the tiebreak for a sealed member that also
/// spells a built-in kind), the type aliases (ADR-326), and the operator sugar's domains
/// (ADR-299).
pub(super) fn install_type_vocabulary(
    heap: &Heap,
    expanded: &[Value],
    file_ns_name: Option<&str>,
    mut import_scope: ImportScope,
    multi_info: &protocol::MultiInfo,
    out: &mut Vec<Finding>,
) {
    annot::set_ability_types(protocol::ability_type_table(heap, expanded));
    annot::set_record_ids(protocol::record_id_names(heap, expanded));
    out.append(&mut import_scope.unknown_only);
    annot::set_type_aliases(
        protocol::type_alias_table(heap, expanded, file_ns_name),
        file_ns_name.map(str::to_string),
        import_scope,
    );
    // A `deftype` whose shape exceeds the lattice's node budget (`MAX_TY_NODES`) is
    // widened to its bare tags by the constructors, silently: every field reads as
    // unrefined. A written shape that came back with no fields (or no positions) is
    // that case exactly; say so at the declaration.
    for &form in expanded {
        let mut aliases = HashMap::new();
        protocol::collect_register_types_into(heap, form, None, &mut aliases);
        let mut aliases: Vec<(String, Value)> = aliases.into_iter().collect();
        aliases.sort_by(|a, b| a.0.cmp(&b.0)); // report order, not hash order (B8)
        for (name, ty_form) in aliases {
            let Some(ty) = annot::parse_type(heap, ty_form) else {
                continue;
            };
            let declares_shape = list_items(heap, ty_form)
                    .and_then(|items| items.first().copied())
                    .is_some_and(|h| matches!(h, Value::Sym(s) if value::symbol_is(s, "record") || value::symbol_is(s, "tuple")));
            if declares_shape && ty.record_fields().is_none() && ty.positional_elems().is_none() {
                out.push(Finding::new(
                        Lint::Declaration,
                        heap.form_pos_only(form),
                        format!(
                            "deftype {name}: the shape exceeds the checker's budget of {} nodes and is read as a bare `{}` — every field is unrefined; split it into named parts",
                            crate::types::MAX_TY_NODES,
                            ty
                        ),
                    ));
            }
        }
    }
    // The operator sugar's domains: `number` plus the records `num/*` / `compare-to`
    // have methods for, from this file's `defmethod`s + the registry.
    sigs::set_operator_domains(protocol::operator_domains(multi_info));
}

/// Register the file's `(sig …)` declarations — the authoritative signatures the call
/// check consults first — and build its ability facts. Two sources of declarations, fed
/// through the same parsers and namespace qualification (`register_declared_sig`):
///
///  (a) the *un-expanded* top-level `(sig …)` forms, where a hand-written declaration
///      is legible (the `sig` macro's own output is dropped from the analysed tree); and
///  (b) every `(%register-sig 'name 'type)` in the *expanded* tree — what a
///      macro-emitted sig lowers to (`defrecord` nests its sigs in a `(do …)` that (a)
///      cannot see). The overlap with (a) re-registers idempotently.
///
/// A declared name is qualified to the file's namespace exactly as the resolve pass
/// qualifies a def head, so `(sig g …)` inside `(defmodule ns …)` keys under `ns/g`.
pub(super) fn register_declarations(
    heap: &mut Heap,
    forms: &[Value],
    expanded: &[Value],
    ctx: &mut Ctx,
    file_ns_name: Option<&str>,
    visible: &HashSet<String>,
) -> std::sync::Arc<protocol::AbilityInfo> {
    // Building a `(sig name type)` form needs `&mut heap`, so collect first and
    // register after — GC is blocked for the whole check, so the handles stay live.
    let mut macro_sig_forms: Vec<Value> = Vec::new();
    for &form in expanded {
        collect_register_sig_forms(heap, form, &mut macro_sig_forms);
    }
    // THIS file's records' declared field types, BEFORE any other sig is parsed: a
    // `(sig f (pt -> …))` resolves the record NAME to a shape carrying them
    // (`annot::record_ty`), and the file is not loaded while it is checked, so the
    // constructor's sig — where `defrecord` puts the field types — is nowhere else.
    // A constructor's sig is the one whose name IS a registered record id and whose
    // result is a record shape.
    {
        let mut field_types = HashMap::new();
        for &form in &macro_sig_forms {
            let Some((name, sig)) = annot::parse_sig_decl(heap, form) else {
                continue;
            };
            let id = value::symbol_name(qualify_decl_name(ctx, file_ns_name, name));
            if let Some(fields) = sig.ret.record_fields() {
                field_types.entry(id).or_insert_with(|| fields.clone());
            }
        }
        annot::set_record_field_types(field_types);
    }
    // The ability facts and the sealed-op occurrence-typing domains go in before any
    // pass runs `sig_of`, so an imported function's inferred sig is never cached
    // without the sealed-op demand (ADR-190). Restricted to the file's world (B8):
    // another file's imported abilities, left in the process registry by checking it
    // first, are none of this file's business.
    let ability_info = std::sync::Arc::new(protocol::build_ability_info_visible(
        heap,
        expanded,
        Some(visible),
    ));
    annot::set_sealed_op_domains(protocol::build_sealed_op_domains(&ability_info));
    // On the ctx from here too: the demand walk asks `is_ability_op` to tell an op
    // function from a same-file function spelling its name (ADR-350).
    ctx.set_ability(ability_info.clone());
    for &form in forms.iter().chain(&macro_sig_forms) {
        register_declared_sig(heap, ctx, file_ns_name, form);
    }
    ability_info
}
