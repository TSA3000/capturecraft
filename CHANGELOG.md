# Changelog

## v0.22.0 (2026-10-19)

- Text, Polygon, Pen, Eraser, Magnify, Spotlight and Crop tools draw.
- Canvas right-click menu; Ctrl+Z / Ctrl+Y / Ctrl+C and the other edit shortcuts; a
  fading toast for messages.
- Layers and History drawer tabs.
- `.capturecraft` save and open (with history) through native dialogs; Save As exports images.
- Grab Text tool and modal (Copy Text, Replace as Editable Text).
- Engine: annotation.delete, arrange, reorder, setProps, group, ungroup, paste, duplicate,
  addText, setText, addPen, addMagnifier, addSpotlight; canvas.crop, canvas.erase; edit.jump;
  ocr.replaceLines; Session::save_archive / open_archive. Format: pen annotations, node groups,
  the edited raster layer, the archive container.

## v0.21.0 (2026-10-19)

- Shape (rectangle / ellipse), Highlight, Blur and Pixelate tools draw on the canvas.
- Engine: `annotation.addShape`, `annotation.addBlur`.
- The canvas pointer shows the active tool's icon.
- Library lists the image files in the captures folders, not only the open captures.

## v0.20.1 (2026-10-19)

- Fix: the editor's right drawer no longer overflows past the window edge.
- Fix: the menu strip's document arrow is drawn (the UI font has no ▾).
- README lists working, missing and upcoming editor tools.

## v0.20.0 (2026-10-19)

- `capturecraft-editor` is the new studio editor (menu strip, icon-over-label toolbar,
  Tool Properties / Effects drawer, utility bar, Recent Captures filmstrip, vector icons);
  the old 7-button editor is removed.
- Selection tool (rectangle) with Transparent / Auto Fill background; Effects tiles for
  border, drop shadow and rounded shadow frame; tags; canvas resize and trim.
- Engine: `document.setTags`, transparent background for Smart Move/Delete,
  `Session::close` / `Session::duplicate`.
- The app installs the UI analyzer; captures get unique document ids.

## v0.19.0 (2026-10-19)

- Preview editor (`capturecraft-next`) usable end to end: HUD, crosshair overlay, five-zone
  editor, Library, resize/rotate gizmo, rounded shadow frame with margin handles.
- Engine commands `node.resize`, `node.rotate`, `node.transform`, `canvas.setBackdrop`.
- Compositor draws text (photocraft-text), rotation, gradients, feathered edges and the
  shadow frame on CPU and GPU; GPU colour readback test against photocraft-cms.
- HiDPI: physical pixels vs logical points in the crosshair overlay.
- Release workflow: robust NSIS install (runner copy, Chocolatey retries, direct download).
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
