//! The editor's zones. Each panel reads state and documents and pushes
//! [`crate::UiIntent`]s; none of them edits a document.

pub mod canvas_viewport;
pub mod editor_toolbar;
pub mod gizmo;
pub mod layers_panel;
pub mod recent_tray;
pub mod right_sidebar;
