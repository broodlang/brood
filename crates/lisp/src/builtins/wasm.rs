//! WASM component interop (ADR-071/145, feature `wasm`): load, call, list the exports
//! of, and close a sandboxed guest. `crate::host::wasm` is the wasmtime host; the
//! `wasm/*` policy is `std/wasm.blsp`.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::{arg, expect_int, expect_string};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // WASM component interop (ADR-071/145, feature `wasm`): the sandboxed
    // native-extension host. Policy — file loading, `use-native` binding —
    // lives in std/wasm.blsp.
    #[cfg(feature = "wasm")]
    primitives.def(
        "%wasm-load",
        Arity::exact(1),
        Sig::new(vec![any], int),
        &[],
        "",
        wasm_load,
    );
    #[cfg(feature = "wasm")]
    primitives.def(
        "%wasm-call",
        Arity::exact(3),
        Sig::new(vec![int, string, any], any),
        &[],
        "",
        wasm_call,
    );
    #[cfg(feature = "wasm")]
    primitives.def(
        "%wasm-exports",
        Arity::exact(1),
        Sig::new(vec![int], any),
        &[],
        "",
        wasm_exports,
    );
    #[cfg(feature = "wasm")]
    primitives.def(
        "%wasm-close",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &[],
        "",
        wasm_close,
    );
}

/// `(%wasm-load content)` — instantiate a sandboxed WASM component from
/// `content`: a `bytes` value (a compiled `.wasm` component) or a string (WAT
/// text — handy for tests and the REPL). Returns the instance token.
pub(super) fn wasm_load(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let bytes: Vec<u8> = match arg(args, 0) {
        Value::Bytes(id) => heap.bytes(id).as_bytes().to_vec(),
        Value::Str(id) => heap.string(id).as_bytes().to_vec(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%wasm-load",
                "bytes (a compiled .wasm component) or string (WAT source)",
                other,
            ))
        }
    };
    crate::host::wasm::load(&bytes).map(|id| Value::Int(id as i64))
}

/// `(%wasm-call inst name args)` — call export `name` of instance `inst` with
/// `args` (a vector), marshalled by the export's WIT types. Fuel-metered.
pub(super) fn wasm_call(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-call", arg(args, 0))? as u64;
    let name = expect_string(heap, "%wasm-call", arg(args, 1))?.to_string();
    let call_args: Vec<Value> = match arg(args, 2) {
        Value::Vector(vid) => heap.vector(vid).to_vec(),
        Value::Nil => Vec::new(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%wasm-call",
                "vector of arguments",
                other,
            ))
        }
    };
    crate::host::wasm::call(heap, id, &name, &call_args)
}

/// `(%wasm-exports inst)` — the instance's exported functions, as a vector of
/// `[name arity]` pairs (sorted by name).
pub(super) fn wasm_exports(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-exports", arg(args, 0))? as u64;
    let entries = crate::host::wasm::exports(id)?;
    let mut out = Vec::with_capacity(entries.len());
    for (name, arity) in entries {
        let n = heap.alloc_string(&name);
        out.push(heap.alloc_vector(vec![n, Value::Int(arity as i64)]));
    }
    Ok(heap.alloc_vector(out))
}

/// `(%wasm-close inst)` — drop the instance (idempotent); the sandbox and
/// everything the guest owns is freed. Returns nil.
pub(super) fn wasm_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-close", arg(args, 0))? as u64;
    crate::host::wasm::close(id);
    Ok(Value::nil())
}
