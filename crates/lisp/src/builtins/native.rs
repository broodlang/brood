//! Native numeric kernels (ADR-807, feature `native-kernels`): open a C-ABI library from the
//! project's own tree, call `int32_t name(const double *in, size_t, double *out, size_t)`,
//! close it. `crate::host::native` is the mechanism; the `native/*` policy is
//! `std/native.blsp`.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%native-open",
        Arity::exact(1),
        Sig::new(vec![string], int),
        &[],
        "",
        native_open,
    );
    primitives.def(
        "%native-call",
        Arity::exact(4),
        Sig::new(vec![int, string, any, any], any),
        &[],
        "",
        native_call,
    );
    primitives.def(
        "%native-close",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &[],
        "",
        native_close,
    );
    primitives.def(
        "%native-forget",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        &[],
        "",
        native_forget,
    );
}

/// `(%native-open path)` — load the kernel library at `path`; the token, owned by the calling
/// process and closed when it exits. Refused under a `_deps` directory, and while the
/// library's bytes are quarantined.
pub(super) fn native_open(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%native-open", arg(args, 0))?;
    crate::host::native::open(&path, crate::process::self_pid())
        .map(|token| Value::Int(token as i64))
}

/// `(%native-call token name input out)` — call entry point `name` on `input` (`host::numbers`'
/// shapes: numbers, vectors of numbers, or `{:records … :keys …}`), asking `out` doubles back:
/// an int for a flat vector of floats, `[count size]` for `count` vectors of `size`.
pub(super) fn native_call(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%native-call";
    let token = expect_int(heap, who, arg(args, 0))? as u64;
    let name = expect_string(heap, who, arg(args, 1))?;
    let items = crate::host::numbers::flat_items(heap, who, arg(args, 2))?;
    let input = items
        .into_iter()
        .map(|item| match item {
            Value::Float(f) => Ok(f),
            Value::Int(n) => Ok(n as f64),
            other => Err(LispError::wrong_type(heap, who, "number", other)),
        })
        .collect::<Result<Vec<f64>, _>>()?;
    let (out_count, row_size) = crate::host::numbers::out_shape(heap, who, arg(args, 3))?;
    let out = crate::host::native::call(token, &name, &input, out_count)?;
    Ok(crate::host::numbers::shaped_floats(heap, out, row_size))
}

/// `(%native-close token)` — close a kernel library.
pub(super) fn native_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let token = expect_int(heap, "%native-close", arg(args, 0))? as u64;
    crate::host::native::close(token).map(|()| Value::nil())
}

/// `(%native-forget path)` — clear the quarantine of the library at `path`; true when there
/// was one.
pub(super) fn native_forget(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%native-forget", arg(args, 0))?;
    crate::host::native::forget(&path).map(Value::Bool)
}
