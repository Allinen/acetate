//! Settings, Users (those with `manage_users`): the people who can log in and the groups
//! they are in, and the groups and what each is allowed. A person's permissions are the
//! union of their groups'. The `admin` group can't be changed. Every row is a draft: it is
//! saved in one go (or discarded). What you may hand out is the server's call: it refuses
//! permissions you don't have yourself.
//!
//! A person's token (for a bot or script, which sends it as `Authorization: Bearer`) is
//! had by logging in as them with their password: the token goes to the clipboard and is
//! never shown, and is a session of theirs like any other (it lasts while it is used, and
//! ends with the rest when their password changes).

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{self, RichText};
use serde::Deserialize;
use uuid::Uuid;

use super::settings::{hint, section};
use crate::ui::task::Task;
use crate::ui::theme::PALETTE as P;
use crate::ui::{widgets, Shell};

const ADMIN_GROUP: &str = "admin";

#[derive(Debug, Clone, Deserialize)]
struct Group {
    id: Uuid,
    name: String,
    description: String,
    permissions: Vec<String>,
    members: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct User {
    id: Uuid,
    username: String,
    groups: Vec<String>,
    has_password: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Permission {
    name: String,
    description: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct GroupDraft {
    name: String,
    description: String,
    permissions: BTreeSet<String>,
}

impl GroupDraft {
    fn of(group: &Group) -> GroupDraft {
        GroupDraft { name: group.name.clone(), description: group.description.clone(), permissions: group.permissions.iter().cloned().collect() }
    }

    fn body(&self) -> serde_json::Value {
        serde_json::json!({ "name": self.name.trim(), "description": self.description.trim(), "permissions": self.permissions })
    }
}

#[derive(Debug, Clone, Default)]
struct NewUser {
    username: String,
    password: String,
    groups: BTreeSet<String>,
}

#[derive(Default)]
pub struct UsersTab {
    users: Task<Result<Vec<User>, String>>,
    groups: Task<Result<Vec<Group>, String>>,
    permissions: Task<Result<Vec<Permission>, String>>,
    loaded_users: Option<Result<Vec<User>, String>>,
    loaded_groups: Option<Result<Vec<Group>, String>>,
    loaded_permissions: Vec<Permission>,
    /// A person's groups being changed (by id); absent: as the server has them. Only
    /// drafts that differ from the server are kept, so once a save's reload brings the
    /// saved values the draft drops out by itself (instead of a stale one being made
    /// from the old values while the reload is on its way).
    user_drafts: HashMap<Uuid, BTreeSet<String>>,
    /// Likewise for groups.
    group_drafts: HashMap<Uuid, GroupDraft>,
    new_user: NewUser,
    new_group: GroupDraft,
    /// The group asked to be deleted (asked again before it is).
    deleting: Option<Uuid>,
    action: Task<Result<String, String>>,
    said: Option<Result<String, String>>,
    /// The person whose token is being asked for, and their password as typed.
    token_for: Option<Uuid>,
    token_password: String,
    /// A login as them: the token, and their name.
    token: Task<Result<(String, String), String>>,
}

#[derive(Deserialize)]
struct LoginAnswer {
    token: String,
}

impl UsersTab {
    pub fn opened(&mut self, shell: &Shell) {
        self.users.start_with(shell, |api| async move { api.send_json::<Vec<User>>(reqwest::Method::GET, "/api/users", None).await.map_err(|e| e.message) });
        self.groups.start_with(shell, |api| async move { api.send_json::<Vec<Group>>(reqwest::Method::GET, "/api/groups", None).await.map_err(|e| e.message) });
        self.permissions.start_with(shell, |api| async move { api.send_json::<Vec<Permission>>(reqwest::Method::GET, "/api/permissions", None).await.map_err(|e| e.message) });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if let Some(users) = self.users.poll() {
            if let Ok(users) = &users {
                self.user_drafts.retain(|id, _| users.iter().any(|u| u.id == *id));
            }
            self.loaded_users = Some(users);
        }
        if let Some(groups) = self.groups.poll() {
            if let Ok(groups) = &groups {
                self.group_drafts.retain(|id, _| groups.iter().any(|g| g.id == *id));
            }
            self.loaded_groups = Some(groups);
        }
        if let Some(Ok(permissions)) = self.permissions.poll() {
            self.loaded_permissions = permissions;
        }
        if let Some(done) = self.action.poll() {
            if done.is_ok() {
                self.deleting = None;
                self.opened(shell);
            }
            self.said = Some(done);
        }
        if let Some(done) = self.token.poll() {
            if done.is_ok() {
                self.token_for = None;
            }
            self.said = Some(done.map(|(token, name)| {
                ui.ctx().copy_text(token);
                format!("Copied a token for {name} to the clipboard. Sent as “Authorization: Bearer <token>”; it is one of {name}'s sessions, and lasts while it is used.")
            }));
        }
        match &self.said {
            Some(Ok(text)) => {
                ui.label(RichText::new(text.as_str()).color(P.accent));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
            None => {}
        }
        let (Some(Ok(users)), Some(Ok(groups))) = (&self.loaded_users, &self.loaded_groups) else {
            match (&self.loaded_users, &self.loaded_groups) {
                (Some(Err(err)), _) | (_, Some(Err(err))) => ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger)),
                _ => ui.label(RichText::new("Loading…").color(P.faint)),
            };
            return;
        };
        let (users, groups) = (users.clone(), groups.clone());
        let busy = self.action.is_running();
        section(ui, "People");
        hint(ui, "What a person may do is what their groups allow. Click a group to put them in it or take them out, then save.");
        for user in &users {
            self.user_row(ui, shell, user, &groups, busy);
            ui.separator();
        }
        self.new_user_form(ui, shell, &groups, busy);
        section(ui, "Groups");
        hint(ui, "Hover a permission for what it allows. “admin” allows everything and can't be changed. Deleting a group takes its members out of it; one that restricted tracks are limited to can't be deleted until their access is changed.");
        for group in &groups {
            self.group_row(ui, shell, group, busy);
        }
        ui.add_space(8.0);
        self.new_group_form(ui, shell, busy);
    }

    fn user_row(&mut self, ui: &mut egui::Ui, shell: &Shell, user: &User, groups: &[Group], busy: bool) {
        let server_has: BTreeSet<String> = user.groups.iter().cloned().collect();
        let mut draft = self.user_drafts.remove(&user.id).unwrap_or_else(|| server_has.clone());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&user.username).strong());
            if !user.has_password {
                ui.label(RichText::new("no password: can't log in").color(P.faint));
            }
        });
        if user.has_password {
            self.token_row(ui, shell, user);
        }
        ui.horizontal_wrapped(|ui| {
            for group in groups {
                let on = draft.contains(&group.name);
                if widgets::chip(ui, &group.name, on) {
                    if on {
                        draft.remove(&group.name);
                    } else {
                        draft.insert(group.name.clone());
                    }
                }
            }
        });
        let changed = draft != server_has;
        let mut discard = false;
        ui.horizontal(|ui| {
            if ui.add_enabled(changed && !busy, egui::Button::new("Save").small()).clicked() {
                let body = serde_json::json!({ "groups": &draft });
                let path = format!("/api/users/{}/groups", user.id);
                let name = user.username.clone();
                self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::PUT, &path, Some(body)).await.map(|_| format!("Saved {name}'s groups.")).map_err(|e| e.message) });
            }
            if ui.add_enabled(changed, egui::Button::new("Discard").small()).clicked() {
                discard = true;
            }
        });
        if changed && !discard {
            self.user_drafts.insert(user.id, draft);
        }
    }

    /// "Copy token": asks for the person's password, logs in as them, and puts the token
    /// on the clipboard (not on screen, which may be shared or streamed).
    fn token_row(&mut self, ui: &mut egui::Ui, shell: &Shell, user: &User) {
        let asking = self.token_for == Some(user.id);
        if !asking {
            if ui.add(egui::Button::new("Copy token…").small()).on_hover_text("For a bot or script: logs in as them with their password and copies the session token").clicked() {
                self.token_for = Some(user.id);
                self.token_password.clear();
            }
            return;
        }
        let running = self.token.is_running();
        ui.horizontal_wrapped(|ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut self.token_password).password(true).hint_text(format!("{}'s password", user.username)).desired_width(180.0));
            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let ready = !self.token_password.is_empty() && !running;
            if ui.add_enabled(ready, egui::Button::new("Copy").small()).clicked() || (entered && ready) {
                let password = std::mem::take(&mut self.token_password);
                let name = user.username.clone();
                let body = serde_json::json!({ "username": name, "password": password });
                self.token.send_with(shell, |api| async move {
                    api.send_json::<LoginAnswer>(reqwest::Method::POST, "/api/auth/login", Some(body)).await.map(|answer| (answer.token, name)).map_err(|e| e.message)
                });
            }
            if ui.add(egui::Button::new("Cancel").small()).clicked() {
                self.token_for = None;
                self.token_password.clear();
            }
        });
    }

    fn new_user_form(&mut self, ui: &mut egui::Ui, shell: &Shell, groups: &[Group], busy: bool) {
        ui.add_space(8.0);
        ui.label(RichText::new("New person").strong());
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_user.username).hint_text("username").desired_width(180.0));
            ui.add(egui::TextEdit::singleline(&mut self.new_user.password).password(true).hint_text("password").desired_width(180.0));
        });
        ui.horizontal_wrapped(|ui| {
            for group in groups {
                let on = self.new_user.groups.contains(&group.name);
                if widgets::chip(ui, &group.name, on) {
                    if on {
                        self.new_user.groups.remove(&group.name);
                    } else {
                        self.new_user.groups.insert(group.name.clone());
                    }
                }
            }
        });
        let ready = !self.new_user.username.trim().is_empty() && !self.new_user.password.is_empty();
        if ui.add_enabled(ready && !busy, egui::Button::new("Add person")).clicked() {
            let new = std::mem::take(&mut self.new_user);
            let name = new.username.trim().to_string();
            let body = serde_json::json!({ "username": name, "password": new.password, "groups": new.groups });
            self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::POST, "/api/users", Some(body)).await.map(|_| format!("Added {name}.")).map_err(|e| e.message) });
        }
    }

    fn group_row(&mut self, ui: &mut egui::Ui, shell: &Shell, group: &Group, busy: bool) {
        let locked = group.name == ADMIN_GROUP;
        let server = GroupDraft::of(group);
        let mut draft = self.group_drafts.remove(&group.id).unwrap_or_else(|| server.clone());
        let mut discard = false;
        let title = format!("{}  ·  {}", group.name, match group.members.len() {
            0 => "no members".to_string(),
            1 => "1 member".to_string(),
            n => format!("{n} members"),
        });
        let permissions = std::mem::take(&mut self.loaded_permissions);
        egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt(group.id).show(ui, |ui| {
            if locked {
                ui.label(RichText::new("Everything is allowed, and this group can't be changed.").color(P.faint));
            } else {
                ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(240.0));
                ui.add(egui::TextEdit::singleline(&mut draft.description).hint_text("what it is for").desired_width(420.0));
                ui.add_space(4.0);
                for permission in &permissions {
                    let mut on = draft.permissions.contains(&permission.name);
                    if ui.checkbox(&mut on, permission.name.as_str()).on_hover_text(permission.description.as_str()).changed() {
                        if on {
                            draft.permissions.insert(permission.name.clone());
                        } else {
                            draft.permissions.remove(&permission.name);
                        }
                    }
                }
            }
            if !group.members.is_empty() {
                ui.label(RichText::new(format!("Members: {}", group.members.join(", "))).color(P.faint));
            }
            if locked {
                return;
            }
            let changed = draft != server;
            let ready = !draft.name.trim().is_empty();
            let body = draft.body();
            ui.horizontal(|ui| {
                if ui.add_enabled(changed && ready && !busy, egui::Button::new("Save").small()).clicked() {
                    let path = format!("/api/groups/{}", group.id);
                    let name = group.name.clone();
                    self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::PUT, &path, Some(body)).await.map(|_| format!("Saved {name}.")).map_err(|e| e.message) });
                }
                if ui.add_enabled(changed, egui::Button::new("Discard").small()).clicked() {
                    discard = true;
                }
                if self.deleting == Some(group.id) {
                    if ui.add_enabled(!busy, egui::Button::new(RichText::new("Delete it for good").color(P.danger)).small()).clicked() {
                        let path = format!("/api/groups/{}", group.id);
                        let name = group.name.clone();
                        self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::DELETE, &path, None).await.map(|_| format!("Deleted {name}.")).map_err(|e| e.message) });
                    }
                    if ui.add(egui::Button::new("Keep it").small()).clicked() {
                        self.deleting = None;
                    }
                } else if ui.add_enabled(!busy, egui::Button::new("Delete").small()).clicked() {
                    self.deleting = Some(group.id);
                }
            });
        });
        self.loaded_permissions = permissions;
        if !discard && draft != server {
            self.group_drafts.insert(group.id, draft);
        }
        ui.separator();
    }

    fn new_group_form(&mut self, ui: &mut egui::Ui, shell: &Shell, busy: bool) {
        ui.label(RichText::new("New group").strong());
        ui.add(egui::TextEdit::singleline(&mut self.new_group.name).hint_text("name").desired_width(240.0));
        ui.add(egui::TextEdit::singleline(&mut self.new_group.description).hint_text("what it is for").desired_width(420.0));
        for permission in &self.loaded_permissions {
            let mut on = self.new_group.permissions.contains(&permission.name);
            if ui.checkbox(&mut on, permission.name.as_str()).on_hover_text(permission.description.as_str()).changed() {
                if on {
                    self.new_group.permissions.insert(permission.name.clone());
                } else {
                    self.new_group.permissions.remove(&permission.name);
                }
            }
        }
        if ui.add_enabled(!self.new_group.name.trim().is_empty() && !busy, egui::Button::new("Add group")).clicked() {
            let new = std::mem::take(&mut self.new_group);
            let (body, name) = (new.body(), new.name.trim().to_string());
            self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::POST, "/api/groups", Some(body)).await.map(|_| format!("Added {name}.")).map_err(|e| e.message) });
        }
    }
}
