//! The names of the Brood globals the kernel reads — one `const` per name, so the spelling
//! lives in exactly one place on the Rust side.
//!
//! Most are registries the prelude defines and Brood code writes (`provide`, `defrecord`,
//! `impl`, `defmulti` …) and the kernel only READS: the checker resolving abilities and
//! multimethods, the optimiser devirtualising an ability call, the load machinery asking
//! what is loaded or loading. Before this module each site re-typed `"*features*"` or
//! `"*impls*"` as a bare string, so a rename meant a grep across the kernel and a typo was
//! a silent miss (the lookup returns nothing, and every reader treats nothing as "empty").
//!
//! Spellings only, like [`crate::core::keywords`]: which registries exist, what they hold
//! and how they are written is policy, and stays in Brood. The one set the kernel used to
//! hard-code as policy — which registries stay LIVE while a module load is staged — is
//! now the prelude's [`LIVE_REGISTRIES`] value, read here by name.
//!
//! Conventionally imported as `use crate::core::registries as reg;`.

// ---- modules and loading (`std/prelude/tools.blsp`) ----------------------------------

/// Loaded modules: a map of module name → `true`. `provide` writes it; `require-one` trusts it.
pub const FEATURES: &str = "*features*";
/// In-flight loads: module name → the pid that claimed the load.
pub const FEATURES_LOADING: &str = "*features-loading*";
/// Processes waiting on an in-flight load: module name → `[pid ref]` list.
pub const FEATURES_WAITING: &str = "*features-waiting*";
/// The registries a module load must write to the live table even while its other writes are
/// staged (ADR-344): a set of registry SYMBOLS. Policy, defined by the prelude beside the
/// registries it names; see `Heap::is_live_coordination_registry`.
pub const LIVE_REGISTRIES: &str = "*live-registries*";
/// The directories `require` searches for a module's source.
pub const LOAD_PATH: &str = "*load-path*";
/// Whether a hot reload's `def` diagnostics print (`[reload] arity changed …`).
pub const RELOAD_DIAGNOSTICS: &str = "*reload-diagnostics*";

// ---- records, abilities, multimethods --------------------------------------------------

/// Record constructors' identities (`std/prelude/seq.blsp`).
pub const RECORD_IDS: &str = "*record-ids*";
/// Declared abilities (`std/prelude/tools.blsp`).
pub const ABILITIES: &str = "*abilities*";
/// An ability's required abilities.
pub const ABILITY_REQUIRES: &str = "*ability-requires*";
/// Ability implementations: ability → identity → op map.
pub const IMPLS: &str = "*impls*";
/// Which ability declares each op.
pub const OP_ABILITY: &str = "*op-ability*";
/// Sealed abilities.
pub const SEALED: &str = "*sealed*";
/// Multimethod methods, and the module each was defined from.
pub const METHODS: &str = "*methods*";
pub const METHOD_FROM: &str = "*method-from*";
/// Multimethod dispatch algebra and declared return types.
pub const MULTI_ALGEBRA: &str = "*multi-algebra*";
pub const MULTI_RET: &str = "*multi-ret*";
/// Declared protocols (`std/protocol.blsp` — a std module, not the prelude, so it may be
/// absent; every reader treats a missing registry as empty).
pub const PROTOCOLS: &str = "*protocols*";
