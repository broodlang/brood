//! A function a module defines records that module as its author (`Closure::module`,
//! ADR-383) on every engine and through every way the module arrives (KI-399).
//!
//! The VM's top-level entries (`compile::run` and the file runner's program thunk) once left
//! the field unset, while the tree-walker set it. Every reader that falls back to the
//! function's qualified name (`reflect/fn-module`, the compiled arm's boundary) hid it; the
//! tree-walker's contract-shim test reads the field directly, and so did the startup image,
//! which is written from the VM and carried no module for any std function. These read the
//! field itself, never a fallback.

use brood::Interp;
use brood::core::value::{self, Value};

fn module_of(interp: &Interp, name: &str) -> Option<String> {
    let sym = value::intern(name);
    let Some(Value::Fn(id)) = interp.heap.env_get(interp.heap.global(), sym) else {
        panic!("{name} is not a closure");
    };
    interp
        .heap
        .closure(id)
        .module
        .map(|module| value::symbol_name(module).to_string())
}

#[test]
fn a_module_function_evaluated_form_by_form_records_its_module() {
    let mut interp = Interp::new();
    interp
        .eval_str("(defmodule closure-module-a)\n(defn helper () 1)\n")
        .expect("module evaluates");
    assert_eq!(
        module_of(&interp, "closure-module-a/helper").as_deref(),
        Some("closure-module-a")
    );
}

#[test]
fn a_module_function_the_program_driver_defines_records_its_module() {
    let mut interp = Interp::new();
    interp
        .run_program("(defmodule closure-module-b)\n(defn helper () 1)\n", None)
        .expect("program runs");
    assert_eq!(
        module_of(&interp, "closure-module-b/helper").as_deref(),
        Some("closure-module-b")
    );
}

#[test]
fn a_std_module_function_records_its_module_however_it_was_loaded() {
    // From the startup image when this binary has one, from source otherwise: both must
    // say `json`.
    let mut interp = Interp::new();
    interp.eval_str("(require-one 'json)").expect("json loads");
    assert_eq!(module_of(&interp, "json/encode").as_deref(), Some("json"));
}
