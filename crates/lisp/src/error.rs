//! Error type used throughout the interpreter.
//!
//! Every fallible operation returns [`LispResult`]. Errors carry a coarse
//! [`ErrorKind`] (useful later for `try`/`catch` and for tooling) plus a
//! human-readable message.

use std::fmt;
use std::path::Path;

use crate::core::value::Value;

/// Return the shortest useful path for display: relative when the file lives
/// under the cwd, absolute otherwise. Best-effort — falls back to `path` as-is
/// if the cwd or canonicalization is unavailable.
fn display_path(path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(rel) = p.strip_prefix(&cwd) {
                return rel.to_string_lossy().into_owned();
            }
        }
    }
    path.to_string()
}

/// A 1-based source position (line and column), used for editor-parseable
/// error reporting (see `docs/tooling.md`). Columns count characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// A half-open byte range into the source text, `start..end`. Used by the
/// tooling CST (`syntax::cst`) to record where every node was read. Byte
/// offsets index `&str` directly; a `LineIndex` (in the LSP layer) projects them
/// to editor positions. `Pos` is the line/col projection used for diagnostics;
/// `Span` is the raw range. See `docs/lsp.md` / ADR-025.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span {
            start: start as u32,
            end: end as u32,
        }
    }
    /// Does this span contain byte offset `at`? Half-open: `start <= at < end`.
    pub fn contains(&self, at: u32) -> bool {
        self.start <= at && at < self.end
    }
    /// Slice the source this span was taken from.
    pub fn slice<'s>(&self, src: &'s str) -> &'s str {
        &src[self.start as usize..self.end as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The reader could not parse the source text.
    Parse,
    /// A symbol was referenced that has no binding.
    Unbound,
    /// A function or special form was called with the wrong number of args.
    Arity,
    /// A value had the wrong type for the operation.
    Type,
    /// A catch-all for runtime failures (overflow, division by zero, ...).
    Runtime,
    /// Raised by `(throw v)` from user code.
    User,
}

impl ErrorKind {
    /// The stable lowercase tag name — the keyword that appears as `:kind` in a
    /// caught built-in error map (e.g. `:unbound`, `:type`). Stable across
    /// versions so agents and Brood code can branch on it (ADR-036,
    /// `docs/llm-native.md` §4).
    pub fn tag_name(self) -> &'static str {
        match self {
            ErrorKind::Parse => "parse",
            ErrorKind::Unbound => "unbound",
            ErrorKind::Arity => "arity",
            ErrorKind::Type => "type",
            ErrorKind::Runtime => "runtime",
            ErrorKind::User => "user",
        }
    }

    /// The label that opens a diagnostic line — `"error:"` for a user `throw`,
    /// `"<kind> error:"` otherwise. The single source of truth for both the
    /// [`fmt::Display`] impl and the CLI's colored `report_error`, so the two
    /// can't drift.
    pub fn label(self) -> &'static str {
        match self {
            ErrorKind::User => "error:",
            ErrorKind::Parse => "parse error:",
            ErrorKind::Unbound => "unbound error:",
            ErrorKind::Arity => "arity error:",
            ErrorKind::Type => "type error:",
            ErrorKind::Runtime => "runtime error:",
        }
    }
}

/// A non-error **control signal** (ADR-100 §7 migration). It travels on the error
/// channel — a [`LispError`] whose `control` is `Some` — so it propagates through the
/// existing `?`/`Err` plumbing *without* re-typing every signature, but it is **not a
/// failure**: error-handling natives (`%try`, and the cleanup natives `binding`/
/// `%isolate`) must **re-raise** it untouched, and the bytecode driver (`vm_run_bc`)
/// **intercepts** it rather than unwinding. It is never a user-visible error and
/// carries no message/kind semantics — `kind`/`message` on a control `LispError` are
/// inert placeholders.
///
/// Distinct from `throw`/`raise`: those *unwind* the stack to a handler (abandon the
/// continuation); `Suspend` *captures* the continuation to resume at the same point.
#[derive(Debug, Clone)]
pub enum Control {
    /// A green process hit `receive` on an empty mailbox: capture its VM
    /// continuation to the scheduler and park it. `deadline` is the absolute wake
    /// time for a `(receive … (after ms …))`, so the scheduler arms a timer; `None`
    /// waits indefinitely.
    Suspend {
        deadline: Option<std::time::Instant>,
    },
}

#[derive(Debug, Clone)]
pub struct LispErrorData {
    pub kind: ErrorKind,
    pub message: String,
    /// `Some` iff this is a [`Control`] signal riding the error channel (a suspend),
    /// not a real error. `None` for every actual error. See [`Control`].
    pub control: Option<Control>,
    /// The value carried by `(throw v)`, so `catch` can rebind it. Built-in
    /// errors leave this `None`; `try_catch` then projects the structured
    /// fields (kind, code, message, location, hint) into a Brood map.
    pub payload: Option<Value>,
    /// Source position, when known. Set by the reader (precise, for parse
    /// errors) or filled in by the file runner with the enclosing top-level
    /// form's start (for runtime errors). Drives `FILE:LINE:COL:` output.
    pub pos: Option<Pos>,
    /// The file the error occurred in, when known (set by `load` / the file
    /// runner). Combined with `pos` for `FILE:LINE:COL:` diagnostics.
    pub file: Option<String>,
    /// Stable error code (`"E0010"`, `"E0030"`, …) — see `error_codes` below
    /// and `docs/error-codes.md`. `None` for errors that haven't been tagged
    /// yet (callers fall back to branching on [`ErrorKind`]). Static `&str`
    /// so the registry is a plain table.
    pub code: Option<&'static str>,
    /// Optional human-readable hint pointing at a likely fix, e.g.
    /// `"scheduler race under -j 0 — try -j 1"`. Set by raise sites that
    /// know the common gotcha; omitted otherwise.
    pub hint: Option<String>,
}

/// `LispError` is a thin newtype over a **boxed** [`LispErrorData`]. It's returned
/// in every [`LispResult`], so keeping it one pointer wide (8 bytes) keeps the
/// happy-path `Result<Value, LispError>` small (~24 B vs the ~144 B it would be
/// inline) — the error payload heap-allocates only on the cold error path.
/// `Deref`/`DerefMut` expose the fields transparently, so every `e.kind` /
/// `e.message` / … access and the `with_*` / `or_*` builders read exactly as if the
/// fields were inline; constructors are the only sites that box.
#[derive(Debug, Clone)]
pub struct LispError(Box<LispErrorData>);

impl std::ops::Deref for LispError {
    type Target = LispErrorData;
    #[inline]
    fn deref(&self) -> &LispErrorData {
        &self.0
    }
}

impl std::ops::DerefMut for LispError {
    #[inline]
    fn deref_mut(&mut self) -> &mut LispErrorData {
        &mut self.0
    }
}

// ---------- error codes (see `docs/error-codes.md`) ---------------------------
//
// **Stable** identifiers attached to built-in errors at construction time. The
// numbering scheme groups by [`ErrorKind`]:
//   E00xx — Parse / reader
//   E01xx — Unbound / scope
//   E02xx — Arity
//   E03xx — Type
//   E04xx — Runtime (division, overflow, IO, …)
//
// Codes never get repurposed — once shipped they're permanent. New errors get
// the next free slot in their range.
pub mod error_codes {
    pub const PARSE_GENERIC: &str = "E0001";
    /// Input ended in the middle of a form or string (EOF mid-parse) — distinct
    /// from a genuine syntax error, so a REPL / editor can tell "needs more
    /// input" (read another line) from "this is wrong" without re-scanning text.
    pub const INCOMPLETE_INPUT: &str = "E0002";
    pub const UNBOUND_SYMBOL: &str = "E0010";
    pub const ARITY_MISMATCH: &str = "E0020";
    pub const TYPE_MISMATCH: &str = "E0030";
    /// `(/ x 0)` or `(rem x 0)` — guard with `(when (not= y 0) …)`.
    pub const DIV_BY_ZERO: &str = "E0040";
    /// Integer overflow on the checked numeric ops (`%add`/`%sub`/`%mul`/
    /// `rem`).
    pub const INT_OVERFLOW: &str = "E0041";
    /// `vector-ref` / `substring` / similar with an out-of-range index.
    pub const INDEX_OUT_OF_RANGE: &str = "E0042";
    /// Evaluation used more stack than the [`stack_budget`] guard allows
    /// (runaway *non-tail* recursion). Raised at the top of `eval` — which
    /// measures bytes of stack used since the outermost frame on this
    /// coroutine/thread — *before* the coroutine stack overflows, so a
    /// `(defn boom (n) (+ 1 (boom (+ n 1))))` becomes a clean, catchable error
    /// instead of a SIGSEGV that aborts the whole host/REPL/MCP process. The fix
    /// is to rewrite as a tail-recursive loop (proper tail calls are O(1) stack).
    /// Tune via `BROOD_STACK_BUDGET`.
    ///
    /// [`stack_budget`]: crate::process::stack_budget
    pub const STACK_DEPTH_EXCEEDED: &str = "E0044";
    /// Allocation crossed the configured *soft* memory limit (ADR-043). Raised
    /// at the eval safepoint so a runaway/hostile program fails cleanly instead
    /// of exhausting host RAM. Catchable; tune via `BROOD_MEM_LIMIT`.
    pub const MEMORY_LIMIT: &str = "E0043";
    /// File IO failed: `load` / `slurp` / `spit` / `make-dir` / `list-dir` /
    /// `cwd` / `check-file` couldn't read or write a path.
    pub const FILE_IO: &str = "E0050";
    /// `run-process` could not start the requested program (typically: not
    /// on PATH).
    pub const SUBPROCESS_FAILED: &str = "E0051";
    /// `node-start` / `connect` / other distribution-layer failure.
    pub const DISTRIBUTION: &str = "E0060";
    /// `send` saw a message value nested past `MAX_MESSAGE_DEPTH` — the
    /// deep-copy stack would have overflowed.
    pub const MESSAGE_TOO_DEEP: &str = "E0070";
    pub const RUNTIME_GENERIC: &str = "E0099";
}

impl LispError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        LispError(Box::new(LispErrorData {
            kind,
            message: message.into(),
            control: None,
            payload: None,
            pos: None,
            file: None,
            code: None,
            hint: None,
        }))
    }

    /// Build a [`Control`] signal (rides the error channel; not a real error).
    /// `kind`/`message` are inert placeholders so the struct stays well-formed.
    pub fn control(c: Control) -> Self {
        LispError(Box::new(LispErrorData {
            kind: ErrorKind::Runtime,
            message: String::new(),
            control: Some(c),
            payload: None,
            pos: None,
            file: None,
            code: None,
            hint: None,
        }))
    }

    /// The `receive`-on-empty suspend control signal (ADR-100 §7). See [`Control`].
    pub fn suspend(deadline: Option<std::time::Instant>) -> Self {
        LispError::control(Control::Suspend { deadline })
    }

    /// Is this a [`Control`] signal (a suspend) rather than a real error? Error-handling
    /// natives test this to **re-raise** it instead of catching/cleaning up.
    pub fn is_control(&self) -> bool {
        self.control.is_some()
    }

    /// Attach a stable error code (builder).
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    /// Attach a human-readable hint (builder).
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Attach a source position (builder style).
    pub fn with_pos(mut self, pos: Pos) -> Self {
        self.pos = Some(pos);
        self
    }

    /// Attach `pos` only if none is set yet — so a precise inner position
    /// (e.g. a parse error) is never overwritten by a coarser fallback.
    pub fn or_pos(mut self, pos: Pos) -> Self {
        if self.pos.is_none() {
            self.pos = Some(pos);
        }
        self
    }

    /// Attach the recorded source position (and originating file, if known) of
    /// `form` only when none is set yet — the [`or_pos`](Self::or_pos) shape,
    /// but driven by [`Heap::form_pos`](crate::core::heap::Heap::form_pos). The
    /// eval loop uses this on every error-propagation path, so an error bubbles
    /// up tagged with the *innermost* form whose position was recorded by the
    /// reader. The lookup happens only on the error path, so the hot path pays
    /// nothing.
    pub fn or_form_pos(self, heap: &crate::core::heap::Heap, form: Value) -> Self {
        if self.pos.is_some() {
            return self;
        }
        match heap.form_pos(form) {
            Some((p, file)) => {
                let e = self.with_pos(p);
                match file {
                    Some(f) => e.or_file(f.as_ref()),
                    None => e,
                }
            }
            None => self,
        }
    }

    /// Attach a file only if none is set yet (the innermost `load` wins).
    pub fn or_file(mut self, file: impl Into<String>) -> Self {
        if self.file.is_none() {
            self.file = Some(file.into());
        }
        self
    }

    /// A one-line GNU diagnostic: `[FILE:][LINE:COL: ]kind error: message`, the
    /// form editors parse (see `docs/tooling.md`). Falls back gracefully when
    /// the file or position is unknown (e.g. at the REPL). The file is shown as
    /// a relative path when it lives under the cwd, and as an absolute path
    /// otherwise.
    pub fn located(&self) -> String {
        let prefix = match (&self.file, self.pos) {
            (Some(f), Some(p)) => format!("{}:{}:{}: ", display_path(f), p.line, p.col),
            (Some(f), None) => format!("{}: ", display_path(f)),
            (None, Some(p)) => format!("{}:{}: ", p.line, p.col),
            (None, None) => String::new(),
        };
        format!("{}{}", prefix, self)
    }
    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Parse, message).with_code(error_codes::PARSE_GENERIC)
    }
    pub fn unbound(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unbound, message).with_code(error_codes::UNBOUND_SYMBOL)
    }
    pub fn arity(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Arity, message).with_code(error_codes::ARITY_MISMATCH)
    }
    pub fn type_err(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Type, message).with_code(error_codes::TYPE_MISMATCH)
    }
    /// A self-identifying type error: which operation (`who`), what it `expected`,
    /// and the actual tag + printed form of what arrived. Threads the heap to
    /// render the offending value, e.g. `first: expected list or vector, got int (5)`.
    pub fn wrong_type(
        heap: &crate::core::heap::Heap,
        who: &str,
        expected: &str,
        got: Value,
    ) -> Self {
        Self::type_err(format!(
            "{}: expected {}, got {} ({})",
            who,
            expected,
            crate::core::value::tag(got).name(),
            crate::syntax::printer::print(heap, got),
        ))
    }
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Runtime, message).with_code(error_codes::RUNTIME_GENERIC)
    }
    /// Construct the error raised by `(throw value)`, carrying the value. User
    /// throws **don't** carry a code — the user controls the payload shape; if
    /// they want one, they throw a map with `:code` themselves.
    pub fn thrown(value: Value, heap: &crate::core::heap::Heap) -> Self {
        LispError(Box::new(LispErrorData {
            kind: ErrorKind::User,
            message: crate::syntax::printer::display(heap, value),
            control: None,
            payload: Some(value),
            pos: None,
            file: None,
            code: None,
            hint: None,
        }))
    }

    /// Project the structured fields into a Brood map for `catch` consumption.
    /// Shape: `{:kind <keyword> :message <string> [:code <string>]
    /// [:file <string> :line <int> :col <int>] [:hint <string>]}` — every
    /// optional field is omitted when absent, so the agent's pattern match
    /// stays simple. Used by `try_catch` when the error carries no user
    /// payload (i.e. it's a built-in error). See `docs/llm-native.md` §4.
    pub fn to_value_map(&self, heap: &mut crate::core::heap::Heap) -> Value {
        use crate::core::value::{intern, Value};
        let kind_kw = Value::keyword(intern(self.kind.tag_name()));
        let msg_str = heap.alloc_string(&self.message);
        let mut entries: Vec<(Value, Value)> = Vec::with_capacity(8);
        entries.push((Value::keyword(intern("kind")), kind_kw));
        entries.push((Value::keyword(intern("message")), msg_str));
        if let Some(code) = self.code {
            let code_str = heap.alloc_string(code);
            entries.push((Value::keyword(intern("code")), code_str));
        }
        if let Some(file) = &self.file {
            let file_str = heap.alloc_string(file);
            entries.push((Value::keyword(intern("file")), file_str));
        }
        if let Some(pos) = self.pos {
            entries.push((Value::keyword(intern("line")), Value::int(pos.line as i64)));
            entries.push((Value::keyword(intern("col")), Value::int(pos.col as i64)));
        }
        if let Some(hint) = &self.hint {
            let hint_str = heap.alloc_string(hint);
            entries.push((Value::keyword(intern("hint")), hint_str));
        }
        heap.map_from_pairs(entries)
    }
}

impl fmt::Display for LispError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind.label(), self.message)
    }
}

impl std::error::Error for LispError {}

/// The result of evaluating something: a [`Value`](crate::core::value::Value) or a [`LispError`].
pub type LispResult = Result<crate::core::value::Value, LispError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_signal_is_distinct_from_errors() {
        // A suspend rides the error channel but reports as a control signal, so
        // error-handling natives (`%try`, `binding`, `%isolate`) re-raise it.
        let s = LispError::suspend(None);
        assert!(s.is_control());
        assert!(matches!(
            s.control,
            Some(Control::Suspend { deadline: None })
        ));
        // Real errors are never control signals.
        assert!(!LispError::new(ErrorKind::Runtime, "boom").is_control());
        assert!(!LispError::type_err("nope").is_control());
        assert!(!LispError::unbound("x").is_control());
    }
}
