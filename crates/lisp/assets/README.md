# Bundled assets

## Fonts

`DejaVuSansMono.ttf`, `DejaVuSansMono-Bold.ttf`, `DejaVuSansMono-Oblique.ttf`, and
`DejaVuSansMono-BoldOblique.ttf` are the [DejaVu
fonts](https://dejavu-fonts.github.io/), bundled (via `include_bytes!`) into the
`gui` feature's windowed display backend (`src/gui.rs`) so a GUI frontend needs
no system font discovery. The four styles are the default `:mono` font family —
regular / bold / italic (oblique) / bold-italic — selected per face (a face's
`:bold`/`:italic` choose the style; `:family` selects another registered family).
DejaVu is released under a permissive Bitstream Vera / Arev free license (free to
use, bundle, and redistribute). They are used only when the crate is built with
`--features gui`.
