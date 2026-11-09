//! The tree-sitter builtins in a runtime built without the feature: each is registered
//! anyway (like `gui-*`), and says how to get it. A missing grammar is nil.

use super::*;

fn without_treesit(who: &str) -> LispError {
    LispError::runtime(format!(
        "{who}: this runtime was built without tree-sitter (rebuild with --features treesit)"
    ))
}

pub fn tree_open(_text: ropey::Rope, _lang: &str, _owner: u64) -> Result<u64, LispError> {
    Err(without_treesit("%tree-sitter-tree"))
}

pub fn tree_reparse(
    _old: u64,
    _text: ropey::Rope,
    _lang: &str,
    _edits: Option<Vec<(usize, usize, usize)>>,
    _owner: u64,
) -> Result<u64, LispError> {
    Err(without_treesit("%tree-sitter-tree-reparse"))
}

pub fn tree_spans(
    _heap: &mut crate::core::heap::Heap,
    _handle: u64,
    _table: &SpanTable,
    _tokens: TokenRules,
    _from: i64,
    _to: i64,
    _injections: Option<&Injections>,
) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-spans"))
}

pub fn tree_changes(
    _heap: &mut crate::core::heap::Heap,
    _handle: u64,
    _injections: Option<&Injections>,
) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-changes"))
}

pub fn tree_free(_handle: u64) -> bool {
    false
}

pub fn tree_give(_handle: u64, _from: u64, _to: u64) -> Result<bool, GiveRefused> {
    Ok(false)
}

pub fn close_process_trees(_pid: u64) {}

pub fn owned_trees(_pid: u64) -> Vec<u64> {
    Vec::new()
}

pub fn tree_chain(_heap: &mut crate::core::heap::Heap, _handle: u64, _offset: i64) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-chain"))
}

pub fn tree_kids(
    _heap: &mut crate::core::heap::Heap,
    _handle: u64,
    _offset: i64,
    _all: bool,
) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-kids"))
}

pub fn tree_alive(_handle: u64) -> bool {
    false
}

pub fn spans(
    heap: &mut crate::core::heap::Heap,
    src: &str,
    lang: &str,
    _table: &SpanTable,
    _tokens: TokenRules,
    _injections: Option<&Injections>,
) -> LispResult {
    parse(heap, src, lang)
}

pub fn range_spans(
    _heap: &mut crate::core::heap::Heap,
    _text: RangeText,
    _lang: &str,
    _ranges: &[(usize, usize)],
    _table: &SpanTable,
    _tokens: TokenRules,
) -> LispResult {
    // no grammar can load in a runtime without tree-sitter, and a missing grammar is nil
    Ok(crate::core::value::Value::Nil)
}

pub fn grammar_info(_heap: &mut crate::core::heap::Heap, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-grammar-info::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

pub fn declare_grammar(_path: &str, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-declare-grammar::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

pub fn grammar_status(_heap: &mut crate::core::heap::Heap, _lang: &str) -> LispResult {
    Ok(crate::core::value::Value::Nil)
}

pub fn declared_grammars(heap: &mut crate::core::heap::Heap) -> LispResult {
    Ok(heap.map_from_pairs(Vec::new()))
}

/// The vetting child's entry (ADR-660): nothing to vet without tree-sitter.
pub fn vetting_entry() {}

pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

pub fn vet_grammar(
    _heap: &mut crate::core::heap::Heap,
    _path: &str,
    lang: &str,
    _corpus: &[Vec<u8>],
    _timeout: std::time::Duration,
    _force: bool,
) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-vet-grammar::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

pub fn grammar_verdict(_heap: &mut crate::core::heap::Heap, _path: &str) -> LispResult {
    Ok(crate::core::value::Value::Nil)
}

pub fn forget_grammar_verdict(_path: &str) -> LispResult {
    Ok(crate::core::value::Value::Bool(false))
}

pub fn languages(heap: &mut crate::core::heap::Heap) -> LispResult {
    Ok(heap.alloc_vector(Vec::new()))
}

pub fn load_grammar(_path: &str, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-load-grammar::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

/// Feature-off stub: the builtin is registered unconditionally (like `gui-*`), so
/// calling it without the parser built in gives a clear rebuild hint.
pub fn parse(_heap: &mut crate::core::heap::Heap, _src: &str, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-parse::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

pub fn chain(heap: &mut crate::core::heap::Heap, src: &str, lang: &str, _o: i64) -> LispResult {
    parse(heap, src, lang)
}

pub fn kids(
    heap: &mut crate::core::heap::Heap,
    src: &str,
    lang: &str,
    _o: i64,
    _all: bool,
) -> LispResult {
    parse(heap, src, lang)
}

pub fn errors(heap: &mut crate::core::heap::Heap, src: &str, lang: &str) -> LispResult {
    parse(heap, src, lang)
}
