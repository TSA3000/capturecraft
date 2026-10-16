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

- Default hotkey: **Print Screen** (Windows/Linux), **Cmd+Shift+2** (macOS).
- Captures go to `<Pictures>/CaptureCraft/capture-YYYY-MM-DD_HH-MM-SS.png` and the clipboard.
- Left-click the tray icon to capture; right-click for the menu.
- Windows 11: if Print Screen is taken by Snipping Tool, turn off
  *Settings → Accessibility → Keyboard → Use the Print screen key to open screen capture*,
  or use `--hotkey`.
- Linux: global hotkeys work on X11 only; the tray uses StatusNotifierItem (KDE, GNOME with AppIndicator extension).

## License

Apache-2.0
