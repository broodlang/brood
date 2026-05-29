# Building an editor in Brood

This is a hands-on guide to writing a text editor *in Brood*, on top of the
pieces the language already ships: the **rope** text kernel, the **buffer**
framework (`std/buffer.blsp`), and the **display/input seam** (`std/display.blsp`
+ the `term-*` primitives). By the end you'll have a small but real terminal
editor — open a file, move around, edit, save — that you can grow into something
Emacs-shaped.

It assumes you've read [`brood-for-claude.md`](brood-for-claude.md) (the language
in a page) and skimmed [`architecture.md`](architecture.md) (why the editor is a
Brood program over a thin Rust substrate). If you're an AI assistant, load the
`writing-brood` skill first.

## The three layers

```
  Rust kernel       rope primitives, term-* draw/input   ← mechanism (in the language)
  ───────────────────────────────────────────────────
  Brood framework   std/buffer.blsp · std/display.blsp    ← the editor toolkit (opt-in)
  ───────────────────────────────────────────────────
  Your editor       a nest project: keymaps, commands,    ← what you build here
                    config, UI                              policy, all Brood
```

This mirrors Emacs (C core → built-in elisp → your `init.el` + packages). The
bottom layer — *your editor* — is an ordinary `nest` project that `(require's`
the framework. Nothing about it is privileged; it's Brood functions you can
redefine while it runs. That's the whole point.

Two properties of the framework shape everything below:

1. **A buffer is an immutable value.** Every editing operation returns a *fresh*
   buffer; nothing mutates. (This makes undo nearly free — see the end.)
2. **The frontend is a protocol of data.** You render by building a *frame* — a
   vector of render-op values — and handing it to `term-draw`. The same frame
   could be sent to a remote frontend over a socket; that's how local-native and
   server modes become one code path later.

## 1. Scaffold a project

```bash
nest new my-editor
cd my-editor
```

You get `project.blsp`, `src/`, and `tests/`. The framework modules are baked
into the binary, so you reach them with `require` — no dependency wiring:

```clojure
(require 'buffer)    ; the buffer model
(require 'display)   ; the render-op protocol
```

The `term-*` primitives (`term-enter`, `term-leave`, `term-size`, `term-poll`,
`term-draw`) are always available — they're part of the kernel.

## 2. The buffer model (`std/buffer.blsp`)

A **buffer** is a map `{:rope :point :mark :name :file}`. You rarely touch those
keys directly — you use the pure functions, each returning a new buffer:

```clojure
(def b (make-buffer "hello world"))   ; point at 0, no mark
(buffer-text b)                        ; => "hello world"
(buffer-point b)                       ; => 0

;; movement returns a fresh buffer with point moved (all clamp to bounds)
(-> b (goto-char 5) (insert ",") end-of-line buffer-text)   ; => "hello, world"
```

The vocabulary you'll use most:

- **Read:** `buffer-text` · `buffer-length` · `buffer-line-count` ·
  `buffer-line-at` · `buffer-current-line` · `buffer-column` ·
  `buffer-char-after`/`-before` · `buffer-region`
- **Move point:** `goto-char` · `forward-char`/`backward-char` ·
  `beginning-of-line`/`end-of-line` · `forward-line`/`backward-line`
  (column-preserving) · `beginning-of-buffer`/`end-of-buffer`
- **Edit:** `insert` · `delete-char` · `delete-backward-char` · `delete-region` ·
  `set-mark`/`clear-mark`
- **Files:** `buffer-from-file` (slurp) · `save-buffer` (spit)

Because operations are pure, you compose them with `->` and they never surprise
you with aliasing. A command is just a function `buffer -> buffer`.

> **Process-local rope.** A buffer holds a rope, and ropes never cross a process
> boundary (they're owned by one process). For a single-window editor you don't
> notice this; for multiple buffers/windows you either keep them all in one
> process, or use `spawn-buffer` (the actor shell) and talk to each buffer
> process with `buffer-edit`/`buffer-query`, which reply only with *derived
> views* (text, line strings, positions) — never the buffer itself.

## 3. The display protocol (`std/display.blsp`)

You render by building a **frame**: a vector of render ops. The constructors are
pure data builders:

```clojure
(frame
  (clear)                              ; clear the screen
  (text 0 0 "── my-editor ──" {:reverse true})   ; row 0, col 0, styled
  (text 1 0 "hello world")             ; an unstyled line
  (cursor 1 5))                        ; place the hardware cursor
```

- `(clear)` → `[:clear]`
- `(text row col s)` / `(text row col s face)` → a line of text at a cell
- `(cursor row col)` → the cursor position
- a **face** is a map: `{:fg :red :bg :blue :bold true :reverse true}` (colours
  are keywords: `:black :red :green :yellow :blue :magenta :cyan :white :grey
  :dark-grey`)

Rows and columns are 0-based character cells. `frame` drops `nil` ops, so you can
splice conditional ops inline.

The terminal primitives:

- `(term-enter)` / `(term-leave)` — take over / restore the terminal
- `(term-size)` → `[cols rows]`
- `(term-poll ms)` → a key, or `nil` on timeout. A printable key is a **1-char
  string** (`"a"`); special keys are **keywords** (`:up :down :left :right :enter
  :escape :backspace :tab :delete :home :end :page-up :page-down`, and control
  combos like `:ctrl-c`/`:ctrl-s`). Always pass a finite `ms`.
- `(term-draw frame)` — paint a frame.

## 4. Render a buffer to a frame

The editor's view is a pure function from state to a frame. Keep a tiny state
value — the buffer plus the top visible line (the viewport) — and render it:

```clojure
;; src/render.blsp
(defmodule render "Pure buffer → frame rendering for my-editor.")
(require 'buffer)
(require 'display)

(defn render--line (ed row line-idx cols)
  "One text op for buffer line `line-idx` drawn at screen `row`, clipped to `cols`."
  (let (raw (if (< line-idx (buffer-line-count (get ed :buffer)))
              (buffer-line-at (get ed :buffer) line-idx)
              "~")                       ; past end-of-buffer
        ;; strip the trailing newline rope-line includes
        s (if (ends-with? raw "\n") (substring raw 0 (dec (string-length raw))) raw))
    (text row 0 (if (> (string-length s) cols) (substring s 0 cols) s))))

(defn render-frame (ed cols rows)
  "Pure: render editor state `ed` ({:buffer :top}) on a `cols`×`rows` terminal —
a status line, the visible buffer lines, and the cursor."
  (let (buf (get ed :buffer)
        top (get ed :top)
        text-rows (max 1 (dec rows))             ; reserve the last row for status
        body (map (fn (i) (render--line ed i (+ top i) cols))
                  (range text-rows))
        cur-row (- (buffer-current-line buf) top)
        cur-col (buffer-column buf)
        status (str " " (buffer-name buf)
                    "  L" (inc (buffer-current-line buf)) " C" (inc (buffer-column buf))
                    "  ^S save  ^Q quit"))
    (apply frame
           (append
             [(clear)]
             body
             [(text (dec rows) 0 status {:reverse true})
              ;; only show the cursor when it's on screen
              (if (and (>= cur-row 0) (< cur-row text-rows))
                (cursor cur-row cur-col)
                nil)]))))
```

Because `render-frame` takes plain data and returns plain data, you can
**unit-test it without a terminal** — exactly how `tests/observe_test.blsp` tests
the process observer. That's the payoff of the pure-core / thin-IO split.

## 5. Keymaps and commands

A **command** is a function `editor -> editor` (or `editor -> :quit` to exit). A
**keymap** is a map from a key (the value `term-poll` returns) to a command:

```clojure
;; src/commands.blsp
(defmodule commands "Editor commands + the keymap for my-editor.")
(require 'buffer)

;; edit/move commands lift a buffer op into an editor-state op
(defn cmd--on-buffer (f)
  "A command that applies buffer transform `f` and re-scrolls to the cursor."
  (fn (ed) (scroll-to-cursor (assoc ed :buffer (f (get ed :buffer))))))

(defn scroll-to-cursor (ed)
  "Adjust `:top` so the cursor line stays on screen (keep it simple: page-ish)."
  (let (line (buffer-current-line (get ed :buffer))
        top (get ed :top)
        height (get ed :height))
    (cond
      (< line top)              (assoc ed :top line)
      (>= line (+ top height))  (assoc ed :top (- line (dec height)))
      :else ed)))

(def *keymap*
  {:up        (cmd--on-buffer (fn (b) (backward-line b)))
   :down      (cmd--on-buffer (fn (b) (forward-line b)))
   :left      (cmd--on-buffer (fn (b) (backward-char b)))
   :right     (cmd--on-buffer (fn (b) (forward-char b)))
   :enter     (cmd--on-buffer (fn (b) (insert b "\n")))
   :backspace (cmd--on-buffer (fn (b) (delete-backward-char b)))
   :home      (cmd--on-buffer beginning-of-line)
   :end       (cmd--on-buffer end-of-line)
   :ctrl-s    (fn (ed) (do (save-buffer (get ed :buffer)) ed))
   :ctrl-q    (fn (ed) :quit)
   :ctrl-c    (fn (ed) :quit)})

(defn dispatch (ed key)
  "Run the command bound to `key`. A printable 1-char string self-inserts; an
unbound key is a no-op. Returns a new editor state, or `:quit`."
  (cond
    (contains? *keymap* key) ((get *keymap* key) ed)
    (and (string? key) (= (string-length key) 1))
      ((cmd--on-buffer (fn (b) (insert b key))) ed)
    :else ed))
```

Note how `dispatch` is pure: `editor + key -> editor`. The keymap is just a map
value, so a running editor can `def` a new `*keymap*` to rebind keys live, or a
command can be redefined on the fly — that's the self-editing story, for free,
because it's all late-bound globals.

## 6. The event loop

The only impure part. Mirror the observer's structure: a tail-recursive loop that
renders, polls a key with a finite timeout, dispatches, and recurses with the new
state — until a command returns `:quit`.

```clojure
;; src/main.blsp
(defmodule main "my-editor entry point.")
(require 'buffer)
(require 'display)
(require 'render)
(require 'commands)

(defn editor--loop (ed)
  "Render, wait for a key, dispatch, repeat — until a command returns :quit."
  (let ([cols rows] (term-size)
        ed (assoc ed :height (max 1 (dec rows))))   ; keep height in state for scrolling
    (term-draw (render-frame ed cols rows))
    (let (key (term-poll 1000))
      (if (nil? key)
        (editor--loop ed)                            ; timeout: just redraw (e.g. for a clock)
        (let (next (dispatch ed key))
          (if (= next :quit)
            (term-leave)
            (editor--loop next)))))))

(defn run (path)
  "Open `path` and run the editor. Pairs with a Rust-side terminal-restore guard
(like `nest observe`) so a crash never wrecks the terminal."
  (let (buf (if (file-exists? path) (buffer-from-file path) (make-buffer "" path path)))
    (term-enter)
    (editor--loop {:buffer buf :top 0 :height 1})))

(defn main (args)
  (if (empty? args)
    (eprintln "usage: my-editor <file>")
    (run (first args))))
```

Run it: `nest run -- README.md`. (For a production editor, wrap the loop in a
Rust RAII guard that calls the terminal-restore on panic — see `cmd_observe` in
`crates/nest/src/main.rs` for the pattern. Until then, an error leaves the screen
in raw mode; `reset` in your shell fixes it.)

That's a working editor: arrows move, typing inserts, Enter/Backspace edit, ^S
saves, ^Q quits.

## 7. Test the pure core

Everything except the loop is pure, so test it like any other Brood code:

```clojure
;; tests/editor_test.blsp
(require 'test)
(require render)
(require commands)

(describe "editing commands"
  (test "self-insert + arrows"
    (let (ed {:buffer (make-buffer "ab") :top 0 :height 10}
          ed (dispatch ed "X")            ; insert X at point 0
          ed (dispatch ed :right))
      (assert= (buffer-text (get ed :buffer)) "Xab")
      (assert= (buffer-point (get ed :buffer)) 2)))
  (test "render-frame fits the terminal"
    (let (f (render-frame {:buffer (make-buffer "one\ntwo") :top 0 :height 5} 20 6))
      (is (vector? f))
      (is (every? (fn (op) (or (not (= (first op) :text))
                               (<= (string-length (nth op 3)) 20))) f)))))
```

## 8. Where to grow it

- **Undo — nearly free.** A buffer is an immutable value, so undo is just a
  *stack of past buffers*: keep `{:buffer b :history (list …)}`, push the old
  buffer before each edit, pop to undo. No diffing, no special data structure —
  this falls straight out of the immutable design.
- **Multiple buffers / windows.** Either hold a list of buffers in editor state,
  or give each its own process via `spawn-buffer` and render the *views* it
  replies with. The reply-with-views boundary is the same one a remote frontend
  uses.
- **A minibuffer / command line.** It's just another buffer rendered on the last
  row, with its own keymap — `M-x`-style command entry is reading a line into a
  buffer then `eval`-ing or looking up the command.
- **Syntax highlighting.** Faces are data: tokenize a line and emit `text` ops
  with `{:fg …}` per span. The tokenizer is pure Brood over the line string.
- **Live self-editing.** Because commands and the keymap are globals, you can
  redefine them from a running editor (a REPL buffer, or `nest run --watch`) and
  the change takes effect on the next keystroke — the Emacs superpower, with no
  host-language hot-reload machinery (it's just `def`).
- **Remote / web frontend (M4/M5).** Your `render-frame` already emits a
  serialisable frame; a socket frontend interprets the *same* ops. Nothing in the
  editor changes.
- **Drop in a process observer.** Your editor is a runtime full of processes
  (buffers-as-processes, timers, jobs). `(require 'observe)` then bind a key/command
  to `(observe-attach)` — it brings up the full-screen process viewer over *your
  editor's own* processes and returns control on `q`. (Since it `term-leave`s on
  quit, redraw your UI afterward.) The observer reads the same `process-info` /
  display seam this guide uses; remote-attaching to a running editor over a node
  link is the same loop with a remote data source.

## Reference

- Buffer API: [`std/buffer.blsp`](../std/buffer.blsp) (every function has a
  docstring; `nest doc buffer`).
- Display protocol + terminal primitives: [`std/display.blsp`](../std/display.blsp),
  [`primitives.md`](primitives.md) (the **Terminal** section), ADR-046 in
  [`decisions.md`](decisions.md).
- A complete, smaller worked example on the same seam: the process observer,
  [`std/observe.blsp`](../std/observe.blsp) + [`tests/observe_test.blsp`](../tests/observe_test.blsp).
- The immutable-data rule that makes the buffer model what it is: ADR-026
  (`docs/language.md` §Immutability).
