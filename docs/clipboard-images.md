# Clipboard images (design)

`(os/clipboard-image)` — read an image off the OS clipboard, natively, on macOS, X11 and
Wayland: one builtin, no `wl-paste` / `xclip` / `pngpaste` to install or shell out to.

> Status: **proposal.** Nothing here is implemented. It comes from Zubr (the coding agent in
> `claudette/`), which wants "paste a screenshot into the prompt" like Claude Code does.
> Once agreed, record it as an ADR next to
> [ADR-095](decisions.md#adr-095--os-clipboard-clipboard-get--clipboard-set-builtins-the-clipboard-feature),
> which it extends.

## Why

The clipboard builtins are text only. An app that wants a pasted screenshot has to shell
out, and which tool works depends on the machine:

| Platform | Tool an app has to try today | Catch |
|---|---|---|
| macOS | `pngpaste` | not installed by default (`brew install pngpaste`) |
| Wayland | `wl-paste --type image/png` | needs the `wl-clipboard` package |
| X11 | `xclip -selection clipboard -t image/png -o` | X11 only; through XWayland it can miss what a Wayland-native app copied |

So the app carries three code paths and a "please install X" message, and on a fresh
machine paste silently does nothing. The runtime already links the crate that does this
natively.

## What already exists (checked)

- `crates/lisp/Cargo.toml` links `arboard` 3 (feature `clipboard`, pulled in by `gui`) with
  `default-features = false, features = ["wayland-data-control"]`. The comment there says
  `default-features = false` was chosen to drop arboard's `image-data` default so the
  `image` crate is not pulled in.
- That reason is gone: the runtime **already** builds `image` 0.25 (png, jpeg, gif, webp,
  bmp) for `image-thumb` (`builtins/filesystem.rs`).
- arboard 3.6.1 has, behind `image-data`, `Clipboard::get_image() -> Result<ImageData,
  Error>` with `ImageData { width, height, bytes }` (RGBA8, row-major), and
  `set_image(ImageData)`. It returns `Error::ContentNotAvailable` when the clipboard is
  empty or holds no image.
- `host/clipboard.rs` keeps one process-lifetime `Clipboard` in a `OnceLock<Option<Mutex<…>>>`
  (needed so an X11/Wayland selection owner stays alive) and degrades to no-ops without a
  display or without the feature.

## Proposal

### Builtin

```
(os/clipboard-image)            ;; => {:png bytes :width w :height h}  or nil
(os/clipboard-image max-edge)   ;; same, downscaled so the longer side is <= max-edge
```

- `:png` is the image **PNG-encoded** (a `bytes` value), so a caller can write it to a file
  or base64 it into an API request without knowing about pixel formats. `:width` / `:height`
  are the dimensions of what is in `:png` (after any downscale).
- Returns `nil` when the clipboard is empty, holds text or a file list, is unreadable, no
  display is reachable, or the build has no `clipboard` feature. Same "graceful no-op"
  contract as `clipboard-get`; a caller never branches on a build flag.
- `max-edge` is optional and downscale-only (never upscales), reusing the resize path of
  `image-thumb`. Model APIs cap image size (the long edge is downscaled server-side anyway,
  and there is a byte limit), so letting the caller bound it here avoids encoding a 5K
  Retina screenshot only to throw most of it away.
- Primitive `%clipboard-image`, wrapped in `std/os.blsp` next to `clipboard-get`, so the
  docstring and signature live in the same two places as the text pair.

Optional companion, mainly so the feature is testable end to end:

```
(os/clipboard-set-image png-bytes)   ;; => png-bytes; no-op when unavailable
```

### Rust

- `Cargo.toml`: `arboard = { …, features = ["wayland-data-control", "image-data"] }`;
  update the comment above it (text **and** images now). `image-data` pulls, per platform,
  arboard's own use of `image` (png only on Linux), `objc2-core-graphics` and
  `objc2-core-foundation` on macOS, `windows-sys` on Windows. Cargo unifies `image`
  versions, so no second copy on Linux/macOS. **Check with `cargo tree -d` and compare the
  release binary size before and after.**
- `host/clipboard.rs`: add `get_image() -> Option<(usize, usize, Vec<u8>)>` under the same
  `cfg` and the same handle; a no-op returning `None` without the feature.
- `builtins/clipboard.rs`: `%clipboard-image` takes the RGBA buffer, optionally downscales
  (`image::imageops::thumbnail`, as in `image-thumb`), PNG-encodes with
  `image::codecs::png`, and builds `{:png :width :height}` the way `image_thumb` builds its
  map (no eval between allocating the bytes and `map_from_pairs`, so no GC hazard).
- Guard the size like `image-thumb` does: refuse a source over 16384 px on a side or an
  allocation over 512 MB, and answer `nil`. The clipboard is untrusted input.

## Behaviour to settle

1. **GNOME on Wayland.** arboard's Wayland path uses the data-control protocol
   (`ext-data-control-v1` / `wlr-data-control-unstable-v1`). My understanding is that
   GNOME's compositor (Mutter) does not implement it; arboard then falls back to X11 via
   XWayland, which can miss images copied in a Wayland-native app. **Not verified on real
   hardware.** Test on GNOME/Wayland, KDE/Wayland and Sway before claiming Wayland works;
   if GNOME copes badly, document that the native path is best-effort there and let apps
   fall back to `wl-paste`.
2. **The window has focus, not the terminal.** In the terminal frontend the app is not the
   focused Wayland client when the user pastes, so a compositor that gates clipboard reads
   on focus (some do for data-control) may refuse. Worth a test with `nest run` in a
   terminal, not only `--gui`.
3. **Formats.** arboard decodes what the source offers into RGBA. On macOS that is the
   pasteboard's TIFF/PNG; on Linux `image/png`. An app that puts only `image/jpeg` or
   `image/bmp` on the clipboard may come back `nil` on Linux. Say so in the docstring.
4. **Alpha.** A screenshot is opaque; a copied UI element may not be. Keep alpha in the PNG
   and do not flatten. The consumer (a model API) handles it.
5. **A copied file** (Finder, Files) puts a path or URI list on the clipboard, not pixels, so
   this returns `nil` and `clipboard-get` returns the path. Apps that want "paste a file"
   already have a path to read.
6. **Cost.** Reading the clipboard can be slow (the selection owner has to serialise a large
   image). It blocks the calling process; the doc should say to call it from a worker, not
   the UI loop, for big images.

## How Zubr will use it

Zubr's `src/images.blsp` handles Ctrl+V. It should try, in order:

1. `(os/clipboard-image 2000)`: native, no dependency;
2. the existing shell-outs (`wl-paste`, `xclip`, `pngpaste`) if that returned `nil` and the
   platform's tool exists (covers GNOME/Wayland, and runtimes built without the feature).

Either way the PNG is saved under `~/.cache/zubr/images/` and `[Image #n]` goes on the
prompt line. The model sees it as an `image_url` data URL. Nothing else in Zubr changes.

## Testing

- **Pure, runs everywhere:** the RGBA to PNG encode and downscale, as a Rust unit test on a
  small fixed buffer (decode the PNG back with `image` and compare pixels and size), plus
  the size guard.
- **Headless / no display / no feature:** `(os/clipboard-image)` returns `nil` and does not
  raise (like `clipboard-get`).
- **With a display:** a round trip, `clipboard-set-image` then `clipboard-image`, marked as a
  display test (skipped in CI without `WAYLAND_DISPLAY` / `DISPLAY`), the way the GUI dump
  tests already gate. The process-global clipboard makes these order dependent, so they
  must not run in the parallel green-process suite; run them serially by name.
- **By hand, per platform**, before merging (this is the part that can only be found by
  trying): copy a screenshot with the OS's own tool, then paste in Zubr. macOS
  (Cmd-Ctrl-Shift-4), GNOME (Print), KDE (Spectacle), Sway (`grim -` with `wl-copy`), X11
  (`maim | xclip -selection clipboard -t image/png`).

## Alternatives considered

- **Keep shelling out (status quo).** Works where the tool is installed; every app
  re-implements the three-way probe. The native path is small because the crates are
  already linked.
- **Return raw RGBA `{:width :height :rgba}`**, like `image-thumb`. Fine for rendering, but
  every consumer that wants a file or an API payload would need a PNG encoder, which Brood
  would then have to ship. Encoding once in the runtime is cheaper.
- **A general `clipboard-get` with a MIME type** (`(os/clipboard-get "image/png")`).
  More general, but arboard exposes typed accessors, not arbitrary MIME, so it would be a
  leaky abstraction over what it can actually do.
- **Windows.** arboard supports it, but it is untested here; the builtin should work, and
  the doc should not promise it until someone runs it.

## Acceptance

- On macOS, X11 and at least one Wayland compositor, copying a screenshot and calling
  `(os/clipboard-image)` returns a PNG that opens in an image viewer at the right size.
- On a runtime without the `clipboard` feature, and with an empty or text clipboard, it
  returns `nil`.
- `cargo tree -d` shows no duplicate `image`; the size change of the release binary is
  recorded in the ADR.
- The GNOME/Wayland behaviour is written down, whichever way it turns out.
