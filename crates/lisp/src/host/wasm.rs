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
//! this slice), never a `Value` you can `send`. The token is a random 62-bit
//! capability, not a counter — holding it is the authority to use the
//! instance, so a process can only reach an instance it was handed, never one
//! it guessed. An instance is **owned by the process that loaded it** and is
//! closed when that process retires (`close_process_instances`, beside the
//! socket and subprocess sweeps), so a crashed loader cannot strand gigabytes
//! of linear memory for the runtime's life. Calls serialize per instance
//! (a `Store` is single-threaded); different instances run concurrently —
//! including on the ADR-144 offload pool (`%wasm-call` is offload-safe:
//! handle + name + data args all cross as messages).
//!
//! Mechanism only — loading policy (paths, manifests, `use-native` binding)
//! lives in `std/wasm.blsp` and the package manager.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use wasmtime::component::{Component, Func, Linker, Type, Val};
use wasmtime::{Config, Engine, ResourceLimiter, Store};

use crate::core::heap::Heap;
use crate::core::value::{self, Value};
use crate::error::{LispError, LispResult};
use crate::host::owned::OwnedTable;

/// Fuel budget per call — generous for real compute (hundreds of millions of
/// abstract ops) while still bounding a runaway guest to well under a second.
const FUEL_PER_CALL: u64 = 2_000_000_000;

/// Per-instance space cap, summed over EVERY linear memory and table the guest
/// owns. Fuel meters *instructions*, not space — without this a component that
/// declares a huge `memory`/`table` or runs one `memory.grow` could OOM the host
/// for ~1 fuel unit. 256 MiB is generous for a codec/parser (the intended use)
/// while keeping one guest from exhausting host RAM; a real consumer that needs
/// more sets it deliberately later.
///
/// **This is a whole-store budget, not a per-memory one, and it must stay that
/// way.** `wasmtime`'s off-the-shelf `StoreLimits::memory_size` applies to each
/// memory *individually* and leaves `table_elements` unlimited, so a component
/// declaring 20 core instances of a 256 MiB memory sailed through it at 5.1 GB
/// RSS, and a single `(table 100000000 funcref)` — never charged at all — at
/// 851 MB. Both are one `%wasm-load` away, and a host OOM is a SIGKILL no Brood
/// `try` can catch. Hence [`GuestBudget`] below rather than `StoreLimits`.
const MAX_GUEST_BYTES: usize = 256 * 1024 * 1024;
/// Cap the source handed to `Component::new` — compiling/validating an
/// arbitrarily large blob is unmetered CPU + memory at load time.
const MAX_COMPONENT_BYTES: usize = 64 * 1024 * 1024;
/// How many compiled components [`COMPILED`] keeps. A caller reloads a handful of
/// distinct components, not hundreds; each entry pins its machine code for as
/// long as it stays cached, so the bound is small.
const COMPILED_CACHE_ENTRIES: usize = 4;
/// How many linear memories / tables one component may own. The byte budget
/// already bounds their total *size*; these bound the per-object bookkeeping
/// (and the multi-GiB virtual reservation wasmtime makes per memory), which the
/// budget cannot see. Far above anything a real component declares — wasmtime's
/// own default is 10 000 each.
const MAX_GUEST_MEMORIES: usize = 128;
const MAX_GUEST_TABLES: usize = 128;
/// wasmtime charges one pointer of host storage per table element.
const TABLE_ELEM_BYTES: usize = std::mem::size_of::<usize>();

/// A whole-store space budget: every memory and table growth in this instance is
/// charged against one [`MAX_GUEST_BYTES`] pot. See that constant for why the
/// stock `StoreLimits` is not enough.
#[derive(Default)]
struct GuestBudget {
    /// Bytes currently charged across all of this store's memories and tables.
    used: usize,
    /// The delta the most recent approval charged, so a growth wasmtime then
    /// fails for its own reasons can be refunded (`*_grow_failed`).
    pending: usize,
}

impl GuestBudget {
    /// Charge a growth from `current` to `desired` bytes; false = deny.
    fn charge(&mut self, current: usize, desired: usize) -> bool {
        // Memories and tables only ever grow, so a `desired < current` callback
        // (which wasmtime does not make today) charges nothing rather than
        // crediting the pot back with bytes it never took.
        let delta = desired.saturating_sub(current);
        if self.used.saturating_add(delta) > MAX_GUEST_BYTES {
            return false;
        }
        self.used += delta;
        self.pending = delta;
        true
    }

    /// Un-charge the last approval (wasmtime calls this only right after the
    /// matching `*_growing` returned true, so `pending` is always the one to
    /// give back).
    fn refund(&mut self) {
        self.used = self.used.saturating_sub(std::mem::take(&mut self.pending));
    }
}

impl ResourceLimiter for GuestBudget {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(self.charge(current, desired))
    }

    fn memory_grow_failed(&mut self, _e: wasmtime::Error) -> wasmtime::Result<()> {
        self.refund();
        Ok(())
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(self.charge(
            current.saturating_mul(TABLE_ELEM_BYTES),
            desired.saturating_mul(TABLE_ELEM_BYTES),
        ))
    }

    fn table_grow_failed(&mut self, _e: wasmtime::Error) -> wasmtime::Result<()> {
        self.refund();
        Ok(())
    }

    fn tables(&self) -> usize {
        MAX_GUEST_TABLES
    }

    fn memories(&self) -> usize {
        MAX_GUEST_MEMORIES
    }
}

/// The store's host data: the resource limiter fuel can't provide.
struct HostState {
    limits: GuestBudget,
}

struct WasmInst {
    store: Store<HostState>,
    /// name → (callable, param types, result types), resolved at load.
    exports: HashMap<String, (Func, Box<[Type]>, Box<[Type]>)>,
}

/// A fresh store with the memory/table limiter armed.
fn new_store(engine: &Engine) -> Store<HostState> {
    let mut store = Store::new(
        engine,
        HostState {
            limits: GuestBudget::default(),
        },
    );
    store.limiter(|s| &mut s.limits);
    store
}

/// The live instances, indexed by the pid that loaded each one (whose retirement closes
/// it). The owner sits outside the instance's own lock so the retirement sweep never
/// waits on a guest call in flight.
static REGISTRY: LazyLock<Mutex<OwnedTable<Arc<Mutex<WasmInst>>>>> =
    LazyLock::new(|| Mutex::new(OwnedTable::default()));

/// The registry lock, poison-tolerant: a panic while another thread held it
/// must not turn every future wasm op into a hard panic (the "must Err, never
/// panic" bar). The map is plain data, so a poisoned guard is still usable.
fn reg() -> std::sync::MutexGuard<'static, OwnedTable<Arc<Mutex<WasmInst>>>> {
    crate::core::sync::lock(&REGISTRY)
}

/// A fresh, unguessable instance token: 62 random bits (positive as a Brood int), never
/// 0 and never one already live. Sequential ids let any process reach any instance by
/// counting from 1.
fn fresh_token(registry: &OwnedTable<Arc<Mutex<WasmInst>>>) -> Result<u64, LispError> {
    loop {
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(|e| wasm_err("%wasm-load", e))?;
        let token = u64::from_le_bytes(raw) & ((1u64 << 62) - 1);
        if token != 0 && !registry.contains_key(&token) {
            return Ok(token);
        }
    }
}

/// Lock one instance, poison-tolerant (see [`reg`]).
fn lock_inst(inst: &Arc<Mutex<WasmInst>>) -> std::sync::MutexGuard<'_, WasmInst> {
    crate::core::sync::lock(inst)
}

/// Compiled components, keyed by the BLAKE3 of their source bytes, least
/// recently used first, at most [`COMPILED_CACHE_ENTRIES`].
///
/// `Component::new` compiles the whole component with Cranelift — seconds on a
/// small ARM board — and a caller reloads the same bytes on purpose: to give back
/// linear memory (it never shrinks) and to recover from a trap (a trapped store is
/// poisoned). Only the compiled code is shared. Every load still builds a new
/// `Store`, so the instance's memories, tables, globals and fuel start fresh.
/// The key is a cryptographic hash, not `Hash`: the source is untrusted, and a
/// collision would hand one caller another component's code.
struct CompiledCache {
    entries: VecDeque<([u8; 32], Component)>,
}

impl CompiledCache {
    /// The component compiled from source hashing to `key`, marked most recently used.
    fn get(&mut self, key: &[u8; 32]) -> Option<Component> {
        let position = self
            .entries
            .iter()
            .position(|(entry_key, _)| entry_key == key)?;
        let entry = self.entries.remove(position)?;
        let component = entry.1.clone();
        self.entries.push_back(entry);
        Some(component)
    }

    /// Remember `component` under `key`, evicting the least recently used entry
    /// past the bound. A key already present (two loads of the same bytes compiled
    /// at once) keeps the entry it has.
    fn insert(&mut self, key: [u8; 32], component: Component) {
        if self.entries.iter().any(|(entry_key, _)| *entry_key == key) {
            return;
        }
        while self.entries.len() >= COMPILED_CACHE_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back((key, component));
    }
}

static COMPILED: LazyLock<Mutex<CompiledCache>> = LazyLock::new(|| {
    Mutex::new(CompiledCache {
        entries: VecDeque::with_capacity(COMPILED_CACHE_ENTRIES),
    })
});

/// The compiled form of `source`: from [`COMPILED`] when these exact bytes were
/// compiled before, else compiled now and remembered. A source that fails to
/// compile is not remembered, so it fails the same way every time.
fn compiled_component(engine: &Engine, source: &[u8]) -> Result<Component, LispError> {
    let key = *blake3::hash(source).as_bytes();
    if let Some(component) = crate::core::sync::lock(&COMPILED).get(&key) {
        return Ok(component);
    }
    // Compile outside the lock: it takes seconds, and a load of some other
    // component must not wait behind it.
    let component = Component::new(engine, source).map_err(|e| wasm_err("%wasm-load", e))?;
    crate::core::sync::lock(&COMPILED).insert(key, component.clone());
    Ok(component)
}

/// The shared engine: fuel metering on, Cranelift, built once.
fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| {
        let mut cfg = Config::new();
        cfg.consume_fuel(true);
        // Never RESERVE more address space for one linear memory than the whole store is
        // ever allowed to use. wasmtime's default is a 4 GiB reservation per memory (so
        // a bounds check is a guard-page fault rather than a branch), which is free on a
        // machine with unlimited address space and fatal on one without: a component with
        // eight small memories asked for 32 GiB of `mmap` and was DENIED — reported to
        // Brood as the sandbox refusing a module it is documented to allow. That is how
        // `tests/wasm_sandbox_limits_test.blsp` came to be listed in CLAUDE.md as a test
        // that "fails under the address-space cap", i.e. a standing exception that would
        // have hidden a real sandbox regression.
        //
        // `MAX_GUEST_BYTES` is the honest bound: `GuestBudget` denies any growth past it,
        // summed over every memory, so a reservation larger than that can never be used.
        // Memories stay movable (the wasmtime default), so a guest that grows within the
        // budget is reallocated rather than trapped.
        cfg.memory_reservation(MAX_GUEST_BYTES as u64);
        Engine::new(&cfg).expect("wasm engine")
    })
}

/// A wasmtime failure as a catchable Brood error.
///
/// The **alternate** formatter is deliberate: `wasmtime::Error` is an
/// `anyhow::Error`, whose plain `Display` prints only the outermost context —
/// for a trap that is the useless `"error while executing at wasm backtrace: …"`,
/// with the actual cause (`all fuel consumed by WebAssembly`, `wasm trap: call
/// stack exhausted`, `out of bounds memory access`) hidden one link down the
/// source chain. `{e:#}` walks the chain, so a Brood `catch` can tell a runaway
/// guest apart from a buggy one.
fn wasm_err(who: &str, e: impl std::fmt::Display) -> LispError {
    LispError::runtime(format!("{who}: {e}"))
}

fn trap_err(who: &str, e: wasmtime::Error) -> LispError {
    LispError::runtime(format!("{who}: {e:#}"))
}

/// Instantiate a component from source bytes (a compiled `.wasm` component or
/// WAT text — `wasmtime` accepts both), owned by process `owner`. Returns the
/// registry token. The compiled code comes from [`COMPILED`] when these bytes
/// were loaded before; the instance and its store are always new.
pub fn load(src: &[u8], owner: u64) -> Result<u64, LispError> {
    if src.len() > MAX_COMPONENT_BYTES {
        return Err(wasm_err(
            "%wasm-load",
            format!(
                "component is {} bytes, over the {MAX_COMPONENT_BYTES}-byte load cap",
                src.len()
            ),
        ));
    }
    let engine = engine();
    let component = compiled_component(engine, src)?;
    let mut store = new_store(engine);
    // Instantiation may run start functions — meter it like a call.
    store
        .set_fuel(FUEL_PER_CALL)
        .map_err(|e| wasm_err("%wasm-load", e))?;
    let linker: Linker<HostState> = Linker::new(engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|e| trap_err("%wasm-load", e))?;
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
    let mut registry = reg();
    let id = fresh_token(&registry)?;
    registry.insert(id, owner, Arc::new(Mutex::new(WasmInst { store, exports })));
    Ok(id)
}

/// The exported functions of instance `id`: `(name, arity)` pairs.
pub fn exports(id: u64) -> Result<Vec<(String, usize)>, LispError> {
    let inst = instance("%wasm-exports", id)?;
    let inst = lock_inst(&inst);
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
    reg().remove(&id);
}

/// Close every instance process `pid` still owns — called from the scheduler's
/// retirement path beside `net::close_process_sockets`, the same
/// resource-dies-with-its-owner rule. Without it an instance outlived its loader for
/// the runtime's life: twenty processes that each loaded a 3200-page memory and died
/// left 10 GB of address space mapped. A no-op for a process that closed its own.
pub fn close_process_instances(pid: u64) {
    // Drop the entries outside the registry lock: the last `Arc` frees a whole store.
    let closed = reg().remove_owned_by(pid);
    drop(closed);
}

/// Look up instance `id`. `who` is the *calling* primitive, so a stale handle
/// handed to `%wasm-exports` doesn't report itself as a `%wasm-call` failure.
fn instance(who: &str, id: u64) -> Result<Arc<Mutex<WasmInst>>, LispError> {
    reg().get(&id).cloned().ok_or_else(|| {
        LispError::runtime(format!("{who}: no such wasm instance (already closed?)"))
    })
}

/// Call export `name` of instance `id`. Marshals `args` by the export's WIT
/// parameter types, refuels, calls, lifts the results. One call at a time per
/// instance (the store is single-threaded); a trap — including out-of-fuel —
/// is a catchable error.
pub fn call(heap: &mut Heap, id: u64, name: &str, args: &[Value]) -> LispResult {
    let inst = instance("%wasm-call", id)?;
    let mut inst = lock_inst(&inst);
    let (func, param_tys, result_tys) = match inst.exports.get(name) {
        Some(entry) => (entry.0, entry.1.clone(), entry.2.clone()),
        None => {
            return Err(LispError::runtime(format!(
                "%wasm-call: no export `{name}` in this component (see %wasm-exports)"
            )));
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
        .map_err(|e| trap_err(&format!("%wasm-call `{name}`"), e))?;
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
                        )));
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
            // Fast path: a `list<u8>` parameter accepts a Brood `bytes` value directly
            // (the byte-oriented calls — hashing, compression, codecs, binary parsing —
            // pass bytes, not an int vector). Lower each octet to `u8` in one pass; a
            // vector/list of ints still lowers via the generic path below.
            if matches!(elem, Type::U8)
                && let Value::Bytes(id) = v
            {
                let out = heap
                    .bytes(id)
                    .as_bytes()
                    .iter()
                    .map(|&b| Val::U8(b))
                    .collect();
                return Ok(Val::List(out));
            }
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
        Type::Option(o) => {
            // `option<option<T>>` is ambiguous through Brood: `nil` is the only
            // "none", so an outer None and an inner None collapse. Reject it
            // rather than silently mis-marshal (a `some(none)` would be
            // unreachable). A caller that needs it should use a `result`/variant.
            if matches!(o.ty(), Type::Option(_)) {
                return Err(LispError::runtime(format!(
                    "{who}: nested `option<option<…>>` is not representable (nil is the only \
                     none, so the two levels collapse) — use a result/variant"
                )));
            }
            match v {
                Value::Nil => Val::Option(None),
                some => Val::Option(Some(Box::new(lower(heap, who, some, &o.ty())?))),
            }
        }
        other => {
            return Err(LispError::runtime(format!(
                "{who}: unsupported WIT parameter type {other:?} (this slice marshals \
                 scalars, strings, lists, tuples, and options)"
            )));
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
                )));
            }
        },
        Val::Float32(f) => Value::Float(*f as f64),
        Val::Float64(f) => Value::Float(*f),
        Val::Char(c) => heap.alloc_string(&c.to_string()),
        Val::String(s) => heap.alloc_string(s),
        // A `list<u8>` result lifts to a Brood `bytes` value — the byte-oriented
        // return (a hash, compressed output, an encoded frame). Detected from the
        // element vals (all `u8`), so no result-type threading is needed. An EMPTY
        // list is ambiguous (`list<u8>` vs `list<s32>` both lower to `[]`), so it
        // stays an empty vector; a caller needing empty bytes builds one explicitly.
        Val::List(items) if !items.is_empty() && items.iter().all(|i| matches!(i, Val::U8(_))) => {
            let bytes: Vec<u8> = items
                .iter()
                .map(|i| match i {
                    Val::U8(b) => *b,
                    _ => unreachable!("guarded all-u8 above"),
                })
                .collect();
            heap.alloc_bytes(crate::core::blob::SharedBlob::new(&bytes))
        }
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
            )));
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
                        ));
                    }
                }
            }
            Ok(out)
        }
        other => Err(LispError::wrong_type(heap, who, "list or vector", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A component whose `bump` counts calls in a global AND in linear memory,
    /// returning `global + 1000 * memory-word` — so a first call on a fresh
    /// instance is 1001 and anything else means state leaked between instances.
    const COUNTER_WAT: &str = r#"(component
      (core module $m
        (memory 1)
        (global $count (mut i32) (i32.const 0))
        (func (export "bump") (result i32)
          (global.set $count (i32.add (global.get $count) (i32.const 1)))
          (i32.store (i32.const 0) (i32.add (i32.load (i32.const 0)) (i32.const 1)))
          (i32.add (global.get $count) (i32.mul (i32.load (i32.const 0)) (i32.const 1000)))))
      (core instance $i (instantiate $m))
      (func $bump (result s32) (canon lift (core func $i "bump")))
      (export "bump" (func $bump)))"#;

    /// A distinct, trivially valid component per `index` (the export name differs).
    fn numbered_wat(index: usize) -> String {
        format!(
            r#"(component
              (core module $m (func (export "f") (result i32) i32.const {index}))
              (core instance $i (instantiate $m))
              (func $f (result s32) (canon lift (core func $i "f")))
              (export "f{index}" (func $f)))"#
        )
    }

    fn bump(id: u64) -> i32 {
        let instance = instance("test", id).expect("live instance");
        let mut instance = lock_inst(&instance);
        let instance = &mut *instance;
        let func = instance.exports.get("bump").expect("bump export").0;
        instance.store.set_fuel(FUEL_PER_CALL).expect("fuel");
        let mut results = [Val::Bool(false)];
        func.call(&mut instance.store, &[], &mut results)
            .expect("bump call");
        match results[0] {
            Val::S32(value) => value,
            ref other => panic!("bump returned {other:?}"),
        }
    }

    fn cache_holds(source: &[u8]) -> bool {
        let key = *blake3::hash(source).as_bytes();
        crate::core::sync::lock(&COMPILED)
            .entries
            .iter()
            .any(|(entry_key, _)| *entry_key == key)
    }

    #[test]
    fn reloading_the_same_source_reuses_the_compiled_component_but_not_its_state() {
        let source = COUNTER_WAT.as_bytes();
        let first = load(source, 0).expect("first load");
        assert_eq!(bump(first), 1001);
        assert_eq!(bump(first), 2002);
        let second = load(source, 0).expect("second load");
        assert_ne!(first, second);
        // A fresh store: the global and the linear memory both start over.
        assert_eq!(bump(second), 1001);
        // And the first instance is untouched by the second.
        assert_eq!(bump(first), 3003);
        close(first);
        close(second);
    }

    #[test]
    fn a_source_that_fails_to_compile_is_not_cached_and_fails_again() {
        let source = b"(component (bogus";
        assert!(load(source, 0).is_err());
        assert!(!cache_holds(source));
        assert!(load(source, 0).is_err());
    }

    #[test]
    fn the_cache_is_bounded_and_evicts_the_least_recently_used() {
        let engine = engine();
        let component = Component::new(engine, numbered_wat(0)).expect("compile");
        let mut cache = CompiledCache {
            entries: VecDeque::new(),
        };
        let key = |index: u8| [index; 32];
        for index in 0..COMPILED_CACHE_ENTRIES as u8 {
            cache.insert(key(index), component.clone());
        }
        // Touch the oldest, so the next insert evicts the second-oldest instead.
        assert!(cache.get(&key(0)).is_some());
        cache.insert(key(100), component.clone());
        assert_eq!(cache.entries.len(), COMPILED_CACHE_ENTRIES);
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(1)).is_none());
        assert!(cache.get(&key(100)).is_some());
        // Inserting a key already present keeps one entry for it.
        cache.insert(key(100), component);
        assert_eq!(cache.entries.len(), COMPILED_CACHE_ENTRIES);
    }

    #[test]
    fn distinct_sources_past_the_bound_still_load() {
        let mut ids = Vec::new();
        for index in 0..COMPILED_CACHE_ENTRIES + 2 {
            ids.push(load(numbered_wat(index).as_bytes(), 0).expect("load"));
        }
        assert!(crate::core::sync::lock(&COMPILED).entries.len() <= COMPILED_CACHE_ENTRIES);
        for id in ids {
            close(id);
        }
    }
}
