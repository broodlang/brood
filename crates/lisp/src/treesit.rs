//! Tree-sitter parsing for *foreign* languages (feature `treesit`) — ROADMAP §C.
//!
//! Brood parses its own `.blsp` with the reader (`parse-source-positioned` in
//! `builtins.rs` projects a positioned CST). For Ruby, Elixir, … there is no
//! Brood reader, so this module wraps tree-sitter — the incremental,
//! error-tolerant parser — behind a single builtin, `tree-sitter-parse`.
//!
//! The point is *shape parity*: it converts a tree-sitter tree into the **same
//! positioned node maps** the Brood CST gives — `{:kind :start :end :named}` for
//! leaves (plus `:text`, the raw source), and additionally `:kids` (a vector of
//! child maps) for any node with children. `:start`/`:end` are half-open
//! CHARACTER offsets (tree-sitter counts bytes; we project them, exactly as
//! `parse-source-positioned`), so `std/tool/sexp`'s structural navigation and the
//! editor's `:fontify` service run over a foreign tree **unchanged**. `:named`
//! distinguishes grammar nodes from anonymous tokens (keywords/punctuation like
//! `def`/`end`/`(`), which a fontifier wants and a navigator filters out.
//!
//! Mechanism only: parse + project. All policy (which node kinds get which face,
//! how to navigate) lives in Brood (`std/editor/treesit.blsp` + the modes). Add a
//! language = add a grammar crate in `Cargo.toml` + one arm in `language_for`.
//!
//! Like the `gui` backend, the builtin is always registered; without the feature
//! it returns a runtime error telling you to rebuild with `--features treesit`.

use crate::core::value::Value;
use crate::error::{LispError, LispResult};

#[cfg(feature = "treesit")]
use crate::core::heap::Heap;
#[cfg(feature = "treesit")]
use crate::core::value;
#[cfg(feature = "treesit")]
use std::collections::HashMap;
#[cfg(feature = "treesit")]
use std::sync::{LazyLock, Mutex};

/// `(tree-sitter-parse source lang)` — parse `source` (a string) with the
/// grammar named by keyword `lang` (`:ruby`, `:elixir`) into a positioned CST: a
/// `{:kind :start :end :named :kids/:text}` node map (see the module docs). Errors
/// on an unknown language, or when the runtime wasn't built `--features treesit`.
#[cfg(feature = "treesit")]
pub fn parse(heap: &mut Heap, src: &str, lang: &str) -> LispResult {
    let language = language_for(lang)?;
    let mut parser = checkout_parser(lang, &language)?;
    let tree = parser.parse(src, None);
    return_parser(lang, parser);
    let tree = tree
        .ok_or_else(|| LispError::runtime(format!("tree-sitter-parse: {lang}: parse failed")))?;
    let b2c = byte_to_char_offsets(src);
    Ok(node_to_positioned(heap, tree.root_node(), src, &b2c))
}

/// `(tree-sitter-reparse key source lang)` — like `tree-sitter-parse`, but
/// **incremental**: the last `(source, tree)` for `key` (a caller-chosen buffer
/// id) is kept, and a re-parse re-uses it via tree-sitter's edit machinery so
/// only the changed region is re-scanned — what incremental parsing exists for
/// (a self-editing editor reparsing on every keystroke). The result is the SAME
/// positioned CST `tree-sitter-parse` returns; only the work to produce it
/// shrinks. The edit is derived by diffing the cached source against `source`
/// (longest common prefix + suffix → one contiguous `InputEdit`), so the editor
/// needn't track edit ranges itself. Identical source re-uses the cached tree
/// with no re-parse. Call `tree-sitter-forget` when a buffer closes. The result
/// is **identical** to a from-scratch `tree-sitter-parse` (a test asserts this);
/// incrementality is a pure optimization.
#[cfg(feature = "treesit")]
pub fn parse_incremental(heap: &mut Heap, key: i64, src: &str, lang: &str) -> LispResult {
    let language = language_for(lang)?;
    let mut parser = checkout_parser(lang, &language)?;
    let cache_key = (key, lang.to_string());
    // Take the cached (src, tree) out (so the lock isn't held across the parse).
    let prev = TREE_CACHE.lock().expect("treesit cache").remove(&cache_key);
    let tree = match prev {
        Some((old_src, mut old_tree)) => match compute_edit(&old_src, src) {
            // Unchanged: re-use the cached tree, no re-parse.
            None => Some(old_tree),
            Some(edit) => {
                old_tree.edit(&edit);
                parser.parse(src, Some(&old_tree))
            }
        },
        None => parser.parse(src, None),
    };
    return_parser(lang, parser);
    let tree = tree
        .ok_or_else(|| LispError::runtime(format!("tree-sitter-reparse: {lang}: parse failed")))?;
    let b2c = byte_to_char_offsets(src);
    let result = node_to_positioned(heap, tree.root_node(), src, &b2c);
    // Re-cache for the next reparse (move the tree in after projecting).
    cache_store(cache_key, src.to_string(), tree);
    Ok(result)
}

/// `(tree-sitter-forget key)` — drop every cached incremental tree for `key`
/// (across all languages). Call when a buffer closes so the cache can't grow
/// unbounded. Returns the number of entries dropped. No-op without `treesit`.
#[cfg(feature = "treesit")]
pub fn forget(key: i64) -> i64 {
    let mut cache = TREE_CACHE.lock().expect("treesit cache");
    let before = cache.len();
    cache.retain(|(k, _), _| *k != key);
    (before - cache.len()) as i64
}

/// The grammar for a language keyword's name. Each arm is gated on its own
/// `treesit-<lang>` feature — the kernel ships no grammar by default (a `treesit`
/// build with no grammar feature has zero arms and reports every language as not
/// built in). One cfg'd arm per language is the single place a grammar plugs in
/// (plus its `Cargo.toml` dep). The unused-var `allow` covers the no-grammar
/// build, where `lang` is only echoed in the error.
#[cfg(feature = "treesit")]
#[cfg_attr(
    not(any(feature = "treesit-ruby", feature = "treesit-elixir")),
    allow(unused_variables)
)]
fn language_for(lang: &str) -> Result<tree_sitter::Language, LispError> {
    match lang {
        #[cfg(feature = "treesit-ruby")]
        "ruby" => Ok(tree_sitter_ruby::LANGUAGE.into()),
        #[cfg(feature = "treesit-elixir")]
        "elixir" => Ok(tree_sitter_elixir::LANGUAGE.into()),
        other => Err(LispError::runtime(format!(
            "tree-sitter-parse: language :{other} is not built into this runtime \
             (rebuild with --features treesit-{other}, or treesit-grammars for all)"
        ))),
    }
}

/// A pool of ready `Parser`s per language, so a parse doesn't pay
/// `Parser::new()` + `set_language` (grammar load) every call. `Parser` is
/// `Send`, and Brood green processes migrate across worker threads, so this is a
/// global pool rather than thread-local. Capped so a burst of concurrent parses
/// can't retain an unbounded number of parsers.
#[cfg(feature = "treesit")]
static PARSER_POOL: LazyLock<Mutex<HashMap<String, Vec<tree_sitter::Parser>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "treesit")]
const PARSER_POOL_PER_LANG: usize = 8;

/// Borrow a parser configured for `lang` from the pool (or build one). Held by
/// the caller during the parse — *not* across the pool lock — so parses run
/// concurrently. Return it with [`return_parser`].
#[cfg(feature = "treesit")]
fn checkout_parser(
    lang: &str,
    language: &tree_sitter::Language,
) -> Result<tree_sitter::Parser, LispError> {
    if let Some(p) = PARSER_POOL
        .lock()
        .expect("treesit parser pool")
        .get_mut(lang)
        .and_then(Vec::pop)
    {
        return Ok(p);
    }
    let mut p = tree_sitter::Parser::new();
    p.set_language(language)
        .map_err(|e| LispError::runtime(format!("tree-sitter-parse: {lang}: {e}")))?;
    Ok(p)
}

/// Return a parser to the pool for re-use (dropping it if the pool is full).
#[cfg(feature = "treesit")]
fn return_parser(lang: &str, parser: tree_sitter::Parser) {
    let mut pool = PARSER_POOL.lock().expect("treesit parser pool");
    let slot = pool.entry(lang.to_string()).or_default();
    if slot.len() < PARSER_POOL_PER_LANG {
        slot.push(parser);
    }
}

/// The incremental-parse cache: the last `(source, tree)` per `(buffer-key,
/// language)`. `Tree` is `Send + Sync`, so this is a global map shared across
/// worker threads. Bounded by [`TREE_CACHE_CAP`] buffers (an arbitrary entry is
/// evicted on overflow) so it can't grow without limit if a caller never calls
/// `tree-sitter-forget`.
#[cfg(feature = "treesit")]
#[allow(clippy::type_complexity)]
static TREE_CACHE: LazyLock<Mutex<HashMap<(i64, String), (String, tree_sitter::Tree)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "treesit")]
const TREE_CACHE_CAP: usize = 256;

#[cfg(feature = "treesit")]
fn cache_store(key: (i64, String), src: String, tree: tree_sitter::Tree) {
    let mut cache = TREE_CACHE.lock().expect("treesit cache");
    if cache.len() >= TREE_CACHE_CAP && !cache.contains_key(&key) {
        // Bounded: evict one arbitrary entry rather than grow past the cap.
        if let Some(victim) = cache.keys().next().cloned() {
            cache.remove(&victim);
        }
    }
    cache.insert(key, (src, tree));
}

/// Derive a single contiguous `InputEdit` from the difference between `old` and
/// `new` source: the longest common prefix and suffix bracket the changed middle.
/// `None` when the texts are identical (no re-parse needed). Treating multiple
/// disjoint edits as one spanning edit is conservative — tree-sitter re-scans a
/// little more, never less — so the resulting tree is always correct (asserted by
/// the incremental == from-scratch test). Byte offsets are snapped outward to
/// char boundaries so the edit never splits a multi-byte char.
#[cfg(feature = "treesit")]
fn compute_edit(old: &str, new: &str) -> Option<tree_sitter::InputEdit> {
    if old == new {
        return None;
    }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    // Common prefix, snapped back to a shared char boundary.
    let mut start = 0;
    let max_pre = ob.len().min(nb.len());
    while start < max_pre && ob[start] == nb[start] {
        start += 1;
    }
    while start > 0 && !new.is_char_boundary(start) {
        start -= 1; // boundaries coincide in the shared prefix
    }
    // Common suffix; old_end/new_end snapped back together (the suffix is shared,
    // so decrementing both keeps them aligned) and not past `start`.
    let mut suf = 0;
    let max_suf = max_pre - start;
    while suf < max_suf && ob[ob.len() - 1 - suf] == nb[nb.len() - 1 - suf] {
        suf += 1;
    }
    let mut old_end = ob.len() - suf;
    let mut new_end = nb.len() - suf;
    while (old_end > start && new_end > start)
        && (!old.is_char_boundary(old_end) || !new.is_char_boundary(new_end))
    {
        old_end -= 1;
        new_end -= 1;
    }
    Some(tree_sitter::InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point_at(old, start),
        old_end_position: point_at(old, old_end),
        new_end_position: point_at(new, new_end),
    })
}

/// The tree-sitter `Point` (zero-based row + **byte** column, as tree-sitter
/// counts it) at byte offset `byte` in `s`.
#[cfg(feature = "treesit")]
fn point_at(s: &str, byte: usize) -> tree_sitter::Point {
    let pre = &s.as_bytes()[..byte];
    let row = bytecount_newlines(pre);
    let column = match pre.iter().rposition(|&b| b == b'\n') {
        Some(nl) => byte - nl - 1,
        None => byte,
    };
    tree_sitter::Point { row, column }
}

#[cfg(feature = "treesit")]
fn bytecount_newlines(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c == b'\n').count()
}

/// Per-byte → character-offset table for `s`: `t[b]` is the count of characters
/// before byte offset `b`. Length `s.len() + 1` so a node's end byte (which can
/// equal `s.len()`) is indexable. tree-sitter spans land on char boundaries (it
/// parses UTF-8); a byte interior to a multi-byte char maps to that char's own
/// index. (Mirror of `builtins.rs::byte_to_char_offsets`, kept local so the
/// feature-off build links neither.)
#[cfg(feature = "treesit")]
fn byte_to_char_offsets(s: &str) -> Vec<u32> {
    let mut t = vec![0u32; s.len() + 1];
    let mut byte = 0usize;
    let mut ci = 0u32;
    for ch in s.chars() {
        let w = ch.len_utf8();
        for k in 0..w {
            t[byte + k] = ci;
        }
        byte += w;
        ci += 1;
    }
    t[s.len()] = ci;
    t
}

/// Convert a tree-sitter node (and its subtree) into a positioned node map,
/// mirroring `builtins.rs::cst_to_positioned`: a node with children carries
/// `:kids` (ALL children — named and anonymous, so keywords/operators are
/// present for fontify); a leaf carries `:text`.
#[cfg(feature = "treesit")]
fn node_to_positioned(heap: &mut Heap, node: tree_sitter::Node, src: &str, b2c: &[u32]) -> Value {
    let kw = |k: &str| Value::keyword(value::intern(k));
    let start = Value::int(b2c[node.start_byte()] as i64);
    let end = Value::int(b2c[node.end_byte()] as i64);
    let mut pairs: Vec<(Value, Value)> = vec![
        (kw("kind"), kw(node.kind())),
        (kw("start"), start),
        (kw("end"), end),
        (kw("named"), Value::boolean(node.is_named())),
    ];
    // Surface tree-sitter's error-recovery state so editor mode services can draw
    // diagnostics over a foreign tree: `:error` is an `ERROR` node, `:missing` a
    // zero-width inserted node (which `:kind` can't signal — it has no error-string
    // and zero width). Pushed only when set, to keep the common (valid) node small.
    if node.is_error() {
        pairs.push((kw("error"), Value::boolean(true)));
    }
    if node.is_missing() {
        pairs.push((kw("missing"), Value::boolean(true)));
    }
    if node.child_count() == 0 {
        let text = heap.alloc_string(&src[node.start_byte()..node.end_byte()]);
        pairs.push((kw("text"), text));
    } else {
        // Collect child maps first (recursion needs `&mut heap`), then the vector.
        let mut cursor = node.walk();
        let children: Vec<tree_sitter::Node> = node.children(&mut cursor).collect();
        let kids: Vec<Value> = children
            .into_iter()
            .map(|c| node_to_positioned(heap, c, src, b2c))
            .collect();
        let kids_vec = heap.alloc_vector(kids);
        pairs.push((kw("kids"), kids_vec));
    }
    heap.map_from_pairs(pairs)
}

/// Feature-off stub: the builtin is registered unconditionally (like `gui-*`), so
/// calling it without the parser built in gives a clear rebuild hint.
#[cfg(not(feature = "treesit"))]
pub fn parse(_heap: &mut crate::core::heap::Heap, _src: &str, lang: &str) -> LispResult {
    let _ = Value::nil();
    Err(LispError::runtime(format!(
        "tree-sitter-parse: :{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

#[cfg(not(feature = "treesit"))]
pub fn parse_incremental(
    _heap: &mut crate::core::heap::Heap,
    _key: i64,
    _src: &str,
    lang: &str,
) -> LispResult {
    Err(LispError::runtime(format!(
        "tree-sitter-reparse: :{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

/// Without `treesit` there's no cache, so nothing is ever forgotten.
#[cfg(not(feature = "treesit"))]
pub fn forget(_key: i64) -> i64 {
    0
}
