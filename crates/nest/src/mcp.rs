//! The Rust half of `nest mcp`: the reference documents this binary embeds.
//!
//! The server itself — transport, dispatch, JSON, error mapping, the tool catalogue — is
//! Brood (`std/tool/mcp.blsp`, routed like every other Brood subcommand). What only the
//! binary can supply is the text it compiled in: the docs an agent reads as MCP resources,
//! the prelude source, and the `brood-task` prompt. They reach the server as the dynamic
//! `mcp/*documents*`, bound around the dispatcher call.

use brood::introspect::string_literal;

/// `(name text)` for every document the server serves, keyed as `std/tool/mcp.blsp`'s
/// resource table and `prompts/get` name them.
const DOCUMENTS: &[(&str, &str)] = &[
    (
        "brood-for-claude",
        include_str!("../../../docs/brood-for-claude.md"),
    ),
    (
        "incarnations",
        include_str!("../../../docs/incarnations.md"),
    ),
    (
        "llm-native",
        include_str!("../../../docs/archive/llm-native.md"),
    ),
    (
        "claude-demo-findings",
        include_str!("../../../docs/archive/claude-demo-findings.md"),
    ),
    ("language", include_str!("../../../docs/language.md")),
    // The whole ADR log: the index, then the files it is split across by number.
    (
        "decisions",
        concat!(
            include_str!("../../../docs/decisions.md"),
            include_str!("../../../docs/decisions/adr-001-099.md"),
            include_str!("../../../docs/decisions/adr-100-199.md"),
            include_str!("../../../docs/decisions/adr-200-299.md"),
            include_str!("../../../docs/decisions/adr-300-399.md"),
            include_str!("../../../docs/decisions/adr-400-499.md"),
            include_str!("../../../docs/decisions/adr-500-599.md"),
            include_str!("../../../docs/decisions/adr-600-699.md"),
        ),
    ),
    ("types", include_str!("../../../docs/types.md")),
    ("error-codes", include_str!("../../../docs/error-codes.md")),
    ("prelude", brood::boot::PRELUDE),
    (
        "brood-task",
        include_str!("../../../docs/prompts/brood-task.md"),
    ),
];

/// `call` (the `(nest/main …)` form) with `mcp/*documents*` bound to the embedded texts.
/// `mcp` is loaded first so its `defdyn` declares the variable before it is bound.
pub fn with_embedded_documents(call: &str) -> String {
    let entries: Vec<String> = DOCUMENTS
        .iter()
        .map(|(name, text)| format!("\"{name}\" {}", string_literal(text)))
        .collect();
    format!(
        "(do (require-one 'mcp) (binding (mcp/*documents* {{{}}}) {call}))",
        entries.join(" ")
    )
}
