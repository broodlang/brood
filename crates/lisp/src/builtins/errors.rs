// Extracted from system.rs (file-organization split).
#![allow(unused_imports)]
use super::*;
use super::system::*;
use super::numeric::{arg, expect_int, expect_string, expect_symbol};
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::{cst, printer, reader};
use crate::eval::compile::apply_engine;

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

