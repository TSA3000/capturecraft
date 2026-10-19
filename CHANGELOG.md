# Changelog

## v0.19.0 (2026-10-19)

- Preview editor (`capturecraft-next`) usable end to end: HUD, crosshair overlay, five-zone
  editor, Library, resize/rotate gizmo, rounded shadow frame with margin handles.
- Engine commands `node.resize`, `node.rotate`, `node.transform`, `canvas.setBackdrop`.
- Compositor draws text (photocraft-text), rotation, gradients, feathered edges and the
  shadow frame on CPU and GPU; GPU colour readback test against photocraft-cms.
- HiDPI: physical pixels vs logical points in the crosshair overlay.
- Shipped programs unchanged.

## v0.18.0 (2026-10-19)

- Layered workspace for the next-generation editor: format, engine, CPU + wgpu compositor
  with parity tests, capture, OCR, egui UI and an eframe + wgpu preview app
  (`capturecraft-next`, built from source only).
- Layered crates on eframe/egui 0.36, wgpu 30 and PhotoCraft v0.9.0 (theme tokens included).
- Shipped programs unchanged.

## v0.17.2 (2026-10-16)

- Fix NSIS paths and WiX component GUIDs.

## v0.17.1 (2026-10-16)

- Fix Rust 1.99 clippy lints; Windows installers.

## v0.17.0 (2026-10-16)

- Annotation editor, PhotoCraft themes and history, preferences popup, Windows installers
  (NSIS and MSI).

## v0.16.3 (2026-10-16)

- Annotation editor, PhotoCraft themes and history, menus, preferences popup.

## v0.10.3 (2026-10-16)

- Selection modes with window snapping, settings file, custom title bar, tray polish.

## v0.4.0 (2026-10-16)

- Config file, hotkey fallback, autostart, windowless tray, CI fixes.

## v0.3.0 (2026-10-16)

- Tray + global hotkey, multi-monitor overlay, magnifier, arrow-key nudge.

## v0.1.0 (2026-10-16)

- Cross-platform capture, CI and release builds.
