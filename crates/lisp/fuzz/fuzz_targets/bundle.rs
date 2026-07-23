#![no_main]
//! The bundle footer/archive parser must never panic/abort/over-allocate on
//! ANY file bytes — `brood` inspects its own binary tail on every start, and
//! a corrupt/hostile bundle must degrade to "not a bundle", not a crash.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    brood::bundle::fuzz_parse(data);
});
