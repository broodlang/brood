//! Runs the mylisp-level test suite (`tests/suite.lisp`) through the
//! interpreter. The suite signals failure by raising an error, so an `Ok`
//! result means every in-language assertion passed.

use mylisp::Interp;

#[test]
fn mylisp_suite_passes() {
    let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/suite.lisp"));
    let mut interp = Interp::new();
    if let Err(e) = interp.eval_str(src) {
        panic!("mylisp test suite failed: {}", e);
    }
}
