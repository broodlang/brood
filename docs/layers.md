# Layers — composable, runtime-reconfigurable behaviour (design-of-record)

`std/layers.blsp` (framework tier, opt-in, pure Brood over `keymap`, **zero kernel
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
{:layers '(grid/layer magit/layer) :type :magit-status …}
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

`std/buffer.blsp` stays **layer-agnostic**; the registries + seeding live here, and
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

## API (Phase 1 — the buffer-free core)

```
layer-collect (ctx facet)            -> list of facet values, highest-precedence first
push-layer / remove-layer / replace-base-layer / layer-active?   (stack ops)
activate-layer (ctx layer)           -> push + run that layer's :activate
deactivate-layer (ctx name)          -> run that layer's :deactivate + remove
replace-base-layer (ctx layer)       -> deactivate old base, activate new (major-swap)
run-event (ctx event)                -> fan event hooks across active layers (focus/blur/close)
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
- Phase 3 — scopes, focus/blur/close events, async, `ui-run` integration + demo.

## Deferred (named, not precluded)

`:applies-to` (decentralised type binding) · per-binding `when`-guards · cross-layer
prefix/chord merging · `:commands` manifest + M-x / which-key palette ·
precedence-as-a-value / nested layers · regex filename matching (M2 engine).
