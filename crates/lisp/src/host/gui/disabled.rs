//! The stub the GUI surface resolves to when the `gui` feature is off: every entry point
//! keeps its real signature and reports that no backend was compiled in.

use super::Op;
use super::NOT_COMPILED;

/// No GUI is compiled in, so nothing will ever want the process main thread: this is
/// the plain `join` it stands in for. The signature matches the real backend's so
/// `cli_support::run_on_main_stack` needs no `cfg` of its own.
pub fn host_main_thread<T: Send + 'static>(handle: std::thread::JoinHandle<T>, name: &str) -> T {
    handle
        .join()
        .unwrap_or_else(|_| panic!("{name} thread panicked"))
}

pub fn open(_subscriber: u64, _spec: super::WindowSpec) -> Result<u64, String> {
    Err(NOT_COMPILED.into())
}
pub fn close(_id: u64) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn title(_id: u64, _title: String) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn icon(_id: u64, _rgba: Vec<u8>, _w: u32, _h: u32) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn focus(_id: u64) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn grab(_id: u64, _on: bool) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn maximize(_id: u64, _on: bool) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn minimize(_id: u64) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn drag_move(_id: u64) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn drag_resize(_id: u64, _dir: &str) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn fullscreen(_id: u64, _on: bool) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn size(_id: u64) -> Result<(u16, u16), String> {
    Err(NOT_COMPILED.into())
}
pub fn held_key(_id: u64) -> Result<Option<super::Key>, String> {
    Err(NOT_COMPILED.into())
}
pub fn draw(_id: u64, _ops: Vec<Op>) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn font(_id: Option<u64>, _family: Option<u32>, _px: Option<f32>) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn inset(_px: f32) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn bg(_rgb: Option<[u8; 3]>) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn line_height(_mult: f32) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
pub fn text_aa(_mode: TextAa) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
/// The text anti-aliasing modes `gui-text-aa!` names, so the builtin parses its
/// argument identically with or without the GUI compiled in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAa {
    Auto,
    Gray,
    Subpixel,
    Bgr,
}
pub fn register_family(
    _name: u32,
    _regular: Vec<u8>,
    _bold: Vec<u8>,
    _italic: Vec<u8>,
    _bold_italic: Vec<u8>,
) -> Result<(), String> {
    Err(NOT_COMPILED.into())
}
