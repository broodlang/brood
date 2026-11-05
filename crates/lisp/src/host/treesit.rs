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
//! CHARACTER offsets (tree-sitter counts s; we project them, exactly as
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

#[cfg(feature = "treesit")]
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
/// C-M-f — and each one was re-parsing from scratch. Parsing 22 KB of source is ~10 ms,
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
const TREE_CACHE_CAP: usize = 8;

/// The edit that turns `old` into `new`, derived from their common prefix and suffix.
///
/// tree-sitter reparses INCREMENTALLY, but only if it is told what changed — and the
/// editor never tells us: it hands over a whole buffer, because that is what a pure
/// `text -> spans` service is. The change is recoverable anyway. Bytes before the first
/// difference and after the last are identical, so the edit is exactly "the span between
/// them was replaced". That is precise for any single contiguous change — a keystroke, a
/// paste, a kill — and still CORRECT for a scattered one, merely wider than necessary,
/// since a larger replaced range only makes tree-sitter reparse more.
///
/// `None` when the texts are equal, or when the derived edit spans most of a large
/// document, where reparsing from the old tree buys nothing over a fresh parse.
#[cfg(feature = "treesit")]
fn derive_edit(old: &str, new: &str) -> Option<tree_sitter::InputEdit> {
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    let mut start = 0usize;
    let max_pre = ob.len().min(nb.len());
    while start < max_pre && ob[start] == nb[start] {
        start += 1;
    }
    let mut back = 0usize;
    while back < max_pre - start && ob[ob.len() - 1 - back] == nb[nb.len() - 1 - back] {
        back += 1;
    }
    let old_end = ob.len() - back;
    let new_end = nb.len() - back;
    if start == old_end && start == new_end {
        return None; // identical
    }
    if (old_end - start) * 4 > ob.len() && ob.len() > 4096 {
        return None; // a change spanning most of it is a new document, not an edit
    }
    // Byte offsets only. The row/column `Point`s matter to a client that renders from
    // the tree's positions; the parse itself is byte-addressed, and zeros are consistent.
    let zero = tree_sitter::Point::new(0, 0);
    Some(tree_sitter::InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: zero,
        old_end_position: zero,
        new_end_position: zero,
    })
}

/// Parse `src` with `lang`, reusing the cached tree when the text is unchanged and
/// reparsing incrementally from it when the text merely changed.
#[cfg(feature = "treesit")]
fn parse_cached(src: &str, lang: &str) -> Result<tree_sitter::Tree, LispError> {
    // The grammar first, before the cache is consulted: a declared grammar that has not
    // loaded yet is a (re)load waiting for this call, and the load clears the trees the
    // cache holds for the language — an exact hit taken before it would be the old grammar's.
    let language = language_for(lang)?;
    // An exact hit is free; a NEAR hit — the same buffer one keystroke later — is the
    // common case while typing, and is what decides how a large file feels. A full parse
    // of 250 KB of source is ~500 ms, so paying it per edit is the whole of "the editor
    // is sluggish"; reparsing from the previous tree is ~1 ms.
    let reuse: Option<(usize, tree_sitter::Tree)> = {
        let mut cache = crate::core::sync::lock(&TREE_CACHE);
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
        // No exact match: the entry this text is an EDIT of — the same document one
        // keystroke earlier. Searched over every entry of the language, most recent first,
        // not just the latest one: an editor parses the same document in more than one
        // shape (the view colours the visible window, the indenter asks about the whole
        // file), and with only the latest entry consulted each shape evicted the other,
        // so every keystroke was a full reparse (18 ms a RET in a 1,300-line source file).
        // The CLOSEST such entry, not the first: a small text (the view's window) is "an edit"
        // of anything by `derive_edit`'s measure, so taking the first match let a whole-document
        // parse claim — and then replace — the window's entry. The edit touching the fewest
        // bytes is the document's own previous version.
        cache
            .iter()
            .enumerate()
            .filter(|(_, (l, _, _))| l == lang)
            .filter_map(|(i, (_, old_src, old_tree))| {
                derive_edit(old_src, src).map(|edit| (i, old_tree, edit))
            })
            .min_by_key(|(_, _, edit)| {
                (edit.old_end_byte - edit.start_byte) + (edit.new_end_byte - edit.start_byte)
            })
            .map(|(i, old_tree, edit)| {
                let mut t = old_tree.clone();
                t.edit(&edit);
                (i, t)
            })
    };
    let mut parser = checkout_parser(lang, &language)?;
    let bytes = src.as_bytes();
    let tree = parser.parse_with_options(
        &mut |byte: usize, _point: tree_sitter::Point| -> &[u8] {
            if byte >= bytes.len() {
                &[]
            } else {
                &bytes[byte..]
            }
        },
        reuse.as_ref().map(|(_, t)| t),
        Some(tree_sitter::ParseOptions::new().progress_callback(&mut interruptible)),
    );
    return_parser(lang, parser);
    let tree = tree.ok_or_else(|| parse_failed(lang))?;
    {
        let mut cache = crate::core::sync::lock(&TREE_CACHE);
        // The entry this was an edit of is the same document's older version: it can never
        // be asked for again, so the new one takes its place — one entry per document (and
        // per window of one), not a generation per keystroke. Another thread may have reshuffled
        // the cache since, so the slot can hold another document now: dropping it costs at
        // most a reparse later, never a wrong tree — a hit always needs the EXACT text.
        let replaced = reuse.and_then(|(i, _)| {
            cache
                .get(i)
                .map(|(l, _, _)| l == lang)
                .filter(|same| *same)
                .map(|_| i)
        });
        if let Some(i) = replaced {
            cache.remove(i);
        } else if cache.len() >= TREE_CACHE_CAP {
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
fn node_shallow<T: SpanText>(heap: &mut Heap, node: tree_sitter::Node, text: &T) -> Value {
    let kw = |k: &str| Value::keyword(value::intern(k));
    heap.map_from_pairs(vec![
        (kw("kind"), kw(node.kind())),
        (
            kw("start"),
            Value::int(text.char_of(node.start_byte()) as i64),
        ),
        (kw("end"), Value::int(text.char_of(node.end_byte()) as i64)),
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
/// projects the WHOLE tree into Brood maps: 9,561 of them for a 22 KB source file. Every
/// consumer that wants "what encloses this point" — an indenter, `backward-up-list`,
/// a which-function display — then walks that to read about ten of them, and pays ~17 ms
/// to do it. The editor calls it on every RET, twice, and it was a quarter of a second of
/// every keypress. The tree already knows the answer in O(depth); this hands it over.
#[cfg(feature = "treesit")]
pub fn chain(heap: &mut Heap, src: &str, lang: &str, offset: i64) -> LispResult {
    let tree = parse_cached(src, lang)?;
    let b2c = byte_to_char_offsets(src);
    Ok(chain_in(heap, &tree, &FlatText { src, b2c: &b2c }, offset))
}

/// The chain walk itself, over any text the tree was parsed from — a flat string (the
/// text-keyed [`chain`]) or a held tree's rope ([`tree_chain`]).
#[cfg(feature = "treesit")]
fn chain_in<T: SpanText>(
    heap: &mut Heap,
    tree: &tree_sitter::Tree,
    text: &T,
    offset: i64,
) -> Value {
    let byte = text.byte_of(offset.max(0) as usize);
    let mut out: Vec<Value> = Vec::new();
    let mut node = tree.root_node();
    let broken = node.has_error();
    loop {
        // `:broken` on the outermost entry: does the tree contain an error anywhere?
        // O(1) from tree-sitter, and the one thing a caller cannot work out from a
        // chain — which only ever sees one path.
        let n = match (
            node.parent().is_none() && broken,
            node_shallow(heap, node, text),
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
    heap.alloc_vector(out)
}

/// `(tree-sitter-kids source lang offset)` — the named children of the deepest node
/// STRICTLY containing char `offset`, each without its own children.
///
/// The sibling list every structural motion needs: `forward-sexp` is "the next one of
/// these starting at or after point", `down-list` is "the first child of the next one
/// that is a container". Same reason as [`chain`]: those motions were each projecting
/// the whole tree to read one list.
#[cfg(feature = "treesit")]
pub fn kids(heap: &mut Heap, src: &str, lang: &str, offset: i64, all: bool) -> LispResult {
    let tree = parse_cached(src, lang)?;
    let b2c = byte_to_char_offsets(src);
    Ok(kids_in(
        heap,
        &tree,
        &FlatText { src, b2c: &b2c },
        offset,
        all,
    ))
}

/// The kids walk itself, over a flat string ([`kids`]) or a held tree's rope
/// ([`tree_kids`]).
///
/// `all` adds the ANONYMOUS children — the parent's own tokens (`{`, `(`, `do`, `end`), each
/// with `:named false` and the token's text as its kind — in source order among the named
/// ones. A bracket matcher reads a delimiter pair in the middle of its parent from that
/// list (an HCL block's braces after its labels) instead of scanning the text between the
/// named children.
#[cfg(feature = "treesit")]
fn kids_in<T: SpanText>(
    heap: &mut Heap,
    tree: &tree_sitter::Tree,
    text: &T,
    offset: i64,
    all: bool,
) -> Value {
    let byte = text.byte_of(offset.max(0) as usize);
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
    let out: Vec<tree_sitter::Node> = if all {
        node.children(&mut cursor).collect()
    } else {
        node.named_children(&mut cursor).collect()
    };
    let vals: Vec<Value> = out
        .into_iter()
        .map(|c| node_shallow(heap, c, text))
        .collect();
    heap.alloc_vector(vals)
}

/// The most error nodes one call reports. A buffer someone is halfway through
/// rewriting can hold hundreds; a caller only ever reads the first few, and the
/// point of this query is to be cheap enough to run on a keystroke.
#[cfg(feature = "treesit")]
const ERROR_CAP: usize = 32;

/// The most source characters an error node carries back as `:text` — enough to name
/// what is unclosed (`do`, `fn`, `"`) without putting a runaway region on the wire.
#[cfg(feature = "treesit")]
const ERROR_TEXT_CAP: usize = 40;

/// Collect the ERROR and MISSING nodes under `node`, outermost first.
#[cfg(feature = "treesit")]
fn errors_into(
    heap: &mut Heap,
    node: tree_sitter::Node,
    src: &str,
    b2c: &[u32],
    out: &mut Vec<Value>,
) {
    if out.len() >= ERROR_CAP {
        return;
    }
    if node.is_error() || node.is_missing() {
        let kw = |k: &str| Value::keyword(value::intern(k));
        // A MISSING node is zero-width and its kind IS the token the grammar wanted,
        // so its text is empty and the kind carries the meaning. An ERROR node spans
        // the text the parser could not place, and there the text is the meaning.
        let text: String = src[node.start_byte()..node.end_byte()]
            .chars()
            .take(ERROR_TEXT_CAP)
            .collect();
        let s = heap.alloc_string(&text);
        let v = heap.map_from_pairs(vec![
            (kw("kind"), kw(node.kind())),
            (kw("start"), Value::int(b2c[node.start_byte()] as i64)),
            (kw("end"), Value::int(b2c[node.end_byte()] as i64)),
            (kw("missing?"), Value::boolean(node.is_missing())),
            (kw("text"), s),
        ]);
        out.push(v);
        // Do not descend into an ERROR: its children are the tokens the parser
        // salvaged out of the unparsable region, and reporting them would bury the
        // one fact the caller wants — where the parse came apart — under its debris.
        return;
    }
    // Only subtrees that contain an error can contain an error node, and `has_error`
    // is O(1) from the tree. That is what keeps this a walk of the broken part rather
    // than of the buffer.
    let mut cursor = node.walk();
    let children: Vec<tree_sitter::Node> = node.children(&mut cursor).collect();
    for c in children {
        if c.has_error() || c.is_missing() {
            errors_into(heap, c, src, b2c, out);
        }
    }
}

/// `(tree-sitter-errors source lang)` — where the parse came apart: the ERROR and
/// MISSING nodes as `{:kind :start :end :missing? :text}`, outermost first.
///
/// **Why the kernel has to answer this.** `chain` already reports `:broken` on the root,
/// which says only *that* the tree has an error somewhere. That is not enough to tell a
/// person typing from a person who is wrong, and the difference is the whole contract of
/// a live evaluator: a form you are still writing must not be shown as a mistake. The
/// tree knows — a MISSING node is a token the grammar expected and inserted for you, an
/// ERROR node is text it could not place, and where each one SITS says the rest (an
/// error running to the end of the buffer is an unclosed construct; one with good code
/// after it is a real mistake). None of that is reachable from Brood today, and building
/// it out of `%tree-sitter-parse` means projecting every node in the file to find the
/// handful that are broken.
///
/// An empty vector means the parse is clean, which is the common answer and costs one
/// `has_error` on the root.
#[cfg(feature = "treesit")]
pub fn errors(heap: &mut Heap, src: &str, lang: &str) -> LispResult {
    let tree = parse_cached(src, lang)?;
    let root = tree.root_node();
    if !root.has_error() && !root.is_missing() {
        return Ok(heap.alloc_vector(Vec::new()));
    }
    let b2c = byte_to_char_offsets(src);
    let mut out: Vec<Value> = Vec::new();
    errors_into(heap, root, src, &b2c, &mut out);
    Ok(heap.alloc_vector(out))
}

/// The text a span walk reads positions and keyword letters from: a flat string with its
/// byte→char table (the slice-based `spans`), or a rope (a tree handle's text). The walk
/// itself is the same either way, so it is written once over this.
#[cfg(feature = "treesit")]
trait SpanText {
    /// The char offset of byte `byte`.
    fn char_of(&self, byte: usize) -> usize;
    /// The byte offset of char `ch`, clamped to the text.
    fn byte_of(&self, ch: usize) -> usize;
    /// Does the text in `[start, end)` begin with a letter? (the keyword rule)
    fn starts_alphabetic(&self, start: usize, end: usize) -> bool;
    /// The text in bytes `[start, end)` — borrowed whenever it lies in one piece, which is
    /// what a selector's `:text` compares against (ADR-487).
    fn text_in(&self, start: usize, end: usize) -> std::borrow::Cow<'_, str>;
    /// The text's length in bytes.
    fn byte_length(&self) -> usize;
    /// The bytes from `byte` to the end of the piece holding it — what a parse callback
    /// hands tree-sitter (an injected parse reads the outer text in place, ADR-489).
    fn bytes_from(&self, byte: usize) -> &[u8];
    /// The tree-sitter points (line, byte column) of `bytes`, which ascend.
    fn points_of(&self, bytes: &[usize]) -> Vec<tree_sitter::Point>;
}

#[cfg(feature = "treesit")]
struct FlatText<'a> {
    src: &'a str,
    b2c: &'a [u32],
}

#[cfg(feature = "treesit")]
impl SpanText for FlatText<'_> {
    fn char_of(&self, byte: usize) -> usize {
        self.b2c[byte] as usize
    }
    fn byte_of(&self, ch: usize) -> usize {
        char_to_byte(self.b2c, u32::try_from(ch).unwrap_or(u32::MAX))
    }
    fn starts_alphabetic(&self, start: usize, end: usize) -> bool {
        self.src[start..end]
            .chars()
            .next()
            .is_some_and(char::is_alphabetic)
    }
    fn text_in(&self, start: usize, end: usize) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(&self.src[start..end])
    }
    fn byte_length(&self) -> usize {
        self.src.len()
    }
    fn bytes_from(&self, byte: usize) -> &[u8] {
        &self.src.as_bytes()[byte.min(self.src.len())..]
    }
    fn points_of(&self, bytes: &[usize]) -> Vec<tree_sitter::Point> {
        // one forward pass over the text, since the bytes ascend
        let source = self.src.as_bytes();
        let (mut row, mut line_start, mut scanned) = (0usize, 0usize, 0usize);
        bytes
            .iter()
            .map(|byte| {
                let byte = (*byte).min(source.len());
                for (offset, character) in source[scanned.min(byte)..byte].iter().enumerate() {
                    if *character == b'\n' {
                        row += 1;
                        line_start = scanned + offset + 1;
                    }
                }
                scanned = scanned.max(byte);
                tree_sitter::Point::new(row, byte - line_start)
            })
            .collect()
    }
}

#[cfg(feature = "treesit")]
impl SpanText for ropey::Rope {
    fn char_of(&self, byte: usize) -> usize {
        self.byte_to_char(byte.min(self.len_bytes()))
    }
    fn byte_of(&self, ch: usize) -> usize {
        self.char_to_byte(ch.min(self.len_chars()))
    }
    fn starts_alphabetic(&self, start: usize, end: usize) -> bool {
        start < end
            && start < self.len_bytes()
            && self.char(self.byte_to_char(start)).is_alphabetic()
    }
    fn text_in(&self, start: usize, end: usize) -> std::borrow::Cow<'_, str> {
        let slice = self.byte_slice(start.min(self.len_bytes())..end.min(self.len_bytes()));
        match slice.as_str() {
            Some(piece) => std::borrow::Cow::Borrowed(piece),
            None => std::borrow::Cow::Owned(slice.to_string()),
        }
    }
    fn byte_length(&self) -> usize {
        self.len_bytes()
    }
    fn bytes_from(&self, byte: usize) -> &[u8] {
        if byte >= self.len_bytes() {
            return &[];
        }
        let (chunk, chunk_start, _, _) = self.chunk_at_byte(byte);
        &chunk.as_bytes()[byte - chunk_start..]
    }
    fn points_of(&self, bytes: &[usize]) -> Vec<tree_sitter::Point> {
        bytes.iter().map(|byte| point_of(self, *byte)).collect()
    }
}

/// How many selector MAPS one face-table key may nest through `:parent` — the key itself, its
/// parent's map and its grandparent's (ADR-487). Three reaches a method call's receiver
/// (`identifier` in `field_expression` in `call_expression`) with room to spare, and a bound
/// keeps a selector's specificity a fixed-size value.
pub const MAX_SELECTOR_DEPTH: usize = 3;

/// The node SOURCE TEXT a selector requires (ADR-487): one of these strings, exactly. Sorted,
/// for a binary search, with the shortest and longest lengths kept so most nodes are refused
/// on their byte length without reading a byte of text.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub struct TextMatch {
    texts: Vec<String>,
    shortest: usize,
    longest: usize,
}

#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
impl TextMatch {
    /// The match for any of `texts` (at least one).
    pub fn new(mut texts: Vec<String>) -> TextMatch {
        texts.sort();
        texts.dedup();
        let shortest = texts.iter().map(String::len).min().unwrap_or(0);
        let longest = texts.iter().map(String::len).max().unwrap_or(0);
        TextMatch {
            texts,
            shortest,
            longest,
        }
    }

    #[cfg(feature = "treesit")]
    fn holds<T: SpanText>(&self, node: tree_sitter::Node, text: &T) -> bool {
        let (start, end) = (node.start_byte(), node.end_byte());
        let length = end - start;
        length >= self.shortest
            && length <= self.longest
            && self
                .texts
                .binary_search_by(|candidate| candidate.as_str().cmp(&*text.text_in(start, end)))
                .is_ok()
    }
}

/// What a face-table key asks of a node (ADR-486, ADR-487): its own KIND, and optionally its
/// CONTEXT — the FIELD it occupies in its parent (tree-sitter's field names: `function`,
/// `name`, `body`), its position among the parent's NAMED children, its exact source TEXT,
/// and a selector its PARENT must satisfy in turn (a plain `:parent :call` is the parent
/// selector with a kind alone). Every constraint present must hold; an absent one is free.
#[derive(Clone, Debug, Default)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub struct Selector {
    pub kind: Option<String>,
    pub parent: Option<Box<Selector>>,
    pub field: Option<String>,
    pub nth: Option<usize>,
    pub text: Option<TextMatch>,
}

#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
impl Selector {
    /// The selector naming a kind alone — a table keyword `:string`, or `:parent :call`.
    pub fn of_kind(kind: String) -> Selector {
        Selector {
            kind: Some(kind),
            ..Selector::default()
        }
    }

    /// Does it constrain anything? An empty selector would colour every node.
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.parent.is_none()
            && self.field.is_none()
            && self.nth.is_none()
            && self.text.is_none()
    }

    /// How many selector levels deep it reaches: 1 for a map with no parent selector.
    pub fn depth(&self) -> usize {
        1 + self.parent.as_ref().map_or(0, |parent| parent.depth())
    }

    /// The selector's own constraints as distinct powers of two — text 16, field 8, parent
    /// 4, nth 2, kind 1 — so two different constraint sets at one level never tie.
    fn level_weight(&self) -> u8 {
        u8::from(self.kind.is_some())
            | (u8::from(self.nth.is_some()) << 1)
            | (u8::from(self.parent.is_some()) << 2)
            | (u8::from(self.field.is_some()) << 3)
            | (u8::from(self.text.is_some()) << 4)
    }

    /// The level weights from the node outward: the node's own constraints, then its parent
    /// selector's, then the grandparent's. Compared lexicographically, so the node's own
    /// constraints decide first and an ancestor's only break a tie.
    fn specificity(&self) -> [u8; MAX_SELECTOR_DEPTH + 1] {
        let mut weights = [0; MAX_SELECTOR_DEPTH + 1];
        let mut level = Some(self);
        for weight in &mut weights {
            let Some(selector) = level else { break };
            *weight = selector.level_weight();
            level = selector.parent.as_deref();
        }
        weights
    }

    /// How many strings its `:text` sets accept, at every level — the narrower wins a tie.
    fn breadth(&self) -> usize {
        self.text.as_ref().map_or(0, |text| text.texts.len())
            + self.parent.as_ref().map_or(0, |parent| parent.breadth())
    }

    /// Does `node`, sitting at `context`, satisfy the selector? The cheap checks first — a
    /// kind, field and position are a comparison each, a parent selector recurses up the
    /// context chain — and the source text, the only one that reads the document, last.
    #[cfg(feature = "treesit")]
    fn holds<T: SpanText>(&self, node: tree_sitter::Node, context: &NodeContext, text: &T) -> bool {
        self.kind.as_deref().is_none_or(|kind| kind == node.kind())
            && (self.field.is_none() || self.field.as_deref() == context.field)
            && (self.nth.is_none() || self.nth == context.nth)
            && match (&self.parent, context.parent) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(parent), Some(parent_node)) => parent.holds(
                    parent_node,
                    context.outer.unwrap_or(&NodeContext::ROOT),
                    text,
                ),
            }
            && self
                .text
                .as_ref()
                .is_none_or(|wanted| wanted.holds(node, text))
    }
}

/// One entry of a face table, as the span walk matches it: its selector, and `reported`,
/// what a span for it carries back — the table's value (a face name), copied through
/// untouched, so which face it means stays the caller's policy.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub struct SpanRule {
    pub selector: Selector,
    pub reported: crate::core::value::Value,
}

/// Where a rule stands when several match one node — the greater wins. Specificity first (the
/// node's own constraints, then each ancestor selector's), then the narrower `:text` sets,
/// then, for two entries that differ only in which strings they accept, the face name that
/// sorts first. Computed once per table, so the winner never depends on the map's (hash)
/// order and the walk takes the first rule that holds.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
struct RuleRank {
    specificity: [u8; MAX_SELECTOR_DEPTH + 1],
    narrowness: std::cmp::Reverse<usize>,
    name: std::cmp::Reverse<String>,
}

#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
impl SpanRule {
    fn rank(&self) -> RuleRank {
        let name = match self.reported {
            crate::core::value::Value::Keyword(symbol) => crate::core::value::symbol_name(symbol),
            _ => String::new(),
        };
        RuleRank {
            specificity: self.selector.specificity(),
            narrowness: std::cmp::Reverse(self.selector.breadth()),
            name: std::cmp::Reverse(name),
        }
    }
}

/// A face table compiled for the span walk: the rules indexed by kind, and the few that name
/// no kind (`{:field :function}` — whatever fills that field), each list most specific
/// first. One hash lookup per node, then the first rule of that kind's handful that holds; a
/// table of plain kinds is exactly the kind set the walk always took. Each rule carries its
/// place in the whole table's order (0 the most specific), which is how a kinded and an
/// unkinded rule that both hold are compared without re-ranking them per node.
#[derive(Default, Debug)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub struct SpanTable {
    by_kind: std::collections::HashMap<String, Vec<(usize, SpanRule)>>,
    any_kind: Vec<(usize, SpanRule)>,
}

#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
impl SpanTable {
    pub fn new(mut rules: Vec<SpanRule>) -> SpanTable {
        rules.sort_by_cached_key(|rule| std::cmp::Reverse(rule.rank()));
        let mut table = SpanTable::default();
        for (place, rule) in rules.into_iter().enumerate() {
            match rule.selector.kind.clone() {
                Some(kind) => table.by_kind.entry(kind).or_default().push((place, rule)),
                None => table.any_kind.push((place, rule)),
            }
        }
        table
    }

    /// A table of plain kinds, each reporting its own kind keyword — the shape the walk took
    /// before tables could name a context.
    pub fn of_kinds(kinds: Vec<String>) -> SpanTable {
        SpanTable::new(
            kinds
                .into_iter()
                .map(|kind| SpanRule {
                    reported: crate::core::value::Value::keyword(crate::core::value::intern(&kind)),
                    selector: Selector::of_kind(kind),
                })
                .collect(),
        )
    }

    /// The most specific rule that colours `node` in `context`, or None: the first that holds
    /// in each of the two lists, the better ranked of those two.
    #[cfg(feature = "treesit")]
    fn rule_for<T: SpanText>(
        &self,
        node: tree_sitter::Node,
        context: &NodeContext,
        text: &T,
    ) -> Option<&SpanRule> {
        fn first_holding<'rules, T: SpanText>(
            rules: &'rules [(usize, SpanRule)],
            node: tree_sitter::Node,
            context: &NodeContext,
            text: &T,
        ) -> Option<&'rules (usize, SpanRule)> {
            rules
                .iter()
                .find(|(_, rule)| rule.selector.holds(node, context, text))
        }
        let kinded = self
            .by_kind
            .get(node.kind())
            .and_then(|rules| first_holding(rules, node, context, text));
        let best = if self.any_kind.is_empty() {
            kinded
        } else {
            match (kinded, first_holding(&self.any_kind, node, context, text)) {
                (Some(kinded), Some(unkinded)) => Some(if unkinded.0 < kinded.0 {
                    unkinded
                } else {
                    kinded
                }),
                (kinded, unkinded) => kinded.or(unkinded),
            }
        };
        best.map(|(_, rule)| rule)
    }
}

/// Where a node sits, as the walk reaches it: its parent node, the field it fills there (None
/// when the grammar names none), its index among the parent's named children (None for an
/// anonymous token, which has no place in that count), and the PARENT's own context — the
/// chain a nested `:parent` selector climbs (ADR-487). Read off the cursor that is already
/// walking the parent's children, and kept on the walk's own stack, so it costs nothing a
/// plain-kind table did not.
#[cfg(feature = "treesit")]
#[derive(Clone, Copy)]
struct NodeContext<'a> {
    parent: Option<tree_sitter::Node<'a>>,
    field: Option<&'a str>,
    nth: Option<usize>,
    outer: Option<&'a NodeContext<'a>>,
}

#[cfg(feature = "treesit")]
impl NodeContext<'_> {
    /// The root's context: no parent, so no parent selector holds there.
    const ROOT: NodeContext<'static> = NodeContext {
        parent: None,
        field: None,
        nth: None,
        outer: None,
    };
}

/// What one span walk is asked for: the face table, the keyword flag, and the byte window
/// `[from, to)` to report — `None` for the whole tree, unclipped.
#[cfg(feature = "treesit")]
struct SpanQuery<'a> {
    table: &'a SpanTable,
    keywords: bool,
    window: Option<(usize, usize)>,
}

#[cfg(feature = "treesit")]
impl SpanQuery<'_> {
    /// Does `node` reach into the window? Zero-width nodes count when they sit inside it.
    fn touches(&self, node: &tree_sitter::Node) -> bool {
        match self.window {
            None => true,
            Some((from, to)) => {
                let (start, end) = (node.start_byte(), node.end_byte());
                (start < to && end > from) || (start == end && from <= start && start < to)
            }
        }
    }
}

/// Collect `(start end reported)` BYTE spans for the OUTERMOST nodes the table colours, in
/// source order, descending only into children that reach the query's window. A span is
/// clipped to the window, so a string opened above it reports only its visible part.
/// `context` is where `node` sits in its parent — what a table's context entries match
/// against. Bytes, not chars, so injected spans (ADR-489) can be spliced in before the one
/// conversion [`raw_spans_value`] makes.
#[cfg(feature = "treesit")]
fn spans_into<T: SpanText>(
    node: tree_sitter::Node,
    context: &NodeContext,
    text: &T,
    query: &SpanQuery,
    out: &mut Vec<RawSpan>,
) {
    // An anonymous token whose text starts with a letter is a language keyword — the
    // cross-language rule, so a table need only name the handful of NAMED nodes it
    // colours. Reported as `:__keyword__` so the caller maps it like any other.
    let reported = query
        .table
        .rule_for(node, context, text)
        .map(|rule| rule.reported)
        .or_else(|| {
            (query.keywords
                && !node.is_named()
                && text.starts_alphabetic(node.start_byte(), node.end_byte()))
            .then(|| Value::keyword(value::intern("__keyword__")))
        });
    if let Some(reported) = reported {
        let (start, end) = match query.window {
            None => (node.start_byte(), node.end_byte()),
            Some((from, to)) => (
                node.start_byte().max(from),
                node.end_byte().min(to).max(node.start_byte().max(from)),
            ),
        };
        out.push((start, end, reported));
        return; // coloured whole: do not descend
    }
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    let mut named_index = 0;
    loop {
        let child = cursor.node();
        if let Some((_, to)) = query.window
            && child.start_byte() >= to
        {
            break; // children are in source order: nothing after this reaches the window
        }
        let child_context = NodeContext {
            parent: Some(node),
            field: cursor.field_name(),
            nth: child.is_named().then_some(named_index),
            outer: Some(context),
        };
        if child.is_named() {
            named_index += 1;
        }
        if query.touches(&child) {
            spans_into(child, &child_context, text, query, out);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// A span as the walk collects it: BYTE offsets `[start, end)` and what it reports.
#[cfg(feature = "treesit")]
type RawSpan = (usize, usize, Value);

/// Raw byte spans as the `[start end reported]` vector a span query answers, in char offsets.
#[cfg(feature = "treesit")]
fn raw_spans_value<T: SpanText>(heap: &mut Heap, text: &T, raw: Vec<RawSpan>) -> Value {
    let mut out: Vec<Value> = Vec::with_capacity(raw.len());
    for (start, end, reported) in raw {
        let span = vec![
            Value::int(text.char_of(start) as i64),
            Value::int(text.char_of(end) as i64),
            reported,
        ];
        out.push(heap.alloc_vector(span));
    }
    heap.alloc_vector(out)
}

/// The host walk of one tree, then — when the caller declared injections — the embedded
/// documents' spans spliced over it (ADR-489). Shared by the text and held-tree paths, so
/// the two answer alike.
#[cfg(feature = "treesit")]
fn host_and_injected_spans<T: SpanText>(
    tree: &tree_sitter::Tree,
    text: &T,
    query: &SpanQuery,
    injections: Option<&Injections>,
) -> Vec<RawSpan> {
    let mut host: Vec<RawSpan> = Vec::new();
    let root = tree.root_node();
    spans_into(root, &NodeContext::ROOT, text, query, &mut host);
    match injections {
        Some(injections) if !injections.rules.is_empty() => {
            splice_injections(root, text, query, injections, host)
        }
        _ => host,
    }
}

/// `(tree-sitter-spans source lang table keywords? injections)` — the fontify query: `[start
/// end reported]` for the outermost nodes `table` colours, in source order.
///
/// **Why a table and not only a kind set (ADR-486).** A kind alone cannot tell apart the
/// names a grammar gives one kind: in HCL a block's type, an attribute's name and a called
/// function are all `identifier`, and so is the function in a Rust or Go call. What tells
/// them apart is where the node sits — its parent's kind, the field it fills, its place
/// among the parent's named children — and the walk is already standing on the parent's
/// cursor when it reaches the node, so matching on that context is free here and would be
/// a Brood callback per node anywhere else.
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
///
/// `injections` (ADR-489) names the nodes whose text is another language's document; their
/// spans replace the host's there.
#[cfg(feature = "treesit")]
pub fn spans(
    heap: &mut Heap,
    src: &str,
    lang: &str,
    table: &SpanTable,
    keywords: bool,
    injections: Option<&Injections>,
) -> LispResult {
    let tree = parse_cached(src, lang)?;
    let b2c = byte_to_char_offsets(src);
    let query = SpanQuery {
        table,
        keywords,
        window: None,
    };
    let text = FlatText { src, b2c: &b2c };
    let raw = host_and_injected_spans(&tree, &text, &query, injections);
    Ok(raw_spans_value(heap, &text, raw))
}

// ---- injections: another language's document inside this one (ADR-489) ---------------
//
// A Markdown fence's body is Rust, an Elixir `~H` sigil's is HEEx, a Terraform heredoc's is
// shell. tree-sitter parses such a document IN PLACE: a parser given `included_ranges` reads
// only those byte ranges of the text, as one document, and its nodes carry the outer text's
// offsets. So the pieces of one embedded document (a heredoc split by `${…}` interpolations)
// parse together, and nothing is copied or re-based.
//
// Two layers. [`range_spans`] colours given ranges with a given language — what a host with
// no tree (a line lexer) calls. [`splice_injections`] is the declarative half for a
// tree-sitter host: the host's injection RULES say, by selector, which nodes hold another
// language's text; the walk finds them, each parent's matching children are one document, and
// that document's spans replace the host's over its pieces.
//
// An injected document is parsed per call, over what the call asked for. No injected tree is
// held: a held host tree answers a frame's window, and the documents reaching into it are
// parsed again for that frame. The caps below bound what one call can parse.

/// The most bytes one embedded document is parsed whole. A longer one is cut to the query's
/// window (losing what lies above it, as a windowed fontify without a held tree does), and
/// without a window it is left in the host's colours.
#[cfg(feature = "treesit")]
pub const INJECTION_DOCUMENT_BYTES: usize = 32 * 1024;

/// The most bytes of embedded documents one call parses; documents past it keep the host's
/// colours. What bounds a file of a thousand fences to the cost of a few.
#[cfg(feature = "treesit")]
pub const INJECTION_CALL_BYTES: usize = 128 * 1024;

/// The most embedded documents one call collects.
#[cfg(feature = "treesit")]
pub const INJECTION_DOCUMENTS: usize = 64;

/// The most pieces (included ranges) one document has; a document of more keeps the host's
/// colours rather than hand tree-sitter an unbounded range list.
#[cfg(feature = "treesit")]
pub const INJECTION_PIECES: usize = 256;

/// The language of an embedded document: fixed by the rule, or read from a node beside it.
#[derive(Debug)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub enum InjectedLanguage {
    /// `:lang :bash` — every matching document is this language.
    Fixed(String),
    /// `:lang-from :info_string` — the first word of a node of this kind among the content's
    /// parent's children (a fence's info string, a sigil's name), or else among the content
    /// node's own; `:aliases` maps a word to a language (`"sh"` → `bash`, `"H"` → `heex`),
    /// and a word with no alias names its language itself, lowercased.
    From {
        kind: String,
        aliases: std::collections::HashMap<String, String>,
    },
}

/// One injection rule: the nodes whose text is the embedded document, and its language.
#[derive(Debug)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub struct InjectionRule {
    pub content: Selector,
    /// `:exclude` (ADR-496): the content's descendants this selects are cut out of the
    /// embedded document and keep the host's colours — a Dockerfile heredoc's `<<EOT` marker
    /// inside the shell it introduces, tree-sitter's `injection.include-children` false.
    pub exclude: Option<Selector>,
    pub language: InjectedLanguage,
}

/// What a host declares about embedded documents: its rules, and the face table to colour
/// each injected language with. A language with no table here is never injected — which is
/// also what keeps an open `:lang-from` word (`~r`, a fence tagged `text`) from reaching a
/// grammar nobody meant.
#[derive(Debug, Default)]
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub struct Injections {
    pub rules: Vec<InjectionRule>,
    pub tables: std::collections::HashMap<String, SpanTable>,
}

/// One embedded document as the walk finds it: the rule that matched, the parent whose
/// matching children it is, and those children — its pieces, in source order — less the byte
/// ranges of their descendants the rule's `:exclude` selects.
#[cfg(feature = "treesit")]
struct InjectedDocument<'tree> {
    rule: usize,
    parent: tree_sitter::Node<'tree>,
    pieces: Vec<tree_sitter::Node<'tree>>,
    excluded: Vec<(usize, usize)>,
}

#[cfg(feature = "treesit")]
fn overlaps(start: usize, end: usize, window: Option<(usize, usize)>) -> bool {
    match window {
        None => true,
        Some((from, to)) => {
            (start < to && end > from) || (start == end && from <= start && start < to)
        }
    }
}

/// Collect the embedded documents under `node` that reach into `window`. At each node the
/// walk tests EVERY child against the rules — a document's pieces outside the window still
/// belong to it, so a window on the middle of a heredoc parses all of it — and descends only
/// into children that matched nothing and reach the window. A matched child is not entered:
/// the outermost content node is the document, and injections do not nest.
#[cfg(feature = "treesit")]
fn injections_into<'tree, T: SpanText>(
    node: tree_sitter::Node<'tree>,
    context: &NodeContext,
    text: &T,
    rules: &[InjectionRule],
    window: Option<(usize, usize)>,
    out: &mut Vec<InjectedDocument<'tree>>,
) {
    if out.len() >= INJECTION_DOCUMENTS {
        return;
    }
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    let mut named_index = 0;
    let mut here: Vec<InjectedDocument<'tree>> = Vec::new();
    loop {
        let child = cursor.node();
        // past the window, only a document already open here can still gain pieces
        let past = window.is_some_and(|(_, to)| child.start_byte() >= to);
        if past && here.is_empty() {
            break;
        }
        let child_context = NodeContext {
            parent: Some(node),
            field: cursor.field_name(),
            nth: child.is_named().then_some(named_index),
            outer: Some(context),
        };
        if child.is_named() {
            named_index += 1;
        }
        match rules
            .iter()
            .position(|rule| rule.content.holds(child, &child_context, text))
        {
            Some(rule) => {
                let mut excluded = Vec::new();
                if let Some(selector) = &rules[rule].exclude {
                    excluded_into(child, &child_context, text, selector, &mut excluded);
                }
                match here.iter_mut().find(|document| document.rule == rule) {
                    Some(document) => {
                        document.pieces.push(child);
                        document.excluded.extend(excluded);
                    }
                    None => here.push(InjectedDocument {
                        rule,
                        parent: node,
                        pieces: vec![child],
                        excluded,
                    }),
                }
            }
            None if !past && overlaps(child.start_byte(), child.end_byte(), window) => {
                injections_into(child, &child_context, text, rules, window, out);
            }
            None => {}
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
    out.extend(here.into_iter().filter(|document| {
        document
            .pieces
            .iter()
            .any(|piece| overlaps(piece.start_byte(), piece.end_byte(), window))
    }));
    out.truncate(INJECTION_DOCUMENTS);
}

/// The byte ranges of `node`'s descendants that `selector` holds for — the outermost ones: a
/// node it selects is not entered. What a rule's `:exclude` cuts out of its document.
#[cfg(feature = "treesit")]
fn excluded_into<T: SpanText>(
    node: tree_sitter::Node,
    context: &NodeContext,
    text: &T,
    selector: &Selector,
    out: &mut Vec<(usize, usize)>,
) {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    let mut named_index = 0;
    loop {
        let child = cursor.node();
        let child_context = NodeContext {
            parent: Some(node),
            field: cursor.field_name(),
            nth: child.is_named().then_some(named_index),
            outer: Some(context),
        };
        if child.is_named() {
            named_index += 1;
        }
        if selector.holds(child, &child_context, text) {
            out.push((child.start_byte(), child.end_byte()));
        } else {
            excluded_into(child, &child_context, text, selector, out);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// The word a `:lang-from` node names: its text's leading run of letters, digits and `_ - +
/// #` (`rust` of `rust,ignore`, `c++`, `H`).
#[cfg(feature = "treesit")]
fn language_word<T: SpanText>(node: tree_sitter::Node, text: &T) -> Option<String> {
    let source = text.text_in(node.start_byte(), node.end_byte());
    let word: String = source
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || "_-+#".contains(*c))
        .collect();
    (!word.is_empty()).then_some(word)
}

/// The language of `document`, by its rule.
#[cfg(feature = "treesit")]
fn document_language<'tree, T: SpanText>(
    document: &InjectedDocument<'tree>,
    rule: &InjectionRule,
    text: &T,
) -> Option<String> {
    let (kind, aliases) = match &rule.language {
        InjectedLanguage::Fixed(lang) => return Some(lang.clone()),
        InjectedLanguage::From { kind, aliases } => (kind, aliases),
    };
    let first_of_kind = |node: tree_sitter::Node<'tree>| -> Option<tree_sitter::Node<'tree>> {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| child.kind() == kind.as_str())
    };
    let named_by = first_of_kind(document.parent).or_else(|| {
        document
            .pieces
            .first()
            .and_then(|piece| first_of_kind(*piece))
    })?;
    let word = language_word(named_by, text)?;
    Some(
        aliases
            .get(&word)
            .cloned()
            .unwrap_or_else(|| word.to_lowercase()),
    )
}

/// Parse byte `pieces` (sorted, disjoint, non-empty) of `text` as ONE `lang` document — the
/// included-ranges parse. None when no grammar for `lang` loads, or the parse fails: an
/// embedded language's grammar is optional, and its absence is never an error.
#[cfg(feature = "treesit")]
fn parse_pieces<T: SpanText>(
    text: &T,
    lang: &str,
    pieces: &[(usize, usize)],
) -> Option<tree_sitter::Tree> {
    let language = language_for(lang).ok()?;
    let mut parser = checkout_parser(lang, &language).ok()?;
    let bytes: Vec<usize> = pieces
        .iter()
        .flat_map(|(start, end)| [*start, *end])
        .collect();
    // a grammar's scanner may ask the lexer for its column (Python's indentation, Markdown's
    // block structure), and the column comes from the range's point
    let points = text.points_of(&bytes);
    let ranges: Vec<tree_sitter::Range> = pieces
        .iter()
        .enumerate()
        .map(|(index, (start, end))| tree_sitter::Range {
            start_byte: *start,
            end_byte: *end,
            start_point: points[2 * index],
            end_point: points[2 * index + 1],
        })
        .collect();
    let tree = parser.set_included_ranges(&ranges).ok().and_then(|()| {
        let length = text.byte_length();
        parser.parse_with_options(
            &mut |byte: usize, _point: tree_sitter::Point| -> &[u8] {
                if byte >= length {
                    &[]
                } else {
                    text.bytes_from(byte)
                }
            },
            None,
            Some(tree_sitter::ParseOptions::new().progress_callback(&mut interruptible)),
        )
    });
    // a pooled parser must not carry this call's ranges into the next parse
    let _ = parser.set_included_ranges(&[]);
    return_parser(lang, parser);
    tree
}

/// `pieces` (sorted, disjoint) with, between each two, the last line break of the text they
/// skip — what the embedded parser reads. An included-ranges parse sees nothing of a gap, so
/// two heredoc lines handed over as two pieces, or a line on each side of an excluded node,
/// were one line to it: `echo a` then `echo b` parsed as `echo a echo b`. The break is parsed,
/// never coloured: spans stay clipped to the pieces, so it keeps the host's colour (ADR-496).
#[cfg(feature = "treesit")]
fn with_line_breaks<T: SpanText>(text: &T, pieces: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut joined = Vec::with_capacity(pieces.len() * 2);
    for (index, piece) in pieces.iter().enumerate() {
        if index > 0 {
            let gap_start = pieces[index - 1].1;
            if gap_start < piece.0
                && let Some(at) = text.text_in(gap_start, piece.0).rfind('\n')
            {
                joined.push((gap_start + at, gap_start + at + 1));
            }
        }
        joined.push(*piece);
    }
    normalized_pieces(joined)
}

/// `pieces` (sorted, disjoint) with every byte of `cut` (sorted, disjoint) taken out — a
/// document's pieces less what its rule's `:exclude` selected.
#[cfg(feature = "treesit")]
fn without_ranges(pieces: Vec<(usize, usize)>, cut: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if cut.is_empty() {
        return pieces;
    }
    let mut kept = Vec::with_capacity(pieces.len() + cut.len());
    for (start, end) in pieces {
        let mut cursor = start;
        for (cut_start, cut_end) in cut {
            if *cut_end <= cursor {
                continue;
            }
            if *cut_start >= end {
                break;
            }
            if *cut_start > cursor {
                kept.push((cursor, *cut_start));
            }
            cursor = (*cut_end).min(end);
        }
        if cursor < end {
            kept.push((cursor, end));
        }
    }
    kept
}

/// The injected document's spans: `pieces` of `text` parsed as one `lang` document (joined by
/// their gaps' line breaks, [`with_line_breaks`]) and walked with `table`, each span clipped to
/// the pieces (a string spanning an interpolation colours only the string's own text) and to
/// `window`. None when there is no grammar.
#[cfg(feature = "treesit")]
fn injected_spans<T: SpanText>(
    text: &T,
    lang: &str,
    pieces: &[(usize, usize)],
    table: &SpanTable,
    keywords: bool,
    window: Option<(usize, usize)>,
) -> Option<Vec<RawSpan>> {
    let tree = parse_pieces(text, lang, &with_line_breaks(text, pieces))?;
    let query = SpanQuery {
        table,
        keywords,
        window: None,
    };
    let mut whole: Vec<RawSpan> = Vec::new();
    spans_into(
        tree.root_node(),
        &NodeContext::ROOT,
        text,
        &query,
        &mut whole,
    );
    let (from, to) = window.unwrap_or((0, usize::MAX));
    let mut clipped = Vec::with_capacity(whole.len());
    for (start, end, reported) in whole {
        for (piece_start, piece_end) in pieces {
            let low = start.max(*piece_start).max(from);
            let high = end.min(*piece_end).min(to);
            if low < high || (start == end && low == high && start == low) {
                clipped.push((low, high, reported));
            }
        }
    }
    Some(clipped)
}

/// Sort byte ranges, drop empty ones and merge the overlapping — what an included-ranges parse
/// requires of them.
#[cfg(feature = "treesit")]
fn normalized_pieces(mut pieces: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    pieces.retain(|(start, end)| start < end);
    pieces.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(pieces.len());
    for (start, end) in pieces {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Bound one document's pieces to what a call may parse: whole when it fits
/// [`INJECTION_DOCUMENT_BYTES`], else cut to `window`; None when it still does not fit, or
/// has more than [`INJECTION_PIECES`] pieces.
#[cfg(feature = "treesit")]
fn bounded_pieces(
    pieces: Vec<(usize, usize)>,
    window: Option<(usize, usize)>,
) -> Option<Vec<(usize, usize)>> {
    let size =
        |pieces: &[(usize, usize)]| pieces.iter().map(|(start, end)| end - start).sum::<usize>();
    let pieces = if size(&pieces) <= INJECTION_DOCUMENT_BYTES {
        pieces
    } else {
        let (from, to) = window?;
        normalized_pieces(
            pieces
                .into_iter()
                .map(|(start, end)| (start.max(from), end.min(to)))
                .collect(),
        )
    };
    (!pieces.is_empty()
        && pieces.len() <= INJECTION_PIECES
        && size(&pieces) <= INJECTION_DOCUMENT_BYTES)
        .then_some(pieces)
}

/// The host's `spans` with every embedded document's spans spliced over it: the documents
/// reaching the query's window are found, parsed within the caps, and over each one's pieces
/// the host's spans are cut away and the injected ones put in. A document whose language
/// has no table or no grammar, or that the caps leave out, keeps the host's colours.
#[cfg(feature = "treesit")]
fn splice_injections<T: SpanText>(
    root: tree_sitter::Node,
    text: &T,
    query: &SpanQuery,
    injections: &Injections,
    host: Vec<RawSpan>,
) -> Vec<RawSpan> {
    let mut documents: Vec<InjectedDocument> = Vec::new();
    injections_into(
        root,
        &NodeContext::ROOT,
        text,
        &injections.rules,
        query.window,
        &mut documents,
    );
    let mut budget = INJECTION_CALL_BYTES;
    let mut covered: Vec<(usize, usize)> = Vec::new();
    let mut injected: Vec<RawSpan> = Vec::new();
    for document in &documents {
        let rule = &injections.rules[document.rule];
        let Some(lang) = document_language(document, rule, text) else {
            continue;
        };
        let Some(table) = injections.tables.get(&lang) else {
            continue;
        };
        let pieces = without_ranges(
            normalized_pieces(
                document
                    .pieces
                    .iter()
                    .map(|piece| (piece.start_byte(), piece.end_byte()))
                    .collect(),
            ),
            &normalized_pieces(document.excluded.clone()),
        );
        let Some(pieces) = bounded_pieces(pieces, query.window) else {
            continue;
        };
        let size: usize = pieces.iter().map(|(start, end)| end - start).sum();
        if size > budget {
            continue;
        }
        let Some(spans) = injected_spans(text, &lang, &pieces, table, query.keywords, query.window)
        else {
            continue;
        };
        budget -= size;
        covered.extend(pieces);
        injected.extend(spans);
    }
    if covered.is_empty() {
        return host;
    }
    let covered = normalized_pieces(covered);
    let mut spliced = subtract_ranges(host, &covered);
    spliced.extend(injected);
    spliced.sort_by_key(|(start, end, _)| (*start, *end));
    spliced
}

/// `spans` with every part inside a `covered` range (sorted, disjoint) cut away: a span
/// straddling one keeps its parts outside it, a zero-width span inside one is dropped.
#[cfg(feature = "treesit")]
fn subtract_ranges(spans: Vec<RawSpan>, covered: &[(usize, usize)]) -> Vec<RawSpan> {
    let mut out = Vec::with_capacity(spans.len());
    for (start, end, reported) in spans {
        // the first covered range that ends after the span starts
        let first = covered.partition_point(|(_, covered_end)| *covered_end <= start);
        let mut cursor = start;
        let mut dropped = false;
        for (covered_start, covered_end) in &covered[first..] {
            if *covered_start >= end && !(start == end && *covered_start == start) {
                break;
            }
            if start == end {
                dropped = *covered_start <= start && start < *covered_end;
                break;
            }
            if *covered_start > cursor {
                out.push((cursor, *covered_start, reported));
            }
            cursor = cursor.max(*covered_end);
        }
        if start == end {
            if !dropped {
                out.push((start, end, reported));
            }
        } else if cursor < end {
            out.push((cursor, end, reported));
        }
    }
    out
}

/// `(%tree-sitter-range-spans text lang ranges table keywords?)` — chars `ranges` of `text`
/// parsed as ONE `lang` document and coloured with `table`: `[start end reported]` in the
/// outer text's char offsets, clipped to the ranges. `Nil` when no grammar for `lang` loads.
/// Ranges are sorted and merged; past [`INJECTION_PIECES`] ranges or [`INJECTION_CALL_BYTES`]
/// bytes the rest are left out, so the call is bounded whatever it is handed.
#[cfg(feature = "treesit")]
pub fn range_spans(
    heap: &mut Heap,
    text: RangeText,
    lang: &str,
    ranges: &[(usize, usize)],
    table: &SpanTable,
    keywords: bool,
) -> LispResult {
    fn bounded_byte_ranges<T: SpanText>(
        text: &T,
        ranges: &[(usize, usize)],
    ) -> Vec<(usize, usize)> {
        let pieces = normalized_pieces(
            ranges
                .iter()
                .map(|(start, end)| (text.byte_of(*start), text.byte_of(*end)))
                .collect(),
        );
        let mut budget = INJECTION_CALL_BYTES;
        let mut kept = Vec::new();
        for (start, end) in pieces.into_iter().take(INJECTION_PIECES) {
            if budget == 0 {
                break;
            }
            let end = end.min(start + budget);
            budget -= end - start;
            kept.push((start, end));
        }
        kept
    }
    fn answer<T: SpanText>(
        heap: &mut Heap,
        text: &T,
        lang: &str,
        ranges: &[(usize, usize)],
        table: &SpanTable,
        keywords: bool,
    ) -> Value {
        let pieces = bounded_byte_ranges(text, ranges);
        if pieces.is_empty() {
            return if language_for(lang).is_ok() {
                heap.alloc_vector(Vec::new())
            } else {
                Value::Nil
            };
        }
        match injected_spans(text, lang, &pieces, table, keywords, None) {
            Some(mut raw) => {
                raw.sort_by_key(|(start, end, _)| (*start, *end));
                raw_spans_value(heap, text, raw)
            }
            None => Value::Nil,
        }
    }
    Ok(match text {
        RangeText::Rope(rope) => answer(heap, &rope, lang, ranges, table, keywords),
        RangeText::Flat(src) => {
            let b2c = byte_to_char_offsets(&src);
            let flat = FlatText {
                src: &src,
                b2c: &b2c,
            };
            answer(heap, &flat, lang, ranges, table, keywords)
        }
    })
}

/// The text a range query colours: a rope, read in place, or a string.
#[cfg_attr(not(feature = "treesit"), allow(dead_code))]
pub enum RangeText {
    Rope(ropey::Rope),
    Flat(String),
}

/// Widen `changed` byte ranges (sorted, disjoint) so each covers every embedded document it
/// reaches, whole — an edit inside a fence can recolour the fence below the edit (a quote
/// opened there), which the HOST tree's changed ranges cannot know, since the host's syntax
/// did not change. A change to what names a document's language (a fence's info string)
/// widens to that document too.
#[cfg(feature = "treesit")]
fn widen_to_documents(
    tree: &tree_sitter::Tree,
    text: &ropey::Rope,
    rules: &[InjectionRule],
    changed: Vec<(usize, usize)>,
) -> Vec<(usize, usize)> {
    let root = tree.root_node();
    let mut widened = Vec::with_capacity(changed.len());
    for (from, to) in changed {
        let mut documents: Vec<InjectedDocument> = Vec::new();
        // a deletion is a zero-width change; the window must still touch the nodes around it
        let window = (from, to.max(from + 1));
        injections_into(
            root,
            &NodeContext::ROOT,
            text,
            rules,
            Some(window),
            &mut documents,
        );
        let (mut low, mut high) = (from, to);
        for document in &documents {
            let start = document
                .pieces
                .first()
                .map_or(low, |piece| piece.start_byte());
            let end = document
                .pieces
                .last()
                .map_or(high, |piece| piece.end_byte());
            low = low.min(start);
            high = high.max(end);
        }
        widened.push((low, high));
    }
    normalized_pieces_keep_empty(widened)
}

/// [`normalized_pieces`] that keeps zero-width ranges (a deletion's place).
#[cfg(feature = "treesit")]
fn normalized_pieces_keep_empty(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

// ---- tree handles: a parse tree a caller keeps, edits and queries (ADR-443) ----------
//
// The slice-based queries above take TEXT and find the tree themselves, which is the
// right shape for a one-off question and the wrong one for an editor's view: the text has
// to be copied out of the buffer on every keystroke, the edit has to be rediscovered by
// diffing it against whatever was parsed last, and a view that colours a window parses
// the window — a slice with no context above it, so a string opened off screen is code.
//
// A handle is the tree of a WHOLE document, held across edits. It is a `Ref`: opaque,
// compared by identity, sendable by value (the registry is runtime-wide, so any process
// holding the ref can query it). Each handle is IMMUTABLE — a reparse returns a NEW handle
// and leaves the old one answering exactly as before, so a model that keeps one beside a
// rope can never see it change underneath it.
//
// A handle is OWNED (ADR-495), the way a socket, a child process and a wasm instance are:
// by the process that opened it, until that process hands it on (`%tree-sitter-tree-give`
// — a task parsing off the loop gives the editor its tree before replying). It goes away
// only when its owner frees it or dies; no other process's allocations can take it. Before
// ADR-495 the registry was a runtime-wide LRU of 32, and a 33rd document anywhere in the
// runtime — another buffer, another test running beside this one — silently dropped the
// oldest, so an editor with many files open quietly fell back to full parses and a test
// that opened a tree, edited it and asked what changed saw `true` (a fresh parse) instead.
// A gone handle still reads as `nil` from every query and reparsing from it is a full
// parse — but now only its holder can make it gone.

/// Per language, bumped by every load of that language's grammar. A tree parsed under an
/// older grammar is never handed to the new grammar's parser as the tree to reparse from:
/// that is not stale but ill-formed, and fails the parse outright (see `load_grammar`).
///
/// Per LANGUAGE, not one counter for the runtime: loading the HCL grammar says nothing about
/// an Elixir tree, and a single counter made every held tree of every language reparse from
/// scratch whenever any grammar loaded — which, with grammars declared and loaded at first
/// use (ADR-481), is whenever a file of a new language is first opened (ADR-583).
#[cfg(feature = "treesit")]
static GRAMMAR_EPOCHS: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "treesit")]
fn grammar_epoch(lang: &str) -> u64 {
    crate::core::sync::lock(&GRAMMAR_EPOCHS)
        .get(lang)
        .copied()
        .unwrap_or(0)
}

/// One handle's document: the tree, the text it was parsed from (a rope clone — ropey
/// shares structure, so this is a reference count, not a copy), and what changed since the
/// tree it was reparsed from.
#[cfg(feature = "treesit")]
#[derive(Clone)]
struct TreeEntry {
    tree: tree_sitter::Tree,
    text: ropey::Rope,
    lang: String,
    epoch: u64,
    /// Byte ranges of THIS text whose highlighting may differ from the previous version's:
    /// the edited text plus tree-sitter's `changed_ranges`. `None` for a fresh parse.
    changed: Option<Vec<(usize, usize)>>,
}

/// Every live handle, indexed by the process that owns it, so a process's death frees its
/// trees in O(what it owned) ([`close_process_trees`]).
#[cfg(feature = "treesit")]
static TREE_HANDLES: LazyLock<Mutex<crate::host::owned::OwnedTable<TreeEntry>>> =
    LazyLock::new(|| Mutex::new(crate::host::owned::OwnedTable::default()));

/// A clone of handle `id`'s entry (cheap: a tree refcount and a rope refcount); `None` when
/// the handle was freed or its owner died.
#[cfg(feature = "treesit")]
fn tree_entry(id: u64) -> Option<TreeEntry> {
    crate::core::sync::lock(&TREE_HANDLES).get(&id).cloned()
}

/// Register `entry` under a fresh ref id, owned by process `owner`.
#[cfg(feature = "treesit")]
fn tree_register(entry: TreeEntry, owner: u64) -> u64 {
    let id = crate::process::monitor::next_ref();
    crate::core::sync::lock(&TREE_HANDLES).insert(id, owner, entry);
    id
}

/// Parse `text` (a rope, read chunk by chunk — never flattened) with `lang`, from `old`.
#[cfg(feature = "treesit")]
fn parse_rope(
    text: &ropey::Rope,
    lang: &str,
    old: Option<&tree_sitter::Tree>,
) -> Result<tree_sitter::Tree, LispError> {
    let language = language_for(lang)?;
    let mut parser = checkout_parser(lang, &language)?;
    let length = text.len_bytes();
    let tree = parser.parse_with_options(
        &mut |byte: usize, _point: tree_sitter::Point| -> &[u8] {
            if byte >= length {
                return &[];
            }
            let (chunk, chunk_start, _, _) = text.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - chunk_start..]
        },
        old,
        Some(tree_sitter::ParseOptions::new().progress_callback(&mut interruptible)),
    );
    return_parser(lang, parser);
    tree.ok_or_else(|| parse_failed(lang))
}

/// The tree-sitter `Point` of `byte` in `text`: its line, and its byte column in that line.
/// Lines are ropey's (which also breaks on `\r` alone and the Unicode separators, where
/// tree-sitter breaks on `\n`); the parse itself is byte-addressed, so a row that differs on
/// such text skews only a point-based query, which nothing here makes.
#[cfg(feature = "treesit")]
fn point_of(text: &ropey::Rope, byte: usize) -> tree_sitter::Point {
    let byte = byte.min(text.len_bytes());
    let row = text.byte_to_line(byte);
    tree_sitter::Point::new(row, byte - text.line_to_byte(row))
}

/// The common prefix of two ropes in bytes, compared chunk run by chunk run (a `memcmp`
/// per run, not a loop per byte).
#[cfg(feature = "treesit")]
fn common_prefix(old: &ropey::Rope, new: &ropey::Rope) -> usize {
    let mut old_chunks = old.chunks();
    let mut new_chunks = new.chunks();
    let mut old_run: &[u8] = &[];
    let mut new_run: &[u8] = &[];
    let mut total = 0usize;
    loop {
        if old_run.is_empty() {
            match old_chunks.next() {
                Some(chunk) => old_run = chunk.as_bytes(),
                None => return total,
            }
            continue;
        }
        if new_run.is_empty() {
            match new_chunks.next() {
                Some(chunk) => new_run = chunk.as_bytes(),
                None => return total,
            }
            continue;
        }
        let n = old_run.len().min(new_run.len());
        if old_run[..n] == new_run[..n] {
            total += n;
            old_run = &old_run[n..];
            new_run = &new_run[n..];
        } else {
            let first = old_run[..n]
                .iter()
                .zip(&new_run[..n])
                .position(|(a, b)| a != b)
                .unwrap_or(n);
            return total + first;
        }
    }
}

/// The common suffix of two ropes in bytes, at most `limit` — the same run comparison as
/// [`common_prefix`], walking the chunks from the end.
#[cfg(feature = "treesit")]
fn common_suffix(old: &ropey::Rope, new: &ropey::Rope, limit: usize) -> usize {
    let mut old_chunks = old.chunks_at_byte(old.len_bytes()).0.reversed();
    let mut new_chunks = new.chunks_at_byte(new.len_bytes()).0.reversed();
    let mut old_run: &[u8] = &[];
    let mut new_run: &[u8] = &[];
    let mut total = 0usize;
    while total < limit {
        if old_run.is_empty() {
            match old_chunks.next() {
                Some(chunk) => old_run = chunk.as_bytes(),
                None => break,
            }
            continue;
        }
        if new_run.is_empty() {
            match new_chunks.next() {
                Some(chunk) => new_run = chunk.as_bytes(),
                None => break,
            }
            continue;
        }
        let n = old_run.len().min(new_run.len()).min(limit - total);
        let old_tail = &old_run[old_run.len() - n..];
        let new_tail = &new_run[new_run.len() - n..];
        if old_tail == new_tail {
            total += n;
            old_run = &old_run[..old_run.len() - n];
            new_run = &new_run[..new_run.len() - n];
        } else {
            let same = old_tail
                .iter()
                .rev()
                .zip(new_tail.iter().rev())
                .position(|(a, b)| a != b)
                .unwrap_or(n);
            return total + same;
        }
    }
    total.min(limit)
}

/// The `[start old-end new-end)` byte edit turning `old` into `new`, from their common
/// prefix and suffix — [`derive_edit`] over ropes. Widened to whole characters, so the
/// edit never splits a UTF-8 sequence. `None` when the texts are equal.
#[cfg(feature = "treesit")]
fn derive_rope_edit(old: &ropey::Rope, new: &ropey::Rope) -> Option<(usize, usize, usize)> {
    let prefix = common_prefix(old, new);
    let shorter = old.len_bytes().min(new.len_bytes());
    let suffix = common_suffix(old, new, shorter - prefix.min(shorter));
    let start = old.char_to_byte(old.byte_to_char(prefix));
    let old_end_raw = old.len_bytes() - suffix;
    // round the end UP to a char boundary: the suffix bytes are identical in both texts, so a
    // boundary in one is a boundary in the other at the same distance from the end
    let old_end_char = old.byte_to_char(old_end_raw);
    let old_end = if old.char_to_byte(old_end_char) == old_end_raw {
        old_end_raw
    } else {
        old.char_to_byte(old_end_char + 1)
    };
    let new_end = new.len_bytes() - (old.len_bytes() - old_end);
    if start == old_end && start == new_end {
        return None;
    }
    Some((start, old_end, new_end.max(start)))
}

/// Compose a sequence of CHAR edits `[start old-end new-end]`, each in the coordinates of the
/// text the previous ones produced, into the one edit that covers them all: `start` and
/// `old-end` in the original text, `new-end` in the final one. tree-sitter is told one edit
/// per reparse, and a wider one is only a wider reparse — never a wrong tree.
#[cfg(feature = "treesit")]
fn compose_char_edits(edits: &[(usize, usize, usize)]) -> Option<(usize, usize, usize)> {
    let mut iter = edits.iter();
    let (mut start, mut old_end, mut new_end) = *iter.next()?;
    for &(next_start, next_old_end, next_new_end) in iter {
        // the covered range in CURRENT coordinates is [start, new_end); the next edit replaces
        // [next_start, next_old_end) of the current text
        let covered_end = new_end.max(next_old_end);
        // map the covered end back to the ORIGINAL text: past the cover the two differ by
        // the cover's own growth
        let original_end = if covered_end == new_end {
            old_end
        } else {
            covered_end + old_end - new_end
        };
        let shifted_end = covered_end + next_new_end - next_old_end;
        start = start.min(next_start);
        old_end = original_end.max(old_end);
        new_end = shifted_end;
    }
    Some((start, old_end, new_end))
}

/// A byte `InputEdit` from a `[start old-end new-end)` byte triple and both texts.
#[cfg(feature = "treesit")]
fn input_edit(
    old: &ropey::Rope,
    new: &ropey::Rope,
    (start, old_end, new_end): (usize, usize, usize),
) -> tree_sitter::InputEdit {
    tree_sitter::InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point_of(old, start),
        old_end_position: point_of(old, old_end),
        new_end_position: point_of(new, new_end),
    }
}

/// `(%tree-sitter-tree text lang)` — parse a whole document into a fresh handle owned by
/// process `owner`.
#[cfg(feature = "treesit")]
pub fn tree_open(text: ropey::Rope, lang: &str, owner: u64) -> Result<u64, LispError> {
    let tree = parse_rope(&text, lang, None)?;
    Ok(tree_register(
        TreeEntry {
            tree,
            text,
            lang: lang.to_string(),
            epoch: grammar_epoch(lang),
            changed: None,
        },
        owner,
    ))
}

/// `(%tree-sitter-tree-reparse tree text lang edits)` — the handle for `text`, reparsed
/// incrementally from `old`'s tree. `edits` is the caller's account of what changed, as CHAR
/// triples `[start old-end new-end]` applied in order; `None` derives the edit by comparing
/// the ropes (prefix and suffix, chunk by chunk). An edit that does not fit the two texts —
/// whose unchanged tails differ in length — is an error. Only the lengths are checked: an edit
/// that fits them but names the wrong place gives a wrong tree. The new handle is `owner`'s,
/// whoever owns `old`.
#[cfg(feature = "treesit")]
pub fn tree_reparse(
    old: u64,
    text: ropey::Rope,
    lang: &str,
    edits: Option<Vec<(usize, usize, usize)>>,
    owner: u64,
) -> Result<u64, LispError> {
    // Resolved before the epoch is read: a declared grammar's first use (re)loads it here and
    // moves the epoch, and the old tree must then not be handed to the new grammar's parser.
    language_for(lang)?;
    let previous = tree_entry(old).filter(|e| e.lang == lang && e.epoch == grammar_epoch(lang));
    let Some(previous) = previous else {
        return tree_open(text, lang, owner); // gone, another language, or another grammar
    };
    let byte_edit = match edits {
        None => derive_rope_edit(&previous.text, &text),
        Some(edits) => match compose_char_edits(&edits) {
            None => derive_rope_edit(&previous.text, &text),
            Some((start, old_end, new_end)) => {
                let old_chars = previous.text.len_chars();
                let new_chars = text.len_chars();
                if start > old_end
                    || old_end > old_chars
                    || start > new_end
                    || new_end > new_chars
                    || old_chars - old_end != new_chars - new_end
                {
                    return Err(LispError::runtime(format!(
                        "%tree-sitter-tree-reparse: edit [{start} {old_end} {new_end}] does \
                         not turn the tree's text ({old_chars} chars) into this one \
                         ({new_chars} chars)"
                    )));
                }
                Some((
                    previous.text.char_to_byte(start),
                    previous.text.char_to_byte(old_end),
                    text.char_to_byte(new_end),
                ))
            }
        },
    };
    let Some(byte_edit) = byte_edit else {
        // the same text: a new handle on the same tree, nothing changed
        return Ok(tree_register(
            TreeEntry {
                changed: Some(Vec::new()),
                text,
                ..previous
            },
            owner,
        ));
    };
    let (start, old_end, new_end) = byte_edit;
    let old_length = previous.text.len_bytes();
    // the same rule as `derive_edit`: an edit spanning most of a large document is a new
    // document, and reparsing from the old tree buys nothing
    if (old_end - start) * 4 > old_length && old_length > 4096 {
        return tree_open(text, lang, owner);
    }
    let mut edited = previous.tree.clone();
    edited.edit(&input_edit(&previous.text, &text, byte_edit));
    let tree = parse_rope(&text, lang, Some(&edited))?;
    let mut changed: Vec<(usize, usize)> = edited
        .changed_ranges(&tree)
        .map(|range| (range.start_byte, range.end_byte))
        .collect();
    changed.push((start, new_end));
    changed.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(changed.len());
    for (from, to) in changed {
        match merged.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    Ok(tree_register(
        TreeEntry {
            tree,
            text,
            lang: lang.to_string(),
            epoch: previous.epoch,
            changed: Some(merged),
        },
        owner,
    ))
}

/// `(%tree-sitter-tree-spans tree table keywords? from to injections)` — the fontify walk over
/// a held tree, restricted to chars `[from, to)` and clipped to it; `Nil` for a gone handle.
/// The walk still starts at the root, so a node's context is the whole document's. Embedded
/// documents reaching the window are parsed for this call (ADR-489): no injected tree is held.
#[cfg(feature = "treesit")]
pub fn tree_spans(
    heap: &mut Heap,
    handle: u64,
    table: &SpanTable,
    keywords: bool,
    from: i64,
    to: i64,
    injections: Option<&Injections>,
) -> LispResult {
    let Some(entry) = tree_entry(handle) else {
        return Ok(Value::Nil);
    };
    let chars = entry.text.len_chars();
    let from = (from.max(0) as usize).min(chars);
    let to = (to.max(0) as usize).min(chars).max(from);
    let window = (entry.text.char_to_byte(from), entry.text.char_to_byte(to));
    let query = SpanQuery {
        table,
        keywords,
        window: Some(window),
    };
    let raw = host_and_injected_spans(&entry.tree, &entry.text, &query, injections);
    Ok(raw_spans_value(heap, &entry.text, raw))
}

/// `(%tree-sitter-tree-changes tree injections)` — the char ranges `[start end]` of `tree`'s
/// text whose colouring may differ from the tree it was reparsed from; `Bool(true)` when it
/// was a fresh parse (everything may differ), `Nil` for a gone handle. With injection rules,
/// each range is widened over the embedded documents it reaches (ADR-489).
#[cfg(feature = "treesit")]
pub fn tree_changes(heap: &mut Heap, handle: u64, injections: Option<&Injections>) -> LispResult {
    let Some(entry) = tree_entry(handle) else {
        return Ok(Value::Nil);
    };
    let Some(changed) = entry.changed else {
        return Ok(Value::Bool(true));
    };
    let changed = match injections {
        Some(injections) if !injections.rules.is_empty() && !changed.is_empty() => {
            widen_to_documents(&entry.tree, &entry.text, &injections.rules, changed)
        }
        _ => changed,
    };
    let ranges: Vec<Value> = changed
        .into_iter()
        .map(|(from, to)| {
            let pair = vec![
                Value::int(entry.text.char_of(from) as i64),
                Value::int(entry.text.char_of(to) as i64),
            ];
            heap.alloc_vector(pair)
        })
        .collect();
    Ok(heap.alloc_vector(ranges))
}

/// `(%tree-sitter-tree-free tree)` — drop a handle now; true when it was still held.
#[cfg(feature = "treesit")]
pub fn tree_free(handle: u64) -> bool {
    crate::core::sync::lock(&TREE_HANDLES)
        .remove(&handle)
        .is_some()
}

/// Why a [`tree_give`] was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum GiveRefused {
    /// The giving process does not own the handle: only the owner decides where it goes.
    NotOwner,
}

/// `(%tree-sitter-tree-give tree pid)` — hand handle `handle`, owned by `from`, to process
/// `to`, which then decides when it goes. `Ok(true)` when `to` holds it now; `Ok(false)` when
/// it was already gone, or `to` has died — a tree given to a dead process dies with it, as
/// it would have had the death come a moment later. The owner is moved BEFORE `to`'s
/// liveness is read, so the retirement sweep either runs after the move (and frees it) or
/// has already run (and the dead `to` is seen here): it is never stranded on a corpse.
#[cfg(feature = "treesit")]
pub fn tree_give(handle: u64, from: u64, to: u64) -> Result<bool, GiveRefused> {
    {
        let mut handles = crate::core::sync::lock(&TREE_HANDLES);
        match handles.owner_of(&handle) {
            None => return Ok(false),
            Some(owner) if owner != from => return Err(GiveRefused::NotOwner),
            Some(_) => handles.set_owner(handle, to),
        }
    }
    if crate::process::is_alive(to) {
        return Ok(true);
    }
    tree_free(handle);
    Ok(false)
}

/// `(%tree-sitter-owned-trees)` — every handle `pid` owns, by ascending id. What a holder sweeps against what it still references: a handle it lost
/// track of (a turn rolled back after opening one) is otherwise held until it exits.
#[cfg(feature = "treesit")]
pub fn owned_trees(pid: u64) -> Vec<u64> {
    let mut owned = crate::core::sync::lock(&TREE_HANDLES).owned_by(pid);
    owned.sort_unstable();
    owned
}

/// Free every tree `pid` owns — the process-death hook, beside the socket, subprocess and
/// wasm sweeps in the scheduler's retirement path. O(the trees it owned).
#[cfg(feature = "treesit")]
pub fn close_process_trees(pid: u64) {
    // Dropped outside the lock: the last reference to a tree frees all of its nodes.
    let closed = crate::core::sync::lock(&TREE_HANDLES).remove_owned_by(pid);
    drop(closed);
}

/// `(%tree-sitter-tree-chain tree offset)` — [`chain`] over a held tree: no parse, no flat
/// copy of the text, O(depth). `Nil` for a gone handle.
#[cfg(feature = "treesit")]
pub fn tree_chain(heap: &mut Heap, handle: u64, offset: i64) -> LispResult {
    let Some(entry) = tree_entry(handle) else {
        return Ok(Value::Nil);
    };
    Ok(chain_in(heap, &entry.tree, &entry.text, offset))
}

/// `(%tree-sitter-tree-kids tree offset)` — [`kids`] over a held tree. `Nil` for a gone
/// handle.
#[cfg(feature = "treesit")]
pub fn tree_kids(heap: &mut Heap, handle: u64, offset: i64, all: bool) -> LispResult {
    let Some(entry) = tree_entry(handle) else {
        return Ok(Value::Nil);
    };
    Ok(kids_in(heap, &entry.tree, &entry.text, offset, all))
}

/// `(%tree-sitter-tree-alive? tree)` — is the handle still held: not freed, its owner alive?
#[cfg(feature = "treesit")]
pub fn tree_alive(handle: u64) -> bool {
    crate::core::sync::lock(&TREE_HANDLES).contains_key(&handle)
}

#[cfg(not(feature = "treesit"))]
fn without_treesit(who: &str) -> LispError {
    LispError::runtime(format!(
        "{who}: this runtime was built without tree-sitter (rebuild with --features treesit)"
    ))
}

#[cfg(not(feature = "treesit"))]
pub fn tree_open(_text: ropey::Rope, _lang: &str, _owner: u64) -> Result<u64, LispError> {
    Err(without_treesit("%tree-sitter-tree"))
}

#[cfg(not(feature = "treesit"))]
pub fn tree_reparse(
    _old: u64,
    _text: ropey::Rope,
    _lang: &str,
    _edits: Option<Vec<(usize, usize, usize)>>,
    _owner: u64,
) -> Result<u64, LispError> {
    Err(without_treesit("%tree-sitter-tree-reparse"))
}

#[cfg(not(feature = "treesit"))]
pub fn tree_spans(
    _heap: &mut crate::core::heap::Heap,
    _handle: u64,
    _table: &SpanTable,
    _keywords: bool,
    _from: i64,
    _to: i64,
    _injections: Option<&Injections>,
) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-spans"))
}

#[cfg(not(feature = "treesit"))]
pub fn tree_changes(
    _heap: &mut crate::core::heap::Heap,
    _handle: u64,
    _injections: Option<&Injections>,
) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-changes"))
}

#[cfg(not(feature = "treesit"))]
pub fn tree_free(_handle: u64) -> bool {
    false
}

#[cfg(not(feature = "treesit"))]
pub fn tree_give(_handle: u64, _from: u64, _to: u64) -> Result<bool, GiveRefused> {
    Ok(false)
}

#[cfg(not(feature = "treesit"))]
pub fn close_process_trees(_pid: u64) {}

#[cfg(not(feature = "treesit"))]
pub fn owned_trees(_pid: u64) -> Vec<u64> {
    Vec::new()
}

#[cfg(not(feature = "treesit"))]
pub fn tree_chain(_heap: &mut crate::core::heap::Heap, _handle: u64, _offset: i64) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-chain"))
}

#[cfg(not(feature = "treesit"))]
pub fn tree_kids(
    _heap: &mut crate::core::heap::Heap,
    _handle: u64,
    _offset: i64,
    _all: bool,
) -> LispResult {
    Err(without_treesit("%tree-sitter-tree-kids"))
}

#[cfg(not(feature = "treesit"))]
pub fn tree_alive(_handle: u64) -> bool {
    false
}

#[cfg(not(feature = "treesit"))]
pub fn spans(
    heap: &mut crate::core::heap::Heap,
    src: &str,
    lang: &str,
    _table: &SpanTable,
    _keywords: bool,
    _injections: Option<&Injections>,
) -> LispResult {
    parse(heap, src, lang)
}

#[cfg(not(feature = "treesit"))]
pub fn range_spans(
    _heap: &mut crate::core::heap::Heap,
    _text: RangeText,
    _lang: &str,
    _ranges: &[(usize, usize)],
    _table: &SpanTable,
    _keywords: bool,
) -> LispResult {
    // no grammar can load in a runtime without tree-sitter, and a missing grammar is nil
    Ok(crate::core::value::Value::Nil)
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
/// drop. Because the loader would then answer a second load of the same PATH with that
/// first object, the library is opened from a copy named for its content
/// ([`open_library`]): a library rebuilt in place and loaded again is the new build.
///
/// Failure is an ordinary error at every step — a missing file, a library with no
/// such grammar, a grammar built for an incompatible tree-sitter. None of them is
/// a crash. What this cannot make safe is a *hostile* library: it is native code
/// and runs with the process's privileges, exactly like an Emacs dynamic module,
/// so an application should load only from a directory its user controls.
#[cfg(feature = "treesit")]
pub fn load_grammar(path: &str, lang: &str) -> LispResult {
    let (abi, _) = load_library("%tree-sitter-load-grammar", path, lang)?;
    // An eager load supersedes a declaration of the same language: the caller has just
    // said which library is the grammar, and a pending one must not replace it at the next
    // parse, nor a failed one shadow it.
    crate::core::sync::lock(&DECLARED).remove(lang);
    Ok(Value::int(abi))
}

/// How a runtime grammar library was opened ([`open_library`]), reported by
/// [`grammar_info`] as `:loaded-via` (ADR-583).
#[cfg(feature = "treesit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadedVia {
    /// A content-named copy beside the library, in its own directory: the same filesystem
    /// (so the same exec permission) and the same `$ORIGIN` for its RPATH.
    Beside,
    /// A content-named copy in the user's cache directory, the library's own directory not
    /// being writable.
    Cache,
    /// The library's own path, no copy being possible: a later load of a library rebuilt at
    /// that path may still be this one until the process restarts.
    InPlace,
}

#[cfg(feature = "treesit")]
impl LoadedVia {
    fn keyword(self) -> &'static str {
        match self {
            LoadedVia::Beside => "beside",
            LoadedVia::Cache => "cache",
            LoadedVia::InPlace => "in-place",
        }
    }
}

/// Held across one copy, open and clean-up, so two loads in this process never write or
/// remove a copy under each other.
#[cfg(feature = "treesit")]
static GRAMMAR_COPY_LOCK: Mutex<()> = Mutex::new(());

/// The user's cache directory for grammar copies, `$XDG_CACHE_HOME/brood/grammars` (else
/// `~/.cache/brood/grammars`), made owner-only — or accepted when it already is: a real
/// directory this user owns that no one else can write, so no one else can have planted a
/// library in it.
#[cfg(feature = "treesit")]
fn grammar_cache_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".cache"))
        })?;
    let dir = base.join("brood").join("grammars");
    std::fs::create_dir_all(dir.parent()?).ok()?;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return None;
        }
    }
    Some(dir)
}

/// Write `bytes` to `copy`, owner-only, through a staging name renamed into place — so a copy
/// of the same name still mapped from an earlier load is replaced by a new file, never
/// truncated under it — then dlopen it and remove the file (the mapping outlives it). `None`
/// when the copy could not be written or opened, for the caller to try the next place.
///
/// # Safety
/// As `libloading::Library::new`.
#[cfg(feature = "treesit")]
unsafe fn open_copy_at(copy: &std::path::Path, bytes: &[u8]) -> Option<libloading::Library> {
    use std::io::Write;
    let staging = copy.with_extension("partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o700);
    let written = options
        .open(&staging)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| std::fs::rename(&staging, copy));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
        return None;
    }
    // SAFETY: the caller's — the dlopen it asked for, of the bytes it named.
    let opened = unsafe { libloading::Library::new(copy) }.ok();
    let _ = std::fs::remove_file(copy);
    opened
}

/// Open the grammar library at `path` (ADR-583): from a copy named for its CONTENT beside it,
/// else from one in the user's cache directory, else — neither being writable or loadable —
/// from `path` itself. Answers the library (or the loader's error for `path`) and how it was
/// opened.
///
/// **Why a copy.** The dynamic loader keys what it has loaded by NAME: `dlopen` of a path it
/// already opened returns the object it has, whatever the file holds now. A library is never
/// unloaded here (see [`load_grammar`]), so loading a grammar, rebuilding it in place and
/// loading it again kept parsing with the first build until the process restarted — an
/// install, an update or a reload all reported success while the old grammar ran. A name
/// made of the content (and this process) gives the loader a new name exactly when the
/// content is new, and the same name when it is not, so an unchanged library loaded again is
/// the object already loaded rather than a second mapping.
///
/// **Why beside it.** A copy in the temporary directory fails on a `noexec` /tmp (a hardened
/// host, a container), and a grammar whose RPATH says `$ORIGIN` finds its sibling libraries
/// only from its own directory. Beside the original it is on the same filesystem with the
/// same `$ORIGIN`; the cache directory is the fallback for a read-only grammar directory, and
/// the path itself the last resort, reported as `:in-place`.
///
/// # Safety
/// As `libloading::Library::new`: the library's initialisers run, with the process's
/// privileges.
#[cfg(feature = "treesit")]
unsafe fn open_library(
    who: &str,
    path: &str,
) -> Result<(Result<libloading::Library, libloading::Error>, LoadedVia), LispError> {
    use std::hash::{Hash, Hasher};
    let bytes =
        std::fs::read(path).map_err(|e| LispError::runtime(format!("{who}:{path}: {e}")))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let original = std::path::Path::new(path);
    let stem = original
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("grammar");
    let name = format!(".{stem}.{:016x}.{}.so", hasher.finish(), std::process::id());
    let _held = crate::core::sync::lock(&GRAMMAR_COPY_LOCK);
    let beside = original.parent().map(|dir| {
        if dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            dir
        }
    });
    // SAFETY: the caller's, for each attempt.
    if let Some(dir) = beside
        && let Some(library) = unsafe { open_copy_at(&dir.join(&name), &bytes) }
    {
        return Ok((Ok(library), LoadedVia::Beside));
    }
    if let Some(dir) = grammar_cache_dir()
        && let Some(library) = unsafe { open_copy_at(&dir.join(&name), &bytes) }
    {
        return Ok((Ok(library), LoadedVia::Cache));
    }
    Ok((
        unsafe { libloading::Library::new(path) },
        LoadedVia::InPlace,
    ))
}

/// dlopen `path`, check the grammar's ABI and register it under `lang` — the work of
/// [`load_grammar`], shared with the lazy load of a declared grammar
/// ([`declared_language`]). `who` is the primitive the caller's errors name. Answers the ABI
/// version and the registered language.
#[cfg(feature = "treesit")]
fn load_library(
    who: &str,
    path: &str,
    lang: &str,
) -> Result<(i64, tree_sitter::Language), LispError> {
    // Brood keywords are spelled with hyphens and C identifiers cannot be, so `:c-sharp`
    // asks for `tree_sitter_c_sharp` — which is exactly what the tree-sitter-c-sharp grammar
    // exports. The library's entry point is named for the GRAMMAR, so `lang` names the
    // grammar rather than being a nickname for it; there is no aliasing here and shouldn't
    // be, since the mode that asks to parse `:c-sharp` is the same name on both sides.
    let symbol = format!("tree_sitter_{}", lang.replace('-', "_"));
    // SAFETY: dlopen'ing a caller-chosen library and calling the grammar constructor it
    // exports. The symbol type is tree-sitter's documented grammar entry point, and the
    // Language it yields is validated against the host's ABI range below before any use.
    // The library is opened from a private copy named for its CONTENT, never from `path`
    // itself: the dynamic loader hands back the object it already has for a name it has
    // seen, so a grammar rebuilt in place and loaded again from the same path would go on
    // parsing with the old one (ADR-583). See `open_library`.
    let (opened, via) = unsafe { open_library(who, path)? };
    let language = unsafe {
        let lib = opened.map_err(|e| LispError::runtime(format!("{who}:{path}: {e}")))?;
        let entry: libloading::Symbol<unsafe extern "C" fn() -> *const ()> = lib
            .get(symbol.as_bytes())
            .map_err(|e| LispError::runtime(format!("{who}:{path} exports no {symbol}: {e}")))?;
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
                "{who}:{path}: {lang} is not usable by this runtime \
                 (grammar ABI {}, this build drives {}..={}): {e}",
                language.abi_version(),
                tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
                tree_sitter::LANGUAGE_VERSION
            ))
        })?;
    let abi = language.abi_version() as i64;
    crate::core::sync::lock(&DYNAMIC).insert(lang.to_string(), language.clone());
    crate::core::sync::lock(&LOADED_FROM).insert(lang.to_string(), (path.to_string(), via));
    // A re-load replaces the entry, so a grammar can be swapped without restarting; the
    // parser pool is keyed by language name and holds parsers bound to the OLD language, so
    // it is cleared for this one rather than left to hand out stale parsers.
    crate::core::sync::lock(&PARSER_POOL).remove(lang);
    // The tree cache is keyed by language name too, and holds trees SHAPED BY the old
    // grammar. An exact hit would hand one back for the new one, and an incremental
    // reparse would hand it to the new grammar's parser as the tree to edit from — which
    // is not merely stale but ill-formed, and fails the parse outright. Same reasoning as
    // the pool above; the cache simply arrived later than the comment did.
    crate::core::sync::lock(&TREE_CACHE).retain(|(l, _, _)| l != lang);
    // Held tree handles keep answering queries from the trees they have — those are valid
    // trees, merely from the old grammar — but none is reparsed FROM again, for the reason
    // above: a reparse after this sees a newer epoch for this language and parses afresh.
    *crate::core::sync::lock(&GRAMMAR_EPOCHS)
        .entry(lang.to_string())
        .or_insert(0) += 1;
    Ok((abi, language))
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
    crate::core::sync::lock(&DYNAMIC).get(lang).cloned()
}

/// The library each grammar in [`DYNAMIC`] was loaded from, keyed by language name — what
/// [`grammar_info`] reports as `:source`, so "which grammar is this" has an answer a user
/// can go and look at. Written beside `DYNAMIC`, after it (never held together).
#[cfg(feature = "treesit")]
static LOADED_FROM: LazyLock<Mutex<HashMap<String, (String, LoadedVia)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(%tree-sitter-grammar-info lang)` — what the grammar `lang` parses with IS (ADR-583):
/// `{:lang :abi :version :name :source :loaded-via :named :anonymous :fields}`. `:named` and `:anonymous`
/// are the visible node kinds (aliases included, hidden and supertype rules not — no tree
/// ever holds one) as keyword sets, `:fields` the field names; `:version` is the grammar's
/// own `"major.minor.patch"` and `:name` its name, both nil before ABI 15, which did not
/// record them; `:source` the library path, or `:built-in`; `:loaded-via` how it was opened —
/// `:beside` / `:cache` (a content-named copy, see [`open_library`]) or `:in-place` (its own
/// path, when no copy could be made: a library rebuilt there may stay the old one until a
/// restart) — nil for a built-in grammar.
///
/// Loads the grammar when it is only declared — asking what a grammar contains is a use of
/// it — and errors as a parse would when there is none. The answer is the whole vocabulary a
/// language's face table, indent spec and injections may name, which is what makes a grammar
/// update that renamed a node DETECTABLE: the editor compares the names it wrote against this
/// set instead of finding out from a file that stopped colouring.
#[cfg(feature = "treesit")]
pub fn grammar_info(heap: &mut Heap, lang: &str) -> LispResult {
    let language = language_for(lang)?;
    let keyword = |name: &str| Value::keyword(value::intern(name));
    let mut named = Vec::new();
    let mut anonymous = Vec::new();
    for id in 0..language.node_kind_count() {
        let Ok(id) = u16::try_from(id) else { break };
        if !language.node_kind_is_visible(id) {
            continue;
        }
        let Some(kind) = language.node_kind_for_id(id) else {
            continue;
        };
        if language.node_kind_is_named(id) {
            named.push(keyword(kind));
        } else {
            anonymous.push(keyword(kind));
        }
    }
    let fields: Vec<Value> = (1..=language.field_count())
        .filter_map(|id| u16::try_from(id).ok())
        .filter_map(|id| language.field_name_for_id(id))
        .map(keyword)
        .collect();
    let named = heap.set_from_elems(named);
    let anonymous = heap.set_from_elems(anonymous);
    let fields = heap.set_from_elems(fields);
    let version = match language.metadata() {
        Some(meta) => heap.alloc_string_owned(format!(
            "{}.{}.{}",
            meta.major_version, meta.minor_version, meta.patch_version
        )),
        None => Value::Nil,
    };
    let name = match language.name() {
        Some(name) => heap.alloc_string(name),
        None => Value::Nil,
    };
    let (source, via) = if dynamic_language(lang).is_some() {
        match crate::core::sync::lock(&LOADED_FROM).get(lang).cloned() {
            Some((path, via)) => (heap.alloc_string_owned(path), keyword(via.keyword())),
            None => (keyword("loaded"), Value::Nil),
        }
    } else {
        (keyword("built-in"), Value::Nil)
    };
    Ok(heap.map_from_pairs(vec![
        (keyword("lang"), keyword(lang)),
        (keyword("abi"), Value::int(language.abi_version() as i64)),
        (keyword("version"), version),
        (keyword("name"), name),
        (keyword("source"), source),
        (keyword("loaded-via"), via),
        (keyword("named"), named),
        (keyword("anonymous"), anonymous),
        (keyword("fields"), fields),
    ]))
}

#[cfg(not(feature = "treesit"))]
pub fn grammar_info(_heap: &mut crate::core::heap::Heap, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-grammar-info::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

/// What a declared grammar is waiting on: its library, or the reason it did not load.
#[cfg(feature = "treesit")]
enum Declared {
    /// Declared and not yet asked for: nothing has been opened.
    Pending { path: String },
    /// Asked for, and the load failed. Kept, not retried: a parse per keystroke would
    /// otherwise dlopen the same broken library per keystroke. A new declaration replaces it.
    Failed { message: String },
}

/// Grammars DECLARED by [`declare_grammar`] and not yet loaded (ADR-481), keyed by language
/// name. The first parse, query or tree that names one loads it ([`declared_language`]),
/// which moves it to [`DYNAMIC`]; a failed load stays here as the reason.
///
/// Lock order: this before [`DYNAMIC`] (and the pool and cache a load clears) — a lazy load
/// holds this lock across the dlopen, which is what makes it happen once.
#[cfg(feature = "treesit")]
static DECLARED: LazyLock<Mutex<HashMap<String, Declared>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(%tree-sitter-declare-grammar path lang)` — say that `lang`'s grammar is the library at
/// `path`, WITHOUT loading it (ADR-481). The first parse, structural query or tree handle that
/// names `lang` dlopens it, once; until then the declaration costs a map entry. An editor
/// can therefore declare every grammar it has at startup and pay for the one the file it
/// opened needs, instead of dlopening them all before its first frame or racing a file
/// opened before a deferred loader got there.
///
/// A declaration replaces any earlier one of the same name, and a declaration of a language
/// already loaded is a RELOAD at its next use — the same swap [`load_grammar`] makes (parser
/// pool, tree cache and epoch), so replacing a library and declaring it again is how a
/// grammar is upgraded without a restart, lazily. A load that fails leaves the grammar the
/// language had before, if any, exactly as a failed [`load_grammar`] does; with none, the
/// parse fails with the load's message. [`grammar_status`] reports which.
#[cfg(feature = "treesit")]
pub fn declare_grammar(path: &str, lang: &str) -> LispResult {
    crate::core::sync::lock(&DECLARED).insert(
        lang.to_string(),
        Declared::Pending {
            path: path.to_string(),
        },
    );
    Ok(Value::Bool(true))
}

#[cfg(not(feature = "treesit"))]
pub fn declare_grammar(_path: &str, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-declare-grammar::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

/// The grammar a DECLARATION of `lang` supplies, loading it if this is its first use:
/// `Ok(None)` when there is no declaration to honour (none, or one that failed while an
/// older grammar still serves), `Err` when the declared library failed and nothing else can
/// parse `lang`. The lock is held across the load, so two processes asking at once load it
/// once — the second finds it loaded.
#[cfg(feature = "treesit")]
fn declared_language(lang: &str) -> Result<Option<tree_sitter::Language>, LispError> {
    let mut declared = crate::core::sync::lock(&DECLARED);
    let path = match declared.get(lang) {
        None => return Ok(None),
        Some(Declared::Failed { message }) => {
            return if has_grammar_without_declaration(lang) {
                Ok(None)
            } else {
                Err(LispError::runtime(message.clone()))
            };
        }
        Some(Declared::Pending { path }) => path.clone(),
    };
    match load_library("%tree-sitter-declare-grammar", &path, lang) {
        Ok((_, language)) => {
            declared.remove(lang);
            Ok(Some(language))
        }
        Err(error) => {
            let message = format!(
                "the :{lang} grammar declared at {path} did not load: {}",
                error.message
            );
            declared.insert(
                lang.to_string(),
                Declared::Failed {
                    message: message.clone(),
                },
            );
            if has_grammar_without_declaration(lang) {
                Ok(None)
            } else {
                Err(LispError::runtime(message))
            }
        }
    }
}

/// Whether `lang` parses without consulting a declaration: loaded earlier, or built in.
#[cfg(feature = "treesit")]
fn has_grammar_without_declaration(lang: &str) -> bool {
    dynamic_language(lang).is_some() || built_in_language(lang).is_some()
}

/// `(%tree-sitter-grammar-status lang)` — where `lang`'s grammar stands: `:declared`
/// (declared, not loaded yet), `:loaded` (loaded at runtime), `:built-in`, a failure value
/// carrying the load's message when the last declaration did not load, or `nil` when the
/// runtime has no grammar for it. Asks; never loads.
#[cfg(feature = "treesit")]
pub fn grammar_status(heap: &mut Heap, lang: &str) -> LispResult {
    let keyword = |name: &str| Value::keyword(value::intern(name));
    let failed = match crate::core::sync::lock(&DECLARED).get(lang) {
        Some(Declared::Pending { .. }) => return Ok(keyword("declared")),
        Some(Declared::Failed { message }) => Some(message.clone()),
        None => None,
    };
    if let Some(message) = failed {
        return Ok(heap.alloc_failure(&message));
    }
    if dynamic_language(lang).is_some() {
        return Ok(keyword("loaded"));
    }
    if built_in_language(lang).is_some() {
        return Ok(keyword("built-in"));
    }
    Ok(Value::Nil)
}

#[cfg(not(feature = "treesit"))]
pub fn grammar_status(_heap: &mut crate::core::heap::Heap, _lang: &str) -> LispResult {
    Ok(crate::core::value::Value::Nil)
}

/// `(%tree-sitter-languages)` — the languages this runtime can parse right now: every
/// grammar loaded at runtime, declared and not yet loaded, or built in, sorted. The question
/// an editor asks before it offers to fetch a grammar, or tells you a file's mode has none:
/// without it the only way to find out was to parse something and read the error. A
/// declared grammar counts — asking must not load it — and one whose load failed does not,
/// unless something else still parses its language.
#[cfg(feature = "treesit")]
pub fn languages(heap: &mut Heap) -> LispResult {
    #[allow(unused_mut)] // a build with no grammar feature never pushes a built-in
    let mut names: Vec<String> = crate::core::sync::lock(&DYNAMIC).keys().cloned().collect();
    names.extend(
        crate::core::sync::lock(&DECLARED)
            .iter()
            .filter(|(_, declared)| matches!(declared, Declared::Pending { .. }))
            .map(|(name, _)| name.clone()),
    );
    #[cfg(feature = "treesit-ruby")]
    names.push("ruby".to_string());
    #[cfg(feature = "treesit-elixir")]
    names.push("elixir".to_string());
    names.sort();
    names.dedup();
    let keywords = names
        .iter()
        .map(|name| Value::keyword(crate::core::value::intern(name)))
        .collect();
    Ok(heap.alloc_vector(keywords))
}

#[cfg(not(feature = "treesit"))]
pub fn languages(heap: &mut crate::core::heap::Heap) -> LispResult {
    Ok(heap.alloc_vector(Vec::new()))
}

#[cfg(not(feature = "treesit"))]
pub fn load_grammar(_path: &str, lang: &str) -> LispResult {
    Err(LispError::runtime(format!(
        "%tree-sitter-load-grammar::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

/// The grammar for a language keyword's name: a declared one first (loading it on this, its
/// first use), then one loaded at runtime, then one built in.
#[cfg(feature = "treesit")]
fn language_for(lang: &str) -> Result<tree_sitter::Language, LispError> {
    // A declaration is the newest word on what `lang`'s grammar is — a pending one is a
    // load or a reload waiting for its first use.
    if let Some(language) = declared_language(lang)? {
        return Ok(language);
    }
    // A grammar loaded at runtime wins over a bundled one of the same name: loading one is a
    // deliberate act, and the reason to do it is usually that the bundled grammar is behind.
    if let Some(language) = dynamic_language(lang) {
        return Ok(language);
    }
    built_in_language(lang).ok_or_else(|| {
        LispError::runtime(format!(
            "%tree-sitter-parse:no grammar for :{lang} — declare one with \
             (%tree-sitter-declare-grammar \"/path/to/libtree-sitter-{lang}.so\" :{lang}) \
             or load it now with %tree-sitter-load-grammar, or build it in with \
             --features treesit-{lang}"
        ))
    })
}

/// The grammar compiled into this runtime for `lang`, if any. Each arm is gated on its own
/// `treesit-<lang>` feature — the kernel ships no grammar by default (a `treesit` build with
/// no grammar feature has zero arms and reports every language as not built in). One cfg'd
/// arm per language is the single place a grammar plugs in (plus its `Cargo.toml` dep).
#[cfg(feature = "treesit")]
fn built_in_language(lang: &str) -> Option<tree_sitter::Language> {
    match lang {
        #[cfg(feature = "treesit-ruby")]
        "ruby" => Some(tree_sitter_ruby::LANGUAGE.into()),
        #[cfg(feature = "treesit-elixir")]
        "elixir" => Some(tree_sitter_elixir::LANGUAGE.into()),
        _ => None,
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
    if let Some(p) = crate::core::sync::lock(&PARSER_POOL)
        .get_mut(lang)
        .and_then(Vec::pop)
    {
        return Ok(p);
    }
    let mut p = tree_sitter::Parser::new();
    p.set_language(language)
        .map_err(|e| LispError::runtime(format!("%tree-sitter-parse:{lang}: {e}")))?;
    Ok(p)
}

/// A parse's progress callback (ADR-491): cancel when the calling process has an interrupt
/// due. A full parse of a large file is the longest single builtin an editor makes (half a
/// second at 250 KB), and the one place a C-g most needs to reach inside one; tree-sitter
/// asks often, and the answer is one thread-local borrow and a relaxed load.
#[cfg(feature = "treesit")]
fn interruptible(_: &tree_sitter::ParseState) -> std::ops::ControlFlow<()> {
    if crate::process::current_interrupt_due() {
        std::ops::ControlFlow::Break(())
    } else {
        std::ops::ControlFlow::Continue(())
    }
}

/// Why a parse returned no tree: an interrupt cancelled it (raised as one), else a failure.
#[cfg(feature = "treesit")]
fn parse_failed(lang: &str) -> LispError {
    match crate::process::take_current_interrupt() {
        Some(by_ceiling) => crate::eval::interrupted_error(by_ceiling),
        None => LispError::runtime(format!("tree-sitter: {lang}: parse failed")),
    }
}

/// Return a parser to the pool for re-use (dropping it if the pool is full).
#[cfg(feature = "treesit")]
fn return_parser(lang: &str, mut parser: tree_sitter::Parser) {
    // a parse an interrupt cancelled would otherwise RESUME on this parser's next use
    parser.reset();
    // Poison-tolerant like every lock here: a parse that panicked must not make every
    // later parse in the runtime panic too.
    let mut pool = crate::core::sync::lock(&PARSER_POOL);
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
    // source file). Only the root carries it: a caller wanting per-node state has
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
    Err(LispError::runtime(format!(
        "%tree-sitter-parse::{lang}: this runtime was built without tree-sitter \
         (rebuild with --features treesit)"
    )))
}

#[cfg(not(feature = "treesit"))]
pub fn chain(heap: &mut crate::core::heap::Heap, src: &str, lang: &str, _o: i64) -> LispResult {
    parse(heap, src, lang)
}

#[cfg(not(feature = "treesit"))]
pub fn kids(
    heap: &mut crate::core::heap::Heap,
    src: &str,
    lang: &str,
    _o: i64,
    _all: bool,
) -> LispResult {
    parse(heap, src, lang)
}

#[cfg(not(feature = "treesit"))]
pub fn errors(heap: &mut crate::core::heap::Heap, src: &str, lang: &str) -> LispResult {
    parse(heap, src, lang)
}

#[cfg(all(test, feature = "treesit-grammars"))]
mod parse_cache_tests {
    use super::*;

    fn cached_texts() -> Vec<String> {
        crate::core::sync::lock(&TREE_CACHE)
            .iter()
            .map(|(_, src, _)| src.clone())
            .collect()
    }

    /// An editor parses one document in two shapes: the indenter the whole text, the view
    /// the window on screen. A keystroke's whole-text parse must replace the whole text's
    /// OWN older entry and leave the window's, so the window's next parse is incremental
    /// too. With one entry per language, each shape evicted the other and every keystroke
    /// paid a full reparse.
    #[test]
    fn an_edit_replaces_its_own_document_and_keeps_the_other_shape() {
        crate::core::sync::lock(&TREE_CACHE).clear();
        let document: String = (0..200)
            .map(|i| format!("def method_{i}\n  {i}\nend\n"))
            .collect();
        let window = document[1000..2000].to_string();
        let edited = format!("{}x{}", &document[..3000], &document[3000..]);

        parse_cached(&document, "ruby").expect("document parses");
        parse_cached(&window, "ruby").expect("window parses");
        parse_cached(&edited, "ruby").expect("edited document parses");

        let texts = cached_texts();
        assert!(
            texts.contains(&window),
            "the window's tree was evicted by the document's edit"
        );
        assert!(texts.contains(&edited), "the edited document is cached");
        assert!(
            !texts.contains(&document),
            "the document's older version is replaced, not kept"
        );
    }
}

#[cfg(all(test, feature = "treesit"))]
mod tree_handle_edit_tests {
    use super::*;

    /// Two edits in sequence compose into the one edit that covers both, in the original
    /// text's coordinates for its start and old end and the final text's for its new end.
    #[test]
    fn edits_in_sequence_compose_into_one_covering_edit() {
        // "abcdef" -> "aXYZdef" (bc -> XYZ) -> "aXYZdeQf" (insert Q before f)
        assert_eq!(compose_char_edits(&[(1, 3, 4), (6, 6, 7)]), Some((1, 5, 7)));
        // an insert BEFORE the first edit widens the start, not the end
        assert_eq!(compose_char_edits(&[(1, 3, 4), (0, 0, 1)]), Some((0, 3, 5)));
        // a delete overlapping the first edit's front
        assert_eq!(compose_char_edits(&[(1, 3, 4), (0, 2, 0)]), Some((0, 3, 2)));
        assert_eq!(compose_char_edits(&[]), None);
    }

    /// The rope diff finds one contiguous change exactly, across chunk boundaries, and
    /// widens it to whole characters when the change is inside a multi-byte one.
    #[test]
    fn a_rope_diff_finds_the_change_and_never_splits_a_character() {
        let base: String = "fn main() { let x = 1; }\n".repeat(400);
        let old = ropey::Rope::from_str(&base);
        assert!(
            old.chunks().count() > 1,
            "the test must cross a chunk boundary"
        );
        let mut edited = base.clone();
        edited.insert_str(5000, "yy");
        let new = ropey::Rope::from_str(&edited);
        assert_eq!(derive_rope_edit(&old, &new), Some((5000, 5000, 5002)));
        assert_eq!(derive_rope_edit(&old, &old.clone()), None);
        // é (C3 A9) -> è (C3 A8): the bytes differ only in the second, the edit is the char
        let accented_old = ropey::Rope::from_str("caf\u{e9}!");
        let accented_new = ropey::Rope::from_str("caf\u{e8}!");
        assert_eq!(
            derive_rope_edit(&accented_old, &accented_new),
            Some((3, 5, 5))
        );
    }
}

#[cfg(all(test, feature = "treesit-grammars"))]
mod tree_handle_tests {
    use super::*;

    fn spans_of(handle: u64, from: i64, to: i64) -> Vec<(i64, i64)> {
        let mut heap = Heap::new();
        let table = SpanTable::of_kinds(vec!["string".to_string(), "comment".to_string()]);
        let v = tree_spans(&mut heap, handle, &table, false, from, to, None).expect("spans");
        heap.seq_items(v)
            .expect("a vector")
            .into_iter()
            .map(|span| {
                let parts = heap.seq_items(span).expect("a span");
                match (parts[0], parts[1]) {
                    (Value::Int(a), Value::Int(b)) => (a, b),
                    _ => panic!("span offsets are ints"),
                }
            })
            .collect()
    }

    /// A reparse from an edit — explicit or derived — colours exactly what a fresh parse of
    /// the new text does, and the old handle still answers for the old text.
    #[test]
    fn an_incremental_reparse_matches_a_fresh_parse() {
        let source: String = (0..300)
            .map(|i| format!("def m{i}\n  x = \"s{i}\" # c{i}\nend\n"))
            .collect();
        let old_text = ropey::Rope::from_str(&source);
        let owner = crate::process::self_pid();
        let old = tree_open(old_text.clone(), "ruby", owner).expect("parses");
        // open a string above the window: everything after it is a string now
        let mut new_text = old_text.clone();
        new_text.insert(4, "\"");
        let length = new_text.len_chars() as i64;
        let fresh = tree_open(new_text.clone(), "ruby", owner).expect("parses");
        let derived = tree_reparse(old, new_text.clone(), "ruby", None, owner).expect("reparses");
        let explicit = tree_reparse(old, new_text.clone(), "ruby", Some(vec![(4, 4, 5)]), owner)
            .expect("reparses");
        let window = (length / 2, length / 2 + 400);
        assert_eq!(
            spans_of(derived, window.0, window.1),
            spans_of(fresh, window.0, window.1)
        );
        assert_eq!(
            spans_of(explicit, window.0, window.1),
            spans_of(fresh, window.0, window.1)
        );
        // the old handle is untouched by the reparse
        assert_eq!(spans_of(old, 0, 40), {
            let again = tree_open(old_text, "ruby", owner).expect("parses");
            spans_of(again, 0, 40)
        });
        // an edit that does not fit the texts is refused
        assert!(tree_reparse(old, new_text, "ruby", Some(vec![(4, 4, 9)]), owner).is_err());
        assert!(tree_free(fresh));
        let mut heap = Heap::new();
        let empty = SpanTable::default();
        let gone = tree_spans(&mut heap, fresh, &empty, false, 0, 10, None).expect("answers");
        assert!(matches!(gone, Value::Nil), "a freed handle reads as nil");
    }

    /// ADR-495: a handle is its owner's. Another process opening any number of trees cannot
    /// take it (the old 32-entry LRU dropped it at the 33rd), and its owner's exit frees it
    /// and only it. The owners are pids no process will ever have, so nothing else in
    /// the runtime frees them under the test.
    #[test]
    fn a_handle_survives_other_owners_and_dies_with_its_own() {
        let mine = u64::MAX - 1;
        let theirs = u64::MAX - 2;
        let text = ropey::Rope::from_str("def a\n  1\nend\n");
        let kept = tree_open(text.clone(), "ruby", mine).expect("parses");
        let others: Vec<u64> = (0..40)
            .map(|_| tree_open(text.clone(), "ruby", theirs).expect("parses"))
            .collect();
        assert!(
            tree_alive(kept),
            "40 trees opened by another owner left mine alone"
        );
        // and an edit from it is incremental: a gone handle would reparse fresh (`None`)
        let mut edited = text.clone();
        edited.insert(9, "2");
        let next = tree_reparse(kept, edited, "ruby", None, mine).expect("reparses");
        assert!(tree_entry(next).expect("held").changed.is_some());
        close_process_trees(theirs);
        assert!(others.iter().all(|handle| !tree_alive(*handle)));
        assert!(
            tree_alive(kept) && tree_alive(next),
            "another owner's exit is not mine"
        );
        // only the owner gives a tree away; given to a dead process, it dies with it
        assert_eq!(tree_give(kept, theirs, mine), Err(GiveRefused::NotOwner));
        assert_eq!(tree_give(kept, mine, theirs), Ok(false));
        assert!(!tree_alive(kept));
        close_process_trees(mine);
        assert!(!tree_alive(next));
    }
}

/// Declared grammars (ADR-481). Each test uses a language name of its own: the tables are
/// process-wide and the tests run in parallel. A real library's first-use load is covered by
/// `tests/tree_sitter_test.blsp`, which has one to load when a C compiler built it.
#[cfg(all(test, feature = "treesit"))]
mod declared_grammar_tests {
    use super::*;

    fn status(lang: &str) -> Value {
        let mut heap = Heap::new();
        grammar_status(&mut heap, lang).expect("status answers")
    }

    fn is_keyword(candidate: Value, name: &str) -> bool {
        matches!(candidate, Value::Keyword(symbol) if symbol == value::intern(name))
    }

    #[test]
    fn declaring_opens_nothing() {
        let lang = "declared-probe-unused";
        declare_grammar("/nonexistent/libtree-sitter-unused.so", lang).expect("declares");
        assert!(is_keyword(status(lang), "declared"));
        assert!(
            dynamic_language(lang).is_none(),
            "a declaration loaded its library before anything asked for it"
        );
        let mut heap = Heap::new();
        let listed = languages(&mut heap).expect("lists");
        let Value::Vector(listed) = listed else {
            panic!("languages answers a vector");
        };
        let listed = heap.vector(listed).to_vec();
        assert!(
            listed.iter().any(|item| is_keyword(*item, lang)),
            "a declared grammar is one the runtime can parse"
        );
    }

    #[test]
    fn a_library_that_does_not_load_is_a_failure_and_is_not_retried() {
        let lang = "declared-probe-missing";
        let path = "/nonexistent/libtree-sitter-missing.so";
        declare_grammar(path, lang).expect("declares");
        let error = parse_cached("x", lang).expect_err("nothing parses this language");
        assert!(error.message.contains("did not load"), "{}", error.message);
        assert!(error.message.contains(path), "{}", error.message);
        let mut heap = Heap::new();
        let reported = grammar_status(&mut heap, lang).expect("status answers");
        assert_eq!(
            heap.failure_message(reported).as_deref(),
            Some(error.message.as_str()),
            "the status carries the load's message as a failure value"
        );
        // Kept, not retried: the second use answers the same message from the table.
        assert!(matches!(
            crate::core::sync::lock(&DECLARED).get(lang),
            Some(Declared::Failed { .. })
        ));
        let again = parse_cached("x", lang).expect_err("still nothing parses it");
        assert_eq!(again.message, error.message);
        // A new declaration is a new attempt.
        declare_grammar(path, lang).expect("declares again");
        assert!(is_keyword(status(lang), "declared"));
    }

    #[test]
    fn a_failed_eager_load_leaves_the_declaration() {
        let lang = "declared-probe-eager";
        declare_grammar("/nonexistent/libtree-sitter-eager.so", lang).expect("declares");
        // The eager load fails too (no library), and a failed eager load changes nothing.
        assert!(load_grammar("/nonexistent/libtree-sitter-eager.so", lang).is_err());
        assert!(is_keyword(status(lang), "declared"));
    }

    #[cfg(feature = "treesit-ruby")]
    #[test]
    fn a_failed_declaration_keeps_the_grammar_the_language_had() {
        // The declaration is ruby's own (a re-declaration of a built-in language), so it is
        // withdrawn at the end; until then, a parallel ruby test meets the failed declaration
        // and parses with the built-in grammar, which is the behaviour under test.
        declare_grammar("/nonexistent/libtree-sitter-ruby.so", "ruby").expect("declares");
        let tree = parse_cached("def a; end\n", "ruby").expect("the built-in grammar parses");
        assert_eq!(tree.root_node().kind(), "program");
        let mut heap = Heap::new();
        let reported = grammar_status(&mut heap, "ruby").expect("status answers");
        assert!(heap.failure_message(reported).is_some());
        crate::core::sync::lock(&DECLARED).remove("ruby");
        assert!(is_keyword(status("ruby"), "built-in"));
    }
}

#[cfg(all(test, feature = "treesit"))]
mod injection_piece_tests {
    use super::*;

    /// ADR-496: an excluded range is cut out of every piece it reaches, wholly or in part.
    #[test]
    fn excluded_ranges_are_cut_out_of_the_pieces() {
        let pieces = vec![(0, 10), (20, 30)];
        assert_eq!(without_ranges(pieces.clone(), &[]), pieces);
        assert_eq!(
            without_ranges(pieces.clone(), &[(2, 4), (8, 22), (29, 40)]),
            vec![(0, 2), (4, 8), (22, 29)]
        );
        assert_eq!(without_ranges(pieces, &[(0, 30)]), Vec::new());
    }

    /// ADR-496: the last line break of each gap joins the pieces it separates, so the
    /// embedded parser still reads two lines as two lines (the break touches both pieces, so
    /// the three merge); a gap without one adds nothing.
    #[test]
    fn a_gap_with_a_line_break_parses_with_it() {
        let text = ropey::Rope::from_str("echo a\necho b <<X y\n");
        assert_eq!(
            with_line_breaks(&text, &[(0, 6), (7, 13), (17, 19)]),
            vec![(0, 13), (17, 19)]
        );
    }
}
