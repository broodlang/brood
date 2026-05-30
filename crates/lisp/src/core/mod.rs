//! Substrate: the value model, the per-process heap, and the byte-counting
//! allocator. The foundation every other layer is addressed through — almost
//! every component threads a `&mut Heap` and speaks in `value::Value` handles.

pub mod alloc;
pub mod blob;
pub mod heap;
pub mod keywords;
pub mod map_champ;
pub mod sync;
pub mod value;
