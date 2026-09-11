//! JIT (ADR-101) — the tier-1 template JIT, behind `--features jit` (a default feature).
//!
//! This module is the **ABI and the backend registry**, not the code generator:
//!
//! | file | role |
//! |---|---|
//! | [`backend`] | the contract — the six obligations any backend must satisfy |
//! | [`rt`] | the `brood_rt_*` callback table — the only heap/GC interface (obligation 4) |
//! | [`cranelift`] | today's sole backend: the Cranelift module + its `JitBackend` impl |
//!
//! The Cranelift *lowering* deliberately lives elsewhere — `eval/compile/jit_lower*`, a
//! child of `eval::compile` because it reads that module's private IR (`Node`, `Inst`,
//! `Chunk`, `CompiledArm`). See [`cranelift`]'s docs for the full map, and
//! `docs/backend-seams.md` for why the seam is drawn here.
//!
//! Read [`backend`] first: the ABI a backend emits against, and the value/GC discipline that
//! makes tier-1 sound under a moving collector, are documented there as obligations rather
//! than as folklore.

pub(crate) mod backend;
pub(crate) mod cranelift;
pub(crate) mod rt;

pub(crate) use backend::JitArmFn;
pub(crate) use backend::JitBackend;
pub(crate) use cranelift::CraneliftBackend;

use std::sync::{LazyLock, Mutex};

/// The backend this build compiles arms with. A `#[cfg]`-selected concrete type, not a
/// `dyn` object: selection is a build-time choice, so it costs nothing at runtime and keeps
/// every call through [`JitBackend`] static and monomorphic. Adding a second backend means
/// adding an arm here (and, if it should be selectable per run rather than per build, a
/// `Box<dyn JitBackend>` here — which would also be free, since `lower_arm` runs once per
/// arm on the background compiler thread; see `backend`'s "Why this costs nothing").
pub(crate) type ActiveBackend = cranelift::CraneliftBackend;

/// The process-wide JIT backend (tiering, 1b). It owns every compiled arm's executable
/// code, which must outlive all installed fn-pointers — hence a single process-lifetime
/// instance. Compilation mutates it (`declare`/`define`/`finalize`), so it's behind a
/// `Mutex`; the resulting machine code lives in a shared executable mmap and is callable
/// from any worker thread once installed (`JITModule` is `Send`). For the int subset a
/// compiled arm is self-contained (no globals), so a process-wide module is correct;
/// arms that reference a runtime's globals bail today, so per-runtime isolation isn't
/// needed yet.
pub(crate) static GLOBAL_JIT: LazyLock<Mutex<ActiveBackend>> =
    LazyLock::new(|| Mutex::new(ActiveBackend::new()));

/// Sentinel in [`crate::eval::compile::CompiledArm`]`::jit_code` for an arm that was
/// tried and is out of the JIT's subset — distinct from null (untried) and a real,
/// 8-aligned code pointer.
pub(crate) const BAILED: *mut u8 = std::ptr::dangling_mut::<u8>();

/// Sentinel: the arm is hot and has been handed to the background compiler thread, but
/// its native code isn't installed yet. Callers run the VM until the real pointer
/// replaces this. Distinct from null/`BAILED`/a real (8-aligned) pointer.
pub(crate) const QUEUED: *mut u8 = 2 as *mut u8;
