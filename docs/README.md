# mylisp documentation

This folder is the detailed record of what mylisp is, how it's built, and where
it's going. Start here.

| Document | What's in it |
|---|---|
| [architecture.md](architecture.md) | The big picture: the runtime, the crate layout, the eval loop, the memory model, and the "one runtime that can also be a server" design that the whole project is organised around. |
| [language.md](language.md) | The language reference *as implemented today* (v0.1): data types, syntax, special forms, and every builtin. |
| [roadmap.md](roadmap.md) | The milestones (M1 → M5), what's done, and what's next — including the editor, the display protocol, and the remote/web frontends. |
| [decisions.md](decisions.md) | The design-decision log (ADR-style): the *why* behind the choices, so future-us doesn't relitigate them by accident. |
| [devlog.md](devlog.md) | A chronological log of work sessions — what changed and why, in order. |

## The one-paragraph version

mylisp is a small, dynamic Lisp implemented in Rust. Its reason for existing is
to be the language a modern, Emacs-like text editor is *written in* — so that a
running editor can redefine its own behaviour by re-evaluating code. v0.1 is the
language core: a reader, a tree-walking evaluator with proper tail calls and
lexical closures, a set of builtins, and a REPL. The editor, the display
protocol, and the remote/web frontends come later (see the roadmap).
