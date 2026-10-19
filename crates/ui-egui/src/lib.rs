//! The CaptureCraft egui shell (L7). Only this layer may use egui, eframe
//! or winit; it draws [`state`] and turns every user action into an engine
//! command.
//!
//! Panels and views never touch a document. They read [`state::AppState`]
//! and the engine's documents, change pure view state (zoom, which tray
//! items are selected, the Quick Style draft) directly, and push
//! [`UiIntent`]s for everything else. The app hands the intents to
//! [`dispatch`], which runs each [`UiIntent::Command`] through
//! `Session::run`, so every click and drag is undoable, scriptable and
//! reachable from the CLI and MCP by the same command id.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod dpi;
pub mod panels;
pub mod state;
pub mod theme;
pub mod views;

use capturecraft_engine::{EngineError, Session};
use serde_json::Value;

/// What a panel asks the app to do this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiIntent {
    /// Run an engine command by id.
    Command { id: &'static str, params: Value },
    /// Show another open document (Recent Tray click): no save prompt.
    ActivateDocument { doc_id: String },
    /// Open the Combine Images wizard with these documents.
    CombineImages { doc_ids: Vec<String> },
    /// Start a new capture (toolbar, tray).
    NewCapture,
    /// Capture now with the HUD's settings: open the crosshair overlay for
    /// `selection` after `delay_secs`.
    StartCapture {
        tab: state::CaptureTab,
        selection: state::SelectionType,
        delay_secs: u32,
    },
    /// Hide the floating capture HUD.
    CloseHud,
    /// Bring up the editor window (HUD, tray menu).
    ShowEditor,
    /// Open the preferences window.
    OpenSettings,
    /// Copy the active document's rendered image to the clipboard.
    CopyImage,
    /// Share/export the active document (file, PDF, upload).
    Share,
    /// Close these documents and drop them from the library (the app asks
    /// before discarding unsaved ones).
    DeleteDocuments { doc_ids: Vec<String> },
}

impl UiIntent {
    pub fn command(id: &'static str, params: Value) -> Self {
        UiIntent::Command { id, params }
    }
}

/// The outcome of one dispatched intent.
#[derive(Debug)]
pub enum Dispatched {
    Ran {
        id: &'static str,
        result: Value,
    },
    Failed {
        id: &'static str,
        error: EngineError,
    },
    /// Not an engine command: the app handles it (window, wizard, capture).
    Shell(UiIntent),
}

/// Runs the frame's commands in order; a failed command leaves the
/// document unchanged (the engine rolls back) and does not stop the rest.
pub fn dispatch(session: &mut Session, intents: Vec<UiIntent>) -> Vec<Dispatched> {
    intents
        .into_iter()
        .map(|i| match i {
            UiIntent::Command { id, params } => match session.run(id, &params) {
                Ok(result) => Dispatched::Ran { id, result },
                Err(error) => Dispatched::Failed { id, error },
            },
            UiIntent::ActivateDocument { doc_id } => {
                let index = session
                    .documents()
                    .iter()
                    .position(|d| d.manifest.id == doc_id);
                match index.map(|i| session.activate(i)) {
                    Some(Ok(())) => Dispatched::Ran {
                        id: "ui.activateDocument",
                        result: Value::Null,
                    },
                    Some(Err(error)) => Dispatched::Failed {
                        id: "ui.activateDocument",
                        error,
                    },
                    None => Dispatched::Failed {
                        id: "ui.activateDocument",
                        error: EngineError::NoDocument,
                    },
                }
            }
            other => Dispatched::Shell(other),
        })
        .collect()
}

/// Every engine command id the panels emit; a test checks that the
/// registry has each of them.
pub const COMMANDS_USED: &[&str] = &[
    "edit.undo",
    "edit.redo",
    "annotation.addArrow",
    "annotation.addStep",
    "annotation.renumberSteps",
    "annotation.addCallout",
    "annotation.addCalloutTail",
    "annotation.moveCalloutTail",
    "annotation.move",
    "annotation.setArrowCurve",
    "annotation.setStyle",
    "canvas.cutOut",
    "canvas.expandBounds",
    "smart.simplify",
    "smart.setPalette",
    // The transform gizmo and the frame UI (`panels::gizmo`).
    "node.resize",
    "node.rotate",
    "canvas.setBackdrop",
    "annotation.setMagnifierSource",
];

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_engine::Pixels;
    use capturecraft_format::Manifest;
    use photocraft_geom::Size;
    use serde_json::json;

    #[test]
    fn every_emitted_command_is_registered() {
        let ids: Vec<_> = capturecraft_engine::registry()
            .iter()
            .map(|c| c.id)
            .collect();
        for id in COMMANDS_USED {
            assert!(ids.contains(id), "{id}");
        }
    }

    #[test]
    fn dispatch_runs_commands_and_passes_shell_intents_on() {
        let mut s = Session::new();
        let size = Size::new(40, 30);
        let mut a = Manifest::new(size);
        a.id = "a".into();
        let mut b = Manifest::new(size);
        b.id = "b".into();
        s.open(a, Pixels::new(size, vec![255; 40 * 30 * 4]))
            .unwrap();
        s.open(b, None).unwrap();
        let out = dispatch(
            &mut s,
            vec![
                UiIntent::ActivateDocument { doc_id: "a".into() },
                UiIntent::command("annotation.addStep", json!({ "at": { "x": 5, "y": 5 } })),
                UiIntent::command("annotation.addStep", json!({ "bogus": 1 })),
                UiIntent::NewCapture,
            ],
        );
        assert!(matches!(out[0], Dispatched::Ran { .. }));
        assert!(matches!(out[1], Dispatched::Ran { .. }));
        assert!(matches!(out[2], Dispatched::Failed { .. }));
        assert!(matches!(out[3], Dispatched::Shell(UiIntent::NewCapture)));
        let active = s.active().unwrap();
        assert_eq!(active.manifest.id, "a");
        assert_eq!(active.manifest.annotations.len(), 1);
    }
}
