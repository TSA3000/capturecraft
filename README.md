# CaptureCraft

Open-source screen capture and annotation tool written in pure Rust.

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
  Capture · Open last capture · Open captures folder · Edit settings… · Reload settings · Start with Windows / Open at login · Quit
- Only one tray instance runs at a time. **Quit** closes the window too.
- Linux: global hotkeys work on X11 only; the tray uses StatusNotifierItem.

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

## License

Apache-2.0
