//! Numeric lists across the component boundary by memory copy (ADR-806).
//!
//! The general path (`lower`/`lift` in `wasm.rs`) builds a [`Val`] per element: a
//! `list<float64>` of 5 600 elements was 5 600 `Val`s built, type-checked and stored on the
//! way in and as many read back out, about 45 ns a float. A kernel's numeric arrays pay that
//! on every call, enough to make a Rust kernel slower than the Brood it replaced.
//!
//! An export whose parameters are all numeric lists (`list<float64|float32|s32|u32>`, up to
//! [`MAX_LIST_PARAMS`]) and whose result is one such list, or nothing, is called through
//! wasmtime's typed API instead, whose primitive-list lowering and lifting copy the bytes
//! in one pass. The element type is the export's, read at call time: [`NumList`] and
//! [`NumListOut`] each stand for any of the four, so one typed signature per arity covers
//! every mix of element types. Answers and errors are the general path's: the same
//! conversions (an int where a float is declared, the same range errors) and the same
//! vector back.
//!
//! The two types implement wasmtime's `ComponentType`/`Lower`/`Lift` by delegating to its
//! own implementations for `[T]` and `Vec<T>`. Those traits' items are hidden, reached
//! through `wasmtime::component::__internal` (what `#[derive(ComponentType)]` expands to),
//! and outside wasmtime's stability guarantee: a wasmtime upgrade that changes them fails
//! this file's build, not a run.

use std::mem::MaybeUninit;

use wasmtime::ValRaw;
use wasmtime::component::__internal::{
    CanonicalAbiInfo, InstanceType, InterfaceType, LiftContext, LowerContext,
};
use wasmtime::component::{ComponentType, Func, Lift, Lower, Type};
use wasmtime::{Result as WasmResult, Store};

use crate::core::heap::Heap;
use crate::core::value::Value;
use crate::error::{LispError, LispResult};

/// The most list parameters the fast path takes; an export with more takes the general one.
pub(super) const MAX_LIST_PARAMS: usize = 4;

/// The element type of a numeric list the fast path carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Elem {
    F64,
    F32,
    S32,
    U32,
}

impl Elem {
    /// The element type of `ty` when it is a list the fast path carries.
    fn of_list(ty: &Type) -> Option<Elem> {
        match ty {
            Type::List(list) => match list.ty() {
                Type::Float64 => Some(Elem::F64),
                Type::Float32 => Some(Elem::F32),
                Type::S32 => Some(Elem::S32),
                Type::U32 => Some(Elem::U32),
                _ => None,
            },
            _ => None,
        }
    }

    /// The element type of a list's `InterfaceType`, as wasmtime hands it to (un)lowering.
    fn of_interface(
        ty: InterfaceType,
        types: &wasmtime::component::__internal::ComponentTypes,
    ) -> Option<Elem> {
        match ty {
            InterfaceType::List(index) => match types[index].element {
                InterfaceType::Float64 => Some(Elem::F64),
                InterfaceType::Float32 => Some(Elem::F32),
                InterfaceType::S32 => Some(Elem::S32),
                InterfaceType::U32 => Some(Elem::U32),
                _ => None,
            },
            _ => None,
        }
    }
}

/// An export the fast path can call: its parameters' element types and its result's.
#[derive(Clone, Debug)]
pub(super) struct Shape {
    params: Box<[Elem]>,
    result: Option<Elem>,
}

impl Shape {
    /// The shape of an export with `params` and `results`, when every parameter is a numeric
    /// list and the result is one or none. `None` sends the call down the general path.
    pub(super) fn of(params: &[Type], results: &[Type]) -> Option<Shape> {
        if params.is_empty() || params.len() > MAX_LIST_PARAMS {
            return None;
        }
        let params = params
            .iter()
            .map(Elem::of_list)
            .collect::<Option<Box<[Elem]>>>()?;
        let result = match results {
            [] => None,
            [one] => Some(Elem::of_list(one)?),
            _ => return None,
        };
        Some(Shape { params, result })
    }
}

/// A numeric list on its way into the guest, in the element type the export declares.
enum NumList {
    F64(Vec<f64>),
    F32(Vec<f32>),
    S32(Vec<i32>),
    U32(Vec<u32>),
}

/// A numeric list the guest returned, in the element type the export declares.
enum NumListOut {
    F64(Vec<f64>),
    F32(Vec<f32>),
    S32(Vec<i32>),
    U32(Vec<u32>),
}

/// Accept any of the four list types: the value is built for (or lifted from) the type the
/// export actually declares, so the check that matters happened in [`Shape::of`].
fn typecheck_any_list(ty: &InterfaceType, types: &InstanceType<'_>) -> WasmResult<()> {
    <[f64] as ComponentType>::typecheck(ty, types)
        .or_else(|_| <[f32] as ComponentType>::typecheck(ty, types))
        .or_else(|_| <[i32] as ComponentType>::typecheck(ty, types))
        .or_else(|_| <[u32] as ComponentType>::typecheck(ty, types))
}

// SAFETY: every item delegates to wasmtime's own `[T]` implementation, whose ABI (a pointer
// pair, two flat `ValRaw`s) is the same for each of the four element types.
unsafe impl ComponentType for NumList {
    type Lower = [ValRaw; 2];
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::POINTER_PAIR;

    fn typecheck(ty: &InterfaceType, types: &InstanceType<'_>) -> WasmResult<()> {
        typecheck_any_list(ty, types)
    }
}

// SAFETY: delegates to wasmtime's `Lower for [T]` for the variant's element type; the
// variant was built for the export's declared type, so `ty` is that list's type.
unsafe impl Lower for NumList {
    fn linear_lower_to_flat<T>(
        &self,
        cx: &mut LowerContext<'_, T>,
        ty: InterfaceType,
        dst: &mut MaybeUninit<[ValRaw; 2]>,
    ) -> WasmResult<()> {
        match self {
            NumList::F64(items) => items.as_slice().linear_lower_to_flat(cx, ty, dst),
            NumList::F32(items) => items.as_slice().linear_lower_to_flat(cx, ty, dst),
            NumList::S32(items) => items.as_slice().linear_lower_to_flat(cx, ty, dst),
            NumList::U32(items) => items.as_slice().linear_lower_to_flat(cx, ty, dst),
        }
    }

    fn linear_lower_to_memory<T>(
        &self,
        cx: &mut LowerContext<'_, T>,
        ty: InterfaceType,
        offset: usize,
    ) -> WasmResult<()> {
        match self {
            NumList::F64(items) => items.as_slice().linear_lower_to_memory(cx, ty, offset),
            NumList::F32(items) => items.as_slice().linear_lower_to_memory(cx, ty, offset),
            NumList::S32(items) => items.as_slice().linear_lower_to_memory(cx, ty, offset),
            NumList::U32(items) => items.as_slice().linear_lower_to_memory(cx, ty, offset),
        }
    }
}

// SAFETY: as for `NumList`: the ABI of `Vec<T>` is the same pointer pair for all four.
unsafe impl ComponentType for NumListOut {
    type Lower = [ValRaw; 2];
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::POINTER_PAIR;

    fn typecheck(ty: &InterfaceType, types: &InstanceType<'_>) -> WasmResult<()> {
        typecheck_any_list(ty, types)
    }
}

/// The declared element type of the list `ty` names, or the error wasmtime would give.
fn lifted_elem(cx: &LiftContext<'_>, ty: InterfaceType) -> WasmResult<Elem> {
    Elem::of_interface(ty, cx.types)
        .ok_or_else(|| wasmtime::format_err!("expected a numeric list result"))
}

// SAFETY: delegates to wasmtime's `Lift for Vec<T>` for the element type the export
// declares, read from `ty` itself.
unsafe impl Lift for NumListOut {
    fn linear_lift_from_flat(
        cx: &mut LiftContext<'_>,
        ty: InterfaceType,
        src: &[ValRaw; 2],
    ) -> WasmResult<Self> {
        Ok(match lifted_elem(cx, ty)? {
            Elem::F64 => NumListOut::F64(Vec::linear_lift_from_flat(cx, ty, src)?),
            Elem::F32 => NumListOut::F32(Vec::linear_lift_from_flat(cx, ty, src)?),
            Elem::S32 => NumListOut::S32(Vec::linear_lift_from_flat(cx, ty, src)?),
            Elem::U32 => NumListOut::U32(Vec::linear_lift_from_flat(cx, ty, src)?),
        })
    }

    fn linear_lift_from_memory(
        cx: &mut LiftContext<'_>,
        ty: InterfaceType,
        bytes: &[u8],
    ) -> WasmResult<Self> {
        Ok(match lifted_elem(cx, ty)? {
            Elem::F64 => NumListOut::F64(Vec::linear_lift_from_memory(cx, ty, bytes)?),
            Elem::F32 => NumListOut::F32(Vec::linear_lift_from_memory(cx, ty, bytes)?),
            Elem::S32 => NumListOut::S32(Vec::linear_lift_from_memory(cx, ty, bytes)?),
            Elem::U32 => NumListOut::U32(Vec::linear_lift_from_memory(cx, ty, bytes)?),
        })
    }
}

/// One Brood argument as the list the export declares — the general path's conversions
/// element for element (`number_arg`, `int_arg` and its range errors).
fn lower_arg(heap: &Heap, who: &str, value: Value, elem: Elem) -> Result<NumList, LispError> {
    let items = crate::host::numbers::flat_items(heap, who, value)?;
    Ok(match elem {
        Elem::F64 => NumList::F64(
            items
                .iter()
                .map(|&item| super::number_arg(heap, who, item))
                .collect::<Result<_, _>>()?,
        ),
        Elem::F32 => NumList::F32(
            items
                .iter()
                .map(|&item| super::number_arg(heap, who, item).map(|f| f as f32))
                .collect::<Result<_, _>>()?,
        ),
        Elem::S32 => NumList::S32(
            items
                .iter()
                .map(|&item| {
                    let n = super::int_arg(heap, who, item)?;
                    i32::try_from(n).map_err(|_| super::range_err(who, "s32", n))
                })
                .collect::<Result<_, _>>()?,
        ),
        Elem::U32 => NumList::U32(
            items
                .iter()
                .map(|&item| {
                    let n = super::int_arg(heap, who, item)?;
                    u32::try_from(n).map_err(|_| super::range_err(who, "u32", n))
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}

/// The guest's list as the vector the general path's `lift` builds.
fn lift_result(heap: &mut Heap, out: NumListOut) -> Value {
    let items: Vec<Value> = match out {
        NumListOut::F64(items) => items.into_iter().map(Value::Float).collect(),
        NumListOut::F32(items) => items.into_iter().map(|f| Value::Float(f as f64)).collect(),
        NumListOut::S32(items) => items.into_iter().map(|n| Value::Int(n as i64)).collect(),
        NumListOut::U32(items) => items.into_iter().map(|n| Value::Int(n as i64)).collect(),
    };
    heap.alloc_vector(items)
}

/// Call `func` (of shape `shape`) with `args` by memory copy. The caller has checked the
/// arity and refuelled the store; a trap comes back as the general path's error.
pub(super) fn call<S: 'static>(
    heap: &mut Heap,
    store: &mut Store<S>,
    func: Func,
    shape: &Shape,
    name: &str,
    args: &[Value],
) -> LispResult {
    let mut lowered = Vec::with_capacity(args.len());
    for (&arg, &elem) in args.iter().zip(shape.params.iter()) {
        lowered.push(lower_arg(heap, name, arg, elem)?);
    }
    let trap = |error| super::trap_err(&format!("%wasm-call `{name}`"), error);
    let typed_err = |error| super::wasm_err("%wasm-call", error);
    macro_rules! typed_call {
        ($($param:ident),+) => {{
            let mut lowered = lowered.into_iter();
            $(let $param = lowered.next().expect("one lowered list per parameter");)+
            match shape.result {
                Some(_) => {
                    let typed = func
                        .typed::<($(typed_call!(@ty $param),)+), (NumListOut,)>(&*store)
                        .map_err(typed_err)?;
                    let (out,) = typed.call(&mut *store, ($($param,)+)).map_err(trap)?;
                    Ok(lift_result(heap, out))
                }
                None => {
                    let typed = func
                        .typed::<($(typed_call!(@ty $param),)+), ()>(&*store)
                        .map_err(typed_err)?;
                    typed.call(&mut *store, ($($param,)+)).map_err(trap)?;
                    Ok(Value::nil())
                }
            }
        }};
        (@ty $param:ident) => { NumList };
    }
    match shape.params.len() {
        1 => typed_call!(first),
        2 => typed_call!(first, second),
        3 => typed_call!(first, second, third),
        4 => typed_call!(first, second, third, fourth),
        _ => unreachable!("Shape::of admits 1..=MAX_LIST_PARAMS parameters"),
    }
}
