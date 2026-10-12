//! The share link popup (Ctrl+Shift+L, or Shift+click "Share"): what the link should be
//! before it's made. Ctrl+L is the plain link and has no options; this is the one for
//! anyone, with its own commandline (declared as `SHARE`, the language is
//! `crate::commandline::schema`'s) and the same
//! things by mouse:
//!
//! * `original:yes|no`: the link plays the original file (needs `download_original`), or,
//!   by default, a lower-quality copy that has none of the original's tags.
//! * `quality:128`: the copy's bitrate in kbps, one of the server's (it says which; the first
//!   it lists is the default). Not for a link that gives the original.
//! * `expires:never|1d|7d|30d|Nd|Nw`: when the link stops working.
//!
//! Keys in the field: ↑/↓ choose, Tab takes the choice, Enter takes it and applies what's
//! typed (an empty field: makes the link), Ctrl+Enter applies it and makes the link. Esc
//! closes.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use crate::catalog::query::loosely_in;
use crate::commandline::schema::{self, Kind, Line, Lists, Next, Node, Row, Schema, Spec, Take};
use crate::links::Target;
use crate::ui::edit::commandline::{self as widget, CommandLine, Pressed};
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::{modal, widgets};

/// The popup's keys: each goes with the others (`original:yes expires:7d`).
static KEYS: [Spec; 3] = [
    Spec::new("original", Kind::Text, "the original file, or a lower-quality copy").aliases(&["orig", "og"]).next(Next::Args(&KEYS)),
    Spec::new("quality", Kind::Text, "the copy's bitrate (kbps)").aliases(&["kbps", "bitrate"]).next(Next::Args(&KEYS)),
    Spec::new("expires", Kind::Text, "never, 1d, 7d, 30d…").aliases(&["expire", "expiry", "exp"]).next(Next::Args(&KEYS)),
];

pub(crate) static SHARE: Schema = Schema { commands: &KEYS, also: &[] };

/// Every key typed, in the order typed.
fn parts(line: &Line) -> Vec<&Node> {
    fn add<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
        out.push(n);
        n.args.iter().for_each(|a| add(a, out));
    }
    let mut out = Vec::new();
    line.commands.iter().for_each(|c| add(c, &mut out));
    out
}

/// What a typed command sets.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Change {
    Original(bool),
    /// None: never.
    Expires(Option<u32>),
    /// None: the server's default.
    Quality(Option<u32>),
}

/// The days for `7d`, `2w`, `30`, or None for "never" (the outer Err: not understood).
fn parse_days(text: &str) -> Result<Option<u32>, String> {
    let text = text.trim().to_lowercase();
    if matches!(text.as_str(), "never" | "none" | "off" | "no") {
        return Ok(None);
    }
    let (digits, unit) = match text.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((at, _)) => text.split_at(at),
        None => (text.as_str(), "d"),
    };
    let n: u32 = digits.parse().map_err(|_| format!("“{text}” isn't a time: try never, 1d, 7d, 30d or 2w"))?;
    let days = match unit.trim() {
        "d" | "day" | "days" => n,
        "w" | "week" | "weeks" => n.saturating_mul(7),
        _ => return Err(format!("“{text}” isn't a time: try never, 1d, 7d, 30d or 2w")),
    };
    if days == 0 || days > 3650 {
        return Err("a link can last 1 day to 10 years (or never)".into());
    }
    Ok(Some(days))
}

/// `128`, `128k`, `128kbps`, or `default`; None: the default.
fn parse_kbps(text: &str) -> Result<Option<u32>, String> {
    let text = text.trim().to_lowercase();
    if matches!(text.as_str(), "default" | "auto" | "") {
        return Ok(None);
    }
    let digits = text.trim_end_matches("kbps").trim_end_matches('k').trim();
    digits.parse().map(Some).map_err(|_| format!("“{text}” isn't a bitrate: try 128 or default"))
}

fn parse_yes(text: &str) -> Option<bool> {
    match text.trim().to_lowercase().as_str() {
        "" | "yes" | "y" | "true" | "on" | "1" => Some(true),
        "no" | "n" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

pub use acetate_core::links::expires_at;

/// The popup's own row: make the link.
#[derive(Debug, Clone, Copy)]
struct Create;

/// The popup's list: each key's values, and making the link (found by words).
struct Choices<'a>(&'a ShareDialog);

impl Lists<Create> for Choices<'_> {
    fn choices(&self, _kind: Kind, typed: &str, node: &Node, _command: &Node) -> Vec<Row<Create>> {
        let own = |a: &str, b: &str| (a.to_string(), b.to_string());
        let options: Vec<(String, String)> = match node.spec.key {
            "original" => vec![own("yes", "the original file"), own("no", "a lower-quality copy")],
            "expires" => vec![own("never", ""), own("1d", "1 day"), own("7d", "7 days"), own("30d", "30 days")],
            _ => std::iter::once(own("default", "the server's usual")).chain(self.0.qualities.iter().flatten().map(|k| (k.to_string(), format!("{k} kbps")))).collect(),
        };
        options.into_iter().filter(|(v, _)| typed.is_empty() || loosely_in(typed, v)).map(|(v, sub)| Row::value(v, sub)).collect()
    }

    fn words(&self, typed: &str) -> Vec<Row<Create>> {
        let found = typed.is_empty() || ["Make the link", "create", "copy", "share"].iter().any(|n| loosely_in(typed, n));
        found.then(|| Row { label: "Make the link".into(), sub: "copies it (Ctrl+Enter)".into(), take: Take::Page(Create) }).into_iter().collect()
    }
}

pub struct ShareDialog {
    pub target: Target,
    cl: CommandLine,
    original: bool,
    days: Option<u32>,
    /// None: the server's default.
    kbps: Option<u32>,
    can_original: bool,
    /// The server's qualities, the default first (asked when the popup opens).
    qualities: Option<Vec<u32>>,
    answer: Arc<Mutex<Option<Vec<u32>>>>,
}

pub enum Outcome {
    Open,
    Close,
    Create { target: Target, original: bool, days: Option<u32>, kbps: Option<u32> },
}

impl ShareDialog {
    /// `answer`: where the server's list of qualities arrives (the app asks).
    pub fn new(target: Target, can_original: bool, answer: Arc<Mutex<Option<Vec<u32>>>>) -> ShareDialog {
        let mut cl = CommandLine::new(egui::Id::new("share-commandline"));
        cl.focus = true;
        ShareDialog { target, cl, original: false, days: None, kbps: None, can_original, qualities: None, answer }
    }

    fn rows(&self, line: &Line, text: &str) -> Vec<Row<Create>> {
        schema::rows(&SHARE, line, text, &Choices(self))
    }

    /// What the typed commands would set, or why one can't.
    fn plan(&self, line: &Line) -> Vec<Result<(Change, String), String>> {
        let mut out: Vec<Result<(Change, String), String>> = parts(line)
            .into_iter()
            .map(|part| match part.spec.key {
                "original" => {
                    let yes = parse_yes(&part.value).ok_or_else(|| format!("original: is yes or no, not “{}”", part.value))?;
                    if yes && !self.can_original {
                        return Err("giving the original file needs the download_original permission".into());
                    }
                    let label = if yes { "the link plays the original file" } else { "the link plays a lower-quality copy" };
                    Ok((Change::Original(yes), label.into()))
                }
                "quality" => {
                    let kbps = parse_kbps(&part.value)?;
                    if let (Some(k), Some(list)) = (kbps, &self.qualities)
                        && !list.contains(&k)
                    {
                        let list: Vec<String> = list.iter().map(|k| k.to_string()).collect();
                        return Err(format!("the server makes copies at {} kbps", list.join(", ")));
                    }
                    if kbps.is_some() && self.qualities.is_none() {
                        return Err("still asking the server which qualities it has…".into());
                    }
                    let label = match kbps {
                        None => "the copy has the server's usual quality".to_string(),
                        Some(k) => format!("the copy is {k} kbps"),
                    };
                    Ok((Change::Quality(kbps), label))
                }
                _ => {
                    let days = parse_days(&part.value)?;
                    let label = match days {
                        None => "the link never expires".to_string(),
                        Some(1) => "the link expires in 1 day".to_string(),
                        Some(n) => format!("the link expires in {n} days"),
                    };
                    Ok((Change::Expires(days), label))
                }
            })
            .collect();
        if let Some(word) = line.words.first().filter(|w| w.contains(':')) {
            out.push(Err(SHARE.not_a_command(word)));
        }
        out
    }

    /// Applies the typed commands if every one is fine. Returns whether they were.
    fn apply(&mut self) -> bool {
        let line = schema::parse(&SHARE, &self.cl.text);
        let plan = self.plan(&line);
        if plan.iter().any(|p| p.is_err()) {
            return false;
        }
        for (change, _) in plan.into_iter().flatten() {
            match change {
                Change::Original(yes) => self.original = yes,
                Change::Expires(days) => self.days = days,
                Change::Quality(kbps) => self.kbps = kbps,
            }
        }
        self.cl.text.clear();
        true
    }

    /// Draws the popup and handles its keys.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        if let Some(list) = self.answer.lock().unwrap().take() {
            self.qualities = Some(list);
        }
        let ctrl_enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        let focused = self.cl.hold_focus(ctx);
        let line = schema::parse(&SHARE, &self.cl.text);
        let rows = self.rows(&line, &self.cl.text);
        let pressed = if focused { self.cl.keys(ctx, rows.len(), false, false) } else { Pressed::default() };
        self.cl.highlighted = self.cl.highlighted.min(rows.len().saturating_sub(1));
        let units: Vec<Result<(String, Option<String>), String>> = self.plan(&line).into_iter().map(|p| p.map(|(_, label)| (label, None))).collect();
        let mut create = false;
        let mut run = false;
        let (can_original, mut original, mut days, mut kbps) = (self.can_original, self.original, self.days, self.kbps);
        let qualities = self.qualities.clone().unwrap_or_default();
        let mut make = false;
        let closed = modal::show(ctx, "share-dialog", "Share link", egui::vec2(560.0, 520.0), |ui| {
            ui.label(RichText::new("Anyone with the link can open it, no account needed. You can take it back in the web UI's Shares.").color(P.muted));
            ui.add_space(10.0);
            ui.add_enabled_ui(can_original, |ui| {
                ui.checkbox(&mut original, RichText::new("Give the original file").font(font(14.0, medium())));
            })
            .response
            .on_disabled_hover_text("Needs the download_original permission");
            ui.label(
                RichText::new(if original { "The link plays the original file." } else { "The link plays a lower-quality copy, with none of the file's tags." })
                    .color(P.faint)
                    .small(),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Expires").color(P.muted));
                for (label, value) in [("never", None), ("1 day", Some(1)), ("7 days", Some(7)), ("30 days", Some(30))] {
                    if widgets::chip(ui, label, days == value) {
                        days = value;
                    }
                }
                if let Some(n) = days.filter(|n| ![1, 7, 30].contains(n)) {
                    widgets::chip(ui, &format!("{n} days"), true);
                }
            });
            if !original {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Quality").color(P.muted));
                    if widgets::chip(ui, "default", kbps.is_none()) {
                        kbps = None;
                    }
                    for k in &qualities {
                        if widgets::chip(ui, &format!("{k} kbps"), kbps == Some(*k)) {
                            kbps = Some(*k);
                        }
                    }
                });
            }
            ui.add_space(12.0);
            self.cl.field(ui, true, "original:yes quality:64 expires:7d, or Enter to make the link");
            ui.add_space(4.0);
            let driven = self.cl.drive(ui, pressed, &rows, &SHARE, &mut schema::Pins::default(), true, "↑↓ choose · Tab takes · Enter takes and applies (empty: makes the link) · Ctrl+Enter both · Esc closes");
            // "Make the link", or Enter on an empty line: made; Enter otherwise: applied.
            if driven.page.is_some() || (driven.run && self.cl.text.trim().is_empty()) {
                create = true;
            } else {
                run = driven.run;
            }
            ui.add_space(6.0);
            widget::would_do(ui, &units, "Enter on an empty line makes the link.");
            ui.add_space(12.0);
            if ui.button(RichText::new("Make the link").font(font(14.0, medium()))).on_hover_text("Ctrl+Enter").clicked() {
                make = true;
            }
        });
        self.original = original;
        self.days = days;
        self.kbps = kbps;

        // (After the toggles: what's typed is applied on top.)
        if run || ctrl_enter {
            self.apply();
        }
        create |= ctrl_enter;
        if create || make {
            // What the popup shows is what's made (a toggle clicked this frame included).
            return Outcome::Create { target: self.target, original: self.original, days: self.days, kbps: if self.original { None } else { self.kbps } };
        }
        if closed { Outcome::Close } else { Outcome::Open }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(can_original: bool) -> ShareDialog {
        {
            let mut d = ShareDialog::new(Target::Track(uuid::Uuid::nil()), can_original, Default::default());
            d.qualities = Some(vec![128, 64]);
            d
        }
    }

    fn type_and_apply(d: &mut ShareDialog, text: &str) -> bool {
        d.cl.text = text.to_string();
        d.apply()
    }

    #[test]
    fn times() {
        assert_eq!(parse_days("never"), Ok(None));
        assert_eq!(parse_days("7d"), Ok(Some(7)));
        assert_eq!(parse_days("2w"), Ok(Some(14)));
        assert_eq!(parse_days("30"), Ok(Some(30)));
        assert!(parse_days("0d").is_err());
        assert!(parse_days("soon").is_err());
        assert!(parse_days("99999d").is_err());
    }

    #[test]
    fn typed_commands_set_the_toggles() {
        let mut d = dialog(true);
        assert!(type_and_apply(&mut d, "original:yes expires:7d"));
        assert!(d.original && d.days == Some(7));
        assert!(d.cl.text.is_empty());
        assert!(type_and_apply(&mut d, "og:no | exp:never"));
        assert!(!d.original && d.days.is_none());
        assert!(type_and_apply(&mut d, "quality:64k"));
        assert_eq!(d.kbps, Some(64));
        assert!(!type_and_apply(&mut d, "quality:320"), "a quality the server doesn't make");
        assert!(type_and_apply(&mut d, "quality:default"));
        assert_eq!(d.kbps, None);
    }

    #[test]
    fn the_original_needs_the_permission_and_a_mistake_changes_nothing() {
        let mut d = dialog(false);
        assert!(!type_and_apply(&mut d, "original:yes"));
        assert!(!d.original);
        assert!(!type_and_apply(&mut d, "expires:7d original:maybe"));
        assert_eq!(d.days, None, "all or nothing");
        assert_eq!(d.cl.text, "expires:7d original:maybe", "kept, to fix");
    }

    #[test]
    fn choices_follow_what_is_typed() {
        let d = dialog(true);
        let labels = |text: &str| d.rows(&schema::parse(&SHARE, text), text).into_iter().map(|r| r.label).collect::<Vec<_>>();
        assert_eq!(labels("expires:"), ["never", "1d", "7d", "30d"]);
        assert_eq!(labels("expires:7"), ["7d"]);
        assert_eq!(labels("original:n"), ["no"]);
        assert_eq!(labels(""), ["Make the link", "original:", "quality:", "expires:"]);
        assert!(labels("expires:7d ").starts_with(&["original:".to_string(), "quality:".to_string()]), "done: the keys to go on with");
        assert_eq!(labels("exp"), ["expires:"]);
        assert_eq!(labels("quality:"), ["default", "128", "64"]);
        assert_eq!(labels("quality:6"), ["64"]);
    }
}
