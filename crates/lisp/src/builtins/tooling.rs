use super::args::{arg, expect_string, expect_symbol};
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::{cst, printer};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::{Sig, Ty};
    // The rename ledger (ADR-304): the Rust table `crate::renames::RENAMES` handed to
    // Brood as a map `{old {:to new :adr adr}}`, so `nest check --fix-renames` reads the
    // SAME table the runtime error and the checker diagnostic read.
    primitives.def(
        "%renames",
        Arity::exact(0),
        // `{old-name {:to new-name :adr adr}}` — the value type is what lets
        // `renames/renamed-to` verify its declared `(or map nil)`.
        Sig::new(vec![], Ty::map_of(string, map_ty)),
        &[],
        "The rename ledger (ADR-304): a map of old public name (string) to `{:to new-name :adr adr}` for every deliberate rename the runtime still points at. Backs `renames/ledger`.",
        renames_ledger,
    );
    // `macroexpand` (the fixpoint loop) is written in Brood (`std/prelude.blsp`)
    // over this single-step primitive — ADR-064, so its loop state is auto-rooted
    // rather than hand-rooted in Rust. `macros::macroexpand` (Rust) stays for the
    // compile pass, which runs under MACRO_BLOCK.
    // gensym accepts anything as a prefix (string/sym/keyword/nil/anything is
    // turned into its `display` form), so its prefix slot is `any` — not the
    // narrower `string` the original Sig claimed, which made the checker warn
    // on legitimate `(gensym 'foo)` calls.
    primitives.def(
        "reflect/gensym",
        Arity::range(0, 1),
        Sig::new(vec![any], sym),
        &["prefix"],
        "A fresh, unique symbol, with an optional name prefix.\n\n    (symbol? (reflect/gensym \"tmp\"))   ;=> true\n    (= (reflect/gensym) (reflect/gensym))      ;=> false",
        gensym,
    );
    // source positions (editor tooling; see docs/tooling.md)
    primitives.def(
        "%form-pos",
        Arity::exact(1),
        Sig::new(vec![any], vec_ty.union(nil_ty)),
        &["form"],
        "A form's [line col] source position, or nil.",
        form_pos,
    );
    primitives.def(
        "%current-file",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        &[],
        "The path of the file currently being loaded, or nil.",
        current_file,
    );
    primitives.def(
        "%fn-module",
        Arity::exact(1),
        Sig::new(vec![any], any),
        &["f"],
        "The module whose source function `f` was written in, as a symbol — an anonymous `fn` too, which carries its defining module from the code that built it — or nil for a builtin, a non-function, or code of no known module.",
        fn_module,
    );
    primitives.def(
        "%source-location",
        Arity::exact(1),
        Sig::new(vec![sym], vec_ty.union(nil_ty)),
        &["name"],
        "Where global name was defined, as [file line col], or nil. Quote it: (reflect/source-location 'foo).",
        source_location);
    primitives.def(
        "reflect/private?",
        Arity::exact(1),
        Sig::new(vec![sym], bool_ty),
        &["name"],
        "Whether the global `name` is module-private (ADR-146). Quote the qualified symbol: (reflect/private? 'mod/helper).\n\n    (reflect/private? 'map)   ;=> false",
        private_p);
    primitives.def(
        "%references-in-source",
        Arity::exact(2),
        Sig::new(vec![sym.union(string), string], any),
        &["name", "source"],
        "Occurrences of the global `name` in `source`, as a list of [line col] (1-based); locals that shadow it are excluded.",
        references_in_source);
    primitives.def(
        "%source-deps",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        &["src"],
        "Per TOP-LEVEL form of `src`, in document order, what it defines, what globals it uses, and which of those change the world outside the evaluation (a file written or removed, a program run, a signal sent, the runtime halted — what a live evaluator must not run on a half-typed form): a list of `{:defines (…) :references (…) :effects (…)}` maps of name strings. What a live evaluator needs to re-run only the forms an edit could affect instead of everything below it. Syntactic, read the way find-references reads: locals are excluded, a quoted `'…` is data. It cannot see a name a macro introduces, nor a side effect through which one form reaches another without naming anything.",
        source_deps);
    primitives.def(
        "%declared-sig",
        Arity::exact(1),
        Sig::new(vec![sym.union(string)], any),
        &["name"],
        "The type expression a `(sig …)` DECLARED for global `name`, as written — `(model any -> model)` — or nil when none was declared (a `deftype` alias is not a signature and answers nil too). Symbol or string arg; a `mod/name` reference is rooted to its package like any other.",
        declared_sig);
    primitives.def(
        "%function-effect",
        Arity::exact(1),
        Sig::new(vec![sym.union(string)], string.union(nil_ty)),
        &["name"],
        "The effect a call to the loaded global `name` REACHES, described — `(io/puts …)`, or `calls f, which performs an effect: …` — or nil when the walk finds none. The checker's own purity walk (ADR-351), asked as a question instead of against a `:pure` declaration. It REPORTS an effect and never claims purity: the effectful-primitive list is a deny-list, so a described effect is one the body reaches, while nil means only *not known to perform one*. Backs the Effects line in `nest docs`.",
        function_effect);
    primitives.def(
        "%type-alias",
        Arity::exact(1),
        Sig::new(vec![sym.union(string)], any),
        &["name"],
        "The type expression a `(deftype name T)` alias declares, as written, or nil when `name` is not an alias. The single-name counterpart of `%type-aliases`, which builds the whole list — this is a keyed lookup, because the runtime contract (`type-matches?`) asks it per checked value (KI-165). Symbol or string arg; a bare name resolves in the current namespace like a def head, as `%declared-sig` does.",
        type_alias);
    primitives.def(
        "%type-aliases",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        &[],
        "Every `(deftype name T)` alias the checker knows (ADR-327), as a list of `[qualified-name-string type-form]` — the raw type-expression as written. The prelude's and every loaded module's. The mechanism under `reflect/type-aliases`, which filters by module and renders.",
        type_aliases);
    primitives.def(
        "%type-signature",
        Arity::exact(1),
        Sig::new(vec![sym.union(string)], string.union(nil_ty)),
        &["name"],
        "The checker's type signature for global `name` (declared/curated/inferred) as an arrow string like \"(int -> int)\", or nil if it can't be pinned. Symbol or string arg: (reflect/type-signature 'map).",
        type_signature);
    // introspection (editor tooling; see docs/lsp.md) — derive what we can from
    // the bound value (arglist, doc); enumerate the global table for completion.
    primitives.def(
        "reflect/doc",
        Arity::exact(1),
        Sig::new(vec![any], string.union(nil_ty)),
        &["f"],
        "The docstring of a function, macro, or primitive, or nil.\n\n    (string? (reflect/doc inc))   ;=> true\n    (reflect/doc (fn (x) x))      ;=> nil",
        doc,
    );
    primitives.def(
        "reflect/arglist",
        Arity::exact(1),
        Sig::new(vec![any], list_ty),
        &["f"],
        "The parameter list of a function, macro, or primitive, or nil.\n\n    (reflect/arglist inc)                  ;=> (n)\n    (reflect/arglist (fn (a & more) a))    ;=> (a & more)",
        arglist,
    );
    primitives.def(
        "reflect/arglists",
        Arity::exact(1),
        Sig::new(vec![any], list_ty),
        &["f"],
        "Every arm's parameter list of a function, macro, or primitive — one list per arity of a multi-arity `defn`, where `arglist` shows only the last — or nil.\n\n    (reflect/arglists (fn ((a) a) ((a b) b)))   ;=> ((a) (a b))\n    (reflect/arglists inc)                     ;=> ((n))",
        arglists,
    );
    primitives.def(
        "reflect/global-names",
        Arity::exact(0),
        Sig::nullary(list_ty),
        &[],
        "Every globally bound symbol, sorted by spelling.\n\n    (includes? (reflect/global-names) 'map)   ;=> true",
        global_names,
    );
    primitives.def(
        "reflect/special-forms",
        Arity::exact(0),
        Sig::nullary(list_ty),
        &[],
        "The special-form / core-macro names (strings) that read as keywords — the canonical list shared by the syntax highlighter and the LSP.\n\n    (includes? (reflect/special-forms) \"let\")   ;=> true",
        special_forms);
    primitives.def(
        "reflect/doc-forms",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "The def... heads that carry a docstring, as head -> :fn or :head — the canonical list shared by the syntax highlighter and the LSP. :fn means the head builds a function, where a lone trailing string is the return value, not a doc.\n\n    (get (reflect/doc-forms) \"defn\")   ;=> :fn",
        doc_forms,
    );
    primitives.def(
        "bound?",
        Arity::exact(1),
        Sig::new(vec![sym], bool_ty),
        &["sym"],
        "Whether sym is bound in scope. Quote it: (bound? 'foo).\n\n    (bound? 'first)   ;=> true\n    (bound? 'nope)    ;=> false",
        bound_p,
    );
    primitives.def(
        "%prelude-global?",
        Arity::exact(1),
        Sig::new(vec![sym], bool_ty),
        &["name"],
        "Is `name` a global the PRELUDE bound — its own definitions, every native builtin, the registries it seeds? Fixed at the freeze, identical in every process of a binary. `stdimage/build` audits with it that every root global a std module defines reached an owning section (KI-112).",
        prelude_global_p,
    );
    primitives.def(
        "%global-generation",
        Arity::exact(1),
        Sig::new(vec![sym], int),
        &["sym"],
        "The rebinding generation of global sym: grows with every def of that name in this runtime, 0 for a name never def'd here. For restoring a TEMPORARY rebinding without clobbering a redefinition made in between (debug/untrace-fn). Quote it: (%global-generation 'foo).",
        global_generation,
    );
    // What a load (re)bound, from the generations the runtime already stamps (ADR-603): the
    // generation now, and every global defined or removed after one — so a plugin host asks
    // "what did this package's load define?" without reading every global's value.
    primitives.def(
        "reflect/generation",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "The runtime's global generation now: every later `def` or `reflect/undef` of a global stamps it with a greater one. Hand it to `reflect/rebound-since` after a load to learn what the load bound.\n\n    (int? (reflect/generation))   ;=> true",
        runtime_generation,
    );
    primitives.def(
        "reflect/rebound-since",
        Arity::exact(1),
        Sig::new(vec![int], any),
        &["generation"],
        "Every global defined, redefined or removed since `generation` (a `reflect/generation` taken before), as a list of symbols in no particular order — a removed one is no longer `bound?`. What a load bound, read from the stamps the runtime keeps anyway, with no global's value touched.\n\n    (reflect/rebound-since (reflect/generation))   ;=> ()",
        rebound_since,
    );
    primitives.def(
        "reflect/global-values",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "Every global a `def` may rebind, as a map of symbol -> value: the names the language does not reserve (a project's, a plugin's, a dynamic variable), each by its exact global name. Taken before a load, it is what each global `reflect/rebound-since` names held before the load — the names a macro generated and the ones a project rooted included (ADR-606).\n\n    (map? (reflect/global-values))   ;=> true",
        global_values,
    );
    primitives.def(
        "reflect/source-locations",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "Where every global a `def` may rebind was defined, as a map of symbol -> [file line col]: the companion of `reflect/global-values`, taken before a load so a global given back its old value can say where that value was defined again (`reflect/set-source-location`, ADR-606).\n\n    (map? (reflect/source-locations))   ;=> true",
        source_locations,
    );
    primitives.def(
        "reflect/set-source-location",
        Arity::exact(2),
        Sig::new(vec![sym, any], any),
        &["name", "location"],
        "Record global `name` (its exact name; a root escape `/org/f` is read as `org/f`) as defined at `location` — `[file line col]`, as `reflect/source-location` answers — or forget its site with nil. What a host undoing a load puts back with the value (ADR-606). Answers `location`.\n\n    (reflect/set-source-location 'my-fn nil)   ;=> nil",
        set_source_location,
    );
    primitives.def(
        "reflect/undef",
        Arity::exact(1),
        Sig::new(vec![sym], bool_ty),
        &["name"],
        "Remove the global binding `name` — the inverse of `def` — and everything recorded about it (privacy, metadata, def site, declared sig). Returns true when a binding went away, false when none was bound. The symbol is the global's full name, taken as-is: pass `'my/mod/helper` for a module's definition; a bare `'helper` names the root binding. A name Brood ships (the prelude, a builtin, an embedded std module) is refused, as `def` refuses it.\n\nAfterwards `bound?` is false, and a bare reference compiled from now on resolves as if the name had never been defined — through the module's imports, then root — so removing a module's `map` lets its code reach the prelude `map` again. Code compiled while the name was bound does not keep the old value: every cache of it is invalidated, so such code raises `unbound symbol` naming the removal; re-evaluating it resolves afresh.\n\n    (reflect/undef 'no-such-global)   ;=> false",
        undef,
    );
}

// ---------- source positions (editor tooling; see docs/tooling.md) ----------

/// `(form-pos form)` — the `[line col]` (1-based) where `form` was read, or
/// `nil`. Recorded by the reader for list forms; used by the test macros to
/// capture a test's source line *before* the form expands.
pub(super) fn form_pos(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.form_pos_only(arg(args, 0)) {
        Some(p) => Ok(heap.alloc_vector2(Value::int(p.line as i64), Value::int(p.col as i64))),
        None => Ok(Value::nil()),
    }
}

/// `(current-file)` — the path of the file currently being `load`ed, or `nil`
/// (e.g. at the REPL). Maintained by `load`.
/// `(source-location 'name)` — where `name`'s global definition was loaded from,
/// as `[file line col]`, or `nil` if it has no recorded site (a Rust builtin, or
/// an unknown/local name). Prelude globals resolve to a materialized copy of the
/// standard library. The site is captured at load time
/// before macroexpansion, so `defn`/`defmacro` definitions are located
/// accurately. The image-query foundation for cross-file goto-definition (ADR-031
/// / docs/lsp.md). Takes a symbol, so quote it: `(source-location 'foo)`.
pub(super) fn source_location(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "reflect/source-location", arg(args, 0))?;
    // The root escape asks for exactly that global (ADR-603), as `def` binds it: `/org/f` is
    // `org/f` even inside a project whose own `org` module would root `org/f`.
    if let Some(exact) = root_escaped(name) {
        return Ok(match heap.def_site(exact) {
            Some(loc) => {
                let file = heap.alloc_string(&loc.file);
                heap.alloc_vector(vec![
                    file,
                    Value::int(loc.pos.line as i64),
                    Value::int(loc.pos.col as i64),
                ])
            }
            None => Value::nil(),
        });
    }
    // Rooted as `declared-signature` roots it (ADR-560): inside project bedit,
    // `keyhelp/ed-first-line` is `bedit/keyhelp/ed-first-line`, and the def site is recorded
    // under the full name — so a query by the name the code itself writes answered nil.
    let rooted = rooted_name(heap, name);
    match heap.def_site(rooted).or_else(|| heap.def_site(name)) {
        Some(loc) => {
            let file = heap.alloc_string(&loc.file);
            Ok(heap.alloc_vector(vec![
                file,
                Value::int(loc.pos.line as i64),
                Value::int(loc.pos.col as i64),
            ]))
        }
        None => Ok(Value::nil()),
    }
}

/// `(reflect/private? 'mod/name)` — whether the global `mod/name` is module-private
/// (ADR-146). Reads the recorded privacy fact (`Heap::is_private`), the single
/// source of truth every enforcement/import site now consults, rather than
/// re-deriving anything from the name (a private is spelled like a public since
/// step 2). Takes the qualified symbol as given, so quote it:
/// `(reflect/private? 'mod/helper)` → true when `mod` defined it with `defn-`/`def-`,
/// `(reflect/private? 'mod/pub)` → false. An undefined name is not private.
pub(super) fn private_p(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "reflect/private?", arg(args, 0))?;
    Ok(Value::boolean(heap.is_private(name)))
}

/// A name-query argument: a symbol, or a string naming an already-interned symbol.
/// `Ok(None)` for a string never interned — it names no global.
fn existing_name_arg(heap: &Heap, who: &str, v: Value) -> Result<Option<value::Symbol>, LispError> {
    match v {
        Value::Sym(s) => Ok(Some(s)),
        Value::Str(id) => Ok(value::intern_existing(heap.string(id))),
        other => Err(LispError::wrong_type(heap, who, "symbol or string", other)),
    }
}

/// The global a name query means: `commands/cmd-open` is `bedit/commands/cmd-open` inside
/// project bedit (ADR-070) — the same rooting an `(eval 'commands/cmd-open)` behind a
/// keymap gets; a bare name resolves in the current namespace, as a def head would.
/// The global a root-escaped name (`/org/f`) names exactly — `org/f`, never rooted — or
/// `None` for any other name (`/` alone is division).
fn root_escaped(sym: value::Symbol) -> Option<value::Symbol> {
    let spelled = value::symbol_name(sym);
    match spelled.strip_prefix('/') {
        Some(exact) if !exact.is_empty() => Some(value::intern(exact)),
        _ => None,
    }
}

fn rooted_name(heap: &Heap, sym: value::Symbol) -> value::Symbol {
    heap.root_qualified_ref(sym)
        .unwrap_or_else(|| crate::eval::macros::resolve_reference(heap, sym))
}

/// `(type-signature 'name)` — the checker's type signature for the global `name`
/// (declared, curated, or inferred), as an arrow string like `"(int -> int)"`, or
/// `nil` when it has no pinnable signature (unbound, non-callable, or a value the
/// checker can't type). Reads the loaded image, same discipline as `arglist`/`doc`.
/// The introspection foundation the LSP hover and the `nest mcp` `lookup` tool
/// share (so they can't drift on what a name's type is). `name` may be a symbol or
/// a string; a never-interned name names no global, so it returns `nil`.
pub(super) fn type_signature(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let Some(sym) = existing_name_arg(heap, "reflect/type-signature", arg(args, 0))? else {
        return Ok(Value::nil());
    };
    // Rooted first, as `declared-signature` is (ADR-560): `model/f` inside a project is the
    // project's `bedit/model/f`.
    let rooted = rooted_name(heap, sym);
    match crate::types::check::signature_string(heap, rooted)
        .or_else(|| crate::types::check::signature_string(heap, sym))
    {
        Some(signature) => Ok(heap.alloc_string(&signature)),
        None => Ok(Value::nil()),
    }
}

/// `(%declared-sig name)` — the raw type expression a `(sig …)` declared for `name`, or
/// nil. What an editor shows as a command's CONTRACT: the declaration as the author wrote
/// it (`(model any -> model)`), not the checker's expansion of it. A `deftype` alias entry
/// (`(%type T)`) is a declaration of a type, not of a global, so it answers nil.
pub(super) fn declared_sig(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let Some(sym) = existing_name_arg(heap, "reflect/declared-signature", arg(args, 0))? else {
        return Ok(Value::nil());
    };
    let rooted = rooted_name(heap, sym);
    // The type-form alone: the properties a `(sig f T :pure)` stores beside it are
    // `%declared-sig-props`'s (ADR-351).
    let found = [rooted, sym].into_iter().find_map(|key| {
        crate::builtins::modules::sig_type_of(heap, heap.declared_sig_value(key))
            .map(|form| (key, form))
    });
    let Some((key, form)) = found else {
        return Ok(Value::nil());
    };
    let is_alias = heap.list_to_vec(form).ok().is_some_and(|items| {
        matches!(items.first(), Some(Value::Sym(h)) if value::symbol_is(*h, crate::builtins::modules::TYPE_ALIAS_MARKER))
    });
    // As written: the module-scoping of its type names (`type_scope`) undone.
    Ok(if is_alias {
        Value::nil()
    } else {
        super::type_scope::unscope_form(heap, key, form)
    })
}

/// `(%function-effect name)` — the effect a call to the loaded global `name` reaches, or
/// nil. See the registration's docstring for why this reports rather than certifies.
pub(super) fn function_effect(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let Some(sym) = existing_name_arg(heap, "%function-effect", arg(args, 0))? else {
        return Ok(Value::nil());
    };
    let rooted = rooted_name(heap, sym);
    let described = crate::types::check::effect_of(heap, rooted)
        .or_else(|| crate::types::check::effect_of(heap, sym));
    Ok(match described {
        Some(text) => heap.alloc_string(&text),
        None => Value::nil(),
    })
}

/// `(%type-alias name)` — the type expression `(deftype name T)` declared, or nil.
///
/// The keyed counterpart of [`type_aliases`], which walks every declared sig in the
/// runtime: the runtime contract asks this per checked value, so it must be a lookup
/// rather than a scan (KI-165). Resolution mirrors [`declared_sig`] exactly — the rooted
/// qualified reference first, the bare symbol second — because an alias is registered
/// under the name a def head would take.
pub(super) fn type_alias(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let Some(sym) = existing_name_arg(heap, "%type-alias", arg(args, 0))? else {
        return Ok(Value::nil());
    };
    Ok(alias_expansion(heap, sym).unwrap_or_else(Value::nil))
}

/// The type expression the alias `sym` names, resolved as `%type-alias` resolves it, or
/// `None`. Also asked by the contract's verdict memo (ADR-480), which answers a record
/// one level deep without leaving the kernel.
pub(crate) fn alias_expansion(heap: &Heap, sym: value::Symbol) -> Option<Value> {
    let rooted = rooted_name(heap, sym);
    for key in [rooted, sym] {
        if let Some(inner) = alias_form_at(heap, key) {
            return Some(inner);
        }
    }
    // Neither exact key hit. A contract runs wherever the VALUE is checked, which is not
    // where the alias was declared — a `test` body runs in its own green process, and the
    // bare `c-point` there does not resolve to the `contract-test/c-point` the `deftype`
    // registered. So fall back to the rule the checker's own `alias_ty` ends with: the ONE
    // alias whose qualified name ends in `/name`, declining when two could answer. That is
    // a scan of every declared sig, so it sits behind both keyed lookups AND behind a
    // per-process memo (`Heap::alias_key_by_suffix`): it is reached by every symbol that
    // is not a base type — an alias, an ability name, a `?A` type variable — per checked
    // value, and unmemoised it was three full scans per `conj` under contracts.
    if value::symbol_name_ref(sym).contains('/') {
        return None;
    }
    heap.alias_key_by_suffix(sym, |heap, key| alias_form_at(heap, key).is_some())
        .and_then(|key| alias_form_at(heap, key))
}

/// The type expression of the alias stored under exactly `key`, or `None`. An alias rides
/// the declared-sig store wrapped as `(%type T)` so no reader can mistake it for a
/// signature (ADR-327); this unwraps that marker and nothing else.
pub(crate) fn alias_form_at(heap: &Heap, key: value::Symbol) -> Option<Value> {
    let form = heap.declared_sig_value(key)?;
    let items = heap.list_to_vec(form).ok()?;
    match items.as_slice() {
        [Value::Sym(head), inner]
            if value::symbol_is(*head, crate::builtins::modules::TYPE_ALIAS_MARKER) =>
        {
            Some(*inner)
        }
        _ => None,
    }
}

/// `(%type-aliases)` — every `deftype` alias in the declared-sig store, as
/// `[qualified-name type-form]` vectors. An alias rides the store wrapped as `(%type T)`
/// (ADR-327); this unwraps the marker, and answers ONLY the alias entries — a signature is
/// `%declared-sig`'s. The docs' "Types" section reads it: a `deftype` binds no global, so
/// the module's name walk cannot see it.
pub(super) fn type_aliases(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let entries = heap.declared_sigs_everywhere();
    let mut out = Vec::new();
    for (sym, form) in entries {
        let Ok(items) = heap.list_to_vec(form) else {
            continue;
        };
        let [Value::Sym(head), inner] = items.as_slice() else {
            continue;
        };
        if !value::symbol_is(*head, crate::builtins::modules::TYPE_ALIAS_MARKER) {
            continue;
        }
        let inner = super::type_scope::unscope_form(heap, sym, *inner);
        let name = heap.alloc_string(&value::symbol_name(sym));
        out.push(heap.alloc_vector2(name, inner));
    }
    Ok(heap.list(out))
}

/// `(references-in-source name source)` — every occurrence of the global `name`
/// in `source`, as a list of `[line col]` (both 1-based), in document order. A
/// local that shadows the name is excluded. Pure: it parses the string and
/// holds no project state, so the Brood-side `callers` MCP tool maps it over a
/// project's files for cross-file find-references (ADR-031 §Cross-file,
/// docs/lsp.md). `name` may be a symbol or a string.
pub(super) fn references_in_source(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "reflect/references-in-source",
                "symbol or string",
                other,
            ));
        }
    };
    let src = expect_string(heap, "reflect/references-in-source", arg(args, 1))?;
    let root = cst::parse(&src);
    let tree = crate::syntax::scope::analyze(&root, &src);
    let starts = crate::syntax::scanner::line_starts(&src);
    let occ: Vec<Value> = tree
        .references_to_global(&root, &src, &name)
        .into_iter()
        .map(|span| {
            let (line, col) = line_col(&src, &starts, span.start as usize);
            heap.alloc_vector2(Value::int(line as i64), Value::int(col as i64))
        })
        .collect();
    Ok(heap.list(occ))
}

/// Globals whose call changes something OUTSIDE the evaluation that asked: files written,
/// moved or removed, programs run, signals sent, the clipboard, the runtime halted. What a
/// live evaluator must not run on a half-typed form — an electric pair closes
/// `(file/spit "notes.txt" "")` a moment before you type the text, and a buffer that
/// evaluates as you type would empty the file. Reads, prints and pure computation are not
/// here: running them early costs nothing. Syntactic, like the rest of `source-deps` — a
/// call through a function of your own is not seen.
const WORLD_EFFECTS: &[&str] = &[
    "file/spit",
    "file/spit-append",
    "file/replace",
    "file/spit-bytes",
    "file/spit-bytes-append",
    "file/spit-private",
    "file/set-mtime",
    "file/write-lines",
    "file/cp",
    "file/mkdir",
    "file/rename",
    "file/rm",
    "file/rmdir",
    "os/cmd",
    "os/run-process",
    "os/spawn",
    "os/spawn-pty",
    "os/write",
    "os/signal",
    "os/close",
    "os/clipboard-set",
    "system/halt",
];

/// The module-private def spellings, which are macros rather than special forms and so are
/// absent from [`SPECIAL_FORMS`] — still syntax, still not a dependency.
fn is_private_def_head(name: &str) -> bool {
    name == kw::DEF_PRIVATE || name == kw::DEFN_PRIVATE
}

/// `(source-deps src)` — per TOP-LEVEL form of `src`, in document order, what it defines
/// and what globals it uses: a list of `{:defines (…) :references (…)}` maps of name
/// strings.
///
/// The question a live evaluator has to answer to avoid re-running a whole buffer on every
/// keystroke: *given that form 2 changed, which of the forms below it could possibly be
/// affected?* Re-running everything below is the only correct answer without this, and it
/// makes a buffer of twenty forms cost twenty evaluations per edit. With it the answer is
/// the transitive closure of "references something a re-run form defines" — usually one
/// form, and never fewer than is correct.
///
/// Syntactic, and read the way find-references reads: locals are excluded (a parameter
/// named `map` is not a use of `map`), a plain `'…` quote is data rather than a use, and a
/// quasiquote's contents are counted. Two things it cannot see, both of which belong to the
/// caller's policy: a name introduced by a macro this pass does not expand, and a side
/// effect — a `spawn`, a table write, a print — through which one form reaches another
/// without naming anything.
///
/// Pure: it parses the string and holds no project state, exactly like
/// [`references_in_source`].
pub(super) fn source_deps(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "reflect/source-deps", arg(args, 0))?;
    let root = cst::parse(&src);
    let tree = crate::syntax::scope::analyze(&root, &src);
    let defines_kw = Value::Keyword(value::intern("defines"));
    let references_kw = Value::Keyword(value::intern("references"));
    let effects_kw = Value::Keyword(value::intern("effects"));
    let mut out = Vec::new();
    for form in root.forms() {
        let defines: Vec<Value> = crate::syntax::scope::globals_in(form, &src)
            .into_iter()
            .map(|b| heap.alloc_string(&b.name))
            .collect();
        let names: Vec<String> = crate::syntax::scope::global_refs_in(&tree, form, &src)
            .into_iter()
            // Syntax is not a dependency. `if`, `let`, `defn` and friends resolve as free
            // symbols here because nothing binds them, and listing them says nothing a
            // caller can act on — no form defines `defn`.
            .filter(|n| !SPECIAL_FORMS.contains(&n.as_str()) && !is_private_def_head(n))
            .collect();
        let effects: Vec<Value> = names
            .iter()
            .filter(|n| WORLD_EFFECTS.contains(&n.as_str()))
            .map(|n| heap.alloc_string(n))
            .collect();
        let references: Vec<Value> = names.iter().map(|n| heap.alloc_string(n)).collect();
        let defines = heap.list(defines);
        let references = heap.list(references);
        let effects = heap.list(effects);
        out.push(heap.map_from_pairs(vec![
            (defines_kw, defines),
            (references_kw, references),
            (effects_kw, effects),
        ]));
    }
    Ok(heap.list(out))
}

/// 1-based (line, col) of byte offset `b`, col counted in characters, over the
/// reader's line starts (`scanner::line_starts`, so a lone CR breaks a line here as
/// it does in every diagnostic). `b` must be a char boundary (CST spans always are).
fn line_col(src: &str, starts: &[u32], b: usize) -> (u32, u32) {
    let line = starts.partition_point(|&s| s as usize <= b) - 1; // 0-based
    let col = src[starts[line] as usize..b].chars().count();
    (line as u32 + 1, col as u32 + 1)
}

/// `(%fn-module f)` — the module whose source `f` was written in (ADR-596): the
/// [`Closure::module`](crate::core::value::Closure::module) the contract boundary already
/// keeps, so asking costs a field read. A top-level `defn` leaves that field empty — its
/// qualified name stands in for it at compile time — so its name's namespace answers.
pub(super) fn fn_module(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    Ok(match arg(args, 0) {
        Value::Fn(id) => {
            let closure = heap.closure(id);
            match (closure.module, closure.name) {
                (Some(module), _) => Value::Sym(module),
                (None, Some(name)) => {
                    let spelled = value::symbol_name(name);
                    match spelled.rfind('/') {
                        Some(slash) if slash > 0 => Value::Sym(value::intern(&spelled[..slash])),
                        _ => Value::Nil,
                    }
                }
                (None, None) => Value::Nil,
            }
        }
        _ => Value::Nil,
    })
}

pub(super) fn current_file(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.current_file().map(str::to_string) {
        Some(f) => Ok(heap.alloc_string(&f)),
        None => Ok(Value::nil()),
    }
}

// ---------- introspection (editor tooling; see docs/lsp.md) ----------

/// `(doc f)` — the docstring of a function or macro value, or `nil`. A docstring
/// is the leading string literal in a `fn`/`defn` body (stored on the closure
/// when more body follows it). Powers hover / `describe-function`.
pub(super) fn doc(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let text = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => heap.closure(id).doc.clone(),
        // A primitive's docstring lives on the `NativeFn` (the `PRIMITIVE_DOCS`
        // table), since it has no Brood body to carry a leading string.
        Value::Native(id) => {
            let d = heap.native(id).doc;
            (!d.is_empty()).then(|| d.to_string())
        }
        _ => None,
    };
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::nil()),
    }
}

/// `(arglist f)` — the parameter list of a function, macro, or primitive as a
/// list, mirroring the source surface: required names, then `&optional` names,
/// then `& rest`. `nil` for a non-function (or a primitive without recorded
/// params). Feeds signature help / hover.
/// `(reflect/arglists f)` — one parameter list per arm (see `arglist`, which shows the last).
/// The function a reflective question is about: the ORIGINAL when `f` is a contract shim
/// (ADR-381). A `sig`'d global holds a shim while contracts are armed — under `nest run` and
/// `nest test`, which is where an editor's help asks — and the shim's own parameters are the
/// template's `(a b)`, not the author's (ADR-560).
fn unshimmed(heap: &Heap, f: Value) -> Value {
    super::contracts::shim_original(heap, f).unwrap_or(f)
}

pub(super) fn arglists(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let id = match unshimmed(heap, arg(args, 0)) {
        Value::Fn(id) | Value::Macro(id) => id,
        Value::Native(_) => {
            let one = arglist(args, _env, heap)?;
            return Ok(if matches!(one, Value::Nil) {
                one
            } else {
                heap.list(vec![one])
            });
        }
        _ => return Ok(Value::nil()),
    };
    // As the source spells each arm (`ClosureArm::source_params`).
    let arms: Vec<Vec<value::Symbol>> = heap
        .closure(id)
        .arms
        .iter()
        .map(|arm| arm.source_params())
        .collect();
    let mut lists = Vec::with_capacity(arms.len());
    for params in arms {
        let items: Vec<Value> = params.into_iter().map(Value::Sym).collect();
        lists.push(heap.list(items));
    }
    Ok(heap.list(lists))
}

pub(super) fn arglist(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let id = match unshimmed(heap, arg(args, 0)) {
        Value::Fn(id) | Value::Macro(id) => id,
        // A primitive carries its param names as a flat `&'static` list (incl. any
        // `&`/`&optional` markers, already in order) — hand them back as symbols.
        Value::Native(id) => {
            let params = heap.native(id).params;
            if params.is_empty() {
                return Ok(Value::nil());
            }
            let items: Vec<Value> = params.iter().map(|p| value::sym(p)).collect();
            return Ok(heap.list(items));
        }
        _ => return Ok(Value::nil()),
    };
    // Copy the parts out before re-borrowing the heap mutably to build the list.
    // For a multi-arity closure there's no single arglist; show the last clause
    // (conventionally the most general — e.g. the variadic `(a b & more)`).
    // As the source spells it (`ClosureArm::source_params`).
    let params = heap
        .closure(id)
        .arms
        .last()
        .expect("closure has at least one arm")
        .source_params();
    let items: Vec<Value> = params.into_iter().map(Value::Sym).collect();
    Ok(heap.list(items))
}

/// `(reflect/global-names)` — a list of every symbol bound in the global table
/// (prelude + user `def`s), sorted by spelling so the order is deterministic
/// (for completion / workspace-symbol tooling and reproducible doc generation).
/// Special forms and the core control/binding macros — the keyword-like heads:
/// the single source of truth for "what reads as a keyword". Read from Brood via
/// the `(reflect/special-forms)` primitive (so `std/editor/highlight.blsp` highlights from this
/// list) and from the LSP (`semantic_tokens` / `completion` import it rather than
/// keeping a copy), so the runtime and the tooling can't drift. Mirrors
/// `brood.el`'s `brood-special-forms` plus the `def`-family heads.
pub const SPECIAL_FORMS: &[&str] = &[
    kw::IF,
    kw::DO,
    kw::DEF,
    kw::FN,
    kw::LET,
    kw::LETREC,
    kw::QUOTE,
    kw::QUASIQUOTE,
    kw::DEFMACRO,
    kw::DEFN,
    kw::DEFDYN,
    kw::DEFRECORD,
    kw::DEFABILITY,
    kw::IMPL,
    kw::DEFMODULE,
    kw::WHEN,
    kw::UNLESS,
    kw::COND,
    kw::AND,
    kw::OR,
    kw::MATCH,
    kw::MATCH_STAR,
    kw::CASE,
    kw::COMMENT,
    kw::TRY,
    kw::CATCH,
    kw::THROW,
    kw::RECEIVE,
    kw::BINDING,
    kw::DOLIST,
    kw::DOSEQ,
    kw::DOTIMES,
    kw::FOR,
    kw::THREAD_FIRST,
    // Core macros (std/prelude.blsp) that read as keywords — highlight-only, not
    // evaluator special forms (ADR-092). Promoted here so every editor (VS Code via
    // `nest grammar`, Emacs, the REPL highlighter) + the LSP colour them from one
    // source. `throw`/`receive` are already above (they're in the core set).
    kw::SPAWN,
    kw::SPAWN_LINK,
    kw::ERROR,
    kw::WITH_OUT_STR,
    kw::WITH_ERR_STR,
];

/// `(reflect/special-forms)` — the list of special-form / core-macro names (strings) that
/// read as keywords, for tooling (the highlighter, completion). Returns the
/// canonical `SPECIAL_FORMS`, so Brood and the LSP share one list.
pub(super) fn special_forms(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = SPECIAL_FORMS.iter().map(|s| heap.alloc_string(s)).collect();
    Ok(heap.list(items))
}

/// The `def…` heads whose form carries a DOCSTRING, paired with whether the head
/// builds a *function*. The single source of truth for "is this string documentation
/// or a value?", the way [`SPECIAL_FORMS`] is for "does this read as a keyword": read
/// from Brood via `(reflect/doc-forms)` (the REPL/editor highlighter `std/editor/highlight.blsp`
/// and the generated editor grammars in `std/tool/grammar.blsp`) and from the LSP
/// (`semantic_tokens`), so no editor keeps its own copy to drift.
///
/// In every one of these the docstring is the third form — right after the name —
/// or the fourth, when a parameter list comes between (`(defn f (x) "doc" …)` vs the
/// multi-arity `(defn f "doc" ((x) …) …)` and `(defmodule m "doc")`).
///
/// The flag is the one asymmetry: a *function*'s lone trailing string is its RETURN
/// VALUE, not documentation — `(defn greeting () "hello")` returns `"hello"`, and only
/// `(defn greeting () "doc" "hello")` documents it ([`crate::eval`]'s
/// `parse_closure_template` rule). A `defmodule` / `defability` docstring has no body
/// to follow it, so there the last form may well be the doc.
pub const DOC_FORMS: &[(&str, bool)] = &[
    (kw::DEFN, true),
    (kw::DEFN_PRIVATE, true),
    (kw::DEFMACRO, true),
    // `std/proc/gen.blsp`'s server macro — not a kernel head, but the same shape
    // (`(defserver name (state) "doc" clauses…)`), and it peels the doc the same way.
    ("defserver", true),
    (kw::DEFMODULE, false),
    (kw::DEFABILITY, false),
];

/// `(reflect/doc-forms)` — a map from `def…` head (string) to `:fn` (the head builds a
/// function, so a lone trailing string is the return value) or `:head` (the docstring
/// may be the form's last element). The canonical [`DOC_FORMS`], so the highlighter,
/// the generated grammars and the LSP share one list.
pub(super) fn doc_forms(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut pairs = Vec::with_capacity(DOC_FORMS.len());
    for &(name, fn_like) in DOC_FORMS {
        let key = heap.alloc_string(name);
        pairs.push((key, value::kw(if fn_like { "fn" } else { "head" })));
    }
    Ok(heap.map_from_pairs(pairs))
}

pub(super) fn global_names(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let mut syms = heap.global_symbols();
    // `symbol_name` locks the interner and allocates, so resolve each spelling
    // once (cached) rather than twice per comparison.
    syms.sort_by_cached_key(|&s| value::symbol_name(s));
    let syms: Vec<Value> = syms.into_iter().map(Value::Sym).collect();
    Ok(heap.list(syms))
}

/// `(bound? 'name)` — whether `name` is bound in the current scope (which
/// reaches the global table). Takes a symbol, so quote it: `(bound? 'foo)`.
pub(super) fn bound_p(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Sym(s) => Ok(Value::boolean(heap.env_get(env, s).is_some())),
        other => Err(LispError::wrong_type(heap, "bound?", "symbol", other)),
    }
}

/// `(%prelude-global? 'name)` — is `name` one of the globals the PRELUDE bound: its own
/// definitions, every native builtin (registered during the prelude build and frozen with
/// it), and the registries it seeds. The set is fixed at the freeze and identical in every
/// process of a binary. `stdimage/build` uses it to audit that every root global a std
/// module defines reached an owning section (KI-112): a root that is neither the prelude's
/// nor owned is one the probe missed.
pub(super) fn prelude_global_p(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "%prelude-global?", arg(args, 0))?;
    Ok(Value::boolean(heap.is_prelude_global(name)))
}

/// `(%global-generation 'name)` — the rebinding generation of a global: a number that
/// grows with every `def` of that name in this runtime, 0 for a name never `def`'d here.
/// For code that rebinds a global TEMPORARILY and must restore it without clobbering a
/// redefinition made in between (`debug/untrace-fn`).
pub(super) fn global_generation(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "%global-generation", arg(args, 0))?;
    Ok(Value::Int(heap.global_generation(name) as i64))
}

/// `(reflect/generation)` — the runtime's global generation now (ADR-603).
pub(super) fn runtime_generation(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::Int(heap.runtime_generation() as i64))
}

/// `(reflect/global-values)` — every rebindable global with its value (ADR-606).
pub(super) fn global_values(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let pairs = heap
        .rebindable_globals()
        .into_iter()
        .map(|(sym, value)| (Value::Sym(sym), value))
        .collect();
    // GC-safe: nothing evaluates between the read and the map's build (a builtin never
    // fires GC mid-execution).
    Ok(heap.map_from_pairs(pairs))
}

/// `(reflect/source-locations)` — every rebindable global's def site (ADR-606).
pub(super) fn source_locations(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let sites = heap.rebindable_def_sites();
    // A file's string is shared by every global defined in it (strings are immutable).
    let mut files: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    let mut pairs = Vec::with_capacity(sites.len());
    for (sym, loc) in sites {
        let file = match files.get(&loc.file) {
            Some(&file) => file,
            None => {
                let file = heap.alloc_string(&loc.file);
                files.insert(loc.file.clone(), file);
                file
            }
        };
        let site = heap.alloc_vector(vec![
            file,
            Value::int(loc.pos.line as i64),
            Value::int(loc.pos.col as i64),
        ]);
        pairs.push((Value::Sym(sym), site));
    }
    // GC-safe: nothing evaluates between the allocations and the map's build.
    Ok(heap.map_from_pairs(pairs))
}

/// `(reflect/set-source-location name location)` — put back (or forget) a def site (ADR-606).
pub(super) fn set_source_location(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    const WHO: &str = "reflect/set-source-location";
    let name = expect_symbol(heap, WHO, arg(args, 0))?;
    let name = root_escaped(name).unwrap_or(name);
    let location = arg(args, 1);
    if matches!(location, Value::Nil) {
        heap.forget_def_site(name);
        return Ok(location);
    }
    let items = match location.unpack() {
        value::ValueRef::Vector(id) => heap.vector(id).to_vec(),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                WHO,
                "[file line col] or nil",
                location,
            ));
        }
    };
    let [file, line, col] = items[..] else {
        return Err(LispError::wrong_type(
            heap,
            WHO,
            "[file line col] or nil",
            location,
        ));
    };
    let file = expect_string(heap, WHO, file)?;
    let line = super::args::expect_int(heap, WHO, line)?;
    let col = super::args::expect_int(heap, WHO, col)?;
    heap.set_def_site(
        name,
        crate::core::heap::SourceLoc {
            file,
            pos: crate::error::Pos {
                line: u32::try_from(line).unwrap_or(0),
                col: u32::try_from(col).unwrap_or(0),
            },
        },
    );
    Ok(location)
}

/// `(reflect/rebound-since generation)` — the globals stamped after `generation` (ADR-603).
pub(super) fn rebound_since(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let generation = match arg(args, 0).as_int() {
        Some(generation) => generation.max(0) as u64,
        None => {
            return Err(LispError::wrong_type(
                heap,
                "reflect/rebound-since",
                "int",
                arg(args, 0),
            ));
        }
    };
    let names: Vec<Value> = heap
        .rebound_since(generation)
        .into_iter()
        .map(Value::Sym)
        .collect();
    Ok(heap.list(names))
}

/// `(reflect/undef 'name)` — remove a global binding (ADR-391). Policy here, the table
/// operation and its invalidation in [`Heap::env_undefine`]: root a package-relative name
/// as `def` roots its target, refuse what Brood ships, and take a contracted function's
/// uncontracted alias (ADR-383) with it — the alias is bound exactly while the public
/// name is, and a same-module caller compiled against it must miss too.
pub(super) fn undef(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "reflect/undef", arg(args, 0))?;
    // The root escape names a global exactly, as it does for `def` (ADR-603): `/org/f` is the
    // global `org/f` even where a package context would root `org/f` to `bedit/org/f`.
    let name = root_escaped(name).unwrap_or_else(|| heap.root_qualified_ref(name).unwrap_or(name));
    if heap.is_reserved_global(name) || heap.is_prelude_global(name) {
        let shown = value::symbol_name(name);
        return Err(LispError::type_err(format!(
            "reflect/undef: `{shown}` is a reserved name — it ships with Brood and cannot be removed"
        ))
        .with_hint(
            "Reserved names are everything inside the `brood` binary: the prelude, the builtins, \
             and the embedded std modules. Your own definitions and your packages' can be removed."
                .to_string(),
        ));
    }
    let removed = heap.env_undefine(name);
    if let Some(alias) = crate::builtins::contracts::uncontracted_alias(name) {
        heap.env_undefine(alias);
    }
    Ok(Value::boolean(removed))
}

pub(super) fn gensym(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Str(id) => heap.string(id).to_string(),
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Nil => "g".to_string(),
        other => printer::display(heap, other),
    };
    Ok(value::gensym(&prefix))
}

/// `(%renames)` — the rename ledger as Brood data. See `crate::renames`.
pub(super) fn renames_ledger(_: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let mut pairs = Vec::with_capacity(crate::renames::RENAMES.len());
    for (old, new, adr) in crate::renames::RENAMES {
        let old_value = heap.alloc_string(old);
        let new_value = heap.alloc_string(new);
        let adr_value = heap.alloc_string(adr);
        let entry = heap.map_from_pairs(vec![
            (Value::Keyword(value::intern("to")), new_value),
            (Value::Keyword(value::intern("adr")), adr_value),
        ]);
        pairs.push((old_value, entry));
    }
    Ok(heap.map_from_pairs(pairs))
}
