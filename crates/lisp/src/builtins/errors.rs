use super::args::{arg, expect_string};
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::compile::apply_engine;
// Only the debug-only `%force-panic` renders a value; a release build has no use for it.
#[cfg(debug_assertions)]
use crate::syntax::printer;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    // errors / control
    primitives.def(
        "throw",
        Arity::exact(1),
        Sig::new(vec![any], Ty::NEVER),
        &["x"],
        "Raise x as an error - a non-local exit caught by try/catch.\n\n    (try (throw :boom) (catch e e))                 ;=> :boom\n    (try (throw {:code 7}) (catch e (:code e)))     ;=> 7",
        throw,
    );
    // A *returned* failure — the other channel. `throw` unwinds (bugs, the
    // unexpected); a failure is handed back as a value (input this function
    // cannot interpret), is falsy, and carries its own message.
    primitives.def(
        "%failure",
        Arity::range(1, 2),
        Sig::new(vec![string, map_ty.union(nil_ty)], Ty::of(Tag::Failure)),
        &["message", "fields"],
        "Build a failure value carrying message and, optionally, the fields of a map (which must not name `:message`). The primitive behind the prelude's `failure` and `failure/with`.",
        failure_new,
    );
    primitives.def(
        "%failure-message",
        Arity::exact(1),
        Sig::new(vec![any], string.union(nil_ty)),
        &["f"],
        "The message a failure carries, else nil. Behind the prelude's `error-message`.",
        failure_message,
    );
    primitives.def(
        "%failure-fields",
        Arity::exact(1),
        Sig::new(vec![any], map_ty),
        &["f"],
        "The fields of failure f as a plain map, `:message` among them; raises for anything that is not a failure.",
        failure_fields,
    );
    // `%force-panic` — deliberately panics the Rust thread when called. Exists
    // *only* in debug builds: it gives the MCP-host panic-isolation regression
    // test a reliable trigger without adding a "intentionally crash" knob to
    // the release surface. `cargo test` (and `nest test` against a debug
    // binary) sees it; `--release` binaries don't.
    #[cfg(debug_assertions)]
    primitives.def(
        "%force-panic",
        Arity::range(0, 1),
        Sig::new(vec![any], Ty::NEVER),
        &[],
        "",
        force_panic,
    );
    // Shared-blob inspection primitives — debug-only because they leak the
    // representation (a raw pointer) and because they only exist to assert
    // identity / leak-freedom across processes in the blob-share test. Both
    // return `nil` for an inline string or a non-LOCAL handle (PRELUDE/RUNTIME).
    #[cfg(debug_assertions)]
    primitives.def(
        "%blob-ptr",
        Arity::exact(1),
        Sig::new(vec![string], Ty::ANY),
        &[],
        "",
        blob_ptr,
    );
    #[cfg(debug_assertions)]
    primitives.def(
        "%blob-strong-count",
        Arity::exact(1),
        Sig::new(vec![string], Ty::ANY),
        &[],
        "",
        blob_strong_count,
    );
    primitives.def(
        "%raise",
        Arity::exact(1),
        Sig::new(vec![map_ty], Ty::NEVER),
        &["error"],
        "Raise the error map `error` — `{:kind k :message s :code c …}`, `c` one of the codes the language raises itself (`E0090` `error`, `E0091` `assert`, `E0092` a failed match). A catch binds the map with the `:file`/`:line`/`:col` and `:trace` of the raise added, as for a built-in error; uncaught it renders as `error: <message>`. An `:at form` entry places the error at `form`'s source position instead (and is removed from the map). The primitive behind `error`, `assert` and a failed pattern (ADR-501).",
        raise,
    );
    primitives.def(
        "%try",
        Arity::exact(2),
        Sig::new(vec![callable, callable], any),
        &[],
        "",
        try_catch,
    );
    primitives.def(
        "%try-described",
        Arity::exact(2),
        Sig::new(vec![callable, callable], any),
        &["thunk", "handler"],
        "Call thunk; if it raises, call (handler caught error): caught is what `catch` binds, error the structured description {:kind :message [:code :file :line :col :hint :trace]} of the raise — for any thrown value, a bare `throw` included.",
        try_described,
    );
    primitives.def(
        "%make-macro",
        Arity::exact(1),
        Sig::new(vec![callable], any),
        &["f"],
        "Tag fn f as a macro: the expander calls it on the unevaluated argument forms and splices its result in place. The `defmacro` macro lowers to this.",
        make_macro);
}

// ---------- errors / control ----------

/// `(%make-macro f)` — tag the closure `f` as a macro: the expander calls it on
/// the *unexpanded* argument forms and splices the result in place of the call.
/// The `defmacro` macro (std/prelude.blsp) lowers to this, so macro definition is
/// plain Brood over a one-line primitive rather than its own core special form.
pub(super) fn make_macro(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    expect!(heap, "%make-macro", arg(args, 0), "fn",
        Value::Fn(id) => Value::macro_(id),
    )
}

/// `(%raise m)` — raise the error map `m` with the code its `:code` names, so a catch binds `m`
/// enriched with position and trace (`LispError::caught_value`). A code outside
/// [`error_codes::raised_map_code`] is a type error: a program's own error map is `throw`n.
pub(super) fn raise(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::core::value::intern;
    use crate::error::error_codes;
    let error = arg(args, 0);
    let code = match error {
        Value::Map(map) => match heap.map_get(map, Value::keyword(intern("code"))) {
            Some(Value::Str(id)) => error_codes::raised_map_code(heap.string(id)),
            _ => None,
        },
        _ => None,
    };
    // `:at form` locates the error at `form`'s source position rather than the call that
    // raised it — how a macro points at the sub-form it is complaining about. The form is
    // taken out of the map: it is a place, not part of what a handler is handed.
    let at_key = Value::keyword(intern("at"));
    let (error, at) = match (code, error) {
        (Some(_), Value::Map(map)) => match heap.map_get(map, at_key) {
            Some(form) => (heap.map_dissoc(map, at_key), heap.form_pos(form)),
            None => (error, None),
        },
        _ => (error, None),
    };
    match code {
        Some(code) => {
            let mut raised = LispError::thrown(error, heap).with_code(code);
            if let Some((pos, file)) = at {
                raised = raised.with_pos(pos);
                // the form's own file: the position means nothing against another one
                if let Some(file) = file {
                    raised.file = Some(file.to_string());
                }
            }
            Err(raised)
        }
        None => Err(LispError::wrong_type(
            heap,
            "%raise",
            "an error map whose :code the language raises (E0090, E0091, E0092)",
            error,
        )),
    }
}

/// `(throw v)`. A `v` a handler caught is raised again from where it was first raised —
/// its position, file and trace — rather than from here (KI-291, `Heap::note_caught`).
pub(super) fn throw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let error = LispError::thrown(v, heap);
    if error.pos.is_none() && heap.has_caught_origins() {
        return Err(with_caught_origin(error, v, heap));
    }
    Err(error)
}

/// `throw`'s rethrow half: `error` (raising `v`) located where `v` was first raised, when a
/// handler caught it ([`Heap::take_caught_origin`]).
#[cold]
#[inline(never)]
fn with_caught_origin(mut error: LispError, v: Value, heap: &mut Heap) -> LispError {
    if let Some(origin) = heap.take_caught_origin(v) {
        // Caught before any form tagged it (the tree-walker raising inside a vector
        // literal): the innermost frame's call site is where it was raised. Left empty,
        // the rethrow site's form would claim it.
        let site = origin.trace.iter().find(|f| f.pos.is_some());
        error.pos = origin.pos.or_else(|| site.and_then(|f| f.pos));
        error.file = match (&origin.file, origin.pos) {
            (Some(file), _) => Some(file.clone()),
            (None, None) => site.and_then(|f| f.file.as_deref().map(str::to_string)),
            (None, Some(_)) => None,
        };
        error.trace = origin.trace;
    }
    error
}

/// `(%failure message [fields])` — build a **failure** value. The kernel primitive behind
/// the prelude's `failure` and `failure/with`: a failure is its own `Value` kind, so Brood
/// cannot construct one without a primitive (the language has no way to make a new tag).
/// `fields` adds the caller's own keys beside `:message`, which it may not name: the
/// message is always the one the parts spelled.
pub(super) fn failure_new(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let message = expect_string(heap, "%failure", arg(args, 0))?;
    let fields = match args.get(1).copied().unwrap_or(Value::nil()) {
        Value::Nil => return Ok(heap.alloc_failure(&message)),
        Value::Map(fields) => fields,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%failure",
                "map of fields",
                other,
            ));
        }
    };
    let key = Value::Keyword(crate::core::value::intern("message"));
    if heap.map_get(fields, key).is_some() {
        return Err(LispError::runtime(
            "failure/with: the fields may not name :message; the message parts are the message",
        ));
    }
    let msg = heap.alloc_string(&message);
    match heap.map_assoc(fields, key, msg) {
        Value::Map(id) => Ok(Value::failure(id)),
        other => Ok(other),
    }
}

/// `(%failure-fields f)` — every field a failure carries, `:message` among them, as a plain
/// map. A non-failure raises; the prelude's `fields` asks `failure?` first.
pub(super) fn failure_fields(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Failure(id) => Ok(Value::map(id)),
        other => Err(LispError::wrong_type(
            heap,
            "%failure-fields",
            "failure",
            other,
        )),
    }
}

/// `(%failure-message f)` — the `:message` a failure carries, else `nil`. A failure is
/// deliberately NOT a collection (`get`/`count`/`seq` reject it), so this is the one
/// way in; the prelude's `error-message` is what users call.
pub(super) fn failure_message(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let Value::Failure(id) = arg(args, 0) else {
        return Ok(Value::nil());
    };
    let key = Value::Keyword(crate::core::value::intern("message"));
    Ok(heap.map_get(id, key).unwrap_or(Value::nil()))
}

/// `(%force-panic [msg])` — debug-only. Deliberately panics from a primitive,
/// so tests can exercise the host-side `catch_unwind` boundary (currently the
/// MCP server's `call_tool`). Not a Brood-clean error path — this *is* a Rust
/// `panic!`; if no host catches it, the process dies. There's no Brood
/// reason to call this outside the regression test.
#[cfg(debug_assertions)]
pub(super) fn force_panic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let msg = match args.first() {
        Some(Value::Str(id)) => heap.string(*id).to_string(),
        Some(other) => printer::display(heap, *other),
        None => "%force-panic invoked (no message)".to_string(),
    };
    panic!("{}", msg);
}

/// `(%blob-ptr s)` — debug-only. The raw `SharedBlob` address backing `s`,
/// as an integer (for identity comparison across processes). `nil` for
/// inline (small) strings and PRELUDE/RUNTIME handles.
#[cfg(debug_assertions)]
pub(super) fn blob_ptr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    expect!(heap, "%blob-ptr", arg(args, 0), "string",
        Value::Str(id) => heap
            .local_shared_blob_ptr(id)
            .map(|p| Value::int(p as i64))
            .unwrap_or(Value::nil()),
    )
}

/// `(%blob-strong-count s)` — debug-only. Current `Arc::strong_count` for
/// the `SharedBlob` backing `s`. `nil` for inline / non-LOCAL strings.
/// Approximate under live concurrent senders/receivers (the count moves);
/// stable when callers are quiescent (what the leak-check test asserts).
#[cfg(debug_assertions)]
pub(super) fn blob_strong_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    expect!(heap, "%blob-strong-count", arg(args, 0), "string",
        Value::Str(id) => heap
            .local_shared_blob_strong_count(id)
            .map(|n| Value::int(n as i64))
            .unwrap_or(Value::nil()),
    )
}

pub(super) fn try_catch(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    run_guarded(args, env, heap, |error, heap| {
        handler_args(error, heap, false)
    })
}

/// What a `%try` handler is called with for `error`: what `catch` binds, and for
/// `%try-described` also the raise's structured description. Shared with the tree-walker's
/// `Try` frame. No rooting needed between the two allocations: allocation never collects
/// (the GC fires only at eval safepoints).
pub(crate) fn handler_args(error: LispError, heap: &mut Heap, described: bool) -> Vec<Value> {
    let caught = error.caught_value(heap);
    let argv = if described {
        vec![caught, error.to_value_map(heap)]
    } else {
        vec![caught]
    };
    heap.note_caught(error, caught);
    argv
}

/// `(%try-described thunk handler)` — `%try` whose handler also receives the raise's
/// structured description (`LispError::to_value_map`). A `catch` binds a user `throw`'s
/// value verbatim, which has nowhere to carry the position and trace the error collected
/// as it unwound; this is the one way to read them for ANY thrown value. Never inlined by
/// the compiler (its handler takes two parameters), so it always runs here.
pub(super) fn try_described(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    run_guarded(args, env, heap, |error, heap| {
        handler_args(error, heap, true)
    })
}

/// Run `args[0]` (a thunk); on a raise, call `args[1]` with the arguments `handler_args`
/// builds from the error. A control signal is never caught.
fn run_guarded(
    args: &[Value],
    env: EnvId,
    heap: &mut Heap,
    handler_args: impl FnOnce(LispError, &mut Heap) -> Vec<Value>,
) -> LispResult {
    let thunk = arg(args, 0);
    let handler = arg(args, 1);
    // The thunk runs through `apply`, which can collect at ANY eval depth
    // (ADR-061). On the error path we still need `handler` and `env` afterwards,
    // so root them on the operand stack across the thunk and re-read the
    // relocated handles. (The thrown value / built error map is fresh after the
    // unwind — no safepoint runs while an `Err` propagates — so it needs no
    // rooting.) This is the `(try (loop) (catch e …))` supervised-server shape.
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_root(handler);
    heap.push_env_root(env);
    let outcome = apply_engine(heap, thunk, &[], env);
    let handler = heap.root_at(vb);
    let env = heap.env_root_at(eb);
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    match outcome {
        Ok(value) => Ok(value),
        // A control signal (a `receive` suspend, ADR-100 §7) is **not** an error —
        // re-raise it untouched so it reaches the bytecode driver / scheduler. `%try`
        // must never catch it: it isn't a `throw`/error, and unwinding to the handler
        // here would discard the captured continuation the suspend means to resume.
        Err(e) if e.is_control() => Err(e),
        Err(e) => {
            // The catch sees:
            //   * the user-thrown value verbatim, if there is one (preserves the
            //     "throw shape == catch shape" contract — `(throw 42)` → 42);
            //   * **a structured map** for any built-in error, so Brood code (and
            //     agents via MCP) can `(case (get e :kind) :unbound …)` without
            //     parsing strings (`docs/archive/llm-native.md` §4). Shape on
            //     `LispError::to_value_map`: `{:kind :message [:code] [:file
            //     :line :col] [:hint]}`.
            let handler_argv = handler_args(e, heap);
            apply_engine(heap, handler, &handler_argv, env)
        }
    }
}
