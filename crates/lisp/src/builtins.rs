//! Primitive builtins: the irreducible kernel implemented in Rust. Each takes
//! already-evaluated args, the call-site environment, and `&mut Heap`.
//!
//! Anything that can be written in Brood lives in `std/prelude.blsp` instead.
//! `%`-prefixed names are low-level primitives not meant to be called directly.
//! The annotated list is in `docs/primitives.md`.

use crate::core::heap::Heap;
use crate::core::value::{self, Arity, EnvId, NativeFn, NativeFnPtr, Tag, Value};
use crate::error::{LispError, LispResult};
use crate::eval::apply;
use crate::syntax::{cst, printer, reader};
use crate::types::{Sig, Ty};

/// Install the primitive kernel into `root`.
pub fn register(heap: &mut Heap, root: EnvId) {
    let def = |heap: &mut Heap, name: &str, arity: Arity, sig: Sig, func: NativeFnPtr| {
        let (params, doc) = primitive_doc(name);
        let v = heap.alloc_native(NativeFn {
            name: name.to_string(),
            arity,
            sig,
            func,
            params,
            doc,
        });
        heap.env_define(root, value::intern(name), v);
    };

    // Lattice shorthands used in the signatures below; see types::Ty for the
    // algebra. NUMBER = int ∪ float, LIST = nil ∪ pair, seq = list ∪ vector
    // (the receivers of first/rest). `callable` = fn ∪ native (a thunk or
    // applicable). `ANY` is the "no useful info" lane — overlaps everything,
    // so the disjointness checker never warns against it.
    let any = Ty::ANY;
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let string = Ty::of(Tag::Str);
    let rope = Ty::of(Tag::Rope);
    let socket_ty = Ty::of(Tag::Socket);
    let kw = Ty::of(Tag::Keyword);
    let sym = Ty::of(Tag::Sym);
    let bool_ty = Ty::of(Tag::Bool);
    let nil_ty = Ty::of(Tag::Nil);
    let pair = Ty::of(Tag::Pair);
    let vec_ty = Ty::of(Tag::Vector);
    let map_ty = Ty::of(Tag::Map);
    let pid_ty = Ty::of(Tag::Pid);
    let ref_ty = Ty::of(Tag::Ref);
    let list_ty = Ty::LIST;
    let seq = list_ty.union(vec_ty);
    let callable = Ty::of(Tag::Fn).union(Ty::of(Tag::Native));

    // numeric primitives — `%add`..`%div` accept and return the wider NUMBER
    // (int + int may overflow into Float; the others always do on a Float arg).
    // `%lt` is comparison → bool; `%eq` accepts anything and returns bool.
    def(
        heap,
        "%add",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_add,
    );
    def(
        heap,
        "%sub",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_sub,
    );
    def(
        heap,
        "%mul",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_mul,
    );
    def(
        heap,
        "%div",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_div,
    );
    def(
        heap,
        "%lt",
        Arity::exact(2),
        Sig::new(vec![num, num], bool_ty),
        prim_lt,
    );
    def(
        heap,
        "%eq",
        Arity::exact(2),
        Sig::new(vec![any, any], bool_ty),
        prim_eq,
    );
    // `mod` is Brood over `rem` (std/prelude.blsp); only `rem` is primitive.
    def(
        heap,
        "rem",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        remainder,
    );
    // `floor` is the single irreducible Float→Int crossing; quot/ceil/round/pow/
    // sqrt are all Brood over it + rem/`/`/`*`/`<` (std/prelude.blsp).
    def(
        heap,
        "floor",
        Arity::exact(1),
        Sig::new(vec![num], int),
        floor,
    );

    // bitwise — integer bit-twiddling on the i64 two's-complement representation.
    // Table stakes for hashing, flags, and PRNGs (the std xorshift PRNG is built
    // on these); they were a noted gap (docs/feedback-retro-game-of-life.md).
    def(
        heap,
        "bit-and",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_and,
    );
    def(
        heap,
        "bit-or",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_or,
    );
    def(
        heap,
        "bit-xor",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_xor,
    );
    def(
        heap,
        "bit-not",
        Arity::exact(1),
        Sig::new(vec![int], int),
        bit_not,
    );
    def(
        heap,
        "bit-shift-left",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_shift_left,
    );
    def(
        heap,
        "bit-shift-right",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_shift_right,
    );

    // pair / sequence — `empty?` is Brood (type dispatch over string-length /
    // vector-length / map-keys; std/prelude.blsp). `first`/`rest` ARE the pair
    // accessors (car/cdr), so they stay. `rest` always yields a list (a vector's
    // tail is built via `heap.list`), never a vector.
    def(
        heap,
        "cons",
        Arity::exact(2),
        Sig::new(vec![any, any], pair),
        cons,
    );
    def(
        heap,
        "first",
        Arity::exact(1),
        Sig::new(vec![seq], any),
        first,
    );
    def(
        heap,
        "rest",
        Arity::exact(1),
        Sig::new(vec![seq], list_ty),
        rest,
    );
    // `%sort-asc` is the Rust fast path for the common `(sort coll)` case
    // (ascending by `<`, no custom comparator). Avoids per-comparison Brood
    // eval overhead — the old in-Brood mergesort was ~1.5 s on 10 000 items
    // because every compare went through `eval::apply`. `sort-by` /
    // `(sort cmp coll)` still routes through the Brood merge sort for
    // arbitrary comparators. Items must be all-`int` or all-`float`; mixed
    // numerics work by promotion (matches `<`'s semantics).
    def(
        heap,
        "%sort-asc",
        Arity::exact(1),
        Sig::new(vec![seq], list_ty),
        sort_asc,
    );
    // `%sort-cmp` is the non-numeric fallback for `(sort coll)`: sorts via the
    // Rust-side structural total order (`value_cmp`). Lets `(sort [[1 0] [2 1]])`
    // and the like work without a custom comparator. Brood `sort` (prelude)
    // dispatches: numeric items go through `%sort-asc` (faster), anything else
    // through `%sort-cmp`.
    def(
        heap,
        "%sort-cmp",
        Arity::exact(1),
        Sig::new(vec![seq], list_ty),
        sort_cmp,
    );
    // `(compare a b)` exposes the same structural total order as a binary
    // comparison (-1/0/1), so `sort-by` / `min-by` / custom comparators work over
    // any orderable value (strings, keywords, vectors, …), not just numbers.
    def(
        heap,
        "compare",
        Arity::exact(2),
        Sig::new(vec![any, any], int),
        compare,
    );

    // vector
    def(
        heap,
        "vector",
        Arity::any(),
        Sig::variadic(any, vec_ty),
        vector,
    );
    def(
        heap,
        "vector-ref",
        Arity::exact(2),
        Sig::new(vec![vec_ty, int], any),
        vector_ref,
    );
    def(
        heap,
        "vector-length",
        Arity::exact(1),
        Sig::new(vec![vec_ty], int),
        vector_length,
    );

    // map — the *minimal* kernel: construct, read, two producers, and one
    // enumerator (`map-pairs` → [k v] vectors). `keys`/`vals`/`contains?`/
    // `reduce-kv` and the `get`/`assoc`/`dissoc` surface (variadic + defaults) are
    // all Brood over these (std/prelude.blsp). Maps are immutable: each op returns
    // a fresh map.
    def(
        heap,
        "hash-map",
        Arity::any(),
        Sig::variadic(any, map_ty),
        hash_map,
    );
    def(
        heap,
        "map-get",
        Arity::range(2, 3),
        Sig::with_rest(vec![map_ty, any], any, any),
        map_get,
    );
    def(
        heap,
        "map-assoc",
        Arity::exact(3),
        Sig::new(vec![map_ty, any, any], map_ty),
        map_assoc,
    );
    def(
        heap,
        "map-dissoc",
        Arity::exact(2),
        Sig::new(vec![map_ty, any], map_ty),
        map_dissoc,
    );
    def(
        heap,
        "map-pairs",
        Arity::exact(1),
        Sig::new(vec![map_ty], list_ty),
        map_pairs,
    );

    // string
    def(
        heap,
        "string-length",
        Arity::exact(1),
        Sig::new(vec![string], int),
        string_length,
    );
    def(
        heap,
        "substring",
        Arity::exact(3),
        Sig::new(vec![string, int, int], string),
        substring,
    );
    // Case folding (Unicode tables) and parse-or-nil genuinely need Rust; the rest
    // of the string library (split/join/replace/index-of/trim/…) is Brood over
    // these + `substring`/`str` (std/prelude.blsp).
    def(
        heap,
        "upper",
        Arity::exact(1),
        Sig::new(vec![string], string),
        upper,
    );
    def(
        heap,
        "lower",
        Arity::exact(1),
        Sig::new(vec![string], string),
        lower,
    );
    // string->number returns int *or* float *or* nil (the parse-failed case).
    def(
        heap,
        "string->number",
        Arity::exact(1),
        Sig::new(vec![string], num.union(nil_ty)),
        string_to_number,
    );
    // `to-fixed` renders a number with a fixed count of decimals — the one
    // float→text op `str`/`pr-str` can't express (they print shortest round-trip
    // form, i.e. full f64 precision). `round-to` (a *number*) is Brood over floor.
    def(
        heap,
        "to-fixed",
        Arity::exact(2),
        Sig::new(vec![num, int], string),
        to_fixed,
    );

    // rope — the editor buffer's text storage (ADR-045). The irreducible text
    // mechanism: a `ropey::Rope` gives O(log n) edits + char/line indexing that
    // Brood can't bootstrap over flat strings. Immutable like every value —
    // `rope-insert`/`rope-delete` return a *fresh* rope (cheap structural share).
    // Points, marks, regions, search, the buffer process itself: all Brood above.
    def(
        heap,
        "string->rope",
        Arity::exact(1),
        Sig::new(vec![string], rope),
        string_to_rope,
    );
    def(
        heap,
        "rope->string",
        Arity::exact(1),
        Sig::new(vec![rope], string),
        rope_to_string,
    );
    def(
        heap,
        "rope-length",
        Arity::exact(1),
        Sig::new(vec![rope], int),
        rope_length,
    );
    def(
        heap,
        "rope-line-count",
        Arity::exact(1),
        Sig::new(vec![rope], int),
        rope_line_count,
    );
    def(
        heap,
        "rope-insert",
        Arity::exact(3),
        Sig::new(vec![rope, int, string], rope),
        rope_insert,
    );
    def(
        heap,
        "rope-delete",
        Arity::exact(3),
        Sig::new(vec![rope, int, int], rope),
        rope_delete,
    );
    def(
        heap,
        "rope-slice",
        Arity::exact(3),
        Sig::new(vec![rope, int, int], string),
        rope_slice,
    );
    def(
        heap,
        "rope-line",
        Arity::exact(2),
        Sig::new(vec![rope, int], string),
        rope_line,
    );
    def(
        heap,
        "rope-char->line",
        Arity::exact(2),
        Sig::new(vec![rope, int], int),
        rope_char_to_line,
    );
    def(
        heap,
        "rope-line->char",
        Arity::exact(2),
        Sig::new(vec![rope, int], int),
        rope_line_to_char,
    );

    // TCP sockets (ADR-062), built on the blocking-IO → mailbox seam (ADR-059):
    // inbound data is delivered to the owning process's mailbox as `[:tcp sock
    // data]` / `[:tcp-closed sock]` / `[:tcp-accept lsock client]` messages, which
    // Brood `receive`s — no polling, no worker ever blocked. `connect`/`listen`
    // register the *calling* process as the owner. A socket is an opaque handle,
    // valid across this runtime's processes, never sent across nodes.
    def(
        heap,
        "tcp-connect",
        Arity::exact(2),
        Sig::new(vec![string, int], socket_ty),
        tcp_connect,
    );
    def(
        heap,
        "tcp-listen",
        Arity::exact(2),
        Sig::new(vec![string, int], socket_ty),
        tcp_listen,
    );
    def(
        heap,
        "tls-request",
        Arity::exact(3),
        Sig::new(vec![string, int, string], socket_ty),
        tls_request,
    );
    def(
        heap,
        "tcp-send",
        Arity::exact(2),
        Sig::new(vec![socket_ty, string], nil_ty),
        tcp_send,
    );
    def(
        heap,
        "tcp-controlling-process",
        Arity::exact(2),
        Sig::new(vec![socket_ty, pid_ty], nil_ty),
        tcp_controlling_process,
    );
    def(
        heap,
        "tcp-close",
        Arity::exact(1),
        Sig::new(vec![socket_ty], nil_ty),
        tcp_close,
    );
    def(
        heap,
        "tcp-local-port",
        Arity::exact(1),
        Sig::new(vec![socket_ty], int.union(nil_ty)),
        tcp_local_port,
    );

    // terminal frontend (ADR-046) — the thin crossterm seam that paints the
    // display protocol and reads keys. The protocol itself is Brood data (a
    // vector of render ops); these primitives are mechanism only. `term-poll`
    // returns a key (a 1-char string, or a keyword for specials) or nil on
    // timeout; `term-draw` interprets a frame vector. See std/observer.blsp.
    def(
        heap,
        "term-enter",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_enter,
    );
    def(
        heap,
        "term-leave",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_leave,
    );
    def(
        heap,
        "term-size",
        Arity::exact(0),
        Sig::new(vec![], vec_ty),
        term_size,
    );
    def(
        heap,
        "term-poll",
        Arity::exact(1),
        Sig::new(vec![int], string.union(kw).union(nil_ty)),
        term_poll,
    );
    def(
        heap,
        "term-draw",
        Arity::exact(1),
        Sig::new(vec![vec_ty], nil_ty),
        term_draw,
    );
    // Inline (relative-motion) variant of the seam, for an in-place line editor
    // that must NOT take over the screen: `term-raw-enter`/`term-raw-leave` toggle
    // raw mode only (no alternate screen, cursor stays visible, scrollback kept),
    // and `term-emit` paints relative ops. The self-hosted REPL editor uses these
    // (std/lineedit.blsp); `term-enter`/`term-draw` stay the full-screen path.
    def(
        heap,
        "term-raw-enter",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_raw_enter,
    );
    def(
        heap,
        "term-raw-leave",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_raw_leave,
    );
    def(
        heap,
        "term-emit",
        Arity::exact(1),
        Sig::new(vec![vec_ty], nil_ty),
        term_emit,
    );
    // The windowed (GUI) frontend — the same seam as `term-*`, painting the same
    // render-op protocol to a native window (feature "gui"; the symbols always
    // exist, erroring at call time without the feature). Unlike the single
    // terminal, there can be many windows: `gui-open` returns an integer window id
    // and the other primitives take it, so `(observe)` can spawn several at once.
    // std/observer.blsp's `gui-display` wraps an id as a display map. See gui.rs.
    def(heap, "gui-open", Arity::exact(0), Sig::new(vec![], int), gui_open);
    def(
        heap,
        "gui-close",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        gui_close,
    );
    def(
        heap,
        "gui-size",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        gui_size,
    );
    def(
        heap,
        "gui-draw",
        Arity::exact(2),
        Sig::new(vec![int, vec_ty], nil_ty),
        gui_draw,
    );
    // The font seam: a global default cell font (`gui-font!`) and runtime family
    // registration (`gui-font-register`); a face's `:family`/`:italic` then select
    // per-section, within the fixed cell grid. (gui feature; error without it.)
    def(
        heap,
        "gui-font!",
        Arity::exact(1),
        Sig::new(vec![map_ty], nil_ty),
        gui_font,
    );
    def(
        heap,
        "gui-font-register",
        Arity::exact(2),
        Sig::new(vec![kw, map_ty], kw),
        gui_font_register,
    );
    // The one process-introspection accessor the language can't reach from Brood
    // (the mailbox queue lives behind the scheduler registry). Everything else an
    // observer shows — pid id, liveness — is assembled in Brood (std/observer.blsp).
    def(
        heap,
        "mailbox-size",
        Arity::exact(1),
        Sig::new(vec![pid_ty], int.union(nil_ty)),
        mailbox_size,
    );
    // `(process-info pid)` — an Erlang-`process_info`-style snapshot map for a
    // live local process (nil for remote/dead), the introspection surface a
    // process observer/debugger reads. Assembled in Rust because every field is
    // kernel-internal (registry / scheduler / monitor tables). ADR-051.
    def(
        heap,
        "process-info",
        Arity::exact(1),
        Sig::new(vec![pid_ty], map_ty.union(nil_ty)),
        process_info,
    );

    // type reflection — the tag predicates (nil?/int?/string?/…) are Brood
    // (std/prelude.blsp) over this one reflective primitive.
    def(
        heap,
        "type-of",
        Arity::exact(1),
        Sig::new(vec![any], kw),
        type_of,
    );

    // value <-> text and I/O
    def(
        heap,
        "str",
        Arity::any(),
        Sig::variadic(any, string),
        str_concat,
    );
    def(
        heap,
        "pr-str",
        Arity::exact(1),
        Sig::new(vec![any], string),
        pr_str,
    );
    def(
        heap,
        "print",
        Arity::any(),
        Sig::variadic(any, nil_ty),
        print,
    );
    def(
        heap,
        "eprint",
        Arity::any(),
        Sig::variadic(any, nil_ty),
        eprint,
    );
    def(
        heap,
        "read-line",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        read_line,
    );
    // `println` is Brood over `print` (std/prelude.blsp).
    def(
        heap,
        "stdout-tty?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        stdout_tty,
    );
    def(
        heap,
        "stdin-tty?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        stdin_tty,
    );

    // time
    def(heap, "now", Arity::exact(0), Sig::nullary(int), now);
    def(heap, "now-ns", Arity::exact(0), Sig::nullary(int), now_ns);

    // memory
    def(
        heap,
        "mem-bytes",
        Arity::exact(0),
        Sig::nullary(int),
        mem_bytes,
    );
    def(
        heap,
        "mem-peak",
        Arity::exact(0),
        Sig::nullary(int),
        mem_peak,
    );
    def(
        heap,
        "mem-limit",
        Arity::exact(0),
        Sig::nullary(int),
        mem_limit,
    );
    def(
        heap,
        "mem-soft-limit",
        Arity::exact(0),
        Sig::nullary(int),
        mem_soft_limit,
    );
    def(
        heap,
        "gc-stats",
        Arity::exact(0),
        Sig::nullary(map_ty),
        gc_stats,
    );

    // self-hosting — eval/load/etc. take and return arbitrary forms / values.
    def(
        heap,
        "eval",
        Arity::exact(1),
        Sig::new(vec![any], any),
        eval_builtin,
    );
    def(
        heap,
        "read-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        read_string,
    );
    def(
        heap,
        "eval-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        eval_string,
    );
    def(
        heap,
        "%load-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        load_string,
    );
    // CST parse — mechanism for the in-Brood formatter (std/format.blsp); never
    // fails (malformed input becomes [:error "..."] nodes). Returns nested
    // vectors; see `parse_source` for the shape.
    def(
        heap,
        "parse-source",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        parse_source,
    );
    def(
        heap,
        "load",
        Arity::exact(1),
        Sig::new(vec![string], any),
        load,
    );
    def(
        heap,
        "reload-defs",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        reload_defs,
    );
    def(
        heap,
        "%builtin-module",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        builtin_module,
    );
    def(
        heap,
        "%builtin-doc",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        builtin_doc,
    );
    // `apply`'s last positional arg must be a sequence (it's spliced); the
    // intermediate args can be anything. The `Sig` algebra can express
    // "prefix + repeating tail" but not "the *last* item of the tail is
    // special", so the Sig is `(callable, ...any) -> any` — the closest
    // honest approximation. The sequence-at-tail constraint is checked at
    // call time by `apply_builtin` via `heap.seq_items(args[last])`, which
    // surfaces a `wrong_type` error if the last arg isn't a seq. So the
    // Sig is loose, but the runtime is tight.
    def(
        heap,
        "apply",
        Arity::at_least(2),
        Sig::with_rest(vec![callable], any, any),
        apply_builtin,
    );

    // symbols
    def(
        heap,
        "name",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string),
        name_of,
    );
    def(
        heap,
        "symbol",
        Arity::exact(1),
        Sig::new(vec![string.union(sym).union(kw)], sym),
        to_symbol,
    );
    def(
        heap,
        "keyword",
        Arity::exact(1),
        Sig::new(vec![string.union(sym).union(kw)], kw),
        to_keyword,
    );

    // filesystem — mechanism for the Brood module system + project test runner
    def(heap, "cwd", Arity::exact(0), Sig::nullary(string), cwd);
    def(
        heap,
        "file-exists?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        file_exists,
    );
    def(
        heap,
        "dir?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        is_dir,
    );
    def(
        heap,
        "list-dir",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        list_dir,
    );
    def(
        heap,
        "make-dir",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        make_dir,
    );
    def(
        heap,
        "spit",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        spit,
    );
    def(
        heap,
        "slurp",
        Arity::exact(1),
        Sig::new(vec![string], string),
        slurp,
    );
    def(
        heap,
        "file-mtime",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        file_mtime,
    );
    def(
        heap,
        "file-size",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        file_size,
    );
    def(
        heap,
        "delete-file",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        delete_file,
    );
    def(
        heap,
        "rename-file",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        rename_file,
    );
    // The one hashing primitive (ADR-037): SHA-256 of a string's bytes → hex.
    // Per-file and directory-tree hashing for the package manager are Brood over
    // this + `slurp`/`list-dir` (std/package.blsp), not a directory primitive.
    def(
        heap,
        "%sha256",
        Arity::exact(1),
        Sig::new(vec![string], string),
        sha256_hex,
    );
    // The package manager's git mechanism (ADR-037): resolve a ref to a commit,
    // and clone+checkout a pinned commit. Thin shell-outs to `git`; the cache
    // layout / lock file / conflict policy are all Brood (std/package.blsp).
    def(
        heap,
        "%git-resolve-ref",
        Arity::exact(2),
        Sig::new(vec![string, string], string.union(nil_ty)),
        git_resolve_ref,
    );
    def(
        heap,
        "%git-clone",
        Arity::exact(4),
        Sig::new(vec![string, string, string, string], kw),
        git_clone,
    );
    // Delete a cached dependency tree. Bounded to paths under `_deps/` — refuses
    // anything else, so a mis-pathed `nest update` can't rm the wrong directory.
    def(
        heap,
        "%rm-rf",
        Arity::exact(1),
        Sig::new(vec![string], kw),
        rm_rf,
    );

    // system / environment
    def(
        heap,
        "getenv",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        getenv,
    );
    def(
        heap,
        "run-process",
        Arity::exact(2),
        Sig::new(vec![string, seq], int),
        run_process,
    );

    // macros
    def(
        heap,
        "macroexpand-1",
        Arity::exact(1),
        Sig::new(vec![any], any),
        macroexpand_1,
    );
    // `macroexpand` (the fixpoint loop) is written in Brood (`std/prelude.blsp`)
    // over this single-step primitive — ADR-064, so its loop state is auto-rooted
    // rather than hand-rooted in Rust. `macros::macroexpand` (Rust) stays for the
    // compile pass, which runs under MACRO_BLOCK.
    // gensym accepts anything as a prefix (string/sym/keyword/nil/anything is
    // turned into its `display` form), so its prefix slot is `any` — not the
    // narrower `string` the original Sig claimed, which made the checker warn
    // on legitimate `(gensym 'foo)` calls.
    def(
        heap,
        "gensym",
        Arity::range(0, 1),
        Sig::new(vec![any], sym),
        gensym,
    );

    // advisory type checker (the Ty lattice's first consumer; see docs/types.md)
    def(
        heap,
        "check",
        Arity::exact(1),
        Sig::new(vec![any], list_ty),
        check_builtin,
    );
    def(
        heap,
        "check-file",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        check_file_builtin,
    );
    def(
        heap,
        "check-file-structured",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        check_file_structured,
    );

    // source positions (editor tooling; see docs/tooling.md)
    def(
        heap,
        "form-pos",
        Arity::exact(1),
        Sig::new(vec![any], vec_ty.union(nil_ty)),
        form_pos,
    );
    def(
        heap,
        "current-file",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        current_file,
    );
    def(
        heap,
        "source-location",
        Arity::exact(1),
        Sig::new(vec![sym], vec_ty.union(nil_ty)),
        source_location,
    );
    def(
        heap,
        "references-in-source",
        Arity::exact(2),
        Sig::new(vec![sym.union(string), string], any),
        references_in_source,
    );

    // introspection (editor tooling; see docs/lsp.md) — derive what we can from
    // the bound value (arglist, doc); enumerate the global table for completion.
    def(
        heap,
        "doc",
        Arity::exact(1),
        Sig::new(vec![any], string.union(nil_ty)),
        doc,
    );
    def(
        heap,
        "arglist",
        Arity::exact(1),
        Sig::new(vec![any], list_ty),
        arglist,
    );
    def(
        heap,
        "global-names",
        Arity::exact(0),
        Sig::nullary(list_ty),
        global_names,
    );
    def(
        heap,
        "special-forms",
        Arity::exact(0),
        Sig::nullary(list_ty),
        special_forms,
    );
    def(
        heap,
        "bound?",
        Arity::exact(1),
        Sig::new(vec![sym], bool_ty),
        bound_p,
    );

    // errors / control
    def(
        heap,
        "throw",
        Arity::exact(1),
        Sig::new(vec![any], Ty::NEVER),
        throw,
    );
    // `%force-panic` — deliberately panics the Rust thread when called. Exists
    // *only* in debug builds: it gives the MCP-host panic-isolation regression
    // test a reliable trigger without adding a "intentionally crash" knob to
    // the release surface. `cargo test` (and `nest test` against a debug
    // binary) sees it; `--release` binaries don't.
    #[cfg(debug_assertions)]
    def(
        heap,
        "%force-panic",
        Arity::range(0, 1),
        Sig::new(vec![any], Ty::NEVER),
        force_panic,
    );
    // Shared-blob inspection primitives — debug-only because they leak the
    // representation (a raw pointer) and because they only exist to assert
    // identity / leak-freedom across processes in the blob-share test. Both
    // return `nil` for an inline string or a non-LOCAL handle (PRELUDE/RUNTIME).
    #[cfg(debug_assertions)]
    def(
        heap,
        "%blob-ptr",
        Arity::exact(1),
        Sig::new(vec![string], Ty::ANY),
        blob_ptr,
    );
    #[cfg(debug_assertions)]
    def(
        heap,
        "%blob-strong-count",
        Arity::exact(1),
        Sig::new(vec![string], Ty::ANY),
        blob_strong_count,
    );
    def(
        heap,
        "%try",
        Arity::exact(2),
        Sig::new(vec![callable, callable], any),
        try_catch,
    );
    def(
        heap,
        "%isolate",
        Arity::exact(1),
        Sig::new(vec![callable], any),
        isolate,
    );

    // dynamic variables (the `defdyn`/`binding` surface is Brood — see prelude)
    def(
        heap,
        "%declare-dynamic",
        Arity::exact(1),
        Sig::new(vec![sym], nil_ty),
        declare_dynamic,
    );
    // Namespaces (ADR-065): `%in-ns` sets the namespace being compiled into. The
    // `ns` macro (prelude) emits it; the resolver pass reads `heap.compile_ns`.
    def(heap, "%in-ns", Arity::exact(1), Sig::new(vec![sym], sym), in_ns);
    def(heap, "current-ns", Arity::exact(0), Sig::new(vec![], sym), current_ns);
    // `(%refer 'mod subset)` — populate the current file's import table from a
    // `(:use …)` clause. `subset` is nil (refer all public names) or a seq of
    // bare symbols. The `ns` macro emits it after `(require 'mod)`.
    def(heap, "%refer", Arity::exact(2), Sig::new(vec![sym, any], nil_ty), refer);
    // `%binding`'s first arg is the *list/vector of names*, second is the
    // *list/vector of values*, third is the thunk — the macro `binding` emits
    // these as `(quote (*a* *b* …))` + `[v1 v2 …]` + `(fn () …)`.
    def(
        heap,
        "%binding",
        Arity::exact(3),
        Sig::new(vec![seq, seq, callable], any),
        binding,
    );
    def(
        heap,
        "dynamic?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        dynamic_p,
    );

    // processes (concurrency)
    def(
        heap,
        "%spawn",
        Arity::exact(1),
        Sig::new(vec![callable], pid_ty),
        spawn,
    );
    def(
        heap,
        "%spawn-named",
        Arity::exact(2),
        Sig::new(vec![sym.union(kw), callable], pid_ty),
        spawn_named,
    );
    // `send`'s target is a pid OR a `{:name :node}` address map.
    def(
        heap,
        "send",
        Arity::exact(2),
        Sig::new(vec![pid_ty.union(map_ty), any], nil_ty),
        send,
    );
    // Arg shape: (matcher: callable, timeout: int|nil, on-timeout: callable|nil).
    // The `receive` macro in `std/prelude.blsp` expands to exactly this; the
    // `callable|nil` on the third position is for the no-`after`-clause case
    // (the macro passes `nil`).
    def(
        heap,
        "%receive",
        Arity::exact(3),
        Sig::new(
            vec![callable, int.union(nil_ty), callable.union(nil_ty)],
            any,
        ),
        receive_match,
    );
    def(
        heap,
        "self",
        Arity::exact(0),
        Sig::nullary(pid_ty),
        self_pid,
    );
    def(heap, "ref", Arity::exact(0), Sig::nullary(ref_ty), make_ref);
    // `(exit pid reason)` — send an exit signal (Erlang `exit/2`). `:kill` is the
    // untrappable hard kill; any other reason is the soft (next-`receive`) signal.
    def(heap, "exit", Arity::exact(2), Sig::new(vec![pid_ty, any], nil_ty), exit_proc);
    // `monitor` also accepts a name map (forwarded to the remote node).
    def(
        heap,
        "monitor",
        Arity::exact(1),
        Sig::new(vec![pid_ty.union(map_ty)], ref_ty),
        monitor,
    );
    def(
        heap,
        "demonitor",
        Arity::exact(1),
        Sig::new(vec![ref_ty], nil_ty),
        demonitor,
    );
    // Links (ADR-067): symmetric failure coupling + `trap_exit`, the bidirectional
    // cousin of `monitor`. `link`/`unlink` couple the current process to a pid;
    // `trap-exit` turns a linked peer's death into a `[:EXIT pid reason]` message.
    def(heap, "link", Arity::exact(1), Sig::new(vec![pid_ty], nil_ty), link_proc);
    def(heap, "unlink", Arity::exact(1), Sig::new(vec![pid_ty], nil_ty), unlink_proc);
    def(heap, "trap-exit", Arity::exact(1), Sig::new(vec![any], bool_ty), trap_exit_proc);
    def(
        heap,
        "spawn-count",
        Arity::exact(0),
        Sig::nullary(int),
        spawn_count,
    );
    def(
        heap,
        "peak-threads",
        Arity::exact(0),
        Sig::nullary(int),
        peak_threads,
    );
    def(
        heap,
        "worker-threads",
        Arity::exact(0),
        Sig::nullary(int),
        worker_threads,
    );
    def(
        heap,
        "list-processes",
        Arity::exact(0),
        Sig::nullary(list_ty),
        list_processes,
    );

    // distributed nodes (connect two runtimes over TCP — crate::dist)
    def(
        heap,
        "node-start",
        Arity::exact(3),
        Sig::new(vec![sym, string, string], sym),
        node_start,
    );
    def(
        heap,
        "connect",
        Arity::exact(1),
        Sig::new(vec![string], sym),
        connect,
    );
    def(
        heap,
        "register",
        Arity::exact(2),
        Sig::new(vec![sym, pid_ty], pid_ty),
        register_name,
    );
    def(
        heap,
        "whereis",
        Arity::exact(1),
        Sig::new(vec![sym], pid_ty.union(nil_ty)),
        whereis_name,
    );
    // `node-name` is the keyword `:nonode` until `node-start` sets it to a symbol.
    def(
        heap,
        "node-name",
        Arity::exact(0),
        Sig::nullary(sym.union(kw)),
        node_name,
    );
    def(heap, "nodes", Arity::exact(0), Sig::nullary(list_ty), nodes);
    def(
        heap,
        "monitor-node",
        Arity::exact(1),
        Sig::new(vec![sym], ref_ty),
        monitor_node,
    );
}

/// Docstrings + parameter names for the public primitives, so `(doc 'name)`,
/// `(arglist 'name)`, and LSP hover treat a Rust builtin like a Brood `defn`
/// (which can't apply here — primitives have no source body). One row per
/// user-facing primitive; mirrors the "Purpose" column of `docs/primitives.md`.
/// `&` in the params marks a rest (variadic) tail. Internal `%`-prefixed
/// primitives are intentionally absent (they aren't meant to be called directly).
#[rustfmt::skip]
static PRIMITIVE_DOCS: &[(&str, &[&str], &str)] = &[
    ("rem", &["a", "b"], "Integer remainder of a / b (truncated, taking the sign of the dividend)."),
    ("floor", &["x"], "Round x toward negative infinity to an integer."),
    ("bit-and", &["a", "b"], "Bitwise AND of integers a and b."),
    ("bit-or", &["a", "b"], "Bitwise (inclusive) OR of integers a and b."),
    ("bit-xor", &["a", "b"], "Bitwise exclusive-OR of integers a and b."),
    ("bit-not", &["a"], "Bitwise complement of integer a (two's-complement, so (bit-not n) = (- (- n) 1))."),
    ("bit-shift-left", &["a", "n"], "Shift integer a left by n bits (0 <= n < 64); bits shifted past bit 63 are discarded."),
    ("bit-shift-right", &["a", "n"], "Arithmetic (sign-preserving) right shift of integer a by n bits (0 <= n < 64)."),
    ("cons", &["x", "xs"], "A new pair with head x and tail xs."),
    ("first", &["coll"], "The head of a list or vector, or nil if empty."),
    ("rest", &["coll"], "All but the head of a list or vector."),
    ("vector", &["&", "items"], "A vector of the given items."),
    ("vector-ref", &["v", "i"], "The element at index i of vector v."),
    ("vector-length", &["v"], "The number of elements in vector v."),
    ("compare", &["a", "b"], "Structural total-order comparison: -1 if a sorts before b, 0 if equal, 1 if after. Numbers numerically; strings/keywords/symbols by text; vectors/lists lexicographically; cross-kind by a stable tag rank. The binary form of `sort`'s order — `sort-by` and custom comparators build on it."),
    ("hash-map", &["&", "kvs"], "A map from alternating key/value arguments (last wins on duplicate keys)."),
    ("map-get", &["m", "k", "default"], "The value at key k in map m, or default (else nil)."),
    ("map-assoc", &["m", "k", "v"], "A fresh map like m with key k set to v."),
    ("map-dissoc", &["m", "k"], "A fresh map like m with key k removed."),
    ("map-pairs", &["m"], "The entries of m as a list of [k v] vectors, in insertion order."),
    ("string-length", &["s"], "The number of characters in string s."),
    ("substring", &["s", "start", "end"], "The characters of s in the range [start, end), char-indexed."),
    ("upper", &["s"], "s upper-cased (Unicode-aware)."),
    ("lower", &["s"], "s lower-cased (Unicode-aware)."),
    ("to-fixed", &["x", "n"], "Render number x as a string with exactly n digits after the decimal point (rounded). n must be >= 0."),
    ("string->number", &["s"], "Parse s strictly as an int, else a float, else nil (unlike read-string)."),
    ("string->rope", &["s"], "A rope (editor buffer text) holding the characters of string s."),
    ("rope->string", &["r"], "The full text of rope r as a string."),
    ("rope-length", &["r"], "The number of characters in rope r."),
    ("rope-line-count", &["r"], "The number of lines in rope r (a trailing newline ends a line; \"\" is 1 line)."),
    ("rope-insert", &["r", "idx", "s"], "A fresh rope with string s inserted at character index idx."),
    ("rope-delete", &["r", "start", "end"], "A fresh rope with characters [start, end) removed."),
    ("rope-slice", &["r", "start", "end"], "The text of characters [start, end) of rope r, as a string."),
    ("rope-line", &["r", "n"], "The text of line n (0-based) of rope r, including any trailing newline."),
    ("rope-char->line", &["r", "idx"], "The 0-based line index containing character idx."),
    ("rope-line->char", &["r", "n"], "The character index where line n (0-based) begins."),
    ("tcp-connect", &["host", "port"], "Connect to host:port; inbound data is delivered to the calling process as [:tcp sock data] / [:tcp-closed sock] messages. Returns a socket. Throws on failure."),
    ("tcp-listen", &["host", "port"], "Bind a listening socket on host:port (port 0 = OS-assigned); connections arrive as [:tcp-accept lsock client] messages to the calling process. Returns a socket."),
    ("tls-request", &["host", "port", "request"], "Make one HTTPS request to host:port (TLS): the response arrives at the calling process as [:tcp sock data] … [:tcp-closed sock] messages (or [:tcp-error sock msg]). Returns a socket id; pair with tcp-drain. Low-level — prefer http-get."),
    ("tcp-send", &["sock", "s"], "Write the whole string s to sock (blocking). Returns nil; throws on error."),
    ("tcp-controlling-process", &["sock", "pid"], "Make pid the owner of sock's inbound data: starts reading a just-accepted (passive) socket, or retargets an active one. Returns nil."),
    ("tcp-close", &["sock"], "Close sock (a stream or listener), releasing its fd / stopping its accept loop. Idempotent; returns nil."),
    ("tcp-local-port", &["sock"], "The local port sock is bound to, or nil."),
    ("type-of", &["x"], "The runtime type of x as a keyword (:int, :string, :pair, ...)."),
    ("check", &["form"], "Advisory type-check a quoted form: a list of warning strings, or nil. Never raises."),
    ("check-file", &["path"], "Advisory type-check every top-level form in the file at path: a list of `path:line:col: warning: …` strings, or nil. Does not evaluate the file."),
    ("check-file-structured", &["path"], "Like check-file but returns a list of `{:file :line :col :message}` maps instead of GNU-format strings — for tools (the `nest mcp` `check` tool, editor diagnostics)."),
    ("str", &["&", "xs"], "Concatenate the display forms of the arguments into one string."),
    ("pr-str", &["x"], "The readable (re-readable) text form of x."),
    ("print", &["&", "xs"], "Write the display forms of the arguments to stdout; returns nil."),
    ("eprint", &["&", "xs"], "Write the display forms of the arguments to stderr; returns nil."),
    ("stdout-tty?", &[], "True when stdout is an interactive terminal (false when piped or captured)."),
    ("stdin-tty?", &[], "True when stdin is an interactive terminal (false when redirected from a pipe or file). The REPL gates raw-mode line editing on this."),
    ("now", &[], "Wall-clock milliseconds since the Unix epoch."),
    ("now-ns", &[], "Wall-clock nanoseconds since the Unix epoch (finer-grained than now)."),
    ("mem-bytes", &[], "Bytes currently allocated process-wide."),
    ("mem-peak", &[], "High-water mark of allocated bytes since process start."),
    ("mem-limit", &[], "Hard memory ceiling in bytes (0 = unlimited); crossing it aborts the process. Set via BROOD_MEM_LIMIT."),
    ("mem-soft-limit", &[], "Soft memory ceiling in bytes (0 = unlimited); crossing it raises a catchable E0043 at the next safepoint."),
    ("gc-stats", &[], "A snapshot map of this process's GC activity: :collections, :copied, :reclaimed (cumulative object counts), :live, :live-bytes, and :threshold (next-collection trigger). Per-process — reports the caller's own heap."),
    ("eval", &["form"], "Evaluate a form in the global environment."),
    ("read-string", &["s"], "Parse and return the first form in string s."),
    ("parse-source", &["s"], "Parse s into a lossless CST tree as nested vectors (mechanism for std/format.blsp)."),
    ("eval-string", &["s"], "Read and evaluate every form in string s (the string analogue of load)."),
    ("load", &["path"], "Read and evaluate every form in the file at path."),
    ("reload-defs", &["path"], "Re-evaluate only the def-style top-level forms in `path` (def, defn, defmacro, defmodule, defdyn, …) — skipping other top-level calls. Used by file watchers to refresh code without re-running side-effecting top-level calls like a `(main-loop)`. Returns nil."),
    ("apply", &["f", "&", "args"], "Call f with the leading args plus the final list argument spliced in as trailing args."),
    ("name", &["x"], "The spelling of a symbol or keyword as a string (no leading colon)."),
    ("symbol", &["x"], "Coerce a string, symbol, or keyword to the matching symbol (interning if needed)."),
    ("keyword", &["x"], "Coerce a string, symbol, or keyword to the matching keyword (interning if needed)."),
    ("cwd", &[], "The current working directory."),
    ("file-exists?", &["path"], "Whether path exists."),
    ("dir?", &["path"], "Whether path is a directory."),
    ("list-dir", &["path"], "The entry names directly under directory path, sorted."),
    ("make-dir", &["path"], "Create a directory and any missing parents (like mkdir -p)."),
    ("spit", &["path", "s"], "Write string s to the file at path."),
    ("slurp", &["path"], "Read the whole file at path into a string (does not evaluate it)."),
    ("%sha256", &["s"], "Lowercase hex SHA-256 of string s's bytes. The package manager's one hashing primitive (ADR-037); file/tree hashing is Brood over it."),
    ("%git-resolve-ref", &["url", "ref"], "Resolve git `ref` (tag/branch/commit) at remote `url` to a commit hash (via `git ls-remote`), or nil if not found. The package manager's ref-pinning mechanism (ADR-037)."),
    ("%git-clone", &["url", "dest", "ref", "commit"], "Shallow-clone `url` into `dest` and check out the exact `commit` (detached); `ref` is the fetch fallback. Returns :ok or throws. The package manager's fetch mechanism (ADR-037)."),
    ("%rm-rf", &["path"], "Recursively delete `path`. Bounded to paths under `_deps/` (refuses anything else). Idempotent. The package manager's cache-eviction mechanism (ADR-037)."),
    ("read-line", &[], "Read one line from stdin; returns the line as a string (trailing newline stripped) or nil at end of input."),
    ("file-mtime", &["path"], "Last-modified time of path as epoch-milliseconds, or nil if the file is missing. Cheap (stat) — pair with `load` to drive a hot-reloader."),
    ("file-size", &["path"], "Size of the file at path in bytes, or nil if it is missing."),
    ("delete-file", &["path"], "Remove the file at path. Idempotent (nil if already absent); errors on a real I/O failure."),
    ("rename-file", &["from", "to"], "Rename/move file `from` to `to`. Returns nil; errors on failure."),
    ("getenv", &["name"], "The value of environment variable name, or nil if unset."),
    ("run-process", &["prog", "args"], "Run external program prog with an args list, inheriting stdio; returns its exit code."),
    ("macroexpand-1", &["form"], "Expand form by a single macro step."),
    // `macroexpand` is a Brood prelude fn (ADR-064), documented via its docstring.
    ("gensym", &["prefix"], "A fresh, unique symbol, with an optional name prefix."),
    ("form-pos", &["form"], "A form's [line col] source position, or nil."),
    ("current-file", &[], "The path of the file currently being loaded, or nil."),
    ("source-location", &["name"], "Where global name was defined, as [file line col], or nil. Quote it: (source-location 'foo)."),
    ("references-in-source", &["name", "source"], "Occurrences of the global `name` in `source`, as a list of [line col] (1-based); locals that shadow it are excluded."),
    ("doc", &["f"], "The docstring of a function, macro, or primitive, or nil."),
    ("arglist", &["f"], "The parameter list of a function, macro, or primitive, or nil."),
    ("global-names", &[], "Every globally bound symbol, sorted by spelling."),
    ("special-forms", &[], "The special-form / core-macro names (strings) that read as keywords — the canonical list shared by the syntax highlighter and the LSP."),
    ("bound?", &["sym"], "Whether sym is bound in scope. Quote it: (bound? 'foo)."),
    ("dynamic?", &["x"], "Whether x is a symbol declared dynamic with defdyn. Quote it: (dynamic? '*foo*)."),
    ("throw", &["x"], "Raise x as an error - a non-local exit caught by try/catch."),
    ("%spawn", &["thunk"], "Run thunk (a 0-arg fn) in a new green process; returns its pid. Use the `spawn` macro."),
    ("send", &["target", "msg"], "Copy msg into target's mailbox; target is a pid or {:name :node} address. Routes locally or over a node link. Returns nil."),
    ("self", &[], "This process's own pid (carries this node's identity)."),
    ("exit", &["pid", "reason"], "Send an exit signal to process pid, local or remote (Erlang exit/2). reason :kill is the untrappable hard kill — pid dies at its next reduction tick, or immediately if parked. Any other reason is the soft signal — pid dies at its next receive. Monitors fire [:down ref pid reason]. A remote pid is routed to its node over the link. No-op for a dead/unknown pid. Returns nil."),
    ("ref", &[], "A fresh, globally-unique reference token (tags a request to its reply)."),
    ("monitor", &["pid"], "Watch pid; returns a monitor ref. Delivers [:down ref pid reason] when pid dies."),
    ("list-processes", &[], "Every currently-live pid on this runtime (one per registered mailbox). Order is unspecified — sort if you need stability. For agents/tools enumerating spawned processes."),
    ("mailbox-size", &["pid"], "How many messages are queued in pid's mailbox (its receive backlog), or nil if pid is not a live local process. The one process-introspection accessor not reachable from Brood; see std/observer.blsp."),
    ("process-info", &["pid"], "A snapshot map of a live local process: {:id :pid :node :name :status :mailbox :monitored-by :parent :memory :collections :reductions} (:pid the process's pid value, for acting on it with exit/send/monitor; :status is :running or :waiting; :name nil if unregistered; :parent the spawner's id, nil for the root; :memory the LOCAL heap bytes and :collections the cumulative GC count, both as of the process's last receive; :reductions the cumulative reduction count — Erlang's scheduling unit, updated every quantum; exact for spawned processes, coarse for the root). nil for a remote/dead pid. The Erlang-process_info-style introspection the observer reads; see std/observer.blsp."),
    ("term-enter", &[], "Enter raw mode + the alternate screen, hide the cursor, and enable mouse capture, taking over the terminal for a full-screen UI (so click/scroll reach term-poll). Pair with term-leave. (ADR-046 display seam.)"),
    ("term-leave", &[], "Restore the terminal: show the cursor, disable mouse capture, leave the alternate screen, disable raw mode. The normal-path teardown for term-enter."),
    ("term-size", &[], "The terminal size as [cols rows] in character cells."),
    ("term-poll", &["ms"], "Wait up to ms milliseconds for an input event; return a key (a 1-char string for printables, or a keyword for specials: :up :down :left :right :enter :escape :backspace :tab :back-tab :delete :home :end :page-up :page-down, ctrl combos like :ctrl-c, alt combos like :alt-f), a mouse event as a vector [:mouse action button row col] (action: :press :scroll-up :scroll-down; button: :left :right :middle or nil; row/col 0-based cells), or nil on timeout. Always pass a finite ms."),
    ("term-draw", &["frame"], "Paint a frame — a vector of render ops: [:clear], [:text row col str], [:text row col str face], [:cursor row col]. A face is a map like {:fg :red :bold true}. The in-process frontend for the display protocol; returns nil."),
    ("gui-open", &[], "Open a new native window and return its integer id (needs the runtime built with --features gui; errors otherwise). Its key/mouse input is delivered to the CALLING process's mailbox as messages — a key as a 1-char string / keyword (`:up`, `:ctrl-c`), the mouse as `[:mouse action button row col]` — so the consumer parks in `(receive)` instead of polling (ADR-058). Closing the window delivers `:escape`. Starts the GUI thread on the first call; each call is an independent window, so several observers can run at once. Pass the id to the other gui-* primitives; pair with gui-close."),
    ("gui-close", &["id"], "Close window id (the teardown for gui-open). Idempotent; an unknown id is a no-op."),
    ("gui-size", &["id"], "Window id's size as [cols rows] in character cells (tracks resize / HiDPI), same shape as term-size."),
    ("gui-draw", &["id", "frame"], "Paint a frame (the same render-op vector term-draw takes) to window id; returns nil. Unknown ops are skipped (forward-compatible)."),
    ("gui-font!", &["spec"], "Set the global default cell font from spec, a map {:family <keyword> :height <px>} (both keys optional): :family picks a registered font family (bundled :mono, or one added by gui-font-register), :height the cell pixel size. Applies to every open window and ones opened later — the whole-window knob; per-section fonts come from a face's :family/:italic. Needs --features gui. Returns nil."),
    ("gui-font-register", &["name", "styles"], "Register font family name (a keyword) from styles, a map of style → TTF file path {:regular \"…\" :bold \"…\" :italic \"…\" :bold-italic \"…\"}. Only :regular is required; a missing style reuses the regular file. Afterwards a face's :family <name> (or gui-font!) selects it. Needs --features gui. Returns name."),
    ("term-raw-enter", &[], "Enter raw mode only — NO alternate screen, cursor stays visible, scrollback preserved. The seam for an inline line editor (the REPL); use term-enter instead for a full-screen TUI. Pair with term-raw-leave."),
    ("term-raw-leave", &[], "Leave raw mode (the teardown for term-raw-enter). Idempotent with the panic-path restore."),
    ("term-emit", &["ops"], "Paint inline, relative-motion render ops (for an in-place editor that must not take over the screen): [:print str], [:print str face], [:cr], [:nl], [:up n], [:down n], [:col n], [:clear-eol], [:clear-below], [:clear-screen]. A face is a map like {:fg :cyan :bold true}. Queues all ops then flushes once; unknown ops are skipped; returns nil."),
    ("demonitor", &["mref"], "Drop the monitor identified by mref (best-effort)."),
    ("link", &["pid"], "Symmetrically link the current process and pid, local or remote (Erlang link/1). When either dies, the other gets a [:EXIT pid reason] message if it set (trap-exit true), else dies too on an abnormal reason (propagation cascades through links; :normal does not propagate). A remote link fires :noconnection on net-split; linking an already-dead/unreachable pid notifies the caller (:noproc / :noconnection). Returns nil."),
    ("unlink", &["pid"], "Drop the symmetric link between the current process and pid (local or remote; best-effort). Returns nil."),
    ("trap-exit", &["on"], "Set the current process's trap_exit flag (Erlang process_flag(trap_exit, …)); returns the previous value. When on, a linked peer's death arrives as a trappable [:EXIT pid reason] message instead of killing this process."),
    ("spawn-count", &[], "How many green processes have been spawned since program start."),
    ("peak-threads", &[], "High-water mark of OS threads running processes concurrently."),
    ("worker-threads", &[], "The size of the scheduler's worker-thread pool (about nproc)."),
    ("node-start", &["name", "addr", "cookie"], "Name this runtime and listen for peers on addr (\"host:port\"); cookie authenticates links. Returns the node name."),
    ("connect", &["spec"], "Link to a peer node named in spec (\"name@host:port\"); cookie-authenticated. Returns the peer's node name."),
    ("register", &["name", "pid"], "Bind a local name so peers can address this process via {:name name :node this-node}. Returns the pid."),
    ("whereis", &["name"], "The local pid registered under `name`, or nil. Strictly local — does not query other nodes."),
    ("node-name", &[], "This runtime's node name (:nonode until node-start)."),
    ("nodes", &[], "A list of currently connected peer node names."),
    ("monitor-node", &["name"], "Get [:nodedown name] when the link to node `name` goes down (heartbeat timeout or close)."),
];

/// The `(params, doc)` for a primitive `name`, or `(&[], "")` if undocumented.
fn primitive_doc(name: &str) -> (&'static [&'static str], &'static str) {
    PRIMITIVE_DOCS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|&(_, p, d)| (p, d))
        .unwrap_or((&[], ""))
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Nil)
}

/// Destructure exactly two args. The declared `Arity` is the *primary* arity
/// check (enforced once in `eval::call_native` before any builtin runs); this
/// re-check is defense-in-depth for a direct Rust call that bypasses the gate
/// (e.g. a unit test) — it keeps such a call a clean error instead of a panic.
fn two(args: &[Value], who: &str) -> Result<(Value, Value), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!(
            "{}: expected 2 arguments, got {}",
            who,
            args.len()
        )));
    }
    Ok((args[0], args[1]))
}

// ---------- numeric ----------

/// Require a value of a particular shape, or raise a self-identifying type
/// error attributed to `who` (the primitive that needed it). One macro behind
/// every `expect_*` helper below — the alternative was six hand-written
/// `match v { Value::X(id) => Ok(id), _ => Err(wrong_type(…, "kind", v)) }`
/// copies that drifted on the error helper used (`expect_node_name` chose
/// `type_err` over `wrong_type` and lost the offending value from its
/// message). The macro lifts that one rule into one place; the human-readable
/// `$expected` string is what the error message will say.
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

/// Require a number, coerced to `f64`; otherwise a self-identifying type error
/// attributed to `who` (the primitive that needed it).
fn expect_number(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    expect!(heap, who, v, "number",
        Value::Int(n) => n as f64,
        Value::Float(f) => f,
    )
}

/// Require a string, returned **owned** so the `heap` borrow is released before
/// the builtin reads or allocates further (most callers go on to touch
/// `&mut heap`). The string analogue of [`expect_int`]/[`expect_number`].
fn expect_string(heap: &Heap, who: &str, v: Value) -> Result<String, LispError> {
    expect!(heap, who, v, "string",
        Value::Str(id) => heap.string(id).to_string(),
    )
}

/// Require a rope, returned **owned** (a cheap `Arc`-node clone) so the `heap`
/// borrow is released before the builtin edits or allocates a fresh rope.
fn expect_rope(heap: &Heap, who: &str, v: Value) -> Result<ropey::Rope, LispError> {
    expect!(heap, who, v, "rope",
        Value::Rope(id) => heap.rope(id).clone(),
    )
}

/// Require an integer; otherwise a self-identifying type error.
fn expect_int(heap: &Heap, who: &str, v: Value) -> Result<i64, LispError> {
    expect!(heap, who, v, "int",
        Value::Int(n) => n,
    )
}

/// Require a symbol; otherwise a self-identifying type error.
fn expect_symbol(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "symbol",
        Value::Sym(s) => s,
    )
}

fn num_bin(
    heap: &Heap,
    args: &[Value],
    who: &str,
    int_op: fn(i64, i64) -> Option<i64>,
    float_op: fn(f64, f64) -> f64,
) -> LispResult {
    let (a, b) = two(args, who)?;
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => int_op(x, y).map(Value::Int).ok_or_else(|| {
            LispError::runtime(format!("{}: integer overflow", who))
                .with_code(crate::error::error_codes::INT_OVERFLOW)
        }),
        _ => Ok(Value::Float(float_op(
            expect_number(heap, who, a)?,
            expect_number(heap, who, b)?,
        ))),
    }
}

fn prim_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(heap, args, "%add", i64::checked_add, |a, b| a + b)
}
fn prim_sub(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(heap, args, "%sub", i64::checked_sub, |a, b| a - b)
}
fn prim_mul(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(heap, args, "%mul", i64::checked_mul, |a, b| a * b)
}

fn prim_div(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%div")?;
    let bf = expect_number(heap, "%div", b)?;
    if bf == 0.0 {
        return Err(LispError::runtime("division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (/ x y))"));
    }
    match (a, b) {
        // Exact integer quotient when it divides evenly; otherwise a float.
        // `checked_*` guards the one overflowing case (`i64::MIN / -1`), which
        // then falls through to the float path instead of panicking.
        (Value::Int(x), Value::Int(y)) => match (x.checked_rem(y), x.checked_div(y)) {
            (Some(0), Some(q)) => Ok(Value::Int(q)),
            _ => Ok(Value::Float(x as f64 / y as f64)),
        },
        _ => Ok(Value::Float(expect_number(heap, "%div", a)? / bf)),
    }
}

fn prim_lt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%lt")?;
    // Compare two ints directly; coercing to f64 first loses precision past 2^53
    // (e.g. `(< 9007199254740992 9007199254740993)` would wrongly be false).
    let lt = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x < y,
        _ => expect_number(heap, "%lt", a)? < expect_number(heap, "%lt", b)?,
    };
    Ok(Value::Bool(lt))
}

fn prim_eq(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%eq")?;
    Ok(Value::Bool(heap.equal(a, b)))
}

fn int_pair(heap: &Heap, args: &[Value], who: &str) -> Result<(i64, i64), LispError> {
    let (a, b) = two(args, who)?;
    Ok((expect_int(heap, who, a)?, expect_int(heap, who, b)?))
}

fn remainder(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = int_pair(heap, args, "rem")?;
    match a.checked_rem(b) {
        Some(r) => Ok(Value::Int(r)),
        None if b == 0 => Err(LispError::runtime("rem: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (rem x y))")),
        None => Err(LispError::runtime("rem: integer overflow")
            .with_code(crate::error::error_codes::INT_OVERFLOW)),
    }
}

/// Floor toward negative infinity, returning an `Int` — the one Float→Int
/// crossing the language can't bootstrap (no other primitive produces an `Int`
/// from a `Float`). An `Int` passes through; a `Float` is floored. `NaN` and
/// values whose floor doesn't fit in `i64` are runtime errors — pre-fix the
/// `as i64` cast silently saturated, so `(floor (* 1e308 1e308))` returned
/// `i64::MAX` and `(floor (/ 0.0 0.0))` returned `0`. `ceil`/`round`/`quot`/
/// `pow`/`sqrt` are all Brood over this + `rem`/`/`/`*`/`<` (std/prelude.blsp).
fn floor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::Int(n)),
        v => {
            let f = expect_number(heap, "floor", v)?.floor();
            if !f.is_finite() {
                return Err(LispError::runtime(format!(
                    "floor: argument {} has no integer floor",
                    f
                ))
                .with_code(crate::error::error_codes::INT_OVERFLOW));
            }
            // `i64::MIN as f64` rounds *down* to a value still in range; the
            // upper bound `i64::MAX as f64` rounds *up* past `i64::MAX`, so
            // the open upper comparison is the right one.
            if f < i64::MIN as f64 || f >= i64::MAX as f64 + 1.0 {
                return Err(
                    LispError::runtime(format!("floor: {} is out of range for i64", f))
                        .with_code(crate::error::error_codes::INT_OVERFLOW),
                );
            }
            Ok(Value::Int(f as i64))
        }
    }
}

// ---------- bitwise ----------

fn bit_and(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = int_pair(heap, args, "bit-and")?;
    Ok(Value::Int(a & b))
}

fn bit_or(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = int_pair(heap, args, "bit-or")?;
    Ok(Value::Int(a | b))
}

fn bit_xor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = int_pair(heap, args, "bit-xor")?;
    Ok(Value::Int(a ^ b))
}

fn bit_not(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "bit-not", arg(args, 0))?;
    Ok(Value::Int(!n))
}

/// A shift amount must be in `[0, 64)` — Rust's `<<`/`>>` panic outside the bit
/// width, so reject it as a clean runtime error rather than crash the runtime.
fn shift_amount(n: i64, who: &str) -> Result<u32, LispError> {
    if !(0..64).contains(&n) {
        return Err(LispError::runtime(format!(
            "{}: shift amount {} out of range [0, 64)",
            who, n
        )));
    }
    Ok(n as u32)
}

fn bit_shift_left(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, n) = int_pair(heap, args, "bit-shift-left")?;
    // Logical shift of the two's-complement bit pattern (bits shifted past the
    // top are discarded) — the conventional `bit-shift-left`. Wrapping, not
    // checked: callers mask back down (e.g. the xorshift PRNG) rather than
    // expecting an overflow error.
    Ok(Value::Int(
        a.wrapping_shl(shift_amount(n, "bit-shift-left")?),
    ))
}

fn bit_shift_right(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, n) = int_pair(heap, args, "bit-shift-right")?;
    // Arithmetic (sign-preserving) right shift, matching the signed i64 model.
    Ok(Value::Int(a >> shift_amount(n, "bit-shift-right")?))
}

// ---------- pair / sequence ----------

fn cons(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "cons")?;
    Ok(heap.alloc_pair(a, b))
}

fn first(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Pair(p) => Ok(heap.car(p)),
        Value::Vector(id) => Ok(heap.vector(id).first().copied().unwrap_or(Value::Nil)),
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::wrong_type(heap, "first", "list or vector", v)),
    }
}

fn rest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Pair(p) => Ok(heap.cdr(p)),
        Value::Vector(id) => {
            let items: Vec<Value> = heap.vector(id).iter().skip(1).copied().collect();
            Ok(heap.list(items))
        }
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::wrong_type(heap, "rest", "list or vector", v)),
    }
}

/// `(%sort-asc coll)` — stable ascending sort of a numeric collection by `<`.
/// The fast path behind `(sort coll)` when no custom comparator is given;
/// the all-Brood `merge-sort` in `std/prelude.blsp` still handles
/// `(sort less? coll)`. ~50× faster than the in-Brood mergesort on 10 000
/// items because every comparison is a Rust `match` instead of an
/// `eval::apply` round-trip.
///
/// Items must be `Int` / `Float` / mixed (the same shape `<` accepts).
/// Mixed Int+Float promote to float for the compare (matching `prim_lt`).
/// Any non-numeric item is a `wrong_type` error against the offending value.
fn sort_asc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Collect into a Vec. `seq_items` walks the cons spine (or copies a
    // vector) once. Values are `Copy` so the Vec holds plain handles — no
    // GC root machinery needed because `sort_by` does no eval and can't
    // trigger a safepoint.
    let mut items = heap.seq_items(arg(args, 0))?;

    // Validate before sorting so a non-numeric item produces one clear
    // error rather than an indeterminate-order partial sort.
    for &v in &items {
        match v {
            Value::Int(_) | Value::Float(_) => {}
            _ => return Err(LispError::wrong_type(heap, "sort", "number", v)),
        }
    }

    // Stable sort. The int-int branch keeps full precision; mixed pairs
    // promote to f64 (same compromise as `prim_lt`'s mixed case — past
    // 2^53 the float compare can collapse two distinct ints, but that
    // matches what `<` itself would do).
    items.sort_by(|a, b| match (*a, *b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(&y),
        _ => {
            let xf = match *a {
                Value::Int(n) => n as f64,
                Value::Float(f) => f,
                _ => unreachable!(),
            };
            let yf = match *b {
                Value::Int(n) => n as f64,
                Value::Float(f) => f,
                _ => unreachable!(),
            };
            // NaN sorts as Equal (would otherwise break `sort_by`'s total
            // ordering). Real Brood `<` doesn't admit NaN past `(nan? x)`
            // anyway, so this is the lesser evil.
            xf.partial_cmp(&yf).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    Ok(heap.list(items))
}

/// `(%sort-cmp coll)` — stable ascending sort by the structural total order
/// (`Heap::value_cmp`). The Brood `sort` (prelude) routes here when items
/// aren't all numeric, so `(sort [[1 0] [2 1]])` and similar work without a
/// custom comparator. Cross-kind items get a defined tag-rank order rather
/// than the old "expected number" trap.
fn sort_cmp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut items = heap.seq_items(arg(args, 0))?;
    // `value_cmp` reads heap data through `&Heap` only; the items are `Copy`
    // handles, so no GC root machinery is needed.
    items.sort_by(|a, b| heap.value_cmp(*a, *b));
    Ok(heap.list(items))
}

/// `(compare a b)` — the structural total order as a binary comparison: `-1` if
/// `a` sorts before `b`, `0` if equal, `1` if after. Numbers compare
/// numerically; strings/keywords/symbols by text; vectors/lists
/// lexicographically; cross-kind values by a stable tag rank. The binary form of
/// the order `sort` uses, so `sort-by` and custom comparators work over any
/// orderable value, not just numbers.
fn compare(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::cmp::Ordering;
    let ord = match heap.value_cmp(arg(args, 0), arg(args, 1)) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    Ok(Value::Int(ord))
}

// ---------- vector ----------

fn vector(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_vector(args.to_vec()))
}

fn vector_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let n = expect_int(heap, "vector-ref", arg(args, 1))?;
    match v {
        Value::Vector(id) if n >= 0 && (n as usize) < heap.vector(id).len() => {
            Ok(heap.vector(id)[n as usize])
        }
        Value::Vector(id) => Err(LispError::runtime(format!(
            "vector-ref: index {} out of range [0, {})",
            n,
            heap.vector(id).len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)),
        _ => Err(LispError::wrong_type(heap, "vector-ref", "vector", v)),
    }
}

fn vector_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Vector(id) => Ok(Value::Int(heap.vector(id).len() as i64)),
        _ => Err(LispError::wrong_type(heap, "vector-length", "vector", v)),
    }
}

// ---------- map ----------

/// Require a map; otherwise a self-identifying type error attributed to `who`.
fn expect_map(heap: &Heap, who: &str, v: Value) -> Result<value::MapId, LispError> {
    expect!(heap, who, v, "map",
        Value::Map(id) => id,
    )
}

/// `(hash-map k v k v …)` — build a map from alternating key/value args (the
/// programmatic form of the `{ }` literal). Errors on an odd count; last-wins on
/// duplicate keys.
fn hash_map(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if !args.len().is_multiple_of(2) {
        return Err(LispError::arity(
            "hash-map: expected an even number of arguments (key/value pairs)",
        ));
    }
    let pairs: Vec<(Value, Value)> = args.chunks_exact(2).map(|kv| (kv[0], kv[1])).collect();
    Ok(heap.map_from_pairs(pairs))
}

/// `(map-get m k [default])` — the value `k` maps to, or `default` (nil if
/// omitted) when absent.
fn map_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-get", arg(args, 0))?;
    Ok(heap
        .map_get(id, arg(args, 1))
        .unwrap_or_else(|| arg(args, 2)))
}

/// `(map-assoc m k v)` — a fresh map with `k` bound to `v`.
fn map_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-assoc", arg(args, 0))?;
    Ok(heap.map_assoc(id, arg(args, 1), arg(args, 2)))
}

/// `(map-dissoc m k)` — a fresh map with `k` removed.
fn map_dissoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-dissoc", arg(args, 0))?;
    Ok(heap.map_dissoc(id, arg(args, 1)))
}

/// `(map-pairs m)` — the entries as a list of `[k v]` vectors, in insertion
/// order, in one O(n) pass. The *single* map enumerator: `keys`/`vals`/
/// `contains?`/`reduce-kv` are all Brood over it (std/prelude.blsp).
fn map_pairs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-pairs", arg(args, 0))?;
    let entries = heap.map_entries(id); // copy out, releasing the borrow before we alloc
    let pairs: Vec<Value> = entries
        .into_iter()
        .map(|(k, v)| heap.alloc_vector(vec![k, v]))
        .collect();
    Ok(heap.list(pairs))
}

fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::Int(heap.string(id).chars().count() as i64)),
        _ => Err(LispError::wrong_type(heap, "string-length", "string", v)),
    }
}

// ---------- type reflection ----------

/// `(type-of x)` — the runtime type tag of `x` as a keyword: `:int` `:float`
/// `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:fn`
/// `:macro` `:native`. The single irreducible reflective primitive: the tag
/// predicates (`int?`/`string?`/…) are Brood wrappers over it (`std/prelude.blsp`),
/// and the in-language type checks build on it too.
fn type_of(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(value::kw(value::tag(arg(args, 0)).name()))
}

// ---------- value <-> text and I/O ----------

fn str_concat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut s = String::new();
    for &a in args {
        s.push_str(&printer::display(heap, a));
    }
    Ok(heap.alloc_string(&s))
}

fn pr_str(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = printer::print(heap, arg(args, 0));
    Ok(heap.alloc_string(&s))
}

/// Start capturing the current process's output into a fresh buffer. While active,
/// `print` / terminal output ([`write_term_bytes`]) appends there instead of real
/// stdout — and so does output from any process this one `spawn`s (the capture is
/// **process-scoped and inherited**, living in the process `Ctx`; see
/// `scheduler::begin_capture`). The `nest mcp` dispatcher installs one around each
/// `tools/call` so a handler's output — even a handler run in a spawned, killable
/// process under a timeout — can't corrupt the JSON-RPC stdout stream; the captured
/// text rides back in the result envelope. Pair with [`take_captured_stdout`].
pub fn begin_stdout_capture() {
    crate::process::begin_capture();
}

/// Stop capturing and return what was written since [`begin_stdout_capture`] —
/// `Some(text)` (possibly empty) if capture was active, `None` otherwise.
pub fn take_captured_stdout() -> Option<String> {
    crate::process::take_capture()
}

/// If a capture is active on the current process, append `s` to it and return
/// `true`; otherwise `false`. The single divert point shared by `print` and
/// `write_term_bytes`.
fn capture_write(s: &str) -> bool {
    crate::process::capture_append(s)
}

fn print(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    let text = parts.join(" ");
    // Divert to the capture buffer if one is active (the MCP channel must stay pure
    // JSON-RPC); otherwise write real stdout.
    let captured = capture_write(&text);
    if !captured {
        print!("{text}");
        use std::io::Write;
        std::io::stdout().flush().ok();
    }
    Ok(Value::Nil)
}

fn eprint(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    eprint!("{}", parts.join(" "));
    use std::io::Write;
    std::io::stderr().flush().ok();
    Ok(Value::Nil)
}

/// `(stdout-tty?)` — true when stdout is an interactive terminal, false when it's
/// captured (a pipe, a file, `cargo test`). The test framework uses this to emit
/// ANSI colour only when a human is watching, so captured output (what an LLM or
/// CI reads) stays clean plain text.
fn stdout_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::Bool(std::io::stdout().is_terminal()))
}

/// `(stdin-tty?)` — true when stdin is an interactive terminal, false when it's
/// redirected (a pipe, a file). The REPL gates raw-mode line editing on this:
/// `echo … | brood` has a piped stdin (even with a TTY stdout), so it must take
/// the plain `read-line` path, not the interactive editor.
fn stdin_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::Bool(std::io::stdin().is_terminal()))
}

// ---------- time ----------

/// `(now)` — wall-clock milliseconds since the Unix epoch, as an integer.
/// Subtract two readings to measure elapsed time (see `std/test.blsp`).
fn now(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::Int(ms))
}

/// `(now-ns)` — wall-clock nanoseconds since the Unix epoch, as an integer.
/// The fine-grained partner to `now`; subtract two readings to time sub-
/// millisecond work that `now`'s resolution would round to zero. (i64
/// nanoseconds since 1970 stays in range until the year 2262.)
fn now_ns(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(Value::Int(ns))
}

// ---------- memory ----------

/// `(mem-bytes)` — bytes currently allocated across the whole process.
fn mem_bytes(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::live_bytes() as i64))
}

/// `(mem-peak)` — high-water mark of allocated bytes since the process started.
fn mem_peak(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::peak_bytes() as i64))
}

/// `(gc-stats)` — a snapshot map of this process's garbage-collection activity
/// (Tier-1 observability; `docs/memory-review.md` §7). Per-process: it reports
/// the *calling* process's own LOCAL heap, never another's. Keys:
/// `:collections` (collections run since start — the automatic Stage-B
/// safepoint copies), `:copied` (cumulative LOCAL
/// objects relocated by those collections), `:reclaimed` (cumulative LOCAL
/// objects dropped), `:live` (LOCAL objects live right now), `:live-bytes` (a
/// cheap byte estimate of the LOCAL slabs — see `mem-bytes` for the process-wide
/// figure), and `:threshold` (the live count that triggers the next collection —
/// the slow/stable dial).
fn gc_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (runs, copied, reclaimed) = heap.gc_counters();
    let pairs = vec![
        (value::kw("collections"), Value::Int(runs as i64)),
        (value::kw("copied"), Value::Int(copied as i64)),
        (value::kw("reclaimed"), Value::Int(reclaimed as i64)),
        (
            value::kw("live"),
            Value::Int(heap.local_live_count() as i64),
        ),
        (
            value::kw("live-bytes"),
            Value::Int(heap.local_bytes() as i64),
        ),
        (
            value::kw("threshold"),
            Value::Int(heap.gc_threshold() as i64),
        ),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(mem-limit)` — the hard memory ceiling in bytes (0 = unlimited). ADR-043.
fn mem_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::hard_limit() as i64))
}

/// `(mem-soft-limit)` — the soft memory ceiling in bytes (0 = unlimited). ADR-043.
fn mem_soft_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::soft_limit() as i64))
}

// ---------- self-hosting ----------

fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    crate::eval::eval(heap, form, root)
}

fn read_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-string", arg(args, 0))?;
    reader::read_one(heap, &s)
}

/// `(parse-source s)` — parse s into a lossless CST tree as nested vectors, the
/// mechanism behind `std/format.blsp`. Never raises: malformed input becomes
/// `[:error "raw"]` nodes (parsing resumes after them). See `syntax::cst`.
///
/// Shape (each node is a vector `[kind …]`):
/// - Leaves carry the original source text:
///   `[:symbol "foo"]`, `[:keyword ":foo"]`, `[:int "42"]`, `[:float "1.5"]`,
///   `[:bool "true"]`, `[:nil "nil"]`, `[:str "\"hi\""]` (raw — quotes/escapes
///   included), `[:whitespace "  \n"]`, `[:comment ";; hi\n"]`, `[:error "raw"]`.
/// - Reader macros wrap a single child form:
///   `[:quote child]`, `[:quasi child]`, `[:unquote child]`, `[:splice child]`.
/// - Containers carry a child vector:
///   `[:root [child …]]`, `[:list [child …]]`, `[:vector [child …]]`,
///   `[:map [child …]]`.
///
/// Roundtrip property: concatenating every leaf's text in tree order reproduces
/// the input — this is what makes the CST a faithful basis for formatting.
fn parse_source(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "parse-source", arg(args, 0))?;
    let root = cst::parse(&s);
    Ok(cst_to_value(heap, &root, &s))
}

fn cst_to_value(heap: &mut Heap, node: &cst::Node, src: &str) -> Value {
    use cst::NodeKind::*;
    let tag = |k: &'static str| Value::Keyword(value::intern(k));
    match node.kind {
        // Leaves: [kind raw-text].
        Symbol | Keyword | Int | Float | Str | Bool | Nil | Whitespace | Comment | Error => {
            let k = match node.kind {
                Symbol => "symbol",
                Keyword => "keyword",
                Int => "int",
                Float => "float",
                Str => "str",
                Bool => "bool",
                Nil => "nil",
                Whitespace => "whitespace",
                Comment => "comment",
                Error => "error",
                _ => unreachable!(),
            };
            let text = heap.alloc_string(node.text(src));
            heap.alloc_vector(vec![tag(k), text])
        }
        // Reader-macro wrappers: [kind child]. The single structural child is
        // the wrapped form; any leading whitespace child is dropped (the wrapper
        // owns its position via its parent's children list).
        Quote | Quasi | Unquote | Splice => {
            let k = match node.kind {
                Quote => "quote",
                Quasi => "quasi",
                Unquote => "unquote",
                Splice => "splice",
                _ => unreachable!(),
            };
            // A reader-macro node's children are the wrapped form's parse
            // result(s) — usually a single form. Walk and pick the first
            // non-trivia child; nest the rest as following siblings would be a
            // parse bug, but in case of empty (EOF after ~/`/'/), emit nil.
            let child = node
                .forms()
                .next()
                .map(|c| cst_to_value(heap, c, src))
                .unwrap_or(Value::Nil);
            heap.alloc_vector(vec![tag(k), child])
        }
        // Containers: [kind [child …]]. Children include trivia (whitespace +
        // comments) so the formatter can preserve blank-line + comment intent.
        Root | List | Vector | Map => {
            let k = match node.kind {
                Root => "root",
                List => "list",
                Vector => "vector",
                Map => "map",
                _ => unreachable!(),
            };
            let kids: Vec<Value> = node
                .children
                .iter()
                .map(|c| cst_to_value(heap, c, src))
                .collect();
            let kids_vec = heap.alloc_vector(kids);
            heap.alloc_vector(vec![tag(k), kids_vec])
        }
    }
}

/// `(reload-defs path)` — like `load`, but only re-evaluates **definitions**
/// (`def`/`defmacro` and `def…`-named macros: `defn`, `defmodule`, `defdyn`,
/// `defonce`, user definers). All other top-level forms — `(require …)`,
/// `(load …)`, a `(main-loop 0)` entry call — are silently skipped. Used by the
/// file watcher (`std/reload.blsp`): on the **second** and subsequent visits to
/// a file we want to refresh the code (so the running program sees the new
/// behaviour via late binding) but **not** re-run side-effecting top-level calls
/// — re-executing those would spawn a duplicate long-running process (a
/// tail-recursive loop) or block the watcher itself.
///
/// **Atomicity:** the whole file is read before any form is evaluated, so a
/// half-saved / syntactically broken file applies *zero* defs (read fails
/// first). Forms are then expanded+evaluated one at a time, exactly like
/// `load`, so a macro a form defines is visible to later forms in the same file
/// (`lib.rs`). The residual non-atomic window is a *runtime* error while
/// evaluating form N, after 1..N-1 already landed; full snapshot/rollback is
/// deferred (docs/live-editing.md Stage 2). Returns `nil`. ADR-013 hot reload's
/// mechanism flowing through to the tool layer.
fn reload_defs(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "reload-defs", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("reload-defs: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let root = heap.env_root(env);
    let prev = heap.set_current_file(Some(path.clone()));
    // Namespace bracketing + forward-ref pre-scan, like `load` (ADR-065): a
    // reloaded namespaced file re-establishes its own namespace (its `(ns …)` form
    // is re-evaluated below) so its re-saved defs are qualified correctly.
    let prev_ns = heap.set_compile_ns(None);
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let known = if crate::eval::macros::file_opens_ns(heap, &form_vals) {
        crate::eval::macros::scan_def_names(heap, &form_vals)
    } else {
        std::collections::HashSet::new()
    };
    let prev_known = heap.set_ns_known_names(known);
    let prev_imports = heap.set_imports(std::collections::HashMap::new());
    let mut result = Ok(Value::Nil);
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds; re-fetch
    // each from the (relocated) root stack rather than the stale `forms` Vec. Same
    // discipline as `load`.
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        // Re-eval only *definitions*; skip side-effecting top-level forms
        // (`(require …)`, `(load …)`, a `(main-loop 0)` entry call). A form is a
        // definition when its head symbol starts with "def" **and** is actually a
        // definer — one of the `def`/`defmacro` core special forms, or a symbol
        // currently bound to a macro (`defn`/`defmodule`/`defdyn`/`defonce` and
        // any user `def…` macro). The macro check drops the false positive on a
        // plain top-level *call* to a function whose name merely starts with
        // "def" (e.g. `(default-config)`): that head resolves to a `Fn`, not a
        // macro, so it's correctly skipped.
        //
        // Known limitation (accepted — docs/live-editing.md Stage 2): a definer
        // macro *not* named `def…` (e.g. `(register-handler …)` expanding to a
        // `def`) is skipped. Workaround: prefix definer macros with `def`, the
        // Lisp convention anyway. (`require` skipping is likewise intentional: we
        // don't transitively reload other modules; the user watches each path
        // explicitly with `reload-on-change`.)
        let head_is_def = match form {
            Value::Pair(p) => {
                let (head, _) = heap.pair(p);
                match head {
                    Value::Sym(s) => {
                        let nm = value::symbol_name(s);
                        // The `(defmodule …)` header is re-evaluated too (so the
                        // reloaded file's namespace + imports are re-established for
                        // its defs, ADR-065) — it's a `def…`-named macro, caught here.
                        nm.starts_with("def")
                            && (nm == "def"
                                || nm == "defmacro"
                                || matches!(heap.env_get(root, s), Some(Value::Macro(_))))
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !head_is_def {
            continue;
        }
        // Same def-site recording / expand / eval shape as `load` for the
        // forms we *do* evaluate, so cross-file goto still lands at the
        // re-saved def site.
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_imports(prev_imports);
    result.map(|_| Value::Nil)
}

fn load(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "load", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("load: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    // Read positioned so errors point at a line; tag every error with the file
    // (`FILE:LINE:COL:`, see docs/tooling.md).
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let root = heap.env_root(env);
    // Expose the file to Brood (`(current-file)`) for the duration of the load,
    // so the test macros can record each test's source location; restore the
    // previous file afterward since loads nest.
    let prev = heap.set_current_file(Some(path.clone()));
    // A loaded file starts at the root namespace and its own `(ns …)` form sets the
    // current namespace for the rest of the file (ADR-065); restore the caller's
    // namespace afterward so loads nest and ns state never leaks out of a file.
    let prev_ns = heap.set_compile_ns(None);
    // Forward-reference pre-scan (ADR-065): if the file opens a namespace, record
    // the bare names it will define so a reference to a later definition resolves.
    // Cheap (read-only, no GC), gated on the file actually using `(ns …)`.
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let known = if crate::eval::macros::file_opens_ns(heap, &form_vals) {
        crate::eval::macros::scan_def_names(heap, &form_vals)
    } else {
        std::collections::HashSet::new()
    };
    let prev_known = heap.set_ns_known_names(known);
    let prev_imports = heap.set_imports(std::collections::HashMap::new());

    // **Bounded loading — the core memory guarantee (docs/memory-review.md).**
    // The collector now reclaims at ANY eval depth (ADR-061), so a file loaded
    // here is bounded no matter how deep `(load …)` sits — no `GcBlockReset`
    // depth-1 trick is needed any more. We still root the unevaluated forms across
    // the per-form eval: a collection during form `i` relocates the LOCAL forms
    // `i+1..` this loop still holds, so we re-fetch each from the (relocated) root
    // stack via `root_at` rather than the stale `forms` Vec. (Living in `load`,
    // the core, means every entry path — `brood`, `nest`, MCP `eval`, the future
    // editor — inherits the bound for free.)
    let mut result = Ok(Value::Nil);
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_imports(prev_imports);
    result
}

/// `(eval-string "src")` — read and evaluate every form in a string against the
/// global environment. Inherits the current namespace (ADR-065): the REPL evaluates
/// each entry through here, so a `(ns foo)` typed at the REPL sticks to later
/// entries. To load a *module* source at the root namespace, use `%load-string`.
fn eval_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "eval-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, false)
}

/// `(%load-string "src")` — the string analogue of `load`: read+eval every form,
/// but bracket the current namespace (reset to root, restore the caller's after),
/// so an embedded module's own `(ns …)` governs it and ns state doesn't leak to the
/// caller. Used by `require-one` for baked-in std modules (ADR-065).
fn load_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%load-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, true)
}

/// Shared body of `eval-string` / `%load-string`. When `reset_ns`, the current
/// namespace is reset to root for the duration and the caller's restored after.
fn eval_string_inner(heap: &mut Heap, env: EnvId, src: &str, reset_ns: bool) -> LispResult {
    let root = heap.env_root(env);
    let forms = reader::read_all(heap, src)?;
    // When loading a module (`reset_ns`), bracket the namespace at root and
    // pre-scan its def heads for forward references; the plain `eval-string` (REPL,
    // inline) inherits the current namespace and does neither (ADR-065).
    let (prev_ns, prev_known, prev_imports) = if reset_ns {
        let pn = heap.set_compile_ns(None);
        let known = if crate::eval::macros::file_opens_ns(heap, &forms) {
            crate::eval::macros::scan_def_names(heap, &forms)
        } else {
            std::collections::HashSet::new()
        };
        let pk = heap.set_ns_known_names(known);
        let pi = heap.set_imports(std::collections::HashMap::new());
        (Some(pn), Some(pk), Some(pi))
    } else {
        (None, None, None)
    };
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds.
    let base = heap.roots_len();
    for &form in &forms {
        heap.push_root(form);
    }
    let mut result: LispResult = Ok(Value::Nil);
    for i in 0..forms.len() {
        let form = heap.root_at(base + i);
        match crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
        {
            Ok(v) => result = Ok(v),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    heap.truncate_roots(base);
    if let Some(pn) = prev_ns {
        heap.set_compile_ns(pn);
    }
    if let Some(pk) = prev_known {
        heap.set_ns_known_names(pk);
    }
    if let Some(pi) = prev_imports {
        heap.set_imports(pi);
    }
    result
}

/// Standard-library modules baked into the binary (like the prelude), so they load
/// from any directory with no file paths. The require / provide / load-path
/// *policy* is written in Brood (`std/prelude.blsp`, ADR-019); Rust only exposes
/// an embedded module's source here, via `%builtin-module` (ADR-006/008).
const EMBEDDED_MODULES: &[(&str, &str)] = &[
    ("test", include_str!("../../../std/test.blsp")),
    ("project", include_str!("../../../std/project.blsp")),
    // The package manager (ADR-037): resolves the manifest's :dependencies into a
    // lock file + load-path entries. Required lazily by `project-setup` only when a
    // project actually declares deps. Opt-in, never in the prelude.
    ("package", include_str!("../../../std/package.blsp")),
    // TCP sockets (ADR-062): active-socket helpers + a spawn-per-connection
    // server over the non-blocking tcp-* primitives. Opt-in, never in the prelude.
    ("tcp", include_str!("../../../std/tcp.blsp")),
    // The file & filesystem library: whole-file/line I/O, directory walking, path
    // helpers — Brood over the fs primitives. Opt-in, never in the prelude.
    ("file", include_str!("../../../std/file.blsp")),
    // A minimal HTTP/1.0 server (ADR-062) over the tcp + file libraries — request
    // parsing, response rendering, a router, static files. Opt-in.
    ("http", include_str!("../../../std/http.blsp")),
    ("docs", include_str!("../../../std/docs.blsp")),
    // JSON ↔ Brood data, written entirely in Brood (a recursive-descent parser +
    // encoder over the string primitives; the reader's `\u{}` escape is the
    // codepoint→char mechanism). Opt-in, never in the prelude.
    ("json", include_str!("../../../std/json.blsp")),
    // A Server-Sent Events (text/event-stream) client: a reader process that streams
    // events to a subscriber's mailbox (pairs with ui's `with-events`). Pure frame
    // parsing + a thin IO loop over tcp; reuses http's URL/header helpers. Opt-in.
    ("sse", include_str!("../../../std/sse.blsp")),
    ("hatch", include_str!("../../../std/hatch.blsp")),
    ("supervisor", include_str!("../../../std/supervisor.blsp")),
    // The editor framework's buffer model (M2 Phase 1, ADR-045): an immutable
    // buffer over the rope primitives, opt-in, never in the prelude.
    ("buffer", include_str!("../../../std/buffer.blsp")),
    // The display/input seam (M3, ADR-046): `display` is the render-op protocol
    // (pure data constructors); `keymap` is the rebindable key→command dispatcher
    // shared by the line editor and the observer; `observer` is a process-viewer
    // built on them + the `term-*`/`gui-*` primitives. All opt-in, never in the prelude.
    // The shared named-face / theme registry (the counterpart to `keymap`): style
    // named once, referenced everywhere, restyled in one place. Required by `ui`
    // (so every ui-run app gets it) and the observer.
    ("face", include_str!("../../../std/face.blsp")),
    ("display", include_str!("../../../std/display.blsp")),
    ("keymap", include_str!("../../../std/keymap.blsp")),
    ("ui", include_str!("../../../std/ui.blsp")),
    ("observer", include_str!("../../../std/observer.blsp")),
    // Bare ANSI escape *strings* for simple terminal scripts (`print` them
    // directly) — the lightweight counterpart to the `display` render-op
    // protocol. Opt-in, never in the prelude.
    ("ansi", include_str!("../../../std/ansi.blsp")),
    // Sets as a library over maps (ADR-062): a set is a map of `element → true`,
    // so membership/elements/size reuse `contains?`/`keys`/`count`; the module
    // adds `set`/`conj`/`disj`/`union`/`intersection`/`difference`/`subset?`.
    // Opt-in, never in the prelude (no `#{…}` literal / distinct type yet).
    ("set", include_str!("../../../std/set.blsp")),
    // The interactive REPL line editor (ADR-052): `highlight` is the pure lexical
    // syntax-highlighter / bracket-matcher / signature + completion scanners;
    // `lineedit` is the raw-mode, emacs-style editor built on it + the inline
    // `term-*` seam. Both opt-in, never in the prelude; `repl` requires them.
    ("highlight", include_str!("../../../std/highlight.blsp")),
    ("lineedit", include_str!("../../../std/lineedit.blsp")),
    ("format", include_str!("../../../std/format.blsp")),
    ("reload", include_str!("../../../std/reload.blsp")),
    // The Model Context Protocol tool surface — `(mcp-tools)` returns the
    // catalogue the `nest mcp` dispatcher reads (ADR-036, docs/mcp.md, step 3).
    ("mcp", include_str!("../../../std/mcp.blsp")),
    // The read-eval-print loop itself, written in Brood (`(require 'repl)`):
    // policy over the `read-line`/`eval-string`/`pr-str` primitives. The Rust
    // binaries (`brood`, `nest repl`) just bootstrap into `(repl-run)`.
    ("repl", include_str!("../../../std/repl.blsp")),
];

/// Baked-in reference *documents* (markdown), the counterpart to
/// [`EMBEDDED_MODULES`] for non-module text. `(%builtin-doc 'brood-for-claude)`
/// returns the language guide that `nest new` scaffolds into each new project,
/// so a freshly-scaffolded project is self-contained without depending on a
/// Brood install path.
const EMBEDDED_DOCS: &[(&str, &str)] = &[
    (
        "brood-for-claude",
        include_str!("../../../docs/brood-for-claude.md"),
    ),
    // The Claude Code skill that `nest new` drops into each project's
    // `.claude/skills/`, so an AI assistant editing the project auto-loads the
    // Brood-writing rules. The full reference is `brood-for-claude`; this is the
    // short triggerable checklist (`SKILL.md` frontmatter + the LLM traps).
    // Canonical source lives here in `docs/` (a tracked path); the repo's own
    // `.claude/skills/writing-brood/SKILL.md` is a local symlink to it — `.claude/`
    // is gitignored, and a compile-time `include_str!` must not depend on an
    // untracked path (it would break a fresh clone's build).
    (
        "writing-brood-skill",
        include_str!("../../../docs/writing-brood-skill.md"),
    ),
];

/// The lookup body shared by `%builtin-module` and `%builtin-doc`: coerce the
/// (symbol | keyword | string) name argument, find it in `table`, return the
/// baked-in source as a fresh string (or `nil` if absent). `who`/`label` are
/// used only in the type-error message.
fn lookup_embedded(
    args: &[Value],
    heap: &mut Heap,
    table: &[(&str, &str)],
    who: &'static str,
    label: &'static str,
) -> LispResult {
    let v = arg(args, 0);
    let name = match v {
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, who, label, v)),
    };
    match table.iter().find(|(n, _)| *n == name) {
        Some((_, src)) => Ok(heap.alloc_string(src)),
        None => Ok(Value::Nil),
    }
}

/// `(%builtin-module name)` — the source of a baked-in std module as a string,
/// or nil if there is none. Mechanism only: `require` (Brood) consults this
/// before searching the load-path.
fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    lookup_embedded(
        args,
        heap,
        EMBEDDED_MODULES,
        "%builtin-module",
        "module name",
    )
}

/// `(%builtin-doc name)` — the source of a baked-in reference document as a
/// string, or nil if there is none. Used by `nest new` to scaffold the language
/// guide into each new project.
fn builtin_doc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    lookup_embedded(args, heap, EMBEDDED_DOCS, "%builtin-doc", "doc name")
}

/// `(name x)` — the spelling of a symbol or keyword as a string (no leading `:`),
/// or the string unchanged. The module system uses it to turn a module name into
/// a filename.
fn name_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(s) | Value::Keyword(s) => Ok(heap.alloc_string(&value::symbol_name(s))),
        Value::Str(_) => Ok(v),
        _ => Err(LispError::wrong_type(
            heap,
            "name",
            "symbol, keyword, or string",
            v,
        )),
    }
}

/// `(symbol x)` — the symbol whose spelling is `x`. Accepts a string (intern as
/// a fresh-or-existing symbol), a symbol (identity), or a keyword (same spelling,
/// retagged as a symbol). The lenient inverse of `name`; pairs with `keyword`.
fn to_symbol(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(_) => Ok(v),
        Value::Keyword(s) => Ok(Value::Sym(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::Sym(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "symbol",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(keyword x)` — the keyword whose spelling is `x`. Accepts a string (intern),
/// a keyword (identity), or a symbol (same spelling, retagged as a keyword).
/// Mirrors `symbol`; the two share an interner so a keyword and a symbol with the
/// same spelling carry equal `Symbol` ids (the tag is the only distinction).
fn to_keyword(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Keyword(_) => Ok(v),
        Value::Sym(s) => Ok(Value::Keyword(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::Keyword(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "keyword",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(substring s start end)` — the characters of `s` in `[start, end)`,
/// char-indexed (consistent with `string-length`). Errors if out of range.
fn substring(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "substring", arg(args, 0))?;
    let start = expect_int(heap, "substring", arg(args, 1))?;
    let end = expect_int(heap, "substring", arg(args, 2))?;
    let len = s.chars().count() as i64;
    if start < 0 || end < start || end > len {
        return Err(LispError::runtime(format!(
            "substring: range [{}, {}) out of bounds for length {}",
            start, end, len
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let sub: String = s
        .chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect();
    Ok(heap.alloc_string(&sub))
}

/// `(to-fixed x n)` — x rendered with exactly `n` digits after the decimal point
/// (rounded). The one float→text op the language can't bootstrap: `str`/`pr-str`
/// print the shortest round-tripping form (full f64 precision, e.g.
/// `0.015873015873015872`), which is wrong for tabular/console output. An int `x`
/// is promoted, so `(to-fixed 3 2)` is `"3.00"`. `n` must be non-negative.
fn to_fixed(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = expect_number(heap, "to-fixed", arg(args, 0))?;
    let n = expect_int(heap, "to-fixed", arg(args, 1))?;
    if n < 0 {
        return Err(LispError::runtime(format!(
            "to-fixed: decimal places must be non-negative, got {}",
            n
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let s = format!("{:.*}", n as usize, x);
    Ok(heap.alloc_string(&s))
}

/// `(upper s)` — `s` with every character upper-cased. Case folding is
/// Unicode-aware (e.g. `ß` → `SS`), so it leans on the standard library's tables
/// rather than being expressible in Brood.
fn upper(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "upper", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_uppercase()))
}

/// `(lower s)` — `s` with every character lower-cased (Unicode-aware, like `upper`).
fn lower(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "lower", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_lowercase()))
}

// ---------- rope (editor buffer text — ADR-045) ----------
//
// All indices are **character** indices (matching the language's char-based
// string indexing), not bytes. Edits return a *fresh* rope (immutability):
// ropey clones share structure, so `clone()`-then-edit only copies touched
// B-tree nodes. Out-of-range indices raise a clean E-code error rather than
// letting ropey panic.

/// Raise a uniform out-of-range error attributed to `who`.
fn rope_oob(who: &str, what: &str, got: i64, max: usize) -> LispError {
    LispError::runtime(format!(
        "{}: {} {} out of bounds (valid 0..={})",
        who, what, got, max
    ))
    .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)
}

/// `(string->rope s)` — a rope holding the text of string `s`.
fn string_to_rope(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->rope", arg(args, 0))?;
    Ok(heap.alloc_rope(ropey::Rope::from_str(&s)))
}

/// `(rope->string r)` — the full text of rope `r` as a string.
fn rope_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope->string", arg(args, 0))?;
    Ok(heap.alloc_string(&r.to_string()))
}

/// `(rope-length r)` — the number of characters in `r`.
fn rope_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-length", arg(args, 0))?;
    Ok(Value::Int(r.len_chars() as i64))
}

/// `(rope-line-count r)` — the number of lines in `r` (ropey counts a trailing
/// newline as ending a line, so `"a\n"` is 2 lines and `""` is 1).
fn rope_line_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line-count", arg(args, 0))?;
    Ok(Value::Int(r.len_lines() as i64))
}

/// `(rope-insert r idx s)` — a fresh rope with string `s` inserted at character
/// index `idx` (0..=length).
fn rope_insert(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "rope-insert", arg(args, 0))?;
    let idx = expect_int(heap, "rope-insert", arg(args, 1))?;
    let s = expect_string(heap, "rope-insert", arg(args, 2))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("rope-insert", "index", idx, len));
    }
    r.insert(idx as usize, &s);
    Ok(heap.alloc_rope(r))
}

/// `(rope-delete r start end)` — a fresh rope with characters `[start, end)`
/// removed.
fn rope_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "rope-delete", arg(args, 0))?;
    let start = expect_int(heap, "rope-delete", arg(args, 1))?;
    let end = expect_int(heap, "rope-delete", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("rope-delete", "range end", end, len));
    }
    r.remove(start as usize..end as usize);
    Ok(heap.alloc_rope(r))
}

/// `(rope-slice r start end)` — the text of characters `[start, end)` as a string.
fn rope_slice(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-slice", arg(args, 0))?;
    let start = expect_int(heap, "rope-slice", arg(args, 1))?;
    let end = expect_int(heap, "rope-slice", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("rope-slice", "range end", end, len));
    }
    let s = r.slice(start as usize..end as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-line r n)` — the text of line `n` (0-based), including its trailing
/// newline if present. The viewport-rendering primitive.
fn rope_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line", arg(args, 0))?;
    let n = expect_int(heap, "rope-line", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize >= lines {
        return Err(rope_oob("rope-line", "line", n, lines.saturating_sub(1)));
    }
    let s = r.line(n as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-char->line r idx)` — the 0-based line index containing character `idx`.
fn rope_char_to_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-char->line", arg(args, 0))?;
    let idx = expect_int(heap, "rope-char->line", arg(args, 1))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("rope-char->line", "index", idx, len));
    }
    Ok(Value::Int(r.char_to_line(idx as usize) as i64))
}

/// `(rope-line->char r n)` — the character index where line `n` (0-based) begins.
fn rope_line_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line->char", arg(args, 0))?;
    let n = expect_int(heap, "rope-line->char", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize > lines {
        return Err(rope_oob("rope-line->char", "line", n, lines));
    }
    Ok(Value::Int(r.line_to_char(n as usize) as i64))
}

// ---------- TCP sockets (ADR-062) ----------
//
// Thin non-blocking mechanism over `crate::net`; the active-socket / framing /
// HTTP policy is Brood (std/tcp.blsp). A socket is `Value::Socket(id)`.

fn expect_socket(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "socket",
        Value::Socket(id) => id,
    )
}

fn socket_port(who: &str, p: i64) -> Result<u16, LispError> {
    u16::try_from(p).map_err(|_| LispError::runtime(format!("{}: port {} out of range 0..=65535", who, p)))
}

fn tcp_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tcp-connect", arg(args, 0))?;
    let port = socket_port("tcp-connect", expect_int(heap, "tcp-connect", arg(args, 1))?)?;
    let owner = crate::process::self_pid();
    match crate::net::connect(&host, port, owner) {
        Ok(id) => Ok(Value::Socket(id)),
        Err(e) => Err(LispError::runtime(format!("tcp-connect {}:{}: {}", host, port, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

fn tcp_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tcp-listen", arg(args, 0))?;
    let port = socket_port("tcp-listen", expect_int(heap, "tcp-listen", arg(args, 1))?)?;
    let owner = crate::process::self_pid();
    match crate::net::listen(&host, port, owner) {
        Ok(id) => Ok(Value::Socket(id)),
        Err(e) => Err(LispError::runtime(format!("tcp-listen {}:{}: {}", host, port, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

fn tls_request(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tls-request", arg(args, 0))?;
    let port = socket_port("tls-request", expect_int(heap, "tls-request", arg(args, 1))?)?;
    let request = expect_string(heap, "tls-request", arg(args, 2))?;
    let owner = crate::process::self_pid();
    let id = crate::net::tls_request(&host, port, request.to_string(), owner);
    Ok(Value::Socket(id))
}

fn tcp_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-send", arg(args, 0))?;
    let data = expect_string(heap, "tcp-send", arg(args, 1))?;
    crate::net::send(id, data.as_bytes())
        .map_err(|e| LispError::runtime(format!("tcp-send: {}", e)))?;
    Ok(Value::Nil)
}

fn tcp_controlling_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-controlling-process", arg(args, 0))?;
    let pid = match arg(args, 1) {
        Value::Pid { id, .. } => id,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "tcp-controlling-process",
                "pid",
                other,
            ))
        }
    };
    crate::net::controlling_process(id, pid)
        .map_err(|e| LispError::runtime(format!("tcp-controlling-process: {}", e)))?;
    Ok(Value::Nil)
}

fn tcp_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-close", arg(args, 0))?;
    crate::net::close(id);
    Ok(Value::Nil)
}

fn tcp_local_port(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-local-port", arg(args, 0))?;
    Ok(crate::net::local_port(id)
        .map(|p| Value::Int(p as i64))
        .unwrap_or(Value::Nil))
}

// ----- terminal frontend (ADR-046) -------------------------------------------
//
// The thin crossterm seam: enter/leave the alternate screen, read keys, and
// paint a *frame* — a Brood vector of render ops. The protocol's meaning is
// data (the ops); these primitives are the in-process frontend that interprets
// it, so a remote/web frontend can implement the identical op vocabulary later.
// Errors surface as clean `LispError`s (never a crossterm panic), mirroring the
// rope primitives' discipline.

/// Map a crossterm I/O error into a runtime `LispError`.
fn term_err(e: std::io::Error) -> LispError {
    LispError::runtime(format!("terminal: {}", e))
}

/// `(term-enter)` — take over the terminal: raw mode + alternate screen, cursor
/// hidden. Pair with `term-leave`. The Rust-side `nest observe` guard also
/// restores the terminal if the program panics, so a crash never wrecks it.
fn term_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Hide;
    use crossterm::event::EnableMouseCapture;
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
    enable_raw_mode().map_err(term_err)?;
    // Mouse capture rides with the full-screen path only (not the inline REPL
    // seam), so click/scroll reach `term-poll`. It costs terminal text-selection
    // while active — standard for a TUI, and only for the duration of the UI.
    crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        Hide
    )
    .map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-leave)` — restore the terminal (show cursor, leave alternate screen,
/// disable raw mode). The normal-path teardown for `term-enter`.
fn term_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    crossterm::execute!(
        std::io::stdout(),
        Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .map_err(term_err)?;
    disable_raw_mode().map_err(term_err)?;
    Ok(Value::Nil)
}

/// Best-effort terminal restore — the abnormal-path backstop a host binary holds
/// in an RAII guard so a panic or error during a full-screen UI (`nest observe`)
/// never leaves the terminal in raw mode / the alternate screen. Idempotent and
/// errors are swallowed (the normal path is the Brood `term-leave`).
pub fn restore_terminal() {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    let _ = crossterm::execute!(
        std::io::stdout(),
        Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// Lighter restore for the *inline* seam (the REPL line editor, `term-raw-enter`):
/// only leave raw mode. Unlike `restore_terminal` it writes no escape sequences —
/// `disable_raw_mode` is a termios ioctl — so a host binary can hold this in an
/// RAII guard around the REPL without polluting a piped (non-TTY) stdout on exit.
/// Idempotent; errors are swallowed (the normal path is the Brood `term-raw-leave`).
pub fn restore_raw() {
    let _ = crossterm::terminal::disable_raw_mode();
}

/// `(term-size)` — the terminal size as `[cols rows]` (character cells).
fn term_size(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (cols, rows) = crossterm::terminal::size().map_err(term_err)?;
    Ok(heap.alloc_vector(vec![Value::Int(cols as i64), Value::Int(rows as i64)]))
}

/// `(term-poll ms)` — wait up to `ms` ms for a key; return it (a 1-char string,
/// or a keyword for specials) or `nil` on timeout. Always called with a finite
/// `ms`: the observer is the root process, so blocking here blocks only the root
/// thread (never a scheduler worker), but an *infinite* poll on a green process
/// would pin a worker (native blocking can't be preempted) — hence finite.
fn term_poll(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::event::{poll, read, Event, KeyEventKind};
    let ms = expect_int(heap, "term-poll", arg(args, 0))?.max(0) as u64;
    if poll(std::time::Duration::from_millis(ms)).map_err(term_err)? {
        match read().map_err(term_err)? {
            // Ignore key *release* events (reported on some platforms with the
            // enhanced-keyboard protocol) so a keypress isn't seen twice.
            Event::Key(k) if k.kind != KeyEventKind::Release => Ok(key_to_value(heap, k)),
            Event::Mouse(m) => Ok(mouse_to_value(heap, m)),
            _ => Ok(Value::Nil),
        }
    } else {
        Ok(Value::Nil)
    }
}

/// Encode a Brood mouse event as the vector `[:mouse action button row col]` —
/// the shared shape both frontends yield, so the observer (and any future UI)
/// reads one form. `action` is a keyword, `button` a keyword or nil, `row`/`col`
/// 0-based cell coordinates.
fn mouse_value(heap: &mut Heap, action: &str, button: Option<&str>, row: u16, col: u16) -> Value {
    let btn = button.map(value::kw).unwrap_or(Value::Nil);
    heap.alloc_vector(vec![
        value::kw("mouse"),
        value::kw(action),
        btn,
        Value::Int(row as i64),
        Value::Int(col as i64),
    ])
}

/// Translate a crossterm mouse event into the shared `[:mouse …]` vector. Only
/// the minimal vocabulary `gui::MouseAction` also produces — a click and the wheel
/// — is surfaced; release / drag / motion / horizontal-scroll yield nil (a no-op
/// poll), so both frontends emit exactly the same set.
fn mouse_to_value(heap: &mut Heap, m: crossterm::event::MouseEvent) -> Value {
    use crossterm::event::{MouseButton as CB, MouseEventKind as MK};
    let button = |b: CB| match b {
        CB::Left => "left",
        CB::Right => "right",
        CB::Middle => "middle",
    };
    let (action, btn) = match m.kind {
        MK::Down(b) => ("press", Some(button(b))),
        MK::ScrollUp => ("scroll-up", None),
        MK::ScrollDown => ("scroll-down", None),
        _ => return Value::Nil,
    };
    mouse_value(heap, action, btn, m.row, m.column)
}

/// Encode a crossterm key event as a Brood value: a printable char becomes a
/// 1-char string; a control combo and the named special keys become keywords.
fn key_to_value(heap: &mut Heap, k: crossterm::event::KeyEvent) -> Value {
    use crossterm::event::{KeyCode, KeyModifiers};
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    match k.code {
        KeyCode::Char(c) if ctrl => {
            Value::Keyword(value::intern(&format!("ctrl-{}", c.to_ascii_lowercase())))
        }
        // Alt/Meta combos (M-f, M-b, … — emacs word motion). Some terminals send
        // these as an Esc prefix; crossterm normalises them to the ALT modifier.
        KeyCode::Char(c) if alt => {
            Value::Keyword(value::intern(&format!("alt-{}", c.to_ascii_lowercase())))
        }
        KeyCode::Char(c) => heap.alloc_string(&c.to_string()),
        KeyCode::Up => value::kw("up"),
        KeyCode::Down => value::kw("down"),
        KeyCode::Left => value::kw("left"),
        KeyCode::Right => value::kw("right"),
        KeyCode::Enter => value::kw("enter"),
        KeyCode::Esc => value::kw("escape"),
        KeyCode::Backspace => value::kw("backspace"),
        KeyCode::Tab => value::kw("tab"),
        KeyCode::BackTab => value::kw("back-tab"),
        KeyCode::Delete => value::kw("delete"),
        KeyCode::Home => value::kw("home"),
        KeyCode::End => value::kw("end"),
        KeyCode::PageUp => value::kw("page-up"),
        KeyCode::PageDown => value::kw("page-down"),
        _ => Value::Nil,
    }
}

/// `(term-draw frame)` — paint a frame: a vector of op vectors `[:clear]`,
/// `[:text row col str]`, `[:text row col str face]`, `[:cursor row col]`.
/// Unknown ops are skipped (forward-compatible protocol). Queues all ops then
/// flushes once, so a frame paints without intermediate tearing.
/// Write rendered terminal bytes (escape sequences) to stdout — unless an MCP
/// stdout-capture is active on this thread, in which case divert them into the
/// capture buffer instead. During a `nest mcp` `tools/call`, stdout *is* the
/// JSON-RPC channel, so a `term-draw` / `term-emit` writing raw escapes there would
/// corrupt the protocol and wedge the client (the `print` capture only catches
/// Brood `print`, not these direct crossterm writes). Diverting keeps the channel
/// pure and rides the rendered bytes back in the result envelope, so an agent can
/// still inspect what a frame produced. Mirrors `print`'s capture check.
fn write_term_bytes(bytes: &[u8]) -> std::io::Result<()> {
    if !capture_write(&String::from_utf8_lossy(bytes)) {
        use std::io::Write;
        let mut real = std::io::stdout();
        real.write_all(bytes)?;
        real.flush()?;
    }
    Ok(())
}

fn term_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::cursor::MoveTo;
    use crossterm::style::{Attribute, Print, ResetColor, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};

    let ops: Vec<Value> = match arg(args, 0) {
        Value::Vector(id) => heap.vector(id).to_vec(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "term-draw",
                "vector (a frame)",
                other,
            ))
        }
    };
    let clear_t = value::intern("clear");
    let text_t = value::intern("text");
    let cursor_t = value::intern("cursor");
    let mut out: Vec<u8> = Vec::new();
    for op in ops {
        let parts: Vec<Value> = match op {
            Value::Vector(id) => heap.vector(id).to_vec(),
            _ => continue,
        };
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => continue,
        };
        if tag == clear_t {
            crossterm::queue!(out, Clear(ClearType::All)).map_err(term_err)?;
        } else if tag == cursor_t {
            let row = expect_int(heap, "term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "term-draw", arg(&parts, 2))?;
            crossterm::queue!(out, MoveTo(clamp_u16(col), clamp_u16(row))).map_err(term_err)?;
        } else if tag == text_t {
            let row = expect_int(heap, "term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "term-draw", arg(&parts, 2))?;
            let s = expect_string(heap, "term-draw", arg(&parts, 3))?;
            crossterm::queue!(out, MoveTo(clamp_u16(col), clamp_u16(row))).map_err(term_err)?;
            apply_face(&mut out, heap, parts.get(4).copied().unwrap_or(Value::Nil))?;
            crossterm::queue!(out, Print(s), SetAttribute(Attribute::Reset), ResetColor)
                .map_err(term_err)?;
        }
    }
    write_term_bytes(&out).map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-raw-enter)` — raw mode only: no alternate screen, the cursor stays
/// visible, scrollback is preserved. The seam for an *inline* line editor (the
/// self-hosted REPL, std/lineedit.blsp), as opposed to `term-enter` which takes
/// over the whole screen for a full-screen TUI. Pair with `term-raw-leave`.
///
/// Defensively *shows the cursor and disables mouse capture* on entry: terminal
/// state persists across processes, so a prior full-screen app (`term-enter` hides
/// the cursor + captures the mouse) that exited without restoring — a crash, a
/// hard `Ctrl-C`, a killed observer — would otherwise leave the inline editor with
/// no cursor and mouse-movement escape sequences injected as input. The inline
/// editor only runs on a TTY (the REPL gates `lineedit-read` on stdin+stdout being
/// terminals; the piped path uses `read-line`), so these escapes never reach a
/// redirected stream. Idempotent: showing a visible cursor / disabling inactive
/// capture are no-ops.
fn term_raw_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    crossterm::terminal::enable_raw_mode().map_err(term_err)?;
    crossterm::execute!(std::io::stdout(), Show, DisableMouseCapture).map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-raw-leave)` — leave raw mode (the teardown for `term-raw-enter`).
/// Idempotent with the panic-path `restore_terminal`.
fn term_raw_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    crossterm::terminal::disable_raw_mode().map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-emit ops)` — inline, relative-motion rendering for an in-place editor
/// that must not take over the screen (unlike `term-draw`, which paints absolute
/// cells on the alternate screen). Interprets a vector of op vectors, queued then
/// flushed once so a repaint doesn't tear:
///   `[:print str]` / `[:print str face]`  print at the cursor (face via apply_face)
///   `[:cr]`                                carriage return to column 0
///   `[:nl]`                                newline (`"\r\n"`)
///   `[:up n]` / `[:down n]`                move the cursor n rows
///   `[:col n]`                             move to absolute column n (0-based)
///   `[:clear-eol]`                         clear from the cursor to end of line
///   `[:clear-below]`                       clear from the cursor to end of screen
///   `[:clear-screen]`                      clear the whole screen, cursor to (0,0)
/// Unknown ops are skipped (forward-compatible, like `term-draw`).
fn term_emit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::cursor::{MoveDown, MoveToColumn, MoveUp};
    use crossterm::style::{Attribute, Print, ResetColor, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};

    let ops: Vec<Value> = match arg(args, 0) {
        Value::Vector(id) => heap.vector(id).to_vec(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "term-emit",
                "vector (ops)",
                other,
            ))
        }
    };
    let print_t = value::intern("print");
    let cr_t = value::intern("cr");
    let nl_t = value::intern("nl");
    let up_t = value::intern("up");
    let down_t = value::intern("down");
    let col_t = value::intern("col");
    let clear_eol_t = value::intern("clear-eol");
    let clear_below_t = value::intern("clear-below");
    let clear_screen_t = value::intern("clear-screen");
    let mut out: Vec<u8> = Vec::new();
    for op in ops {
        let parts: Vec<Value> = match op {
            Value::Vector(id) => heap.vector(id).to_vec(),
            _ => continue,
        };
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => continue,
        };
        if tag == print_t {
            let s = expect_string(heap, "term-emit", arg(&parts, 1))?;
            apply_face(&mut out, heap, parts.get(2).copied().unwrap_or(Value::Nil))?;
            crossterm::queue!(out, Print(s), SetAttribute(Attribute::Reset), ResetColor)
                .map_err(term_err)?;
        } else if tag == cr_t {
            crossterm::queue!(out, MoveToColumn(0)).map_err(term_err)?;
        } else if tag == nl_t {
            crossterm::queue!(out, Print("\r\n")).map_err(term_err)?;
        } else if tag == up_t {
            let n = expect_int(heap, "term-emit", arg(&parts, 1))?;
            if n > 0 {
                crossterm::queue!(out, MoveUp(clamp_u16(n))).map_err(term_err)?;
            }
        } else if tag == down_t {
            let n = expect_int(heap, "term-emit", arg(&parts, 1))?;
            if n > 0 {
                crossterm::queue!(out, MoveDown(clamp_u16(n))).map_err(term_err)?;
            }
        } else if tag == col_t {
            let n = expect_int(heap, "term-emit", arg(&parts, 1))?;
            crossterm::queue!(out, MoveToColumn(clamp_u16(n))).map_err(term_err)?;
        } else if tag == clear_eol_t {
            crossterm::queue!(out, Clear(ClearType::UntilNewLine)).map_err(term_err)?;
        } else if tag == clear_below_t {
            crossterm::queue!(out, Clear(ClearType::FromCursorDown)).map_err(term_err)?;
        } else if tag == clear_screen_t {
            crossterm::queue!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))
                .map_err(term_err)?;
        }
    }
    write_term_bytes(&out).map_err(term_err)?;
    Ok(Value::Nil)
}

/// Apply a face map (`{:fg :red :bg :blue :bold true :reverse true}`) as
/// crossterm style commands. A non-map (or nil) face is a no-op. Unknown colour
/// names are skipped. Callers reset attributes after the text.
fn apply_face<W: std::io::Write>(out: &mut W, heap: &Heap, face: Value) -> Result<(), LispError> {
    use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
    let Value::Map(id) = face else { return Ok(()) };
    if let Some(fg) = heap.map_get(id, value::kw("fg")).and_then(color_of) {
        crossterm::queue!(out, SetForegroundColor(fg)).map_err(term_err)?;
    }
    if let Some(bg) = heap.map_get(id, value::kw("bg")).and_then(color_of) {
        crossterm::queue!(out, SetBackgroundColor(bg)).map_err(term_err)?;
    }
    if heap.map_get(id, value::kw("bold")).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Bold)).map_err(term_err)?;
    }
    if heap
        .map_get(id, value::kw("italic"))
        .is_some_and(face_truthy)
    {
        crossterm::queue!(out, SetAttribute(Attribute::Italic)).map_err(term_err)?;
    }
    if heap
        .map_get(id, value::kw("underline"))
        .is_some_and(face_truthy)
    {
        crossterm::queue!(out, SetAttribute(Attribute::Underlined)).map_err(term_err)?;
    }
    if heap
        .map_get(id, value::kw("reverse"))
        .is_some_and(face_truthy)
    {
        crossterm::queue!(out, SetAttribute(Attribute::Reverse)).map_err(term_err)?;
    }
    Ok(())
}

/// Brood truthiness for a face flag: only `nil`/`false` are falsy.
fn face_truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// A face colour keyword (`:red`, `:dark-grey`, …) to a crossterm `Color`.
fn color_of(v: Value) -> Option<crossterm::style::Color> {
    use crossterm::style::Color;
    let Value::Keyword(s) = v else { return None };
    Some(match value::symbol_name(s).as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "grey" | "gray" => Color::Grey,
        "dark-grey" | "dark-gray" => Color::DarkGrey,
        _ => return None,
    })
}

/// Clamp a Brood int to a terminal coordinate (crossterm uses `u16`).
fn clamp_u16(n: i64) -> u16 {
    n.clamp(0, u16::MAX as i64) as u16
}

// ---- the GUI frontend (ADR-046, feature "gui") ------------------------------
//
// `gui-*` mirror `term-*`: a second frontend that paints the *same* render-op
// protocol (a frame is the same Brood data) to a native window and reads keys
// back in the same encoding. The window/loop machinery lives in `crate::gui`
// (behind the `gui` feature); these primitives just translate Brood `Value`s ⇄
// the plain `gui::Op`/`gui::Key`/`gui::Face` the backend speaks. A composite
// "broadcast" display in std/observer.blsp drives term + gui (+ remote later)
// from one frame — so the frontends can't drift. Without `--features gui` the
// backend functions return a clear "rebuild with --features gui" error.

/// A face colour keyword (`:red`, `:dark-grey`, …) to an RGB triple for the GUI
/// framebuffer. The same palette `color_of` maps to crossterm `Color`s, so the
/// two frontends agree on what `:red` looks like.
fn color_rgb(v: Value) -> Option<[u8; 3]> {
    let Value::Keyword(s) = v else { return None };
    Some(match value::symbol_name(s).as_str() {
        "black" => [0x00, 0x00, 0x00],
        "red" => [0xcd, 0x31, 0x31],
        "green" => [0x0d, 0xbc, 0x79],
        "yellow" => [0xe5, 0xe5, 0x10],
        "blue" => [0x24, 0x72, 0xc8],
        "magenta" => [0xbc, 0x3f, 0xbc],
        "cyan" => [0x11, 0xa8, 0xcd],
        "white" => [0xe5, 0xe5, 0xe5],
        "grey" | "gray" => [0x80, 0x80, 0x80],
        "dark-grey" | "dark-gray" => [0x50, 0x50, 0x50],
        _ => return None,
    })
}

/// Resolve a face map (`{:fg :red :bg :blue :bold true :reverse true}`) into the
/// plain `gui::Face` the backend renders. A non-map face is the default face.
fn gui_face(heap: &Heap, face: Value) -> crate::gui::Face {
    let mut f = crate::gui::Face::default();
    let Value::Map(id) = face else { return f };
    f.fg = heap.map_get(id, value::kw("fg")).and_then(color_rgb);
    f.bg = heap.map_get(id, value::kw("bg")).and_then(color_rgb);
    f.bold = heap.map_get(id, value::kw("bold")).is_some_and(face_truthy);
    f.italic = heap
        .map_get(id, value::kw("italic"))
        .is_some_and(face_truthy);
    f.underline = heap
        .map_get(id, value::kw("underline"))
        .is_some_and(face_truthy);
    f.reverse = heap
        .map_get(id, value::kw("reverse"))
        .is_some_and(face_truthy);
    // `:family` is a keyword naming a registered font family; carry its interned
    // id so the renderer can pick the matching font set (`:mono` / unknown → default).
    f.family = match heap.map_get(id, value::kw("family")) {
        Some(Value::Keyword(s)) => Some(s),
        _ => None,
    };
    f
}

/// Read a window-id argument (the integer `gui-open` returned) for the windowed
/// primitives. Negative ids clamp to 0 (no such window → a clean "not open" error).
fn gui_window_id(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    Ok(expect_int(heap, who, v)?.max(0) as u64)
}

/// `(gui-open)` — open a new native window and return its integer id. Its key/mouse
/// input is delivered to the **calling process's mailbox** (ADR-058), so the
/// observer parks in `(receive)` rather than pinning a worker in a blocking poll.
/// Starts the GUI thread on the first call; each call is an independent window.
fn gui_open(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let id = crate::gui::open(crate::process::self_pid()).map_err(LispError::runtime)?;
    Ok(Value::Int(id as i64))
}

/// `(gui-close id)` — close window `id` (the teardown for `gui-open`; idempotent).
fn gui_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-close", arg(args, 0))?;
    crate::gui::close(id).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-size id)` — window `id`'s size as `[cols rows]` (character cells), same
/// shape as `term-size`.
fn gui_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-size", arg(args, 0))?;
    let (cols, rows) = crate::gui::size(id).map_err(LispError::runtime)?;
    Ok(heap.alloc_vector(vec![Value::Int(cols as i64), Value::Int(rows as i64)]))
}

/// `(gui-draw id frame)` — paint a frame (the same op vector `term-draw` takes) to
/// window `id`. Parses the ops into plain `gui::Op`s (it has heap access) and ships
/// them to the GUI thread. Unknown ops are skipped (forward-compatible).
fn gui_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let win = gui_window_id(heap, "gui-draw", arg(args, 0))?;
    let ops_v: Vec<Value> = match arg(args, 1) {
        Value::Vector(id) => heap.vector(id).to_vec(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "gui-draw",
                "vector (a frame)",
                other,
            ))
        }
    };
    let clear_t = value::intern("clear");
    let text_t = value::intern("text");
    let cursor_t = value::intern("cursor");
    let mut ops = Vec::with_capacity(ops_v.len());
    for op in ops_v {
        let parts: Vec<Value> = match op {
            Value::Vector(id) => heap.vector(id).to_vec(),
            _ => continue,
        };
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => continue,
        };
        if tag == clear_t {
            ops.push(crate::gui::Op::Clear);
        } else if tag == cursor_t {
            let row = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let col = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            ops.push(crate::gui::Op::Cursor { row, col });
        } else if tag == text_t {
            let row = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let col = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            let s = expect_string(heap, "gui-draw", arg(&parts, 3))?;
            let face = gui_face(heap, parts.get(4).copied().unwrap_or(Value::Nil));
            ops.push(crate::gui::Op::Text { row, col, s, face });
        }
    }
    crate::gui::draw(win, ops).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// Read a `:height` value from a font spec as a pixel size (int or float), or None.
fn font_px(heap: &Heap, id: crate::core::value::MapId) -> Option<f32> {
    match heap.map_get(id, value::kw("height")) {
        Some(Value::Int(n)) => Some(n as f32),
        Some(Value::Float(f)) => Some(f as f32),
        _ => None,
    }
}

/// `(gui-font! spec)` — set the global default cell font from `spec`, a map
/// `{:family <keyword> :height <px>}` (either key optional): `:family` picks a
/// registered font family (the bundled `:mono`, or one added by
/// `gui-font-register`), `:height` the cell pixel size. Applies to every open
/// window and any opened later — the whole-window knob (per-section fonts come
/// from a face's `:family`). Returns nil.
fn gui_font(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let Value::Map(id) = arg(args, 0) else {
        return Err(LispError::wrong_type(
            heap,
            "gui-font!",
            "map (a font spec)",
            arg(args, 0),
        ));
    };
    let family = match heap.map_get(id, value::kw("family")) {
        Some(Value::Keyword(s)) => Some(s),
        _ => None,
    };
    crate::gui::font(family, font_px(heap, id)).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-font-register name styles)` — register font family `name` (a keyword) from
/// `styles`, a map of style → TTF file path: `{:regular "…" :bold "…" :italic "…"
/// :bold-italic "…"}`. Only `:regular` is required; a missing style reuses the
/// regular file (so a single-file family works). The fonts are read here and parsed
/// on the GUI thread; afterwards a face's `:family <name>` selects them. Returns
/// `name`.
fn gui_font_register(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) => s,
        other => return Err(LispError::wrong_type(heap, "gui-font-register", "keyword", other)),
    };
    let Value::Map(id) = arg(args, 1) else {
        return Err(LispError::wrong_type(
            heap,
            "gui-font-register",
            "map (style → path)",
            arg(args, 1),
        ));
    };
    // a style's path, or None when the key is absent/nil
    let path = |key: &str| -> Result<Option<String>, LispError> {
        match heap.map_get(id, value::kw(key)) {
            None | Some(Value::Nil) => Ok(None),
            Some(v) => Ok(Some(expect_string(heap, "gui-font-register", v)?)),
        }
    };
    let read = |p: &str| -> Result<Vec<u8>, LispError> {
        std::fs::read(p).map_err(|e| LispError::runtime(format!("gui-font-register: {p}: {e}")))
    };
    let regular_path = path("regular")?
        .ok_or_else(|| LispError::runtime("gui-font-register: a :regular path is required"))?;
    let regular = read(&regular_path)?;
    // each missing style falls back to the regular file's bytes
    let style = |key: &str| -> Result<Vec<u8>, LispError> {
        match path(key)? {
            Some(p) => read(&p),
            None => Ok(regular.clone()),
        }
    };
    let bold = style("bold")?;
    let italic = style("italic")?;
    let bold_italic = style("bold-italic")?;
    crate::gui::register_family(name, regular, bold, italic, bold_italic)
        .map_err(LispError::runtime)?;
    Ok(Value::Keyword(name))
}

/// `(mailbox-size pid)` — the number of queued messages in a local process's
/// mailbox, or `nil` for a remote/dead pid. The one process-introspection
/// accessor Brood can't reach (the queue lives behind the scheduler registry);
/// `std/observer.blsp` assembles everything else (id, liveness) from Brood.
fn mailbox_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            Ok(crate::process::mailbox_len(id)
                .map(|n| Value::Int(n as i64))
                .unwrap_or(Value::Nil))
        }
        Value::Pid { .. } => Ok(Value::Nil),
        other => Err(LispError::wrong_type(heap, "mailbox-size", "pid", other)),
    }
}

/// `(process-info pid)` — a snapshot map of a **live local** process, or `nil`
/// for a remote/dead pid (a non-pid is a type error). The fields are all
/// kernel-internal, so the map is assembled here from the registry / scheduler /
/// name / monitor tables (ADR-051):
///
///   `{:id <int> :node <kw> :name <kw|nil> :status <kw> :mailbox <int>
///     :monitored-by <int> :parent <int|nil>}`
///
/// `:status` is `:running` / `:waiting` (parked in `receive`). `:name` is the
/// registered name or nil. `:parent` is the spawner's id (nil for the root).
/// `:memory` (per-process bytes) joins once the kernel tracks it, and `:status`
/// sharpens when an explicit state enum lands (the observer tolerates the gap).
/// Each accessor takes one lock independently, so no two are held at once.
fn process_info(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Dead/unknown pid → nil (matches `mailbox-size`).
            if !crate::process::is_alive(id) {
                return Ok(Value::Nil);
            }
            let name = crate::dist::name_for_pid(id)
                .map(Value::Keyword)
                .unwrap_or(Value::Nil);
            let status = crate::process::process_status(id)
                .map(value::kw)
                .unwrap_or(Value::Nil);
            let mailbox = Value::Int(crate::process::mailbox_len(id).unwrap_or(0) as i64);
            let monitored = Value::Int(crate::process::monitored_by(id) as i64);
            // `:parent` is the spawner's id, or nil for the root.
            let parent = crate::process::parent_of(id)
                .map(|p| Value::Int(p as i64))
                .unwrap_or(Value::Nil);
            // `:memory` — the process's LOCAL heap footprint (bytes), published on
            // its last `receive`; 0 for a process that has never received.
            let memory = Value::Int(crate::process::process_mem(id).unwrap_or(0) as i64);
            // `:collections` — the process's cumulative GC count, republished on
            // its last `receive` (0 for one that has never received). The signal
            // for "is this process churning memory?" in the observer.
            let collections = Value::Int(crate::process::process_gc_runs(id).unwrap_or(0) as i64);
            // `:reductions` — the process's cumulative reduction count (Erlang's
            // scheduling unit), updated every scheduling quantum. The observer's
            // "is this process doing work / busy?" signal. Exact for spawned
            // processes; coarse (whole-budget increments) for the root.
            let reductions = Value::Int(crate::process::process_reductions(id).unwrap_or(0) as i64);
            let pairs = vec![
                (value::kw("id"), Value::Int(id as i64)),
                // The process's actual pid value (not just its numeric id), so a
                // caller — e.g. the observer's kill command — can act on the
                // process directly with `exit`/`send`/`monitor`.
                (value::kw("pid"), Value::Pid { node, id }),
                (value::kw("node"), Value::Keyword(node)),
                (value::kw("name"), name),
                (value::kw("status"), status),
                (value::kw("mailbox"), mailbox),
                (value::kw("monitored-by"), monitored),
                (value::kw("parent"), parent),
                (value::kw("memory"), memory),
                (value::kw("collections"), collections),
                (value::kw("reductions"), reductions),
            ];
            Ok(heap.map_from_pairs(pairs))
        }
        Value::Pid { .. } => Ok(Value::Nil),
        other => Err(LispError::wrong_type(heap, "process-info", "pid", other)),
    }
}

/// `(string->number s)` — parse `s` as an integer if it is one, else as a float,
/// else `nil`. The inverse of `number->string`. A robust parse-or-nil can't be
/// expressed over `read-string` (which would read `"3abc"` as `3` and stop), so
/// the strict parse is a primitive. Surrounding whitespace is not accepted —
/// `trim` first if the input may carry any.
fn string_to_number(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->number", arg(args, 0))?;
    if let Ok(i) = s.parse::<i64>() {
        Ok(Value::Int(i))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(Value::Float(f))
    } else {
        Ok(Value::Nil)
    }
}

// ---------- filesystem ----------
// Mechanism only: existence / directory reflection so the Brood module system and
// the project test runner can resolve load paths and discover test files. Path
// manipulation and all policy live in Brood (`std/prelude.blsp`, `std/project.blsp`).

/// `(cwd)` — the process's current working directory as a string.
fn cwd(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_dir() {
        Ok(p) => Ok(heap.alloc_string(&p.to_string_lossy())),
        Err(e) => {
            Err(LispError::runtime(format!("cwd: {}", e))
                .with_code(crate::error::error_codes::FILE_IO))
        }
    }
}

/// `(file-exists? path)` — true if a file or directory exists at `path`.
fn file_exists(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-exists?", arg(args, 0))?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

/// `(dir? path)` — true if `path` exists and is a directory.
fn is_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "dir?", arg(args, 0))?;
    Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
}

/// `(list-dir path)` — the entry names (not full paths) directly under a
/// directory, sorted for determinism. Errors if `path` isn't a readable directory.
fn list_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "list-dir", arg(args, 0))?;
    let mut names: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) => {
            return Err(LispError::runtime(format!("list-dir: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO))
        }
    };
    names.sort();
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(heap.alloc_string(n));
    }
    Ok(heap.list(items))
}

/// `(make-dir path)` — create `path` and any missing parents (like `mkdir -p`).
/// Returns nil. Used by the project scaffolder (`nest new`).
fn make_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "make-dir", arg(args, 0))?;
    std::fs::create_dir_all(&path).map_err(|e| {
        LispError::runtime(format!("make-dir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Nil)
}

/// `(spit path content)` — write `content` (a string) to `path`, replacing any
/// existing file. Returns nil. The write-side counterpart to `load`.
fn spit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let path = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string path", pv)),
    };
    let cv = arg(args, 1);
    let content = match cv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string content", cv)),
    };
    std::fs::write(&path, content).map_err(|e| {
        LispError::runtime(format!("spit: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Nil)
}

/// `(%sha256 s)` — the lowercase hex SHA-256 of `s`'s UTF-8 bytes. The single
/// hashing mechanism for the package manager (ADR-037); per-file hashing
/// (`(%sha256 (slurp p))`) and the canonical directory-tree hash are Brood over
/// it (`std/package.blsp`), keeping a directory walk out of the kernel.
fn sha256_hex(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha2::{Digest, Sha256};
    let s = expect_string(heap, "%sha256", arg(args, 0))?;
    let digest = Sha256::digest(s.as_bytes());
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    Ok(heap.alloc_string(&hex))
}

/// Run `git` with `args` (optionally in `cwd`), capturing stdout+stderr. The
/// shared mechanism behind the package manager's git primitives (ADR-037).
fn run_git(args: &[&str], cwd: Option<&str>) -> Result<std::process::Output, LispError> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.output().map_err(|e| {
        LispError::runtime(format!("git {}: {}", args.join(" "), e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
            .with_hint("is `git` installed and on PATH?")
    })
}

/// Run a `git` subcommand that's expected to succeed; turn a non-zero exit into a
/// `LispError` carrying git's stderr.
fn git_or_err(args: &[&str], cwd: Option<&str>) -> Result<(), LispError> {
    let out = run_git(args, cwd)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(LispError::runtime(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED))
    }
}

/// `(%git-resolve-ref url ref)` — resolve `ref` (a tag, branch, or commit) at the
/// remote `url` to a full commit hash via `git ls-remote`, or `nil` if no such
/// ref exists. For an annotated tag, prefers the peeled `^{}` line (the commit the
/// tag points to). When `ref` is already a commit SHA the remote doesn't advertise
/// (ls-remote returns nothing), it's returned as-is — a commit pins itself.
/// The package manager's ref-pinning mechanism (ADR-037); pinning policy is Brood.
fn git_resolve_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-resolve-ref", arg(args, 0))?;
    let r = expect_string(heap, "%git-resolve-ref", arg(args, 1))?;
    let out = run_git(&["ls-remote", &url, &r], None)?;
    if !out.status.success() {
        return Err(LispError::runtime(format!(
            "%git-resolve-ref: git ls-remote {} {} failed: {}",
            url,
            r,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut first: Option<&str> = None;
    let mut peeled: Option<&str> = None;
    for line in stdout.lines() {
        let sha = line.split_whitespace().next();
        if first.is_none() {
            first = sha;
        }
        if line.trim_end().ends_with("^{}") {
            peeled = sha;
        }
    }
    if let Some(s) = peeled.or(first) {
        return Ok(heap.alloc_string(s));
    }
    // No advertised ref: if `ref` itself looks like a commit SHA, it pins itself.
    let looks_like_sha = r.len() >= 7 && r.len() <= 40 && r.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_sha {
        Ok(heap.alloc_string(&r))
    } else {
        Ok(Value::Nil)
    }
}

/// `(%git-clone url dest ref commit)` — populate `dest` with a shallow clone of
/// `url` checked out at the exact `commit` (detached HEAD). Tries to fetch the
/// commit directly (servers that allow SHA-in-want, e.g. GitHub); falls back to
/// fetching `ref` then checking out `commit`. Returns `:ok`, or throws with git's
/// stderr. The package manager's fetch mechanism (ADR-037); the cache layout and
/// when-to-reclone policy are Brood (std/package.blsp).
fn git_clone(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-clone", arg(args, 0))?;
    let dest = expect_string(heap, "%git-clone", arg(args, 1))?;
    let gref = expect_string(heap, "%git-clone", arg(args, 2))?;
    let commit = expect_string(heap, "%git-clone", arg(args, 3))?;

    if let Some(parent) = std::path::Path::new(&dest).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                LispError::runtime(format!("%git-clone: cannot create {}: {}", parent.display(), e))
                    .with_code(crate::error::error_codes::FILE_IO)
            })?;
        }
    }

    git_or_err(&["init", "-q", &dest], None)?;
    git_or_err(&["-C", &dest, "remote", "add", "origin", &url], None)?;

    // Fast path: fetch the exact commit shallowly. Many servers (GitHub) allow it.
    let direct = run_git(&["-C", &dest, "fetch", "-q", "--depth", "1", "origin", &commit], None)?;
    if !direct.status.success() {
        // Fallback: fetch the named ref (shallow first, then full if the server
        // rejects a shallow ref fetch), which must contain the locked commit.
        if git_or_err(&["-C", &dest, "fetch", "-q", "--depth", "1", "origin", &gref], None).is_err() {
            git_or_err(&["-C", &dest, "fetch", "-q", "origin", &gref], None)?;
        }
    }

    if git_or_err(&["-C", &dest, "checkout", "-q", "--detach", &commit], None).is_err() {
        return Err(LispError::runtime(format!(
            "%git-clone: commit {} is not reachable from {} at {}",
            commit, gref, url
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
        .with_hint("the ref may have moved since it was locked — try `nest update`"));
    }
    Ok(crate::core::value::kw("ok"))
}

/// `(%rm-rf path)` — recursively delete `path`. **Bounded to `_deps/`**: refuses
/// any path without a `_deps` component, so a mis-computed cache path can't delete
/// something outside the package cache. Idempotent (`:ok` if already absent). The
/// package manager's cache-eviction mechanism (ADR-037); `nest update` re-clones.
fn rm_rf(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%rm-rf", arg(args, 0))?;
    let under_deps = std::path::Path::new(&path)
        .components()
        .any(|c| c.as_os_str() == "_deps");
    if !under_deps {
        return Err(LispError::runtime(format!(
            "%rm-rf: refusing to delete {} — only paths under _deps/ may be removed",
            path
        ))
        .with_code(crate::error::error_codes::FILE_IO));
    }
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(crate::core::value::kw("ok")),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(crate::core::value::kw("ok")),
        Err(e) => Err(LispError::runtime(format!("%rm-rf: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(slurp path)` — read the whole file at `path` and return it as a string. The
/// read-side counterpart to `spit`; unlike `load` it does not evaluate, so the
/// doc tooling can inspect a module's source (e.g. its leading docstring form).
/// `(read-line)` — read one line from stdin, returning it as a string with the
/// trailing newline stripped, or `nil` at end of input (EOF / Ctrl-D). The one
/// irreducible I/O mechanism the Brood-hosted REPL (`std/repl.blsp`) can't
/// bootstrap; line *editing* on a TTY comes free from the terminal's cooked
/// mode, so this stays a plain blocking read.
fn read_line(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::BufRead;
    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line).map_err(|e| {
        LispError::runtime(format!("read-line: {}", e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    if n == 0 {
        return Ok(Value::Nil); // EOF
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(heap.alloc_string(&line))
}

fn slurp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "slurp", arg(args, 0))?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("slurp: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(heap.alloc_string(&content))
}

/// `(file-mtime path)` — last-modified time of `path` as epoch-milliseconds, or
/// `nil` if the file is missing or its mtime can't be read. A cheap `stat`, not a
/// read — pairs with `load` to drive a hot-reloader: poll `file-mtime`, reload
/// only when it changes. Resolution is platform-dependent (typically nanoseconds
/// on Linux, truncated to ms here).
/// `(file-size path)` — the size of `path` in bytes, or nil if it's missing.
/// GC-safe: the arg is copied to an owned `String` up front and the result is a
/// scalar — no `Value` handle is held across an allocation or eval (and a builtin
/// never fires GC mid-execution; see `docs/memory-model.md`).
fn file_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-size", arg(args, 0))?;
    match std::fs::metadata(&path) {
        Ok(meta) => Ok(Value::Int(meta.len() as i64)),
        Err(_) => Ok(Value::Nil),
    }
}

/// `(delete-file path)` — remove the file at `path`. Idempotent (nil if already
/// absent); errors on a real I/O failure (e.g. it's a directory, or permission).
fn delete_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "delete-file", arg(args, 0))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::Nil),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Nil),
        Err(e) => Err(LispError::runtime(format!("delete-file: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(rename-file from to)` — rename/move `from` to `to` (replacing `to` if it
/// exists, per the platform). Returns nil; errors on failure.
fn rename_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "rename-file", arg(args, 0))?;
    let to = expect_string(heap, "rename-file", arg(args, 1))?;
    std::fs::rename(&from, &to).map_err(|e| {
        LispError::runtime(format!("rename-file: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Nil)
}

fn file_mtime(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-mtime", arg(args, 0))?;
    let Ok(meta) = std::fs::metadata(&path) else {
        return Ok(Value::Nil);
    };
    let Ok(modified) = meta.modified() else {
        return Ok(Value::Nil);
    };
    let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return Ok(Value::Nil);
    };
    Ok(Value::Int(since.as_millis() as i64))
}

/// `(getenv name)` — the value of environment variable `name` as a string, or nil
/// if it is unset. Lets Brood locate things like the user config directory.
fn getenv(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_string(heap, "getenv", arg(args, 0))?;
    match std::env::var(&name) {
        Ok(val) => Ok(heap.alloc_string(&val)),
        Err(_) => Ok(Value::Nil),
    }
}

/// `(run-process prog args)` — run external program `prog` with `args` (a list or
/// vector of strings), inheriting stdio, and return its exit code as an integer
/// (-1 if killed by a signal). The Emacs `call-process` analogue: the general
/// subprocess mechanism (used by the project scaffolder's `git init`).
fn run_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let prog = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "run-process",
                "string program",
                pv,
            ))
        }
    };
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        match a {
            Value::Str(id) => argv.push(heap.string(id).to_string()),
            _ => {
                return Err(LispError::type_err(
                    "run-process: arguments must be strings",
                ))
            }
        }
    }
    match std::process::Command::new(&prog).args(&argv).status() {
        Ok(status) => Ok(Value::Int(status.code().unwrap_or(-1) as i64)),
        Err(e) => Err(LispError::runtime(format!("run-process: {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
            .with_hint("check that the program is on PATH and the args are well-formed")),
    }
}

fn apply_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity(
            "apply: expected a function and an argument list",
        ));
    }
    // Bind `last` after the guard so the slice indexing below is robust to
    // refactors of the guard: anyone moving / tightening it can't accidentally
    // leave a bare `args[args.len() - 1]` indexing into an empty slice.
    let last = args.len() - 1;
    let f = args[0];
    let mut argv = args[1..last].to_vec();
    argv.extend(heap.seq_items(args[last])?);
    apply(heap, f, &argv, env)
}

// ---------- macros ----------

fn macroexpand_1(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let (expanded, _) = crate::eval::macros::macroexpand_1(heap, arg(args, 0), env)?;
    Ok(expanded)
}
// `macroexpand` is now a Brood prelude fn over `macroexpand-1` (ADR-064).

/// `(check 'form)` — run the advisory type checker over `form` (macro-expanded
/// first, like the real compile pass) and return a list of warning strings, or
/// `nil` when nothing is provably wrong. Advisory only: it never raises.
fn check_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    let warnings = crate::types::check::check_form(heap, form);
    let mut out = Vec::with_capacity(warnings.len());
    for w in &warnings {
        out.push(heap.alloc_string(w));
    }
    Ok(heap.list(out))
}

/// `(check-file path)` — run the advisory type checker over every top-level
/// form in the file at `path` and return a list of pre-formatted warning
/// strings (each `"path:line:col: warning: message"`), or `nil` if clean.
///
/// Reads but does **not** evaluate the file — same `check_file` walk the
/// `brood --check` CLI uses, with the file-globals accumulator threaded
/// across top-level forms. The whole-file-at-once shape is what lets `(defn
/// foo …)` at line 1 silence the unbound check on `(foo …)` at line 100. Used
/// by `(check-project)` in `std/project.blsp` for the `nest test` / `nest run`
/// pre-flight.
fn check_file_builtin(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("check-file: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
    let mut out = Vec::with_capacity(warnings.len());
    for (pos, msg) in &warnings {
        let s = match pos {
            Some(p) => format!("{}:{}:{}: warning: {}", path, p.line, p.col, msg),
            None => format!("{}: warning: {}", path, msg),
        };
        out.push(heap.alloc_string(&s));
    }
    Ok(heap.list(out))
}

/// `(check-file-structured path)` — the data-shaped counterpart of
/// `check-file`. Returns a list of `{:file :line :col :message}` maps (or
/// `{:file :message}` for warnings without a position — the advisory
/// checker doesn't carry spans through macroexpansion yet, ADR-024). Used
/// by the `nest mcp` `check` tool (step 1c-a) and any other consumer that
/// wants structured diagnostics rather than a GNU-line string to re-parse.
fn check_file_structured(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file-structured", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!(
            "check-file-structured: cannot read {}: {}",
            path, e
        ))
        .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
    let file_kw = Value::Keyword(value::intern("file"));
    let line_kw = Value::Keyword(value::intern("line"));
    let col_kw = Value::Keyword(value::intern("col"));
    let msg_kw = Value::Keyword(value::intern("message"));
    let file_val = heap.alloc_string(&path);
    let mut out = Vec::with_capacity(warnings.len());
    for (pos_opt, msg) in &warnings {
        let msg_val = heap.alloc_string(msg);
        let mut entries: Vec<(Value, Value)> = Vec::with_capacity(4);
        entries.push((file_kw, file_val));
        if let Some(p) = pos_opt {
            entries.push((line_kw, Value::Int(p.line as i64)));
            entries.push((col_kw, Value::Int(p.col as i64)));
        }
        entries.push((msg_kw, msg_val));
        out.push(heap.map_from_pairs(entries));
    }
    Ok(heap.list(out))
}

// ---------- source positions (editor tooling; see docs/tooling.md) ----------

/// `(form-pos form)` — the `[line col]` (1-based) where `form` was read, or
/// `nil`. Recorded by the reader for list forms; used by the test macros to
/// capture a test's source line *before* the form expands.
fn form_pos(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.form_pos(arg(args, 0)) {
        Some(p) => Ok(heap.alloc_vector(vec![Value::Int(p.line as i64), Value::Int(p.col as i64)])),
        None => Ok(Value::Nil),
    }
}

/// `(current-file)` — the path of the file currently being `load`ed, or `nil`
/// (e.g. at the REPL). Maintained by `load`.
/// `(source-location 'name)` — where `name`'s global definition was loaded from,
/// as `[file line col]`, or `nil` if it has no recorded site (a Rust builtin, or
/// an unknown/local name). Prelude globals resolve to a materialized copy of the
/// standard library. The site is captured at load time
/// before macroexpansion, so `defn`/`defmacro` definitions are located
/// accurately. The image-query foundation for cross-file goto-definition (ADR-031
/// / docs/lsp.md). Takes a symbol, so quote it: `(source-location 'foo)`.
fn source_location(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "source-location",
                "symbol",
                other,
            ))
        }
    };
    match heap.def_site(name) {
        Some(loc) => {
            let file = heap.alloc_string(&loc.file);
            Ok(heap.alloc_vector(vec![
                file,
                Value::Int(loc.pos.line as i64),
                Value::Int(loc.pos.col as i64),
            ]))
        }
        None => Ok(Value::Nil),
    }
}

/// `(references-in-source name source)` — every occurrence of the global `name`
/// in `source`, as a list of `[line col]` (both 1-based), in document order. A
/// local that shadows the name is excluded. Pure: it parses the string and
/// holds no project state, so the Brood-side `callers` MCP tool maps it over a
/// project's files for cross-file find-references (ADR-031 §Cross-file,
/// docs/lsp.md). `name` may be a symbol or a string.
fn references_in_source(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "references-in-source",
                "symbol or string",
                other,
            ))
        }
    };
    let src = expect_string(heap, "references-in-source", arg(args, 1))?;
    let root = cst::parse(&src);
    let tree = crate::syntax::scope::analyze(&root, &src);
    let starts = line_starts(&src);
    let occ: Vec<Value> = tree
        .references_to_global(&root, &src, &name)
        .into_iter()
        .map(|span| {
            let (line, col) = line_col(&src, &starts, span.start as usize);
            heap.alloc_vector(vec![Value::Int(line as i64), Value::Int(col as i64)])
        })
        .collect();
    Ok(heap.list(occ))
}

/// Byte offsets of each line start in `src` (line 0 begins at 0). Built once so
/// repeated byte→line/col lookups in one source are cheap.
fn line_starts(src: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(src.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// 1-based (line, col) of byte offset `b`, col counted in characters. `b` must
/// be a char boundary (CST spans always are).
fn line_col(src: &str, starts: &[usize], b: usize) -> (u32, u32) {
    let line = starts.partition_point(|&s| s <= b) - 1; // 0-based
    let col = src[starts[line]..b].chars().count();
    (line as u32 + 1, col as u32 + 1)
}

fn current_file(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.current_file().map(str::to_string) {
        Some(f) => Ok(heap.alloc_string(&f)),
        None => Ok(Value::Nil),
    }
}

// ---------- introspection (editor tooling; see docs/lsp.md) ----------

/// `(doc f)` — the docstring of a function or macro value, or `nil`. A docstring
/// is the leading string literal in a `fn`/`defn` body (stored on the closure
/// when more body follows it). Powers hover / `describe-function`.
fn doc(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let text = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => heap.closure(id).doc.clone(),
        // A primitive's docstring lives on the `NativeFn` (the `PRIMITIVE_DOCS`
        // table), since it has no Brood body to carry a leading string.
        Value::Native(id) => {
            let d = heap.native(id).doc;
            (!d.is_empty()).then(|| d.to_string())
        }
        _ => None,
    };
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::Nil),
    }
}

/// `(arglist f)` — the parameter list of a function, macro, or primitive as a
/// list, mirroring the source surface: required names, then `&optional` names,
/// then `& rest`. `nil` for a non-function (or a primitive without recorded
/// params). Feeds signature help / hover.
fn arglist(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let id = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => id,
        // A primitive carries its param names as a flat `&'static` list (incl. any
        // `&`/`&optional` markers, already in order) — hand them back as symbols.
        Value::Native(id) => {
            let params = heap.native(id).params;
            if params.is_empty() {
                return Ok(Value::Nil);
            }
            let items: Vec<Value> = params.iter().map(|p| value::sym(p)).collect();
            return Ok(heap.list(items));
        }
        _ => return Ok(Value::Nil),
    };
    // Copy the parts out before re-borrowing the heap mutably to build the list.
    // For a multi-arity closure there's no single arglist; show the last clause
    // (conventionally the most general — e.g. the variadic `(a b & more)`).
    let (params, optionals, rest) = {
        let cl = heap.closure(id);
        let arm = cl.arms.last().expect("closure has at least one arm");
        (
            arm.params.clone(),
            arm.optionals.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
            arm.rest,
        )
    };
    let mut items: Vec<Value> = params.into_iter().map(Value::Sym).collect();
    if !optionals.is_empty() {
        items.push(value::sym("&optional"));
        items.extend(optionals.into_iter().map(Value::Sym));
    }
    if let Some(r) = rest {
        items.push(value::sym("&"));
        items.push(Value::Sym(r));
    }
    Ok(heap.list(items))
}

/// `(global-names)` — a list of every symbol bound in the global table
/// (prelude + user `def`s), sorted by spelling so the order is deterministic
/// (for completion / workspace-symbol tooling and reproducible doc generation).
/// Special forms and the core control/binding macros — the keyword-like heads:
/// the single source of truth for "what reads as a keyword". Read from Brood via
/// the `(special-forms)` primitive (so `std/highlight.blsp` highlights from this
/// list) and from the LSP (`semantic_tokens` / `completion` import it rather than
/// keeping a copy), so the runtime and the tooling can't drift. Mirrors
/// `brood.el`'s `brood-special-forms` plus the `def`-family heads.
pub const SPECIAL_FORMS: &[&str] = &[
    "if",
    "do",
    "def",
    "fn",
    "lambda",
    "let",
    "let*",
    "letrec",
    "quote",
    "quasiquote",
    "defmacro",
    "defn",
    "defdyn",
    "defmodule",
    "when",
    "unless",
    "cond",
    "and",
    "or",
    "match",
    "match*",
    "try",
    "catch",
    "throw",
    "receive",
    "binding",
    "dolist",
    "doseq",
    "dotimes",
    "for",
    "->",
    "->>",
];

/// `(special-forms)` — the list of special-form / core-macro names (strings) that
/// read as keywords, for tooling (the highlighter, completion). Returns the
/// canonical `SPECIAL_FORMS`, so Brood and the LSP share one list.
fn special_forms(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = SPECIAL_FORMS.iter().map(|s| heap.alloc_string(s)).collect();
    Ok(heap.list(items))
}

fn global_names(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let mut syms = heap.global_symbols();
    // `symbol_name` locks the interner and allocates, so resolve each spelling
    // once (cached) rather than twice per comparison.
    syms.sort_by_cached_key(|&s| value::symbol_name(s));
    let syms: Vec<Value> = syms.into_iter().map(Value::Sym).collect();
    Ok(heap.list(syms))
}

/// `(bound? 'name)` — whether `name` is bound in the current scope (which
/// reaches the global table). Takes a symbol, so quote it: `(bound? 'foo)`.
fn bound_p(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Sym(s) => Ok(Value::Bool(heap.env_get(env, s).is_some())),
        other => Err(LispError::wrong_type(heap, "bound?", "symbol", other)),
    }
}

fn gensym(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Str(id) => heap.string(id).to_string(),
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Nil => "g".to_string(),
        other => printer::display(heap, other),
    };
    Ok(value::gensym(&prefix))
}

// ---------- errors / control ----------

fn throw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Err(LispError::thrown(arg(args, 0), heap))
}

/// `(%force-panic [msg])` — debug-only. Deliberately panics from a primitive,
/// so tests can exercise the host-side `catch_unwind` boundary (currently the
/// MCP server's `call_tool`). Not a Brood-clean error path — this *is* a Rust
/// `panic!`; if no host catches it, the process dies. There's no Brood
/// reason to call this outside the regression test.
#[cfg(debug_assertions)]
fn force_panic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let msg = match args.first() {
        Some(Value::Str(id)) => heap.string(*id).to_string(),
        Some(other) => printer::display(heap, *other),
        None => "%force-panic invoked (no message)".to_string(),
    };
    panic!("{}", msg);
}

/// `(%blob-ptr s)` — debug-only. The raw `SharedBlob` address backing `s`,
/// as an integer (for identity comparison across processes). `nil` for
/// inline (small) strings and PRELUDE/RUNTIME handles.
#[cfg(debug_assertions)]
fn blob_ptr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_ptr(id)
            .map(|p| Value::Int(p as i64))
            .unwrap_or(Value::Nil)),
        other => Err(LispError::type_err(format!(
            "%blob-ptr: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

/// `(%blob-strong-count s)` — debug-only. Current `Arc::strong_count` for
/// the `SharedBlob` backing `s`. `nil` for inline / non-LOCAL strings.
/// Approximate under live concurrent senders/receivers (the count moves);
/// stable when callers are quiescent (what the leak-check test asserts).
#[cfg(debug_assertions)]
fn blob_strong_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_strong_count(id)
            .map(|n| Value::Int(n as i64))
            .unwrap_or(Value::Nil)),
        other => Err(LispError::type_err(format!(
            "%blob-strong-count: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

// ---------- processes ----------

fn spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pid = crate::process::spawn(heap, arg(args, 0))?;
    Ok(crate::process::pid_value(pid))
}

/// `(%spawn-named name thunk)` — idempotent named spawn. If `name` (a
/// keyword or symbol) is currently registered to a still-alive pid, return
/// that pid and **do not** spawn — `thunk` is never evaluated. Otherwise,
/// drop any stale registration, spawn the thunk as a new green process,
/// register it under `name`, and return the new pid.
///
/// The check-or-spawn step is atomic under `NAMES`'s write lock — two
/// concurrent `(spawn :name …)` calls can't both spawn; the loser sees
/// the winner's pid. The user-facing `(spawn name expr)` macro wraps an
/// expression into a thunk the same way `(spawn expr)` does, so the
/// expression's free locals are captured lexically (ADR-033).
fn spawn_named(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) | Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%spawn-named",
                "keyword or symbol",
                v,
            ))
        }
    };
    let thunk = arg(args, 1);
    if !matches!(thunk, Value::Fn(_)) {
        return Err(LispError::wrong_type(
            heap,
            "%spawn-named",
            "function",
            thunk,
        ));
    }
    // `spawn_or_get`'s spawner is fallible — `?` propagates a real
    // `LispError` if `process::spawn` rejects the thunk (defensive: with the
    // `Value::Fn(_)` type-check above, that shouldn't fire today, but a
    // future change to `promote`/`spawn` won't silently panic).
    let pid = crate::dist::spawn_or_get(name, || crate::process::spawn(heap, thunk))?;
    Ok(crate::process::pid_value(pid))
}

fn send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::send(heap, arg(args, 0), arg(args, 1))?;
    Ok(Value::Nil)
}

/// `(exit pid reason)` — send an exit signal to a local green process (Erlang
/// `exit/2`). `reason = :kill` is the untrappable hard kill (dies at its next
/// reduction tick, or now if parked); any other reason is the soft signal (dies at
/// its next `receive`). Returns nil. A no-op for a dead/unknown pid.
fn exit_proc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let reason = crate::process::to_message(heap, arg(args, 1))?;
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::exit(id, reason);
            Ok(Value::Nil)
        }
        // Cross-node exit (ADR-067): ship a non-link `Frame::Exit` routed to the
        // peer's `scheduler::exit` (kill-style, like the local path).
        Value::Pid { node, id } => {
            crate::dist::exit_remote(node, id, reason);
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err("exit: first argument must be a pid")),
    }
}

/// `(link pid)` — symmetrically link the current process and `pid`, local or
/// remote (ADR-067). A cross-node link ships a `Frame::Link`; either side's death
/// reaches the other, and a net-split fires `:noconnection`. Returns nil.
fn link_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::link_self(id);
            Ok(Value::Nil)
        }
        Value::Pid { node, id } => {
            crate::dist::link_remote(node, id, crate::process::self_pid());
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err("link: argument must be a pid")),
    }
}

/// `(unlink pid)` — drop the link between the current process and `pid` (local or
/// remote).
fn unlink_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::unlink_self(id);
            Ok(Value::Nil)
        }
        Value::Pid { node, id } => {
            crate::dist::unlink_remote(node, id, crate::process::self_pid());
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err("unlink: argument must be a pid")),
    }
}

/// `(trap-exit on)` — set the current process's `trap_exit` flag; return the
/// previous value. Only `nil`/`false` are falsy (the language truthiness rule).
fn trap_exit_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let on = !matches!(arg(args, 0), Value::Nil | Value::Bool(false));
    let prev = crate::process::set_trap_exit(crate::process::self_pid(), on);
    Ok(Value::Bool(prev))
}

/// `(monitor pid)` — watch `pid`; returns a monitor `ref`. The caller receives
/// `[:down <ref> <pid> <reason>]` when `pid` dies (immediately, reason `:noproc`,
/// if it is already dead).
fn monitor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Local pid: in-process registration, returns a fresh mref.
            Ok(crate::process::monitor(id))
        }
        Value::Pid { node, id } => {
            // Remote pid: same shape — mint a mref, register *here* (so
            // demonitor can find it later, and net-split can fire
            // `:noconnection`), and ship a `Frame::Monitor` to the peer
            // which routes through the same `process::add_monitor` on the
            // far side.
            let mref = crate::process::next_ref();
            let watcher = crate::process::self_pid();
            crate::dist::monitor_remote(node, id, watcher, mref);
            Ok(Value::Ref(mref))
        }
        // `{:name n :node node}` address: resolve to a pid via `whereis` and
        // monitor that pid. Only the local-node case is supported — a remote
        // `{:name :node}` address has no protocol to resolve the name on the
        // far side at monitor time, so we redirect the user to ship the pid
        // directly. Documented in `docs/primitives.md`.
        Value::Map(mid) => {
            let (name, node) = crate::process::read_name_address(heap, mid)?;
            if crate::dist::is_local(node) {
                match crate::dist::whereis(name) {
                    Some(pid) => Ok(crate::process::monitor(pid)),
                    // Unregistered name: behave as if the pid were already
                    // dead — fire :noproc immediately. `process::monitor`
                    // already does this for an unknown local pid, so route
                    // through it with a fresh-but-dead id placeholder.
                    None => Ok(crate::process::monitor(u64::MAX)),
                }
            } else {
                Err(LispError::type_err(
                    "monitor: remote {:name :node} addresses aren't resolvable for monitor — pass the pid",
                ))
            }
        }
        _ => Err(LispError::type_err(
            "monitor: first argument must be a pid or a {:name :node} address",
        )),
    }
}

/// `(demonitor mref)` — drop the monitor created by `(monitor …)`. Tries the
/// local table first; if the mref isn't there it must have been on a remote
/// peer, so a `Frame::Demonitor` is fanned out to every connected peer that
/// holds a pending remote monitor with this watcher + mref.
fn demonitor(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Ref(n) => {
            // Local first (in-process MONITORS table).
            crate::process::demonitor(n);
            // Then ask any peer holding this mref to drop their watcher.
            // We scan PENDING_REMOTE for matching entries and `Demonitor` each
            // unique peer once. The same `process::drop_monitor` predicate the
            // local demonitor used is reused on the far side via the frame
            // handler.
            crate::process::demonitor_remote_fanout(n);
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err(
            "demonitor: argument must be a monitor ref",
        )),
    }
}

/// `(%receive matcher timeout on-timeout)` — the selective-receive primitive the
/// `receive` macro (`std/prelude.blsp`) expands to. See `crate::process::receive_match`.
fn receive_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive_match(heap, arg(args, 0), arg(args, 1), arg(args, 2))
}

fn self_pid(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(crate::process::pid_value(crate::process::self_pid()))
}

/// `(ref)` — a fresh, globally-unique reference token. Shares the runtime's ref
/// counter with `(monitor …)` so every ref is distinct.
fn make_ref(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Ref(crate::process::next_ref()))
}

// ----- distributed nodes -----------------------------------------------------

/// Coerce a node/name argument (a keyword or symbol) to its interned `Symbol`.
/// Goes through the same `wrong_type` formatter as the other `expect_*`
/// helpers — pre-fix this one used `type_err` and lost the offending value
/// from the message, the one expect-family inconsistency the review flagged.
fn expect_node_name(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "keyword or symbol",
        Value::Keyword(s) => s,
        Value::Sym(s) => s,
    )
}

/// `(node-start name "host:port" cookie)` — name this runtime and listen for peer
/// nodes. Returns the node name.
fn node_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "node-start", arg(args, 0))?;
    let addr = expect_string(heap, "node-start", arg(args, 1))?;
    let cookie = expect_string(heap, "node-start", arg(args, 2))?;
    crate::dist::node_start(name, &addr, cookie).map_err(|e| {
        LispError::runtime(format!("node-start: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::Keyword(name))
}

/// `(connect "name@host:port")` — link to a peer node (cookie-authenticated).
/// Returns the peer's node name on success.
fn connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let spec = expect_string(heap, "connect", arg(args, 0))?;
    let peer = crate::dist::connect(&spec).map_err(|e| {
        LispError::runtime(format!("connect: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::Keyword(peer))
}

/// `(register name pid)` — bind a local name so peers can address this process by
/// `{:name name :node this-node}` before they hold its pid. Returns the pid.
fn register_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "register", arg(args, 0))?;
    match arg(args, 1) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::dist::register(name, id);
            Ok(Value::Pid { node, id })
        }
        Value::Pid { .. } => Err(LispError::type_err(
            "register: can only register a local pid",
        )),
        _ => Err(LispError::type_err(
            "register: second argument must be a pid",
        )),
    }
}

/// `(node-name)` — this runtime's node name (`:nonode` until `node-start`).
fn node_name(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Keyword(crate::dist::local_node()))
}

/// `(whereis name)` — the **local** pid registered under `name`, or `nil`.
/// Lets idempotent bootstrap shapes test for "is this server already running
/// here?" before re-`spawn`ing — see `remote-spawn` in `std/prelude.blsp`.
/// A remote-side registration isn't visible here; this is a strictly local
/// lookup over the `NAMES` table.
fn whereis_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "whereis", arg(args, 0))?;
    match crate::dist::whereis(name) {
        Some(id) => Ok(Value::Pid {
            node: crate::dist::local_node(),
            id,
        }),
        None => Ok(Value::Nil),
    }
}

/// `(monitor-node name)` — the calling process is sent `[:nodedown name]` when a
/// link to `name` goes down (heartbeat timeout or clean close). Returns the name.
fn monitor_node(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "monitor-node", arg(args, 0))?;
    crate::dist::monitor_node(name, crate::process::self_pid());
    Ok(Value::Keyword(name))
}

/// `(nodes)` — a list of currently connected peer node names.
fn nodes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let names: Vec<Value> = crate::dist::connected_nodes()
        .into_iter()
        .map(Value::Keyword)
        .collect();
    Ok(heap.list(names))
}

/// `(spawn-count)` — how many green processes have been spawned since the program
/// started. (Green processes are cheap coroutines, not OS threads — step 4b.)
fn spawn_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::spawn_count() as i64))
}

/// `(peak-threads)` — high-water mark of processes running *simultaneously*
/// (bounded by the worker-pool size); how much parallelism was actually reached.
fn peak_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::peak_threads() as i64))
}

/// `(worker-threads)` — size of the scheduler's worker-thread pool that runs the
/// green processes (≈ `nproc`, or the `-j` setting); 0 until the first spawn.
fn worker_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::worker_threads() as i64))
}

/// `(list-processes)` — every currently-live local pid as a `Pid` value
/// (carrying this runtime's node identity, so the list is `send`-routable as
/// returned). Order is unspecified; sort by `.id` if you need stability.
/// Used by agents / the `nest mcp` `processes` tool to enumerate what's been
/// spawned in the session.
fn list_processes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = crate::process::list_local_pids()
        .into_iter()
        .map(crate::process::pid_value)
        .collect();
    Ok(heap.list(items))
}

/// `(%isolate thunk)` — call `thunk` (no args) with a *private copy* of the
/// runtime's global bindings: any `def` it makes is rolled back when it
/// returns, so it cannot affect other code. The test framework wraps each
/// `:isolated` test in this so a test's definitions never leak to another test.
/// Restores the bindings even if the thunk raises (the error then propagates).
///
/// This only isolates *bindings* — the shared code slabs and the symbol interner
/// still grow (memory, not behaviour; there's no GC yet) — and it is sound only
/// with no other process mutating globals concurrently, which the runner ensures
/// by running isolated tests alone.
fn isolate(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let saved = heap.snapshot_globals();
    let result = apply(heap, thunk, &[], env);
    heap.restore_globals(saved);
    result
}

fn try_catch(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let handler = arg(args, 1);
    // The thunk runs through `apply`, which can collect at ANY eval depth
    // (ADR-061). On the error path we still need `handler` and `env` afterwards,
    // so root them on the operand stack across the thunk and re-read the
    // relocated handles. (The thrown value / built error map is fresh after the
    // unwind — no safepoint runs while an `Err` propagates — so it needs no
    // rooting.) This is the `(try (loop) (catch e …))` supervised-server shape.
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_root(handler);
    heap.push_env_root(env);
    let outcome = apply(heap, thunk, &[], env);
    let handler = heap.root_at(vb);
    let env = heap.env_root_at(eb);
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    match outcome {
        Ok(value) => Ok(value),
        Err(e) => {
            // The catch sees:
            //   * the user-thrown value verbatim, if there is one (preserves the
            //     "throw shape == catch shape" contract — `(throw 42)` → 42);
            //   * **a structured map** for any built-in error, so Brood code (and
            //     agents via MCP) can `(case (get e :kind) :unbound …)` without
            //     parsing strings (`docs/llm-native.md` §4). Shape on
            //     `LispError::to_value_map`: `{:kind :message [:code] [:file
            //     :line :col] [:hint]}`.
            let caught = match e.payload {
                Some(v) => v,
                None => e.to_value_map(heap),
            };
            apply(heap, handler, &[caught], env)
        }
    }
}

// ----- dynamic variables -----------------------------------------------------
//
// The kernel for `defdyn`/`binding`; the surface macros are in the prelude. A
// dynamic variable's *value* resolves through the per-process binding stack in
// the `Heap` (see `Heap::env_get`), so reads need no primitive here — only the
// declaration, the scoped rebind, and the predicate.

/// `(%declare-dynamic 'name)` — mark a symbol as a dynamic variable, so
/// `binding` will accept it (and `dynamic?` reports it). `defdyn` expands to
/// this plus a plain `def` of the default value.
fn declare_dynamic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%declare-dynamic", arg(args, 0))?;
    value::mark_dynamic(sym);
    Ok(Value::Sym(sym))
}

/// `(%in-ns 'foo)` — set the namespace being compiled into (ADR-065). Emitted by
/// the `ns` macro; the resolver pass qualifies subsequent definitions and free
/// references to `foo/…`. Returns the namespace symbol.
fn in_ns(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%in-ns", arg(args, 0))?;
    heap.set_compile_ns(Some(sym));
    Ok(Value::Sym(sym))
}

/// `(current-ns)` — the namespace currently being compiled into (a symbol), or
/// `nil` at root. Reflection + a handle for tests (ADR-065).
fn current_ns(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.compile_ns().map(Value::Sym).unwrap_or(Value::Nil))
}

/// `(%refer 'mod subset)` — add `(:use …)` imports to the current file's import
/// table (ADR-065 inc-2). `mod` must already be loaded (the `ns` macro emits a
/// `(require 'mod)` first). `subset` nil → refer every *public* `mod/name` (no
/// `--` private marker, not itself nested); else a seq of bare symbols → refer
/// just those as `mod/name`. Each becomes a bare → qualified entry the resolver
/// consults after the current namespace and before root.
fn refer(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mod_sym = expect_symbol(heap, "%refer", arg(args, 0))?;
    let mod_name = value::symbol_name(mod_sym);
    let prefix = format!("{}/", mod_name);
    match arg(args, 1) {
        Value::Nil => {
            // Refer all public names: enumerate the live globals under `mod/`.
            for g in heap.global_symbols() {
                let name = value::symbol_name(g);
                if let Some(bare) = name.strip_prefix(&prefix) {
                    if !bare.is_empty() && !bare.contains('/') && !bare.contains("--") {
                        let bare_sym = value::intern(bare);
                        heap.add_import(bare_sym, g);
                    }
                }
            }
        }
        subset => {
            // Refer just the named symbols as `mod/name` (existence not required —
            // an unbound `mod/name` surfaces as a normal unbound-reference error).
            for item in heap.seq_items(subset)? {
                let bare = expect_symbol(heap, "%refer", item)?;
                let qualified = value::intern(&format!("{}/{}", mod_name, value::symbol_name(bare)));
                heap.add_import(bare, qualified);
            }
        }
    }
    Ok(Value::Nil)
}

/// `(dynamic? x)` — true when `x` is a symbol declared dynamic with `defdyn`.
/// A non-symbol is simply not dynamic (no error), so it composes in predicates.
fn dynamic_p(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(
        matches!(arg(args, 0), Value::Sym(s) if value::is_dynamic(s)),
    ))
}

/// `(%binding syms vals thunk)` — run `thunk` (no args) with each dynamic var in
/// `syms` bound to the matching value in `vals` for the dynamic extent of the
/// call, restoring the previous bindings on return *or* error. `syms` (a quoted
/// list) and `vals` (a vector) are equal-length sequences built by the `binding`
/// macro — both emitted as unshadowable literals, so a local rebinding of `list`
/// can't break the form. Every name must be declared dynamic (else it's almost
/// certainly a typo — a plain global won't track the rebind). The bindings live
/// in this process's heap, so they don't reach a `spawn`ed child.
fn binding(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let syms = heap.seq_items(arg(args, 0))?;
    let vals = heap.seq_items(arg(args, 1))?;
    let thunk = arg(args, 2);
    // Validate every name up front, before pushing anything — so a bad `binding`
    // leaves the dynamic stack untouched rather than half-pushed.
    let mut names = Vec::with_capacity(syms.len());
    for s in &syms {
        let sym = expect_symbol(heap, "binding", *s)?;
        if !value::is_dynamic(sym) {
            return Err(LispError::runtime(format!(
                "binding: {} is not a dynamic variable (declare it with defdyn)",
                value::symbol_name(sym)
            )));
        }
        names.push(sym);
    }
    for (i, &sym) in names.iter().enumerate() {
        heap.push_dynamic(sym, arg(&vals, i));
    }
    let result = apply(heap, thunk, &[], env);
    for _ in 0..names.len() {
        heap.pop_dynamic();
    }
    result
}

#[cfg(test)]
mod gui_face_tests {
    use super::gui_face;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    // `gui_face` is the seam between a Brood face map and the GUI backend; verify it
    // reads the per-section font keys (`:family`/`:italic`) + flags. No window needed.
    #[test]
    fn reads_family_italic_and_flags() {
        let mut heap = Heap::new();
        let mono = value::intern("mono");
        let face = heap.map_from_pairs(vec![
            (value::kw("fg"), value::kw("red")),
            (value::kw("bold"), Value::Bool(true)),
            (value::kw("italic"), Value::Bool(true)),
            (value::kw("underline"), Value::Bool(true)),
            (value::kw("family"), Value::Keyword(mono)),
        ]);
        let f = gui_face(&heap, face);
        assert_eq!(f.fg, Some([0xcd, 0x31, 0x31]));
        assert!(f.bold);
        assert!(f.italic);
        assert!(f.underline);
        assert_eq!(f.family, Some(mono));
    }

    // A non-map (or nil) face is the default face: no colours, no flags, no family.
    #[test]
    fn non_map_face_is_default() {
        let heap = Heap::new();
        let f = gui_face(&heap, Value::Nil);
        assert!(f.fg.is_none());
        assert!(!f.bold && !f.italic && !f.underline && !f.reverse);
        assert!(f.family.is_none());
    }
}
