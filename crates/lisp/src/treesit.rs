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
    let tree = parse_cached(src, lang)?;
    let b2c = byte_to_char_offsets(src);
    Ok(node_to_positioned(heap, tree.root_node(), src, &b2c))
}

/// The last few parses, keyed by (language, source).
///
/// An editor asks the SAME text several questions in a row — fontify while redrawing,
/// then the enclosing chain when you press RET, then a sibling list when you press
/// C-M-f — and each one was re-parsing from scratch. Parsing 22 KB of Elixir is ~10 ms,
/// which is the whole cost of a structural motion once the projection is gone, so
/// repeated motion over an unchanged buffer went from paying it every time to paying it
/// once.
///
/// Small and exact: four entries, and a hit requires the source to be EQUAL, not merely
/// hashed the same. A miss is only ever a re-parse, so the cache can never be wrong —
/// but a false hit would hand back a tree for different text, which would be.
#[cfg(feature = "treesit")]
static TREE_CACHE: LazyLock<Mutex<Vec<(String, String, tree_sitter::Tree)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

#[cfg(feature = "treesit")]
const TREE_CACHE_CAP: usize = 4;

/// Parse `src` with `lang`, reusing the cached tree when the text is unchanged.
#[cfg(feature = "treesit")]
fn parse_cached(src: &str, lang: &str) -> Result<tree_sitter::Tree, LispError> {
    {
        let mut cache = TREE_CACHE.lock().expect("treesit tree cache");
        if let Some(i) = cache
            .iter()
            .position(|(l, s, _)| l == lang && s.as_str() == src)
        {
            // move to the back: most-recently-used, so the eviction below drops the
            // buffer nobody is working in
            let hit = cache.remove(i);
            let tree = hit.2.clone();
            cache.push(hit);
            return Ok(tree);
        }
    }
    let language = language_for(lang)?;
    let mut parser = checkout_parser(lang, &language)?;
    let tree = parser.parse(src, None);
    return_parser(lang, parser);
    let tree =
        tree.ok_or_else(|| LispError::runtime(format!("tree-sitter: {lang}: parse failed")))?;
    {
        let mut cache = TREE_CACHE.lock().expect("treesit tree cache");
        if cache.len() >= TREE_CACHE_CAP {
            cache.remove(0);
        }
        cache.push((lang.to_string(), src.to_string(), tree.clone()));
    }
    Ok(tree)
}

/// One node, WITHOUT its children — `{:kind :start :end :named :container}`.
///
/// `:container` replaces the presence of `:kids`, which is how the full projection says
/// "this node has children"; the point of these queries is not to build them.
#[cfg(feature = "treesit")]
fn node_shallow(heap: &mut Heap, node: tree_sitter::Node, b2c: &[u32]) -> Value {
    let kw = |k: &str| Value::keyword(value::intern(k));
    heap.map_from_pairs(vec![
        (kw("kind"), kw(node.kind())),
        (kw("start"), Value::int(b2c[node.start_byte()] as i64)),
        (kw("end"), Value::int(b2c[node.end_byte()] as i64)),
        (kw("named"), Value::boolean(node.is_named())),
        (kw("container"), Value::boolean(node.child_count() > 0)),
    ])
}

/// The byte offset for a char offset — the inverse of `byte_to_char_offsets`, by binary
/// search over it, so a caller's char position can address the byte-indexed tree.
#[cfg(feature = "treesit")]
fn char_to_byte(b2c: &[u32], ch: u32) -> usize {
    match b2c.binary_search(&ch) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

/// `(tree-sitter-chain source lang offset)` — the nodes containing char `offset`,
/// outermost first, each without its children.
///
/// **Why this exists rather than walking `tree-sitter-parse`'s result.** That call
/// projects the WHOLE tree into Brood maps: 9,561 of them for a 22 KB Elixir file. Every
/// consumer that wants "what encloses this point" — an indenter, `backward-up-list`,
/// a which-function display — then walks that to read about ten of them, and pays ~17 ms
/// to do it. The editor calls it on every RET, twice, and it was a quarter of a second of
/// every keypress. The tree already knows the answer in O(depth); this hands it over.
#[cfg(feature = "treesit")]
pub fn chain(heap: &mut Heap, src: &str, lang: &str, offset: i64) -> LispResult {
    let tree = parse_cached(src, lang)?;
    let b2c = byte_to_char_offsets(src);
    let byte = char_to_byte(&b2c, offset.max(0) as u32);
    let mut out: Vec<Value> = Vec::new();
    let mut node = tree.root_node();
    let broken = node.has_error();
    loop {
        // `:broken` on the outermost entry: does the tree contain an error anywhere?
        // O(1) from tree-sitter, and the one thing a caller cannot work out from a
        // chain — which only ever sees one path.
        let n = match (
            node.parent().is_none() && broken,
            node_shallow(heap, node, &b2c),
        ) {
            (true, Value::Map(id)) => heap.map_assoc(
                id,
                Value::keyword(value::intern("broken")),
                Value::boolean(true),
            ),
            (_, v) => v,
        };
        out.push(n);
        // The deepest child strictly containing the byte. `named_child` is not enough:
        // an anonymous token can be the one that contains it, and the chain of KINDS is
        // what a caller counts, so the walk follows every child.
        let mut cursor = node.walk();
        let next = node
            .children(&mut cursor)
            .find(|c| c.start_byte() <= byte && byte < c.end_byte());
        match next {
            Some(c) => node = c,
            None => break,
        }
    }
    Ok(heap.alloc_vector(out))
}

/// `(tree-sitter-kids source lang offset)` — the named children of the deepest node
/// STRICTLY containing char `offset`, each without its own children.
///
/// The sibling list every structural motion needs: `forward-sexp` is "the next one of
/// these starting at or after point", `down-list` is "the first child of the next one
/// that is a container". Same reason as [`chain`]: those motions were each projecting
/// the whole tree to read one list.
#[cfg(feature = "treesit")]
pub fn kids(heap: &mut Heap, src: &str, lang: &str, offset: i64) -> LispResult {
    let tree = parse_cached(src, lang)?;
    let b2c = byte_to_char_offsets(src);
    let byte = char_to_byte(&b2c, offset.max(0) as u32);
    // STRICTLY containing, so a point sitting exactly at a node's start belongs to that
    // node's parent — which is what makes `forward-sexp` step OVER the form at point
    // rather than into it.
    let mut node = tree.root_node();
    loop {
        let mut cursor = node.walk();
        let next = node
            .children(&mut cursor)
            .find(|c| c.child_count() > 0 && c.start_byte() < byte && byte < c.end_byte());
        match next {
            Some(c) => node = c,
            None => break,
        }
    }
    let mut cursor = node.walk();
    let out: Vec<tree_sitter::Node> = node.named_children(&mut cursor).collect();
    let vals: Vec<Value> = out
        .into_iter()
        .map(|c| node_shallow(heap, c, &b2c))
        .collect();
    Ok(heap.alloc_vector(vals))
}

/// Collect `[start end kind]` for the OUTERMOST nodes whose kind is wanted.
#[cfg(feature = "treesit")]
fn spans_into(
    heap: &mut Heap,
    node: tree_sitter::Node,
    src: &str,
    b2c: &[u32],
    wanted: &std::collections::HashSet<String>,
    keywords: bool,
    out: &mut Vec<Value>,
) {
    let kind = node.kind();
    // An anonymous token whose text starts with a letter is a language keyword — the
    // cross-language rule, so a table need only name the handful of NAMED nodes it
    // colours. Reported under its own kind so the caller maps it like any other.
    let is_keyword = keywords
        && !node.is_named()
        && src[node.start_byte()..node.end_byte()]
            .chars()
            .next()
            .is_some_and(char::is_alphabetic);
    if wanted.contains(kind) || is_keyword {
        let k = if wanted.contains(kind) {
            kind
        } else {
            "__keyword__"
        };
        let span = vec![
            Value::int(b2c[node.start_byte()] as i64),
            Value::int(b2c[node.end_byte()] as i64),
            Value::keyword(value::intern(k)),
        ];
        let v = heap.alloc_vector(span);
        out.push(v);
        return; // coloured whole: do not descend
    }
    let mut cursor = node.walk();
    let children: Vec<tree_sitter::Node> = node.children(&mut cursor).collect();
    for c in children {
        spans_into(heap, c, src, b2c, wanted, keywords, out);
    }
}

/// `(tree-sitter-spans source lang kinds keywords?)` — the fontify query: `[start end
/// kind]` for the outermost nodes whose kind is in `kinds`, in source order.
///
/// **Why the kernel takes the kind set.** Fontify is the one consumer that genuinely
/// walks the WHOLE tree, so it was the one paying the full projection — thousands of
/// Brood maps built per keystroke to read `:kind` off each and throw the rest away. The
/// set is data the caller supplies and the answer is spans, so which kinds get which
/// face — the actual policy — stays entirely in the editor; what moves here is only the
/// walk, which is where the allocation was.
///
/// `keywords?` adds the cross-language rule that an anonymous token beginning with a
/// letter is a keyword, reported as kind `:__keyword__`. It is a flag rather than a kind
/// because no grammar names it: keywords surface as anonymous tokens of every kind.
#[cfg(feature = "treesit")]
pub fn spans(
    heap: &mut Heap,
    src: &str,
    lang: &str,
    kinds: Vec<String>,
    keywords: bool,
) -> LispResult {
    let tree = parse_cached(src, lang)?;
    let b2c = byte_to_char_offsets(src);
    let wanted: std::collections::HashSet<String> = kinds.into_iter().collect();
    let mut out: Vec<Value> = Vec::new();
    spans_into(
        heap,
        tree.root_node(),
        src,
        &b2c,
        &wanted,
        keywords,
        &mut out,
    );
    Ok(heap.alloc_vector(out))
}

#[cfg(not(feature = "treesit"))]
pub fn spans(
    heap: &mut crate::core::heap::Heap,
    src: &str,
    lang: &str,
    _kinds: Vec<String>,
    _keywords: bool,
) -> LispResult {
    parse(heap, src, lang)
}

/// `(tree-sitter-load-grammar path lang)` — load a tree-sitter grammar from a
/// shared library at runtime and register it under the language keyword `lang`,
/// so `tree-sitter-parse` can use it like a built-in one. Returns the grammar's
/// ABI version.
///
/// **Why this rather than a search path.** The kernel does not know where an
/// application keeps its grammars, and should not: a path convention is policy.
/// This takes the library the caller chose, which leaves "look in
/// `~/.config/bedit/grammars`, named for the file's mode" entirely in the editor,
/// and leaves the kernel with the mechanism it can actually own — dlopen the
/// library, find `tree_sitter_<lang>`, check the ABI, remember it.
///
/// **The library is leaked, deliberately.** A `Language` is a pointer into the
/// loaded object's static tables, so unmapping the library would dangle every
/// tree ever parsed with it. Grammars are loaded once and used for the life of
/// the process; `std::mem::forget` says that in one line, where an `Arc` holding
/// the `Library` alongside every `Language` would say it in many and still never
/// drop.
///
/// Failure is an ordinary error at every step — a missing file, a library with no
/// such grammar, a grammar built for an incompatible tree-sitter. None of them is
/// a crash. What this cannot make safe is a *hostile* library: it is native code
/// and runs with the process's privileges, exactly like an Emacs dynamic module,
/// so an application should load only from a directory its user controls.
#[cfg(feature = "treesit")]
pub fn load_grammar(path: &str, lang: &str) -> LispResult {
    // Brood keywords are spelled with hyphens and C identifiers cannot be, so `:c-sharp`
    // asks for `tree_sitter_c_sharp` — which is exactly what the tree-sitter-c-sharp grammar
    // exports. The library's entry point is named for the GRAMMAR, so `lang` names the
    // grammar rather than being a nickname for it; there is no aliasing here and shouldn't
    // be, since the mode that asks to parse `:c-sharp` is the same name on both sides.
    let symbol = format!("tree_sitter_{}", lang.replace('-', "_"));
    // SAFETY: dlopen'ing a caller-chosen library and calling the grammar constructor it
    // exports. The symbol type is tree-sitter's documented grammar entry point, and the
    // Language it yields is validated against the host's ABI range below before any use.
    let language = unsafe {
        let lib = libloading::Library::new(path)
            .map_err(|e| LispError::runtime(format!("tree-sitter-load-grammar: {path}: {e}")))?;
        let entry: libloading::Symbol<unsafe extern "C" fn() -> *const ()> =
            lib.get(symbol.as_bytes()).map_err(|e| {
                LispError::runtime(format!(
                    "tree-sitter-load-grammar: {path} exports no {symbol}: {e}"
                ))
            })?;
        let f = *entry;
        std::mem::forget(lib); // the grammar's tables must stay mapped — see above
        tree_sitter::Language::new(tree_sitter_language::LanguageFn::from_raw(f))
    };
    // `set_language` is the ABI gate: it range-checks the grammar's version against what
    // this tree-sitter can drive, so a grammar from another era is a message, not a crash.
    tree_sitter::Parser::new()
        .set_language(&language)
        .map_err(|e| {
            LispError::runtime(format!(
                "tree-sitter-load-grammar: {path}: {lang} is not usable by this runtime \
                 (grammar ABI {}, this build drives {}..={}): {e}",
                language.abi_version(),
                tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
                tree_sitter::LANGUAGE_VERSION
            ))
        })?;
    let abi = language.abi_version() as i64;
    DYNAMIC
        .lock()
        .expect("treesit dynamic grammars")
        .insert(lang.to_string(), language);
    // A re-load replaces the entry, so a grammar can be swapped without restarting; the
    // parser pool is keyed by language name and holds parsers bound to the OLD language, so
    // it is cleared for this one rather than left to hand out stale parsers.
    PARSER_POOL
        .lock()
        .expect("treesit parser pool")
        .remove(lang);
    Ok(Value::int(abi))
}

/// Grammars loaded at runtime by [`load_grammar`], keyed by language name. Consulted
/// BEFORE the compile-time arms, so loading one deliberately overrides a bundled grammar of
/// the same name — the point of being able to load one at all is to move faster than the
/// runtime's release cycle.
#[cfg(feature = "treesit")]
static DYNAMIC: LazyLock<Mutex<HashMap<String, tree_sitter::Language>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "treesit")]
fn dynamic_language(lang: &str) -> Option<tree_sitter::Language> {
    DYNAMIC
        .lock()
        .expect("treesit dynamic grammars")
        .get(lang)
        .cloned()
}

#[cfg(not(feature = "treesit"))]
pub fn load_grammar(_path: &str, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "tree-sitter-load-grammar: :{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
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
    // A grammar loaded at runtime wins over a bundled one of the same name: loading one is a
    // deliberate act, and the reason to do it is usually that the bundled grammar is behind.
    if let Some(l) = dynamic_language(lang) {
        return Ok(l);
    }
    match lang {
        #[cfg(feature = "treesit-ruby")]
        "ruby" => Ok(tree_sitter_ruby::LANGUAGE.into()),
        #[cfg(feature = "treesit-elixir")]
        "elixir" => Ok(tree_sitter_elixir::LANGUAGE.into()),
        other => Err(LispError::runtime(format!(
            "tree-sitter-parse: no grammar for :{other} — load one at runtime with \
             (tree-sitter-load-grammar \"/path/to/libtree-sitter-{other}.so\" :{other}), \
             or build it in with --features treesit-{other}"
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
    // `:broken` on the ROOT: does this tree contain an error ANYWHERE? tree-sitter
    // keeps that as a bit on every node, so asking is O(1) — where deciding it in
    // Brood means walking the whole projected tree, which is what the indenter was
    // doing on every keypress (two trees, thousands of nodes, ~250 ms on a 22 KB
    // Elixir file). Only the root carries it: a caller wanting per-node state has
    // `:error`/`:missing` already, and putting a redundant flag on every node would
    // grow every tree to save one lookup.
    if node.parent().is_none() && node.has_error() {
        pairs.push((kw("broken"), Value::boolean(true)));
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
pub fn chain(heap: &mut crate::core::heap::Heap, src: &str, lang: &str, _o: i64) -> LispResult {
    parse(heap, src, lang)
}

#[cfg(not(feature = "treesit"))]
pub fn kids(heap: &mut crate::core::heap::Heap, src: &str, lang: &str, _o: i64) -> LispResult {
    parse(heap, src, lang)
}
