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
| `capturecraft-editor` | Studio editor (eframe + wgpu): `capturecraft-editor [image.png …]` |
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

`capturecraft-editor` is the studio editor (crate `capturecraft-app`, eframe +
wgpu). Turn on **Open in editor** (or `editor = true`) and every capture is
saved and then opened in it; the tray's **Open editor** opens the newest
capture. It also takes image files:

```sh
capturecraft-editor shot.png other.jpg
```

Layout, top to bottom:

- **Menu strip**: File, Edit, Image, Video, Share, View, Help and, in the
  middle, the document name (`2026-10-19_18-43-26.capturecraft ▾`) to switch
  between open captures.
- **Toolbar** (icon over label): Library, Assets │ Capture, Create ▾ on the
  left; Favorites, Arrow, Text, Callout, Shape, Stamp, Fill, Move, Selection,
  More ▾ │ Undo, Redo centred; Copy All and the blue **Share / Save** split
  button (PNG to the captures folder; the chevron offers JPG, WebP, SVG, PDF
  and Clipboard) on the right. More holds Step, Blur, Highlight,
  Pen, Eraser, Magnify, Spotlight, Cut Out, Smart Move, Simplify and Crop;
  the pin next to each puts it on the toolbar (right-click to unpin).
- **Drawer**: two stacked panel groups as in PhotoCraft, `Properties | Effects`
  above and `Layers | History` below, each with its own scroll bar. Drag the
  gap between them to resize; double-click a tab (or use the ≡ menu) to fold
  a group to its tab strip. View › Panels hides the drawer.
- **Properties**: with a callout or text selected, its text, size, colour and
  alignment on top. Below, Quick Style tiles and the property controls of
  the active tool (colour, outline, width, arrowheads, shadow direction,
  corner radius, tail, numbering). Right-click a Quick Style to add it to
  Favorites. The Selection tool shows Rectangle / Ellipse / Polygon / Lasso
  tiles and **Background Fill**: Transparent, or Auto Fill (inpainted by
  `capturecraft-ocr`) when the selection is moved or deleted.
- **Effects**: Border, Drop Shadow and Rounded Drop-Shadow Frame for
  the whole canvas, with the frame's settings.
- **Canvas**: the capture on a dark surround, with eight resize squares on
  its border to add padding.
- **Utility bar**: Hide/Show Recent, Tag, zoom (Fit, 25–400 %) and canvas size
  (Resize / Trim Canvas).
- **Recent Captures**: 130 × 85 thumbnails of the open captures; click to
  switch, × to close, right-click for Duplicate, Combine Images, Save As,
  Copy and Delete.

#### What works

| Tool or feature | How it works |
|-----------------|--------------|
| Arrow | Drag from tail to tip; drag the green handle to bend it |
| Callout | Click for a default bubble or drag its size; the yellow diamond aims the tail, the `+` adds another; double-click it (or use Properties) to type its text |
| Step | Click to drop the next number (1, 2, 3 / A, B, C / I, II, III) |
| Text | Click to type (or drag a box); Enter finishes, Shift+Enter breaks the line; double-click a text to edit it |
| Shape | Drag a rectangle or ellipse, or click the corners of a polygon (double-click or Enter closes it), in the selected Quick Style |
| Highlight | Drag over text for a translucent marker band (yellow, green, pink or blue) |
| Blur | Drag over private details to blur or pixelate them, non-destructively: pick one of four blur or four pixelate Quick Styles, or set Type and Intensity (1–100); both also change the selected blurs |
| Pen | Drag a freehand line, smoothed into curves the way PhotoCraft's Freeform Pen fits them |
| Eraser | Drag over the capture to erase it to transparency (size and hardness); annotations stay |
| Magnify | Click to drop a 1.5×–4× lens (or drag its size); move the source ring afterwards |
| Spotlight | Drag a rectangle or ellipse; everything around it is dimmed |
| Crop | Adjust the frame over the canvas (handles, move, draw a new one, Alt from the centre); Enter or double-click crops, Esc resets |
| Grab Text | Drag over text (a click reads the whole capture): the text appears in reading order or as on screen, to copy or to turn into editable text boxes |
| Move | Select, Shift+click to add, drag to move; handles resize (Shift keeps the aspect, Alt from the centre), rotate (Shift snaps to 15°) and round corners |
| Selection | Rectangle: drag to select part of the capture, drag inside it to move it, Delete to remove it; the hole is Transparent or Auto Fill |
| Cut Out | Drag across the capture to remove a strip and close the gap (straight or torn seam) |
| Simplify | Replaces text and detail with clean shapes; pick a palette tile |
| Quick Styles | Pick a style for the next annotation or apply it to the selection; edit colour, width, arrowheads, shadow, corner radius, tail and numbering; save your own; star favourites |
| Effects | Border, Drop Shadow, Rounded Drop-Shadow Frame (margin, corners, solid/linear/radial backdrop, shadow, border) |
| Canvas | Resize squares on the border add padding; Resize / Trim Canvas from the size button |
| Share / Save | PNG to the captures folder; JPG, WebP, SVG, PDF (every open capture, one per page) and Clipboard from the arrow; Copy All |
| Captures | Recent Captures strip (switch, close, Duplicate, Copy, Delete), Tag, New Blank Image, zoom, Undo / Redo |
| Files | File › Open, Save, Save As: `.capturecraft` documents keep every annotation, the edited pixels and the whole undo history; Save As also writes PNG, JPEG or WebP |
| Library | The open captures plus every image and `.capturecraft` document in the captures folders (Pictures/CaptureCraft, the tray's output folder, Share › File folders); double-click opens one |
| Layers / History | The drawer's lower group: hide, lock, rename (double-click; Enter saves, Esc cancels) and restack annotations; click any past step to go back to it |
| Right-click | Cut, Copy, Paste, Duplicate, Bring to Front / Forward, Send Backward / to Back, Group, Ungroup, Delete, Grab Text, Copy All, the selection's Quick Styles and a tool strip |
| Shortcuts | Ctrl+Z undo; Ctrl+Y or Ctrl+Shift+Z redo; Ctrl+C copies the selected annotations, or the whole image ("Copied to clipboard!") with nothing selected; Ctrl+X, Ctrl+V, Ctrl+D, Delete, Ctrl+G / Ctrl+Shift+G, Ctrl+A, Ctrl+O, Ctrl+S, Ctrl+Shift+S |
| Pointer | The canvas pointer carries the active tool's icon |

#### Not working yet

The buttons are there and say "not available yet" when used:

- **Tools**: Stamp, Fill and Smart Move. They can be selected and their
  Quick Styles edited, but they do not draw yet.
- **Selection**: Ellipse, Polygon and Freehand Lasso modes, Snap to Object.
- **Effects**: Torn Edge, Perspective / Shear, Spotlight and Color Filter.
- **Files**: Simplify's palette and exclude choices are not kept in a
  saved document (its shapes and heals are), and closing an unsaved capture
  does not ask to save it yet.
- **Other**: Assets, Create › Image from Template / Video from Images, the
  Video menu, Combine Images and Preferences.

#### Coming next

Roughly in this order:

1. **Stamp, Fill and the Smart Move tool**, and Combine Images.
2. **Ellipse, Polygon and Lasso selections** and Snap to Object.
3. **Torn Edge, Perspective, Spotlight and Color Filter effects.**
4. **A save prompt for unsaved captures**, autosave and recovery.
5. **Video and scrolling capture**, Preferences, and moving the tray, global
   hotkey and autostart into the new app.

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

The workspace holds the layered architecture that is replacing the
programs above; `capturecraft-editor` already runs on it.

```sh
cargo run -p capturecraft-app --release   # binary: capturecraft-editor
```

| Crate | Layer | What it is |
|-------|-------|------------|
| `capturecraft-format` | L5 | `.capturecraft` document: annotations (callouts, step badges, arrows, blur, magnifier, spotlight, cut outs, shapes, smart objects); rotation, linear/radial gradients and a rounded drop-shadow frame |
| `capturecraft-engine` | L5 | Command registry and session: every edit is an undoable command (`annotation.addStep`, `node.resize`, `node.rotate`, `canvas.setBackdrop`, `smart.simplify`, …) |
| `capturecraft-compositor` | L4 | CPU reference compositor and wgpu compositor with pixel parity (≤ 2/255) |
| `capturecraft-capture` | L6 | Capture backends, frozen-desktop hit testing, scrolling-capture stitcher |
| `capturecraft-ocr` | L6 | UI segmentation, inpainting and text recognition for the smart tools |
| `capturecraft-ui-egui` | L7 | Capture HUD, crosshair overlay and the studio editor: toolbar, drawer, canvas, utility bar, filmstrip, vector icons (PhotoCraft themes) |
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
- The studio editor described under [Editor](#editor).
- Canvas gizmo: 8 resize handles, a rotation handle (Shift snaps to 15°),
  yellow diamonds for callout tails and corner radius.
- Rendered on CPU and GPU alike: callout text and step badge numbers
  (photocraft-text), rotation, linear and radial gradients, feathered blur
  and spotlight edges, and the rounded drop-shadow frame with margin
  handles.

Not yet in the new app: tray, global hotkey and autostart (the programs
above provide them), preferences, video and scrolling capture.

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
