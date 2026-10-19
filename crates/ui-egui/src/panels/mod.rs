//! The editor's zones. Each panel reads state and documents and pushes
//! [`crate::UiIntent`]s; none of them edits a document.

pub mod canvas_viewport;
pub mod recent_tray;
pub mod right_sidebar;
