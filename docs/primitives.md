# Native primitive kernel

The **complete set of functions implemented in Rust** (every `Value::Native`
registered in `crates/lisp/src/builtins/`, split by domain). Everything else in the language —
`+ - * / < = map filter reduce defn -> …` — is written *in Brood*
(`std/prelude.blsp`) on top of these. Keeping this list small is a deliberate,
load-bearing choice (ADR-006 "write the language in the language", ADR-008
"Rust is a primitive kernel").

`%`-prefixed names are low-level primitives not meant to be called directly.

The **Arity** column below is now machine-enforced: each builtin declares an
`Arity` (`value.rs`) and the evaluator checks it once, at the single native call
gate (`eval::call_native`), before the primitive runs — so a wrong-count call is
a clean arity error (`type-of: expected 1 argument, got 0`) rather than a missing
arg silently becoming `nil`.

## Native primitive functions

| Category | Primitive | Arity | Purpose |
|---|---|---|---|
| **Numeric** (arithmetic substrate) | `%add` `%sub` `%mul` `%div` | 2 | int-preserving arithmetic; `%div` is exact-int-or-float and errors on ÷0 |
| | `%lt` | 2 | numeric `<` → bool |
| | `%eq` | 2 | structural equality → bool |
| | `rem` | 2 | integer remainder (truncated, sign of dividend) — **irreducible**: deriving it via float division would lose precision past 2^53. `mod` (euclidean) and `quot` (truncated division) are Brood over it |
| | `floor` | 1 | floor toward −∞ → **int** (an int passes through) — the one Float→Int crossing the language can't bootstrap. `ceil`/`round`/`sqrt`/`pow` are Brood over it |
| **Pair / sequence** | `cons` | 2 | make a pair |
| | `first` `rest` | 1 | head / tail (nil, pair, or vector) — these *are* car/cdr; `empty?` is Brood over them + the length primitives |
| | `range?` | 1 | True if x is a lazy range (as produced by range). Ranges fold/reduce/sum/count without materialising; other ops treat them as the list they stand for. |
| | `seqview?` | 1 | True if x is a lazy sequence view — the reducible produced by range/map/filter/… before it is realized (into/count/…). |
| **Vector** (data type, O(1)) | `vector` | n | construct a vector |
| | `vector-ref` | 2 | index |
| | `vector-length` | 1 | length |
| | `vector-assoc` | 3 | a fresh vector with index `i` (in `[0, len)`) replaced — the vector counterpart of `map-assoc`; the polymorphic `assoc`/`update` are Brood over it |
| | `subvec` | 2–3 | a fresh vector slice `[start, end)`; `end` defaults to the length — the vector-preserving counterpart of the list-returning `take`/`drop`; `remove-nth` is Brood over it |
| **Ordering** | `compare` | 2 | `(compare a b)` → `-1`/`0`/`1` by the structural total order (numbers numerically; strings/keywords/symbols by text; vectors/lists lexicographically; cross-kind by stable tag rank). The binary form of `sort`'s order — `sort-by` and custom comparators build on it |
| **Map** (immutable; data type) | `hash-map` | n | construct a map from `k v k v …` args (the `{ }` literal's programmatic form); last-wins on dup keys |
| | `map-get` | 2–3 | value at a key, or the optional default (else nil) |
| | `map-assoc` | 3 | a fresh map with `key`→`val` added/updated |
| | `map-dissoc` | 2 | a fresh map with a key removed |
| | `map-pairs` | 1 | entries as a list of `[k v]` vectors, insertion order, one O(n) pass — the sole enumerator; `keys`/`vals`/`contains?`/`reduce-kv` are all Brood over it |
| | `map-count` | 1 | the number of entries in a map — O(1) (the CHAMP root tracks its size) |
| | `map-int-add` | 3 | `(map-int-add m k delta)` → a fresh map with key `k`'s integer value incremented by `delta` (inserts `delta` when `k` is absent) — a single trie traversal, equivalent to `(assoc m k (+ (get m k 0) delta))` without the extra walk |
| **String** | `string-length` | 1 | char count |
| | `substring` | 2-3 | characters `[start, end)`, char-indexed; `end` defaults to `(string-length s)` |
| | `%str-index-of` | 2-3 | char index of the first occurrence of a substring at or after the optional start (or -1; empty needle → the start). Linear (byte-level `find` → char index) — the search counterpart of `substring`, needed in Rust because Brood has no O(1) char access (a pure-Brood scan is O(n²)). `index-of` / `includes?` ride on it. The start offset is taken here rather than by slicing in Brood: `(index-of s needle from)` used to search `(substring s from n)`, copying the suffix on every call |
| | `%str-last-index-of` | 2-3 | char index of the **last** occurrence starting strictly before the optional `before` bound (default end; -1 if none). One forward pass with an advancing cursor. Backs `last-index-of`, which used to walk forward in Brood calling `index-of` per match — O(matches × length), and on an editor hot path both ways (reverse buffer search; finding the current line's start per keystroke) |
| | `upper` | 1 | `s` upper-cased (Unicode-aware, e.g. `ß` → `SS`) |
| | `lower` | 1 | `s` lower-cased (Unicode-aware) |
| | `string->number` | 1 | strict parse → int, else float, else `nil` (`"3abc"` → `nil`, unlike `read-string`) |
| | `string->codepoints` | 1 | the characters of `s` as a vector of integer Unicode codepoints, one O(n) pass — the random-access form text parsers index with `nth` and compare as ints (`codepoints->string` is its Brood inverse) |
| | `string-span` | 3 | `(string-span s start chars)` → the char index just past the maximal run of chars in the set `chars` (a string) starting at char `start` — `start` itself if the char there isn't in the set. The forward char-class scan a tokenizer skips a whitespace/digit run with; O(run) native |
| | `string-span-until` | 3 | the char index of the first char of `s` in the set `chars` at or after `start`, or `(string-length s)` if none — the maximal run of chars *not* in the set, for scanning up to a delimiter. The complement of `string-span` |
| | `string-split` | 2 | Split s into a list of substrings on each occurrence of sep, in one O(n) pass. An empty separator splits s into its individual characters. |
| **Bytes** (immutable byte sequences; `crates/lisp/src/builtins/bytes.rs`) | `byte-at` | 2 | `(byte-at b i)` → the byte at index `i` as an int 0–255 (out of range errors) |
| | `subbytes` | 2–3 | the byte slice `[start, end)` (`end` defaults to the length) as a fresh bytes value |
| | `bytes-index-of` | 2–3 | the first index of the `needle` bytes in `haystack` at or after `from` (default 0), or -1 if not present — the byte-protocol workhorse (find a `\r\n\r\n`, a frame delimiter, …) |
| | `byte-length` | 1 | The number of bytes in b. O(1). |
| | `bytes-concat` | any | One bytes value joining all arguments, each an iolist (ADR-139): a string (UTF-8), a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those. The in-memory materialiser of the iolist model. |
| | `bytes->list` | 1 | The bytes b as a list of integers 0–255. |
| **Table** (Brood's ETS — the one identity-mutable structure; shared by identity, deep-clones in/out) | `table` | 0 | create a new empty in-memory table: a shared, mutable key→value store behind an opaque handle — mutated in place and shared by identity (the handle can be sent to other processes, which all see the same store); keys/values are deep-cloned in and out, so no two processes alias stored data. Local to this runtime; returns the handle |
| | `table-put` | 3 | store `v` under key `k` (overwriting; structural key equality); returns `t` for threading |
| | `table-get` | 2–3 | a fresh copy of the value stored under `k`, or the default (nil if omitted) when absent |
| | `table-has?` | 2 | true if the table has an entry for key `k` |
| | `table-delete` | 2 | remove key `k` if present; returns `t` |
| | `table-incr` | 2–3 | atomically add `delta` (default 1) to the integer at key `k` (absent → 0) and return the new value — the read-modify-write is atomic under the table lock, so concurrent increments never lose an update; errors if the existing value is not an integer |
| | `table-count` | 1 | the number of entries |
| | `table-snapshot` | 1 | a consistent point-in-time copy of the whole table as an immutable map — atomic, O(n), unaffected by later mutation |
| | `table-drop` | 1 | remove the table from the registry, freeing its store; idempotent, returns true if it existed (other handles then error on use) |
| **Rope** (editor buffer text; immutable, char-indexed — ADR-045) | `string->rope` | 1 | a rope holding the characters of a string — the constructor |
| | `rope->string` | 1 | the full text of a rope as a string (the only way a rope's content crosses a process: ropes are process-local) |
| | `rope-length` | 1 | character count |
| | `rope-line-count` | 1 | line count (a trailing newline ends a line; `""` is 1 line) |
| | `rope-insert` | 3 | `(rope-insert r idx s)` → a **fresh** rope with `s` inserted at char `idx` |
| | `rope-delete` | 3 | `(rope-delete r start end)` → a **fresh** rope with chars `[start, end)` removed |
| | `rope-slice` | 3 | text of chars `[start, end)` as a string |
| | `rope-line` | 2 | text of line `n` (0-based), including its trailing newline — the viewport primitive |
| | `rope-char->line` | 2 | 0-based line index containing a char index |
| | `rope-line->char` | 2 | char index where a 0-based line begins |
| **Terminal** (the editor/display/input seam — ADR-046; the in-process crossterm frontend that paints the render-op protocol, `std/editor/display.blsp`) | `term-enter` | 0 | take over the terminal: raw mode + alternate screen, cursor hidden → nil. Pair with `term-leave` |
| | `term-leave` | 0 | restore the terminal (show cursor, leave alternate screen, disable raw mode) → nil |
| | `term-size` | 0 | terminal size as `[cols rows]` (character cells) |
| | `term-poll` | 1 | `(term-poll ms)` → wait up to `ms` ms for a key: a 1-char string (printable), a keyword for specials (`:up` `:down` `:left` `:right` `:enter` `:escape` `:backspace` `:tab` `:back-tab` `:delete` `:home` `:end` `:page-up` `:page-down`, ctrl combos like `:ctrl-c`, alt combos like `:alt-f`), or `nil` on timeout. Always pass a finite `ms`. **Enter caveat:** `:enter` is the named-key event, but a raw CR/LF byte (a pty, CRLF translation, or piped input) arrives as `:ctrl-m` (CR `0x0d`) / `:ctrl-j` (LF `0x0a`) — a line editor should treat all three as "submit" |
| | `term-draw` | 1 | paint a **frame** — a vector of render ops `[:clear]` / `[:text row col s]` / `[:text row col s face]` / `[:cursor row col]`, where a face is a map like `{:fg :red :bold true}` → nil. The frontend that interprets the display protocol |
| | `term-raw-enter` | 0 | enter raw mode **only** — no alternate screen, cursor stays visible, scrollback preserved → nil. The seam for an *inline* editor (the REPL, `std/editor/lineedit.blsp`); use `term-enter` for a full-screen TUI. Pair with `term-raw-leave` |
| | `term-raw-leave` | 0 | leave raw mode (teardown for `term-raw-enter`) → nil |
| | `term-emit` | 1 | paint inline, **relative**-motion ops: `[:print str]` / `[:print str face]` / `[:cr]` / `[:nl]` / `[:up n]` / `[:down n]` / `[:col n]` / `[:clear-eol]` / `[:clear-below]` / `[:clear-screen]` → nil. The inline counterpart to `term-draw` (which is absolute); queues all ops then flushes once |
| **Process introspection** | `mailbox-size` | 1 | `(mailbox-size pid)` → the number of messages queued in a live local process's mailbox (its receive backlog), or `nil` for a remote/dead pid. The one process-state accessor Brood can't reach (the queue lives behind the scheduler registry); `(list-processes)` + `self` are the others. Used by `std/tool/observer.blsp` |
| | `process-info` | 1 | `(process-info pid)` → an Erlang-`process_info`-style snapshot map of a live local process — `{:id :node :name :status :mailbox :monitored-by :parent :memory}` (`:status` `:running`/`:runnable`/`:waiting`; `:name` nil if unregistered; `:parent` the spawner's id, nil for the root; `:memory` LOCAL heap-footprint bytes, published on `receive` — 0 for a process that never receives) — or `nil` for a remote/dead pid. Assembled from registry/scheduler/monitor cells (ADR-051). The observer reads this |
| | `list-processes` | 0 | Every currently-live pid on this runtime (one per registered mailbox). Order is unspecified — sort if you need stability. For agents/tools enumerating spawned processes. |
| **Type reflection** | `type-of` | 1 | the runtime type tag as a keyword (`:int` `:string` …); the one irreducible reflective primitive. The tag predicates (`nil?` `pair?` `int?` `float?` `bool?` `string?` `symbol?` `keyword?` `vector?` `map?` `fn?`) are Brood wrappers over it, as are the in-language type checks |
| **Type checking** (advisory; see [types.md](types.md)) | `check` | 1 | run the advisory type checker over a *quoted* form: macro-expand it (like the real compile pass), then return a **list of warning strings** for provably-wrong primitive arguments (e.g. `(first 5)` → `"first: argument 1 expects nil \| pair \| vector, got int (5)"`), or `nil` when nothing is wrong. Advisory: never raises |
| | `check-file` | 1–2 | check every top-level form in the file at `path`, returning pre-formatted `"path:line:col: warning: …"` strings (or `nil` if clean). Reads but does **not** evaluate. Used by `(check-project)` for the `nest test` / `nest run` / `nest check` pre-flight |
| | `check-file-structured` | 1–2 | Like check-file but returns a list of `{:file :line :col :message}` maps instead of GNU-format strings — for tools (the `nest mcp` `check` tool, editor diagnostics). |
| | `check-string-structured` | 1 | Advisory type-check the source string `src`, returning a list of `{:line :col :message}` maps (1-based positions), or `()` when `src` doesn't parse (e.g. incomplete input) — the string-source counterpart of check-file-structured, for live editor-buffer diagnostics. |
| | `check-file-deps` | 1–2 | Incremental-cache check (ADR-119): returns [warnings dep-keys fingerprint] — the GNU warning strings, the set of global observations the check made, and a fingerprint of them against the current image. Store dep-keys+fingerprint; |
| | `check-deps-fp` | 1 | Recompute the fingerprint of a file's dep-keys (from check-file-deps) against the current global image. The incremental check cache reuses a file's warnings iff this equals the stored fingerprint. |
| **Value ↔ text & I/O** | `str` | n | concatenate the *display* forms of args → string |
| | `pr-str` | 1 | *readable* form of a value → string |
| | `print` | n | write display forms to stdout → nil (`println`, which adds a newline, is Brood over it) |
| | `eprint` | n | write display forms to **stderr** → nil (mirrors `print`; `eprintln` is the Brood newline-adding wrapper) |
| | `stdout-tty?` | 0 | true when stdout is an interactive terminal (false when piped/captured) — gates colour output |
| | `stdin-tty?` | 0 | true when stdin is an interactive terminal (false when redirected from a pipe/file) — the REPL gates raw-mode line editing on this |
| | `%render` | any | The space-joined display forms of the arguments as one string (no output). The rendering half of `print`; Brood's print/println route the result through the dynamic `*out*` port. |
| | `%write-out` | 1 | Write the ready string `s` to the current stdout sink — the active capture buffer (`with-out-str`) if set, else real stdout. The default `*out*` port. |
| | `%write-err` | 1 | Write the ready string `s` to real stderr (never captured). The default `*err*` port. |
| | `read-line` | 0 | Read one line from stdin; returns the line as a string (trailing newline stripped) or nil at end of input. |
| | `read-all` | 1 | Parse every form in string s and return them as a list (the all-forms sibling of read-string). |
| | `read-first` | 1 | Parse and return the first form in string s, ignoring any trailing forms (the lenient sibling of read-string — for peeking a multi-form source's leading form, e.g. a file's (defmodule …) header). |
| **Time** | `now` | 0 | wall-clock milliseconds since the Unix epoch (integer); subtract two readings for elapsed time |
| | `now-ns` | 0 | Wall-clock nanoseconds since the Unix epoch (finer-grained than now). |
| **Memory** | `mem-bytes` | 0 | bytes currently allocated process-wide (from the counting global allocator) |
| | `mem-peak` | 0 | high-water mark of allocated bytes since process start |
| **Self-hosting hooks** | `eval` | 1 | evaluate a form in the global env |
| | `read-string` | 1 | parse one form from text |
| | `eval-string` | 1 | read + evaluate every form in a string (string analogue of `load`) |
| | `load` | 1 | read + evaluate a file |
| | `%builtin-module` | 1 | source of a baked-in std module by name, or nil (used by Brood `require`) |
| | `apply` | ≥2 | call a function with a spliced argument list |
| | `%run-program-file` | 1 | Run the program file at `path` as its own green process (ADR-135) and block until it finishes; nil, or raises if a top-level form did. |
| | `reload-defs` | 1 | Re-evaluate only the def-style top-level forms in `path` (def, defn, defmacro, defmodule, defdyn, …) — skipping other top-level calls. Used by file watchers to refresh code without re-running side-effecting top-level calls like a `(main-loop)`. Returns nil. |
| | `%offload` | 2 | Run the blocking native `f` with `args` (a vector) on the dirty-offload OS pool (ADR-144) instead of this process's scheduler worker. Returns a token int immediately; the pool later delivers [:offload token result] or [:offload-error token err] to the calling process's mailbox. |
| **Symbols** | `name` | 1 | a symbol/keyword's spelling as a string (no leading `:`) |
| | `symbol` | 1 | coerce a string / symbol / keyword to the matching symbol (intern as needed). Lenient inverse of `name`; strict `string->symbol` is a Brood wrapper |
| | `keyword` | 1 | coerce a string / symbol / keyword to the matching keyword (intern as needed). Mirrors `symbol`; they share an interner so `(= (name 'x) (name :x))` |
| **Filesystem** | `cwd` | 0 | current working directory |
| | `file-exists?` `dir?` | 1 | path exists / is a directory → bool |
| | `list-dir` | 1 | entry names directly under a directory (sorted) |
| | `make-dir` | 1 | create a directory and parents (`mkdir -p`) |
| | `spit` | 2 | write a string to a file (write-side of `load`) |
| | `slurp` | 1 | read a whole file into a string (read-side of `spit`; unlike `load`, does not evaluate) |
| | `file-mtime` | 1 | last-modified time as epoch-milliseconds, or nil if missing (cheap stat; pair with `load` for hot-reload) |
| | `file-stat` | 1 | one-stat metadata map `{:dir? :size :mtime :atime :symlink? :exec? :mode :nlink :uid :gid :owner :group}`, or nil if missing (collapses `dir?`+`file-size`+`file-mtime` for a directory lister + a recency sort) |
| | `slurp-bytes` | 1 | Read the whole file at path as a bytes value. The byte-faithful read slurp can't be (slurp is UTF-8 and throws on a non-text file). Pairs with hash/sha256-bytes / hash/sha256-raw and the encoding byte variants — e.g. hashing a binary asset. |
| | `spit-bytes` | 2 | Write any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) to path byte-faithfully, replacing any existing file. Returns nil. |
| | `append-bytes` | 2 | Append any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) to the file at path byte-faithfully, creating it if absent. Returns nil. |
| | `spit-append` | 2 | Append s (any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139)) to the file at path, creating it if absent (unlike spit, which truncates). Returns nil. |
| | `spit-private` | 2 | Write string s to path with owner-only (0600) permissions, creating the parent dir if needed. The private-by-default write for a secret (spit leaves a world-readable file). |
| | `%file-swap` | 4 | Replace the entire contents of data-path with new, but ONLY if they currently equal expected; returns true when swapped, false when they differ (re-read, recompute, retry). |
| | `canonicalize` | 1 | The real absolute path of `path` with symlinks and ./.. resolved. Works for a not-yet-existing target (the longest existing ancestor is resolved, then the remaining components appended). Relative paths are taken against the cwd. nil only if the cwd itself can't be read. |
| | `copy-file` | 2 | Copy file `from` to `to` (replacing `to`), preserving contents and permissions. Binary-safe (unlike slurp+spit). Returns nil; errors on failure. |
| | `rename-file` | 2 | Rename/move file `from` to `to`. Returns nil; errors on failure. |
| | `delete-file` | 1 | Remove the file at path. Idempotent (nil if already absent); errors on a real I/O failure. |
| | `delete-dir` | 1 | Remove a directory and everything under it (recursive). Idempotent (nil if already absent); errors on a real I/O failure. |
| **System** | `getenv` | 1 | environment-variable value, or nil if unset |
| | `run-process` | 2 | run an external program (`prog`, args list), inherit stdio → exit code |
| | `hostname` | 0 | This machine's short hostname (no domain). Used to qualify a node name as name@host. |
| | `%env-all` | 0 | All environment variables as a map of string→string. |
| | `%argv` | 0 | Command-line arguments as a vector of strings (including argv[0]). |
| | `%os-type` | 0 | The host OS as a keyword: :linux, :macos, or :windows. |
| | `%os-cmd` | 1+ | Run prog (with optional args list) capturing stdout/stderr; returns {:stdout s :stderr s :exit n}. |
| | `%halt` | 1 | Terminate the process with exit code. Never returns. |
| | `image-thumb` | 3 | Decode an encoded image (PNG/JPEG/GIF/WebP/BMP) from a byte sequence and downscale it to fit within max-w×max-h pixels (aspect ratio preserved), returning {:width :height :rgba} where :rgba is a width*height*4 bytes value (row-major RGBA8). |
| **Macro support** | `macroexpand-1` `macroexpand` | 1 | expand a form (one step / fully) |
| | `gensym` | 0–1 | a fresh, unique symbol (optional name prefix) |
| **Source positions** (editor tooling) | `form-pos` | 1 | a form's `[line col]` source position vector, or nil |
| | `current-file` | 0 | path of the file currently being loaded, or nil |
| | `source-location` | 1 | `[file line col]` of where `'name` was defined (`def`/`defn`/`defmacro`/`defdyn` site), or nil. Captured pre-expansion so macros' surface forms are located accurately (ADR-031) |
| | `parse-source` | 1 | parse a `.blsp` source string into a span-carrying CST node (`Atom`/`Cst`); the formatter and LSP read structure + positions from this rather than re-reading source. ADR-025 |
| | `parse-source-positioned` | 1 | parse a source string into a CST of **maps** — `{:kind :start :end}` (leaves add `:text`, containers/wrappers add `:kids`) with half-open **character** offsets; backs structural navigation (`std/sexp`) without re-deriving positions in Brood. ADR-045 |
| | `tree-sitter-parse` | 2 | parse `source` with the tree-sitter grammar named by keyword `lang` (`:ruby`/`:elixir`) into the **same** positioned-CST map shape as `parse-source-positioned` (`:kind` a node-type keyword, `:named` false for anonymous tokens) — so `std/sexp` + the editor's `:fontify` run over a foreign tree unchanged. Error-recovery nodes additionally carry `:error true` (an `ERROR` node) or `:missing true` (a zero-width inserted token), so a fontifier can draw diagnostics. Feature `treesit`; errors otherwise. ROADMAP §C |
| | `tree-sitter-reparse` | 3 | incremental `tree-sitter-parse` keyed by integer buffer id `key`: caches the last `(source, tree)` and re-uses it (deriving the edit by diffing the old source) so only the changed region is re-scanned. **Same** positioned CST as `tree-sitter-parse` — incrementality is a pure optimization. Feature `treesit`. ROADMAP §C |
| | `tree-sitter-forget` | 1 | drop every cached incremental tree for integer buffer id `key`; returns the count dropped. Call when a buffer closes so the reparse cache stays bounded. Feature `treesit`. |
| | `scan-tokens` | 1 | Lexically tokenize Brood source s into a vector of [start end kind text] tokens (char offsets, end-exclusive; whitespace skipped). kind is :comment, :string, :number, :keyword, :symbol, :open, or :close. |
| | `scan-form-start` | 2 | The greatest char offset <= pos of a column-0 open bracket in s lying OUTSIDE any string or ; comment, else 0 — the string/comment-aware beginning-of-defun behind highlight/safe-restart and tool/sexp narrowing. |
| | `scan-source-extract` | 1 | Native per-file scan for the whole-project check (ADR-119): parse src and return [counts privs def-names] — a map of --containing symbol counts, this file's --private defs as [bare qual], and every top-level def's qualified name. The fast path replacing the interpreted CST walk. |
| | `span-runs` | 3–4 | Tile text (first char at offset base) into a list of [substring face] runs from ascending, non-overlapping [start end face] spans: gaps are nil-faced, each span its text in its face. |
| **Introspection** (editor tooling) | `doc` | 1 | a function/macro's docstring, or nil |
| | `arglist` | 1 | a function/macro's parameter list (required, `&optional`, `& rest`), or nil |
| | `global-names` | 0 | every globally bound symbol, sorted by spelling (completion / doc generation) |
| | `special-forms` | 0 | the special-form / core-macro names (strings) that read as keywords — the canonical list shared by the syntax highlighter (`std/editor/highlight.blsp`) and the LSP |
| | `bound?` | 1 | whether a symbol is bound in scope → bool |
| | `dynamic?` | 1 | whether a symbol names a dynamic variable (declared via `defdyn`) → bool |
| | `builtin-modules` | 0 | The names of every module baked into this binary, as a sorted list of strings — what `(require 'name)` resolves without a load-path. Backs `nest` shell completion and lets a name be validated before requiring it. |
| | `references-in-source` | 2 | Occurrences of the global `name` in `source`, as a list of [line col] (1-based); locals that shadow it are excluded. |
| | `build-id` | 0 | This brood build's identity as "<version>+<git-sha>+<binary-stamp>" (e.g. "0.1.0+dcab7ca+18f2e1a9b3c4d5e6") — the correct staleness stamp for an on-disk cache of anything the kernel computes. |
| **Errors / control** | `throw` | 1 | raise a value as an error (non-local exit) |
| | `%try` | 2 | call a thunk; on raise, call the handler with the caught value |
| | `%isolate` | 1 | call a thunk against a private copy of the globals; roll back its `def`s afterward (used by `:isolated` tests) |
| **Processes** | `%spawn` | 1 | run a **0-arg thunk** in a new green process; returns its pid. `spawn` is the prelude macro over it — `(spawn expr)` wraps `expr` in the thunk, and `(spawn name expr)` is the named/idempotent form (a live registration under `name` short-circuits and `expr` is never evaluated) |
| | `%spawn-link` | 1 | as `%spawn`, but the symmetric caller↔child link is registered **before the child is enqueued** (atomic spawn+link, ADR-067), so an instant exit still reports its true reason — a spawn-then-`link` can find the child already dead and reports `:noproc`, losing it. `spawn-link` is the macro over it |
| | `send` | 2 | copy a message into a pid's mailbox |
| | `%receive` | 4 | selective-receive primitive: matcher fn, timeout-ms-or-nil, clause **tag** vector-or-nil (the leading-keyword pre-filter, ADR-178), and the **pin** value-or-nil (the receive-mark hint — the ref every clause pins, when they all pin the same one, ADR-195). `receive` is a Brood macro over it, and derives both hints at expansion time. (The old entry said the third argument was an on-timeout thunk; it has been the tag filter for some time.) |
| | `self` | 0 | this process's pid |
| | `ref` | 0 | a fresh, globally-unique reference token (`Value::Ref`); tags request↔reply |
| | `monitor` | 1 | watch a pid (local or remote); returns a monitor ref. Delivers `[:down ref pid reason]` on death (`:noproc` if already dead; `:noconnection` if a remote peer's link drops) |
| | `demonitor` | 1 | drop a monitor by its ref (best-effort; remote demonitor is fanned out to the holding peer) |
| | `exit` | 2 | `(exit pid reason)` — send an exit signal to a local process (Erlang `exit/2`). `:kill` is the untrappable hard kill (dies at its next reduction tick, or now if parked); any other reason is the soft signal (dies at its next `receive`). Monitors fire `[:down ref pid reason]`. No-op for a dead/unknown pid (ADR-063) |
| | `register` | 2 | bind a local name → pid so peers can address it via `{:name n :node this-node}`. Returns the pid |
| | `whereis` | 1 | the local pid registered under `name`, or nil. Strictly local — does not query other nodes |
| | `spawn-count` | 0 | green processes spawned since program start |
| | `peak-threads` | 0 | high-water mark of spawned threads running concurrently (bounded by the CLI's `-j`) |
| | `worker-threads` | 0 | size of the scheduler's worker-thread pool (≈ nproc; `-j` overrides) |
| | `link` | 1 | Symmetrically link the current process and pid, local or remote (Erlang link/1). When either dies, the other gets a [:EXIT pid reason] message if it set (trap-exit true), else dies too on an abnormal reason (propagation cascades through links; :normal does not propagate). |
| | `unlink` | 1 | Drop the symmetric link between the current process and pid (local or remote; best-effort). Returns nil. |
| **Distributed nodes** ([docs](distribution.md), ADR-034) | `node-start` | 3 | name this runtime (`node`, `"host:port"`, `cookie`), start the acceptor; cookie is the HMAC key for handshake v2 (never on the wire). Returns the node name |
| | `connect` | 1 | dial `"name@host:port"`, complete the v2 handshake (magic+version, nonce-exchange, HMAC challenge-response). Returns the peer's node name |
| | `node-name` | 0 | this runtime's node name — a **keyword** like `:alice@host` (`:nonode` until `node-start`); `(str (node-name))` for string ops. `node-start`/`connect` likewise return keywords |
| | `nodes` | 0 | list of currently connected peer node names |
| | `monitor-node` | 1 | get `[:nodedown name]` when the link to node `name` drops (heartbeat timeout or clean close). Persistent — fires on each down |
| | `disconnect` | 1 | tear the link to peer node `name` down now, **without exiting this process** (Erlang's `disconnect_node`); fires `[:nodedown name]` on both sides and prunes `name` from `(nodes)`. Returns `true` if a link existed, `false` otherwise. The clean way to leave a node/cluster while staying alive |
| | `demonitor-node` | 1 | Cancel this process's node monitor for node `name` (undo monitor-node); a no-op if none is registered. Returns nil. |

**Why this set is irreducible:** every entry needs Rust — raw number ops, heap
construct/inspect, the type-tag *reflection* (`type-of`), I/O, value→text
conversion, the wall clock, the allocator counters, the `Ty`-lattice checker
pass, or a hook into `eval`/the reader. None of it can be written in Brood. Everything that *can* be is already
in the prelude — including the tag predicates (over `type-of`), the full
arithmetic/comparison families `+ - * / < <= > >= = not=` (over `%add`/`%lt`/`%eq`),
the whole math library `mod`/`quot`/`ceil`/`round`/`pow`/`sqrt`/`even?`/`odd?` +
variadic `min`/`max` (over `rem`/`floor`/`/`/`*`/`<` — `sqrt` is Newton's method),
the whole sequence library
(`range`/`take`/`drop`/`take-while`/`drop-while`/`any?`/`every?`/`find`/`zip`/
`partition`/`sort`/`sort-by` — a Brood merge sort), `empty?` (type dispatch over
the length primitives), `println` (over `print`), and the map surface
`get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?`/`reduce-kv` (over `map-get`/
`map-assoc`/`map-dissoc`/`map-pairs`). Of the math library only **`floor`** (the Float→Int crossing) and
**`rem`** (exact integer remainder) need Rust — everything else is Brood over
them. The map literal `{ }` is read by the reader and evaluated like a vector
literal — no constructor call.

| **Bitwise** (integers, two's-complement) | `bit-and` | 2 | Bitwise AND of integers a and b. |
|  | `bit-or` | 2 | Bitwise (inclusive) OR of integers a and b. |
|  | `bit-xor` | 2 | Bitwise exclusive-OR of integers a and b. |
|  | `bit-not` | 1 | Bitwise complement of integer a (two's-complement, so (bit-not n) = (- (- n) 1)). |
|  | `bit-shift-left` | 2 | Shift integer a left by n bits (0 <= n < 64); bits shifted past bit 63 are discarded. |
|  | `bit-shift-right` | 2 | Arithmetic (sign-preserving) right shift of integer a by n bits (0 <= n < 64). |
|  | `bit-count` | 1 | Population count: the number of 1 bits in integer a's two's-complement representation (a negative a counts its sign bits, so (bit-count -1) = 64). For a bignum it is the popcount of the magnitude. |
|  | `bit-positions` | 1 | A vector of the 0-based bit indices set in non-negative integer a, ascending (e.g. (bit-positions 6) = [1 2]). O(number of set bits) — for a bignum it scans the magnitude. The inverse of summing (bit-shift-left 1 i); handy for enumerating the set bits of an integer. |
| **Float bit-level** | `float->bits` | 1 | The IEEE 754 binary64 bit pattern of x, as a non-negative integer (a bignum when the sign bit is set). Reinterpretation, not conversion — the only exact float comparison there is: it separates -0.0 from 0.0 and distinguishes NaN payloads, both of which = collapses. The inverse of bits->float. |
|  | `bits->float` | 1 | The binary64 float whose bit pattern is n (0 <= n < 2^64). The inverse of float->bits. |
|  | `%f64-sqrt` | 1 | The IEEE 754 square root of x (f64::sqrt). x must be non-negative; raises otherwise. Handles subnormals and ±0 correctly. |
| **Math** (transcendental — all return floats) | `sin` | 1 | The sine of x (radians). Returns a float. |
|  | `cos` | 1 | The cosine of x (radians). Returns a float. |
|  | `tan` | 1 | The tangent of x (radians). Returns a float. |
|  | `asin` | 1 | The arcsine of x in radians. x must be in [-1, 1]; raises otherwise. |
|  | `acos` | 1 | The arccosine of x in radians. x must be in [-1, 1]; raises otherwise. |
|  | `atan` | 1 | The arctangent of x in radians (result in [-π/2, π/2]). |
|  | `atan2` | 2 | The angle in radians of the vector (x, y) from the positive x-axis, in (-π, π]. Handles x=0. |
|  | `exp` | 1 | e raised to the power x. Returns a float. |
|  | `ln` | 1 | The natural logarithm of x. x must be positive; raises otherwise. |
|  | `log2` | 1 | The base-2 logarithm of x. x must be positive; raises otherwise. |
|  | `log10` | 1 | The base-10 logarithm of x. x must be positive; raises otherwise. |
| **Decimal** (exact base-10 for money — the `1.50M` literal) | `decimal` | 1 | Construct an exact arbitrary-precision base-10 decimal from x: a string ("1.50"), an int (3), a bignum, or a float (converted from its shortest round-trip form, since a float is inexact). For money / Postgres numeric — values a float can't hold exactly. The literal form is a trailing M, e.g. 1.50M. |
|  | `decimal->string` | 1 | The canonical decimal string of decimal d (no M suffix). |
|  | `decimal->float` | 1 | Decimal d as an (inexact) float. |
|  | `to-fixed` | 2 | Render number x as a string with exactly n digits after the decimal point (rounded). n must be >= 0. |
| **Ratio** (exact rational — the `1/2` literal; `/` on integers is exact, ADR-196) | `numerator` | 1 | The numerator of a ratio (`(numerator 3/4)` → 3), or an integer itself. |
|  | `denominator` | 1 | The positive denominator of a ratio (`(denominator 3/4)` → 4), or 1 for an integer. |
|  | `->decimal` | 1 | A number as an exact base-10 decimal — exact for an integer or terminating ratio (`1/2` → `0.5M`); a non-terminating ratio rounds to the default precision. (`->float`, `ratio?`, and `rational` are prelude functions.) |
| **Set** (`#{…}`; CHAMP-backed. `%`-internal — `std/set.blsp` is the library) | `%set` | any | Build a set from the element args (the programmatic form of the `#{ }` literal). Dedups by structural equality. The `set` library's constructor is Brood over this. |
|  | `%set-add` | 2 | A fresh set like s with element x added (a set already holding x is returned unchanged). O(log n). |
|  | `%set-remove` | 2 | A fresh set like s with element x removed (absent → unchanged). O(log n). |
|  | `%set-has?` | 2 | Is x an element of set s? O(log n). |
|  | `%set-count` | 1 | The number of elements in set s. O(1) — the CHAMP root tracks its size. |
| **Unicode / text width** | `display-width` | 1 | How many terminal/grid cells string s occupies (grapheme-cluster aware: an emoji / flag / CJK char counts as 2, a combining mark 0). The width-aware counterpart to string-length. |
|  | `string-normalize` | 2 | s in Unicode normalization form, one of :nfc :nfd :nfkc :nfkd. Brood's = is byte-structural, so text that reads identically ('é' as U+00E9 vs U+0065 U+0301) compares unequal until normalized. Canonical (:nfc/:nfd) preserves meaning; |
|  | `char->int` | 1 | Unicode codepoint of the first character of string s (identical to the byte value for ASCII). |
|  | `int->char` | 1 | A 1-char string for Unicode codepoint n. Errors on an invalid codepoint. |
|  | `string->utf8-bytes` | 1 | The UTF-8 encoding of s as a bytes value. |
|  | `utf8-bytes->string` | 1 | Decode UTF-8 bytes (a bytes value, vector, or list of ints 0–255) into a string. Errors on invalid UTF-8. |
| **Networking** (thin non-blocking socket mechanism, ADR-062; `std/net/*` is the Brood library) | `tcp-listen` | 2 | Bind a listening socket on host:port (port 0 = OS-assigned); connections arrive as [:tcp-accept lsock client] messages to the calling process. Returns a socket. |
|  | `tcp-connect` | 2 | Connect to host:port; inbound data is delivered to the calling process as [:tcp sock data] / [:tcp-closed sock] messages. Returns a socket. Throws on failure. |
|  | `tcp-send` | 2 | Write data to sock (blocking). data is any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139). A string leaf is always sent as its UTF-8 bytes, whatever the socket's mode (ADR-141); |
|  | `tcp-close` | 1 | Close sock (a stream or listener), releasing its fd / stopping its accept loop. Idempotent; returns nil. |
|  | `tcp-local-port` | 1 | The local port sock is bound to, or nil. |
|  | `tcp-set-binary` | 2 | Switch sock's INBOUND decode between text mode (default) and binary mode; outbound tcp-send is unaffected (ADR-141). |
|  | `tcp-set-idle-timeout` | 2 | Arm (or, with ms 0, disarm) an idle timeout on an established stream: the reactor drops the connection if no bytes move in EITHER direction for ms milliseconds, delivering [:tcp-closed] (or [:tcp-error] for a one-shot TLS client). |
|  | `tcp-controlling-process` | 2 | Make pid the owner of sock's inbound data: starts reading a just-accepted (passive) socket, or retargets an active one. Returns nil. |
|  | `tls-listen` | 4 | Bind a TLS listening socket on host:port using the PEM certificate chain cert-pem and private key key-pem (port 0 = OS-assigned). Like tcp-listen, connections arrive as [:tcp-accept lsock client]; |
|  | `tls-request` | 3–4 | Make one HTTPS request to host:port (TLS): the response arrives at the calling process as [:tcp sock data] … [:tcp-closed sock] messages (or [:tcp-error sock msg]). request is any iolist (a string, bytes, or nested tree — ADR-141); the socket honors tcp-set-binary for the response. |
|  | `tls-self-signed` | 1 | Generate a self-signed TLS certificate + private key for host (a DNS name like "localhost"), for zero-config dev TLS. Returns [cert-pem key-pem] — pass them to tls-listen. Not for production (clients reject a self-signed cert unless told to trust it). |
| **Subprocess** (a persistent child OS process, ADR-104 — distinct from green processes) | `proc-spawn` | 2–3 | Spawn prog (a string) with args (a list/vector of strings) as a persistent child process with piped stdio. An optional opts map tunes the child: :cwd (a string) sets its working directory, :env (a map of string->string) adds environment variables on top of the inherited environment. |
|  | `proc-send` | 2 | Write data to subprocess p's stdin (blocking) and flush. data is any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139); a string leaf is always its UTF-8 bytes, whatever the child's mode (ADR-141). Returns nil; |
|  | `proc-set-binary` | 2 | Switch subprocess p's INBOUND decode between text mode (default) and binary mode (mirrors tcp-set-binary; outbound proc-send is unaffected, ADR-141). |
|  | `proc-close` | 1 | Terminate subprocess p: kill it if still running and close its stdin. Idempotent; returns nil. The final [:proc-closed handle code] still arrives at the owner. |
| **Crypto / random** (`%`-internal; `std/crypto.blsp` + `std/hash.blsp` are the libraries) | `%random-bytes` | 1 | n cryptographically-strong random bytes as a bytes value. |
|  | `%digest` | 2 | Raw digest of a byte sequence (bytes value, vector, or list of byte ints 0–255) under algorithm keyword `algo` (:md5 :sha1 :sha256 :sha384 :sha512), returned as a bytes value (not hex). The one digest primitive; the public sha256/md5/… hex/string names are Brood over this in std/hash.blsp. |
|  | `%hmac` | 3 | HMAC of `msg-bytes` keyed by `key-bytes` (both byte sequences) under algorithm keyword `algo` (:md5 :sha1 :sha256 :sha384 :sha512), returned as a bytes value (raw MAC, not hex). The public hmac-sha256/… names are Brood over this in std/hash.blsp. |
|  | `%chacha20-encrypt` | 3 | Encrypt plaintext-bytes with ChaCha20-Poly1305 (AEAD). key-bytes must be 32 bytes; nonce-bytes must be 12 bytes. Returns ciphertext bytes (plaintext + 16-byte auth tag). NEVER reuse a (key, nonce) pair — use a fresh nonce per message (see crypto/random-nonce). |
|  | `%chacha20-decrypt` | 3 | Decrypt ciphertext-bytes with ChaCha20-Poly1305. Returns plaintext bytes, or :error if authentication fails. |
|  | `%pbkdf2-sha256-bytes` | 4 | PBKDF2-HMAC-SHA256 key derivation over byte-sequence password and salt (raw bytes, not UTF-8 strings — a binary salt round-trips faithfully). Returns a key-len-byte bytes value. Use iterations >= 600000 for password storage. |
|  | `random-token` | 1 | n cryptographically-strong random bytes from the OS RNG, hex-encoded as a 2n-char string. Used to mint a node cookie. |
| **Git / archives** (`%`-internal; the package manager's substrate, ADR-037) | `%git-resolve-ref` | 2 | Resolve git `ref` (tag/branch/commit) at remote `url` to a commit hash (via `git ls-remote`), or nil if not found. The package manager's ref-pinning mechanism (ADR-037). |
|  | `%git-clone` | 4 | Shallow-clone `url` into `dest` and check out the exact `commit` (detached); `ref` is the fetch fallback. Returns :ok or throws. The package manager's fetch mechanism (ADR-037). |
|  | `%git-changed-files` | 1 | Absolute paths of files NOT committed-clean under `dir` (modified, staged, or untracked — the union `git status --porcelain` reports). Returns a list of strings (nil when the tree is clean — an empty list is nil), or the keyword :not-a-repo when `dir` is not inside a git work tree. |
|  | `%untar-gz` | 3 | Extract a gzip'd tar `archive` into `dest`, stripping `strip` leading path components (package convention: 1). Shells to `tar`. Returns :ok or throws. The tarball-dep delivery mechanism (ADR-037). |
|  | `%rm-rf` | 1 | Recursively delete `path`. Bounded to paths under `_deps/` (refuses anything else). Idempotent. The package manager's cache-eviction mechanism (ADR-037). |
| **Coverage** (ADR-148; armed by `BROOD_COVERAGE=1`) | `%coverage-lines` | 0 | Every source line recorded as EXECUTED, as a list of [file (line …)]. Empty unless the run was started with BROOD_COVERAGE=1 (`nest test --cover-lines`). |
|  | `%coverage-instrumented` | 0 | Every source line the compiler INSTRUMENTED, as a list of [file (line …)] — the denominator %coverage-lines is a subset of. Arms compile when defined, so a never-called function appears here and not there. |
|  | `%coverage-precompile` | 1 | Compile f's body now, without calling it, so its lines count toward %coverage-instrumented. Returns true if a body was compiled. |
|  | `%coverage-reset` | 0 | Forget every line recorded by %coverage-lines, so a long-lived image can measure more than once without runs bleeding together. |
| **GUI** (optional native window backend, ADR-046; needs `--features gui`) | `gui-open` | 0–4 | Open a new native window and return its integer id (needs the runtime built with --features gui; errors otherwise). An optional `title` string sets the OS title-bar text (default `Brood`); change it later with gui-title!. An optional `opts` map carries the attributes fixed at build time: `{:decorations false}` for a borderless window, `{:app-id "my-app"}` for the desktop application id (Wayland `app_id` / X11 `WM_CLASS`) the installed `my-app.desktop` entry is named after — what gives the window its own icon and name in the dash instead of the desktop's generic fallback. |
|  | `gui-close` | 1 | Close window id (the teardown for gui-open). Idempotent; an unknown id is a no-op. |
|  | `gui-draw` | 2 | Paint a frame (the same render-op vector term-draw takes) to window id; returns nil. Unknown ops are skipped (forward-compatible). |
|  | `gui-size` | 1 | Window id's size as [cols rows] in character cells (tracks resize / HiDPI), same shape as term-size. |
|  | `gui-title!` | 2 | Set window id's OS title-bar text to the string text at runtime (the title gui-open gave it, or the default, otherwise). Needs --features gui; a no-op if the GUI thread never started or id isn't a live window. Returns nil. |
|  | `gui-icon!` | 4 | Set window id's taskbar / title-bar icon from raw RGBA pixels: rgba is a vector of w*h*4 byte ints (0-255), row-major, 4 per pixel (red, green, blue, alpha). Needs --features gui; a silent no-op if the GUI thread never started, id isn't a live window, or the data length isn't w*h*4. |
|  | `gui-focus` | 1 | Raise window id to the front and give it OS keyboard focus, un-minimising it first. Lets an app surface an already-open (singleton) window instead of opening a duplicate — e.g. `(observe)` focuses its existing window rather than spawning a second. Errors only if id isn't a live window. |
|  | `gui-bg!` | 1 | Set the window background colour: the fill for :clear, the per-frame pre-clear, and — being outside every cell — the gui-inset! margin and the cell-grid snap remainder. So a GUI app's padding matches its own theme background instead of the hardcoded default. |
|  | `gui-inset!` | 1 | Set the window content inset to px logical pixels: a blank margin before the cell grid on every window edge, so a GUI app's text breathes instead of sitting flush against the frame. Applies to every open window and the default for ones opened later; |
|  | `gui-font!` | 1–2 | Set a cell font from spec, a map {:family <keyword> :height <px>} (both keys optional): :family picks a registered font family (bundled :mono, or one added by gui-font-register), :height the cell pixel size. (gui-font! spec) sets the global default — every open window and ones opened later; |
|  | `gui-font-register` | 2 | Register font family name (a keyword) from styles, a map of style → TTF file path {:regular "…" :bold "…" :italic "…" :bold-italic "…"}. Only :regular is required; a missing style reuses the regular file. Afterwards a face's :family <name> (or gui-font!) selects it. Needs --features gui. |
|  | `gui-fullscreen!` | 2 | Make window id borderless-fullscreen while `on` is truthy (covering the whole monitor it's on, NO title bar / decorations — distraction-free), or restore it to a normal window otherwise. For a big-but-normal window that keeps its title bar, use gui-maximize! instead. |
|  | `gui-maximize!` | 2 | Maximise window id while `on` is truthy (fill the screen's work area, KEEPING the title bar / decorations), or restore it to its previous size otherwise — e.g. an editor's init file opening big without going true-fullscreen. |
|  | `gui-grab-cursor` | 2 | Confine the pointer to window id while `on` is truthy, release it otherwise — for mouse-look that shouldn't let the cursor slip out of the window and click another app. |
|  | `gui-held-key` | 1 | The key window id currently sees as physically held — the same value its press delivered (a 1-char string, or a keyword like :ctrl-n / :up) — or nil when none is held. |
| **Audio** (needs `--features audio`) | `audio-beep` | 2–3 | Play a short tone of freq-hz for ms milliseconds, optionally at peak amplitude vol (0..1, default ~0.18 — pass a small vol for quiet/ambient sounds). Fire-and-forget — it never blocks the caller, and overlapping beeps mix — so a game can blip from its frame loop. |
| **Clipboard** | `clipboard-get` | 0 | The OS clipboard's text, or nil when empty / non-text / unavailable (no display server, or a build without the clipboard feature). |
|  | `clipboard-set!` | 1 | Copy string s to the OS clipboard so other apps can paste it; returns s. A no-op (still returns s) when no clipboard is available or the clipboard feature is off. |
| **Scheduler / signals** | `sched-stats` | 0 | A snapshot map of the scheduler's cumulative counters: {:spawned :exited :preempts :steals :migrations :workers :peak-threads}. :spawned - :exited is the live-process figure; :preempts counts reduction-budget quantum exhaustions; :steals/:migrations count work-stealing activity. |
|  | `steal-count` | 0 | How many fresh processes the scheduler work-stole across worker threads since program start; 0 means placement-at-spawn kept the pool even. |
|  | `profile-start` | 0–1 | Arm the sampling CPU profiler at hz samples/sec (default 99, clamped 1..10000), resetting the histogram. Sampling walks each process's reified call stack (named frames) at its next VM frame boundary after every tick — no signals, near-zero cost when off (one relaxed load per frame boundary). |
|  | `profile-stop` | 0 | Disarm the sampling profiler and return the histogram: a list of {:stack (fn-names... innermost-first) :count n} maps, most-sampled first. Empty list if never armed. A sample whose frames were all anonymous appears with :stack ("<anonymous>"). |
|  | `system-monitor` | 0–2 | Read, arm, or clear the kernel system monitor — runtime events pushed to ONE subscriber process as [:system kind subject-pid detail] mailbox messages (Erlang system_monitor/2 shape; the observability event stream's kernel sources). |
|  | `process-flag` | 1–2 | Read or set a per-process runtime flag on the current process (Erlang process_flag/2); returns the previous (or, with no value, current) setting. Flags: :max-heap — this process's heap limit in bytes (BEAM max_heap_size analogue; positive int sets, nil clears, absent reads). |
|  | `trap-exit` | 1 | Set the current process's trap_exit flag (Erlang process_flag(trap_exit, …)); returns the previous value. When on, a linked peer's death arrives as a trappable [:EXIT pid reason] message instead of killing this process. |
| **GC / VM stats** | `gc-stats` | 0 | A snapshot map of GC activity: :collections, :copied, :reclaimed (cumulative object counts), :live, :live-bytes, :threshold (next-collection trigger), and the pause-duration trio :pause-total-us/:pause-max-us/:pause-last-us (cumulative wall time in collections, worst single pause, most recent — the |
|  | `gc-collect` | 0 | Force a collection of this process's LOCAL heap now, returning the post-collection gc-stats map. An observability/test aid, not a load-bearing trigger — automatic collection at the eval safepoint already keeps memory bounded. |
|  | `gc-trace` | 0–1 | Query (no arg) or set (truthy arg) per-collection GC trace logging for this process; returns the resulting state. When on, each minor/major collection prints a one-line summary to stderr. Defaulted from BROOD_GC_TRACE. |
|  | `runtime-collect` | 0 | Compact the shared RUNTIME code region, reclaiming superseded versions of redefined globals (hot-reload churn). Returns {:before N :after M :reclaimed (N-M) :ran bool} (closure counts). |
|  | `vm-stats` | 0 | A snapshot map of VM work-attribution counters (the perf-stats feature). :enabled is false unless the binary was built with --features perf-stats; |
|  | `mem-limit` | 0 | Hard memory ceiling in bytes (0 = unlimited); crossing it aborts the process. Set via BROOD_MEM_LIMIT. |
|  | `mem-soft-limit` | 0 | Soft memory ceiling in bytes (0 = unlimited); crossing it raises a catchable E0043 at the next safepoint. |
## Special forms (not primitives)

These are evaluation rules in `crates/lisp/src/eval/mod.rs`, not functions — they
control how their arguments are evaluated and cannot be passed as values:

```
quote  if  do  def  fn  let  letrec  quasiquote
```

`defmacro`, `when`, `unless`, `cond`, `and`, and `or` are **prelude macros**, not
special forms (ADR-022). `defmacro` lowers to `(def name (%make-macro (fn …)))` —
a macro is just a closure the expander calls, and `%make-macro` is the lone
primitive that tags one. There is no `set!` and no `while`: data is immutable and there is
no local mutation — `def` (redefining a global) is the only mutation, and loops
are recursion or processes (ADR-026).

---

## Error handling (implemented)

Error signalling and handling, with a minimal kernel footprint — **two new
primitives, zero new special forms** — keeping the ergonomic layer in Brood.

| New | Where | What |
|---|---|---|
| `throw` | **primitive** (kernel) | `(throw v)` raises `v` as an error — a non-local exit. |
| `%try` | **primitive** (kernel) | `(%try thunk handler)` — call `thunk` (a 0-arg fn); if it raises, call `handler` with the caught value, else return the thunk's result. The low-level catch mechanism. |
| `try` / `catch` | **prelude macro** (Brood) | `(try body... (catch e handler...))` — sugar that wraps the body and handler in `fn`s and calls `%try`. |
| `error` | **prelude** (Brood) | `(error msg & parts)` ⇒ `(throw (str msg ...))` — the common "raise a message" case. |

Net kernel growth: **+2 primitives (`throw`, `%try`), and zero new special forms.**
The `try`/`catch` *syntax* is a macro written in the language — keeping the
evaluator's special-form set unchanged, per "the language must be as small as
possible." Two functions are a smaller addition to the *language* than one
special form, because special forms are core evaluator semantics while
primitives are just Rust-implemented functions.

### Supporting change

`LispError` gains an optional payload so a thrown value can ride along the error.
It has since grown a call trace and a control channel; the shape today
(`crates/lisp/src/error.rs`, boxed as `LispError(Box<LispErrorData>)` to keep the
`Result` small) is:

```rust
struct LispErrorData {
    kind: ErrorKind,
    message: String,
    trace: Vec<TraceFrame>,     // captured as the raise unwinds; surfaced as `:trace`
    control: Option<Control>,   // Some only for a suspend riding the error channel
    payload: Option<Value>,     // the value carried by `(throw v)`
    …                           // position/hint fields — see error.rs
}
```

`throw` sets `payload`; kernel errors (e.g. `%div` ÷0, arity, type) leave it
`None`, and `try_catch` then projects the structured map described below.

### `try` / `catch` semantics

```clojure
(try
  (risky-thing)
  (catch e
    (println "failed:" e)
    :recovered))
```

- Evaluate the body forms in order; the value of the last is the result.
- If a body form raises, bind `e` to the **caught value** and evaluate the
  handler forms; the value of the last handler is the result.
- The `catch` clause is the last form of the `try`.
- (No `finally` in v1 — can add later.)

It desugars to the `%try` primitive:

```clojure
(try a b (catch e h))
;; expands to:
(%try (fn () a b) (fn (e) h))
```

### What `catch` binds

For `(throw v)`, `e` is the thrown value `v`, unchanged — user throws are never
wrapped. `error` throws a string, so `e` is that string.

For a **kernel-raised** error (parse, unbound, arity, type, runtime — e.g. `%div`
÷0), `e` is a **structured map**:

```lisp
{:kind <keyword>           ; :parse | :unbound | :arity | :type | :runtime | :user
 :message <string>         ; the rendered text
 :code <string>            ; stable, "E00xx"
 :file <string>            ; when known (set by load / the file runner)
 :line <int> :col <int>    ; when known (1-based)
 :hint <string>}           ; optional, points at a likely fix
```

So branch on `:code` or `:kind` programmatically and show `:message`/`:hint` to a
human:

```clojure
(try (/ 1 0)
  (catch e (if (= (get e :code) "E0040") :div-by-zero (throw e))))
```

The full code list and the numbering scheme are in
[`error-codes.md`](error-codes.md); the projection an agent sees over MCP is in
[`mcp.md`](mcp.md).

> Historically `e` was only the error's *message string* — a deliberate
> simplicity choice (ADR-011) that lost the `kind`. The structured map replaced it
> as the "once map literals exist" refinement that note anticipated; a user throw
> keeping its exact value is what preserved backward compatibility.
