//! Edit mode's pieces: forms that keep the revision they were filled from, save on
//! Ctrl+Enter (one revision, with an optional reason), discard on Esc, and handle a
//! stale save (someone else saved in between) by loading theirs and keeping your
//! changes on top; and pickers for artists, albums and tracks (fields that search as you
//! type, or the palette in a mode that hands the choice back to the form that asked).
//! New artists and albums chosen there are made when the form is saved, not before: a
//! discarded form leaves nothing behind.

pub mod album_form;
pub mod artist_form;
pub mod bulk;
pub mod cover_list;
pub mod credits;
pub mod inline_search;
pub mod og_file;
pub mod lyrics;
pub mod lyrics_sync;
pub mod part_of;
pub mod sources;
pub mod track_form;
pub mod versions;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, RichText};
use uuid::Uuid;

use crate::ui::theme::{font, medium};


/// What a picker picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Artist,
    Album,
    Track,
}

/// A choice from a picker: which (None: a new one, made when the form is saved), and its
/// name.
#[derive(Debug, Clone)]
pub struct Picked {
    pub kind: PickKind,
    pub id: Option<Uuid>,
    pub name: String,
}

/// Where picks from the palette arrive, for the form that asked.
#[derive(Default, Clone)]
pub struct Picker {
    picked: Arc<Mutex<Option<Picked>>>,
}

impl Picker {
    pub fn pick(&self, kind: PickKind, id: Option<Uuid>, name: String) {
        *self.picked.lock().unwrap() = Some(Picked { kind, id, name });
    }

    /// The pick, if one arrived (taken: each is handed out once).
    pub fn take(&self) -> Option<Picked> {
        self.picked.lock().unwrap().take()
    }
}

/// Where a form's save is.
#[derive(Debug, Clone, Default)]
pub enum SaveState {
    #[default]
    Idle,
    Saving,
    Saved(Instant),
    /// Someone else saved in between: the server's message.
    Stale(String),
    Failed(String),
}

/// A drop-down of `options` (shown by `name`) for `value`. A value not among them (an
/// older list) is kept, and shown as it is.
pub fn combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, value: &mut String, options: &[String], name: impl Fn(&str) -> String) {
    egui::ComboBox::from_id_salt(id).selected_text(RichText::new(name(value)).font(font(13.0, medium()))).show_ui(ui, |ui| {
        for option in options {
            if ui.selectable_label(value == option, name(option)).clicked() {
                *value = option.clone();
            }
        }
    });
}

pub fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
