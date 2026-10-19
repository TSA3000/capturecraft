//! Editor keyboard shortcuts, dispatched the way PhotoCraft's
//! `shortcut_dispatch` does: a binding table walked over the frame's key
//! events in order (so ⇧⌘Z is matched before ⌘Z), each matched event
//! removed, and a focus guard that leaves text editing keys to a focused
//! text field. `Cmd` is Ctrl on Windows and Linux.
//!
//! Copy, Cut and Paste arrive from eframe as `Event::Copy` / `Cut` /
//! `Paste`, not as key events, and are matched by kind.

use egui::{Event, Key, KeyboardShortcut, Modifiers};

/// What a shortcut asks the editor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKey {
    Undo,
    Redo,
    /// The selected annotations, or Copy All without a selection.
    Copy,
    Cut,
    Paste,
    Delete,
    Duplicate,
    Group,
    Ungroup,
    SelectAll,
    Save,
    SaveAs,
    Open,
}

/// Key bindings, most specific first.
pub const BINDINGS: &[(Modifiers, Key, EditorKey)] = &[
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::Z,
        EditorKey::Redo,
    ),
    (Modifiers::COMMAND, Key::Z, EditorKey::Undo),
    // Ctrl+Y as well: the Windows convention (PhotoCraft has no Cmd+Y here).
    (Modifiers::COMMAND, Key::Y, EditorKey::Redo),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::G,
        EditorKey::Ungroup,
    ),
    (Modifiers::COMMAND, Key::G, EditorKey::Group),
    (Modifiers::COMMAND, Key::D, EditorKey::Duplicate),
    (Modifiers::COMMAND, Key::A, EditorKey::SelectAll),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::S,
        EditorKey::SaveAs,
    ),
    (Modifiers::COMMAND, Key::S, EditorKey::Save),
    (Modifiers::COMMAND, Key::O, EditorKey::Open),
    (Modifiers::NONE, Key::Delete, EditorKey::Delete),
    (Modifiers::NONE, Key::Backspace, EditorKey::Delete),
];

/// Keys a focused text field owns (PhotoCraft's `TEXT_OWNED`).
fn text_owned(k: EditorKey) -> bool {
    matches!(
        k,
        EditorKey::Undo
            | EditorKey::Redo
            | EditorKey::Copy
            | EditorKey::Cut
            | EditorKey::Paste
            | EditorKey::Delete
            | EditorKey::SelectAll
    )
}

fn matches(sc: &KeyboardShortcut, key: Key, mods: Modifiers) -> bool {
    sc.logical_key == key
        && mods.shift == sc.modifiers.shift
        && mods.alt == sc.modifiers.alt
        && mods.matches_logically(sc.modifiers)
        && (sc.modifiers.command || !mods.command)
}

/// The shortcuts in `events`, in order; matched events are removed.
/// With `text_focus`, keys a text field uses are left alone.
pub fn take_shortcuts(events: &mut Vec<Event>, text_focus: bool) -> Vec<EditorKey> {
    let mut out = Vec::new();
    events.retain(|e| {
        let hit = match e {
            Event::Copy => Some(EditorKey::Copy),
            Event::Cut => Some(EditorKey::Cut),
            Event::Paste(_) => Some(EditorKey::Paste),
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => BINDINGS
                .iter()
                .find(|(m, k, _)| matches(&KeyboardShortcut::new(*m, *k), *key, *modifiers))
                .map(|(_, _, a)| *a),
            _ => None,
        };
        match hit {
            Some(k) if !(text_focus && text_owned(k)) => {
                out.push(k);
                false
            }
            _ => true,
        }
    });
    out
}

/// Reads and consumes this frame's editor shortcuts.
pub fn read(ctx: &egui::Context) -> Vec<EditorKey> {
    let text_focus = ctx.text_edit_focused();
    ctx.input_mut(|i| take_shortcuts(&mut i.events, text_focus))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn bindings_match_most_specific_first_and_are_consumed() {
        let mut ev = vec![
            key(Key::Z, Modifiers::COMMAND),
            key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
            key(Key::Y, Modifiers::COMMAND),
            Event::Copy,
            key(Key::Q, Modifiers::COMMAND),
            key(Key::Z, Modifiers::NONE),
        ];
        let got = take_shortcuts(&mut ev, false);
        assert_eq!(
            got,
            [
                EditorKey::Undo,
                EditorKey::Redo,
                EditorKey::Redo,
                EditorKey::Copy
            ]
        );
        assert_eq!(ev.len(), 2, "unbound keys stay for others");
    }

    #[test]
    fn a_focused_text_field_keeps_its_editing_keys() {
        let mut ev = vec![
            key(Key::Z, Modifiers::COMMAND),
            Event::Paste("x".into()),
            key(Key::Backspace, Modifiers::NONE),
            key(Key::S, Modifiers::COMMAND),
        ];
        assert_eq!(take_shortcuts(&mut ev, true), [EditorKey::Save]);
        assert_eq!(ev.len(), 3);
    }
}
