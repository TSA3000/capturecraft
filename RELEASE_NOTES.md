# v0.23.0

## Changes

- **Drawer in two groups**, like PhotoCraft's panels: `Properties | Effects` on top and
  `Layers | History` underneath, each with its own tabs and scroll bar. Drag the gap between
  them to give one more room; double-click a tab, or use the ≡ menu, to fold a group to its
  tab strip. The Effects / Properties / Layers / History tabs left the utility bar; View ›
  Panels shows or hides the drawer.
- **Text in callouts**: select a callout and type its text in Properties, with size, colour
  and left / centre / right alignment. Or double-click the callout on the canvas and type
  straight into the bubble (Enter finishes, Shift+Enter breaks the line). The text is drawn
  inside the bubble, wrapped to it.
- **Fix: renaming a layer** no longer gets stuck in the text box. Enter or a click elsewhere
  saves the name, Esc cancels.

## Upgrading

- Install over v0.22.0; nothing else to do. The drawer opens on Properties and Layers.

# v0.22.0

## Changes

- **Every annotation tool draws**: Text (click to type, Enter finishes, double-click to edit),
  Shape with a new Polygon mode, Pen (smooth freehand curves, PhotoCraft's Freeform Pen fit),
  Eraser (erases the capture to transparency), Magnify, Spotlight and Crop (PhotoCraft-style
  frame: handles, thirds guides, Enter to crop, Esc to reset).
- **Right-click menu** on the canvas: Cut, Copy, Paste, Duplicate, Bring to Front / Forward,
  Send Backward / to Back, Group, Ungroup, Delete, Grab Text, Copy All, the selection's Quick
  Styles and a tool strip.
- **Keyboard shortcuts**: Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z, Ctrl+C (selected annotations, or the
  whole image when nothing is selected), Ctrl+X, Ctrl+V, Ctrl+D, Delete, Ctrl+G,
  Ctrl+Shift+G, Ctrl+A, Ctrl+O, Ctrl+S, Ctrl+Shift+S. Messages appear as a toast at the bottom
  that fades out ("Copied to clipboard!").
- **Layers and History** tabs in the drawer: show/hide, lock, rename and drag to restack
  annotations; click any step in the history to go back or forward to it.
- **Save and open `.capturecraft` documents** (File › Open, Save, Save As) with native
  dialogs. A document keeps its annotations, the edited pixels, Smart Move objects and the
  whole undo/redo history; Save As can also write PNG, JPEG or WebP. The Library lists saved
  documents too.
- **Grab Text**: a new tool (on the toolbar and in the right-click menu). Drag over text to
  read it; copy it ("Text copied to clipboard!"), choose reading order or the on-screen column
  layout, or replace it with editable text boxes in one step.

## Upgrading

- Install over v0.21.0; nothing else to do.
- Grab Text reads words only when the text recognition models are installed
  (`cargo xtask models` for source builds); without them it finds the lines but cannot read
  them, and says so.
- A toolbar layout saved by an earlier version is kept, so Grab Text appears under More there;
  pin it from More to put it on the toolbar (new installs have it pinned).

# v0.21.0

## Changes

- **Shape tool**: drag a rectangle or an ellipse (Rectangle / Ellipse switch in Tool
  Properties) in the selected Quick Style, including its corner radius and shadow.
- **Highlight tool**: drag over text for a translucent marker band. Its Quick Styles are
  Yellow, Green, Pink and Blue.
- **Blur and Pixelate tools**: drag over private details. Blur has Light / Medium / Strong
  cards, Pixelate has Fine / Medium / Coarse block sizes.
- **Tool pointer**: on the canvas the pointer carries a small badge with the active tool's
  icon.
- **Library shows the captures on disk**: besides the open captures it now lists every image
  in the captures folders (Pictures/CaptureCraft, the tray app's output folder and any
  Share › File folder), newest first, with thumbnails loaded in the background. Double-click
  one to open it in the editor. Delete in the Library only closes open captures; it never
  deletes files.
- **Engine**: new `annotation.addShape` (rect/ellipse, fill, stroke, shadow, multiply blend
  for highlighters) and `annotation.addBlur` (gaussian or pixelate) commands.

## Upgrading

- Install over v0.20.x; nothing else to do.
- Highlight Quick Styles saved by an earlier version keep their old colours (red, orange,
  blue, black). To get the new marker colours, reset the editor's saved layout or add your
  own Quick Style.

# v0.20.1

## Changes

- Fix: the editor's right drawer (Tool Properties / Effects) no longer slides off the right
  edge of the window. Its sliders and dropdowns now fit the drawer, and the canvas no longer
  covers the drawer's left side.
- Fix: the arrow after the document name in the menu strip is drawn instead of showing as a
  box.
- README: what works in the new editor, what doesn't yet, and what's coming next.

## Upgrading

- Install over v0.20.0; nothing else to do.

# v0.20.0

## Changes

- **New editor**: `capturecraft-editor` is now the studio editor built on the layered crates
  (eframe + wgpu). The old editor with its row of text buttons is gone. The tray's
  **Open editor** and **Open in editor** start the new one.
  - Menu strip (File, Edit, Image, Video, Share, View, Help) and the document name.
  - 58 px toolbar of icon-over-label buttons: Library, Assets, Capture, Create ▾ │ Favorites,
    Arrow, Text, Callout, Shape, Stamp, Fill, Move, Selection, More ▾ │ Undo, Redo │ Copy All
    and the blue Share / Save split button. Tools in More can be pinned to the toolbar.
  - Right drawer: Tool Properties (Quick Style tiles and controls; the Selection tool's
    Rectangle mode with a Transparent or Auto Fill background) or Effects (Border, Drop Shadow,
    Rounded Drop-Shadow Frame).
  - Utility bar: Hide/Show Recent, Tag, zoom, canvas size (Resize / Trim Canvas), Effects /
    Properties tabs.
  - Recent Captures filmstrip with real thumbnails, a × to close, and Duplicate / Copy /
    Delete in the context menu.
  - Vector icons for every button, and the dark studio palette in the Studio theme.
  - Opens image files given on the command line; New Blank Image.
- **Engine**: new `document.setTags` command; Smart Move/Delete take
  `background: "transparent"`; `Session::close` and `Session::duplicate`; canvas-to-base
  rectangle mapping.
- **The app installs the UI analyzer**, so Simplify, Smart Move and Auto Fill work there.
- Fix: every capture opened in the new app gets its own document id, so captures no longer
  collide in the Recent Captures strip.
- Not available yet in the new editor: drawing with Text, Shape, Stamp, Fill, Blur, Pixelate,
  Highlight, Pen, Eraser, Magnify, Spotlight and Crop; the Save As dialog; saving
  `.capturecraft` files. The controls for these say so.

## Upgrading

- The old editor's Box, Highlight, Pixelate, Blur and Crop tools are not in the new editor
  yet. Use v0.19.0's `capturecraft-editor` if you need them.
- Release archives and installers ship the new `capturecraft-editor` under the same name and
  path.

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
- Release builds: the Windows installer step no longer fails when Chocolatey's feed is down
  (it uses the runner's NSIS, retries Chocolatey, then downloads NSIS directly). This is what
  left v0.18.0 without its x64 Windows files.

## Upgrading

- Nothing to do for the shipped programs.
- The preview is still built from source only: `cargo run -p capturecraft-app --release`
  (Rust 1.95+).
- Preview gaps: tray, global hotkey, autostart, copy, share, preferences, thumbnails, video
  and scrolling capture. On desktops whose monitors use different scales the overlay is exact
  on the monitor egui reports; the size badge is exact everywhere.
