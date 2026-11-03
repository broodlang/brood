//! Regular expressions over a ROPE, in place: the primitives under `regex/rope-find`,
//! `regex/rope-find-backward`, `regex/rope-count` and `regex/rope-spans` (ADR-540).
//!
//! An editor's buffer is a rope, and `regex/find` takes a string. Flattening the rope to
//! search it costs a copy of the document per search, and incremental search searches on
//! every keystroke — in a 50 MB file that is the whole budget of a frame, twice. So these
//! drive the pattern's lazy DFA ourselves, a byte at a time over the rope's leaves (the
//! state machine regex-automata lets a caller step, as [`super::regex_native`]'s stepped
//! search does over a string): nothing whole-buffer is allocated, and a search costs the
//! distance to its match.
//!
//! - **Forward** is the engine's own two passes: the forward DFA (leftmost-first) to the
//!   match's END, then the reverse DFA, anchored there, back to its START.
//! - **Backward** is Emacs's `re-search-backward`: the match STARTING last before `before`
//!   and ENDING at or before it. The reverse DFA run UNANCHORED backward from `before`
//!   meets the largest such start first; the forward DFA, anchored there and bounded at
//!   `before`, gives the end.
//! - **Groups** are read off the match alone: its text (and one character either side, for
//!   look-around) handed to the `meta` regex anchored and confined to it.
//! - **Bounded** scans (the count's slices, a window's highlights) stop at the first
//!   position past their limit where the DFA is back in its START state with nothing found
//!   — the one point where no match can be in progress, so resuming there loses nothing.
//!   A literal is back in a start state a character after a near miss; a pattern in a
//!   buffer's line mode, at the next newline.
//!
//! Matches are NON-EMPTY. A search box asks where the text is, and an empty match (`x*`
//! before every character) answers nothing; one is stepped over.
//!
//! Where the lazy DFA cannot go — a Unicode word boundary (`\b`) before non-ASCII text — the
//! search falls back to the `meta` regex over the text it needs, materialised. That is the
//! one path that copies, and it says so in each fallback below.

use super::args::{arg, expect_int, expect_rope_ref, expect_string_ref};
use super::regex_native::{Kind, SEARCH_WINDOW, compiled, kw};
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::InterruptPoll;
use regex_automata::nfa::thompson::{self, WhichCaptures};
use regex_automata::util::prefilter::Prefilter;
use regex_automata::util::start;
use regex_automata::{Anchored, Input, MatchKind, hybrid};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%rope-regex-find",
        Arity::exact(4),
        Sig::new(vec![rope, string, int, any], map_or_nil),
        &["r", "pattern", "from", "empty?"],
        "The leftmost-first match (NON-EMPTY unless empty? — then an empty match is an answer too) of the regex-automata pattern in rope r starting at or after char index from, as {:start :end :text :groups [...]} in CHAR offsets — or nil. Searches the rope in place (its lazy DFA stepped over the leaves), so the cost is the distance to the match. regex/rope-find is this over the translated pattern.",
        rope_regex_find,
    );
    primitives.def(
        "%rope-regex-rfind",
        Arity::exact(4),
        Sig::new(vec![rope, string, int, any], map_or_nil),
        &["r", "pattern", "before", "empty?"],
        "The match (NON-EMPTY unless empty?, when an empty one AT before counts too) of the regex-automata pattern in rope r that starts LAST while ending at or before char index before (Emacs's re-search-backward), as the map %rope-regex-find answers — or nil. Reads backward from before, so the cost is the distance to the match.",
        rope_regex_rfind,
    );
    primitives.def(
        "%rope-regex-count",
        Arity::exact(4),
        Sig::new(vec![rope, string, int, int], vec_ty),
        &["r", "pattern", "from", "limit"],
        "[n resume]: how many non-overlapping NON-EMPTY matches of the pattern rope r holds that START in char range [from, limit), scanning from from — resume is where the next slice begins (the first match starting at or past limit, or the first point past it where no match is in progress), or nil when the scan reached the rope's end. A count in slices is exact: the slices' counts add up to the whole one, and (rope-count … 0 p) is how many matches start before p.",
        rope_regex_count,
    );
    primitives.def(
        "%rope-regex-spans",
        Arity::exact(4),
        Sig::new(vec![rope, string, int, int], vec_ty),
        &["r", "pattern", "from", "to"],
        "Every non-overlapping NON-EMPTY match of the pattern in rope r that starts in char range [from, to), as a vector of [start end] (char offsets; an end may lie past to), scanning from from — at most 4096 of them. What a window's highlight wants: the cost is the window, not the rope.",
        rope_regex_spans,
    );
}

/// The most spans one `%rope-regex-spans` answers — a window's worth many times over.
const SPANS_CAP: usize = 4096;

/// The lazy DFAs a rope search drives: the forward one (leftmost-first, start states
/// tagged so a bounded scan can see where nothing is in progress, and the prefilter `meta`
/// would use) and the reverse one (`MatchKind::All`, for a match's start and for the
/// backward search). A pool of cache pairs, as [`super::regex_native`]'s stepped search
/// keeps: a cache is the DFA built so far.
struct Engine {
    forward: hybrid::dfa::DFA,
    reverse: hybrid::dfa::DFA,
    caches: Mutex<Vec<(hybrid::dfa::Cache, hybrid::dfa::Cache)>>,
}

impl Engine {
    fn build(source: &str) -> Option<Engine> {
        let hir = regex_automata::util::syntax::parse(source).ok()?;
        let prefilter = Prefilter::from_hirs_prefix(MatchKind::LeftmostFirst, &[&hir])
            .filter(|prefilter| prefilter.is_fast());
        let nfa_config = || {
            thompson::Config::new()
                .nfa_size_limit(Some(10 * (1 << 20)))
                .which_captures(WhichCaptures::None)
        };
        let forward = hybrid::dfa::DFA::builder()
            .configure(
                hybrid::dfa::DFA::config()
                    .match_kind(MatchKind::LeftmostFirst)
                    .prefilter(prefilter)
                    .specialize_start_states(true)
                    .unicode_word_boundary(true)
                    .minimum_cache_clear_count(None),
            )
            .thompson(nfa_config())
            .build(source)
            .ok()?;
        let reverse = hybrid::dfa::DFA::builder()
            .configure(
                hybrid::dfa::DFA::config()
                    .match_kind(MatchKind::All)
                    .unicode_word_boundary(true)
                    .minimum_cache_clear_count(None),
            )
            .thompson(nfa_config().reverse(true))
            .build(source)
            .ok()?;
        Some(Engine {
            forward,
            reverse,
            caches: Mutex::new(Vec::new()),
        })
    }

    fn with_caches<T>(
        &self,
        run: impl FnOnce(&mut hybrid::dfa::Cache, &mut hybrid::dfa::Cache) -> T,
    ) -> T {
        let pooled = self.caches.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let (mut forward, mut reverse) =
            pooled.unwrap_or_else(|| (self.forward.create_cache(), self.reverse.create_cache()));
        let answer = run(&mut forward, &mut reverse);
        self.caches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((forward, reverse));
        answer
    }
}

/// The engine for `source`, built once and kept by its text (cleared whole at the
/// regex cache's limit, as there). `None` when the pattern is past the NFA cap — the
/// searches below then take the materialised path.
fn engine(source: &str) -> Arc<Option<Engine>> {
    static ENGINES: OnceLock<Mutex<HashMap<Box<str>, Arc<Option<Engine>>>>> = OnceLock::new();
    let engines = ENGINES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = engines
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(source)
    {
        return Arc::clone(hit);
    }
    let fresh = Arc::new(Engine::build(source));
    let mut map = engines.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= 4096 {
        map.clear();
    }
    map.insert(source.into(), Arc::clone(&fresh));
    fresh
}

/// A rope read a byte at a time: the leaf holding the last byte read is kept, so a scan
/// pays the O(log n) leaf lookup once per leaf, in either direction.
struct RopeBytes<'r> {
    rope: &'r ropey::Rope,
    chunk: &'r [u8],
    start: usize,
    len: usize,
}

impl<'r> RopeBytes<'r> {
    fn new(rope: &'r ropey::Rope) -> RopeBytes<'r> {
        RopeBytes {
            rope,
            chunk: &[],
            start: 0,
            len: rope.len_bytes(),
        }
    }

    /// The leaf holding byte `at` (< len), and the byte index it starts at.
    fn leaf(&mut self, at: usize) -> (&'r [u8], usize) {
        if at < self.start || at >= self.start + self.chunk.len() {
            let (chunk, chunk_start, _, _) = self.rope.chunk_at_byte(at);
            self.chunk = chunk.as_bytes();
            self.start = chunk_start;
        }
        (self.chunk, self.start)
    }

    fn byte(&mut self, at: usize) -> u8 {
        let (chunk, start) = self.leaf(at);
        chunk[at - start]
    }

    fn before(&mut self, at: usize) -> Option<u8> {
        (at > 0).then(|| self.byte(at - 1))
    }

    fn after(&mut self, at: usize) -> Option<u8> {
        (at < self.len).then(|| self.byte(at))
    }

    fn is_boundary(&mut self, at: usize) -> bool {
        at >= self.len || (self.byte(at) & 0xC0) != 0x80
    }

    /// The byte just past the character starting at `at`.
    fn next_char(&mut self, at: usize) -> usize {
        let mut next = at + 1;
        while next < self.len && !self.is_boundary(next) {
            next += 1;
        }
        next
    }

    /// The character boundary at or before `at`.
    fn boundary_down(&mut self, at: usize) -> usize {
        let mut at = at.min(self.len);
        while at > 0 && !self.is_boundary(at) {
            at -= 1;
        }
        at
    }
}

/// What one forward scan found.
enum Scan {
    /// a match ends here (its start is the reverse pass's to find)
    End(usize),
    /// a bounded scan stopped here, nothing in progress — resume from it
    Resume(usize),
    /// no match before the scan's end
    Missing,
    /// the lazy DFA gave up (a Unicode word boundary before non-ASCII text)
    Quit,
}

/// Drive the forward DFA from byte `from` up to byte `end`, look-around seeing the real
/// bytes on either side: the end of the leftmost-first match (anchored at `from` when
/// `anchored`), or where a scan past `limit` found itself back in a start state with nothing
/// found. A start state with nothing found also lets the prefilter skip to its next
/// candidate — never past the leaf's last possible straddle, nor past `limit`.
#[allow(clippy::too_many_arguments)]
fn scan_forward(
    dfa: &hybrid::dfa::DFA,
    cache: &mut hybrid::dfa::Cache,
    bytes: &mut RopeBytes<'_>,
    from: usize,
    end: usize,
    limit: usize,
    anchored: bool,
    poll: &mut InterruptPoll,
) -> Result<Scan, LispError> {
    let mode = if anchored {
        Anchored::Yes
    } else {
        Anchored::No
    };
    let start_at = |bytes: &mut RopeBytes<'_>, cache: &mut hybrid::dfa::Cache, at: usize| {
        let config = start::Config::new()
            .anchored(mode)
            .look_behind(bytes.before(at));
        dfa.start_state(cache, &config).ok()
    };
    let Some(mut state) = start_at(bytes, cache, from) else {
        return Ok(Scan::Quit);
    };
    let prefilter = if anchored {
        None
    } else {
        dfa.get_config().get_prefilter()
    };
    let universal_start = dfa.get_nfa().look_set_prefix_any().is_empty();
    let mut found = None;
    let mut at = from;
    let mut since_poll = 0usize;
    while at < end {
        if found.is_none() && !anchored && state.is_start() {
            if at >= limit {
                return Ok(Scan::Resume(at));
            }
            if let Some(prefilter) = prefilter {
                let (chunk, chunk_start) = bytes.leaf(at);
                let leaf_end = (chunk_start + chunk.len()).min(end);
                let span = regex_automata::Span::from(at - chunk_start..leaf_end - chunk_start);
                let next = match prefilter.find(chunk, span) {
                    Some(candidate) => chunk_start + candidate.start,
                    // a candidate may straddle into the next leaf: step its first bytes
                    None => leaf_end.saturating_sub(prefilter.max_needle_len().saturating_sub(1)),
                };
                let next = next.min(limit).max(at);
                if next > at {
                    since_poll += next - at;
                    at = next;
                    if !universal_start {
                        let Some(restart) = start_at(bytes, cache, at) else {
                            return Ok(Scan::Quit);
                        };
                        state = restart;
                    }
                    if since_poll >= SEARCH_WINDOW {
                        poll.advance(InterruptPoll::EVERY)?;
                        since_poll = 0;
                    }
                    continue;
                }
            }
        }
        let byte = bytes.byte(at);
        let Ok(next) = dfa.next_state(cache, state, byte) else {
            return Ok(Scan::Quit);
        };
        state = next;
        if state.is_tagged() {
            if state.is_match() {
                found = Some(at);
            } else if state.is_dead() {
                return Ok(found.map_or(Scan::Missing, Scan::End));
            } else if state.is_quit() {
                return Ok(Scan::Quit);
            }
        }
        at += 1;
        since_poll += 1;
        if since_poll >= SEARCH_WINDOW {
            poll.advance(InterruptPoll::EVERY)?;
            since_poll = 0;
        }
    }
    let last = match bytes.after(end) {
        Some(byte) => dfa.next_state(cache, state, byte),
        None => dfa.next_eoi_state(cache, state),
    };
    let Ok(last) = last else {
        return Ok(Scan::Quit);
    };
    if last.is_match() {
        found = Some(end);
    } else if last.is_quit() {
        return Ok(Scan::Quit);
    }
    Ok(found.map_or(Scan::Missing, Scan::End))
}

/// The leftmost start, at or after byte `from`, of a match ending at byte `end`: the
/// reverse DFA anchored at `end` and driven back to `from`. `None` when it quit.
fn scan_start(
    dfa: &hybrid::dfa::DFA,
    cache: &mut hybrid::dfa::Cache,
    bytes: &mut RopeBytes<'_>,
    from: usize,
    end: usize,
    poll: &mut InterruptPoll,
) -> Result<Option<Option<usize>>, LispError> {
    let config = start::Config::new()
        .anchored(Anchored::Yes)
        .look_behind(bytes.after(end));
    let Ok(mut state) = dfa.start_state(cache, &config) else {
        return Ok(None);
    };
    let mut found = None;
    let mut at = end;
    let mut since_poll = 0usize;
    while at > from {
        at -= 1;
        let Ok(next) = dfa.next_state(cache, state, bytes.byte(at)) else {
            return Ok(None);
        };
        state = next;
        if state.is_tagged() {
            if state.is_match() {
                found = Some(at + 1);
            } else if state.is_dead() {
                return Ok(Some(found));
            } else if state.is_quit() {
                return Ok(None);
            }
        }
        since_poll += 1;
        if since_poll >= SEARCH_WINDOW {
            poll.advance(InterruptPoll::EVERY)?;
            since_poll = 0;
        }
    }
    let last = match bytes.before(from) {
        Some(byte) => dfa.next_state(cache, state, byte),
        None => dfa.next_eoi_state(cache, state),
    };
    let Ok(last) = last else {
        return Ok(None);
    };
    if last.is_match() {
        found = Some(from);
    } else if last.is_quit() {
        return Ok(None);
    }
    Ok(Some(found))
}

/// What a forward search found.
enum Found {
    Match(usize, usize),
    Resume(usize),
    Missing,
    Quit,
}

/// The leftmost-first match starting at or after byte `from` (bounded by `limit` as
/// [`scan_forward`] is), by the DFAs — non-empty unless `allow_empty`.
fn find_forward(
    engine: &Engine,
    bytes: &mut RopeBytes<'_>,
    from: usize,
    limit: usize,
    allow_empty: bool,
    poll: &mut InterruptPoll,
) -> Result<Found, LispError> {
    engine.with_caches(|forward_cache, reverse_cache| {
        let mut from = from;
        loop {
            if from > bytes.len {
                return Ok(Found::Missing);
            }
            let end = match scan_forward(
                &engine.forward,
                forward_cache,
                bytes,
                from,
                bytes.len,
                limit,
                false,
                poll,
            )? {
                Scan::End(end) => end,
                Scan::Resume(at) => return Ok(Found::Resume(at)),
                Scan::Missing => return Ok(Found::Missing),
                Scan::Quit => return Ok(Found::Quit),
            };
            if !bytes.is_boundary(end) {
                // only an empty match ends inside a character; nothing starts before it
                from = end + 1;
                continue;
            }
            let Some(Some(start)) =
                scan_start(&engine.reverse, reverse_cache, bytes, from, end, poll)?
            else {
                return Ok(Found::Quit);
            };
            if start == end && !allow_empty {
                if end >= bytes.len {
                    return Ok(Found::Missing);
                }
                from = bytes.next_char(end);
                continue;
            }
            return Ok(Found::Match(start, end));
        }
    })
}

/// The match starting last while ending at or before byte `before` (non-empty unless
/// `allow_empty`, when an empty match AT `before` counts), by the DFAs:
/// the reverse DFA unanchored from `before` meets candidate starts largest first; each is
/// confirmed (and its end found) by the forward DFA anchored there and bounded at `before`.
fn find_backward(
    engine: &Engine,
    bytes: &mut RopeBytes<'_>,
    before: usize,
    allow_empty: bool,
    poll: &mut InterruptPoll,
) -> Result<Found, LispError> {
    engine.with_caches(|forward_cache, reverse_cache| {
        let config = start::Config::new()
            .anchored(Anchored::No)
            .look_behind(bytes.after(before));
        let Ok(mut state) = engine.reverse.start_state(reverse_cache, &config) else {
            return Ok(Found::Quit);
        };
        let confirm = |bytes: &mut RopeBytes<'_>,
                       forward_cache: &mut hybrid::dfa::Cache,
                       start: usize,
                       poll: &mut InterruptPoll|
         -> Result<Option<Found>, LispError> {
            if start > before || (start == before && !allow_empty) || !bytes.is_boundary(start) {
                return Ok(None);
            }
            match scan_forward(
                &engine.forward,
                forward_cache,
                bytes,
                start,
                before,
                before,
                true,
                poll,
            )? {
                Scan::End(end) if end > start || allow_empty => Ok(Some(Found::Match(start, end))),
                Scan::Quit => Ok(Some(Found::Quit)),
                _ => Ok(None),
            }
        };
        let mut at = before;
        let mut since_poll = 0usize;
        while at > 0 {
            at -= 1;
            let Ok(next) = engine
                .reverse
                .next_state(reverse_cache, state, bytes.byte(at))
            else {
                return Ok(Found::Quit);
            };
            state = next;
            if state.is_tagged() {
                if state.is_match() {
                    if let Some(found) = confirm(bytes, forward_cache, at + 1, poll)? {
                        return Ok(found);
                    }
                } else if state.is_dead() {
                    return Ok(Found::Missing);
                } else if state.is_quit() {
                    return Ok(Found::Quit);
                }
            }
            since_poll += 1;
            if since_poll >= SEARCH_WINDOW {
                poll.advance(InterruptPoll::EVERY)?;
                since_poll = 0;
            }
        }
        let Ok(last) = engine.reverse.next_eoi_state(reverse_cache, state) else {
            return Ok(Found::Quit);
        };
        if last.is_match()
            && let Some(found) = confirm(bytes, forward_cache, 0, poll)?
        {
            return Ok(found);
        }
        if last.is_quit() {
            return Ok(Found::Quit);
        }
        Ok(Found::Missing)
    })
}

// ---- the materialised fallback ------------------------------------------------------
// Where the lazy DFA quit, the `meta` regex runs over the text the question needs, copied
// out of the rope. Byte offsets below are the rope's.

/// The rope's bytes `[from, to)` as a string (both char boundaries).
fn text_of(rope: &ropey::Rope, from: usize, to: usize) -> String {
    rope.byte_slice(from..to).to_string()
}

/// [`find_forward`] by `meta` over `[context, scan_end)` of the rope (`context` a char
/// boundary at or before `from`, for look-behind): matches starting before `scan_end`.
fn fallback_forward(
    source: &str,
    rope: &ropey::Rope,
    bytes: &mut RopeBytes<'_>,
    from: usize,
    allow_empty: bool,
) -> Result<Option<(usize, usize)>, LispError> {
    let rx = compiled("%rope-regex-find", source, Kind::LeftmostFirst)?;
    let context = bytes.boundary_down(from.saturating_sub(1));
    let text = text_of(rope, context, bytes.len);
    let mut at = from - context;
    let mut poll = InterruptPoll::new();
    while at <= text.len() {
        poll.tick()?;
        let Some(found) = rx.search(&Input::new(&text).range(at..)) else {
            return Ok(None);
        };
        if found.is_empty() && !allow_empty {
            at = found.end() + text[found.end()..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        return Ok(Some((context + found.start(), context + found.end())));
    }
    Ok(None)
}

/// [`find_backward`] by `meta`: an anchored attempt at each start going back from
/// `before`, over the text up to `before` and the character after it.
fn fallback_backward(
    source: &str,
    rope: &ropey::Rope,
    bytes: &mut RopeBytes<'_>,
    before: usize,
    allow_empty: bool,
) -> Result<Option<(usize, usize)>, LispError> {
    let rx = compiled("%rope-regex-rfind", source, Kind::LeftmostFirst)?;
    let text_end = if before < bytes.len {
        bytes.next_char(before)
    } else {
        before
    };
    let text = text_of(rope, 0, text_end);
    let mut poll = InterruptPoll::new();
    // candidate starts from `before` itself (an empty match there) or the byte before it, down
    let first = if allow_empty {
        Some(before)
    } else {
        before.checked_sub(1)
    };
    let Some(first) = first else {
        return Ok(None);
    };
    for at in (0..=first).rev() {
        if !text.is_char_boundary(at) {
            continue;
        }
        poll.tick()?;
        let input = Input::new(&text).range(at..before).anchored(Anchored::Yes);
        if let Some(found) = rx.search(&input)
            && (allow_empty || !found.is_empty())
        {
            return Ok(Some((found.start(), found.end())));
        }
    }
    Ok(None)
}

// ---- answering ------------------------------------------------------------------

/// A group's span as plain data: char offsets and its text.
type GroupSpan = Option<(usize, usize, String)>;

/// The groups of the match `[start, end)` (bytes): `meta` anchored and confined to it, over
/// its text and a character either side — the match is the highest-priority one starting
/// there, so confining the search to it changes neither it nor its groups.
fn groups_of(
    source: &str,
    rope: &ropey::Rope,
    bytes: &mut RopeBytes<'_>,
    start: usize,
    end: usize,
) -> Result<Vec<GroupSpan>, LispError> {
    let rx = compiled("%rope-regex-find", source, Kind::LeftmostFirst)?;
    let context_start = bytes.boundary_down(start.saturating_sub(1));
    let context_end = if end < bytes.len {
        bytes.next_char(end)
    } else {
        end
    };
    let text = text_of(rope, context_start, context_end);
    let mut caps = rx.create_captures();
    let input = Input::new(&text)
        .range(start - context_start..end - context_start)
        .anchored(Anchored::Yes);
    rx.search_captures(&input, &mut caps);
    if !caps.is_match() {
        // cannot happen for a match the DFAs found; answer the whole match alone
        let whole = text[start - context_start..end - context_start].to_owned();
        return Ok(vec![Some((
            rope.byte_to_char(start),
            rope.byte_to_char(end),
            whole,
        ))]);
    }
    Ok((0..caps.group_len())
        .map(|i| {
            caps.get_group(i).map(|group| {
                (
                    rope.byte_to_char(context_start + group.start),
                    rope.byte_to_char(context_start + group.end),
                    text[group.start..group.end].to_owned(),
                )
            })
        })
        .collect())
}

fn span_value(heap: &mut Heap, span: &(usize, usize, String)) -> Value {
    let text = heap.alloc_string(&span.2);
    heap.map_from_pairs(vec![
        (kw("start"), Value::Int(span.0 as i64)),
        (kw("end"), Value::Int(span.1 as i64)),
        (kw("text"), text),
    ])
}

/// The map `regex/find` answers, from a match's group spans.
fn match_value(heap: &mut Heap, spans: &[GroupSpan]) -> Value {
    let groups: Vec<Value> = spans[1..]
        .iter()
        .map(|group| match group {
            Some(span) => span_value(heap, span),
            None => Value::Nil,
        })
        .collect();
    let groups = heap.alloc_vector(groups);
    let whole = spans[0].as_ref().expect("group 0 is the match");
    let text = heap.alloc_string(&whole.2);
    heap.map_from_pairs(vec![
        (kw("start"), Value::Int(whole.0 as i64)),
        (kw("end"), Value::Int(whole.1 as i64)),
        (kw("text"), text),
        (kw("groups"), groups),
    ])
}

/// Whether a flag argument is set (anything but nil and false).
fn truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// A char-index argument as the rope's byte index, clamped to it.
fn byte_arg(heap: &Heap, who: &str, v: Value, rope: &ropey::Rope) -> Result<usize, LispError> {
    let chars = expect_int(heap, who, v)?.max(0) as usize;
    Ok(rope.char_to_byte(chars.min(rope.len_chars())))
}

/// `(%rope-regex-find r pattern from)`
fn rope_regex_find(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%rope-regex-find";
    let spans = {
        let source = expect_string_ref(heap, who, arg(args, 1))?;
        let rope = expect_rope_ref(heap, who, arg(args, 0))?;
        let allow_empty = truthy(arg(args, 3));
        let from = byte_arg(heap, who, arg(args, 2), rope)?;
        let mut bytes = RopeBytes::new(rope);
        let mut poll = InterruptPoll::new();
        let engine = engine(source);
        let found = match engine.as_ref() {
            Some(engine) => {
                find_forward(engine, &mut bytes, from, usize::MAX, allow_empty, &mut poll)?
            }
            None => Found::Quit,
        };
        let span = match found {
            Found::Match(start, end) => Some((start, end)),
            Found::Quit => fallback_forward(source, rope, &mut bytes, from, allow_empty)?,
            Found::Resume(_) | Found::Missing => None,
        };
        match span {
            Some((start, end)) => groups_of(source, rope, &mut bytes, start, end)?,
            None => return Ok(Value::Nil),
        }
    };
    Ok(match_value(heap, &spans))
}

/// `(%rope-regex-rfind r pattern before)`
fn rope_regex_rfind(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%rope-regex-rfind";
    let spans = {
        let source = expect_string_ref(heap, who, arg(args, 1))?;
        let rope = expect_rope_ref(heap, who, arg(args, 0))?;
        let before = byte_arg(heap, who, arg(args, 2), rope)?;
        let allow_empty = truthy(arg(args, 3));
        let mut bytes = RopeBytes::new(rope);
        let mut poll = InterruptPoll::new();
        let engine = engine(source);
        let found = match engine.as_ref() {
            Some(engine) => find_backward(engine, &mut bytes, before, allow_empty, &mut poll)?,
            None => Found::Quit,
        };
        let span = match found {
            Found::Match(start, end) => Some((start, end)),
            Found::Quit => fallback_backward(source, rope, &mut bytes, before, allow_empty)?,
            Found::Resume(_) | Found::Missing => None,
        };
        match span {
            Some((start, end)) => groups_of(source, rope, &mut bytes, start, end)?,
            None => return Ok(Value::Nil),
        }
    };
    Ok(match_value(heap, &spans))
}

/// The matches from byte `from`, handed to `each` as byte spans until it answers false,
/// the DFAs bounded by `limit`: the byte to resume from, or `None` at the rope's end. A
/// pattern the DFA cannot step is answered by `meta` over the rest of the rope, whole.
fn each_match(
    source: &str,
    rope: &ropey::Rope,
    from: usize,
    limit: usize,
    mut each: impl FnMut(usize, usize) -> bool,
) -> Result<Option<usize>, LispError> {
    let mut bytes = RopeBytes::new(rope);
    let mut poll = InterruptPoll::new();
    let engine = engine(source);
    let mut at = from;
    loop {
        poll.tick()?;
        let found = match engine.as_ref() {
            Some(engine) => find_forward(engine, &mut bytes, at, limit, false, &mut poll)?,
            None => Found::Quit,
        };
        match found {
            Found::Match(start, end) => {
                if !each(start, end) {
                    return Ok(Some(start));
                }
                at = end;
            }
            Found::Resume(resume) => return Ok(Some(resume)),
            Found::Missing => return Ok(None),
            Found::Quit => match fallback_forward(source, rope, &mut bytes, at, false)? {
                Some((start, end)) => {
                    if !each(start, end) {
                        return Ok(Some(start));
                    }
                    at = end;
                }
                None => return Ok(None),
            },
        }
    }
}

/// `(%rope-regex-count r pattern from limit)`
fn rope_regex_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%rope-regex-count";
    let (count, resume) = {
        let source = expect_string_ref(heap, who, arg(args, 1))?;
        let rope = expect_rope_ref(heap, who, arg(args, 0))?;
        let from = byte_arg(heap, who, arg(args, 2), rope)?;
        let limit = byte_arg(heap, who, arg(args, 3), rope)?;
        let mut count = 0i64;
        // a match starting at or past `limit` is the next slice's, and where it resumes
        let resume = each_match(source, rope, from, limit, |start, _| {
            if start >= limit {
                return false;
            }
            count += 1;
            true
        })?;
        (count, resume.map(|byte| rope.byte_to_char(byte) as i64))
    };
    Ok(heap.alloc_vector2(Value::Int(count), resume.map_or(Value::Nil, Value::Int)))
}

/// `(%rope-regex-spans r pattern from to)`
fn rope_regex_spans(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%rope-regex-spans";
    let spans = {
        let source = expect_string_ref(heap, who, arg(args, 1))?;
        let rope = expect_rope_ref(heap, who, arg(args, 0))?;
        let from = byte_arg(heap, who, arg(args, 2), rope)?;
        let to = byte_arg(heap, who, arg(args, 3), rope)?;
        let mut spans = Vec::new();
        each_match(source, rope, from, to, |start, end| {
            if start >= to || spans.len() >= SPANS_CAP {
                return false;
            }
            spans.push((rope.byte_to_char(start), rope.byte_to_char(end)));
            true
        })?;
        spans
    };
    let items: Vec<Value> = spans
        .iter()
        .map(|&(start, end)| heap.alloc_vector2(Value::Int(start as i64), Value::Int(end as i64)))
        .collect();
    Ok(heap.alloc_vector(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The naive answers over the flattened text, in byte offsets.
    fn naive_all(source: &str, text: &str) -> Vec<(usize, usize)> {
        let rx = regex_automata::meta::Regex::new(source).unwrap();
        let mut all = Vec::new();
        let mut at = 0;
        while at <= text.len() {
            let Some(found) = rx.search(&Input::new(text).range(at..)) else {
                break;
            };
            if found.is_empty() {
                at = found.end() + text[found.end()..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            all.push((found.start(), found.end()));
            at = found.end();
        }
        all
    }

    /// A rope of many small leaves, so a match straddles them.
    fn rope_of(text: &str) -> ropey::Rope {
        let mut builder = ropey::RopeBuilder::new();
        for piece in text.split_inclusive(' ') {
            builder.append(piece);
        }
        builder.finish()
    }

    fn all_forward(source: &str, rope: &ropey::Rope) -> Vec<(usize, usize)> {
        let mut all = Vec::new();
        each_match(source, rope, 0, usize::MAX, |start, end| {
            all.push((start, end));
            true
        })
        .unwrap();
        all
    }

    #[test]
    fn forward_matches_agree_with_meta() {
        let text = "foo bar foobar\nbaz föö Foo fooo x foo".repeat(50);
        let rope = rope_of(&text);
        for source in [
            "foo",
            "(?i)foo",
            "fo+",
            "b[a-z]r",
            "(?m:^)foo",
            "o(?m:$)",
            r"\bfoo\b",
            "x*",
            "f[^\n]*o",
        ] {
            assert_eq!(
                all_forward(source, &rope),
                naive_all(source, &text),
                "{source}"
            );
        }
    }

    #[test]
    fn counting_in_slices_adds_up() {
        let text = "ab abab xab ".repeat(2000);
        let rope = ropey::Rope::from_str(&text);
        let whole = naive_all("ab", &text).len();
        let mut total = 0;
        let mut at = Some(0usize);
        while let Some(from) = at {
            let mut slice = 0;
            at = each_match("ab", &rope, from, from + 1000, |_, _| {
                slice += 1;
                true
            })
            .unwrap();
            total += slice;
        }
        assert_eq!(total, whole);
    }

    #[test]
    fn backward_is_the_last_start_ending_before() {
        let text = "foo foo xfoo";
        let rope = rope_of(text);
        let engine = Engine::build("fo+").unwrap();
        let mut bytes = RopeBytes::new(&rope);
        let mut poll = InterruptPoll::new();
        let at = |bytes: &mut RopeBytes<'_>, poll: &mut InterruptPoll, before| match find_backward(
            &engine, bytes, before, false, poll,
        )
        .unwrap()
        {
            Found::Match(start, end) => Some((start, end)),
            _ => None,
        };
        assert_eq!(at(&mut bytes, &mut poll, 12), Some((9, 12)));
        // a match must END by `before`: from inside the last `foo`, the one before it
        assert_eq!(at(&mut bytes, &mut poll, 11), Some((9, 11)));
        assert_eq!(at(&mut bytes, &mut poll, 9), Some((4, 7)));
        assert_eq!(at(&mut bytes, &mut poll, 1), None);
    }

    #[test]
    fn unicode_word_boundary_falls_back() {
        let text = "über foo über";
        let rope = rope_of(text);
        assert_eq!(all_forward(r"\bfoo\b", &rope), naive_all(r"\bfoo\b", text));
        assert_eq!(all_forward(r"\bü", &rope), naive_all(r"\bü", text));
    }

    #[test]
    fn an_empty_match_is_an_answer_when_asked_for() {
        let rope = rope_of("ab xx");
        let engine = Engine::build("x*").unwrap();
        let mut bytes = RopeBytes::new(&rope);
        let mut poll = InterruptPoll::new();
        let forward =
            |bytes: &mut RopeBytes<'_>, poll: &mut InterruptPoll, from, empty| match find_forward(
                &engine,
                bytes,
                from,
                usize::MAX,
                empty,
                poll,
            )
            .unwrap()
            {
                Found::Match(start, end) => Some((start, end)),
                _ => None,
            };
        assert_eq!(forward(&mut bytes, &mut poll, 0, true), Some((0, 0)));
        assert_eq!(forward(&mut bytes, &mut poll, 0, false), Some((3, 5)));
        let backward = |bytes: &mut RopeBytes<'_>, poll: &mut InterruptPoll, before, empty| {
            match find_backward(&engine, bytes, before, empty, poll).unwrap() {
                Found::Match(start, end) => Some((start, end)),
                _ => None,
            }
        };
        // an empty match AT `before` is the last start; without it, the `x` before it
        assert_eq!(backward(&mut bytes, &mut poll, 2, true), Some((2, 2)));
        assert_eq!(backward(&mut bytes, &mut poll, 5, false), Some((4, 5)));
    }
}
