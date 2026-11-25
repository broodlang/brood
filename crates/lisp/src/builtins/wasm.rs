//! WASM component interop (ADR-071/145, feature `wasm`): load, call, list the exports
//! of, and close a sandboxed guest. `crate::host::wasm` is the wasmtime host; the
//! `wasm/*` policy is `std/wasm.blsp`.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string};

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
        Arity::range(1, 2),
        Sig::with_rest(vec![any], int, any),
        &[],
        "",
        wasm_load,
    );
    #[cfg(feature = "wasm")]
    primitives.def(
        "%wasm-call",
        Arity::range(3, 4),
        Sig::with_rest(vec![int, string, any], any, any),
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
    #[cfg(feature = "wasm")]
    primitives.def(
        "%wasm-release",
        Arity::exact(2),
        Sig::new(vec![int, int], bool_ty),
        &[],
        "",
        wasm_release,
    );
}

/// `(%wasm-load content)` / `(%wasm-load content grants)` — instantiate a sandboxed WASM
/// component from `content`: a `bytes` value (a compiled `.wasm` component) or a string
/// (WAT text — handy for tests and the REPL). Returns the instance token. The calling
/// process owns the instance: it is closed when that process exits. `grants` (a map, or
/// nil for none) links WASI preview 2 — see [`grants_arg`].
pub(super) fn wasm_load(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let bytes: Vec<u8> = expect!(heap, "%wasm-load", arg(args, 0),
        "bytes (a compiled .wasm component) or string (WAT source)",
        Value::Bytes(id) => heap.bytes(id).as_bytes().to_vec(),
        Value::Str(id) => heap.string(id).as_bytes().to_vec(),
    )?;
    let grants = grants_arg(heap, arg(args, 1))?;
    crate::host::wasm::load(&bytes, crate::process::self_pid(), grants.as_ref())
        .map(|id| Value::Int(id as i64))
}

/// The grants map (ADR-453): `:dirs` a vector of `[host guest]` or `[host guest :write]`
/// (read-only unless `:write`), `:env` a map of string to string, `:args` a vector of
/// strings, `:stdio` true to pass the guest's stdout and stderr through. Nil is no grants
/// at all, so WASI is not linked; an empty map links it with only clocks and randomness.
fn grants_arg(heap: &Heap, v: Value) -> Result<Option<crate::host::wasm::Grants>, LispError> {
    const WHO: &str = "%wasm-load";
    let map = match v {
        Value::Nil => return Ok(None),
        Value::Map(id) => id,
        other => return Err(LispError::wrong_type(heap, WHO, "grants map or nil", other)),
    };
    let key = |name: &str| heap.map_get(map, Value::Keyword(value::intern(name)));
    let items = |field: Value| -> Result<Vec<Value>, LispError> {
        match field {
            Value::Nil => Ok(Vec::new()),
            Value::Vector(id) => Ok(heap.vector(id).to_vec()),
            other => Err(LispError::wrong_type(heap, WHO, "vector", other)),
        }
    };
    let mut grants = crate::host::wasm::Grants::default();
    for dir in items(key("dirs").unwrap_or(Value::Nil))? {
        let parts = items(dir)?;
        let writable = match parts.get(2) {
            None => false,
            Some(Value::Keyword(k)) if *k == value::intern("write") => true,
            Some(_) => {
                return Err(LispError::runtime(format!(
                    "{WHO}: a :dirs entry is [host guest] or [host guest :write]"
                )));
            }
        };
        match (parts.first(), parts.get(1)) {
            (Some(host), Some(guest)) if parts.len() <= 3 => grants.dirs.push((
                expect_string(heap, WHO, *host)?,
                expect_string(heap, WHO, *guest)?,
                writable,
            )),
            _ => {
                return Err(LispError::runtime(format!(
                    "{WHO}: a :dirs entry is [host guest] or [host guest :write]"
                )));
            }
        }
    }
    match key("env").unwrap_or(Value::Nil) {
        Value::Nil => {}
        Value::Map(env) => {
            for (name, value) in heap.map_entries(env) {
                grants.env.push((
                    expect_string(heap, WHO, name)?,
                    expect_string(heap, WHO, value)?,
                ));
            }
        }
        other => return Err(LispError::wrong_type(heap, WHO, ":env map", other)),
    }
    for argument in items(key("args").unwrap_or(Value::Nil))? {
        grants.args.push(expect_string(heap, WHO, argument)?);
    }
    grants.stdio = !matches!(key("stdio"), None | Some(Value::Nil | Value::Bool(false)));
    Ok(Some(grants))
}

/// `(%wasm-call inst name args)` / `(%wasm-call inst name args size)` — call export `name` of
/// instance `inst` with `args` (a vector), marshalled by the export's WIT types. Fuel-metered.
/// With `size`, a list result comes back as vectors of `size` items (`wasm/call-shaped`,
/// ADR-810): matrices ready to use, with no split in Brood.
pub(super) fn wasm_call(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-call", arg(args, 0))? as u64;
    let name = expect_string(heap, "%wasm-call", arg(args, 1))?;
    let call_args: Vec<Value> = expect!(heap, "%wasm-call", arg(args, 2), "vector of arguments",
        Value::Vector(vid) => heap.vector(vid).to_vec(),
        Value::Nil => Vec::new(),
    )?;
    let result = crate::host::wasm::call(heap, id, &name, &call_args)?;
    match arg(args, 3) {
        Value::Nil => Ok(result),
        size => {
            let size = expect_int(heap, "%wasm-call", size)?;
            let size = usize::try_from(size).unwrap_or(0);
            crate::host::numbers::chunked(heap, "%wasm-call", result, size)
        }
    }
}

/// `(%wasm-exports inst)` — the instance's exported functions, as a vector of
/// `[name arity]` pairs (sorted by name).
pub(super) fn wasm_exports(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-exports", arg(args, 0))? as u64;
    let entries = crate::host::wasm::exports(id)?;
    let mut out = Vec::with_capacity(entries.len());
    for (name, arity) in entries {
        let n = heap.alloc_string(&name);
        out.push(heap.alloc_vector2(n, Value::Int(arity as i64)));
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

/// `(%wasm-release inst resource)` — drop a guest resource the host holds, running the
/// guest's destructor. True if it was held, false if already released or moved into the
/// guest.
pub(super) fn wasm_release(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-release", arg(args, 0))? as u64;
    let token = expect_int(heap, "%wasm-release", arg(args, 1))? as u64;
    crate::host::wasm::release(id, token).map(Value::Bool)
}
