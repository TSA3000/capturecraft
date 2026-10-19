# v0.19.0

## Changes

- **The shipped programs are unchanged**: `capturecraft-app`, `capturecraft-editor`,
  `capturecraft-tray` and `capturecraft` work as in v0.18.0.
- **Preview editor (`capturecraft-next`) is now usable end to end**:
  - Capture HUD → crosshair overlay over the frozen desktop → the capture opens in the
    five-zone editor (toolbar, canvas, sidebar, Recent Tray, Library).
  - Canvas gizmo: 8 resize handles, a rotation handle (Shift snaps to 15°), yellow diamonds
    for callout tails and corner radius. Rotated nodes resize around the opposite handle.
  - Rounded drop-shadow frame around the screenshot, with margin drag handles and a sidebar
    section (solid, linear or radial backdrop, corner radius, shadow).
- **Engine**: new commands `node.resize`, `node.rotate`, `node.transform` and
  `canvas.setBackdrop`, each one undo step.
- **Compositor** (CPU and wgpu, identical output on the parity scenes):
  - callout text and step badge numbers/letters shaped with photocraft-text;
  - rotation and scaled smart objects;
  - linear and radial gradients;
  - feathered (signed-distance) edges for blur regions and spotlights;
  - the rounded shadow frame (photocraft-vector clip, photocraft-algo Gaussian shadow).
- **Crosshair overlay**: physical pixels and logical points are separate types, so the size
  badge shows physical pixels and the crosshair stays on the cursor on scaled (HiDPI) screens.
- **Colour**: a GPU readback test checks #FF5733, #1E293B and #F8FAFC within 1/255 against
  photocraft-cms; the canvas texture stays `Rgba8Unorm` (no double gamma in egui).
- Fix: PhotoCraft's fonts are installed before its theme is applied (headings could panic).

## Upgrading

- Nothing to do for the shipped programs.
- The preview is still built from source only: `cargo run -p capturecraft-app --release`
  (Rust 1.95+).
- Preview gaps: tray, global hotkey, autostart, copy, share, preferences, thumbnails, video
  and scrolling capture. On desktops whose monitors use different scales the overlay is exact
  on the monitor egui reports; the size badge is exact everywhere.
