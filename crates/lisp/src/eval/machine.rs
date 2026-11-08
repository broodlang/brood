//! The tree-walker's frame stack (ADR-416).
//!
//! The evaluator used to keep its pending work on the Rust stack: evaluating an argument, an
//! `if` test, a `let` binding or a non-last body form was a nested `eval` call. Here that work
//! is a [`Kont`] — a continuation naming what to do with the sub-form's value — on a stack the
//! [`Machine`] owns, and the sub-form runs on the same loop. A tail position pushes nothing, so
//! proper tail calls hold by construction.
//!
//! **One continuation is one old nested `eval`.** It sets the pushing frame's registers aside
//! (its trace tracker [`TwFrame`] and lazy-expansion count) and the sub-form starts a frame of
//! its own, exactly as the nested call did. A value pops it and resumes the pushing frame. An
//! error pops it the way the nested call's error left it: the inner frame's trace entry is
//! closed ([`close_frame`]), the sub-form's position is attached when the old site used
//! `eval_at` (`Kont::form`), and the continuation's operand roots are dropped, as `root_scope`
//! dropped them. The error then belongs to the pushing frame and keeps unwinding.
//!
//! **GC.** A continuation holds no raw handle: what it keeps (the unchosen branch, the forms
//! left, the arguments so far, the environment) sits on the heap's operand stacks as
//! [`Root`]/[`EnvRoot`] slots, which both collectors relocate in place — the discipline every
//! site already followed, now owned by the frame. `Marks` are the stack depths from before a
//! continuation rooted anything; finishing it truncates back to them.
//!
//! **Leaves run in place.** An atom, or a symbol bound here, evaluates without running
//! anything — it cannot collect or fail — so a sub-form that is one ([`leaf_value`]) is read
//! directly and costs no frame. `(+ a b)` pushes nothing.
//!
//! **Entering a closure is one path** ([`Machine::enter`]), whoever calls: a call this frame
//! makes, a macro expander (whose expansion then replaces the call, `KontKind::Expand`), or a
//! native calling back into Brood (`apply_closure`, which runs the body on a machine of its
//! own). Its unsupplied `&optional` defaults run as frames too (`KontKind::Defaults`).
//!
//! **`try` and `binding` are frames** under a tier-0 ceiling (ADR-416 stage 3), as the VM runs
//! them inline (ADR-400/402): a call of `%try`/`%try-described`/`%binding` with a closure
//! thunk pushes `KontKind::Try`/`Binding` and enters the thunk; an error unwinding into a
//! `Try` frame runs its handler as a frame (`KontKind::Callback`), and a `Binding` frame
//! restores the dynamic stack on the way out either way. Above tier 0 they stay natives: their
//! `apply_engine` routes the thunk to the VM. `proc/interruptible` is one too (ADR-494), as
//! the VM runs it as a region (ADR-491): `KontKind::Interruptible` holds the window open
//! while the thunk runs, so a `receive` in an interrupt window parks a capturing machine.
//!
//! **Capture** (ADR-416 stage 4). A green process's body, and each top-level form of a
//! program, runs at tier 0 on a capturing machine ([`run_captured`]): a `%receive` that finds
//! nothing, or a spent reduction budget at a step's safepoint, stops the machine with its
//! frames intact ([`Park`]) and hands them to the scheduler as a [`TwSuspended`], which
//! [`resume_captured`] continues on any worker. Like the VM's, the continuation is control
//! state only: everything it references is already on the heap's root stacks, which a
//! collection while parked relocates in place. A `receive` is redone from scratch on resume,
//! as the VM re-runs its rewound call.
//!
//! Still native recursion: a seq-view realised for `apply`, and every other native that calls
//! back into Brood (each such callback is one `apply_closure`, so one machine per native
//! level). A `receive` under one blocks its worker, as on the VM (`TreeWalkGuard`).

use super::*;
use crate::core::heap::EnvRoot;

/// The deepest the frame stack may grow: runaway non-tail recursion fails here, catchably,
/// as it does at the VM's frame limit. A level of non-tail recursion costs at least one
/// continuation and often two or three (an inline `try` pushes its `Try` frame as well as
/// the pending call's), so `1 << 18` stopped tier 0 well short of a depth the VM reaches
/// (`vm_nested_stack_guard_test.blsp`: 200 000 levels through `try` died near 131 000). It
/// is not the VM's `1 << 20` either: a tree-walked level holds several KB of heap besides
/// its frames, and at that cap a runaway `%try` recursion hit the test runner's 2 GB
/// memory limit before this error — the catchable answer has to come first.
const MAX_TW_FRAMES: usize = 1 << 19;

/// "recursion too deep …" — the tree-walker's frame-count guard.
fn tw_frame_depth_error(frames: usize) -> LispError {
    LispError::runtime(format!(
        "recursion too deep: exceeded the tree-walker's {frames}-frame non-tail-call \
         limit (runaway non-tail recursion?)",
    ))
    .with_code(crate::error::error_codes::STACK_DEPTH_EXCEEDED)
    .with_hint(
        "rewrite as a tail-recursive loop (proper tail calls are O(1) stack), \
         or drive the iteration with a process",
    )
}

/// The operand-stack depths from before a continuation rooted anything.
#[derive(Clone, Copy)]
struct Marks {
    roots: usize,
    envs: usize,
}

impl Marks {
    fn take(heap: &Heap) -> Marks {
        Marks {
            roots: heap.roots_len(),
            envs: heap.env_roots_len(),
        }
    }

    fn restore(self, heap: &mut Heap) {
        heap.truncate_roots(self.roots);
        heap.truncate_env_roots(self.envs);
    }
}

/// A frame entered across a native boundary (`apply_closure`, a macro expander) ran its
/// body's last form on its own loop, and an error leaving that loop took the form's position.
/// The frame keeps the form until its tail is reached (`Pending`), and from then on an error
/// unwinding out of it is tagged with it (`Armed`) — before the frame's trace entry closes,
/// as the old loop's `map_err` did. Errors from the non-last forms are tagged with their own
/// forms instead.
#[derive(Clone, Copy)]
enum TailTag {
    None,
    Pending(Root),
    Armed(Root),
}

impl TailTag {
    fn tag(self, heap: &Heap, e: LispError) -> LispError {
        match self {
            TailTag::Armed(r) => e.or_form_pos(heap, heap.read_root(r)),
            _ => e,
        }
    }
}

/// A pending piece of work: what the frame that pushed it does with the value of the
/// sub-form now running.
struct Kont {
    /// The pushing frame's registers — trace tracker, lazy-expansion count, tail tag — set
    /// aside while the sub-form runs as a frame of its own.
    entered: Option<TwFrame>,
    lazy: u32,
    tail: TailTag,
    /// The sub-form, when an error leaving it takes its position (the old `eval_at`).
    form: Option<Root>,
    marks: Marks,
    kind: KontKind,
}

/// How many frames a full segment of [`KontStack`] holds.
const KONT_SEGMENT: usize = 1024;

/// The frame stack, in fixed segments. A `Vec` doubles, so a deep recursion held up to twice
/// the frames it had and, while growing, the old buffer and the new one at once: at 400 000
/// levels that was ~280 MB of a tier-0 run's ~390 MB. Here the top segment grows as a `Vec`
/// does — a shallow machine, the usual one, pays nothing more — until it holds
/// [`KONT_SEGMENT`] frames; then it is set aside whole and never moves again. One emptied
/// segment is kept, so a recursion going up and down across a boundary does not allocate and
/// free a segment each time.
#[derive(Default)]
struct KontStack {
    full: Vec<Vec<Kont>>,
    top: Vec<Kont>,
    spare: Vec<Kont>,
}

impl KontStack {
    fn push(&mut self, k: Kont) {
        if self.top.len() == KONT_SEGMENT {
            let next = std::mem::take(&mut self.spare);
            let next = if next.capacity() == KONT_SEGMENT {
                next
            } else {
                Vec::with_capacity(KONT_SEGMENT)
            };
            self.full.push(std::mem::replace(&mut self.top, next));
        }
        self.top.push(k);
    }

    fn pop(&mut self) -> Option<Kont> {
        if self.top.is_empty() {
            let below = self.full.pop()?;
            self.spare = std::mem::replace(&mut self.top, below);
        }
        self.top.pop()
    }

    fn len(&self) -> usize {
        self.full.len() * KONT_SEGMENT + self.top.len()
    }
}

enum KontKind {
    /// `if`: choose a branch on the test's value.
    If {
        then_r: Root,
        else_r: Root,
        env_r: EnvRoot,
    },
    /// A body held as a cons list (`do`, `let`, `letrec`): `cursor` is the cell whose form is
    /// running.
    Body { cursor: Root, env_r: EnvRoot },
    /// A closure body's non-last form is running; `rest` are the forms after it (the last of
    /// them is the body's tail).
    Forms {
        rest: SmallVec<[Root; 4]>,
        env_r: EnvRoot,
    },
    /// A `let`/`letrec` binding's value.
    Bind(Box<BindState>),
    /// `def`'s value, under a tier-0 ceiling.
    Def { name: Symbol, env_r: EnvRoot },
    /// An element of a vector, set or map literal.
    Literal(Box<LiteralState>),
    /// A computed head (`((f) …)`).
    Head { call_form_r: Root, env_r: EnvRoot },
    /// An argument of a call.
    Args(ArgsState),
    /// An `&optional` default of a closure being entered.
    Defaults(Box<EntryState>),
    /// A macro expander's body: its value is the form that replaces the call.
    Expand { env_r: EnvRoot },
    /// `%try`'s thunk (`%try-described`'s, when `described`): its value is the call's, and an
    /// error that is not a control signal runs the handler instead ([`Machine::catch`]).
    Try {
        handler_r: Root,
        env_r: EnvRoot,
        described: bool,
    },
    /// `%binding`'s thunk: `dynamics` is the dynamic stack's depth from before its bindings,
    /// restored when the thunk ends, by a value or an error.
    Binding { dynamics: usize },
    /// A callback whose value is the call's (a `%try` handler).
    Callback,
    /// `proc/interruptible`'s thunk (ADR-494): its value is answered `[:ok v]`, an interrupt
    /// unwinding into it `[:interrupted e]` when it was raised for this frame's `window`, the
    /// depth this call opened (ADR-493: windows nest, the innermost takes a request); any other
    /// error passes through. However the frame ends, it closes its window and any inside it.
    Interruptible { window: u32 },
}

/// The natives the tree-walker treats specially under a tier-0 ceiling: `try`/`binding` and
/// `proc/interruptible` run as frames, and a capturing machine can park at a `receive`.
#[derive(Clone, Copy)]
enum FrameNative {
    Try { described: bool },
    Binding,
    Interruptible,
    Receive,
}

impl FrameNative {
    fn of(heap: &Heap, id: NativeId, argc: usize) -> Option<FrameNative> {
        match (heap.native(id).name.as_str(), argc) {
            ("%try", 2) => Some(FrameNative::Try { described: false }),
            ("%try-described", 2) => Some(FrameNative::Try { described: true }),
            ("%binding", 3) => Some(FrameNative::Binding),
            ("proc/interruptible", 2) => Some(FrameNative::Interruptible),
            ("%receive", 4) => Some(FrameNative::Receive),
            // The other native that waits on the mailbox the way `%receive` does, and so
            // suspends a capturing machine: parked unrooted, its suspend surfaced as an empty
            // `runtime error:` (ADR-494).
            ("proc/receive-lane", 1) => Some(FrameNative::Receive),
            _ => None,
        }
    }
}

/// Why a capturing machine stopped with its frames intact, and how to go on.
struct Park {
    /// A `receive` that found nothing; else a preemption. (The `receive`'s `after` deadline
    /// is already the mailbox's: `receive_match` persisted it and armed its timer.)
    suspend: bool,
    redo: Redo,
    /// The root-stack depths from before `redo` rooted anything.
    marks: Marks,
}

/// Where a parked machine picks up.
enum Redo {
    /// Step `expr` in `env`: preempted at a step's safepoint, before the step did anything.
    Step { expr: Root, env: EnvRoot },
    /// Apply `callee` (a `%receive`) to `argv` again.
    Apply {
        callee: Root,
        argv: SmallVec<[Root; 4]>,
        call_form: Root,
        env: EnvRoot,
    },
}

/// The depths of the heap's stacks when a capturing run started: where it leaves them when
/// it ends, by a value or an error.
#[derive(Clone, Copy)]
struct Floors {
    marks: Marks,
    dynamics: usize,
}

impl Floors {
    fn take(heap: &Heap) -> Floors {
        Floors {
            marks: Marks::take(heap),
            dynamics: heap.dynamics_len(),
        }
    }

    fn restore(self, heap: &mut Heap) {
        self.marks.restore(heap);
        heap.truncate_dynamics(self.dynamics);
    }
}

/// A tree-walked process's continuation: the frame stack and the bottom frame's registers of
/// a capturing machine that parked (ADR-416 stage 4). `Send` control state; the values live
/// on the process heap's root stacks.
pub(crate) struct TwSuspended {
    konts: KontStack,
    entered: Option<TwFrame>,
    lazy: u32,
    tail: TailTag,
    redo: Redo,
    redo_marks: Marks,
    floors: Floors,
}

impl TwSuspended {
    /// One-line park/resume descriptor for `BROOD_SCHED_DBG`.
    pub(crate) fn dbg_line(&self) -> String {
        let at = match self.redo {
            Redo::Step { .. } => "step",
            Redo::Apply { .. } => "receive",
        };
        format!("tw {at} frames={}", self.konts.len())
    }
}

/// Where a capturing run starts.
pub(crate) enum TwStart {
    /// A process body: a 0-argument closure, entered across the boundary the scheduler is.
    Closure(ClosureId),
    /// A top-level form, evaluated in `env` as `eval` would.
    Form(Value, EnvId),
}

/// Marks the capturing machine as the innermost driver for its lifetime, so a `%receive` it
/// calls directly suspends instead of blocking (`capture_top_level`). Restores the previous
/// value on drop; a nested tree-walk (`TreeWalkGuard`) clears it again beneath.
struct DriverGuard(bool);

impl DriverGuard {
    fn enter() -> DriverGuard {
        DriverGuard(crate::process::set_capture_top_level(true))
    }
}

impl Drop for DriverGuard {
    fn drop(&mut self) {
        crate::process::set_capture_top_level(self.0);
    }
}

/// Run a green process's body, or one top-level form, on a capturing machine: the tier-0
/// driver `run_one` reaches through `run_process_body` (ADR-416 stage 4). `Done` with the
/// value, `Suspended`/`Preempted` with the frames to resume, or the error — whose trace
/// entry for the bottom frame is closed here, as `apply_closure`/`eval` close theirs.
pub(crate) fn run_captured(
    heap: &mut Heap,
    start: TwStart,
) -> Result<crate::eval::compile::VmOutcome, LispError> {
    let floors = Floors::take(heap);
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _driver = DriverGuard::enter();
    let mut entered = None;
    let mut m = Machine::new(Value::nil(), EnvId::GLOBAL, &mut entered);
    m.capture = crate::process::in_capture_run();
    let first = match start {
        TwStart::Closure(id) => m.enter(heap, id, &[], None),
        TwStart::Form(expr, env) => {
            eval_trace(heap, expr);
            m.expr = expr;
            m.env = env;
            Ok(Step::Continue)
        }
    };
    let out = m.drive(heap, first);
    m.finish(heap, out, floors)
}

/// Continue a parked tree-walked run on this worker.
pub(crate) fn resume_captured(
    heap: &mut Heap,
    s: Box<TwSuspended>,
) -> Result<crate::eval::compile::VmOutcome, LispError> {
    let TwSuspended {
        konts,
        mut entered,
        lazy,
        tail,
        redo,
        redo_marks,
        floors,
    } = *s;
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _driver = DriverGuard::enter();
    let mut m = Machine {
        expr: Value::nil(),
        env: EnvId::GLOBAL,
        lazy,
        entered: &mut entered,
        tail,
        konts,
        capture: crate::process::in_capture_run(),
        park: None,
    };
    let first = match redo {
        Redo::Step { expr, env } => {
            m.expr = heap.read_root(expr);
            m.env = heap.read_root_env(env);
            redo_marks.restore(heap);
            Ok(Step::Continue)
        }
        Redo::Apply {
            callee,
            argv,
            call_form,
            env,
        } => {
            let callee = heap.read_root(callee);
            let argv = argv.iter().map(|&r| heap.read_root(r)).collect();
            let call_form = heap.read_root(call_form);
            let env = heap.read_root_env(env);
            redo_marks.restore(heap);
            m.apply(heap, callee, argv, call_form, env)
        }
    };
    let out = m.drive(heap, first);
    m.finish(heap, out, floors)
}

struct BindState {
    /// Name/value-interleaved, every name already checked to be a symbol.
    binds: SmallVec<[Root; 8]>,
    /// The binding whose value is pending.
    i: usize,
    scope_r: EnvRoot,
    body_r: Root,
    /// Plain `let`: a later binding goes into a fresh child frame once an earlier value could
    /// have captured this one (see `bind_sequential`'s history in the devlog, and below).
    lexical: bool,
    captured: bool,
}

#[derive(Clone, Copy)]
enum LiteralKind {
    Vector,
    Set,
    /// `src`/`out` hold key, value, key, value, …
    Map,
}

struct LiteralState {
    kind: LiteralKind,
    src: SmallVec<[Root; 8]>,
    out: SmallVec<[Root; 8]>,
    env_r: EnvRoot,
}

struct ArgsState {
    callee_r: Root,
    call_form_r: Root,
    /// The cell whose form is the next (or the running) argument.
    spine_r: Root,
    env_r: EnvRoot,
    args: SmallVec<[Root; 4]>,
}

/// A closure entry waiting on its unsupplied `&optional` defaults (`Bound` holds the
/// unrooted original).
struct EntryState {
    /// Read before binding: a default may compact the RUNTIME region and stale the
    /// closure's id, and a `Symbol` is GC-stable.
    name: Option<Symbol>,
    module: Option<Symbol>,
    /// The call form, for a call this frame made; `None` across a native boundary.
    site: Option<Root>,
    scope_r: EnvRoot,
    defaults: SmallVec<[(Symbol, Root); 4]>,
    /// The default whose value is pending.
    i: usize,
    rest: Option<Symbol>,
    body: SmallVec<[Root; 4]>,
}

impl EntryState {
    fn define(&mut self, heap: &mut Heap, val: Value) {
        let scope = heap.read_root_env(self.scope_r);
        heap.env_define(scope, self.defaults[self.i].0, val);
        self.i += 1;
    }

    /// An error leaving a default: a failed bind, attributed as one.
    fn unwound(&self, heap: &Heap, e: LispError) -> LispError {
        bind_failed(heap, e, self.name, self.site.map(|r| heap.read_root(r)))
    }
}

/// A bind failure (arity, or an `&optional` default's error) for a call of a closure named
/// `name`: from a call site, tagged with the call and carrying a name-only trace entry for
/// the callee, as the VM attributes it; across a native boundary the boundary frame's own
/// entry already names the callee.
fn bind_failed(heap: &Heap, e: LispError, name: Option<Symbol>, site: Option<Value>) -> LispError {
    match site {
        None => e,
        Some(call_form) => {
            let mut e = e.or_form_pos(heap, call_form);
            e.push_trace(crate::error::TraceFrame {
                name: name.map(value::symbol_name_ref),
                file: None,
                pos: None,
            });
            e
        }
    }
}

/// What one step left for the driver.
enum Step {
    /// The running frame produced its value.
    Value(Value),
    /// `expr`/`env` were replaced — a tail position, or a sub-form whose continuation was
    /// pushed. Step again.
    Continue,
}

/// A form that evaluates without running anything: an atom is itself, and a symbol bound
/// here reads its binding. Neither can collect or fail, so a caller evaluates it in place
/// instead of pushing a frame. `None` for everything else — a call, a collection literal, or
/// an unbound symbol (whose lookup may autoload a module).
#[inline]
fn leaf_value(heap: &Heap, form: Value, env: EnvId) -> Option<Value> {
    match form.unpack() {
        ValueRef::Sym(s) => heap
            .env_get(env, s)
            .map(|v| crate::builtins::contracts::boundary_resolve(heap, env, v)),
        ValueRef::Pair(_) | ValueRef::Vector(_) | ValueRef::Map(_) | ValueRef::Set(_) => None,
        _ => Some(form),
    }
}

/// A symbol's value in `env`. A miss may autoload the name's module (ADR-335), which runs
/// code and can collect; the caller roots what it holds across that.
fn lookup(heap: &mut Heap, env: EnvId, s: Symbol) -> LispResult {
    match heap.env_get(env, s) {
        Some(v) => Ok(crate::builtins::contracts::boundary_resolve(heap, env, v)),
        None => crate::eval::derive::global_miss(heap, env, s),
    }
}

/// What a symbol miss resolved to in a machine ([`machine_miss`]).
enum MachineMiss {
    /// The name's value, loaded by the miss if it had to be.
    Bound(Value),
    /// Another process holds the load of the name's module: the prelude's `%lazy-global` and
    /// the module, to run as frames of this machine so the wait in `%require-await` parks.
    HandOff(Value, Symbol),
}

/// The miss on `s` in `env`, as the VM's `exec_chunk` makes it. A machine that can park
/// (`capture`) takes the parkable miss (`derive::parkable_miss`), which hands the wait off
/// when another process holds the module's load — including one that takes the claim while
/// this miss is on its way to it; under `global_miss` that wait would block the worker. Any
/// other machine makes the plain miss. A load runs code and can collect, so `form` and `env`
/// are rooted across it and returned re-read, beside the outcome.
#[cold]
#[inline(never)]
fn machine_miss(
    heap: &mut Heap,
    capture: bool,
    env: EnvId,
    s: Symbol,
    form: Value,
) -> (Result<MachineMiss, LispError>, Value, EnvId) {
    use crate::eval::derive::{ParkableMiss, global_miss, parkable_miss};
    let (vb, eb) = (heap.roots_len(), heap.env_roots_len());
    let form_r = heap.root(form);
    let env_r = heap.root_env(env);
    let missed = match capture {
        true => parkable_miss(heap, env, s, None),
        false => global_miss(heap, env, s).map(ParkableMiss::Bound),
    };
    let missed = match missed {
        Ok(ParkableMiss::Bound(v)) => Ok(MachineMiss::Bound(v)),
        Ok(ParkableMiss::Elsewhere(module)) => {
            match heap.env_get(heap.global(), crate::core::value::intern("%lazy-global")) {
                Some(helper) => Ok(MachineMiss::HandOff(helper, module)),
                // No `%lazy-global` (a bare boot without the prelude): the plain miss, which
                // waits where it is.
                None => {
                    let env = heap.read_root_env(env_r);
                    global_miss(heap, env, s).map(MachineMiss::Bound)
                }
            }
        }
        Err(e) => Err(e),
    };
    let form = heap.read_root(form_r);
    let env = heap.read_root_env(env_r);
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    (missed, form, env)
}

/// The cdr of a cons cell, `nil` for anything else.
fn cdr(heap: &Heap, cell: Value) -> Value {
    match cell.unpack() {
        ValueRef::Pair(p) => heap.pair(p).1,
        _ => Value::nil(),
    }
}

/// `BROOD_EVAL_TRACE`: print each form a frame starts on.
#[cfg(debug_assertions)]
pub(super) fn eval_trace(heap: &Heap, form: Value) {
    static EVAL_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if crate::diagnostics::debug_flags::is_armed(&EVAL_TRACE, "BROOD_EVAL_TRACE") {
        eprintln!("[eval-trace] {}", crate::syntax::printer::print(heap, form));
    }
}

#[cfg(not(debug_assertions))]
pub(super) fn eval_trace(_: &Heap, _: Value) {}

/// The tree-walker's registers and its frame stack.
pub(super) struct Machine<'e> {
    expr: Value,
    env: EnvId,
    /// Consecutive LAZY macro expansions in the running frame (a macro defined in the same
    /// top-level form as its use, so the compile pass never saw it). Each re-enters the loop
    /// with the expansion as `expr`; a macro that expands to itself would otherwise spin here
    /// forever — the compile-time `macroexpand` bounds its fixpoint at `MAX_EXPAND_ROUNDS`,
    /// and this is the same bound for the runtime path. Reset whenever a closure body is
    /// entered, so a tail loop THROUGH a lazily expanded macro (one expansion per iteration)
    /// is never charged for its iterations.
    lazy: u32,
    /// The running frame's trace tracker. At the bottom of the stack this is the caller's
    /// (`eval` reads it after the run).
    entered: &'e mut Option<TwFrame>,
    /// The running frame's tail tag (see [`TailTag`]).
    tail: TailTag,
    konts: KontStack,
    /// The process's body driver (a green process at tier 0): may park ([`run_captured`]).
    capture: bool,
    /// Set when a capturing machine parks; the run then returns with its frames intact.
    park: Option<Park>,
}

impl<'e> Machine<'e> {
    pub(super) fn new(expr: Value, env: EnvId, entered: &'e mut Option<TwFrame>) -> Self {
        Machine {
            expr,
            env,
            lazy: 0,
            entered,
            tail: TailTag::None,
            konts: KontStack::default(),
            capture: false,
            park: None,
        }
    }

    /// The end of a capturing run: the continuation if it parked, else the outcome, with the
    /// heap's stacks back at `floors`.
    fn finish(
        &mut self,
        heap: &mut Heap,
        out: LispResult,
        floors: Floors,
    ) -> Result<crate::eval::compile::VmOutcome, LispError> {
        use crate::eval::compile::{Continuation, VmOutcome};
        if let Some(park) = self.park.take() {
            let cont = Box::new(TwSuspended {
                konts: std::mem::take(&mut self.konts),
                entered: self.entered.take(),
                lazy: self.lazy,
                tail: self.tail,
                redo: park.redo,
                redo_marks: park.marks,
                floors,
            });
            return Ok(match park.suspend {
                true => VmOutcome::Suspended(Continuation::Tw(cont)),
                false => VmOutcome::Preempted(Continuation::Tw(cont)),
            });
        }
        let out = out.map_err(|e| close_frame(e, self.entered.take()));
        floors.restore(heap);
        out.map(VmOutcome::Done)
    }

    /// Apply closure `cl` to `argv` across a native boundary (`apply_closure`): its body,
    /// `&optional` defaults included, runs on this machine as the bottom frame. `entered`
    /// is the frame's trace tracker, for the caller to close. Runs under the caller's
    /// `root_scope`, which drops the roots the bottom frame keeps.
    pub(super) fn run_closure(
        heap: &mut Heap,
        cl: ClosureId,
        argv: &[Value],
        entered: &'e mut Option<TwFrame>,
    ) -> LispResult {
        // `expr`/`env` are placeholders until `enter` hands the loop a form.
        let mut m = Machine::new(Value::nil(), EnvId::GLOBAL, entered);
        let first = m.enter(heap, cl, argv, None);
        m.drive(heap, first)
    }

    /// Run until the bottom frame has its value or its error.
    pub(super) fn run(&mut self, heap: &mut Heap) -> LispResult {
        let first = self.step(heap);
        self.drive(heap, first)
    }

    /// Drive the loop on from the running frame's step `next`.
    fn drive(&mut self, heap: &mut Heap, mut next: Result<Step, LispError>) -> LispResult {
        loop {
            let mut outcome = match next {
                Ok(Step::Continue) => {
                    next = self.step(heap);
                    continue;
                }
                Ok(Step::Value(v)) => Ok(v),
                Err(e) => Err(e),
            };
            // Hand the finished frame's outcome to the frames below it until one of them
            // has more to evaluate.
            loop {
                // A parked machine keeps its frames: the run returns as it stands.
                if outcome.is_err() && self.park.is_some() {
                    return outcome;
                }
                let Some(k) = self.konts.pop() else {
                    // The bottom frame's trace entry is the caller's to close.
                    return outcome.map_err(|e| self.tail.tag(heap, e));
                };
                let inner = std::mem::replace(self.entered, k.entered);
                let inner_tail = std::mem::replace(&mut self.tail, k.tail);
                self.lazy = k.lazy;
                let resumed = match outcome {
                    Ok(v) => self.resume(heap, k.kind, k.marks, v),
                    Err(e) => {
                        let mut e = close_frame(inner_tail.tag(heap, e), inner);
                        match k.kind {
                            // A control signal (a kill, a suspend) is never caught.
                            KontKind::Try {
                                handler_r,
                                env_r,
                                described,
                            } if !e.is_control() => {
                                let call_form_r = k.form.expect("a Try frame keeps its call");
                                self.catch(
                                    heap,
                                    e,
                                    handler_r,
                                    env_r,
                                    described,
                                    call_form_r,
                                    k.marks,
                                )
                            }
                            // The window this frame opened closes BEFORE the interrupt is
                            // noted as caught, so the sticky re-raise is not armed (ADR-491).
                            KontKind::Interruptible { window }
                                if !e.is_control()
                                    && e.kind == crate::error::ErrorKind::Interrupted
                                    && crate::process::interrupt_raised_for(window) =>
                            {
                                crate::process::close_interrupt_window(window);
                                k.marks.restore(heap);
                                let caught = e.caught_value(heap);
                                Ok(Step::Value(
                                    heap.alloc_vector2(value::kw("interrupted"), caught),
                                ))
                            }
                            kind => {
                                if let Some(form_r) = k.form {
                                    let form = heap.read_root(form_r);
                                    e = e.or_form_pos(heap, form);
                                }
                                match &kind {
                                    KontKind::Defaults(st) => e = st.unwound(heap, e),
                                    KontKind::Binding { dynamics } => {
                                        heap.truncate_dynamics(*dynamics)
                                    }
                                    // Any other error, a kill included, still ends the
                                    // window this frame opened.
                                    KontKind::Interruptible { window } => {
                                        crate::process::close_interrupt_window(*window)
                                    }
                                    _ => {}
                                }
                                k.marks.restore(heap);
                                Err(e)
                            }
                        }
                    }
                };
                match resumed {
                    Ok(Step::Continue) => break,
                    Ok(Step::Value(v)) => outcome = Ok(v),
                    Err(e) => outcome = Err(e),
                }
            }
            next = self.step(heap);
        }
    }

    /// Set the running frame aside behind `kind` and start a frame evaluating `form` in
    /// `env`. `marks` are the depths from before `kind` rooted anything; `tag` says whether an
    /// error leaving `form` takes its position.
    fn push(
        &mut self,
        heap: &mut Heap,
        marks: Marks,
        kind: KontKind,
        form: Value,
        tag: bool,
        env: EnvId,
    ) -> Result<Step, LispError> {
        self.push_tagging(heap, marks, kind, form, tag.then_some(form), env)
    }

    /// [`push`](Self::push), with the form an error from the new frame is tagged with given
    /// separately: an argument that has no position of its own (a bare symbol) is tagged with
    /// its call, so an unbound name there reports at the call — as the VM does — rather than
    /// at whatever enclosing form has a position.
    fn push_tagging(
        &mut self,
        heap: &mut Heap,
        marks: Marks,
        kind: KontKind,
        form: Value,
        tag: Option<Value>,
        env: EnvId,
    ) -> Result<Step, LispError> {
        let within = self.save(heap, marks, kind, tag);
        eval_trace(heap, form);
        if !within {
            // Raised by the new frame, as the old nested `eval` raised it on entry.
            return Err(tw_frame_depth_error(MAX_TW_FRAMES).or_form_pos(heap, form));
        }
        self.expr = form;
        self.env = env;
        Ok(Step::Continue)
    }

    /// Set the running frame aside behind `kind` and clear the registers for a new frame;
    /// `false` when that frame is past the limit (the new frame raises it).
    fn save(&mut self, heap: &mut Heap, marks: Marks, kind: KontKind, tag: Option<Value>) -> bool {
        let form = tag.map(|f| heap.root(f));
        self.konts.push(Kont {
            entered: self.entered.take(),
            lazy: std::mem::take(&mut self.lazy),
            tail: std::mem::replace(&mut self.tail, TailTag::None),
            form,
            marks,
            kind,
        });
        self.konts.len() <= MAX_TW_FRAMES
    }

    /// Continue the frame that pushed `kind`, now that its sub-form produced `v`. The
    /// frame's registers are already restored. A path that finishes with `kind` restores
    /// `marks` before anything that can fail; a path that needs another sub-form re-pushes
    /// with the same `marks`.
    fn resume(
        &mut self,
        heap: &mut Heap,
        kind: KontKind,
        marks: Marks,
        v: Value,
    ) -> Result<Step, LispError> {
        match kind {
            KontKind::If {
                then_r,
                else_r,
                env_r,
            } => {
                let branch = heap.read_root(if truthy(v) { then_r } else { else_r });
                self.env = heap.read_root_env(env_r);
                marks.restore(heap);
                self.expr = branch;
                Ok(Step::Continue)
            }
            KontKind::Body { cursor, env_r } => {
                let next = cdr(heap, heap.read_root(cursor));
                let env = heap.read_root_env(env_r);
                marks.restore(heap);
                self.body(heap, next, env)
            }
            KontKind::Forms { rest, env_r } => {
                let rest: SmallVec<[Value; 4]> = rest.iter().map(|&r| heap.read_root(r)).collect();
                let env = heap.read_root_env(env_r);
                marks.restore(heap);
                self.forms(heap, &rest, env)
            }
            KontKind::Bind(mut st) => {
                st.define(heap, v);
                st.i += 2;
                self.binds(heap, st, marks)
            }
            KontKind::Def { name, env_r } => {
                let env = heap.read_root_env(env_r);
                marks.restore(heap);
                def_bind(heap, name, v, env).map(Step::Value)
            }
            KontKind::Literal(mut st) => {
                let r = heap.root(v);
                st.out.push(r);
                self.literal_next(heap, st, marks)
            }
            KontKind::Head { call_form_r, env_r } => {
                let call_form = heap.read_root(call_form_r);
                let env = heap.read_root_env(env_r);
                marks.restore(heap);
                let spine = cdr(heap, call_form);
                // A symbol head got here through `%lazy-global` (another process was loading
                // its module), whose answer may be a macro — which the direct lookup expands,
                // so this does too. A computed head's value is called as it is.
                if let ValueRef::Macro(mid) = v.unpack()
                    && let ValueRef::Pair(p) = call_form.unpack()
                    && matches!(heap.pair(p).0.unpack(), ValueRef::Sym(_))
                {
                    self.env = env;
                    return self.expand_macro(heap, mid, call_form, spine);
                }
                self.call(heap, v, call_form, spine, env)
            }
            KontKind::Args(mut st) => {
                let r = heap.root(v);
                st.args.push(r);
                let next = cdr(heap, heap.read_root(st.spine_r));
                st.spine_r = heap.advance_root(st.spine_r, next);
                self.args_next(heap, st, marks)
            }
            KontKind::Defaults(mut st) => {
                st.define(heap, v);
                self.defaults(heap, st, marks)
            }
            KontKind::Expand { env_r } => {
                // A template may emit `/name` — the root escape (ADR-236) that stops a user
                // module capturing a prelude reference. The compile pass resolves it, but a
                // macro reached HERE was not expanded at compile time (it was defined after
                // its use site, or defined at runtime), so nothing has rewritten it and the
                // evaluator would see the literal symbol `/nil?` and call it unbound. Strip it
                // on the way out — the runtime half of `macros::resolve`'s root case. Cheap:
                // the scan allocates nothing and rebuilds only when an escape is present.
                let expanded = crate::eval::macros::strip_root_escapes(heap, v);
                self.env = heap.read_root_env(env_r);
                marks.restore(heap);
                self.expr = expanded;
                Ok(Step::Continue)
            }
            KontKind::Try { .. } | KontKind::Callback => {
                marks.restore(heap);
                Ok(Step::Value(v))
            }
            KontKind::Binding { dynamics } => {
                heap.truncate_dynamics(dynamics);
                marks.restore(heap);
                Ok(Step::Value(v))
            }
            KontKind::Interruptible { window } => {
                crate::process::close_interrupt_window(window);
                marks.restore(heap);
                Ok(Step::Value(heap.alloc_vector2(value::kw("ok"), v)))
            }
        }
    }

    /// One step of the running frame: a leaf answers, a special form or a call either
    /// answers, hands a tail position back to the loop, or pushes a sub-form.
    fn step(&mut self, heap: &mut Heap) -> Result<Step, LispError> {
        match self.expr.unpack() {
            ValueRef::Sym(s) => {
                return match heap.env_get(self.env, s) {
                    Some(v) => Ok(Step::Value(crate::builtins::contracts::boundary_resolve(
                        heap, self.env, v,
                    ))),
                    None => self.symbol_miss(heap, s),
                };
            }
            ValueRef::Vector(id) => {
                let items = heap.vector(id).to_vec();
                return self.literal(heap, LiteralKind::Vector, &items);
            }
            ValueRef::Map(id) => {
                // Key, value, key, value — evaluated in that order, then canonicalised
                // (last-wins on equal keys).
                let items: Vec<Value> = heap
                    .map_entries(id)
                    .into_iter()
                    .flat_map(|(k, v)| [k, v])
                    .collect();
                return self.literal(heap, LiteralKind::Map, &items);
            }
            ValueRef::Set(id) => {
                let items = heap.set_elems(id);
                return self.literal(heap, LiteralKind::Set, &items);
            }
            ValueRef::Pair(_) => {} // combination, handled below
            _ => return Ok(Step::Value(self.expr)),
        }

        // GC safepoint — fires at ANY eval depth (ADR-061). The loop-top is the one point in
        // a step where the only live LOCAL transients are `expr` / `env`; they're passed as
        // roots below. Every pending frame's transients sit on the heap operand stack
        // (`roots`/`env_roots`), which `collect` relocates in place, and the dynamic stack is
        // scanned too — so no Rust frame holds an unrooted LOCAL handle a moving collection
        // would strand. The one exception is the macro-expansion compile pass (its forms
        // aren't operand-stack rooted), which opts out via `MACRO_BLOCK`. (The soft-memory
        // limit check below is deliberately *not* gated.) Cost when not collecting: one TLS
        // read + a `gc_due` compare.
        if !crate::process::macro_block_active() && heap.gc_due() {
            // Copying collection: `expr`/`env` MOVE to fresh slabs, so write the relocated
            // handles back into the registers (the dynamic stack and explicit root stack are
            // relocated in place by `collect`).
            let mut roots = [self.expr];
            let mut envs = [self.env];
            heap.collect(&mut roots, &mut envs);
            self.expr = roots[0];
            self.env = envs[0];
        }
        // RUNTIME-region safepoint — auto-compact the shared code region once hot-reload churn
        // crosses the threshold (the shared-code analog of the LOCAL collect above). Same
        // `macro_block_active` gate and same `expr`/`env` rewrite contract:
        // `maybe_runtime_collect` only rewrites RUNTIME handles (never moves LOCAL data), and
        // the rooted set it touches — globals + `roots`/`env_roots`/`dynamics` + both LOCAL
        // gens — is exactly the ADR-061 invariant the LOCAL safepoint already relies on (so
        // the VM's frame slots on `roots` are covered), plus the live `expr`/`env` passed
        // here. Runs only when this heap uniquely owns the runtime (single-process /
        // quiescent); a shared runtime backs off. Gated on `rt_dirty` (a real mint since the
        // last check), mirroring the VM safepoint: without it, `rt_gc_due` — a `cur_code()`
        // `ArcSwap` load — ran on every tree-walk step, and during a lingering multigen
        // drain that is pure overhead.
        if heap.rt_dirty() && !crate::process::macro_block_active() {
            heap.rt_dirty_clear();
            if heap.rt_gc_due() {
                let mut roots = [self.expr];
                let mut envs = [self.env];
                heap.maybe_runtime_collect(&mut roots, &mut envs);
                self.expr = roots[0];
                self.env = envs[0];
            }
        }
        // RUNTIME-drain cooperative report (ADR-091 Stage 3c). While a generation drain is
        // armed (rare), report whether this process still references the draining
        // generation, so the collector's cross-process union learns when it's dead. The
        // probe is read-only (it moves nothing), so it needs no GC/macro gate; the hot path
        // is one atomic load returning false.
        if heap.drain_active() {
            crate::process::report_drain_liveness(heap);
        }
        // Memory safety backstop (ADR-043): if total allocation has crossed the soft
        // ceiling, fail *here* with a clean, catchable error rather than running on to the
        // hard allocator limit (which aborts the whole process). Off by default; the test
        // runners set it so adversarial / hostile code can't exhaust host RAM. Checked at
        // every step, so a runaway loop sitting in *argument* position (`(f (build …))`) is
        // caught too. Cheap when disabled: one relaxed load of a zero, early `None`.
        // Crossing it stops the process holding the most memory, which is not necessarily
        // this one (`process/pressure.rs`, KI-229).
        match crate::core::alloc::soft_limit_state() {
            crate::core::alloc::SoftLimit::Clear => {}
            crate::core::alloc::SoftLimit::Near => crate::process::near_soft_limit(heap),
            crate::core::alloc::SoftLimit::Over(used) => {
                if let Some(error) = crate::process::soft_limit_crossed(heap, used) {
                    return Err(error.or_form_pos(heap, self.expr));
                }
            }
        }
        // Per-process heap limit (`(proc/flag :max-heap n)`): the sticky flag a collection
        // armed raises here, killing (catchably) just this process.
        if let Some(live) = heap.take_proc_limit_hit() {
            let limit = heap.proc_mem_limit().unwrap_or(0);
            return Err(proc_memory_limit_error(live, limit).or_form_pos(heap, self.expr));
        }
        // Mailbox bound (`(proc/flag :max-mailbox n)`, ADR-307): the breach a sender armed
        // raises here, in the flooded process only.
        if let Some((len, limit)) = crate::process::take_current_mailbox_overflow() {
            return Err(proc_mailbox_limit_error(len, limit).or_form_pos(heap, self.expr));
        }
        // Reduction-counted preemption: bound the work a process does before it yields its
        // worker (fairness — a CPU-bound process can't monopolise a core). Counted per
        // *combination* (a function call / special form), which is where loops actually
        // occur — leaf evals (symbols, literals) return above without a tick. A cheap
        // thread-local decrement; a no-op for the root thread. See `process::tick`.
        //
        // On the rollover the tick also honours a pending hard `:kill`
        // (`tick_nested`): the tree-walker runs nested behind native frames
        // (`eval`/`reflect/eval-string`), so it can't return a `Killed` outcome — but it can
        // *unwind*, and the untrappable kill signal is converted to process death by the
        // body driver. Without this (and the same check in the passthrough redirect, where
        // thin-wrapper-heavy loops actually tick), a green process spinning inside `eval`
        // could not be killed at all.
        //
        // A capturing machine is its process's driver, so it takes the driver's safepoint
        // instead, as the VM's loop top does: on a spent budget with work waiting it parks
        // here, before the step has done anything, and resumes by stepping `expr` again.
        if self.capture {
            if let crate::process::Safepoint::Quantum { yield_now } =
                crate::process::driver_safepoint(true)
            {
                // The once-per-quantum half of the VM's checks (`quantum_checks`); the rest
                // run on every step above.
                crate::core::alloc::refresh_container_limits();
                if crate::process::capture_hard_kill_pending() {
                    return Err(LispError::kill_signal());
                }
                crate::eval::check_interrupt()?;
                if yield_now {
                    let marks = Marks::take(heap);
                    let redo = Redo::Step {
                        expr: heap.root(self.expr),
                        env: heap.root_env(self.env),
                    };
                    self.park = Some(Park {
                        suspend: false,
                        redo,
                        marks,
                    });
                    return Err(LispError::suspend(None));
                }
            }
        } else {
            // A kill is deliberately undecorated (no `or_form_pos`): a control signal is
            // not a user-visible error — it never prints, it only unwinds to the driver.
            crate::process::tick_nested()?;
        }

        let expr = self.expr;
        let (head, rest) = match expr.unpack() {
            ValueRef::Pair(p) => heap.pair(p),
            _ => unreachable!(),
        };

        // --- special forms ---
        if let ValueRef::Sym(s) = head.unpack() {
            match special_form(s) {
                Some(SpecialForm::Quote) => {
                    // `(quote x)` returns x literally — but only x; reject `(quote a b)`
                    // rather than silently dropping the tail. A *missing* operand is the
                    // degenerate case, not a dropped one, and follows the house rule the
                    // other forms use — `(do)`, `(if)`, `((fn ()))` all yield nil, so
                    // `(quote)` does too (tests/malformed_test.blsp §degenerate special
                    // forms).
                    let (form, r) = uncons(heap, rest);
                    if !matches!(r.unpack(), ValueRef::Nil) {
                        return Err(LispError::arity("quote: expected exactly one argument")
                            .or_form_pos(heap, expr));
                    }
                    return Ok(Step::Value(form));
                }
                Some(SpecialForm::If) => {
                    // (if test then else?) — read the operands straight off the cons spine;
                    // a missing branch defaults to nil, so no intermediate Vec is allocated
                    // per conditional.
                    let (test_form, r) = uncons(heap, rest);
                    let (then_form, r) = uncons(heap, r);
                    let (else_form, r) = uncons(heap, r);
                    // The else-branch is a *single* form — reject a trailing tail rather
                    // than silently dropping it. In Emacs Lisp `(if c then e1 e2 …)` runs
                    // the trailing forms as an implicit-`progn` else; that muscle memory
                    // would otherwise vanish here without a peep. Point at `do`.
                    if !matches!(r.unpack(), ValueRef::Nil) {
                        return Err(LispError::arity(
                            "if: too many arguments — the else-branch is a single form; \
                             wrap multiple forms in (do …)",
                        )
                        .or_form_pos(heap, expr));
                    }
                    if let Some(test) = leaf_value(heap, test_form, self.env) {
                        self.expr = if truthy(test) { then_form } else { else_form };
                        return Ok(Step::Continue);
                    }
                    let marks = Marks::take(heap);
                    let then_r = heap.root(then_form);
                    let else_r = heap.root(else_form);
                    let env_r = heap.root_env(self.env);
                    let kind = KontKind::If {
                        then_r,
                        else_r,
                        env_r,
                    };
                    return self.push(heap, marks, kind, test_form, true, self.env);
                }
                Some(SpecialForm::Do) => return self.body(heap, rest, self.env),
                Some(SpecialForm::Def) => return self.def(heap, expr, rest),
                Some(SpecialForm::Fn) => {
                    // Fallback: a multi-clause / pattern-parameter `fn` normally lowers to
                    // `match*` in the compile pass, but can reach eval unlowered (built by a
                    // quasiquote, or a macro expanded lazily within its defining form).
                    // Lower it here and re-enter. The common case is detected away cheaply,
                    // so this never touches an ordinary `fn`.
                    if crate::eval::macros::fn_needs_lowering(heap, expr) {
                        self.expr = crate::eval::macros::macroexpand_all(heap, expr, self.env)?;
                        return Ok(Step::Continue);
                    }
                    let module = top_level_module(heap, self.env);
                    return make_closure(heap, None, rest, self.env, module).map(Step::Value);
                }
                Some(SpecialForm::Quasiquote) => {
                    // One template, and no trailing tail — the same rule `quote` follows,
                    // for the same reason: `` `(a) b `` must not silently drop `b`. (And
                    // the same degenerate case: no template at all yields nil, like
                    // `(quote)` / `(do)` / `(if)`, because nothing is being *dropped*.)
                    let (template, r) = uncons(heap, rest);
                    if !matches!(r.unpack(), ValueRef::Nil) {
                        return Err(
                            LispError::arity("quasiquote: expected exactly one argument")
                                .or_form_pos(heap, expr),
                        );
                    }
                    // Expand the template into *builder code* (a pure structural transform
                    // — no eval re-entry, so no GC-rooting hazard), then hand it back to
                    // the loop to evaluate. Tail position: the builder code's value is this
                    // form's value, and the loop-top safepoint roots the new `expr`.
                    self.expr = crate::eval::macros::expand_quasiquote(heap, template)
                        .map_err(|e| e.or_form_pos(heap, expr))?;
                    return Ok(Step::Continue);
                }
                Some(SpecialForm::Let) => return self.let_form(heap, expr, rest, false),
                Some(SpecialForm::Letrec) => return self.let_form(heap, expr, rest, true),
                None => {}
            }
        }

        // --- macro expansion + function application ---
        // Resolve the head once: a symbol head is looked up in the environment (a single
        // walk, shared by the macro check and the callee), any other head form is
        // evaluated. A macro expands and re-enters the loop; anything else becomes the
        // callee. `call_form` is this combination — the fallback position for an error from
        // a sub-eval / primitive / closure-arity check that didn't tag itself with a
        // more-inner one (`or_form_pos` checks `pos.is_none()` first, so the innermost
        // annotation wins).
        let mut call_form = expr;
        let mut spine = rest;
        let callee = match head.unpack() {
            ValueRef::Sym(s) => {
                // An unbound-symbol error from a *tail-position* call exits this frame
                // directly — no outer `or_form_pos` will see it. Attach `call_form`'s
                // position here so the diagnostic points at the failing call's line, not the
                // enclosing top-level form's start. A hit does no GC work; a MISS may
                // autoload the module (ADR-335), which collects — so that path roots
                // `call_form` + `env` and re-derives the spine.
                let v = match heap.env_get(self.env, s) {
                    Some(v) => crate::builtins::contracts::boundary_resolve(heap, self.env, v),
                    None => {
                        let (missed, new_call_form, new_env) =
                            machine_miss(heap, self.capture, self.env, s, call_form);
                        call_form = new_call_form;
                        self.env = new_env;
                        spine = cdr(heap, call_form);
                        match missed.map_err(|e| e.or_form_pos(heap, call_form))? {
                            MachineMiss::Bound(v) => v,
                            // Another process holds the head's module (`head_handoff`).
                            MachineMiss::HandOff(helper, module) => {
                                return self.head_handoff(heap, s, call_form, helper, module);
                            }
                        }
                    }
                };
                if let ValueRef::Macro(mid) = v.unpack() {
                    return self.expand_macro(heap, mid, call_form, spine);
                }
                v
            }
            _ => match leaf_value(heap, head, self.env) {
                Some(callee) => callee,
                None => {
                    // A computed head (`((f) …)`) is evaluated as a frame of its own; the
                    // call resumes from `Head` with the callee.
                    let marks = Marks::take(heap);
                    let call_form_r = heap.root(call_form);
                    let env_r = heap.root_env(self.env);
                    let kind = KontKind::Head { call_form_r, env_r };
                    return self.push(heap, marks, kind, head, true, self.env);
                }
            },
        };
        self.call(heap, callee, call_form, spine, self.env)
    }

    /// A symbol `s` unbound in this frame's environment. When another process holds its
    /// module's load, `%lazy-global` waits for that load as frames of this machine, so the
    /// wait parks and its value answers this frame; otherwise the plain miss ([`machine_miss`]).
    /// Out of line: `step` is the loop's hot path, and this is its rare branch.
    #[cold]
    #[inline(never)]
    fn symbol_miss(&mut self, heap: &mut Heap, s: Symbol) -> Result<Step, LispError> {
        let (missed, expr, env) = machine_miss(heap, self.capture, self.env, s, self.expr);
        self.expr = expr;
        self.env = env;
        match missed? {
            MachineMiss::Bound(v) => Ok(Step::Value(v)),
            MachineMiss::HandOff(helper, module) => {
                let argv = SmallVec::from_slice(&[Value::symbol(module), Value::symbol(s)]);
                self.apply(heap, helper, argv, self.expr, self.env)
            }
        }
    }

    /// A call head `s` unbound in this frame's environment whose module another process holds
    /// the load of: the head is `helper`'s (`%lazy-global`'s) value, computed as a frame of
    /// its own like a computed head (`KontKind::Head`), so the wait parks.
    fn head_handoff(
        &mut self,
        heap: &mut Heap,
        s: Symbol,
        call_form: Value,
        helper: Value,
        module: Symbol,
    ) -> Result<Step, LispError> {
        let marks = Marks::take(heap);
        let call_form_r = heap.root(call_form);
        let env_r = heap.root_env(self.env);
        let kind = KontKind::Head { call_form_r, env_r };
        if !self.save(heap, marks, kind, Some(call_form)) {
            return Err(tw_frame_depth_error(MAX_TW_FRAMES).or_form_pos(heap, call_form));
        }
        let argv = SmallVec::from_slice(&[Value::symbol(module), Value::symbol(s)]);
        self.apply(heap, helper, argv, call_form, self.env)
    }

    /// A macro call: run the expander as a frame of its own (`KontKind::Expand`), whose
    /// value — the expansion — this frame then evaluates in the call's place.
    fn expand_macro(
        &mut self,
        heap: &mut Heap,
        mid: ClosureId,
        call_form: Value,
        spine: Value,
    ) -> Result<Step, LispError> {
        self.lazy += 1;
        if self.lazy > crate::eval::macros::MAX_EXPAND_ROUNDS {
            return Err(LispError::runtime(format!(
                "macro expansion did not reach a fixpoint after {} rounds \
                 (a macro that expands to itself?)",
                crate::eval::macros::MAX_EXPAND_ROUNDS
            ))
            .or_form_pos(heap, call_form));
        }
        // The argument forms go straight into the expander's frame: binding runs nothing,
        // so they need no rooting.
        let arg_forms = heap.list_to_vec(spine)?;
        let marks = Marks::take(heap);
        let env_r = heap.root_env(self.env);
        // Above tier 0 an expander with a compiled arm runs on the VM, as any native-boundary
        // call of it would (`apply_closure`).
        if let Some(out) = tw_vm_route(heap, mid, &arg_forms) {
            let out = out.map(|v| crate::eval::macros::strip_root_escapes(heap, v));
            self.env = heap.read_root_env(env_r);
            marks.restore(heap);
            self.expr = out.map_err(|e| e.or_form_pos(heap, call_form))?;
            return Ok(Step::Continue);
        }
        if !self.save(heap, marks, KontKind::Expand { env_r }, Some(call_form)) {
            return Err(tw_frame_depth_error(MAX_TW_FRAMES).or_form_pos(heap, call_form));
        }
        self.enter(heap, mid, &arg_forms, None)
    }

    /// `(def name value?)`.
    fn def(&mut self, heap: &mut Heap, expr: Value, rest: Value) -> Result<Step, LispError> {
        let args = heap.list_to_vec(rest)?;
        // `(def name value?)` — at most a name and one value. Reject a trailing tail rather
        // than silently dropping it (a `(def pi 3.14 "doc")` docstring habit, or stray forms
        // the user expected to run, would vanish without a peep). `def` takes no docstring
        // — use `defn`, or attach docs to the value.
        if args.len() > 2 {
            return Err(LispError::arity(
                "def: too many arguments — expected (def name value); \
                 def takes no docstring and runs no extra forms",
            )
            .or_form_pos(heap, expr));
        }
        let name = as_symbol(
            heap,
            "def: name",
            args.first()
                .copied()
                .ok_or_else(|| LispError::runtime("def: missing name"))?,
        )?;
        // ADR-070: root a qualified def TARGET exactly as a qualified *reference* roots.
        // During a module load this is a no-op — the expand pass already rewrote the target
        // to its rooted spelling. It matters for a `def` built as DATA and eval'd with no
        // compile-ns in scope, which is how instrumentation rebinds a function:
        // `std/tool/debug`'s `(eval (list 'def 'main/greet wrapper))`. Unrooted, that writes
        // a NEW global `main/greet` and leaves the real `brk/main/greet` — the one every
        // compiled call site resolves — untouched, so the "traced" function is never the
        // one that runs. Idempotent on an already-rooted name.
        let name = heap.root_qualified_ref(name).unwrap_or(name);
        // **Reserved names** (ADR-166): a global `def` may not rebind anything the language
        // ships — the prelude, the builtins, or an embedded std module. Checked here because
        // every `def` routes through this arm (the VM defers `def` forms to the evaluator),
        // so one gate covers both engines, `load`, `eval` and the REPL.
        //
        // **Before** the value is evaluated: the name is fully known by now, so running the
        // RHS first only meant a rejected `def` had already fired its side effects —
        // `(def map (do (print "SIDE") 1))` printed, *then* errored.
        // A special form's name is reserved too, and more absolutely: `(if …)` is the form
        // whatever the global table holds, so `(def if 1)` bound a global nothing could call
        // and only a bare `if` read back — a definition that worked by accident.
        if is_special_form(name) {
            let shown = value::symbol_name(name);
            return Err(LispError::type_err(format!(
                "def: `{shown}` is a special form — the language's syntax, not a global — and \
                 cannot be defined"
            ))
            .with_hint(format!(
                "A call `({shown} …)` is always the special form, so a global named `{shown}` \
                 could never be called. Pick another name."
            ))
            .or_form_pos(heap, expr));
        }
        if heap.is_reserved_global(name) {
            let shown = value::symbol_name(name);
            return Err(LispError::type_err(format!(
                "def: `{shown}` is a reserved name — it ships with Brood and cannot be redefined"
            ))
            .with_hint(format!(
                "Reserved names are everything inside the `brood` binary: the prelude, the \
                 builtins, and the embedded std modules. Your own code and your packages stay \
                 fully redefinable (that is what hot reload is for). Either pick another name, \
                 shadow it locally — `(let ({shown} …) …)` is still fine — or define it inside a \
                 `(defmodule your/mod …)`, where `(defn {shown} …)` defines `your/mod/{shown}` \
                 and is yours."
            ))
            .or_form_pos(heap, expr));
        }
        let Some(&rhs) = args.get(1) else {
            return def_bind(heap, name, Value::nil(), self.env).map(Step::Value);
        };
        // Above tier 0 the RHS runs on the VM (it's already macroexpanded+resolved if this
        // `def` came through the normal pipeline; `run` falls back to the tree-walker for
        // anything it can't compile). Without this a top-level `(def x <expr>)` evaluates
        // `<expr>` entirely on the tree-walker — `def` is a special form, so the whole form
        // (RHS included) was deferring. Not `run_top_form`: this tags the error with the
        // RHS's own position.
        if compile::tier_ceiling() >= compile::Tier::Bytecode {
            let env = self.env;
            let (v, new_env) = heap.root_scope(|heap| {
                let env_r = heap.root_env(env);
                let v = compile::run(heap, rhs, env).map_err(|e| e.or_form_pos(heap, rhs))?;
                Ok((v, heap.read_root_env(env_r)))
            })?;
            self.env = new_env;
            return def_bind(heap, name, v, new_env).map(Step::Value);
        }
        if let Some(v) = leaf_value(heap, rhs, self.env) {
            return def_bind(heap, name, v, self.env).map(Step::Value);
        }
        let marks = Marks::take(heap);
        let env_r = heap.root_env(self.env);
        self.push(
            heap,
            marks,
            KontKind::Def { name, env_r },
            rhs,
            true,
            self.env,
        )
    }

    /// `let` (sequential, lexical) or `letrec` (every name visible to every value).
    fn let_form(
        &mut self,
        heap: &mut Heap,
        expr: Value,
        rest: Value,
        rec: bool,
    ) -> Result<Step, LispError> {
        let who = if rec { "letrec" } else { "let" };
        let (binds_form, body) = uncons(heap, rest);
        if !matches!(rest.unpack(), ValueRef::Pair(_)) {
            return Err(LispError::runtime(format!("{who}: missing bindings")));
        }
        let binds = as_binding_vec(heap, who, binds_form)?;
        if binds.len() % 2 != 0 {
            return Err(scheme_binding_error(heap, who, &binds));
        }
        let pattern_target = binds
            .iter()
            .step_by(2)
            .any(|&b| !matches!(b.unpack(), ValueRef::Sym(_)));
        let scope = if rec {
            // Plain-symbol targets only — letrec exists for mutual recursion of *named*
            // values; pattern binding would muddy what "the names visible in the RHSs"
            // means.
            if pattern_target {
                // A non-symbol target is always an error here (letrec has no
                // destructuring), so flagging the nested-Scheme shape is zero-regression:
                // `(letrec ((f …) (g …)) …)` — even and function-valued, which the literal
                // guard misses — gets the flatten hint via scheme_binding_error's own
                // `looks_nested`.
                if binds.iter().all(|&b| {
                    matches!(b.unpack(), ValueRef::Pair(_))
                        && heap.seq_items(b).map(|it| it.len() == 2).unwrap_or(false)
                }) {
                    return Err(scheme_binding_error(heap, "letrec", &binds));
                }
                return Err(LispError::runtime(
                    "letrec: binding targets must be plain symbols",
                ));
            }
            // Mutual local recursion: every binding's name is visible in every binding's
            // RHS (and to itself). The frame is a last-write-wins association vector
            // (`env_define` pushes, lookup scans from the end), so all names are made
            // visible by pre-defining them to nil, then each RHS is evaluated in the scope
            // and the real value pushed — closures built during the bind phase capture the
            // scope and resolve names lazily at call time, so mutual recursion just works.
            // Brood stays immutable: touching a name before its RHS has been evaluated
            // reads the placeholder, but for the recursion-of-functions case letrec is
            // meant for, the bodies don't fire until call time, by which point the real
            // value is the latest entry. (No eval here → no GC.)
            let scope = heap.new_env(Some(self.env));
            for pair in binds.chunks(2) {
                let bind_name = as_symbol(heap, "binding name", pair[0])?;
                heap.env_define(scope, bind_name, Value::nil());
            }
            scope
        } else {
            // An *even*-length nested-Scheme form — `(let ((a 1) (b 2)) …)` — otherwise
            // slips past the odd-length guard above into the pattern-lowering fallback and
            // dies with a confusing "unbound symbol" (it evaluates `(b 2)` as a call).
            // Catch the literal-valued shape here so the flatten hint fires. Guarded by
            // `is_literal_atom` on every value slot so it can't regress a genuine bare-list
            // destructure like `(let ((a b) '(1 2)) …)`.
            if even_bindings_look_scheme(heap, &binds) {
                return Err(scheme_binding_error(heap, "let", &binds));
            }
            // Fallback: a pattern (non-symbol) binding target reached eval unlowered — same
            // paths as `fn`. Lower via the compile pass and re-enter; the common all-symbol
            // `let` skips this.
            if pattern_target {
                self.expr = crate::eval::macros::macroexpand_all(heap, expr, self.env)?;
                return Ok(Step::Continue);
            }
            heap.new_env(Some(self.env))
        };
        let marks = Marks::take(heap);
        let st = Box::new(BindState {
            binds: binds.iter().map(|&b| heap.root(b)).collect(),
            i: 0,
            scope_r: heap.root_env(scope),
            body_r: heap.root(body),
            lexical: !rec,
            captured: false,
        });
        self.binds(heap, st, marks)
    }

    /// Bind from `st.i` on, pushing at the first value that is not a leaf; then run the
    /// body.
    fn binds(
        &mut self,
        heap: &mut Heap,
        mut st: Box<BindState>,
        marks: Marks,
    ) -> Result<Step, LispError> {
        while st.i < st.binds.len() {
            let name = heap.read_root(st.binds[st.i]);
            if let Err(e) = as_symbol(heap, "binding name", name) {
                marks.restore(heap);
                return Err(e);
            }
            let rhs = heap.read_root(st.binds[st.i + 1]);
            st.open_frame_for_fn(heap, rhs);
            let scope = heap.read_root_env(st.scope_r);
            match leaf_value(heap, rhs, scope) {
                Some(v) => {
                    st.define(heap, v);
                    st.i += 2;
                }
                None => return self.push(heap, marks, KontKind::Bind(st), rhs, true, scope),
            }
        }
        let scope = heap.read_root_env(st.scope_r);
        let body = heap.read_root(st.body_r);
        marks.restore(heap);
        self.body(heap, body, scope)
    }

    /// Run a body held as a cons list: each non-last form for effect, the last as this
    /// frame's tail. Errors on an improper list.
    fn body(&mut self, heap: &mut Heap, body: Value, env: EnvId) -> Result<Step, LispError> {
        let mut cell = body;
        loop {
            match cell.unpack() {
                ValueRef::Nil => return Ok(Step::Value(Value::nil())),
                ValueRef::Pair(p) => {
                    let (form, next) = heap.pair(p);
                    if matches!(next.unpack(), ValueRef::Nil) {
                        self.expr = form;
                        self.env = env;
                        return Ok(Step::Continue);
                    }
                    if leaf_value(heap, form, env).is_some() {
                        cell = next;
                        continue;
                    }
                    let marks = Marks::take(heap);
                    let cursor = heap.root(cell);
                    let env_r = heap.root_env(env);
                    return self.push(
                        heap,
                        marks,
                        KontKind::Body { cursor, env_r },
                        form,
                        true,
                        env,
                    );
                }
                _ => return Err(LispError::type_err("improper body list")),
            }
        }
    }

    /// Run a closure body (non-empty): each non-last form for effect, the last as this
    /// frame's tail.
    fn forms(&mut self, heap: &mut Heap, forms: &[Value], env: EnvId) -> Result<Step, LispError> {
        let (&last, init) = forms.split_last().expect("a closure body runs non-empty");
        for (i, &form) in init.iter().enumerate() {
            if leaf_value(heap, form, env).is_some() {
                continue;
            }
            let marks = Marks::take(heap);
            let rest = forms[i + 1..].iter().map(|&f| heap.root(f)).collect();
            let env_r = heap.root_env(env);
            return self.push(
                heap,
                marks,
                KontKind::Forms { rest, env_r },
                form,
                true,
                env,
            );
        }
        self.lazy = 0;
        if let TailTag::Pending(r) = self.tail {
            self.tail = TailTag::Armed(r);
        }
        self.expr = last;
        self.env = env;
        Ok(Step::Continue)
    }

    /// A vector, set or map literal: each element evaluated in order.
    fn literal(
        &mut self,
        heap: &mut Heap,
        kind: LiteralKind,
        items: &[Value],
    ) -> Result<Step, LispError> {
        let marks = Marks::take(heap);
        let st = Box::new(LiteralState {
            kind,
            src: items.iter().map(|&it| heap.root(it)).collect(),
            out: SmallVec::with_capacity(items.len()),
            env_r: heap.root_env(self.env),
        });
        self.literal_next(heap, st, marks)
    }

    fn literal_next(
        &mut self,
        heap: &mut Heap,
        mut st: Box<LiteralState>,
        marks: Marks,
    ) -> Result<Step, LispError> {
        while st.out.len() < st.src.len() {
            let form = heap.read_root(st.src[st.out.len()]);
            let env = heap.read_root_env(st.env_r);
            match leaf_value(heap, form, env) {
                Some(v) => {
                    let r = heap.root(v);
                    st.out.push(r);
                }
                // Untagged, as the literal's old nested `eval` was.
                None => return self.push(heap, marks, KontKind::Literal(st), form, false, env),
            }
        }
        let out: Vec<Value> = st.out.iter().map(|&r| heap.read_root(r)).collect();
        marks.restore(heap);
        Ok(Step::Value(match st.kind {
            LiteralKind::Vector => heap.alloc_vector(out),
            LiteralKind::Set => heap.set_from_elems(out),
            LiteralKind::Map => {
                let pairs = out.chunks(2).map(|kv| (kv[0], kv[1])).collect();
                heap.map_from_pairs(pairs)
            }
        }))
    }

    /// Evaluate a call's arguments off the cons `spine`, then apply `callee`. The callee, the
    /// call form, the spine cursor, the environment and every argument so far are rooted for
    /// as long as an argument runs as a frame of its own.
    fn call(
        &mut self,
        heap: &mut Heap,
        callee: Value,
        call_form: Value,
        spine: Value,
        env: EnvId,
    ) -> Result<Step, LispError> {
        if matches!(spine.unpack(), ValueRef::Nil) {
            return self.apply(heap, callee, SmallVec::new(), call_form, env);
        }
        let marks = Marks::take(heap);
        let st = ArgsState {
            call_form_r: heap.root(call_form),
            callee_r: heap.root(callee),
            spine_r: heap.root(spine),
            env_r: heap.root_env(env),
            args: SmallVec::new(),
        };
        self.args_next(heap, st, marks)
    }

    fn args_next(
        &mut self,
        heap: &mut Heap,
        mut st: ArgsState,
        marks: Marks,
    ) -> Result<Step, LispError> {
        loop {
            let form = match heap.read_root(st.spine_r).unpack() {
                ValueRef::Nil => break,
                ValueRef::Pair(p) => heap.pair(p).0,
                // A dotted argument spine — `(f a . b)`. Tagged with the call form's own
                // position like every other call-shape error.
                _ => {
                    let call_form = heap.read_root(st.call_form_r);
                    marks.restore(heap);
                    return Err(LispError::type_err("improper argument list in call")
                        .with_hint(
                            "The argument list ends in a dotted tail. A call's arguments must be \
                             a proper list — to call with a list of arguments use `(apply f xs)`.",
                        )
                        .or_form_pos(heap, call_form));
                }
            };
            let env = heap.read_root_env(st.env_r);
            match leaf_value(heap, form, env) {
                Some(v) => {
                    let r = heap.root(v);
                    st.args.push(r);
                    let next = cdr(heap, heap.read_root(st.spine_r));
                    st.spine_r = heap.advance_root(st.spine_r, next);
                }
                None => {
                    let tag = if heap.form_pos_only(form).is_some() {
                        form
                    } else {
                        heap.read_root(st.call_form_r)
                    };
                    return self.push_tagging(
                        heap,
                        marks,
                        KontKind::Args(st),
                        form,
                        Some(tag),
                        env,
                    );
                }
            }
        }
        let argv: SmallVec<[Value; 8]> = st.args.iter().map(|&r| heap.read_root(r)).collect();
        let callee = heap.read_root(st.callee_r);
        let call_form = heap.read_root(st.call_form_r);
        let env = heap.read_root_env(st.env_r);
        marks.restore(heap);
        self.apply(heap, callee, argv, call_form, env)
    }

    /// Apply `callee` to the evaluated `argv`: a native answers, a closure's body becomes
    /// this frame's (its last form the tail).
    fn apply(
        &mut self,
        heap: &mut Heap,
        callee: Value,
        argv: SmallVec<[Value; 8]>,
        call_form: Value,
        env: EnvId,
    ) -> Result<Step, LispError> {
        let mut call_form = call_form;
        let mut env = env;
        // Inline `apply` unfolding: when the callee is the `apply` builtin, splice the
        // trailing sequence into `argv` and dispatch the real callee here — instead of going
        // through `call_native(apply)` → `apply_builtin` → `eval::apply` (which would add
        // ~4 Rust frames per chained `(apply f …)` and defeat TCO for `apply`-driven
        // recursion). Looped so nested `(apply apply (list f xs))` is also flattened. A
        // mis-arity (`(apply f)`) is left for `call_native` to flag with the canonical
        // "expected at least 2 arguments" message.
        let mut cur_callee = callee;
        let mut cur_argv = argv;
        // Re-entered when `apply` is unfolded or a thin-wrapper closure (`passthrough_arm`)
        // redirects to its inner call — both rewrite `cur_callee`/`cur_argv` and
        // `continue 'dispatch` rather than recursing.
        'dispatch: loop {
            while let ValueRef::Native(id) = cur_callee.unpack() {
                if !heap.native(id).is_apply || cur_argv.len() < 2 {
                    break;
                }
                let last = cur_argv.pop().expect("argv non-empty (checked above)");
                let mut real = cur_argv.remove(0);
                // A lazy seq-view as the spliced arg list realises first — `seq_items`
                // can't run its transducer. The realise re-enters `eval` (a GC safepoint
                // that relocates LOCAL handles), so root `real` and the remaining leading
                // args across it and re-read after — otherwise
                // `(apply <local-closure> … <seq-view>)` derefs a stale handle →
                // use-after-GC (ADR-114 re-read discipline; mirrors `realize_seqviews`).
                let last = if matches!(last.unpack(), ValueRef::SeqView(_)) {
                    heap.root_scope(|heap| -> LispResult {
                        let real_r = heap.root(real);
                        let arg_roots: Vec<_> = cur_argv.iter().map(|&v| heap.root(v)).collect();
                        let realized = crate::builtins::realize_seqview(heap, env, last)?;
                        real = heap.read_root(real_r);
                        for (slot, &r) in cur_argv.iter_mut().zip(arg_roots.iter()) {
                            *slot = heap.read_root(r);
                        }
                        Ok(realized)
                    })
                    .map_err(|e| e.or_form_pos(heap, call_form))?
                } else {
                    last
                };
                cur_argv.extend(
                    heap.seq_items(last).map_err(|e| {
                        crate::eval::named_by("apply", e).or_form_pos(heap, call_form)
                    })?,
                );
                cur_callee = real;
            }

            match cur_callee.unpack() {
                ValueRef::Native(id) => {
                    if compile::tier_ceiling() < compile::Tier::Bytecode
                        && let Some(native) = FrameNative::of(heap, id, cur_argv.len())
                    {
                        if let FrameNative::Receive = native {
                            if self.capture {
                                return self
                                    .receive(heap, id, cur_callee, &cur_argv, call_form, env);
                            }
                        } else if let Some(step) =
                            self.native_frame(heap, native, &cur_argv, call_form, env)
                        {
                            return step;
                        }
                    }
                    // A builtin that hands its call back (ADR-505) — a record's `num/add`
                    // under `+` — has it made here, in its place: a closure's body becomes
                    // this frame's, so a `receive` inside parks a capturing machine.
                    match call_native_redirectable(heap, id, &cur_argv, env) {
                        Err(error) if error.is_redirect() => {
                            let (callee, argv) = heap.take_redirect();
                            cur_callee = callee;
                            cur_argv = SmallVec::from_slice(&argv);
                            continue 'dispatch;
                        }
                        result => {
                            return result
                                .map(Step::Value)
                                .map_err(|e| e.or_form_pos(heap, call_form));
                        }
                    }
                }
                ValueRef::Fn(id) => {
                    // Thin-wrapper elision: redirect a pure pass-through arm (`(%add a b)`
                    // for `+`, etc.) straight to its inner call on the already-evaluated
                    // args, skipping the redundant frame + bind + body walk. Only redirect
                    // when it resolves to a function; anything else falls through to the
                    // normal path.
                    if let Some((head, map)) = passthrough_arm(heap, id, cur_argv.len()) {
                        let cl_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                        // Inner-head resolution. A bound, non-dynamic symbol is a plain
                        // lookup, which cannot collect. Anything else is a full symbol
                        // lookup — and that CAN collect: an unbound qualified head is a lazy
                        // module load (ADR-335), i.e. arbitrary evaluation. So the slow path
                        // roots the args across it (a `bytes` arg reached `seq` stale under
                        // `BROOD_TIER=0 nest test` before it did, deterministic with
                        // `BROOD_GC_STRESS=1`). The shared `passthrough_redirect_ok` then
                        // gates the redirect (callable inner only), counts the reduction, and
                        // honours the deadline.
                        let Value::Sym(head) = head else {
                            unreachable!("a passthrough head is a symbol (compute_passthrough)")
                        };
                        let fast = if heap.is_dynamic(head) {
                            None
                        } else {
                            heap.env_get(cl_env, head)
                        };
                        let capture = self.capture;
                        let inner = match fast {
                            Some(v) => v,
                            None => heap
                                .root_scope(|heap| {
                                    let arg_roots: SmallVec<[Root; 8]> =
                                        cur_argv.iter().map(|&v| heap.root(v)).collect();
                                    let form_r = heap.root(call_form);
                                    let env_r = heap.root_env(env);
                                    // Another process holds the inner head's module: no
                                    // redirect — the wrapper's own body runs, and its
                                    // call-head miss hands the wait to `%lazy-global`, where
                                    // it parks. Nil is not callable, so the redirect below
                                    // declines.
                                    let inner = match heap.env_get(cl_env, head) {
                                        None if capture => {
                                            match crate::eval::derive::parkable_miss(
                                                heap, cl_env, head, None,
                                            )? {
                                                crate::eval::derive::ParkableMiss::Bound(v) => v,
                                                crate::eval::derive::ParkableMiss::Elsewhere(_) => {
                                                    Value::nil()
                                                }
                                            }
                                        }
                                        _ => lookup(heap, cl_env, head)?,
                                    };
                                    for (slot, &r) in cur_argv.iter_mut().zip(arg_roots.iter()) {
                                        *slot = heap.read_root(r);
                                    }
                                    call_form = heap.read_root(form_r);
                                    env = heap.read_root_env(env_r);
                                    Ok(inner)
                                })
                                .map_err(|e: LispError| e.or_form_pos(heap, call_form))?,
                        };
                        // A wrapper forwarding to a contract shim of its own module forwards
                        // to the original instead (the boundary, ADR-383).
                        let inner = crate::builtins::contracts::boundary_redirect(heap, id, inner)
                            .unwrap_or(inner);
                        // A redirect back to the *same* closure is direct self-recursion
                        // (`(defn hog () (hog))`), not a thin wrapper — fall through to the
                        // normal call path (which re-enters the loop, whose reduction check
                        // can preempt) rather than spinning the redirect.
                        let self_cycle =
                            matches!(inner.unpack(), ValueRef::Fn(iid) if iid.0 == id.0);
                        if !self_cycle
                            && passthrough_redirect_ok(heap, inner)
                                .map_err(|e| e.or_form_pos(heap, call_form))?
                        {
                            let mut next_argv: SmallVec<[Value; 8]> =
                                SmallVec::with_capacity(map.len());
                            for &i in &map {
                                next_argv.push(cur_argv[i]);
                            }
                            cur_callee = inner;
                            cur_argv = next_argv;
                            continue 'dispatch;
                        }
                    }
                    // Tree-walker→VM router (see `tw_vm_route`): a VM-eligible callee runs
                    // at engine speed instead of being tree-walked because its CALLER
                    // happened to defer. Off below the bytecode tier.
                    if let Some(r) = tw_vm_route(heap, id, &cur_argv) {
                        return r
                            .map(Step::Value)
                            .map_err(|e| e.or_form_pos(heap, call_form));
                    }
                    return self.enter(heap, id, &cur_argv, Some(call_form));
                }
                // A keyword in head position is an accessor (ADR-165); the same helper
                // `apply` uses, so a direct `(:k m)` and a routed `(apply :k …)` can't
                // diverge.
                ValueRef::Keyword(_) => {
                    return apply_keyword(heap, cur_callee, &cur_argv)
                        .map(Step::Value)
                        .map_err(|e| e.or_form_pos(heap, call_form));
                }
                other => {
                    // A local head is named — only at a call with a source position, as the
                    // VM can (it keys the name by position), so the tiers agree on positionless
                    // code: the prelude's, or a form built at run time.
                    let mut err = not_a_function_error(heap, other);
                    if heap.form_pos(call_form).is_none() {
                        // library code calling a value the caller handed in
                        err = crate::eval::from_library_code(err);
                    } else if let Some(ValueRef::Sym(head)) = heap
                        .list_items(call_form)
                        .and_then(|items| items.first().map(|h| h.unpack()))
                        && heap.is_lexically_bound(env, head)
                    {
                        err = crate::eval::name_local_callee(heap, err, head);
                    }
                    return Err(err.or_form_pos(heap, call_form));
                }
            }
        }
    }
}

impl Machine<'_> {
    /// Enter closure `id`'s arm for `argv` as the running frame's body. `site` is the call
    /// form when this frame made the call. `None` is a native-boundary entry (`apply_closure`,
    /// a macro expander): the frame's trace entry starts as the callee's name with no call
    /// site, and an error leaving the body's tail takes the tail form's position
    /// ([`TailTag`]).
    fn enter(
        &mut self,
        heap: &mut Heap,
        id: ClosureId,
        argv: &[Value],
        site: Option<Value>,
    ) -> Result<Step, LispError> {
        // Read before binding: an `&optional` default may compact the RUNTIME region and
        // stale `id`.
        let (name, module) = {
            let cl = heap.closure(id);
            (cl.name, cl.module)
        };
        if site.is_none() {
            *self.entered = Some(TwFrame::boundary(name));
        }
        let bound = bind_params(heap, id, argv).map_err(|e| bind_failed(heap, e, name, site))?;
        if bound.defaults.is_empty() {
            return self.start_body(heap, name, module, bound.scope, &bound.body, site);
        }
        let marks = Marks::take(heap);
        let st = Box::new(EntryState {
            name,
            module,
            site: site.map(|f| heap.root(f)),
            scope_r: heap.root_env(bound.scope),
            defaults: bound
                .defaults
                .iter()
                .map(|&(n, d)| (n, heap.root(d)))
                .collect(),
            i: 0,
            rest: bound.rest,
            body: bound.body.iter().map(|&f| heap.root(f)).collect(),
        });
        self.defaults(heap, st, marks)
    }

    /// Evaluate the entry's `&optional` defaults from `st.i` on, each in the closure's scope,
    /// pushing at the first that is not a leaf; then start the body.
    fn defaults(
        &mut self,
        heap: &mut Heap,
        mut st: Box<EntryState>,
        marks: Marks,
    ) -> Result<Step, LispError> {
        while st.i < st.defaults.len() {
            let form = heap.read_root(st.defaults[st.i].1);
            let scope = heap.read_root_env(st.scope_r);
            match leaf_value(heap, form, scope) {
                Some(v) => st.define(heap, v),
                None => return self.push(heap, marks, KontKind::Defaults(st), form, true, scope),
            }
        }
        let scope = heap.read_root_env(st.scope_r);
        if let Some(rs) = st.rest {
            let empty = heap.list_from_slice(&[]);
            heap.env_define(scope, rs, empty);
        }
        let body: SmallVec<[Value; 4]> = st.body.iter().map(|&r| heap.read_root(r)).collect();
        let site = st.site.map(|r| heap.read_root(r));
        marks.restore(heap);
        self.start_body(heap, st.name, st.module, scope, &body, site)
    }

    /// The closure is bound: record the entry and run its body as this frame's.
    fn start_body(
        &mut self,
        heap: &mut Heap,
        name: Option<Symbol>,
        module: Option<Symbol>,
        scope: EnvId,
        body: &[Value],
        site: Option<Value>,
    ) -> Result<Step, LispError> {
        let Some(&last) = body.last() else {
            return Ok(Step::Value(Value::nil()));
        };
        let shim_file = contract_shim_file(heap, module, body);
        match site {
            Some(call_form) => record_tw_entry(heap, self.entered, name, shim_file, call_form),
            None => {
                if let Some(fr) = self.entered.as_mut() {
                    // A boundary entry has no call form: a shim keeps the call it had.
                    let call = fr.shim.take().and_then(|shim| shim.call);
                    fr.shim = shim_file.map(|file| Box::new(crate::eval::ShimEntry { file, call }));
                }
                self.tail = TailTag::Pending(heap.root(last));
            }
        }
        self.forms(heap, body, scope)
    }
}

impl Machine<'_> {
    /// Run `native`'s call as frames: push its continuation and enter the thunk across a
    /// native boundary, as the native's `apply_engine` did at tier 0. `None` when the thunk
    /// is not a closure (the native runs it). An error the native itself would raise before
    /// running the thunk — a bad `binding` — takes the call's position, as `call_native`'s
    /// did.
    fn native_frame(
        &mut self,
        heap: &mut Heap,
        native: FrameNative,
        argv: &[Value],
        call_form: Value,
        env: EnvId,
    ) -> Option<Result<Step, LispError>> {
        let (thunk, kind) = match native {
            FrameNative::Try { described } => {
                let ValueRef::Fn(thunk) = argv[0].unpack() else {
                    return None;
                };
                let marks = Marks::take(heap);
                let kind = KontKind::Try {
                    handler_r: heap.root(argv[1]),
                    env_r: heap.root_env(env),
                    described,
                };
                (thunk, (marks, kind))
            }
            FrameNative::Receive => return None,
            // The thunk runs on this machine, so a `receive` in it parks a capturing one
            // where, run nested under the builtin, it blocked the worker (ADR-494). The
            // window opens before the frame is saved, which needs its depth: a frame past
            // the limit is still pushed, and the depth error unwinding through it closes it.
            FrameNative::Interruptible => {
                let ValueRef::Fn(thunk) = argv[1].unpack() else {
                    return None;
                };
                let ceiling = match crate::builtins::interrupt_ceiling(heap, argv[0]) {
                    Ok(ceiling) => ceiling,
                    Err(e) => return Some(Err(e.or_form_pos(heap, call_form))),
                };
                let window = crate::process::open_interrupt_window(ceiling);
                if !self.save(
                    heap,
                    Marks::take(heap),
                    KontKind::Interruptible { window },
                    Some(call_form),
                ) {
                    return Some(Err(
                        tw_frame_depth_error(MAX_TW_FRAMES).or_form_pos(heap, call_form)
                    ));
                }
                return Some(self.enter(heap, thunk, &[], None));
            }
            FrameNative::Binding => {
                let ValueRef::Fn(thunk) = argv[2].unpack() else {
                    return None;
                };
                let dynamics = match crate::builtins::binding_enter(heap, argv[0], argv[1]) {
                    Ok(d) => d,
                    Err(e) => return Some(Err(e.or_form_pos(heap, call_form))),
                };
                (thunk, (Marks::take(heap), KontKind::Binding { dynamics }))
            }
        };
        let (marks, kind) = kind;
        if !self.save(heap, marks, kind, Some(call_form)) {
            return Some(Err(
                tw_frame_depth_error(MAX_TW_FRAMES).or_form_pos(heap, call_form)
            ));
        }
        Some(self.enter(heap, thunk, &[], None))
    }

    /// A capturing machine's `%receive` (or `proc/receive-lane`): the call is rooted across
    /// the native, so when the mailbox has nothing it matches (`Control::Suspend`) the machine
    /// parks, to redo the call on resume.
    fn receive(
        &mut self,
        heap: &mut Heap,
        id: NativeId,
        callee: Value,
        argv: &[Value],
        call_form: Value,
        env: EnvId,
    ) -> Result<Step, LispError> {
        let marks = Marks::take(heap);
        let redo = Redo::Apply {
            callee: heap.root(callee),
            argv: argv.iter().map(|&a| heap.root(a)).collect(),
            call_form: heap.root(call_form),
            env: heap.root_env(env),
        };
        match call_native(heap, id, argv, env) {
            Err(e) if e.is_suspend_signal() => {
                self.park = Some(Park {
                    suspend: true,
                    redo,
                    marks,
                });
                Err(e)
            }
            out => {
                let Redo::Apply { call_form, .. } = redo else {
                    unreachable!("built as Apply above")
                };
                let call_form = heap.read_root(call_form);
                marks.restore(heap);
                out.map(Step::Value)
                    .map_err(|e| e.or_form_pos(heap, call_form))
            }
        }
    }

    /// An error `e` unwound into a `Try` frame (whose registers are restored): run the handler
    /// on what `catch` binds, as a frame whose value is the call's.
    #[allow(clippy::too_many_arguments)]
    fn catch(
        &mut self,
        heap: &mut Heap,
        e: LispError,
        handler_r: Root,
        env_r: EnvRoot,
        described: bool,
        call_form_r: Root,
        marks: Marks,
    ) -> Result<Step, LispError> {
        // Allocation only, so `handler`, `env` and the arguments stay valid until the handler
        // is bound.
        let argv = crate::builtins::handler_args(e, heap, described);
        let handler = heap.read_root(handler_r);
        let call_form = heap.read_root(call_form_r);
        if let ValueRef::Fn(id) = handler.unpack() {
            // The handler's frame keeps the Try's marks: its slots go when the handler ends.
            if !self.save(heap, marks, KontKind::Callback, Some(call_form)) {
                return Err(tw_frame_depth_error(MAX_TW_FRAMES).or_form_pos(heap, call_form));
            }
            return self.enter(heap, id, &argv, None);
        }
        let env = heap.read_root_env(env_r);
        let out = apply(heap, handler, &argv, env).map(Step::Value);
        let call_form = heap.read_root(call_form_r);
        let out = out.map_err(|e| e.or_form_pos(heap, call_form));
        marks.restore(heap);
        out
    }
}

impl BindState {
    /// A `(fn …)` value sees its own name (the VM's `compile_let` matches this through
    /// `letrec_self`), because the closure captures the frame it is then bound into. So when
    /// [`define`](Self::define) would open a fresh frame for this binding, open it BEFORE the
    /// closure is built: opened after, the closure captured the parent and its own name was
    /// unbound — `(let (x (+ 1 2) f (fn (n) (f n))) …)` failed at tier 0 and ran on the VM.
    fn open_frame_for_fn(&mut self, heap: &mut Heap, rhs: Value) {
        if self.lexical && self.captured && crate::eval::compile::is_fn_form(heap, rhs) {
            let scope = heap.read_root_env(self.scope_r);
            let scope = heap.new_env(Some(scope));
            self.scope_r = heap.root_env(scope);
            self.captured = false;
        }
    }

    /// Bind the pending binding (`i`) to `val`. Plain `let`: once a value that could have
    /// built a closure has been bound, the next binding goes into a fresh CHILD frame instead
    /// of being appended to the frame that closure may have captured. Frame lookup is by name
    /// over the whole chain, so appending made a later binding visible to an earlier closure:
    /// `(let (x 1 f (fn () x) x 2) (f))` answered 2, where the compiled tiers (one slot per
    /// binding, resolved at compile time) answer 1. Only a list value form can build a
    /// closure, so the common atom-valued `let` still binds into one frame. `letrec` binds
    /// into its one frame, its names meant to be visible to every value.
    fn define(&mut self, heap: &mut Heap, val: Value) {
        let name = match heap.read_root(self.binds[self.i]).unpack() {
            ValueRef::Sym(s) => s,
            _ => unreachable!("binding names are checked before their value runs"),
        };
        let rhs_is_list = matches!(
            heap.read_root(self.binds[self.i + 1]).unpack(),
            ValueRef::Pair(_)
        );
        let mut scope = heap.read_root_env(self.scope_r);
        if self.lexical {
            if self.captured {
                // `val` stays live across `new_env`: an allocation, never a collection.
                scope = heap.new_env(Some(scope));
                self.scope_r = heap.root_env(scope);
            }
            self.captured = rhs_is_list;
        }
        heap.env_define(scope, name, val);
    }
}

/// Bind `name` to `val` as a global (the rest of `def`, once its value is known).
fn def_bind(heap: &mut Heap, name: Symbol, val: Value, env: EnvId) -> LispResult {
    let val = name_value(heap, val, name);
    let root = heap.env_root(env);
    // Arity-change diagnostic: if `def` is *rebinding* a callable to one of a different
    // arity, callers expecting the old shape will hit a runtime arity error on the next
    // call. Surface it at reload time so the mismatch isn't a silent surprise (the
    // hot-reload + late-binding contract — docs/shared-code.md). Fires only when an old
    // binding exists, so the prelude/std first-time build is silent.
    if let Some(old) = heap.env_get(root, name) {
        if let (Some(old_a), Some(new_a)) = (value_arity(heap, old), value_arity(heap, val))
            && (old_a.min != new_a.min || old_a.max != new_a.max)
            && reload_diagnostics_enabled(heap, root)
        {
            eprintln!(
                "[reload] arity changed for {}: {} -> {}",
                value::symbol_name(name),
                arity_to_string(old_a),
                arity_to_string(new_a),
            );
        }
        // Macro-redefinition diagnostic (hot reload): redefining a macro does *not*
        // re-expand callers already compiled with the old expansion — they keep the old
        // code until re-eval'd. `defmacro` is a macro lowering to `(def name (%make-macro
        // …))`, so the diagnostic lives on the `def` side.
        if matches!(old.unpack(), ValueRef::Macro(_))
            && matches!(val.unpack(), ValueRef::Macro(_))
            && reload_diagnostics_enabled(heap, root)
        {
            eprintln!(
                "[reload] macro {} redefined; callers expanded before now keep the old expansion — re-eval them",
                value::symbol_name(name)
            );
        }
    }
    heap.env_define(root, name, val);
    // While an embedded std module is loading, its own definitions become reserved as they
    // appear — that is how `set/union` and friends join the reserved set without a static
    // name list. Same rule as the seed filter: an embedded module's FUNCTIONS become
    // reserved, its data globals (its own registries, e.g. `protocol`'s
    // `*protocols*`/`*impls*`) stay rebindable — they are rebound by the module's own
    // functions at runtime.
    if heap.in_module_load()
        && matches!(
            val.unpack(),
            ValueRef::Fn(_) | ValueRef::Macro(_) | ValueRef::Native(_)
        )
    {
        heap.reserve_global(name);
    }
    // The name's uncontracted alias, if one is bound, follows every rebinding (ADR-383) —
    // before the hook below re-wraps a closure, and for the rebindings the hook does not
    // handle.
    crate::builtins::contracts::mirror_alias(heap, root, name, val);
    // A closure whose name carries a declared signature is offered to the contract policy
    // (ADR-381): armed, the binding becomes a checking shim; unarmed, this is one store
    // lookup. Order-free — a `sig` above its `defn` is enforced here, one below it at
    // `%register-sig` — and a hot reload re-wraps, since it is a `def`. Whole-module batches
    // (an embedded std module, the prelude at boot) are swept from Brood instead, so the
    // stdlib image never carries a shim.
    if matches!(val.unpack(), ValueRef::Fn(_))
        && !heap.in_module_load()
        && heap.declared_sig_value(name).is_some()
    {
        crate::builtins::contracts::contract_apply(
            heap,
            name,
            crate::builtins::contracts::OnShim::Rewrap,
        )?;
    }
    Ok(Value::symbol(name))
}

#[cfg(test)]
mod kont_stack_tests {
    use super::*;

    fn kont(i: u32) -> Kont {
        Kont {
            entered: None,
            lazy: i,
            tail: TailTag::None,
            form: None,
            marks: Marks { roots: 0, envs: 0 },
            kind: KontKind::Callback,
        }
    }

    /// The frame stack is last-in first-out across segment boundaries, and a deep stack holds
    /// no growth slack: every set-aside segment is exactly [`KONT_SEGMENT`] frames, where a
    /// doubling `Vec` held up to twice its frames (and both buffers while it grew).
    #[test]
    fn the_frame_stack_is_lifo_across_segments_and_holds_no_slack() {
        let mut stack = KontStack::default();
        let n = 3 * KONT_SEGMENT + 5;
        for i in 0..n {
            stack.push(kont(i as u32));
        }
        assert_eq!(stack.len(), n);
        assert_eq!(
            stack.full.len(),
            n / KONT_SEGMENT,
            "the stack did not set segments aside"
        );
        assert!(
            stack.top.capacity() <= KONT_SEGMENT,
            "the top segment grew past a segment"
        );
        assert!(
            stack
                .full
                .iter()
                .all(|s| s.capacity() == KONT_SEGMENT && s.len() == KONT_SEGMENT),
            "a full segment holds more than its frames"
        );
        // Down across a boundary and back up again reuses the spare segment.
        for i in (2 * KONT_SEGMENT..n).rev() {
            assert_eq!(stack.pop().map(|k| k.lazy), Some(i as u32));
        }
        for i in 2 * KONT_SEGMENT..n {
            stack.push(kont(i as u32));
        }
        assert!(stack.full.iter().all(|s| s.capacity() == KONT_SEGMENT));
        for i in (0..n).rev() {
            assert_eq!(
                stack.pop().map(|k| k.lazy),
                Some(i as u32),
                "frame {i} out of order"
            );
        }
        assert!(stack.pop().is_none());
        assert_eq!(stack.len(), 0);
    }
}
