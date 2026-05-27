//! Runs the mylisp-level test suite (`tests/suite.lisp`) through the
//! interpreter. The suite signals failure by raising an error, so an `Ok`
//! result means every in-language assertion passed.

use mylisp::Interp;

#[test]
fn mylisp_suite_passes() {
    let test_lib = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../std/test.lisp"));
    let suite = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/suite.lisp"));
    let mut interp = Interp::new();
    interp.eval_str(test_lib).expect("loading std/test.lisp failed");
    if let Err(e) = interp.eval_str(suite) {
        panic!("mylisp test suite failed: {}", e);
    }
}
