# CaptureCraft

[![release](https://img.shields.io/github/v/release/TSA3000/capturecraft?label=release)](https://github.com/TSA3000/capturecraft/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/TSA3000/capturecraft/ci.yml?branch=main&label=CI)](https://github.com/TSA3000/capturecraft/actions/workflows/ci.yml)
[![downloads](https://img.shields.io/github/downloads/TSA3000/capturecraft/total?label=downloads)](https://github.com/TSA3000/capturecraft/releases)
[![license](https://img.shields.io/github/license/TSA3000/capturecraft?label=license)](LICENSE)
[![rust](https://img.shields.io/badge/rust-1.89%2B-orange?logo=rust)](https://www.rust-lang.org)

Open-source screen capture and annotation tool written in pure Rust.

Release notes: [RELEASE_NOTES.md](RELEASE_NOTES.md) · history: [CHANGELOG.md](CHANGELOG.md)

## Platforms

| OS      | x86_64 | ARM64 | Notes |
|---------|--------|-------|-------|
| Windows | ✅     | ✅    | Windows 8.1+ |
| macOS   | ✅     | ✅    | Grant **Screen Recording** permission (System Settings → Privacy & Security) |
| Linux   | ✅     | ✅    | X11 fully supported; Wayland partial |

## Build

```sh
# builds all three binaries
cargo build --release
cargo test -- --include-ignored   # includes the real screen-capture test
```

Linux build dependencies (Debian/Ubuntu):

```sh
sudo apt-get install pkg-config libclang-dev libxcb1-dev libxrandr-dev libdbus-1-dev libpipewire-0.3-dev libwayland-dev libegl-dev libxkbcommon-dev libgbm-dev
```

## Programs

| Binary | What it is |
|--------|-----------|
| `capturecraft-app` | Main window (own title bar): Capture button, hotkey, clipboard, autostart, save folder |
| `capturecraft-editor` | Standalone annotation editor: `capturecraft-editor [image.png]` (no path = newest capture) |
| `capturecraft-tray` | Background tray + global hotkey (no console; used by autostart) |
| `capturecraft` | Command line: overlay, `--full`, `--region`, `--tray` |

Settings changed in the window are saved to `config.toml` immediately; a running tray reloads them within ~2 s.

## Usage

```sh
capturecraft                          # freeze all screens, drag to select → output folder + clipboard
capturecraft shot.png                 # save to a specific file
capturecraft --delay-ms 2000          # wait 2 s first
capturecraft --tray                   # background mode: tray icon + global hotkey
capturecraft --tray --hotkey ctrl+shift+s
capturecraft --full                   # whole primary screen, no overlay
capturecraft --region 100,100,640,480 # fixed region, no overlay
capturecraft --no-clipboard
```

### Selection modes

Chosen in the window (**Selection** dropdown) or `selection` in `config.toml`:

| Mode | Behaviour |
|------|-----------|
| Region | The window under the cursor is framed and a click captures it. After 250 ms without movement the magnifier appears for pixel-exact work; dragging always selects a free region |
| Window | Click a window |
| Full screen | Whole primary screen, no overlay |

### Overlay

| Input | Action |
|-------|--------|
| Drag | Select a region (also across monitors) |
| Hover | Highlights the window under the cursor |
| Click | Capture the highlighted window (or the monitor if there is none) |
| Arrow keys | Move cursor 1 px (Shift: 10 px), also while dragging |
| Enter | Finish while dragging |
| Esc / right-click | Cancel |

A magnifier with the selection size (or cursor coordinates) follows the cursor.

### Tray mode

```sh
capturecraft --tray       # with console output
capturecraft-tray         # windowless (used by autostart)
```

- Hotkeys are tried in order — default **Print Screen**, then **Ctrl+Shift+S**
  (macOS: **Cmd+Shift+2**, then Ctrl+Shift+S). The active one is shown in the tray menu.
- Captures go to `<Pictures>/CaptureCraft/capture-YYYY-MM-DD_HH-MM-SS.png` and the clipboard.
- Double-click the tray icon to open the window; right-click for the menu:
  Capture · Open editor · Open last capture · Open captures folder · Edit settings… · Reload settings · Start with Windows / Open at login · Quit
- Only one tray instance runs at a time. **Quit** (tray menu, or File › Quit CaptureCraft in either window) closes the capture window, any editor windows and the tray. **Esc** closes just the window you are in.
- Linux: global hotkeys work on X11 only; the tray uses StatusNotifierItem.

### Editor

The editor is its own program. Turn on **Open in editor** (or `editor = true`)
and every capture is saved and then opened in it; **Save** writes back to the
same file. It can also be started on its own:

```sh
capturecraft-editor               # newest capture
capturecraft-editor shot.png
```


| Tool | What it does |
|------|--------------|
| Arrow | Arrow with a filled head |
| Box | Rectangle outline |
| Highlight | Translucent marker |
| Pixelate | Blocks out faces, names, keys |
| Blur | Gaussian blur (PhotoCraft's kernel and two-pass convolution) |
| Step | Numbered badge, counts up by itself |
| Crop | Trims the image; annotations move with it |

Colour swatches, a size slider, undo/redo (Ctrl+Z / Ctrl+Y), **Save** (Ctrl+S),
**Copy** (Ctrl+C) and Esc to close.

Built on PhotoCraft v0.9.0 (storytold/photocraft), the sibling craft app:
`photocraft-geom` is a real dependency, so coordinates are their types; the
history is their `photocraft-ops` model (labelled snapshots per step, capped
by state count, redo cleared on the next edit — the Edit menu shows
"Undo Arrow", "Redo Crop"); the blur is their `photocraft-algo` Gaussian
kernel and separable two-pass convolution; and the themes are their design
tokens. The engine/UI split is theirs too: `editor.rs` has no egui in it.

### Menus and preferences

Both windows follow PhotoCraft's menu grammar. The main window shows only the Capture button, the selection mode, clipboard,
editor and save folder; the hotkey sits under the button and is clicked to
record a new one. Everything else lives in Preferences. Its menus are **File**
(Capture, Open last capture, Open captures folder, Exit), **Edit**
(Preferences, Edit config.toml), **View** (Theme), **Tools** (Open editor,
Start tray) and **Help** (GitHub, Report an issue, About). The editor adds
Save, Save as, Copy, Undo and Redo to File and Edit.

**Edit › Preferences** opens a modal popup inside the window: a section list
beside a page of settings, the shape PhotoCraft's Preferences uses, with the
sections Snagit keeps. The window behind it is dimmed and inert until it
closes (Esc, Close, or a click outside):

| Section | Settings |
|---------|----------|
| Capture | Selection mode, clipboard, open in editor, zoom delay, dimming |
| Save | Output folder |
| Interface | Theme, selection colour, border width, magnifier zoom |
| Hotkeys | Capture hotkey and its fallback |
| Advanced | Start tray with app, start with Windows, capture delay, config.toml |

Changes are written to `config.toml` immediately and a running tray picks
them up within about two seconds.

### Themes

Four palettes ported verbatim from PhotoCraft's design system: **Pro (Dark)**,
**Pro (Medium Gray)**, **Studio (Dark)** and **Studio (Light)**. Pick one from
View › Theme, from Preferences › Interface, or the selector in the editor's
menu bar; it is saved as `theme` in `config.toml`.

### Settings

`config.toml` is created on first run:

| OS | Path |
|----|------|
| Windows | `%APPDATA%\CaptureCraft\config.toml` |
| macOS | `~/Library/Application Support/CaptureCraft/config.toml` |
| Linux | `~/.config/CaptureCraft/config.toml` |

```toml
selection = "region"
hotkeys = ["PrintScreen", "ctrl+shift+s"]
output_dir = "C:\\Users\\you\\Pictures\\CaptureCraft"
clipboard = true
start_tray = true      # start the background tray when the window opens
editor = false         # open captures in the annotation editor first
theme = "proMedium"    # pro | proMedium | studio | studioLight

[overlay]
zoom_delay_ms = 250    # stillness before the magnifier appears (0 = always)
hide_delay_ms = 300    # pause after hiding the window, before freezing
dim = 140              # brightness outside the selection, 0-255
dim_on_drag = true     # Region mode: dim only while dragging, not while hovering
border_color = "#3B82F6"
border_width = 2
zoom = 8               # magnifier pixel size
zoom_radius = 7        # magnifier shows (2r+1)^2 pixels
zoom_offset = 24       # distance from cursor to magnifier
label_scale = 2        # size badge text scale
min_drag = 3           # smaller drags count as a click
min_window = 20        # smaller windows are not snap targets
```

Command-line flags (`--hotkey`, `--out-dir`, `--no-clipboard`) override it.

## Next-generation editor (preview)

The workspace also holds the layered architecture that will replace the
programs above. It is a preview: it is not in the release archives or the
installers yet, and the programs above are unchanged.

```sh
cargo run -p capturecraft-app --release   # binary: capturecraft-next
```

| Crate | Layer | What it is |
|-------|-------|------------|
| `capturecraft-format` | L5 | `.capturecraft` document: annotations (callouts, step badges, arrows, blur, magnifier, spotlight, cut outs, shapes, smart objects); rotation, linear/radial gradients and a rounded drop-shadow frame |
| `capturecraft-engine` | L5 | Command registry and session: every edit is an undoable command (`annotation.addStep`, `node.resize`, `node.rotate`, `canvas.setBackdrop`, `smart.simplify`, …) |
| `capturecraft-compositor` | L4 | CPU reference compositor and wgpu compositor with pixel parity (≤ 2/255) |
| `capturecraft-capture` | L6 | Capture backends, frozen-desktop hit testing, scrolling-capture stitcher |
| `capturecraft-ocr` | L6 | UI segmentation, inpainting and text recognition for the smart tools |
| `capturecraft-ui-egui` | L7 | Capture HUD, crosshair overlay and the five-zone editor (PhotoCraft themes) |
| `capturecraft-app` | app | eframe + wgpu shell hosting the editor, the floating HUD and the overlay on one GPU device |

Everything below the UI is built on the PhotoCraft crates (v0.9.0):
`photocraft-geom`, `-raster` (256 × 256 copy-on-write tiles), `-algo`,
`-vector`, `-text`, `-cms` and the `-ui-egui` theme tokens. The UI stack is
eframe/egui 0.36 with wgpu 30 (Rust 1.95+). Layering is checked by
`cargo xtask layers`.

What the preview does today:

- Capture HUD (borderless, always on top) → the windows hide → crosshair
  overlay over the frozen desktop (physical-pixel size badge, loupe) →
  the capture opens in the editor.
- Five-zone editor: toolbar, canvas, Quick Styles sidebar, Recent Tray and
  the Library view.
- Canvas gizmo: 8 resize handles, a rotation handle (Shift snaps to 15°),
  yellow diamonds for callout tails and corner radius.
- Rendered on CPU and GPU alike: callout text and step badge numbers
  (photocraft-text), rotation, linear and radial gradients, feathered blur
  and spotlight edges, and the rounded drop-shadow frame with margin
  handles.

Not yet: tray, global hotkey and autostart (use the programs above), copy,
share, preferences, thumbnails, video and scrolling capture.

Checks (same as CI):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p capturecraft-compositor --features gpu --tests   # CPU/GPU parity
cargo xtask layers
```

## License

Apache-2.0
