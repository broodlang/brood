//! Surface syntax: the reader (text → `Value`) and the printer (`Value` → text).
//! The two round-trip with each other — a change to one usually means a matching
//! change to the other.
//!
//! [`cst`] is a third surface: a lossless, span-carrying tree for tooling (the
//! language server), built from the same lexical rules ([`atom`]) as the reader
//! but error-tolerant where the reader rejects. See `docs/lsp.md` / ADR-025.

pub mod atom;
pub mod cst;
pub mod printer;
pub mod reader;
pub mod scanner;
pub mod scope;
