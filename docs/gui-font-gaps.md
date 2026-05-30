# GUI font / pane gaps (display seam)

Findings from building a Game of Life demo's split view (board + a larger-font
status strip) on the display seam. Each gap below is a candidate for an ADR or a
`known-issues.md` entry; line refs are into this tree. (Originated in the `foobar`
demo project — see its `docs/gui-font-gaps.md`.)

## TL;DR

On the GUI frontend there is exactly **one font size for everything**. You cannot
make one pane, op, buffer, or window bigger than another. The only way to enlarge
text is to draw glyphs out of multiple grid cells (a hand-rolled "block font",
magnified) — which is what `src/life.blsp` does for its status strip
(`status` / `glyph-row` / `scale-row` / `status-ops`).

## Gap 1 — no per-op / per-region font size

A render op is `[:text row col s face]`, and a `face` is the only per-op styling
hook. The GUI `Face` carries **no size**:

```
crates/lisp/src/gui.rs:43   pub struct Face { fg, bg, bold, italic, underline, reverse, family }
```

`:family` (gui.rs:50) lets a face pick a *registered family*, and `face.blsp`
documents a `:height` attribute — but `:height` is only "a hint, honored by the
whole-window `gui-font!` knob" (`std/face.blsp`), i.e. it does **not** resize an
individual op. The grid is uniform: one global `cell_h` (gui.rs renderer), every
`[:text …]` lands on integer cell coordinates at that one size.

**Impact:** a "big heading", a "large status line", or a zoomed pane is impossible
without the block-font trick. Mixed-size text in a single frame can't be expressed.

**Possible fixes (pick one):**
- Add a size/scale to `Face` (e.g. `:scale n` or `:height px`) and honor it per op
  in the renderer. Cleanest; faces already flow end-to-end.
- A new render op, e.g. `[:scale n & ops]` or `[:text-big row col s n face]`, that
  the frontend rasterizes at n× (and the terminal frontend can ignore or emulate).
- A std helper that *generates* the block-font ops (promote `life.blsp`'s
  `scale-row`/`status-ops` into a `bigtext` std module) — no kernel change, just
  removes the per-app copy.

## Gap 2 — `gui-font!` is global across *all* windows

`gui-font!` is documented as "the whole-window knob", and the implementation
applies it to **every** open window, not just one:

```
crates/lisp/src/gui.rs:570  UserEvent::Font { family, px } => {
crates/lisp/src/gui.rs:577      for w in self.wins.values_mut() { w.renderer.set_font(family, px); … }
```

So even the "two windows" escape hatch fails: opening a second window and calling
`gui-font!` to enlarge it resizes the first one too. There is no per-window font
primitive exposed.

**Impact:** multi-window UIs can't run different font sizes side by side.

**Possible fix:** a per-window form, e.g. `(gui-font! id spec)` / a window-scoped
override, leaving `gui-font!` with no id as the global default.

## Gap 3 — "layers" are behaviour layers, not display layers

`std/layers.blsp` ("composable behaviour layers") is a **keymap/hook** mechanism
(what myedit's modes are built from) — it does not touch rendering. There is no
display-side pane/clip/compositing layer with its own font or viewport. A frame is
one flat op list; clipping and "panes" are the app's job (in `life.blsp`,
`render` clips cells to `vcols`×`vrows` and reserves a bottom strip by hand).

**Impact:** every windowed app re-implements pane layout, clipping, and (for big
text) font scaling.

**Possible fix:** an optional display-layer/pane abstraction (region + clip + an
independent font scale) on the display seam — would make Gap 1 and pane layout fall
out naturally.

## Workaround in use (foobar)

`src/life.blsp` enlarges its status strip by magnifying a 3×5 block font
`*font-scale*`× in both axes (`scale-row` repeats each glyph cell horizontally;
`status-ops` repeats each pixel row vertically). It's correct but manual, limited to
the glyphs defined in `*font*`, and the magnified line easily overruns a narrow
window's width (it just clips). Closing Gap 1 would let this be a normal
`[:text … {:scale 2}]`.
