//! Settings, Health (admins): checks on the library and its files, run on request. They only
//! read: a finding is for a person to act on (nothing is deleted). Quick checks read folder
//! listings and the database; "reads every file" ones take a long time on a big library.
//! The server runs one at a time in the background; this page shows its progress and the
//! last report.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use serde::Deserialize;

use super::settings::{hint, section};
use crate::ui::task::Task;
use crate::ui::theme::PALETTE as P;
use crate::ui::Shell;

#[derive(Debug, Clone, Deserialize)]
struct Check {
    id: String,
    title: String,
    description: String,
    heavy: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Finding {
    severity: String,
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CheckResult {
    id: String,
    title: String,
    total: usize,
    findings: Vec<Finding>,
    error: Option<String>,
    seconds: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct Report {
    finished_at: String,
    by: String,
    results: Vec<CheckResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct Running {
    by: String,
    checks: Vec<String>,
    current: Option<String>,
    step: String,
    done: u64,
    total: u64,
    finished: Vec<CheckResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct Status {
    checks: Vec<Check>,
    running: Option<Running>,
    last: Option<Report>,
}

#[derive(Default)]
pub struct HealthTab {
    status_task: Task<Result<Status, String>>,
    status: Option<Result<Status, String>>,
    asked_at: Option<Instant>,
    /// The checks ticked to run; None until the first answer (then the quick ones).
    selected: Option<HashSet<String>>,
    start: Task<Result<Status, String>>,
    said: Option<String>,
}

impl HealthTab {
    pub fn opened(&mut self, shell: &Shell) {
        self.ask(shell);
    }

    fn ask(&mut self, shell: &Shell) {
        self.asked_at = Some(Instant::now());
        self.status_task.start_with(shell, |api| async move { api.send_json::<Status>(reqwest::Method::GET, "/api/health", None).await.map_err(|e| e.message) });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if let Some(status) = self.status_task.poll() {
            self.status = Some(status);
        }
        if let Some(started) = self.start.poll() {
            match started {
                Ok(status) => {
                    self.said = None;
                    self.status = Some(Ok(status));
                }
                Err(err) => self.said = Some(err),
            }
        }
        let running = matches!(&self.status, Some(Ok(s)) if s.running.is_some());
        if running {
            // Keep asking while a run goes on.
            ui.ctx().request_repaint_after(Duration::from_secs(1));
            if self.asked_at.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) && !self.status_task.is_running() {
                self.ask(shell);
            }
        }
        let status = match &self.status {
            None => {
                ui.label(RichText::new("Loading…").color(P.faint));
                return;
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger));
                return;
            }
            Some(Ok(status)) => status.clone(),
        };
        let selected = self.selected.get_or_insert_with(|| status.checks.iter().filter(|c| !c.heavy).map(|c| c.id.clone()).collect());

        hint(ui, "These only look: nothing is changed or deleted. What a finding means is for you to decide.");
        section(ui, "Run");
        for check in &status.checks {
            let mut on = selected.contains(&check.id);
            let label = if check.heavy { format!("{}  (reads every file)", check.title) } else { check.title.clone() };
            if ui.checkbox(&mut on, label).on_hover_text(check.description.as_str()).changed() {
                if on {
                    selected.insert(check.id.clone());
                } else {
                    selected.remove(&check.id);
                }
            }
            ui.label(RichText::new(check.description.as_str()).size(12.0).color(P.faint));
        }
        ui.add_space(6.0);
        let ids: Vec<String> = status.checks.iter().filter(|c| selected.contains(&c.id)).map(|c| c.id.clone()).collect();
        if ui.add_enabled(!running && !ids.is_empty() && !self.start.is_running(), egui::Button::new("Run the checks")).clicked() {
            let body = serde_json::json!({ "checks": ids });
            self.start.send_with(shell, |api| async move { api.send_json::<Status>(reqwest::Method::POST, "/api/health/run", Some(body)).await.map_err(|e| e.message) });
        }
        if let Some(said) = &self.said {
            ui.label(RichText::new(said.as_str()).color(P.danger));
        }

        if let Some(run) = &status.running {
            section(ui, "Running");
            ui.label(RichText::new(format!("Started by {}", run.by)).color(P.faint));
            let name = |id: &str| status.checks.iter().find(|c| c.id == id).map_or(id.to_string(), |c| c.title.clone());
            for id in &run.checks {
                let done = run.finished.iter().any(|r| r.id == *id);
                let now = run.current.as_deref() == Some(id.as_str());
                let mark = if done { "done" } else if now { "now" } else { "waiting" };
                ui.label(RichText::new(format!("{}  ·  {mark}", name(id))).color(if now { P.accent } else { P.muted }));
            }
            if run.total > 0 {
                ui.add(egui::ProgressBar::new(run.done as f32 / run.total as f32).desired_width(420.0).text(format!("{}: {} of {}", run.step, run.done, run.total)));
            } else if !run.step.is_empty() {
                ui.label(RichText::new(format!("{}…", run.step)).color(P.faint));
            }
            for result in &run.finished {
                show_result(ui, result);
            }
        } else if let Some(report) = &status.last {
            section(ui, "Last run");
            ui.label(RichText::new(format!("{} by {}", when(&report.finished_at), report.by)).color(P.faint));
            for result in &report.results {
                show_result(ui, result);
            }
        } else {
            ui.add_space(8.0);
            ui.label(RichText::new("Nothing has been run since the server started.").color(P.faint));
        }
    }
}

fn when(timestamp: &str) -> String {
    // "2026-10-12T14:03:22.123Z" → "2026-10-12 14:03 UTC"
    format!("{} UTC", timestamp.get(..16).unwrap_or(timestamp).replace('T', " "))
}

fn show_result(ui: &mut egui::Ui, result: &CheckResult) {
    ui.add_space(6.0);
    if let Some(err) = &result.error {
        ui.label(RichText::new(format!("{}: couldn't run: {err}", result.title)).color(P.danger));
        return;
    }
    if result.total == 0 {
        ui.label(RichText::new(format!("{}: nothing found ({:.1} s)", result.title, result.seconds)).color(P.muted));
        return;
    }
    let problems = result.findings.iter().filter(|f| f.severity == "problem").count();
    let title = format!("{}: {} found{}", result.title, result.total, if problems > 0 { format!(", {problems} problems") } else { String::new() });
    egui::CollapsingHeader::new(RichText::new(title).strong().color(if problems > 0 { P.danger } else { P.text })).id_salt(&result.id).default_open(problems > 0).show(ui, |ui| {
        for finding in &result.findings {
            let color = if finding.severity == "problem" { P.danger } else { P.muted };
            ui.add(egui::Label::new(RichText::new(finding.message.as_str()).color(color).size(12.5)).wrap());
        }
        if result.total > result.findings.len() {
            ui.label(RichText::new(format!("…and {} more", result.total - result.findings.len())).color(P.faint));
        }
    });
}
