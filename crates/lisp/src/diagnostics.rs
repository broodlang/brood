//! **Diagnostics**: the observability instruments — counters, a sampling profiler, line
//! coverage, and the catalogue of `BROOD_*` flags that arm them. Nothing here changes
//! what a program computes; each module is off (and compiles to nothing, or to one cached
//! `var_os`) unless a feature or a flag switches it on.

pub mod coverage; // line-coverage recording, off unless BROOD_COVERAGE is set (ADR-148)
pub mod debug_flags; // the BROOD_* diagnostic-flag catalogue (`brood --debug-flags`)
pub mod perf; // VM work-attribution counters (feature "perf-stats") — docs/benchmarking.md
pub mod profile; // sampling CPU profiler over the VM's reified frames (observability timing tier)
