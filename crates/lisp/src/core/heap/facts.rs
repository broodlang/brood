//! Side facts — what evaluating a definition **records about** a name rather than **binds
//! to** it (ADR-320). Child of heap.
//!
//! Every startup image in this repo rests on one sentence:
//!
//! > **Materialising defines bindings and evaluates nothing** — so anything the evaluation
//! > *did* must be replayed, and anything it *recorded on the side* must be written.
//!
//! The rule is right and was unenforceable as prose, because "anything it recorded on the
//! side" is an open set that grows whenever someone adds a new kind of fact about a name.
//! `write_prelude_image` carried five such facts in five hand-written blocks, and **every one
//! of them was added after a bug** — KI-72, KI-84, KI-89's residual, KI-105 and KI-106, plus
//! ADR-314's own three amendments. The source comment on the last block says so outright:
//! "found the same way — late." Omitting a fact is silent, and its symptom lands far from the
//! omission: a checker warning about a record in another file, an `unbound symbol` for a
//! module that is fine, a stdlib jump-to-definition that quietly does nothing.
//!
//! So the list is not maintained by hand any more. [`FactKind`] names the kinds and
//! [`FactKind::ALL`] is generated from the same macro invocation that declares them, so the
//! two cannot disagree; [`Heap::side_facts`] collects by iterating `ALL` through an
//! **exhaustive match**, and [`Heap::replay_fact`] applies one the same way. Adding a sixth
//! kind therefore cannot be forgotten at the *carry* step — it is carried by construction —
//! and the only thing left to write is its encoding, which is another exhaustive match in
//! `builtins/startup_image.rs`.
//!
//! That conversion is the whole point: **from a silent omission whose symptom appears in
//! another subsystem, to a compile error at the point of the change.** Everything else here
//! is mechanism.
//!
//! The principle was already in the tree, applied once — `registry_lock` guards a *value*
//! because "naming them by hand went stale three times, silently; this set is derived from
//! the writes themselves". This generalises that to every kind, including the set that
//! `registry_lock` holds, which is the one the image writer then forgot (KI-106).

use super::*;
use crate::core::value;

/// Declare the fact kinds once, and derive [`FactKind::ALL`] from the same list.
///
/// A separate hand-written `ALL` is exactly the failure this module exists to remove: it
/// would be a second place to forget, and the alternatives section of ADR-320 rejects
/// "a completeness test that enumerates the kinds" for that reason — it moves the
/// hand-maintained list rather than removing it.
macro_rules! fact_kinds {
    ($($(#[$attr:meta])* $variant:ident),+ $(,)?) => {
        /// The kinds of fact a name can carry beside its binding.
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub enum FactKind {
            $($(#[$attr])* $variant),+
        }

        impl FactKind {
            /// Every kind, generated from the declaration above — complete by construction.
            pub const ALL: &'static [FactKind] = &[$(FactKind::$variant),+];
        }
    };
}

fact_kinds! {
    /// Module-private globals (ADR-146). The name is clean — privacy is declared by the
    /// def FORM, not spelled in the name — so this set is the only authority.
    Private,
    /// Stability metadata (ADR-283): `:since` / `:deprecated` / `:beta`, recorded by
    /// `%register-meta`. A module re-runs that on load; the prelude is *inserted*, so
    /// nothing re-runs it.
    Meta,
    /// Where a name was defined, so `M-.` works on an imaged boot. ADR-138 kept a whole
    /// positioned read alive for this; here it is thirty bytes a name.
    DefSite,
    /// Globals a `%registry-update!` has written (ADR-218). A registry is precisely a
    /// global that loading MUTATES rather than creates, so the `global-names` diff an
    /// image is built from cannot see it, and one left out is lost with no error (KI-106).
    RegistryName,
    /// `defdyn`-ness, which lives in a process-global set (`value::DYNAMICS`) rather than
    /// in the binding — so restoring the VALUE of `*require-parent*` without its mark
    /// leaves `binding`, and therefore every `require`, rejecting it.
    Dynamic,
}

/// One recorded fact, with its subject. The payload differs per kind, which is why this is
/// an enum rather than a `(FactKind, Symbol)` pair: encoding is then an exhaustive match.
#[derive(Clone, Debug)]
pub enum Fact {
    Private(Symbol),
    Meta(Symbol, NameMeta),
    DefSite(Symbol, SourceLoc),
    RegistryName(Symbol),
    Dynamic(Symbol),
}

impl Fact {
    /// This fact's kind. Exhaustive on purpose — a new variant must be classified here.
    pub fn kind(&self) -> FactKind {
        match self {
            Fact::Private(_) => FactKind::Private,
            Fact::Meta(..) => FactKind::Meta,
            Fact::DefSite(..) => FactKind::DefSite,
            Fact::RegistryName(_) => FactKind::RegistryName,
            Fact::Dynamic(_) => FactKind::Dynamic,
        }
    }

    /// The name this fact is about.
    pub fn subject(&self) -> Symbol {
        match self {
            Fact::Private(s)
            | Fact::Meta(s, _)
            | Fact::DefSite(s, _)
            | Fact::RegistryName(s)
            | Fact::Dynamic(s) => *s,
        }
    }
}

impl Heap {
    /// Every side fact this runtime has recorded, in [`FactKind::ALL`] order.
    ///
    /// **This is the function a new fact kind must not be able to skip**, and the match
    /// below is what makes that true: add a variant to [`FactKind`] and this stops
    /// compiling. Adding the arm is then the whole of the carry step — the image writer
    /// iterates whatever this returns.
    ///
    /// Reads each kind from the storage that already owns it rather than from a parallel
    /// log, so there is exactly one authority per fact and no way for a journal to drift
    /// from the state it describes.
    pub fn side_facts(&self) -> Vec<Fact> {
        let mut out = Vec::new();
        for kind in FactKind::ALL {
            match kind {
                FactKind::Private => {
                    out.extend(self.private_names_snapshot().into_iter().map(Fact::Private));
                }
                FactKind::Meta => {
                    out.extend(
                        self.name_meta_snapshot()
                            .into_iter()
                            .map(|(s, m)| Fact::Meta(s, m)),
                    );
                }
                FactKind::DefSite => {
                    // Read from the same two places — and in the same precedence — that
                    // `Heap::def_site` reads: this runtime's map first, then the frozen
                    // prelude's. Both are needed because the two callers stand at opposite
                    // sides of the freeze. The image WRITER runs on the builder heap, where
                    // the sites are still in the runtime map and the shared one is empty; a
                    // LIVE process reads them from the shared one, because
                    // `freeze_as_shared_code` moved them there. A snapshot that saw only the
                    // runtime map reported **zero** def sites in every live process — and
                    // reported it identically in both differential arms, which is precisely
                    // how a fingerprint agrees without checking anything.
                    let runtime = self.def_sites_snapshot();
                    let in_runtime: std::collections::HashSet<Symbol> =
                        runtime.iter().map(|(s, _)| *s).collect();
                    out.extend(runtime.into_iter().map(|(s, l)| Fact::DefSite(s, l)));
                    out.extend(
                        self.prelude
                            .def_sites
                            .iter()
                            .filter(|(s, _)| !in_runtime.contains(s))
                            .map(|(s, l)| Fact::DefSite(*s, l.clone())),
                    );
                }
                FactKind::RegistryName => {
                    out.extend(self.registry_names().into_iter().map(Fact::RegistryName));
                }
                FactKind::Dynamic => {
                    out.extend(
                        value::dynamic_names()
                            .into_iter()
                            .map(|n| Fact::Dynamic(value::intern(&n))),
                    );
                }
            }
        }
        out
    }

    /// Re-record one fact — the restore half of [`Heap::side_facts`], and exhaustive for
    /// the same reason. Every arm routes to the same entry point ordinary evaluation uses,
    /// so a replayed fact is indistinguishable from a recorded one.
    pub fn replay_fact(&self, fact: &Fact) {
        match fact {
            Fact::Private(sym) => self.mark_private(*sym),
            Fact::Meta(sym, meta) => self.set_name_meta(*sym, meta.clone()),
            Fact::DefSite(sym, loc) => self.set_def_site(*sym, loc.clone()),
            Fact::RegistryName(sym) => self.mark_registry_names(&[*sym]),
            Fact::Dynamic(sym) => value::mark_dynamic(*sym),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL` is generated from the variant list, so this cannot drift — but a reader
    /// coming from ADR-320 wants to see the count asserted somewhere, and a future
    /// hand-edit of the macro would be caught here.
    #[test]
    fn every_kind_is_in_all_and_classifies_itself() {
        assert_eq!(FactKind::ALL.len(), 5);
        let sym = value::intern("x");
        let samples = [
            Fact::Private(sym),
            Fact::Meta(sym, NameMeta::default()),
            Fact::DefSite(
                sym,
                SourceLoc {
                    file: "f".into(),
                    pos: crate::error::Pos { line: 1, col: 1 },
                },
            ),
            Fact::RegistryName(sym),
            Fact::Dynamic(sym),
        ];
        for kind in FactKind::ALL {
            assert!(
                samples.iter().any(|f| f.kind() == *kind),
                "no sample Fact for {kind:?} — add one, and check the image encodes it"
            );
        }
        for f in &samples {
            assert_eq!(f.subject(), sym);
        }
    }
}
