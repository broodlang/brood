//! WASM component interop (ADR-071/145): the embedded `wasmtime` host.
//!
//! A package ships native code as a **WebAssembly component** — hash-pinned
//! `.wasm` data, never kernel code — and the runtime instantiates it
//! **sandboxed**: linear memory only (it cannot touch the Brood heap or
//! segfault the runtime), deny-by-default capabilities (no WASI wired in this
//! slice — pure compute), and **fuel-metered** calls (a runaway guest traps to
//! a catchable Brood error instead of wedging a scheduler worker).
//!
//! The boundary **marshals, never shares handles** (docs/interop.md): args
//! lower Brood → component values guided by the export's WIT type, results
//! lift back — scalars, strings, lists, tuples, options, records, results.
//! An instance is *mutable state*, so it follows the language's rule for
//! mutable state: an **opaque handle behind primitives** (an int token in
//! this slice), never a `Value` you can `send`. Calls serialize per instance
//! (a `Store` is single-threaded); different instances run concurrently —
//! including on the ADR-144 offload pool (`%wasm-call` is offload-safe:
//! handle + name + data args all cross as messages).
//!
//! Mechanism only — loading policy (paths, manifests, `use-native` binding)
//! lives in `std/wasm.blsp` and the package manager.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use wasmtime::component::{Component, Func, Linker, Type, Val};
use wasmtime::{Config, Engine, Store};

use crate::core::heap::Heap;
use crate::core::value::{self, Value};
use crate::error::{LispError, LispResult};

/// Fuel budget per call — generous for real compute (hundreds of millions of
/// abstract ops) while still bounding a runaway guest to well under a second.
const FUEL_PER_CALL: u64 = 2_000_000_000;

struct WasmInst {
    store: Store<()>,
    /// name → (callable, param types, result types), resolved at load.
    exports: HashMap<String, (Func, Box<[Type]>, Box<[Type]>)>,
}

static REGISTRY: LazyLock<Mutex<HashMap<u64, Arc<Mutex<WasmInst>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The shared engine: fuel metering on, Cranelift, built once.
fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| {
        let mut cfg = Config::new();
        cfg.consume_fuel(true);
        Engine::new(&cfg).expect("wasm engine")
    })
}

fn wasm_err(who: &str, e: impl std::fmt::Display) -> LispError {
    LispError::runtime(format!("{who}: {e}"))
}

/// Instantiate a component from source bytes (a compiled `.wasm` component or
/// WAT text — `wasmtime` accepts both). Returns the registry token.
pub fn load(src: &[u8]) -> Result<u64, LispError> {
    let engine = engine();
    let component = Component::new(engine, src).map_err(|e| wasm_err("%wasm-load", e))?;
    let mut store = Store::new(engine, ());
    // Instantiation may run start functions — meter it like a call.
    store
        .set_fuel(FUEL_PER_CALL)
        .map_err(|e| wasm_err("%wasm-load", e))?;
    let linker: Linker<()> = Linker::new(engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|e| wasm_err("%wasm-load", e))?;
    // Resolve every top-level function export now: types via the component
    // type, callables via the instance.
    let mut exports = HashMap::new();
    let sigs: Vec<(String, Box<[Type]>, Box<[Type]>)> = component
        .component_type()
        .exports(engine)
        .filter_map(|(name, item)| match item.ty {
            wasmtime::component::types::ComponentItem::ComponentFunc(f) => Some((
                name.to_string(),
                f.params().map(|(_, t)| t).collect(),
                f.results().collect(),
            )),
            _ => None,
        })
        .collect();
    for (name, params, results) in sigs {
        if let Some(func) = instance.get_func(&mut store, name.as_str()) {
            exports.insert(name, (func, params, results));
        }
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    REGISTRY
        .lock()
        .expect("wasm registry")
        .insert(id, Arc::new(Mutex::new(WasmInst { store, exports })));
    Ok(id)
}

/// The exported functions of instance `id`: `(name, arity)` pairs.
pub fn exports(id: u64) -> Result<Vec<(String, usize)>, LispError> {
    let inst = instance(id)?;
    let inst = inst.lock().expect("wasm instance");
    let mut out: Vec<(String, usize)> = inst
        .exports
        .iter()
        .map(|(n, (_, p, _))| (n.clone(), p.len()))
        .collect();
    out.sort();
    Ok(out)
}

/// Drop instance `id` (idempotent). The store — and everything the guest
/// owns — is freed when the last in-flight call releases it.
pub fn close(id: u64) {
    REGISTRY.lock().expect("wasm registry").remove(&id);
}

fn instance(id: u64) -> Result<Arc<Mutex<WasmInst>>, LispError> {
    REGISTRY
        .lock()
        .expect("wasm registry")
        .get(&id)
        .cloned()
        .ok_or_else(|| LispError::runtime("%wasm-call: no such wasm instance (already closed?)"))
}

/// Call export `name` of instance `id`. Marshals `args` by the export's WIT
/// parameter types, refuels, calls, lifts the results. One call at a time per
/// instance (the store is single-threaded); a trap — including out-of-fuel —
/// is a catchable error.
pub fn call(heap: &mut Heap, id: u64, name: &str, args: &[Value]) -> LispResult {
    let inst = instance(id)?;
    let mut inst = inst.lock().expect("wasm instance");
    let (func, param_tys, result_tys) = match inst.exports.get(name) {
        Some(entry) => (entry.0, entry.1.clone(), entry.2.clone()),
        None => {
            return Err(LispError::runtime(format!(
                "%wasm-call: no export `{name}` in this component (see %wasm-exports)"
            )))
        }
    };
    if args.len() != param_tys.len() {
        return Err(LispError::runtime(format!(
            "%wasm-call: `{name}` takes {} argument(s), got {}",
            param_tys.len(),
            args.len()
        )));
    }
    let mut lowered = Vec::with_capacity(args.len());
    for (v, t) in args.iter().zip(param_tys.iter()) {
        lowered.push(lower(heap, name, *v, t)?);
    }
    let store = &mut inst.store;
    store
        .set_fuel(FUEL_PER_CALL)
        .map_err(|e| wasm_err("%wasm-call", e))?;
    let mut results = vec![Val::Bool(false); result_tys.len()];
    func.call(&mut *store, &lowered, &mut results)
        .map_err(|e| wasm_err(&format!("%wasm-call `{name}`"), e))?;
    match results.len() {
        0 => Ok(Value::nil()),
        1 => lift(heap, name, &results[0]),
        _ => {
            let vals: Result<Vec<Value>, LispError> =
                results.iter().map(|r| lift(heap, name, r)).collect();
            Ok(heap.alloc_vector(vals?))
        }
    }
}

fn int_arg(heap: &Heap, who: &str, v: Value) -> Result<i64, LispError> {
    match v {
        Value::Int(n) => Ok(n),
        other => Err(LispError::wrong_type(heap, who, "integer", other)),
    }
}

fn range_err(who: &str, ty: &str, n: i64) -> LispError {
    LispError::runtime(format!(
        "{who}: {n} does not fit the component's `{ty}` parameter"
    ))
}

/// Lower one Brood value to a component value of the expected WIT type.
fn lower(heap: &mut Heap, who: &str, v: Value, ty: &Type) -> Result<Val, LispError> {
    Ok(match ty {
        Type::Bool => Val::Bool(!matches!(v, Value::Nil | Value::Bool(false))),
        Type::S8 => {
            let n = int_arg(heap, who, v)?;
            Val::S8(i8::try_from(n).map_err(|_| range_err(who, "s8", n))?)
        }
        Type::U8 => {
            let n = int_arg(heap, who, v)?;
            Val::U8(u8::try_from(n).map_err(|_| range_err(who, "u8", n))?)
        }
        Type::S16 => {
            let n = int_arg(heap, who, v)?;
            Val::S16(i16::try_from(n).map_err(|_| range_err(who, "s16", n))?)
        }
        Type::U16 => {
            let n = int_arg(heap, who, v)?;
            Val::U16(u16::try_from(n).map_err(|_| range_err(who, "u16", n))?)
        }
        Type::S32 => {
            let n = int_arg(heap, who, v)?;
            Val::S32(i32::try_from(n).map_err(|_| range_err(who, "s32", n))?)
        }
        Type::U32 => {
            let n = int_arg(heap, who, v)?;
            Val::U32(u32::try_from(n).map_err(|_| range_err(who, "u32", n))?)
        }
        Type::S64 => Val::S64(int_arg(heap, who, v)?),
        Type::U64 => {
            let n = int_arg(heap, who, v)?;
            Val::U64(u64::try_from(n).map_err(|_| range_err(who, "u64", n))?)
        }
        Type::Float32 => Val::Float32(number_arg(heap, who, v)? as f32),
        Type::Float64 => Val::Float64(number_arg(heap, who, v)?),
        Type::Char => match v {
            Value::Str(id) => {
                let s = heap.string(id).to_string();
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Val::Char(c),
                    _ => {
                        return Err(LispError::runtime(format!(
                            "{who}: a `char` parameter takes a 1-character string"
                        )))
                    }
                }
            }
            other => return Err(LispError::wrong_type(heap, who, "1-char string", other)),
        },
        Type::String => match v {
            Value::Str(id) => Val::String(heap.string(id).to_string()),
            other => return Err(LispError::wrong_type(heap, who, "string", other)),
        },
        Type::List(l) => {
            let elem = l.ty();
            let items = seq_items(heap, who, v)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(lower(heap, who, item, &elem)?);
            }
            Val::List(out)
        }
        Type::Tuple(t) => {
            let tys: Vec<Type> = t.types().collect();
            let items = seq_items(heap, who, v)?;
            if items.len() != tys.len() {
                return Err(LispError::runtime(format!(
                    "{who}: a {}-tuple parameter got {} element(s)",
                    tys.len(),
                    items.len()
                )));
            }
            let mut out = Vec::with_capacity(tys.len());
            for (item, t) in items.into_iter().zip(tys.iter()) {
                out.push(lower(heap, who, item, t)?);
            }
            Val::Tuple(out)
        }
        Type::Option(o) => match v {
            Value::Nil => Val::Option(None),
            some => Val::Option(Some(Box::new(lower(heap, who, some, &o.ty())?))),
        },
        other => {
            return Err(LispError::runtime(format!(
                "{who}: unsupported WIT parameter type {other:?} (this slice marshals \
                 scalars, strings, lists, tuples, and options)"
            )))
        }
    })
}

fn number_arg(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    match v {
        Value::Int(n) => Ok(n as f64),
        Value::Float(f) => Ok(f),
        other => Err(LispError::wrong_type(heap, who, "number", other)),
    }
}

/// Lift one component value back to Brood.
fn lift(heap: &mut Heap, who: &str, v: &Val) -> LispResult {
    Ok(match v {
        Val::Bool(b) => Value::Bool(*b),
        Val::S8(n) => Value::Int(*n as i64),
        Val::U8(n) => Value::Int(*n as i64),
        Val::S16(n) => Value::Int(*n as i64),
        Val::U16(n) => Value::Int(*n as i64),
        Val::S32(n) => Value::Int(*n as i64),
        Val::U32(n) => Value::Int(*n as i64),
        Val::S64(n) => Value::Int(*n),
        Val::U64(n) => match i64::try_from(*n) {
            Ok(n) => Value::Int(n),
            Err(_) => {
                return Err(LispError::runtime(format!(
                    "{who}: a u64 result ({n}) exceeds the integer range"
                )))
            }
        },
        Val::Float32(f) => Value::Float(*f as f64),
        Val::Float64(f) => Value::Float(*f),
        Val::Char(c) => heap.alloc_string(&c.to_string()),
        Val::String(s) => heap.alloc_string(s),
        Val::List(items) | Val::Tuple(items) => {
            let vals: Result<Vec<Value>, LispError> =
                items.iter().map(|i| lift(heap, who, i)).collect();
            heap.alloc_vector(vals?)
        }
        Val::Option(o) => match o {
            None => Value::nil(),
            Some(inner) => lift(heap, who, inner)?,
        },
        Val::Result(r) => match r {
            Ok(ok) => match ok {
                None => Value::nil(),
                Some(inner) => lift(heap, who, inner)?,
            },
            Err(err) => {
                let payload = match err {
                    None => Value::nil(),
                    Some(inner) => lift(heap, who, inner)?,
                };
                let msg = crate::syntax::printer::print(heap, payload);
                return Err(LispError::runtime(format!(
                    "{who}: the component returned an error: {msg}"
                )));
            }
        },
        Val::Enum(name) => Value::Keyword(value::intern(name)),
        Val::Record(fields) => {
            let mut pairs = Vec::with_capacity(fields.len());
            for (k, fv) in fields {
                let key = Value::Keyword(value::intern(k));
                pairs.push((key, lift(heap, who, fv)?));
            }
            heap.map_from_pairs(pairs)
        }
        Val::Variant(name, payload) => {
            let tag = Value::Keyword(value::intern(name));
            match payload {
                None => heap.alloc_vector(vec![tag]),
                Some(inner) => {
                    let p = lift(heap, who, inner)?;
                    heap.alloc_vector(vec![tag, p])
                }
            }
        }
        other => {
            return Err(LispError::runtime(format!(
                "{who}: unsupported WIT result type {other:?} in this slice"
            )))
        }
    })
}

/// The items of a vector or proper list — the sequence shapes a WIT `list`
/// parameter accepts.
fn seq_items(heap: &Heap, who: &str, v: Value) -> Result<Vec<Value>, LispError> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Vector(id) => Ok(heap.vector(id).to_vec()),
        Value::Pair(_) => {
            let mut out = Vec::new();
            let mut cur = v;
            loop {
                match cur {
                    Value::Pair(id) => {
                        let cell = heap.pair(id);
                        let (car, cdr) = (cell.0, cell.1);
                        out.push(car);
                        cur = cdr;
                    }
                    Value::Nil => break,
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            who,
                            "proper list or vector",
                            other,
                        ))
                    }
                }
            }
            Ok(out)
        }
        other => Err(LispError::wrong_type(heap, who, "list or vector", other)),
    }
}
