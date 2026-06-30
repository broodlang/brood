#![no_main]
//! Eval must never PANIC/abort/corrupt memory on any parseable input — only return
//! Ok or a clean Err. Run under ASAN so a heap bug (use-after-free, OOB) is caught.
use libfuzzer_sys::fuzz_target;
use brood::Interp;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 { return; }      // bound input size
        let mut interp = Interp::new();
        let _ = interp.eval_str(s);        // Err is fine; a panic/abort/ASAN error is a bug
    }
});
