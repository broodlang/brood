# GUI font / pane gaps (display seam) — resolved

Findings from building a Game of Life demo's split view (board + a larger-font
status strip) on the display seam. **Status: these gaps are now closed** — Gaps 1
and 2 by ADR-079 (per-op `:scale` faces + per-window `gui-font!`), Gap 3 largely
by `std/editor/pane.blsp` — so this doc is a record of *resolved* gaps, kept for
the reasoning; line refs are into this tree. (Originated in the `foobar`
demo project — see its `docs/gui-font-gaps.md`.)

## TL;DR

Originally the GUI frontend had exactly **one font size for everything** — the
only way to enlarge text was a hand-rolled "block font" drawn across multiple
grid cells (what `src/life.blsp` did for its status strip). No longer: a face's
`:scale` attribute (ADR-079) draws an op's text n× larger, and `(gui-font! id
spec)` sets a font per window. Mixed-size text in a single frame is now a normal
`[:text … {:scale 2}]`.

## Gap 1 — no per-op / per-region font size — RESOLVED (ADR-079)

A render op is `[:text row col s face]`, and a `face` is the only per-op styling
hook. The GUI `Face` originally carried **no size**. It now does:

```
crates/lisp/src/gui.rs:54   pub struct Face { fg, bg, bold, italic, underline, reverse, family, scale }
```

`:family` lets a face pick a *registered family*; `:height` remains only "a
hint, honored by the whole-window `gui-font!` knob" (`std/editor/face.blsp`).
The new **`:scale`** attribute (documented at `std/editor/face.blsp:37`; a
positive integer, default 1, capped at 16) is the per-op knob: the op's text is
drawn `:scale`× larger, occupying a scale×scale block of base cells anchored at
its (row, col). The renderer bakes a separate glyph canvas per
(cluster, family, style, scale) — the glyph cache at `crates/lisp/src/gui.rs`
(~2173) is keyed on scale. The terminal frontend ignores it (renders 1×).

This was the first of the "possible fixes" originally listed here — *add a
size/scale to `Face` and honor it per op in the renderer* — and it's what
shipped: a big heading, a large status line, or a zoomed section is
`[:text … {:scale 2}]`, no block-font trick.

## Gap 2 — `gui-font!` was global across *all* windows — RESOLVED

`gui-font!` originally applied to **every** open window, so even the "two
windows" escape hatch failed: opening a second window and calling `gui-font!` to
enlarge it resized the first one too. The originally proposed fix — a
per-window form, leaving the no-id call as the global default — shipped:

```
crates/lisp/src/gui.rs:505  (gui-font! spec)     — the global default: every open
                            window, and remembered for windows opened later
crates/lisp/src/gui.rs:506  (gui-font! id spec)  — just window `id`; does not touch
                            the global default
```

`UserEvent::Font` carries `id: Option<u64>` and the event loop handles the
global-default and per-window arms separately (gui.rs:1859; dispatch ~1370), so
two windows can run different fonts side by side.

## Gap 3 — "layers" are behaviour layers, not display layers — largely closed

`std/editor/layers.blsp` ("composable behaviour layers") is an **editor/keymap/hook**
mechanism (what myedit's modes are built from) — it does not touch rendering.
A frame is still one flat op list. But pane layout is no longer every app's job:
**`std/editor/pane.blsp`** provides Emacs-style tiled panes — an immutable
binary-tree layout with splits, computed rects, divider cells, and pure
drag-resize geometry (`divider-at` / `pane-ratio-for` / `pane-set-ratio`) — so a
windowed app composes its frame from pane rects instead of re-implementing
layout by hand.

The remaining app-side piece is per-pane clip / font-scale on the display seam:
clipping ops to a pane's rect (and applying a pane-wide `:scale`) is still done
by the app when it renders each pane's content, rather than by a display-side
layer with its own viewport.

## Workaround that was in use (foobar)

`src/life.blsp` enlarged its status strip by magnifying a 3×5 block font
`*font-scale*`× in both axes (`scale-row` repeats each glyph cell horizontally;
`status-ops` repeats each pixel row vertically). It was correct but manual,
limited to the glyphs defined in `*font*`, and the magnified line easily overran
a narrow window's width. With Gap 1 closed this is a normal
`[:text … {:scale 2}]`.
