//! The compiling execution engine — ADR-076, [`docs/bytecode-vm.md`].
//!
//! The plan is to replace the tree-walker (`eval::eval`) with a
//! **closure-compiling VM over a lexically-addressed IR**: each form compiles
//! once into a [`Node`] tree run by a trampoline, with frame slots living as
//! regions of the existing `Heap::roots` operand stack so the moving collector
//! relocates them with no new root set (the crux — see the doc).
//!
//! **This is Stage 0: scaffolding only.** The pipeline (compile a form → a
//! [`Node`] → [`exec`]) exists and is reachable behind the `BROOD_VM` env flag,
//! but every form currently compiles to [`Node::Defer`], which hands the original
//! form straight back to the tree-walker. So `BROOD_VM=1` is at exact parity with
//! the default — the point is to land the module, the flag, and the entry seam
//! green before Stage 1 introduces real lexical addressing and frame slots.
//!
//! Naming note: this module's [`compile_form`] is the *IR compiler*; it runs
//! **after** `eval::macros::compile` (the macroexpand-all + namespace-resolve
//! pass), on the already-expanded, already-resolved form.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::LispResult;

/// Is the compiling VM enabled? `BROOD_VM` set in the environment turns it on.
/// **Off by default** — the tree-walker is the engine until the Stage 3 cutover
/// (ADR-076). Read once and cached; the flag can't change mid-run.
pub fn vm_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_VM").is_some())
}

/// A compiled IR node (ADR-076). Stage 0 only has [`Node::Defer`]; later stages
/// add `Const` / `Local{depth,index}` / `Global(Symbol)` / `SymbolRef(Symbol)` /
/// `If` / `Do` / `Call` / `TailCall` / `MakeClosure` / `LetBind`, at which point
/// `exec` runs them directly instead of deferring.
pub enum Node {
    /// Not yet compiled to IR — execute by handing the original (already
    /// macroexpanded + resolved) form to the tree-walker. The Stage-0 catch-all
    /// and the permanent correctness fallback for forms a later stage declines to
    /// compile.
    Defer(Value),
}

/// Compile an already-macroexpanded, already-namespace-resolved `form` into a
/// [`Node`]. **Stage 0:** everything defers to the tree-walker.
pub fn compile_form(_heap: &mut Heap, form: Value) -> Node {
    Node::Defer(form)
}

/// Execute a compiled [`Node`] in `env`. **Stage 0:** [`Node::Defer`] runs the
/// tree-walker, so this is behaviourally identical to calling `eval::eval`
/// directly — which is exactly the parity property Stage 0 must hold.
pub fn exec(heap: &mut Heap, node: Node, env: EnvId) -> LispResult {
    match node {
        Node::Defer(form) => crate::eval::eval(heap, form, env),
    }
}

/// Compile-then-execute a resolved `form` — the VM entry the top-level form loop
/// uses when `vm_enabled()`. Kept as one call so the seam in `eval_source` is a
/// single branch and Stage 1 has one place to grow.
pub fn run(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    let node = compile_form(heap, form);
    exec(heap, node, env)
}
