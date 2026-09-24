//! `BROOD_FEATURES_AUDIT=1` — catch the write that leaves `*features*` listing a module
//! whose bindings are gone.
//!
//! That state is the end of KI-119, KI-120, KI-170 and the 2026-09-23 wave (KI-193's
//! entry): `require-one` trusts `*features*`, so a module listed there with nothing bound is
//! never reloaded, every `(:use m)` after it imports nothing, and each bare use dies
//! `unbound symbol` in some other process, long after and far from the write that caused it.
//! `%refer`'s `[refer] … imported NOTHING` line reports the symptom; this reports the cause.
//!
//! Two checks, both only when the flag is set (one cached read when it is not):
//!
//! - **After every multi-name table swap** — a module publish, an `%isolate` restore — and
//!   after every live write of `*features*`: a module listed in `*features*` that HAD bindings
//!   at an earlier audit and has none now. Remembering which modules were ever seen bound is
//!   what keeps it quiet about the modules that legitimately define nothing public (a test
//!   file's own module, a fixture).
//! - **At a live whole-map registry write of `*features*`:** the new map adds keys the
//!   operation does not name — a map computed from a stale view, resurrecting entries a
//!   restore removed.
//!
//! Each report carries the event, pid, isolate scope and a Rust backtrace, once per module.
//! Deliberately cheap enough to leave armed in a full suite run (one pass over the table's
//! keys per event), because heavier tracing (`BROOD_TRACE_GLOBAL`) suppressed this race.

use super::*;
use crate::core::registries as reg;

fn armed() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_FEATURES_AUDIT").is_some())
}

/// Modules seen with at least one binding while listed in `*features*`.
static EVER_BOUND: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

fn features_sym() -> Symbol {
    static SYM: std::sync::OnceLock<Symbol> = std::sync::OnceLock::new();
    *SYM.get_or_init(|| crate::core::value::intern(reg::FEATURES))
}

impl Heap {
    fn feature_keys(&self, v: Option<Value>) -> std::collections::BTreeSet<String> {
        match v.map(|v| v.unpack()) {
            Some(ValueRef::Map(id)) => self
                .map_entries(id)
                .into_iter()
                .map(|(k, _)| {
                    crate::syntax::printer::print(self, k)
                        .trim_matches('"')
                        .to_string()
                })
                .collect(),
            _ => Default::default(),
        }
    }

    /// Audit `table` — the globals table as it stands after `event` — for a listed module
    /// whose bindings have gone. Callers pass the table they hold the lock on.
    pub(super) fn features_audit_table(&self, table: &SymbolMap<Value>, event: &str) {
        if !armed() {
            return;
        }
        let listed = self.feature_keys(table.get(&features_sym()).copied());
        let bound: std::collections::HashSet<String> = table
            .keys()
            .filter_map(|s| {
                crate::core::value::symbol_name(*s)
                    .rsplit_once('/')
                    .map(|(module, _)| module.to_string())
            })
            .collect();
        let mut ever = EVER_BOUND.lock().unwrap_or_else(|e| e.into_inner());
        let ever = ever.get_or_insert_with(Default::default);
        for module in listed {
            if bound.contains(&module) {
                ever.insert(module);
            } else if ever.remove(&module) {
                report(
                    event,
                    &format!("*features* lists `{module}`, whose bindings are gone"),
                );
            }
        }
    }

    /// [`Self::features_audit_table`] after a plain `def` — only when that `def` wrote
    /// `*features*` itself, so an armed run does not scan the table once per definition.
    pub(super) fn features_audit_define(&self, sym: Symbol, table: &SymbolMap<Value>) {
        if armed() && sym == features_sym() {
            self.features_audit_table(table, "live define of *features*");
        }
    }

    /// Audit a live whole-map write of registry `sym`, about to replace the table's value
    /// with `after`, by an operation on `path`: does it add `*features*` keys the operation
    /// does not name? Call before the write lands.
    pub(super) fn features_audit_write(&self, sym: Symbol, path: &[Value], after: Value) {
        if !armed() || sym != features_sym() {
            return;
        }
        let before = self.runtime.globals_read().get(&sym).copied();
        let named: Vec<String> = path
            .iter()
            .map(|p| {
                crate::syntax::printer::print(self, *p)
                    .trim_matches('"')
                    .to_string()
            })
            .collect();
        let before = self.feature_keys(before);
        let resurrected: Vec<String> = self
            .feature_keys(Some(after))
            .difference(&before)
            .filter(|k| !named.contains(k))
            .cloned()
            .collect();
        if !resurrected.is_empty() {
            report(
                "live *features* write",
                &format!("an operation on {named:?} also adds {resurrected:?} — a map computed from a stale view"),
            );
        }
    }
}

fn report(event: &str, what: &str) {
    eprintln!(
        "[features-audit] {event}: {what} (pid={:?} scope={})\n{}",
        crate::process::current_pid(),
        crate::process::self_isolate_scope(),
        std::backtrace::Backtrace::force_capture()
    );
}
