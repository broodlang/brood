//! The **host** layer: the optional, feature-gated mechanisms that bind the runtime to
//! the machine it runs on — a window, a sound device, a socket, a child OS process, a
//! WASM guest, a tree-sitter parser. Each is a thin Rust *mechanism* whose *policy* is
//! Brood (`std/gui.blsp`, `std/net/*`, `std/wasm.blsp`, …); none of them is part of the
//! language, and a build can leave most of them out.

pub mod audio;
pub mod clipboard; // OS clipboard via `arboard` (feature "clipboard", pulled in by "gui"); no-ops without it // optional audio output backend (feature "audio", pulled in by "gui")
pub mod gui; // optional windowed display backend (feature "gui") — ADR-046 frontend #2
#[cfg(not(target_arch = "wasm32"))]
pub mod net; // thin non-blocking TCP socket mechanism (ADR-062); policy lives in bundled std/net/* (ADR-097)
#[cfg(target_arch = "wasm32")]
#[path = "host/net_wasm.rs"]
pub mod net; // wasm has no sockets — stub with the same API (fails at runtime)
pub mod subprocess; // persistent child-process mechanism: spawn + stdio pipes over the mailbox seam (ADR-104)
pub mod text_width; // grapheme-cluster display-cell width (the `string/display-width` builtin + the GUI grid)
pub mod treesit; // optional tree-sitter parsing for foreign languages (feature "treesit") — ROADMAP §C
#[cfg(feature = "wasm")]
pub mod wasm; // WASM component interop host (ADR-071/145); policy lives in std/wasm.blsp
