//! The share link popup (Ctrl+Shift+L, or Shift+click "Share"): what the link should be
//! before it's made. Ctrl+L is the plain link and has no options; this is the one for
//! anyone, with its own commandline (the language is `crate::commandline`'s) and the same
//! things by mouse:
//!
//! * `original:yes|no`: the link plays the original file (needs `download_original`), or,
//!   by default, a lower-quality copy that has none of the original's tags.
//! * `quality:128`: the copy's bitrate in kbps, one of the server's (it says which; the first
//!   it lists is the default). Not for a link that gives the original.
//! * `expires:never|1d|7d|30d|Nd|Nw`: when the link stops working.
//!
//! Keys in the field: ↑/↓ choose, Tab takes the choice, Enter applies what's typed (an
//! empty field: makes the link), Ctrl+Enter applies it and makes the link. Esc closes.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText};

use crate::catalog::query::loosely_in;
use crate::commandline::{self, Keys, Line};
use crate::links::Target;
use crate::ui::edit::commandline::{self as widget, CommandLine, Pressed};
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::{modal, widgets};

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    Original,
    Expires,
    Quality,
}

const KEYS: &[(K, &[&str])] = &[(K::Original, &["original", "orig", "og"]), (K::Expires, &["expires", "expire", "expiry", "exp"]), (K::Quality, &["quality", "kbps", "bitrate"])];

impl Keys for K {
    const MINUS: bool = false;

    fn key(name: &str) -> Option<K> {
        let name = name.to_lowercase();
        KEYS.iter().find(|(_, names)| names.contains(&name.as_str())).map(|(k, _)| *k)
    }

    fn name(self) -> &'static str {
        KEYS.iter().find(|(k, _)| *k == self).map(|(_, names)| names[0]).unwrap_or("")
    }

    /// Both go with each other: `original:yes expires:7d`.
    fn takes(_main: K, _arg: K) -> bool {
        true
    }
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

/// `days` from now, as the server reads it (RFC 3339, UTC).
pub fn expires_at(days: u32) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    rfc3339(now + days as u64 * 86_400)
}

fn rfc3339(secs: u64) -> String {
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

enum What {
    /// Replaces the value being typed.
    Value(String),
    /// Replaces the whole line (a key to type a value for).
    Insert(String),
    Create,
}

struct Row {
    label: String,
    sub: String,
    what: What,
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

    fn rows(&self, line: &Line<K>) -> Vec<Row> {
        let row = |label: &str, sub: &str, what: What| Row { label: label.to_string(), sub: sub.to_string(), what };
        if let Some(last) = line.parts.last() {
            let typed = &last.value;
            let own = |a: &str, b: &str| (a.to_string(), b.to_string());
            let options: Vec<(String, String)> = match last.key {
                K::Original => vec![own("yes", "the original file"), own("no", "a lower-quality copy")],
                K::Expires => vec![own("never", ""), own("1d", "1 day"), own("7d", "7 days"), own("30d", "30 days")],
                K::Quality => std::iter::once(own("default", "the server's usual"))
                    .chain(self.qualities.iter().flatten().map(|k| (k.to_string(), format!("{k} kbps"))))
                    .collect(),
            };
            return options
                .into_iter()
                .filter(|(v, _)| typed.is_empty() || loosely_in(typed, v))
                .map(|(v, sub)| Row { label: v.clone(), sub, what: What::Value(v) })
                .collect();
        }
        let words = line.words.join(" ");
        let found = |name: &str, aka: &[&str]| words.is_empty() || loosely_in(&words, name) || aka.iter().any(|a| loosely_in(&words, a));
        let mut rows = Vec::new();
        if found("Make the link", &["create", "copy", "share"]) {
            rows.push(row("Make the link", "copies it (Ctrl+Enter)", What::Create));
        }
        if found("original", &["file"]) {
            rows.push(row("original:", "the original file, or a lower-quality copy", What::Insert("original:".into())));
        }
        if found("quality", &["kbps", "bitrate"]) {
            rows.push(row("quality:", "the copy's bitrate (kbps)", What::Insert("quality:".into())));
        }
        if found("expires", &["expiry", "time"]) {
            rows.push(row("expires:", "never, 1d, 7d, 30d…", What::Insert("expires:".into())));
        }
        rows
    }

    /// What the typed commands would set, or why one can't.
    fn plan(&self, line: &Line<K>) -> Vec<Result<(Change, String), String>> {
        line.parts
            .iter()
            .map(|part| match part.key {
                K::Original => {
                    let yes = parse_yes(&part.value).ok_or_else(|| format!("original: is yes or no, not “{}”", part.value))?;
                    if yes && !self.can_original {
                        return Err("giving the original file needs the download_original permission".into());
                    }
                    let label = if yes { "the link plays the original file" } else { "the link plays a lower-quality copy" };
                    Ok((Change::Original(yes), label.into()))
                }
                K::Quality => {
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
                K::Expires => {
                    let days = parse_days(&part.value)?;
                    let label = match days {
                        None => "the link never expires".to_string(),
                        Some(1) => "the link expires in 1 day".to_string(),
                        Some(n) => format!("the link expires in {n} days"),
                    };
                    Ok((Change::Expires(days), label))
                }
            })
            .collect()
    }

    /// Applies the typed commands if every one is fine. Returns whether they were.
    fn apply(&mut self) -> bool {
        let line = commandline::parse::<K>(&self.cl.text);
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

    /// A choice taken (Tab, Enter or a click). Returns true if it asks to make the link.
    fn take(&mut self, ctx: &egui::Context, line: &Line<K>, row: Option<Row>) -> bool {
        let Some(row) = row else { return false };
        match row.what {
            What::Create => return true,
            What::Insert(text) => {
                self.cl.text = text;
                self.cl.highlighted = 0;
                self.cl.cursor_to_end(ctx);
            }
            What::Value(value) => {
                if let Some(last) = line.parts.last() {
                    self.cl.take(ctx, last.value_start, last.end, Some(&value), None);
                }
            }
        }
        false
    }

    /// Draws the popup and handles its keys.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        if let Some(list) = self.answer.lock().unwrap().take() {
            self.qualities = Some(list);
        }
        let ctrl_enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        let focused = self.cl.hold_focus(ctx);
        let line = commandline::parse::<K>(&self.cl.text);
        let mut rows = self.rows(&line);
        let pressed = if focused { self.cl.keys(ctx, rows.len(), false, false) } else { Pressed::default() };
        self.cl.highlighted = self.cl.highlighted.min(rows.len().saturating_sub(1));

        let mut create = false;
        let mut clicked = None;
        if pressed.tab && !rows.is_empty() {
            let row = rows.swap_remove(self.cl.highlighted);
            create |= self.take(ctx, &line, Some(row));
        } else if pressed.enter {
            if self.cl.text.trim().is_empty() {
                create = true;
            } else {
                // What's typed doesn't make sense yet (half a value): the highlighted choice
                // finishes it, then it's applied.
                let broken = self.plan(&line).iter().any(|p| p.is_err());
                if broken && !rows.is_empty() {
                    let row = rows.swap_remove(self.cl.highlighted);
                    create |= self.take(ctx, &line, Some(row));
                }
                if !create && !self.cl.text.ends_with(':') {
                    self.apply();
                }
            }
        }
        if ctrl_enter {
            self.apply();
            create = true;
        }

        // What's shown is worked out again: the text may have changed above.
        let line = commandline::parse::<K>(&self.cl.text);
        let rows = self.rows(&line);
        let plan = self.plan(&line);
        let units: Vec<Result<(String, Option<String>), String>> =
            plan.into_iter().map(|p| p.map(|(_, label)| (label, None))).collect();
        let list: Vec<(String, String)> = rows.iter().map(|r| (r.label.clone(), r.sub.clone())).collect();
        let highlighted = self.cl.highlighted;

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
            widget::list(ui, list, highlighted, &mut clicked, "↑↓ choose · Tab takes · Enter applies (empty: makes the link) · Ctrl+Enter both · Esc closes");
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

        if let Some(n) = clicked
            && n < rows.len()
        {
            let row = rows.into_iter().nth(n);
            create |= self.take(ctx, &line, row);
        }
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
    fn dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(rfc3339(1_709_164_800 + 86_399), "2024-02-29T23:59:59Z");
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
        let labels = |text: &str| d.rows(&commandline::parse::<K>(text)).into_iter().map(|r| r.label).collect::<Vec<_>>();
        assert_eq!(labels("expires:"), ["never", "1d", "7d", "30d"]);
        assert_eq!(labels("expires:7"), ["7d"]);
        assert_eq!(labels("original:n"), ["no"]);
        assert_eq!(labels(""), ["Make the link", "original:", "quality:", "expires:"]);
        assert_eq!(labels("exp"), ["expires:"]);
        assert_eq!(labels("quality:"), ["default", "128", "64"]);
        assert_eq!(labels("quality:6"), ["64"]);
    }
}
