# Bundled assets

## Fonts

`DejaVuSansMono.ttf` and `DejaVuSansMono-Bold.ttf` are the [DejaVu
fonts](https://dejavu-fonts.github.io/), bundled (via `include_bytes!`) into the
`gui` feature's windowed display backend (`src/gui.rs`) so a GUI frontend needs
no system font discovery. DejaVu is released under a permissive Bitstream Vera /
Arev free license (free to use, bundle, and redistribute). They are used only
when the crate is built with `--features gui`.
