//! The pattern grammar, read once: what every part of a pattern is, for each Rust pass that
//! interprets one — the checker, the hygiene pass and the scope analysis over the CST.
//!
//! `std/prelude/match.blsp` compiles patterns and is the authority on what one means. The
//! Rust passes each used to re-read the raw syntax with their own switch over the heads, and
//! the switches drifted: one did not know `bytes` and read a `(bytes …)` parameter as a list,
//! another counted `or` and `record` as bound names. This module is the one Rust reading.
//!
//! - [`ListForm`] says what a list pattern's head makes it. A reader that walks syntax it
//!   must rebuild or that is not a heap value (the hygiene rewrite, the CST) matches on it
//!   with no catch-all arm, so a new form fails to compile at every reader until it is
//!   handled there.
//! - [`Pattern`] is a heap pattern parsed into a tree, for the passes that analyse one.
//!
//! `crates/lisp/tests/pattern_grammar.rs` holds the two languages together: Rust's binders
//! must equal the prelude's `%pattern-vars` for every form, and the heads the prelude's
//! compiler recognises must be exactly [`ListForm::NAMED`].

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, MapId, Symbol, Value};

/// What a list pattern's head makes it. Every form but [`ListForm::List`] is named by its head
/// symbol; any other list is a list pattern, its head included.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListForm {
    /// `'datum`, read as `(quote datum)`: equal to the datum.
    Quote,
    /// `^expr`, read as `(%pin expr)`: equal to the current value of `expr`.
    Pin,
    /// `(or p…)`: the first alternative that matches.
    Or,
    /// `(and p…)`: every pattern, against the same value.
    And,
    /// `(record name {map-pattern}?)`: a record of that name, then its fields.
    Record,
    /// `(bytes segment…)`: binary destructuring.
    Bytes,
    /// `(str part…)`: a string by its literal ends, one part matched against the middle.
    Str,
    /// `(not …)`: refused by the compiler (ADR-152); the test belongs in a `:when` guard.
    Not,
    /// `(unquote …)`, the old `~x` pin spelling: refused by the compiler.
    Unquote,
    /// Any other head: a list pattern.
    List,
}

impl ListForm {
    /// Every named form with the head that names it.
    pub const NAMED: [(&'static str, ListForm); 9] = [
        (kw::QUOTE, ListForm::Quote),
        (kw::PIN, ListForm::Pin),
        (kw::OR, ListForm::Or),
        (kw::AND, ListForm::And),
        ("record", ListForm::Record),
        ("bytes", ListForm::Bytes),
        ("str", ListForm::Str),
        (kw::NOT, ListForm::Not),
        (kw::UNQUOTE, ListForm::Unquote),
    ];

    /// The form a list pattern whose head is the symbol `head` has (`None`: no symbol head).
    pub fn of_head(head: Option<&str>) -> ListForm {
        head.and_then(|name| Self::NAMED.iter().find(|(named, _)| *named == name))
            .map_or(ListForm::List, |&(_, form)| form)
    }

    /// The form of a heap list pattern, read from its first element.
    pub fn of_list(items: &[Value]) -> ListForm {
        match items.first() {
            Some(Value::Sym(head)) => Self::of_head(Some(value::symbol_name_ref(*head))),
            _ => ListForm::List,
        }
    }
}

/// A pattern, parsed. Expressions inside it (a pin's, a default, a dynamic segment size) are
/// kept as the forms they are written as.
#[derive(Clone, Debug)]
pub enum Pattern {
    /// `_`
    Wildcard,
    /// A symbol: binds it (a name repeated in one pattern is an equality constraint).
    Bind(Symbol),
    /// A number, string, keyword, bool, `nil` or bytes literal, matched by equality.
    Literal(Value),
    /// `'datum`
    Quoted(Value),
    /// `^expr`: the expression.
    Pin(Value),
    /// `(p… &optional q… & rest)`
    List(SeqPattern),
    /// `[p…]`: a vector of exactly that length.
    Vector(Vec<Pattern>),
    /// `{:keys [a b] :or {a default} key p …}`
    Map(MapPattern),
    /// `(or p…)`
    Or(Vec<Pattern>),
    /// `(and p…)`
    And(Vec<Pattern>),
    /// `(record name {…}?)`: the name symbol, then the field pattern if written.
    Record {
        name: Symbol,
        fields: Option<MapPattern>,
    },
    /// `(bytes segment…)`
    Bytes(Vec<Segment>),
    /// `(str part…)`: string literals and pins around at most one pattern, the slot, which is
    /// matched against what lies between them. `pinned` holds the pins' expressions (an end whose
    /// text is known when the match runs); a `slot` of `None` is the exact string.
    Str {
        pinned: Vec<Value>,
        slot: Option<Box<Pattern>>,
    },
    /// A form the compiler refuses with an error naming it: `(not …)`, `(unquote …)`, `&` or
    /// `&optional` in a vector, a misplaced marker, `:as` in a map, a malformed record or
    /// bytes segment. It binds nothing and admits nothing a reader may rely on.
    Refused(Value),
}

/// A list pattern's parts: the required elements, the `&optional` slots with their default
/// expressions, and the `& rest` pattern.
#[derive(Clone, Debug, Default)]
pub struct SeqPattern {
    pub required: Vec<Pattern>,
    pub optional: Vec<(Pattern, Option<Value>)>,
    pub rest: Option<Box<Pattern>>,
}

/// A map pattern's parts. `:keys` names bind leniently (an absent key binds its `:or` default
/// or `nil`); every other entry requires its key and matches its value against a pattern.
#[derive(Clone, Debug, Default)]
pub struct MapPattern {
    pub keys: Vec<Symbol>,
    /// `(name, default expression)` from `:or`.
    pub defaults: Vec<(Value, Value)>,
    /// `(key, pattern)`: the key is a literal, never evaluated.
    pub entries: Vec<(Value, Pattern)>,
}

/// One segment of a `(bytes …)` pattern. A binder of `None` is `_`, which consumes without
/// binding.
#[derive(Clone, Debug)]
pub enum Segment {
    /// An int (one byte) or `#b"…"` literal: those exact bytes.
    Literal(Value),
    /// `name`: one byte, as an int.
    Byte(Option<Symbol>),
    /// `(name n)`: `n` bytes, where `n` is an int or an expression over earlier binders.
    Sized { binder: Option<Symbol>, size: Value },
    /// `(name :u16)`: a typed integer.
    Typed {
        binder: Option<Symbol>,
        spec: Symbol,
    },
    /// `& name`: the bytes that remain.
    Rest(Option<Symbol>),
}

impl Pattern {
    /// Parse `pattern` as `std/prelude/match.blsp` compiles it.
    pub fn parse(heap: &Heap, pattern: Value) -> Pattern {
        // A checked file may hold a pattern deeper than the compiler's cap, which refuses it
        // only when the file loads: grow the stack rather than overflow it.
        crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || Self::parse_at(heap, pattern))
    }

    fn parse_at(heap: &Heap, pattern: Value) -> Pattern {
        match pattern {
            Value::Sym(s) if value::symbol_is(s, "_") => Pattern::Wildcard,
            // A marker outside a list's element sequence is no binder.
            Value::Sym(s) if is_marker(s) => Pattern::Refused(pattern),
            Value::Sym(s) => Pattern::Bind(s),
            Value::Pair(_) => match heap.list_items(pattern) {
                Some(items) => Self::parse_list(heap, pattern, &items),
                None => Pattern::Refused(pattern), // a dotted pair
            },
            Value::Vector(id) => {
                let items = heap.vector(id).to_vec();
                if items
                    .iter()
                    .any(|&item| matches!(item, Value::Sym(s) if is_marker(s)))
                {
                    return Pattern::Refused(pattern);
                }
                Pattern::Vector(
                    items
                        .into_iter()
                        .map(|item| Self::parse(heap, item))
                        .collect(),
                )
            }
            Value::Map(id) => match MapPattern::parse(heap, id) {
                Some(map) => Pattern::Map(map),
                None => Pattern::Refused(pattern),
            },
            literal => Pattern::Literal(literal),
        }
    }

    fn parse_list(heap: &Heap, pattern: Value, items: &[Value]) -> Pattern {
        let operands = || {
            items[1..]
                .iter()
                .map(|&item| Self::parse(heap, item))
                .collect()
        };
        match ListForm::of_list(items) {
            ListForm::Quote => match items {
                [_, datum] => Pattern::Quoted(*datum),
                _ => Pattern::Refused(pattern),
            },
            ListForm::Pin => match items {
                [_, expression] => Pattern::Pin(*expression),
                _ => Pattern::Refused(pattern),
            },
            ListForm::Or => Pattern::Or(operands()),
            ListForm::And => Pattern::And(operands()),
            ListForm::Record => match items {
                [_, Value::Sym(name)] => Pattern::Record {
                    name: *name,
                    fields: None,
                },
                [_, Value::Sym(name), Value::Map(id)] => match MapPattern::parse(heap, *id) {
                    Some(fields) => Pattern::Record {
                        name: *name,
                        fields: Some(fields),
                    },
                    None => Pattern::Refused(pattern),
                },
                _ => Pattern::Refused(pattern),
            },
            ListForm::Bytes => match Segment::parse_all(heap, &items[1..]) {
                Some(segments) => Pattern::Bytes(segments),
                None => Pattern::Refused(pattern),
            },
            ListForm::Str => {
                let mut pinned = Vec::new();
                let mut slots = Vec::new();
                for &item in &items[1..] {
                    match item {
                        Value::Str(_) => {}
                        Value::Pair(_) => match heap.list_items(item).as_deref() {
                            Some([Value::Sym(head), expression])
                                if value::symbol_is(*head, kw::PIN) =>
                            {
                                pinned.push(*expression)
                            }
                            _ => slots.push(item),
                        },
                        _ => slots.push(item),
                    }
                }
                match slots.as_slice() {
                    [Value::Sym(s)] if is_marker(*s) => Pattern::Refused(pattern),
                    [] => Pattern::Str { pinned, slot: None },
                    [slot] => Pattern::Str {
                        pinned,
                        slot: Some(Box::new(Self::parse(heap, *slot))),
                    },
                    // a second slot: the text between them could split more than one way
                    _ => Pattern::Refused(pattern),
                }
            }
            ListForm::Not | ListForm::Unquote => Pattern::Refused(pattern),
            ListForm::List => match SeqPattern::parse(heap, items) {
                Some(seq) => Pattern::List(seq),
                None => Pattern::Refused(pattern),
            },
        }
    }

    /// The names the pattern binds, in order, a repeated name once per occurrence — exactly
    /// the prelude's `%pattern-vars`. An `or` binds what its first alternative binds (the
    /// compiler requires every alternative to bind the same names).
    pub fn binders(&self) -> Vec<Symbol> {
        let mut out = Vec::new();
        self.collect_binders(&mut out);
        out
    }

    fn collect_binders(&self, out: &mut Vec<Symbol>) {
        crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || self.collect_binders_at(out));
    }

    fn collect_binders_at(&self, out: &mut Vec<Symbol>) {
        match self {
            Pattern::Bind(name) => out.push(*name),
            Pattern::Wildcard
            | Pattern::Literal(_)
            | Pattern::Quoted(_)
            | Pattern::Pin(_)
            | Pattern::Refused(_) => {}
            Pattern::List(seq) => seq.collect_binders(out),
            Pattern::Vector(items) | Pattern::And(items) => {
                for item in items {
                    item.collect_binders(out);
                }
            }
            Pattern::Or(alternatives) => {
                if let Some(first) = alternatives.first() {
                    first.collect_binders(out);
                }
            }
            Pattern::Map(map) => map.collect_binders(out),
            Pattern::Record { fields, .. } => {
                if let Some(map) = fields {
                    map.collect_binders(out);
                }
            }
            Pattern::Bytes(segments) => out.extend(segments.iter().filter_map(Segment::binder)),
            Pattern::Str { slot, .. } => {
                if let Some(slot) = slot {
                    slot.collect_binders(out);
                }
            }
        }
    }

    /// The expressions written inside the pattern — pins, `&optional` and `:or` defaults,
    /// dynamic segment sizes. They are evaluated, so a name they mention is a use.
    pub fn expressions(&self) -> Vec<Value> {
        let mut out = Vec::new();
        self.collect_expressions(&mut out);
        out
    }

    fn collect_expressions(&self, out: &mut Vec<Value>) {
        crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || self.collect_expressions_at(out));
    }

    fn collect_expressions_at(&self, out: &mut Vec<Value>) {
        match self {
            Pattern::Pin(expression) => out.push(*expression),
            Pattern::Wildcard
            | Pattern::Bind(_)
            | Pattern::Literal(_)
            | Pattern::Quoted(_)
            | Pattern::Refused(_) => {}
            Pattern::List(seq) => {
                for item in &seq.required {
                    item.collect_expressions(out);
                }
                for (slot, default) in &seq.optional {
                    slot.collect_expressions(out);
                    out.extend(default);
                }
                if let Some(rest) = &seq.rest {
                    rest.collect_expressions(out);
                }
            }
            Pattern::Vector(items) | Pattern::And(items) | Pattern::Or(items) => {
                for item in items {
                    item.collect_expressions(out);
                }
            }
            Pattern::Map(map) => map.collect_expressions(out),
            Pattern::Record { fields, .. } => {
                if let Some(map) = fields {
                    map.collect_expressions(out);
                }
            }
            Pattern::Str { pinned, slot } => {
                out.extend(pinned);
                if let Some(slot) = slot {
                    slot.collect_expressions(out);
                }
            }
            Pattern::Bytes(segments) => {
                for segment in segments {
                    if let Segment::Sized { size, .. } = segment
                        && !matches!(size, Value::Int(_))
                    {
                        out.push(*size);
                    }
                }
            }
        }
    }
}

impl SeqPattern {
    /// A list pattern's elements split at its markers; `None` where the compiler refuses the
    /// sequence (`&` not followed by exactly one pattern, `&optional` twice or with no slot).
    pub fn parse(heap: &Heap, items: &[Value]) -> Option<SeqPattern> {
        let mut seq = SeqPattern::default();
        let mut optional = false;
        let mut index = 0;
        while index < items.len() {
            let item = items[index];
            match item {
                Value::Sym(s) if value::symbol_is(s, kw::AMP) => {
                    let [rest] = &items[index + 1..] else {
                        return None;
                    };
                    seq.rest = Some(Box::new(Pattern::parse(heap, *rest)));
                    break;
                }
                Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => {
                    if optional {
                        return None;
                    }
                    optional = true;
                    let starts_a_slot = items.get(index + 1).is_some_and(
                        |next| !matches!(next, Value::Sym(s) if value::symbol_is(*s, kw::AMP)),
                    );
                    if !starts_a_slot {
                        return None;
                    }
                }
                _ if optional => seq.optional.push(optional_slot(heap, item)),
                _ => seq.required.push(Pattern::parse(heap, item)),
            }
            index += 1;
        }
        Some(seq)
    }

    fn collect_binders(&self, out: &mut Vec<Symbol>) {
        for item in &self.required {
            item.collect_binders(out);
        }
        for (slot, _) in &self.optional {
            slot.collect_binders(out);
        }
        if let Some(rest) = &self.rest {
            rest.collect_binders(out);
        }
    }
}

/// An `&optional` slot: `(pattern default)` when it is a two-element list, else the slot is
/// the pattern and its default is `nil`. So a list PATTERN in a slot is written with its
/// default, `((a b) nil)`.
fn optional_slot(heap: &Heap, slot: Value) -> (Pattern, Option<Value>) {
    match heap.list_items(slot).as_deref() {
        Some(&[pattern, default]) if matches!(slot, Value::Pair(_)) => {
            (Pattern::parse(heap, pattern), Some(default))
        }
        _ => (Pattern::parse(heap, slot), None),
    }
}

impl MapPattern {
    /// `None` where the compiler refuses the map: an `:as` key (capture the whole with
    /// `(and whole {…})`).
    pub fn parse(heap: &Heap, id: MapId) -> Option<MapPattern> {
        let mut map = MapPattern::default();
        for (key, entry) in heap.map_entries(id) {
            match key {
                Value::Keyword(k) if value::symbol_is(k, "as") => return None,
                Value::Keyword(k) if value::symbol_is(k, "keys") => {
                    map.keys = heap
                        .list_or_vector_items(entry)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|name| match name {
                            Value::Sym(s) => Some(s),
                            _ => None,
                        })
                        .collect();
                }
                Value::Keyword(k) if value::symbol_is(k, "or") => {
                    if let Value::Map(defaults) = entry {
                        map.defaults = heap.map_entries(defaults);
                    }
                }
                _ => map.entries.push((key, Pattern::parse(heap, entry))),
            }
        }
        Some(map)
    }

    fn collect_binders(&self, out: &mut Vec<Symbol>) {
        out.extend(&self.keys);
        for (_, pattern) in &self.entries {
            pattern.collect_binders(out);
        }
    }

    fn collect_expressions(&self, out: &mut Vec<Value>) {
        out.extend(self.defaults.iter().map(|&(_, default)| default));
        for (_, pattern) in &self.entries {
            pattern.collect_expressions(out);
        }
    }
}

impl Segment {
    /// Every segment of a `(bytes …)` pattern; `None` where the compiler refuses one (a
    /// segment it cannot read, `&` not followed by exactly one binder).
    fn parse_all(heap: &Heap, items: &[Value]) -> Option<Vec<Segment>> {
        let mut segments = Vec::with_capacity(items.len());
        let mut index = 0;
        while index < items.len() {
            let item = items[index];
            let segment = match item {
                Value::Sym(s) if value::symbol_is(s, kw::AMP) => {
                    let [Value::Sym(rest)] = &items[index + 1..] else {
                        return None;
                    };
                    segments.push(Segment::Rest(binder_of(*rest)));
                    break;
                }
                Value::Sym(s) if is_marker(s) => return None,
                Value::Sym(s) => Segment::Byte(binder_of(s)),
                Value::Int(_) | Value::Bytes(_) => Segment::Literal(item),
                Value::Pair(_) => match heap.list_items(item).as_deref() {
                    Some(&[Value::Sym(name), Value::Keyword(spec)]) => Segment::Typed {
                        binder: binder_of(name),
                        spec,
                    },
                    Some(&[Value::Sym(name), size]) => Segment::Sized {
                        binder: binder_of(name),
                        size,
                    },
                    _ => return None,
                },
                _ => return None,
            };
            segments.push(segment);
            index += 1;
        }
        Some(segments)
    }

    /// The name this segment binds.
    pub fn binder(&self) -> Option<Symbol> {
        match self {
            Segment::Byte(binder)
            | Segment::Sized { binder, .. }
            | Segment::Typed { binder, .. }
            | Segment::Rest(binder) => *binder,
            Segment::Literal(_) => None,
        }
    }
}

/// `_` binds nothing.
fn binder_of(name: Symbol) -> Option<Symbol> {
    (!value::symbol_is(name, "_")).then_some(name)
}

/// `&` and `&optional`: list-pattern markers, never binders.
fn is_marker(s: Symbol) -> bool {
    value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_OPTIONAL)
}
