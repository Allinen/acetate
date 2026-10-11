//! Changes: what was changed lately in the library's metadata, by anyone, newest first
//! (tracks, albums and artists together): who, when, how and why, and in words what
//! changed from the revision before, as in a page's history. A name opens its page.
//! Imports (a file's first metadata) are left out unless asked for. Needs `view_history`.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use uuid::Uuid;

use acetate_core::changes::{self, Change, Changes};

use crate::catalog::Catalog;
use crate::ui::task::Task;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::views::history::changes_named;
use crate::ui::{widgets, Action, Page, Shell};

type Answer = Task<(bool, Result<Changes, String>)>;

#[derive(Default)]
pub struct ChangesPage {
    imports: bool,
    list: Vec<Change>,
    names: HashMap<Uuid, String>,
    /// The last page was full: there may be older ones.
    more: bool,
    loading: bool,
    error: Option<String>,
    /// (whether it replaces the list, what arrived)
    answer: Answer,
}

impl ChangesPage {
    /// The page was opened: the newest changes.
    pub fn opened(&mut self, shell: &Shell) {
        self.load(shell, true);
    }

    /// Asks for the newest (replacing what's shown), or for the page after the last one.
    fn load(&mut self, shell: &Shell, fresh: bool) {
        self.loading = true;
        self.error = None;
        let before = if fresh { None } else { self.list.last().map(|c| c.id) };
        let path = changes::path(before, self.imports);
        self.answer.start_with(shell, |api| async move {
            let result = api.send_json::<Changes>(reqwest::Method::GET, &path, None).await.map_err(|err| err.message);
            (fresh, result)
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog) {
        let arrived = self.answer.poll();
        if let Some((fresh, result)) = arrived {
            self.loading = false;
            match result {
                Ok(page) => {
                    if fresh {
                        self.list.clear();
                        self.names.clear();
                    }
                    self.more = page.changes.len() == changes::PAGE;
                    self.names.extend(page.alias_names);
                    self.list.extend(page.changes);
                }
                Err(err) => self.error = Some(err),
            }
        }

        ui.label(RichText::new("Changes").font(font(30.0, bold())));
        ui.add_space(4.0);
        if !shell.account.can("view_history") {
            ui.label(RichText::new("Seeing what changed needs the permission to view history.").color(P.muted));
            return;
        }
        ui.horizontal(|ui| {
            if widgets::chip(ui, "Include imports", self.imports) {
                self.imports = !self.imports;
                self.load(shell, true);
            }
            if widgets::chip(ui, if self.loading { "Asking…" } else { "Refresh" }, false) && !self.loading {
                self.load(shell, true);
            }
        });
        ui.add_space(8.0);
        if let Some(err) = &self.error {
            ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger));
        }
        if self.list.is_empty() && !self.loading && self.error.is_none() {
            ui.label(RichText::new("Nothing has been changed yet.").color(P.muted));
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut open = None;
            for c in &self.list {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(c.created_at.get(..16).unwrap_or(&c.created_at).replace('T', " ")).font(font(12.5, semibold())));
                    if widgets::link(ui, &c.name, font(14.0, semibold()), P.accent) {
                        open = Some(match c.entity.as_str() {
                            "album" => Page::Album(c.entity_id),
                            "artist" => Page::Artist(c.entity_id),
                            _ => Page::Track(c.entity_id),
                        });
                    }
                    let who = c.author.clone().unwrap_or_else(|| "the system".into());
                    ui.label(RichText::new(format!("{} · {} by {who}", c.what(), c.source)).color(P.muted));
                });
                if !c.reason.is_empty() {
                    ui.label(RichText::new(format!("“{}”", c.reason)).color(P.muted).italics());
                }
                for line in changes_named(c.before.as_ref(), &c.data, catalog, &self.names) {
                    ui.label(RichText::new(format!("  {line}")).color(P.text));
                }
                ui.add_space(6.0);
            }
            if self.more && !self.loading && ui.button("Older").clicked() {
                self.load(shell, false);
            }
            if let Some(page) = open {
                shell.act(Action::Go(page));
            }
        });
    }
}
