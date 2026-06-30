#![no_main]
//! The reader must never panic/abort on ANY input — only return Ok or a clean Err.
use libfuzzer_sys::fuzz_target;
use brood::core::heap::Heap;
use brood::syntax::reader;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut heap = Heap::new();
        let _ = reader::read_all(&mut heap, s); // Err is fine; a panic/abort is a bug
    }
});
