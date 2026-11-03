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
//! from: building the map in Brood costs several times the match itself, so a primitive
//! that answered offsets would give most of the speed-up back.
//!
//! **Offsets are characters, not bytes**, like every string builtin: the engine answers in
//! UTF-8 byte offsets and they are converted with the string's own char index (O(1) for
//! ASCII) before anything reaches the language.
//!
//! **Interrupts (ADR-491, ADR-493).** regex-automata has no cancellation: one `meta` search
//! cannot be stopped from inside. There is no backtracking — the compiled pattern is capped
//! (`meta`'s defaults: a 10 MiB NFA, a 2 MiB lazy-DFA cache per search) and a search is one
//! pass — but a pass over a large text is long. So every search polls, a different way per
//! pattern ([`find_polled`]): a pattern whose matches have a bounded length is searched a
//! window at a time (`Input::range`); an unbounded one (`a+`, `.*`) over a text longer than
//! a window is STEPPED — its lazy DFA driven a byte at a time by a loop of ours
//! ([`find_stepped`]), which polls between windows with the DFA's state in hand. Where
//! the lazy DFA cannot go — a Unicode word boundary before non-ASCII text — and for the
//! groups of a match longer than a window, a Pike VM of ours simulates the NFA with a poll
//! per position ([`pike_search`]). The loops OVER searches — every match of
//! `find-all`, every token, the shrinking windows of the longest-match walk (quadratic at
//! worst) — poll between searches.
//!
//! Compiled patterns are cached by their text, process-wide. A `meta::Regex` is
//! `Sync` and keeps a pool of per-thread search caches, so the map hands out the SAME `Arc`
//! to every worker thread — a fresh `Regex` per call would rebuild its lazy DFA from
//! nothing each time, which is most of the cost this module exists to remove.

use super::args::{arg, expect_int, expect_string_ref};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, StrId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::InterruptPoll;
use regex_automata::meta::Regex;
use regex_automata::nfa::thompson::{self, State};
use regex_automata::util::captures::Captures;
use regex_automata::util::prefilter::Prefilter;
use regex_automata::util::primitives::NonMaxUsize;
use regex_automata::util::primitives::StateID;
use regex_automata::{Anchored, Input, MatchKind};
use regex_automata::{HalfMatch, Match, hybrid};
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
pub(super) enum Kind {
    LeftmostFirst = 0,
    Longest = 1,
}

/// One map per [`Kind`], keyed by the pattern alone, so a lookup borrows the caller's `&str`
/// instead of allocating a key on every call.
type Cache = HashMap<Box<str>, Arc<Pattern>>;

/// A compiled pattern (or several, a lexer's `any_rule`), and the longest text one of its
/// matches can span — `None` when that is unbounded (`a+`). The bound is what lets a search
/// run a window at a time ([`find_polled`]); the engine does not expose it, so it is read
/// from the parsed pattern once, when the pattern is compiled. An unbounded pattern is
/// searched over a large text by the same patterns as a lazy DFA driven a byte at a time
/// (`stepped`, ADR-493), built the first time one is.
pub(super) struct Pattern {
    regex: Regex,
    longest: Option<usize>,
    sources: Box<[Box<str>]>,
    match_kind: MatchKind,
    stepped: OnceLock<Option<Stepped>>,
    nfa: OnceLock<Option<thompson::NFA>>,
}

impl Pattern {
    fn new(regex: Regex, sources: Box<[Box<str>]>, match_kind: MatchKind) -> Pattern {
        let longest = sources
            .iter()
            .map(|source| {
                regex_automata::util::syntax::parse(source)
                    .ok()
                    .and_then(|hir| hir.properties().maximum_len())
            })
            .try_fold(0, |longest, one| one.map(|one| longest.max(one)));
        Pattern {
            regex,
            longest,
            sources,
            match_kind,
            stepped: OnceLock::new(),
            nfa: OnceLock::new(),
        }
    }

    /// The lazy DFA a stepped search drives, or `None` when this pattern cannot be one (past
    /// the NFA cap `meta` has) and is searched as one `meta` search.
    fn stepped(&self) -> Option<&Stepped> {
        self.stepped
            .get_or_init(|| build_stepped(&self.sources, self.match_kind))
            .as_ref()
    }

    /// The NFA a [`pike_search`] simulates, or `None` past the NFA cap.
    fn nfa(&self) -> Option<&thompson::NFA> {
        self.nfa.get_or_init(|| build_nfa(&self.sources)).as_ref()
    }
}

/// A pattern as the lazy DFA a stepped search drives (ADR-493), and the search caches its
/// searches reuse: a cache is the DFA built so far, so a fresh one per search — `find-all`
/// makes one per match — would rebuild it from nothing each time. A search takes one from
/// the pool and puts it back, so there are only ever as many as searches running at once.
struct Stepped {
    regex: hybrid::regex::Regex,
    caches: Mutex<Vec<hybrid::regex::Cache>>,
}

impl Stepped {
    fn with_cache<T>(
        &self,
        run: impl FnOnce(&hybrid::regex::Regex, &mut hybrid::regex::Cache) -> T,
    ) -> T {
        let pooled = self.caches.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let mut cache = pooled.unwrap_or_else(|| self.regex.create_cache());
        let answer = run(&self.regex, &mut cache);
        self.caches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(cache);
        answer
    }
}

impl std::ops::Deref for Pattern {
    type Target = Regex;
    fn deref(&self) -> &Regex {
        &self.regex
    }
}

fn cache(kind: Kind) -> &'static Mutex<Cache> {
    static CACHES: OnceLock<[Mutex<Cache>; 2]> = OnceLock::new();
    let caches = CACHES.get_or_init(|| [Mutex::new(HashMap::new()), Mutex::new(HashMap::new())]);
    &caches[kind as usize]
}

/// The compiled form of `pattern` under `kind`, from the cache or compiled now. Compiling
/// happens outside the lock: two threads meeting the same new pattern both compile it and
/// one insert wins, which is cheaper than holding every other thread for the compile.
pub(super) fn compiled(who: &str, pattern: &str, kind: Kind) -> Result<Arc<Pattern>, LispError> {
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
    let regex = Regex::builder()
        .configure(Regex::config().match_kind(match_kind))
        .build(pattern)
        .map_err(|e| LispError::runtime(format!("{who}: cannot compile {pattern:?}: {e}")))?;
    let fresh = Arc::new(Pattern::new(regex, Box::new([pattern.into()]), match_kind));
    let mut map = cache(kind).lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= CACHE_LIMIT {
        map.clear();
    }
    map.insert(pattern.into(), Arc::clone(&fresh));
    Ok(fresh)
}

/// A string argument's heap id — what the char↔byte conversions take.
fn string_id(heap: &Heap, who: &str, v: Value) -> Result<StrId, LispError> {
    expect!(heap, who, v, "string",
        Value::Str(id) => id,
    )
}

/// A pattern argument, compiled.
fn pattern_arg(heap: &Heap, who: &str, v: Value, kind: Kind) -> Result<Arc<Pattern>, LispError> {
    let pattern = expect_string_ref(heap, who, v)?;
    compiled(who, pattern, kind)
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
/// of `s`, so `\z` matches only at its real end, never at `limit`. A bounded pattern's
/// anchored search dies within its bound; an unbounded one over a large text is stepped,
/// with a poll every window (ADR-493).
fn longest_at(
    rx: &Pattern,
    s: &str,
    at: usize,
    limit: usize,
    poll: &mut InterruptPoll,
) -> Result<Option<usize>, LispError> {
    let input = Input::new(s).range(at..limit).anchored(Anchored::Yes);
    if rx.longest.is_none()
        && limit - at > SEARCH_WINDOW
        && let Some(stepped) = rx.stepped()
        && let Some(found) = stepped.with_cache(|regex, cache| {
            step_forward(regex.forward(), cache.forward_mut(), &input, poll)
        })?
    {
        return Ok(found.map(|end| end.offset()));
    }
    // The lazy DFA quit (a Unicode word boundary before non-ASCII text): our Pike VM.
    if rx.longest.is_none()
        && limit - at > SEARCH_WINDOW
        && let Some(nfa) = rx.nfa()
    {
        let mut slots = vec![None; nfa.group_info().implicit_slot_len()];
        let found = pike_search(nfa, &input, true, &mut slots, poll)?;
        return Ok(found.map(|end| end.offset()));
    }
    Ok(rx.search_half(&input).map(|h| h.offset()))
}

/// Every end of a match of `rx` anchored at byte `at`, LONGEST first: the longest, then
/// the longest that stops short of it, and so on down to the empty match if there is one.
fn ends_at(rx: &Pattern, s: &str, at: usize) -> Result<Vec<usize>, LispError> {
    let mut ends = Vec::new();
    let mut limit = s.len();
    // one search per end: quadratic in the worst case, so it polls for an interrupt (ADR-491)
    let mut poll = InterruptPoll::new();
    while let Some(end) = longest_at(rx, s, at, limit, &mut poll)? {
        poll.advance(64)?;
        ends.push(end);
        if end == at {
            break;
        }
        // the longest end strictly before this one: shrink the window by one character
        limit = s[..end].char_indices().next_back().map_or(at, |(b, _)| b);
    }
    Ok(ends)
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

pub(super) fn kw(name: &str) -> Value {
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

/// Bytes of text one windowed or stepped search covers between interrupt polls (ADR-491).
pub(super) const SEARCH_WINDOW: usize = 64 * 1024;

/// The leftmost-first match of `rx` in `s` starting at or after byte `start` — `search`
/// over `s[start..]`, but with an interrupt poll every [`SEARCH_WINDOW`] bytes however the
/// pattern is shaped. A pattern whose matches have a bounded length is searched a window at
/// a time (ADR-491): a search confined to a window (`Input::range`, which still lets
/// look-around see the whole text) is the step the engine offers. A window takes the
/// leftmost match STARTING in its first `SEARCH_WINDOW` bytes: such a match is at most the
/// pattern's maximum length long, so the window reaches that far past them, and a match it
/// reports starting later is left for the next window, where it is whole. A pattern with no
/// bound (`a+`, `.*`) cannot be cut into windows — a match starting in one may end anywhere,
/// and a later-starting match inside the window would be taken for the leftmost — so over a
/// text longer than a window it is STEPPED (ADR-493, [`find_stepped`]) — or, where the lazy DFA
/// cannot go (a Unicode word boundary before non-ASCII text), searched by our polling Pike VM
/// ([`find_pike`]).
fn find_polled(
    rx: &Pattern,
    s: &str,
    start: usize,
    poll: &mut InterruptPoll,
) -> Result<Option<Match>, LispError> {
    let Some(longest) = rx.longest else {
        if s.len() - start > SEARCH_WINDOW
            && let Some(found) = match find_stepped(rx, s, start, poll)? {
                Some(found) => Some(found),
                None => find_pike(rx, s, start, poll)?,
            }
        {
            return Ok(found);
        }
        return Ok(rx.search(&Input::new(s).range(start..)));
    };
    let mut from = start;
    loop {
        let starts_end = char_boundary_up(s, from.saturating_add(SEARCH_WINDOW));
        let window_end = char_boundary_up(s, starts_end.saturating_add(longest));
        let found = rx.search(&Input::new(s).range(from..window_end));
        let last = window_end >= s.len();
        match found {
            Some(found) if found.start() < starts_end || last => return Ok(Some(found)),
            None if last => return Ok(None),
            _ => {}
        }
        poll.advance(InterruptPoll::EVERY)?;
        from = starts_end;
    }
}

/// [`find_polled`] with its groups, into `caps` ([`groups_polled`]).
fn search_captures_polled(
    rx: &Pattern,
    s: &str,
    start: usize,
    caps: &mut Captures,
    poll: &mut InterruptPoll,
) -> Result<(), LispError> {
    let Some(found) = find_polled(rx, s, start, poll)? else {
        caps.set_pattern(None);
        return Ok(());
    };
    groups_polled(rx, s, found, caps, poll)
}

/// The groups of `found`, a match of `rx` in `s`, into `caps`: the ones a search anchored at
/// the match and confined to it finds. The match is the highest-priority one starting there,
/// so confining the search to where it ends changes neither it nor its groups. A pattern with
/// no groups of its own needs no second pass; one with groups pays a pass over the match
/// itself, which for a match longer than a window is our polling Pike VM ([`pike_search`])
/// rather than `meta`'s, so it can be stopped too (ADR-493).
fn groups_polled(
    rx: &Pattern,
    s: &str,
    found: Match,
    caps: &mut Captures,
    poll: &mut InterruptPoll,
) -> Result<(), LispError> {
    if rx.group_info().group_len(found.pattern()) == 1 {
        caps.set_pattern(Some(found.pattern()));
        let slots = caps.slots_mut();
        slots[0] = NonMaxUsize::new(found.start());
        slots[1] = NonMaxUsize::new(found.end());
        return Ok(());
    }
    let input = Input::new(s)
        .range(found.range())
        .anchored(Anchored::Pattern(found.pattern()));
    if found.len() > SEARCH_WINDOW
        && let Some(nfa) = rx.nfa()
        && nfa.group_info().slot_len() == caps.slots_mut().len()
    {
        let groups = pike_search(nfa, &input, false, caps.slots_mut(), poll)?;
        caps.set_pattern(groups.map(|end| end.pattern()));
        return Ok(());
    }
    rx.search_captures(&input, caps);
    Ok(())
}

/// The smallest char boundary of `s` at or after byte `at`, clamped to its length.
fn char_boundary_up(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// The lazy DFA `pattern`'s stepped searches drive (ADR-493): the engine `meta` runs first,
/// built on its own so the search loop is ours. It never gives up on a full cache (a cache
/// cleared again and again is slow, not wrong, and the poll still runs), carries the
/// prefilter `meta` would use for a leftmost-first search, and quits on a Unicode word
/// boundary before non-ASCII text, which [`step_forward`] answers as "not stepped".
fn build_stepped(sources: &[Box<str>], match_kind: MatchKind) -> Option<Stepped> {
    let prefilter = if match_kind == MatchKind::LeftmostFirst {
        let hirs = sources
            .iter()
            .map(|source| regex_automata::util::syntax::parse(source).ok())
            .collect::<Option<Vec<_>>>()?;
        Prefilter::from_hirs_prefix(match_kind, &hirs).filter(|prefilter| prefilter.is_fast())
    } else {
        None
    };
    let specialize = prefilter.is_some();
    hybrid::regex::Regex::builder()
        .thompson(
            thompson::Config::new()
                .nfa_size_limit(Some(10 * (1 << 20)))
                .which_captures(thompson::WhichCaptures::None),
        )
        .dfa(
            hybrid::dfa::Config::new()
                .match_kind(match_kind)
                .prefilter(prefilter)
                .specialize_start_states(specialize)
                .unicode_word_boundary(true)
                .minimum_cache_clear_count(None),
        )
        .build_many(sources)
        .ok()
        .map(|regex| Stepped {
            regex,
            caches: Mutex::new(Vec::new()),
        })
}

/// The leftmost-first match of `rx` starting at or after byte `start`, found by driving its
/// lazy DFA ourselves (ADR-493): `Some(answer)`, or `None` when the DFA cannot answer (it
/// quit on a Unicode word boundary before non-ASCII text, or was never built) and the
/// caller runs the one `meta` search instead. regex-automata cannot stop a search inside,
/// but its lazy DFA is a state machine it lets a caller step — `next_state` per byte — so
/// the loop is ours, and the state crosses a poll unchanged. Forward to the match's END,
/// then backward from it to its START (the engine's own two passes, `hybrid::regex`), an
/// empty match inside a character skipped as `meta` skips it.
fn find_stepped(
    rx: &Pattern,
    s: &str,
    start: usize,
    poll: &mut InterruptPoll,
) -> Result<Option<Option<Match>>, LispError> {
    let Some(stepped) = rx.stepped() else {
        return Ok(None);
    };
    stepped.with_cache(|regex, cache| find_stepped_in(regex, cache, s, start, poll))
}

/// [`find_stepped`] with a search cache in hand.
fn find_stepped_in(
    regex: &hybrid::regex::Regex,
    cache: &mut hybrid::regex::Cache,
    s: &str,
    start: usize,
    poll: &mut InterruptPoll,
) -> Result<Option<Option<Match>>, LispError> {
    let (forward_cache, reverse_cache) = cache.as_parts_mut();
    let mut from = start;
    loop {
        if from > s.len() {
            return Ok(Some(None));
        }
        let input = Input::new(s).range(from..);
        let Some(found) = step_forward(regex.forward(), forward_cache, &input, poll)? else {
            return Ok(None);
        };
        let Some(end) = found else {
            return Ok(Some(None));
        };
        let end_at = end.offset();
        if !s.is_char_boundary(end_at) {
            // Only an empty match ends inside a character, and it is the leftmost: nothing
            // starts before it, so look again from the byte after it.
            from = end_at + 1;
            continue;
        }
        if end_at == from {
            return Ok(Some(Some(Match::new(end.pattern(), from..from))));
        }
        let backward = Input::new(s).range(from..end_at).anchored(Anchored::Yes);
        let Some(Some(begin)) = step_reverse(regex.reverse(), reverse_cache, &backward, poll)?
        else {
            return Ok(None);
        };
        return Ok(Some(Some(Match::new(
            end.pattern(),
            begin.offset()..end_at,
        ))));
    }
}

/// The leftmost-first match of `rx` starting at or after byte `start`, by our polling Pike
/// VM (ADR-493): where the lazy DFA cannot step, because it quit on a Unicode word boundary
/// before non-ASCII text. `None` when the NFA could not be built (`meta` answers instead).
/// An empty match inside a character is skipped, as in [`find_stepped`].
fn find_pike(
    rx: &Pattern,
    s: &str,
    start: usize,
    poll: &mut InterruptPoll,
) -> Result<Option<Option<Match>>, LispError> {
    let Some(nfa) = rx.nfa() else {
        return Ok(None);
    };
    let mut slots = vec![None; nfa.group_info().implicit_slot_len()];
    let mut from = start;
    loop {
        if from > s.len() {
            return Ok(Some(None));
        }
        let input = Input::new(s).range(from..);
        let Some(end) = pike_search(nfa, &input, false, &mut slots, poll)? else {
            return Ok(Some(None));
        };
        let pattern = end.pattern().as_usize();
        let (Some(begin), Some(finish)) = (slots[2 * pattern], slots[2 * pattern + 1]) else {
            return Ok(None);
        };
        let (begin, finish) = (begin.get(), finish.get());
        if begin == finish && !s.is_char_boundary(finish) {
            from = finish + 1;
            continue;
        }
        return Ok(Some(Some(Match::new(end.pattern(), begin..finish))));
    }
}

/// Drive `dfa` forward over `input`, a poll every [`SEARCH_WINDOW`] bytes: the end of the
/// match its match kind reports (leftmost-first, or the longest for an anchored `All`), as
/// `Some(found)`, or `None` when the DFA quit. `hybrid`'s own `find_fwd` minus its loop
/// unrolling: a match state is seen one byte late (a match entered on byte `at` ends at
/// `at`), the search goes on past a match until the DFA is dead, and in a start state with
/// nothing found the prefilter jumps to the next candidate — never past the window's end,
/// so a long stretch with no candidate still polls.
fn step_forward(
    dfa: &hybrid::dfa::DFA,
    cache: &mut hybrid::dfa::Cache,
    input: &Input<'_>,
    poll: &mut InterruptPoll,
) -> Result<Option<Option<HalfMatch>>, LispError> {
    let haystack = input.haystack();
    let end = input.end();
    let prefilter = if input.get_anchored().is_anchored() {
        None
    } else {
        dfa.get_config().get_prefilter()
    };
    let universal_start = dfa.get_nfa().look_set_prefix_any().is_empty();
    let Ok(mut state) = dfa.start_state_forward(cache, input) else {
        return Ok(None);
    };
    let mut found = None;
    let mut at = input.start();
    let mut window_end = at;
    loop {
        if at >= window_end {
            if at >= end {
                break;
            }
            if at > input.start() {
                poll.advance(InterruptPoll::EVERY)?;
            }
            window_end = at.saturating_add(SEARCH_WINDOW).min(end);
        }
        if let Some(prefilter) = prefilter
            && found.is_none()
            && state.is_start()
        {
            // A candidate starting before the window's end fits in this reach.
            let reach = window_end
                .saturating_add(prefilter.max_needle_len())
                .min(end);
            let next = prefilter
                .find(haystack, regex_automata::Span::from(at..reach))
                .map_or(window_end, |candidate| candidate.start.min(window_end));
            if next > at {
                at = next;
                if !universal_start {
                    let mut moved = input.clone();
                    moved.set_start(at);
                    let Ok(restart) = dfa.start_state_forward(cache, &moved) else {
                        return Ok(None);
                    };
                    state = restart;
                }
                continue;
            }
        }
        let Ok(next) = dfa.next_state(cache, state, haystack[at]) else {
            return Ok(None);
        };
        state = next;
        if state.is_tagged() {
            if state.is_match() {
                found = Some(HalfMatch::new(dfa.match_pattern(cache, state, 0), at));
            } else if state.is_dead() {
                return Ok(Some(found));
            } else if state.is_quit() {
                return Ok(None);
            }
        }
        at += 1;
    }
    // The end of input: the byte after it when the range stops short of the text, so
    // look-ahead sees what follows, as `meta` does.
    let last = match haystack.get(end) {
        Some(&byte) => dfa.next_state(cache, state, byte),
        None => dfa.next_eoi_state(cache, state),
    };
    let Ok(last) = last else {
        return Ok(None);
    };
    if last.is_match() {
        found = Some(HalfMatch::new(dfa.match_pattern(cache, last, 0), end));
    } else if last.is_quit() {
        return Ok(None);
    }
    Ok(Some(found))
}

/// Drive the REVERSE `dfa` (anchored, `MatchKind::All`) backward over `input` from its end,
/// a poll every [`SEARCH_WINDOW`] bytes: the leftmost start, as `Some(found)`, or `None` when
/// the DFA quit. `hybrid`'s `find_rev`: a match entered on byte `at` starts at `at + 1`, and
/// the byte before the range is look-behind.
fn step_reverse(
    dfa: &hybrid::dfa::DFA,
    cache: &mut hybrid::dfa::Cache,
    input: &Input<'_>,
    poll: &mut InterruptPoll,
) -> Result<Option<Option<HalfMatch>>, LispError> {
    let haystack = input.haystack();
    let start = input.start();
    let Ok(mut state) = dfa.start_state_reverse(cache, input) else {
        return Ok(None);
    };
    let mut found = None;
    let mut at = input.end();
    let mut until_poll = SEARCH_WINDOW;
    while at > start {
        at -= 1;
        let Ok(next) = dfa.next_state(cache, state, haystack[at]) else {
            return Ok(None);
        };
        state = next;
        if state.is_tagged() {
            if state.is_match() {
                found = Some(HalfMatch::new(dfa.match_pattern(cache, state, 0), at + 1));
            } else if state.is_dead() {
                return Ok(Some(found));
            } else if state.is_quit() {
                return Ok(None);
            }
        }
        until_poll -= 1;
        if until_poll == 0 {
            until_poll = SEARCH_WINDOW;
            poll.advance(InterruptPoll::EVERY)?;
        }
    }
    let last = match start.checked_sub(1) {
        Some(before) => dfa.next_state(cache, state, haystack[before]),
        None => dfa.next_eoi_state(cache, state),
    };
    let Ok(last) = last else {
        return Ok(None);
    };
    if last.is_match() {
        found = Some(HalfMatch::new(dfa.match_pattern(cache, last, 0), start));
    } else if last.is_quit() {
        return Ok(None);
    }
    Ok(Some(found))
}

/// `(%regex-match? pattern s)`
fn regex_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let rx = pattern_arg(heap, "%regex-match?", arg(args, 0), Kind::LeftmostFirst)?;
    let s = expect_string_ref(heap, "%regex-match?", arg(args, 1))?;
    let found = find_polled(&rx, s, 0, &mut InterruptPoll::new())?;
    Ok(Value::Bool(found.is_some()))
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
        search_captures_polled(&rx, s, start, &mut caps, &mut InterruptPoll::new())?;
        if !caps.is_match() {
            return Ok(Value::Nil);
        }
        match_spans(s, &caps)
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
        let mut poll = InterruptPoll::new();
        while at <= s.len() {
            poll.advance(16)?;
            search_captures_polled(&rx, s, at, &mut caps, &mut poll)?;
            let Some(whole) = caps.get_match() else { break };
            at = if whole.is_empty() {
                next_char(s, whole.end())
            } else {
                whole.end()
            };
            all.push(match_spans(s, &caps));
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
    any_rule: Pattern,
    rules: Vec<Arc<Pattern>>,
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
    let any_rule = Pattern::new(
        any_rule,
        patterns.clone().into_boxed_slice(),
        MatchKind::LeftmostFirst,
    );
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
    expect!(heap, who, v, "vector",
        Value::Vector(id) => heap.vector(id).to_vec(),
    )
}

/// `(%regex-tokens patterns tags s)`
fn regex_tokens(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let patterns: Vec<Box<str>> = vector_arg(heap, "%regex-tokens", arg(args, 0))?
        .into_iter()
        .map(|p| expect_string_ref(heap, "%regex-tokens", p).map(Box::from))
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
        let mut poll = InterruptPoll::new();
        while at < s.len() {
            poll.advance(16)?;
            // the earliest place ANY rule matches; nothing before it can start a token
            let Some(hit) = find_polled(&lex.any_rule, s, at, &mut poll)? else {
                break;
            };
            let p = hit.start();
            let mut token = None;
            for (k, rule) in lex.rules.iter().enumerate() {
                if let Some(e) = longest_at(rule, s, p, s.len(), &mut poll)?.filter(|&e| e > p) {
                    token = Some((e, k));
                    break;
                }
            }
            match token {
                Some((end, k)) => {
                    tokens.push((span(s, p, end), k));
                    at = end;
                }
                // only empty matches start here: no token, look from the next character
                None => at = next_char(s, p),
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
        let mut found = None;
        for p in ends_at(&prefix, s, 0)? {
            let mut ends = ends_at(&pattern, s, p)?;
            if lazy {
                ends.reverse();
            }
            let mut poll = InterruptPoll::new();
            for e in ends {
                if longest_at(&suffix, s, e, s.len(), &mut poll)?.is_some() {
                    found = Some((p, e));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
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

/// The states a [`pike_search`] has active at one position, in priority order, each with the
/// capture slots of the thread that reached it first — the Pike VM's sparse set and slot table.
struct ActiveStates {
    dense: Vec<StateID>,
    index: Vec<u32>,
    slots_per_state: usize,
    slots: Vec<Option<NonMaxUsize>>,
}

impl ActiveStates {
    fn new(state_count: usize, slots_per_state: usize) -> ActiveStates {
        ActiveStates {
            dense: Vec::new(),
            index: vec![0; state_count],
            slots_per_state,
            slots: vec![None; state_count * slots_per_state],
        }
    }

    /// Add `state` unless it is already active: true when it was added.
    fn insert(&mut self, state: StateID) -> bool {
        let at = self.index[state.as_usize()] as usize;
        if at < self.dense.len() && self.dense[at] == state {
            return false;
        }
        self.index[state.as_usize()] = self.dense.len() as u32;
        self.dense.push(state);
        true
    }

    fn slots_of(&mut self, state: StateID) -> &mut [Option<NonMaxUsize>] {
        let from = state.as_usize() * self.slots_per_state;
        &mut self.slots[from..from + self.slots_per_state]
    }
}

/// One step of an epsilon closure: a state to explore, or a capture slot to put back when the
/// exploration below it is done (the Pike VM's `FollowEpsilon`).
enum Follow {
    Explore(StateID),
    Restore(usize, Option<NonMaxUsize>),
}

/// The Thompson NFA a [`pike_search`] simulates: `sources` compiled with every capture group,
/// `meta`'s NFA cap, and the slot layout `meta`'s `Captures` for the same patterns has.
fn build_nfa(sources: &[Box<str>]) -> Option<thompson::NFA> {
    thompson::Compiler::new()
        .configure(thompson::Config::new().nfa_size_limit(Some(10 * (1 << 20))))
        .build_many(sources)
        .ok()
}

/// A Pike VM search of `nfa` over `input` that polls (ADR-493): regex-automata's own
/// `pikevm::search_imp` — the same epsilon closure, thread priority and capture slots — but
/// in a loop of ours, so `poll` ticks once per position and a search no automaton can step
/// (a Unicode word boundary over non-ASCII text, which the lazy DFA quits on) or a capture
/// search over a long match can be stopped. `all`: go on past the first match and report the
/// LAST position one ends at — the longest, for an anchored search under `MatchKind::All`;
/// otherwise leftmost-first. `slots` takes the winning thread's slots (group 0 is slots
/// `2 * pattern` and `2 * pattern + 1`); answers the matching pattern and the match's end.
fn pike_search(
    nfa: &thompson::NFA,
    input: &Input<'_>,
    all: bool,
    slots: &mut [Option<NonMaxUsize>],
    poll: &mut InterruptPoll,
) -> Result<Option<HalfMatch>, LispError> {
    let (anchored, start) = match input.get_anchored() {
        Anchored::No => (nfa.is_always_start_anchored(), nfa.start_anchored()),
        Anchored::Yes => (true, nfa.start_anchored()),
        Anchored::Pattern(pattern) => match nfa.start_pattern(pattern) {
            Some(start) => (true, start),
            None => return Ok(None),
        },
    };
    let state_count = nfa.states().len();
    let mut current = ActiveStates::new(state_count, slots.len());
    let mut next = ActiveStates::new(state_count, slots.len());
    let mut stack = Vec::new();
    let mut scratch = vec![None; slots.len()];
    let mut found = None;
    let mut at = input.start();
    while at <= input.end() {
        poll.tick()?;
        if current.dense.is_empty()
            && ((found.is_some() && !all) || (anchored && at > input.start()))
        {
            break;
        }
        if (found.is_none() || all) && (!anchored || at == input.start()) {
            scratch.iter_mut().for_each(|slot| *slot = None);
            closure(
                nfa,
                &mut stack,
                &mut scratch,
                &mut current,
                input,
                at,
                start,
            );
        }
        // Each active state in priority order: a byte it takes seeds the next position's
        // closure with its slots; a match ends the lower-priority threads, unless `all`.
        for index in 0..current.dense.len() {
            let state = current.dense[index];
            let target = match nfa.state(state) {
                State::ByteRange { trans } => {
                    trans.matches(input.haystack(), at).then_some(trans.next)
                }
                State::Sparse(sparse) => sparse.matches(input.haystack(), at),
                State::Dense(dense) => dense.matches(input.haystack(), at),
                State::Match { pattern_id } => {
                    found = Some(HalfMatch::new(*pattern_id, at));
                    slots.copy_from_slice(current.slots_of(state));
                    if all {
                        continue;
                    }
                    break;
                }
                _ => None,
            };
            if let Some(target) = target {
                scratch.copy_from_slice(current.slots_of(state));
                closure(
                    nfa,
                    &mut stack,
                    &mut scratch,
                    &mut next,
                    input,
                    at + 1,
                    target,
                );
            }
        }
        std::mem::swap(&mut current, &mut next);
        next.dense.clear();
        at += 1;
    }
    Ok(found)
}

/// Add `start`'s epsilon closure at position `at` to `active`, in priority order, each state
/// taking a copy of `slots` as the closure set them (`pikevm::epsilon_closure`).
fn closure(
    nfa: &thompson::NFA,
    stack: &mut Vec<Follow>,
    slots: &mut [Option<NonMaxUsize>],
    active: &mut ActiveStates,
    input: &Input<'_>,
    at: usize,
    start: StateID,
) {
    stack.push(Follow::Explore(start));
    while let Some(step) = stack.pop() {
        let mut state = match step {
            Follow::Restore(slot, offset) => {
                slots[slot] = offset;
                continue;
            }
            Follow::Explore(state) => state,
        };
        loop {
            if !active.insert(state) {
                break;
            }
            match nfa.state(state) {
                State::Look { look, next } => {
                    if !nfa.look_matcher().matches(*look, input.haystack(), at) {
                        break;
                    }
                    state = *next;
                }
                State::Union { alternates } => {
                    let Some(&first) = alternates.first() else {
                        break;
                    };
                    stack.extend(
                        alternates[1..]
                            .iter()
                            .rev()
                            .map(|&alternate| Follow::Explore(alternate)),
                    );
                    state = first;
                }
                State::BinaryUnion { alt1, alt2 } => {
                    stack.push(Follow::Explore(*alt2));
                    state = *alt1;
                }
                State::Capture { next, slot, .. } => {
                    let slot = slot.as_usize();
                    if slot < slots.len() {
                        stack.push(Follow::Restore(slot, slots[slot]));
                        slots[slot] = NonMaxUsize::new(at);
                    }
                    state = *next;
                }
                _ => {
                    active.slots_of(state).copy_from_slice(slots);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATTERNS: &[&str] = &[
        "a+",
        "a*",
        "",
        "q*",
        "x[yz]+",
        "(b|é)x(y+)",
        "é+x",
        "é*",
        "(?m)^.+$",
        "(?m)$",
        "[a-z]+\\d",
        "zq.*",
        "\\w+",
        "(?:ab|a)+c",
        "\\bxyz",
        "\\Bé",
        "(?-u:\\b)a",
        "\\z",
        "\\Aa+",
        "\\b\\w+\\b",
        "(a)|(é)(x)?",
        "(?:(é+)|a)+9",
    ];

    const TEXTS: &[&str] = &[
        "",
        "aaa",
        "xyzé",
        "ééxyz aé9",
        "line1\nline2\n\nline3",
        "abababac zq zqq",
        "é",
        "aé",
        "  \n",
        "baaab zq\naxyzz é9",
    ];

    /// Every char-boundary start of every text.
    fn starts(text: &str) -> impl Iterator<Item = usize> + '_ {
        (0..=text.len()).filter(|&at| text.is_char_boundary(at))
    }

    #[test]
    fn a_stepped_search_finds_what_one_meta_search_finds() {
        for pattern in PATTERNS {
            let rx = compiled("%regex-find", pattern, Kind::LeftmostFirst).unwrap();
            for text in TEXTS {
                for start in starts(text) {
                    let expected = rx.search(&Input::new(text).range(start..));
                    let stepped =
                        find_stepped(&rx, text, start, &mut InterruptPoll::new()).unwrap();
                    // `None`: the DFA quit (a Unicode \b before non-ASCII) and meta answers.
                    if let Some(stepped) = stepped {
                        assert_eq!(stepped, expected, "{pattern:?} in {text:?} from {start}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_stepped_search_over_several_patterns_finds_what_meta_finds() {
        let sources: Vec<Box<str>> = ["x[yz]+", "a+", "é+", "\\d"]
            .iter()
            .map(|source| Box::from(*source))
            .collect();
        let regex = Regex::new_many(&sources).unwrap();
        let rx = Pattern::new(regex, sources.into_boxed_slice(), MatchKind::LeftmostFirst);
        for text in TEXTS {
            for start in starts(text) {
                let expected = rx.search(&Input::new(text).range(start..));
                let stepped = find_stepped(&rx, text, start, &mut InterruptPoll::new()).unwrap();
                assert_eq!(stepped, Some(expected), "{text:?} from {start}");
            }
        }
    }

    #[test]
    fn a_stepped_longest_match_is_what_meta_finds() {
        for pattern in PATTERNS {
            let rx = compiled("%regex-tokens", pattern, Kind::Longest).unwrap();
            let Some(stepped) = rx.stepped() else {
                continue;
            };
            for text in TEXTS {
                for at in starts(text) {
                    for limit in starts(text).filter(|&limit| limit >= at) {
                        let input = Input::new(text).range(at..limit).anchored(Anchored::Yes);
                        let expected = rx.search_half(&input).map(|h| h.offset());
                        let found = stepped
                            .with_cache(|regex, cache| {
                                step_forward(
                                    regex.forward(),
                                    cache.forward_mut(),
                                    &input,
                                    &mut InterruptPoll::new(),
                                )
                            })
                            .unwrap();
                        if let Some(found) = found {
                            assert_eq!(
                                found.map(|end| end.offset()),
                                expected,
                                "{pattern:?} in {text:?} at {at}..{limit}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_prefilter_candidate_across_a_window_edge_is_found() {
        let rx = compiled("%regex-find", "zq\\w*", Kind::LeftmostFirst).unwrap();
        for pad in [
            SEARCH_WINDOW - 2,
            SEARCH_WINDOW - 1,
            SEARCH_WINDOW,
            2 * SEARCH_WINDOW - 1,
        ] {
            let text = format!("{} zqyy {}", "a ".repeat(pad / 2), "b".repeat(1000));
            let expected = rx.search(&Input::new(&text));
            let stepped = find_stepped(&rx, &text, 0, &mut InterruptPoll::new()).unwrap();
            assert_eq!(stepped, Some(expected), "pad {pad}");
        }
    }

    #[test]
    fn the_polling_pike_vm_finds_what_meta_finds_with_its_groups() {
        for pattern in PATTERNS {
            let rx = compiled("%regex-find", pattern, Kind::LeftmostFirst).unwrap();
            let nfa = rx.nfa().unwrap();
            for text in TEXTS {
                for start in starts(text) {
                    let expected = rx.search(&Input::new(text).range(start..));
                    let found = find_pike(&rx, text, start, &mut InterruptPoll::new()).unwrap();
                    assert_eq!(
                        found,
                        Some(expected),
                        "{pattern:?} in {text:?} from {start}"
                    );
                    let Some(whole) = expected else {
                        continue;
                    };
                    // the groups of the match, anchored at it and confined to it
                    let input = Input::new(*text)
                        .range(whole.range())
                        .anchored(Anchored::Pattern(whole.pattern()));
                    let mut groups = rx.create_captures();
                    rx.search_captures(&input, &mut groups);
                    let mut caps = rx.create_captures();
                    let slots = caps.slots_mut();
                    pike_search(nfa, &input, false, slots, &mut InterruptPoll::new()).unwrap();
                    assert_eq!(
                        caps.slots_mut().to_vec(),
                        groups.slots_mut().to_vec(),
                        "{pattern:?} in {text:?} from {start}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_polling_pike_vm_finds_the_longest_match_meta_finds() {
        for pattern in PATTERNS {
            let rx = compiled("%regex-tokens", pattern, Kind::Longest).unwrap();
            let nfa = rx.nfa().unwrap();
            let mut slots = vec![None; 2];
            for text in TEXTS {
                for at in starts(text) {
                    for limit in starts(text).filter(|&limit| limit >= at) {
                        let input = Input::new(text).range(at..limit).anchored(Anchored::Yes);
                        let expected = rx.search_half(&input).map(|h| h.offset());
                        let found =
                            pike_search(nfa, &input, true, &mut slots, &mut InterruptPoll::new())
                                .unwrap();
                        assert_eq!(
                            found.map(|end| end.offset()),
                            expected,
                            "{pattern:?} in {text:?} at {at}..{limit}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_group_pass_over_a_match_longer_than_a_window_stops_inside() {
        // The search finds the match without probing a pending interrupt; the group pass is
        // then handed one. End to end, a pending interrupt stops the SEARCH first, so this
        // is the only way to show that the second pass — the Pike VM over the match itself —
        // stops too, and stops at its first probe, a few thousand positions into a match of
        // a hundred thousand, rather than running to the match's end.
        let text = format!("{}b", "a".repeat(100_000));
        let rx = compiled("%regex-find", "(a+)(b)", Kind::LeftmostFirst).unwrap();
        let found = find_polled(&rx, &text, 0, &mut InterruptPoll::new())
            .unwrap()
            .expect("the search finds the match");
        assert!(found.len() > SEARCH_WINDOW, "the match spans windows");

        let mut caps = rx.create_captures();
        groups_polled(&rx, &text, found, &mut caps, &mut InterruptPoll::new()).unwrap();
        assert_eq!(caps.get_group(1).map(|g| g.range()), Some(0..100_000));
        assert_eq!(caps.get_group(2).map(|g| g.range()), Some(100_000..100_001));

        let mut poll = InterruptPoll::raising();
        let mut caps = rx.create_captures();
        let stopped = groups_polled(&rx, &text, found, &mut caps, &mut poll);
        let error = stopped.expect_err("a pending interrupt stops the group pass");
        assert_eq!(error.kind, crate::error::ErrorKind::Interrupted);
        assert_eq!(poll.probes(), 1, "it stopped at its first probe");
    }
}
