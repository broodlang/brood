//! Surface syntax: the reader (text → `Value`) and the printer (`Value` → text).
//! The two round-trip with each other — a change to one usually means a matching
//! change to the other.

pub mod printer;
pub mod reader;
