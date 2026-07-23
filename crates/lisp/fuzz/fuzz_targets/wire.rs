#![no_main]
//! The dist wire decoder must never panic/abort/over-allocate on ANY inbound
//! bytes — an unauthenticated peer controls this surface. Only a clean
//! Ok/Err is acceptable.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    brood::dist::fuzz_decode_frame(data);
});
