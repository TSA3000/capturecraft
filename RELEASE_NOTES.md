# v0.18.0

## Changes

- **The shipped programs are unchanged**: `capturecraft-app`, `capturecraft-editor`,
  `capturecraft-tray` and `capturecraft` work as in v0.17.2.
- **New layered workspace** behind them, the base of the next-generation editor:
  - `capturecraft-format`: the `.capturecraft` document (callouts with tails, step badges,
    curved arrows, blur/pixelate, magnifier, spotlight, cut outs, shapes, smart objects),
    with rotation, linear and radial gradients and a rounded drop-shadow frame in the schema.
  - `capturecraft-engine`: every edit is an undoable, scriptable command; copy-on-write raster
    layers; Smart Move and Simplify.
  - `capturecraft-compositor`: CPU reference compositor and wgpu compositor, kept within
    2/255 of each other by parity tests.
  - `capturecraft-capture` (backends, frozen-desktop hit testing, scrolling-capture stitcher)
    and `capturecraft-ocr` (UI segmentation, inpainting, text recognition).
  - `capturecraft-ui-egui`: capture HUD, crosshair overlay, editor panels, PhotoCraft themes.
  - `capturecraft-app` (binary `capturecraft-next`): an eframe + wgpu preview hosting the
    editor, the floating capture HUD and the crosshair overlay on one GPU device.
- The layered crates use eframe/egui 0.36 and wgpu 30, and PhotoCraft v0.9.0's crates
  (geom, raster, algo, vector, text, cms, ui-egui theme tokens).

## Upgrading

- Nothing to do: settings (`config.toml`), captures and installers are as before.
- The preview is not in the archives or installers. Build it from source with
  `cargo run -p capturecraft-app --release` (Rust 1.95+).
- Known gaps in the preview: callout text, step badge numbers, rotation, gradients,
  feathering and the shadow frame are not drawn yet; video and scrolling capture are not
  wired up.
