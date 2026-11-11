//! The **category** every checker finding carries (ADR-673 §5): one [`Lint`] per kind of
//! finding a user would want to silence together, named by the same kebab-case keyword a
//! `(check-allow :category …)` marker or a `(defmodule … (:check-allow :category …))`
//! header clause writes. Tooling (`nest check`'s gate, the LSP, an editor) decides what a
//! finding means from its category, never from the text of its message.

use std::collections::HashSet;

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self as value, Value};
use crate::error::Pos;

/// Declares [`Lint`] with each variant's keyword name, its doc comment, and [`Lint::ALL`]
/// in declaration order — one list, so a name and the variant can never drift apart.
macro_rules! lints {
    ($($(#[$doc:meta])* $variant:ident => $name:literal,)*) => {
        /// The category of one checker finding. See the module doc.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Lint {
            $($(#[$doc])* $variant,)*
        }

        impl Lint {
            /// Every category, in a stable order (the order `%check-categories` lists).
            pub const ALL: &'static [Lint] = &[$(Lint::$variant,)*];

            /// The category's keyword name, without the colon (`non-tail-recursion`).
            pub fn name(self) -> &'static str {
                match self {
                    $(Lint::$variant => $name,)*
                }
            }

            /// The category named `name` (no colon), or `None` for an unknown name.
            pub fn from_name(name: &str) -> Option<Lint> {
                match name {
                    $($name => Some(Lint::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

lints! {
    /// A self-call in non-tail position: deep recursion overflows the green-process stack.
    NonTailRecursion => "non-tail-recursion",
    /// A `match`/`cond` clause or `if` branch that can never run.
    UnreachableClause => "unreachable-clause",
    /// A provable type misuse: an argument, a return, a value, a callback or a call's
    /// shape (its arity, a dispatch with no impl or method) disjoint from what is wanted.
    TypeMismatch => "type-mismatch",
    /// A reference to a name nothing binds — a call that cannot run, including a special
    /// form's name used as a value.
    Unbound => "unbound",
    /// A use of a name its owner marks deprecated (ADR-283).
    Deprecated => "deprecated",
    /// A use of a name its owner marks beta (ADR-283).
    Beta => "beta",
    /// A `:total`-declared function the checker cannot prove covers every case or
    /// terminates (ADR-351).
    Total => "total",
    /// A `:pure`-declared function, or a `ui-memo` thunk, that performs an effect.
    Pure => "pure",
    /// A declared return the checker could only trust, not verify (strict mode).
    Trusted => "trusted",
    /// A computed test that is always true: its type admits neither nil nor false.
    ConstantCondition => "constant-condition",
    /// A truthiness test on a value that may be a list or another collection (ADR-620).
    ListTruthiness => "list-truthiness",
    /// A `catch` whose constant body discards the error it caught.
    DiscardedCatch => "discarded-catch",
    /// A module re-binding another module's global while it loads.
    CrossModuleDef => "cross-module-def",
    /// A multimethod every method of which dispatches on the first argument alone.
    SingleDispatch => "single-dispatch",
    /// A name defined twice in one file (and, from `nest check`, across a project).
    DuplicateDef => "duplicate-def",
    /// A module-private definition nothing references — the whole-project lint
    /// `nest check` runs in Brood; no per-file finding carries it.
    UnusedPrivate => "unused-private",
    /// A `let` binding, parameter or pattern binder the body never reads.
    UnusedBinding => "unused-binding",
    /// A `(:use …)` import none of whose names the file references.
    UnusedImport => "unused-import",
    /// A `(:use m :only [...])` name the module does not define publicly.
    UnknownImport => "unknown-import",
    /// A `(sig …)`, typed parameter or `deftype` that does not read, annotates nothing,
    /// disagrees with its definition, or exceeds the checker's budget.
    Declaration => "declaration",
    /// An impl, behaviour or sealed ability that does not conform to its declaration.
    Conformance => "conformance",
    /// A `match` over a sealed ability with no clause for one of its members.
    NonExhaustive => "non-exhaustive",
    /// An effectful call in a `:when` guard.
    EffectfulGuard => "effectful-guard",
    /// Code that runs but almost certainly does not mean what it says: a name evaluated
    /// and discarded, a zero-argument function printed instead of called, a `:when`
    /// inside a clause's head list.
    Suspicious => "suspicious",
    /// A definition of a reserved name, which fails when it runs (ADR-166).
    ReservedName => "reserved-name",
    /// A form that does not compile — the file would fail to load.
    CompileError => "compile-error",
    /// A budget the checker ran out of: part of the file was read as unknown.
    GaveUp => "gave-up",
    /// The checker itself failed (an internal panic); the advisory pass was aborted.
    Internal => "internal",
    /// A `(:check-allow …)` header clause naming a category that does not exist.
    CheckAllow => "check-allow",
}

/// One checker finding: where (when known), which category, and the message.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Finding {
    pub pos: Option<Pos>,
    pub lint: Lint,
    pub message: String,
}

impl Finding {
    /// A finding of category `lint` at `pos`.
    pub fn new(lint: Lint, pos: Option<Pos>, message: String) -> Finding {
        Finding { pos, lint, message }
    }

    /// The `(pos, message)` pair the tuple-shaped entry points return.
    pub fn into_pair(self) -> (Option<Pos>, String) {
        (self.pos, self.message)
    }
}

/// The categories each `(defmodule …)` of a file allows through its `(:check-allow …)`
/// header clauses, read from the UN-expanded forms before any pass runs (afterwards a
/// module load may have moved them). A module's clause governs its own REGION — the
/// lines from its `defmodule` to the next one's (ADR-223); a finding with no position is
/// governed by the file's first module.
#[derive(Default)]
pub(super) struct FileAllows {
    /// `(first line of the region, allowed categories)`, in source order.
    regions: Vec<(u32, HashSet<Lint>)>,
    /// One `check-allow` finding per unknown category a clause names.
    pub(super) unknown: Vec<Finding>,
}

impl FileAllows {
    pub(super) fn read(heap: &Heap, forms: &[Value]) -> FileAllows {
        let mut allows = FileAllows::default();
        for &form in forms {
            let Some(items) = heap.list_items(form) else {
                continue;
            };
            let (Some(&Value::Sym(head)), Some(&Value::Sym(_))) = (items.first(), items.get(1))
            else {
                continue;
            };
            if !value::symbol_is(head, kw::DEFMODULE) {
                continue;
            }
            let start = heap.form_pos_only(form).map_or(0, |pos| pos.line);
            let mut allowed = HashSet::new();
            for &clause in &items[2..] {
                let Some(clause_items) = heap.list_items(clause) else {
                    continue;
                };
                if !matches!(clause_items.first(), Some(&Value::Keyword(k)) if value::symbol_is(k, "check-allow"))
                {
                    continue;
                }
                let pos = heap
                    .form_pos_only(clause)
                    .or_else(|| heap.form_pos_only(form));
                for &entry in &clause_items[1..] {
                    let named = match entry {
                        Value::Keyword(k) => Lint::from_name(value::symbol_name_ref(k)),
                        _ => None,
                    };
                    match named {
                        Some(lint) => {
                            allowed.insert(lint);
                        }
                        None => allows.unknown.push(Finding::new(
                            Lint::CheckAllow,
                            pos,
                            format!(
                                "(:check-allow …): `{}` is not a check category",
                                crate::syntax::printer::print(heap, entry)
                            ),
                        )),
                    }
                }
            }
            allows.regions.push((start, allowed));
        }
        allows
    }

    /// Whether `finding` is allowed by the header of the module whose region holds it.
    pub(super) fn allows(&self, finding: &Finding) -> bool {
        let region = match finding.pos {
            Some(pos) => self
                .regions
                .iter()
                .rev()
                .find(|(start, _)| *start <= pos.line)
                .or(self.regions.first()),
            None => self.regions.first(),
        };
        region.is_some_and(|(_, allowed)| allowed.contains(&finding.lint))
    }
}
