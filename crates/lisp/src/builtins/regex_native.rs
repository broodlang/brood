//! Native regular-expression matching: the MECHANISM under `std/regex` (ADR-389).
//!
//! `std/regex.blsp` owns the pattern language — its parser reads Brood's dialect (a stray
//! `{` or `*` is the character it looks like, `\n` is an `n`, `.` crosses newlines) and
//! translates it into an unambiguous pattern for `regex-automata`. These primitives only
//! run what the translation produced. So what a pattern MEANS is Brood, and redefinable;
//! how fast it runs is this file. The engine in Brood that this replaces cost about half a
//! millisecond to try a four-pattern table on one short line, and an editor tries such a
//! table on every visible line.
//!
//! Two match semantics, both needed:
//!
//! - **leftmost-first** (Perl's) for `find` / `find-all` / `match?`: of the matches that
//!   start leftmost, the one the pattern's own alternation order and greediness prefer.
//! - **longest** for the lexer (`tokens`, `paint`): an ANCHORED search under
//!   `MatchKind::All` reports the longest match at that position, which is the rule a
//!   lexer runs on — `<=` is one token, not `<` then `=`.
//!
//! The primitives answer the language's own values — the match maps `regex/find` returns,
//! the token maps `regex/tokens` returns — rather than offsets for Brood to build them
//! from. Measured: the engine found a match in 0.8 µs and building its map in Brood took
//! another 6, so a primitive that answered offsets gave most of the speed-up back.
//!
//! **Offsets are characters, not bytes**, like every string builtin: the engine answers in
//! UTF-8 byte offsets and they are converted with the string's own char index (O(1) for
//! ASCII) before anything reaches the language.
//!
//! Compiled patterns are cached by their text, process-wide. A `meta::Regex` is
//! `Sync` and keeps a pool of per-thread search caches, so the map hands out the SAME `Arc`
//! to every worker thread — a fresh `Regex` per call would rebuild its lazy DFA from
//! nothing each time, which is most of the cost this module exists to remove.

use super::numeric::{arg, expect_int, expect_string_ref};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, StrId, Value};
use crate::error::{LispError, LispResult};
use regex_automata::meta::Regex;
use regex_automata::util::captures::Captures;
use regex_automata::{Anchored, Input, MatchKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%regex-match?",
        Arity::exact(2),
        Sig::new(vec![string, string], bool_ty),
        &["pattern", "s"],
        "True when the regex-automata pattern matches anywhere in s. The pattern is the engine's own syntax — std/regex translates Brood's dialect into it; call regex/match? rather than this.",
        regex_match,
    );
    primitives.def(
        "%regex-find",
        Arity::exact(3),
        Sig::new(vec![string, string, int], map_or_nil),
        &["pattern", "s", "from"],
        "The leftmost-first match of the regex-automata pattern in s at or after char index from, as {:start :end :text :groups [...]} — CHAR offsets, each group {:start :end :text} or nil when it did not take part — or nil for no match. \\A still means the start of s, not from. regex/find is this over the translated pattern.",
        regex_find,
    );
    primitives.def(
        "%regex-find-all",
        Arity::exact(2),
        Sig::new(vec![string, string], vec_ty),
        &["pattern", "s"],
        "Every non-overlapping match of the regex-automata pattern in s, left to right, as a vector of the maps %regex-find returns. After an empty match the scan steps one character on, so a pattern that can match nothing terminates — and a match may begin where the previous one ended, even an empty one (a* over \"baa\" is three matches: \"\" \"aa\" \"\").",
        regex_find_all,
    );
    primitives.def(
        "%regex-tokens",
        Arity::exact(3),
        Sig::new(vec![vec_ty, vec_ty, string], vec_ty),
        &["patterns", "tags", "s"],
        "Lex s with a vector of regex-automata patterns, the token of rule k tagged (nth tags k): a vector of {:start :end :text :tag}, CHAR offsets. At the earliest position any rule has a non-empty match, the first rule in order that has one wins and takes its LONGEST match there; the scan resumes at its end, and a position no rule matches is skipped. regex/tokens is this over the translated rules.",
        regex_tokens,
    );
    primitives.def(
        "%regex-paint",
        Arity::exact(5),
        Sig::new(vec![string, string, string, any, string], vec_or_nil),
        &["prefix", "pattern", "suffix", "lazy?", "s"],
        "[start end] (CHAR offsets) of the text pattern matches between a prefix matched at the start of s and a suffix matched after it, or nil: the prefix's longest match first, then pattern's longest end (its shortest, with lazy?) after which the suffix matches, each backtracked when the rest cannot follow. regex/paint is this.",
        regex_paint,
    );
}

/// The most compiled patterns kept at once. Patterns in a program are nearly all literals,
/// so this is never reached by one; a program that BUILDS patterns (a search box) would
/// otherwise grow the map forever. Reaching it clears the map — the next calls recompile
/// what they use, which is cheaper than any bookkeeping a finer eviction would need.
const CACHE_LIMIT: usize = 4096;

/// Which semantics a compiled pattern runs under — and so which cache holds it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    LeftmostFirst = 0,
    Longest = 1,
}

/// One map per [`Kind`], keyed by the pattern alone, so a lookup borrows the caller's `&str`
/// instead of allocating a key on every call.
type Cache = HashMap<Box<str>, Arc<Regex>>;

fn cache(kind: Kind) -> &'static Mutex<Cache> {
    static CACHES: OnceLock<[Mutex<Cache>; 2]> = OnceLock::new();
    let caches = CACHES.get_or_init(|| [Mutex::new(HashMap::new()), Mutex::new(HashMap::new())]);
    &caches[kind as usize]
}

/// The compiled form of `pattern` under `kind`, from the cache or compiled now. Compiling
/// happens outside the lock: two threads meeting the same new pattern both compile it and
/// one insert wins, which is cheaper than holding every other thread for the compile.
fn compiled(who: &str, pattern: &str, kind: Kind) -> Result<Arc<Regex>, LispError> {
    if let Some(hit) = cache(kind)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(pattern)
    {
        return Ok(Arc::clone(hit));
    }
    let match_kind = match kind {
        Kind::LeftmostFirst => MatchKind::LeftmostFirst,
        Kind::Longest => MatchKind::All,
    };
    let fresh = Arc::new(
        Regex::builder()
            .configure(Regex::config().match_kind(match_kind))
            .build(pattern)
            .map_err(|e| LispError::runtime(format!("{who}: cannot compile {pattern:?}: {e}")))?,
    );
    let mut map = cache(kind).lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= CACHE_LIMIT {
        map.clear();
    }
    map.insert(pattern.into(), Arc::clone(&fresh));
    Ok(fresh)
}

/// A string argument's heap id — what the char↔byte conversions take.
fn string_id(heap: &Heap, who: &str, v: Value) -> Result<StrId, LispError> {
    match v {
        Value::Str(id) => Ok(id),
        other => Err(LispError::wrong_type(heap, who, "string", other)),
    }
}

/// A pattern argument, compiled.
fn pattern_arg(heap: &Heap, who: &str, v: Value, kind: Kind) -> Result<Arc<Regex>, LispError> {
    let pattern = expect_string_ref(heap, who, v)?;
    compiled(who, &pattern, kind)
}

/// The byte just past the character that starts at byte `at` of `s` (one past the end
/// when `at` is the end) — how a scan steps over a position without splitting a char.
fn next_char(s: &str, at: usize) -> usize {
    match s[at..].chars().next() {
        Some(c) => at + c.len_utf8(),
        None => s.len() + 1,
    }
}

/// The end of the LONGEST match of `rx` (compiled [`Kind::Longest`]) anchored at byte
/// `at` and ending at or before byte `limit`, or `None`. Look-around still sees the whole
/// of `s`, so `\z` matches only at its real end, never at `limit`.
fn longest_at(rx: &Regex, s: &str, at: usize, limit: usize) -> Option<usize> {
    let input = Input::new(s).range(at..limit).anchored(Anchored::Yes);
    rx.search_half(&input).map(|h| h.offset())
}

/// Every end of a match of `rx` anchored at byte `at`, LONGEST first: the longest, then
/// the longest that stops short of it, and so on down to the empty match if there is one.
fn ends_at(rx: &Regex, s: &str, at: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut limit = s.len();
    while let Some(end) = longest_at(rx, s, at, limit) {
        ends.push(end);
        if end == at {
            break;
        }
        // the longest end strictly before this one: shrink the window by one character
        limit = s[..end].char_indices().next_back().map_or(at, |(b, _)| b);
    }
    ends
}

/// A span found in string `id`, held as plain data until the heap is free to allocate:
/// the text is copied out while the string is borrowed, the offsets converted later.
struct Span {
    start: usize,
    end: usize,
    text: String,
}

fn span(s: &str, start: usize, end: usize) -> Span {
    Span {
        start,
        end,
        text: s[start..end].to_owned(),
    }
}

/// One match: group 0, then each group or `None` when it did not take part.
fn match_spans(s: &str, caps: &Captures) -> Vec<Option<Span>> {
    (0..caps.group_len())
        .map(|i| caps.get_group(i).map(|g| span(s, g.start, g.end)))
        .collect()
}

fn kw(name: &str) -> Value {
    Value::keyword(value::intern(name))
}

/// `{:start :end :text}` for a span, its byte offsets converted to characters.
fn span_pairs(heap: &mut Heap, id: StrId, sp: &Span) -> Vec<(Value, Value)> {
    let text = heap.alloc_string(&sp.text);
    vec![
        (
            kw("start"),
            Value::Int(heap.str_byte_to_char(id, sp.start) as i64),
        ),
        (
            kw("end"),
            Value::Int(heap.str_byte_to_char(id, sp.end) as i64),
        ),
        (kw("text"), text),
    ]
}

/// The map `regex/find` answers: group 0's span, and `:groups` holding the rest.
fn match_value(heap: &mut Heap, id: StrId, spans: &[Option<Span>]) -> Value {
    let groups: Vec<Value> = spans[1..]
        .iter()
        .map(|g| match g {
            Some(sp) => {
                let pairs = span_pairs(heap, id, sp);
                heap.map_from_pairs(pairs)
            }
            None => Value::Nil,
        })
        .collect();
    let groups = heap.alloc_vector(groups);
    let whole = spans[0].as_ref().expect("group 0 is the match");
    let mut pairs = span_pairs(heap, id, whole);
    pairs.push((kw("groups"), groups));
    heap.map_from_pairs(pairs)
}

/// `(%regex-match? pattern s)`
fn regex_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let rx = pattern_arg(heap, "%regex-match?", arg(args, 0), Kind::LeftmostFirst)?;
    let s = expect_string_ref(heap, "%regex-match?", arg(args, 1))?;
    Ok(Value::Bool(rx.is_match(&*s)))
}

/// `(%regex-find pattern s from)`
fn regex_find(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let rx = pattern_arg(heap, "%regex-find", arg(args, 0), Kind::LeftmostFirst)?;
    let id = string_id(heap, "%regex-find", arg(args, 1))?;
    let from = expect_int(heap, "%regex-find", arg(args, 2))?.max(0) as usize;
    let spans = {
        let start = heap.str_char_to_byte(id, from);
        let s = heap.string(id);
        let mut caps = rx.create_captures();
        rx.search_captures(&Input::new(&*s).range(start..), &mut caps);
        if !caps.is_match() {
            return Ok(Value::Nil);
        }
        match_spans(&s, &caps)
    };
    Ok(match_value(heap, id, &spans))
}

/// `(%regex-find-all pattern s)`
fn regex_find_all(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let rx = pattern_arg(heap, "%regex-find-all", arg(args, 0), Kind::LeftmostFirst)?;
    let id = string_id(heap, "%regex-find-all", arg(args, 1))?;
    // Not the engine's own iterator: it refuses an empty match that touches the end of
    // the previous one, and the language's `find-all` keeps it (see the docstring). So
    // the scan is the language's rule, one search per match.
    let all = {
        let s = heap.string(id);
        let mut caps = rx.create_captures();
        let mut all = Vec::new();
        let mut at = 0usize;
        while at <= s.len() {
            rx.search_captures(&Input::new(&*s).range(at..), &mut caps);
            let Some(whole) = caps.get_match() else { break };
            at = if whole.is_empty() {
                next_char(&s, whole.end())
            } else {
                whole.end()
            };
            all.push(match_spans(&s, &caps));
        }
        all
    };
    let items: Vec<Value> = all
        .iter()
        .map(|spans| match_value(heap, id, spans))
        .collect();
    Ok(heap.alloc_vector(items))
}

/// A compiled lexer: one multi-pattern search that finds where the next token could
/// start, and each rule on its own under longest semantics to decide which token it is.
struct Lexer {
    any_rule: Regex,
    rules: Vec<Arc<Regex>>,
}

fn lexers() -> &'static Mutex<HashMap<Vec<Box<str>>, Arc<Lexer>>> {
    static LEXERS: OnceLock<Mutex<HashMap<Vec<Box<str>>, Arc<Lexer>>>> = OnceLock::new();
    LEXERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lexer(patterns: Vec<Box<str>>) -> Result<Arc<Lexer>, LispError> {
    if let Some(hit) = lexers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&patterns)
    {
        return Ok(Arc::clone(hit));
    }
    let any_rule = Regex::new_many(&patterns).map_err(|e| {
        LispError::runtime(format!("%regex-tokens: cannot compile {patterns:?}: {e}"))
    })?;
    let rules = patterns
        .iter()
        .map(|p| compiled("%regex-tokens", p, Kind::Longest))
        .collect::<Result<Vec<_>, _>>()?;
    let fresh = Arc::new(Lexer { any_rule, rules });
    let mut map = lexers().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= CACHE_LIMIT {
        map.clear();
    }
    map.insert(patterns, Arc::clone(&fresh));
    Ok(fresh)
}

/// A vector argument's items.
fn vector_arg(heap: &Heap, who: &str, v: Value) -> Result<Vec<Value>, LispError> {
    match v {
        Value::Vector(id) => Ok(heap.vector(id).to_vec()),
        other => Err(LispError::wrong_type(heap, who, "vector", other)),
    }
}

/// `(%regex-tokens patterns tags s)`
fn regex_tokens(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let patterns: Vec<Box<str>> = vector_arg(heap, "%regex-tokens", arg(args, 0))?
        .into_iter()
        .map(|p| expect_string_ref(heap, "%regex-tokens", p).map(|s| Box::from(&*s)))
        .collect::<Result<_, _>>()?;
    let tags = vector_arg(heap, "%regex-tokens", arg(args, 1))?;
    if tags.len() != patterns.len() {
        return Err(LispError::runtime(format!(
            "%regex-tokens: {} patterns but {} tags",
            patterns.len(),
            tags.len()
        )));
    }
    let lex = lexer(patterns)?;
    let id = string_id(heap, "%regex-tokens", arg(args, 2))?;
    let tokens = {
        let s = heap.string(id);
        let mut tokens: Vec<(Span, usize)> = Vec::new();
        let mut at = 0usize;
        while at < s.len() {
            // the earliest place ANY rule matches; nothing before it can start a token
            let Some(hit) = lex.any_rule.search(&Input::new(&*s).range(at..)) else {
                break;
            };
            let p = hit.start();
            let token = lex.rules.iter().enumerate().find_map(|(k, rule)| {
                longest_at(rule, &s, p, s.len())
                    .filter(|&e| e > p)
                    .map(|e| (e, k))
            });
            match token {
                Some((end, k)) => {
                    tokens.push((span(&s, p, end), k));
                    at = end;
                }
                // only empty matches start here: no token, look from the next character
                None => at = next_char(&s, p),
            }
        }
        tokens
    };
    let items: Vec<Value> = tokens
        .iter()
        .map(|(sp, k)| {
            let mut pairs = span_pairs(heap, id, sp);
            pairs.push((kw("tag"), tags[*k]));
            heap.map_from_pairs(pairs)
        })
        .collect();
    Ok(heap.alloc_vector(items))
}

/// `(%regex-paint prefix pattern suffix lazy? s)`
fn regex_paint(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = pattern_arg(heap, "%regex-paint", arg(args, 0), Kind::Longest)?;
    let pattern = pattern_arg(heap, "%regex-paint", arg(args, 1), Kind::Longest)?;
    let suffix = pattern_arg(heap, "%regex-paint", arg(args, 2), Kind::Longest)?;
    let lazy = !matches!(arg(args, 3), Value::Nil | Value::Bool(false));
    let id = string_id(heap, "%regex-paint", arg(args, 4))?;
    let found = {
        let s = heap.string(id);
        if s.is_empty() {
            return Ok(Value::Nil);
        }
        ends_at(&prefix, &s, 0).into_iter().find_map(|p| {
            let mut ends = ends_at(&pattern, &s, p);
            if lazy {
                ends.reverse();
            }
            ends.into_iter()
                .find(|&e| longest_at(&suffix, &s, e, s.len()).is_some())
                .map(|e| (p, e))
        })
    };
    Ok(match found {
        Some((start, end)) => {
            let pair = vec![
                Value::Int(heap.str_byte_to_char(id, start) as i64),
                Value::Int(heap.str_byte_to_char(id, end) as i64),
            ];
            heap.alloc_vector(pair)
        }
        None => Value::Nil,
    })
}
