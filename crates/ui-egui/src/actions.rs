//! Edit actions shared by the keyboard shortcuts, the Edit menu and the
//! canvas right-click menu: each turns the selection into engine commands
//! (or app intents) the same way wherever it was triggered from.

use capturecraft_engine::DocState;
use capturecraft_engine::node_commands::expand_groups;
use serde_json::{Value, json};

use crate::UiIntent;
use crate::shortcuts::EditorKey;
use crate::state::{EditorState, ToolId};

/// An edit action on the active document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    Delete,
    Duplicate,
    Group,
    Ungroup,
    SelectAll,
    BringToFront,
    BringForward,
    SendBackward,
    SendToBack,
    CopyAll,
    Save,
    SaveAs,
    Open,
}

impl From<EditorKey> for EditAction {
    fn from(k: EditorKey) -> Self {
        match k {
            EditorKey::Undo => EditAction::Undo,
            EditorKey::Redo => EditAction::Redo,
            EditorKey::Copy => EditAction::Copy,
            EditorKey::Cut => EditAction::Cut,
            EditorKey::Paste => EditAction::Paste,
            EditorKey::Delete => EditAction::Delete,
            EditorKey::Duplicate => EditAction::Duplicate,
            EditorKey::Group => EditAction::Group,
            EditorKey::Ungroup => EditAction::Ungroup,
            EditorKey::SelectAll => EditAction::SelectAll,
            EditorKey::Save => EditAction::Save,
            EditorKey::SaveAs => EditAction::SaveAs,
            EditorKey::Open => EditAction::Open,
        }
    }
}

/// How far each paste moves the copies, canvas pixels.
const PASTE_OFFSET: f64 = 16.0;

fn ids(editor: &EditorState) -> Vec<u64> {
    editor.canvas.selection.iter().map(|n| n.0).collect()
}

/// Whether `a` can run now (menus grey out the rest).
pub fn enabled(a: EditAction, editor: &EditorState, doc: Option<&DocState>) -> bool {
    let has_sel = !editor.canvas.selection.is_empty();
    match a {
        EditAction::Undo => doc.is_some_and(DocState::can_undo),
        EditAction::Redo => doc.is_some_and(DocState::can_redo),
        EditAction::Paste => doc.is_some() && !editor.canvas.clipboard.is_empty(),
        EditAction::Group => doc.is_some() && editor.canvas.selection.len() >= 2,
        EditAction::Copy | EditAction::Delete => {
            doc.is_some() && (has_sel || editor.canvas.marquee.is_some())
        }
        EditAction::Cut
        | EditAction::Duplicate
        | EditAction::Ungroup
        | EditAction::BringToFront
        | EditAction::BringForward
        | EditAction::SendBackward
        | EditAction::SendToBack => doc.is_some() && has_sel,
        EditAction::Open => true,
        EditAction::SelectAll | EditAction::CopyAll | EditAction::Save | EditAction::SaveAs => {
            doc.is_some()
        }
    }
}

/// The intents `a` runs; updates the selection and the annotation
/// clipboard (view state) directly.
pub fn run(a: EditAction, editor: &mut EditorState, doc: Option<&DocState>) -> Vec<UiIntent> {
    let Some(doc) = doc else {
        return match a {
            EditAction::Open => vec![UiIntent::OpenDialog],
            _ => Vec::new(),
        };
    };
    let m = &doc.manifest;
    let sel = ids(editor);
    let arrange = |to: &str| {
        (!sel.is_empty())
            .then(|| UiIntent::command("annotation.arrange", json!({ "ids": sel, "to": to })))
            .into_iter()
            .collect()
    };
    match a {
        EditAction::Undo if doc.can_undo() => vec![UiIntent::command("edit.undo", json!({}))],
        EditAction::Redo if doc.can_redo() => vec![UiIntent::command("edit.redo", json!({}))],
        EditAction::Undo | EditAction::Redo => Vec::new(),
        EditAction::Copy | EditAction::Cut => {
            if sel.is_empty() {
                // Nothing selected: Copy is Copy All (Cut does nothing).
                return if a == EditAction::Copy {
                    vec![UiIntent::CopyImage]
                } else {
                    Vec::new()
                };
            }
            editor.canvas.clipboard = m
                .annotations
                .iter()
                .filter(|n| sel.contains(&n.id.0))
                .filter_map(|n| serde_json::to_value(n).ok())
                .collect();
            editor.canvas.pastes = 0;
            let n = editor.canvas.clipboard.len();
            let mut out = vec![UiIntent::Notice(format!(
                "{} annotation{} copied",
                n,
                if n == 1 { "" } else { "s" }
            ))];
            if a == EditAction::Cut {
                out = vec![UiIntent::command(
                    "annotation.delete",
                    json!({ "ids": sel }),
                )];
                editor.canvas.selection.clear();
            }
            out
        }
        EditAction::Paste => {
            if editor.canvas.clipboard.is_empty() {
                return Vec::new();
            }
            editor.canvas.pastes += 1;
            let d = PASTE_OFFSET * f64::from(editor.canvas.pastes);
            let nodes = Value::Array(editor.canvas.clipboard.clone());
            vec![UiIntent::command(
                "annotation.paste",
                json!({ "nodes": nodes, "dx": d, "dy": d }),
            )]
        }
        EditAction::Delete => {
            if editor.toolbar.active == ToolId::Selection && editor.canvas.marquee.is_some() {
                let out: Vec<UiIntent> =
                    crate::panels::canvas_viewport::delete_selection_intent(editor, m)
                        .into_iter()
                        .collect();
                editor.canvas.marquee = None;
                return out;
            }
            if sel.is_empty() {
                return Vec::new();
            }
            editor.canvas.selection.clear();
            vec![UiIntent::command(
                "annotation.delete",
                json!({ "ids": sel }),
            )]
        }
        EditAction::Duplicate if !sel.is_empty() => {
            vec![UiIntent::command(
                "annotation.duplicate",
                json!({ "ids": sel }),
            )]
        }
        EditAction::Group if sel.len() >= 2 => {
            vec![UiIntent::command("annotation.group", json!({ "ids": sel }))]
        }
        EditAction::Ungroup if !sel.is_empty() => {
            vec![UiIntent::command(
                "annotation.ungroup",
                json!({ "ids": sel }),
            )]
        }
        EditAction::Duplicate | EditAction::Group | EditAction::Ungroup => Vec::new(),
        EditAction::SelectAll => {
            editor.toolbar.active = ToolId::Move;
            editor.canvas.selection = m
                .annotations
                .iter()
                .filter(|n| n.visible && !n.locked)
                .map(|n| n.id)
                .collect();
            Vec::new()
        }
        EditAction::BringToFront => arrange("front"),
        EditAction::BringForward => arrange("forward"),
        EditAction::SendBackward => arrange("backward"),
        EditAction::SendToBack => arrange("back"),
        EditAction::CopyAll => vec![UiIntent::CopyImage],
        EditAction::Save => vec![UiIntent::Save { doc_id: None }],
        EditAction::SaveAs => vec![UiIntent::SaveAs { doc_id: None }],
        EditAction::Open => vec![UiIntent::OpenDialog],
    }
}

/// Selects `id` and the rest of its group (the canvas and Layers rule).
pub fn select_with_group(
    editor: &mut EditorState,
    doc: &DocState,
    id: capturecraft_format::NodeId,
) {
    editor.canvas.selection = expand_groups(&doc.manifest, &[id]);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_engine::{Pixels, Session};
    use capturecraft_format::{Manifest, NodeId};
    use photocraft_geom::Size;

    fn session() -> Session {
        let mut s = Session::new();
        let size = Size::new(200, 100);
        s.open(
            Manifest::new(size),
            Pixels::new(size, vec![255; 200 * 100 * 4]),
        )
        .unwrap();
        for x in [0, 40] {
            s.run(
                "annotation.addArrow",
                &json!({ "start": { "x": x, "y": 10 }, "end": { "x": x + 30, "y": 10 } }),
            )
            .unwrap();
        }
        s
    }

    fn run_all(s: &mut Session, intents: Vec<UiIntent>) {
        for d in crate::dispatch(s, intents) {
            assert!(!matches!(d, crate::Dispatched::Failed { .. }), "{d:?}");
        }
    }

    #[test]
    fn copy_paste_cut_and_arrange_through_the_engine() {
        let mut s = session();
        let mut e = EditorState::default();
        // No selection: Copy is Copy All.
        assert_eq!(
            run(EditAction::Copy, &mut e, s.active()),
            [UiIntent::CopyImage]
        );
        assert!(!enabled(EditAction::Paste, &e, s.active()));
        e.canvas.selection = vec![NodeId(1)];
        let out = run(EditAction::Copy, &mut e, s.active());
        assert!(matches!(&out[0], UiIntent::Notice(_)));
        let paste = run(EditAction::Paste, &mut e, s.active());
        run_all(&mut s, paste);
        let paste = run(EditAction::Paste, &mut e, s.active());
        let UiIntent::Command { params, .. } = &paste[0] else {
            unreachable!()
        };
        assert_eq!(params["dx"], 32.0);
        run_all(&mut s, paste);
        assert_eq!(s.active().unwrap().manifest.annotations.len(), 4);
        let front = run(EditAction::SendToBack, &mut e, s.active());
        run_all(&mut s, front);
        assert_eq!(s.active().unwrap().manifest.annotations[0].id, NodeId(1));
        let cut = run(EditAction::Cut, &mut e, s.active());
        run_all(&mut s, cut);
        assert!(e.canvas.selection.is_empty());
        assert_eq!(s.active().unwrap().manifest.annotations.len(), 3);
        run(EditAction::SelectAll, &mut e, s.active());
        assert_eq!(e.canvas.selection.len(), 3);
        let g = run(EditAction::Group, &mut e, s.active());
        run_all(&mut s, g);
        let doc = s.active().unwrap();
        select_with_group(&mut e, doc, doc.manifest.annotations[0].id);
        assert_eq!(e.canvas.selection.len(), 3);
        let del = run(EditAction::Delete, &mut e, s.active());
        run_all(&mut s, del);
        assert!(s.active().unwrap().manifest.annotations.is_empty());
        let undo = run(EditAction::Undo, &mut e, s.active());
        run_all(&mut s, undo);
        assert_eq!(s.active().unwrap().manifest.annotations.len(), 3);
    }
}
