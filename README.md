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
cargo build --release
cargo test -- --include-ignored   # includes the real screen-capture test
```

Linux build dependencies (Debian/Ubuntu):

```sh
sudo apt-get install pkg-config libclang-dev libxcb1-dev libxrandr-dev libdbus-1-dev libpipewire-0.3-dev libwayland-dev libegl-dev libxkbcommon-dev libgbm-dev
```

## Usage

```sh
capturecraft                          # freeze all screens, drag to select → capture.png + clipboard
capturecraft shot.png
capturecraft --tray                   # background mode: tray icon + global hotkey
capturecraft --tray --hotkey ctrl+shift+s
capturecraft --full                   # whole primary screen, no overlay
capturecraft --region 100,100,640,480 # fixed region, no overlay
capturecraft --no-clipboard
```

### Overlay

| Input | Action |
|-------|--------|
| Drag | Select a region (also across monitors) |
| Click | Capture the whole monitor under the cursor |
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
- Left-click the tray icon to capture; right-click for the menu:
  Capture · Open captures folder · Edit settings… · Reload settings · Start with Windows / Open at login · Quit
- Only one tray instance runs at a time.
- Linux: global hotkeys work on X11 only; the tray uses StatusNotifierItem.

### Settings

`config.toml` is created on first run:

| OS | Path |
|----|------|
| Windows | `%APPDATA%\CaptureCraft\config.toml` |
| macOS | `~/Library/Application Support/CaptureCraft/config.toml` |
| Linux | `~/.config/CaptureCraft/config.toml` |

```toml
hotkeys = ["PrintScreen", "ctrl+shift+s"]
output_dir = "C:\\Users\\you\\Pictures\\CaptureCraft"
clipboard = true
```

Command-line flags (`--hotkey`, `--out-dir`, `--no-clipboard`) override it.

## License

Apache-2.0
