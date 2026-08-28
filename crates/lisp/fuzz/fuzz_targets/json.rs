#![no_main]
//! `json/decode` (pure Brood, std/json.blsp) must never panic/abort the HOST
//! on any input string — only a value or a clean in-language error. The
//! interp is built once per fuzz process (the boot is ~ms); each input rides
//! the printer's escape-correct string literal (print->read is a checked
//! fixpoint), wrapped in try/catch so in-language errors don't stop the run.
use std::cell::RefCell;

use brood::Interp;
use libfuzzer_sys::fuzz_target;

thread_local! {
    static INTERP: RefCell<Interp> = RefCell::new({
        let mut i = Interp::new();
        i.eval_str("(require-one 'json)").expect("json loads");
        i
    });
}

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    if s.len() > 8192 {
        return; // bound per-input work
    }
    INTERP.with(|cell| {
        let interp = &mut *cell.borrow_mut();
        let lit = {
            let v = interp.heap.alloc_string(s);
            brood::syntax::printer::print(&interp.heap, v)
        };
        let _ = interp.eval_str(&format!("(try (json/decode {lit}) (catch e nil))"));
    });
});
