//! Another language's document inside this one (ADR-489): a heredoc, a `RUN` line. The rules
//! are data from Brood; this finds each embedded document, parses it and lays its spans over
//! the host's.

use super::*;

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
    let mut previous_named = None;
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
            previous: previous_named,
            outer: Some(context),
        };
        if child.is_named() {
            named_index += 1;
            previous_named = Some(child);
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
    let mut previous_named = None;
    loop {
        let child = cursor.node();
        let child_context = NodeContext {
            parent: Some(node),
            field: cursor.field_name(),
            nth: child.is_named().then_some(named_index),
            previous: previous_named,
            outer: Some(context),
        };
        if child.is_named() {
            named_index += 1;
            previous_named = Some(child);
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
    tokens: TokenRules,
    window: Option<(usize, usize)>,
) -> Option<Vec<RawSpan>> {
    let tree = parse_pieces(text, lang, &with_line_breaks(text, pieces))?;
    let query = SpanQuery {
        table,
        tokens,
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
pub(super) fn splice_injections<T: SpanText>(
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
        let Some(spans) = injected_spans(text, &lang, &pieces, table, query.tokens, query.window)
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
    tokens: TokenRules,
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
        tokens: TokenRules,
    ) -> Value {
        let pieces = bounded_byte_ranges(text, ranges);
        if pieces.is_empty() {
            return if language_for(lang).is_ok() {
                heap.alloc_vector(Vec::new())
            } else {
                Value::Nil
            };
        }
        match injected_spans(text, lang, &pieces, table, tokens, None) {
            Some(mut raw) => {
                raw.sort_by_key(|(start, end, _)| (*start, *end));
                raw_spans_value(heap, text, raw)
            }
            None => Value::Nil,
        }
    }
    Ok(match text {
        RangeText::Rope(rope) => answer(heap, &rope, lang, ranges, table, tokens),
        RangeText::Flat(src) => {
            let b2c = byte_to_char_offsets(&src);
            let flat = FlatText {
                src: &src,
                b2c: &b2c,
            };
            answer(heap, &flat, lang, ranges, table, tokens)
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
pub(super) fn widen_to_documents(
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
