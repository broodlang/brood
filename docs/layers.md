# Layers — composable, runtime-reconfigurable behaviour (design-of-record)

`std/editor/layers.blsp` (framework tier, opt-in, pure Brood over `keymap`, **zero kernel
surface**). The generic mechanism the editor's "modes" are built from — but it is
*not* editor-specific: any interactive Brood UI uses it, with or without buffers.

## Why this exists (and why not "major/minor modes")

"Major/minor mode" is Emacs *vocabulary*, not a primitive. The clunky parts —
fixed major/minor tiers, implicit keymap precedence, mutable global hook vars, an
assumed current buffer — aren't essential. Strip them and the real thing is:

> **A context holds an ordered list of named behaviour layers; input and events
> resolve through them by precedence.**

Everything the editor wants is a *usage* of that, not a separate feature:
- a single specialised GUI → a layer list of length 1;
- a major mode per buffer → the list lives on the buffer;
- 0..n minor modes → push/remove layers;
- "only one major" → replace the base entry.

So **"major"/"minor" become positions in a list, not types.** No mode struct, no
fixed tiers, precedence is explicit (list order).

Prior art this draws on: **CodeMirror 6** (extensions + *facets* with combine
rules + precedence + compartments — the closest modern best-in-class); **VS Code**
(commands addressable by id; document `languageId` + `onLanguage` activation —
the "bind behaviour to a buffer type" idea); **Neovim autocmd** (hooks fire on
*named events*, fanned across handlers); **Emacs** (`auto-mode-alist`
filename→mode; the model we're improving on). The repo's own `ui-run` is The Elm
Architecture (`model`/`view`/`update`), and dispatch already happens inside
`update` — layers slot in there.

## Core model

A **layer** is a `def`'d, late-bound value — resolved by symbol at use, exactly
like a keymap command (so redefining it hot-swaps everywhere; models stay small):

```clojure
(def magit/layer
  {:name 'magit
   :keymap (-> {} (keymap-bind ["s"] 'magit/stage) (keymap-bind ["c" "c"] 'magit/commit))
   :hooks  {:activate '(magit/setup) :deactivate '(magit/teardown)
            :on-focus '(magit/refresh) :on-close '(magit/kill-worker)}})
```

A **context** is any state map (an app model *or* a buffer). It carries `:layers`,
a list of layer references (symbols, or inline maps), **head = highest
precedence**:

```clojure
{:layers '(grid/layer magit/layer) :type :magit-status}   ; … plus whatever else the app keeps
```

Resolution is one generic collector — **facets**: `(layer-collect ctx facet)`
gathers each active layer's value for `facet`, highest-precedence first. Keymaps
and hooks are just the first two facets; a future `:render`/setting facet needs no
new resolution code.

### Pinned semantics

- **Precedence:** head of `:layers` wins. Keymap merge: head overrides; event
  hooks: head runs first.
- **Dormant ≠ deactivated.** Leaving a context (losing focus, switching buffers)
  never runs `:deactivate`; the layer stays on the context, merely not consulted.
  `:deactivate` fires only on explicit removal / close. Re-entry is free.
- **Lifecycle vs. fanned events.** `:activate`/`:deactivate` fire for the *one*
  layer being toggled. `:on-focus`/`:on-blur`/`:on-close` (and any app-defined
  event) are **fanned** across *all* active layers, in precedence order.
- **Hooks are `(state) -> state`**, late-bound, threaded, each error-isolated (a
  throwing hook leaves state unchanged — a bad layer can't crash the loop). Side
  effects (incl. `spawn` for async work) ride along; async results return as
  mailbox messages handled by the `ui-run` loop.
- **Chord ownership:** a prefix sub-map comes wholesale from its winning layer (no
  cross-layer prefix merging in v1).

## Buffer-type binding (Phase 2)

A new buffer needs a starting layer set, determined by its **type**:

- a buffer carries `:type` (`:text`, `:magit-status`, …);
- `*type-layers*` maps `type → (layer refs)` (data, `def`-rebindable → hot-reload);
- `*auto-type-by-file*` maps a filename to a type — an ordered `auto-mode-alist`
  analogue, `[{:match <suffix-or-(fn)> :type …} …]`, first match wins (suffix →
  `ends-with?`, fn → called on the name). **No regex** in v1 (slots in when M2's
  regex engine lands).

Resolution: `(buffer-type-for buf)` = match `:file` against `*auto-type-by-file*`;
**no file → the buffer's explicitly-set `:type`** (default `:fundamental`).
`(set-buffer-type buf type)` is the Brood-side override for fileless/special
buffers. `(init-buffer-layers buf)` resolves the type → seeds `:layers` from
`*type-layers*` → fires each `:activate`.

`std/editor/buffer.blsp` stays **layer-agnostic**; the registries + seeding live here, and
the app calls `init-buffer-layers` on creation.

## Scopes & focus (Phase 3)

Layers never learns about "focus" — the app owns it (it already owns the
window/buffer list). The active set = the **focused** context's layers; switching
focus switches the active set automatically (the other context's layers go
dormant). The app composes scope order — `(focused-buffer ++ window)`, buffer
shadowing window (Emacs local-over-global) — via `active-layer-ctx`; a buffer-less
GUI passes just the window. The app fires `:on-blur`/`:on-focus` on focus change
and `:on-close` (+ deactivate all) on close. `:on-close` is the async-cleanup hook
(kill a worker a layer spawned).

### Taking companions with you (`:on-close` + `request-close`)

A mode often owns more than its own context: the tutorial's `*Workings*` pane, a
REPL's transcript, a debugger's queue — buffers it created that mean nothing once
it is gone. An `:on-close` hook cannot close them, and deliberately so: a hook is
`(ctx) -> ctx`, and a "context" here is whatever the app threads through (a buffer,
a window), so layers has no idea what container they live in. That ignorance is why
it composes with any app.

So a hook does not reach — it **names**:

```
(defn my-on-close (buf) (request-close buf "*Workings*"))   ; in the layer's :hooks
…
(let (closed (close-context buf))                           ; the app, on kill
  (doseq (nm (close-requests closed)) …kill nm…))
```

`request-close` records a companion on the closing context (accumulating,
de-duplicated); `close-requests` reads the list back off `close-context`'s result;
the app — which does know its own pool — performs the closes and decides what an
unrecognised name means (myedit ignores it: the mode may have been unloaded, or the
companion already closed). The same shape as a render op or a keymap command: the
layer says *what*, the app does it. myedit follows one level only, so two modes
naming each other cannot loop.

## API (Phase 1 — the buffer-free core)

```
layer-collect (ctx facet)            -> list of facet values, highest-precedence first
push-layer / remove-layer / replace-base-layer / layer-active?   (stack ops)
activate-layer (ctx layer)           -> push + run that layer's :activate
deactivate-layer (ctx name)          -> run that layer's :deactivate + remove
replace-base-layer (ctx layer)       -> deactivate old base, activate new (major-swap)
run-event (ctx event)                -> fan event hooks across active layers (focus/blur/close)
close-context (ctx)                  -> :on-close fan + deactivate-all (the full close path)
request-close (ctx name)             -> from an :on-close hook: ask the app to close companion `name`
close-requests (ctx)                 -> the companions a closed context asked to take with it
active-keymap (ctx)                  -> merge active layers' :keymap (head wins)
layer-dispatch (ctx pending key fb)  -> active-keymap + keymap-step → [ctx' pending']
```

`push-layer`/`activate-layer` take a layer *reference*; `remove-layer`/
`deactivate-layer`/`layer-active?` take a layer **`:name`** (the stable identity).

## Status

- ✅ **Phase 1 — layer core (keys + hooks), buffer-free.**
- ✅ **Phase 2 — buffer-type binding** (`*type-layers*` / `*auto-type-by-file*`,
  `register-type-layers` / `register-file-type` / `layers-for-type`,
  `buffer-type-for` / `set-buffer-type` / `init-buffer-layers`). Filename matching
  is suffix or predicate, newest-rule-wins. `std/buffer` stays layer-agnostic.
- ✅ **Phase 3 — scopes, focus/close lifecycle, async.** `compose-layers` /
  `scope-keymap` / `scope-dispatch` (multi-scope dispatch, command runs on the app
  model); `switch-focus` (`:on-blur`→`:on-focus`, dormancy not teardown);
  `deactivate-all` / `close-context` (`:on-close` async cleanup, then `:deactivate`).
  A big in-code **HOW TO USE** guide in `std/editor/layers.blsp` covers the whole flow incl.
  a `ui-run` loop recipe. (A live TTY/GUI demo is left to the editor app, since it
  can't run in the test suite; the loop recipe is documented.)

## First consumer: structural navigation + editor modes

The first real use of layers is editing modes — and it sets the tier line:

- **`std/tool/sexp.blsp` (std).** Structural s-expression navigation over Brood's own
  `parse-source` CST (a lossless typed tree — our "tree-sitter" for Brood code):
  annotate positions in one walk, then navigate by structure
  (`point-forward`/`backward`/`up`/`down`/`defun-start`, plus buffer commands).
  Written against an abstract node shape `{:kind :start :end :kids}`, so a
  foreign-language backend (tree-sitter, in the editor) can later produce the same
  shape and reuse these commands. This is *reusable Brood-code tooling* — same tier
  as the formatter / LSP — so it lives in std, not the editor.
- **The modes live in the editor, not brood** (`examples/editor/src/`).
  `text-mode` (the default — registered for `:fundamental`) and `brood-mode`
  (`.blsp` → `:brood`; reuses text-mode's motion + `sexp` nav + `eval-command`'s
  `C-x C-e`, stashing the result in `:message`) are *policy* — which keys do what,
  which file types map to which mode. A different Brood editor would ship different
  ones, so they're app config, loaded from the editor project's `src/` at runtime,
  **not baked into the `brood` binary.** Each mode is just a layer; the
  `:parser :brood` facet marks the structural backend (a `ruby`/`elixir` layer is
  the same shape with a tree-sitter `:parser`/`:grammar` facet — no new concept).
  Tested by the editor's own `nest test` (`examples/editor/tests/`).

## Deferred (named, not precluded)

`:applies-to` (decentralised type binding) · per-binding `when`-guards · cross-layer
prefix/chord merging · `:commands` manifest + M-x / which-key palette ·
precedence-as-a-value / nested layers · regex filename matching (M2 engine).
