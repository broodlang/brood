use super::numeric::{arg, expect_string};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
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
        "Raise x as an error - a non-local exit caught by try/catch.",
        throw,
    );
    // A *returned* failure — the other channel. `throw` unwinds (bugs, the
    // unexpected); a failure is handed back as a value (input this function
    // cannot interpret), is falsy, and carries its own message.
    primitives.def(
        "%failure",
        Arity::exact(1),
        Sig::new(vec![string], Ty::of(Tag::Failure)),
        &["message"],
        "Build a failure value carrying message. The primitive behind the prelude's `failure`.",
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
        "%try",
        Arity::exact(2),
        Sig::new(vec![callable, callable], any),
        &[],
        "",
        try_catch,
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
pub(super) fn make_macro(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Fn(id) => Ok(Value::macro_(id)),
        other => Err(LispError::type_err(format!(
            "%make-macro: expected a fn, got {}",
            value::tag(other).name()
        ))),
    }
}

pub(super) fn throw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Err(LispError::thrown(arg(args, 0), heap))
}

/// `(%failure message)` — build a **failure** value. The kernel primitive behind the
/// prelude's `failure`: a failure is its own `Value` kind, so Brood cannot construct
/// one without a primitive (the language has no way to make a new tag).
pub(super) fn failure_new(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let message = expect_string(heap, "%failure", arg(args, 0))?;
    Ok(heap.alloc_failure(&message))
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
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_ptr(id)
            .map(|p| Value::int(p as i64))
            .unwrap_or(Value::nil())),
        other => Err(LispError::type_err(format!(
            "%blob-ptr: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

/// `(%blob-strong-count s)` — debug-only. Current `Arc::strong_count` for
/// the `SharedBlob` backing `s`. `nil` for inline / non-LOCAL strings.
/// Approximate under live concurrent senders/receivers (the count moves);
/// stable when callers are quiescent (what the leak-check test asserts).
#[cfg(debug_assertions)]
pub(super) fn blob_strong_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_strong_count(id)
            .map(|n| Value::int(n as i64))
            .unwrap_or(Value::nil())),
        other => Err(LispError::type_err(format!(
            "%blob-strong-count: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

pub(super) fn try_catch(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
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
            //     parsing strings (`docs/llm-native.md` §4). Shape on
            //     `LispError::to_value_map`: `{:kind :message [:code] [:file
            //     :line :col] [:hint]}`.
            let caught = match e.payload {
                Some(v) => v,
                None => e.to_value_map(heap),
            };
            apply_engine(heap, handler, &[caught], env)
        }
    }
}
