//! Settings, Sound: the output, volume, streaming quality and the equalizer.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use super::settings::{hint, section};
use crate::playback::PlayerView;
use crate::ui::theme::PALETTE as P;
use crate::ui::{Action, Shell};

#[derive(Default)]
pub struct SoundTab {
    /// The sound outputs there are, listed in the background when the page opens (it can
    /// take a moment).
    outputs: Arc<Mutex<Option<Vec<String>>>>,
    /// The qualities the server makes copies at (`GET /api/qualities`), the default first.
    qualities: Arc<Mutex<Option<Vec<u32>>>>,
}

impl SoundTab {
    pub fn opened(&mut self, shell: &Shell) {
        let outputs = self.outputs.clone();
        shell.runtime.spawn_blocking(move || *outputs.lock().unwrap() = Some(crate::playback::engine::outputs()));
        let (qualities, api, ctx) = (self.qualities.clone(), shell.api.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            #[derive(serde::Deserialize)]
            struct Qualities {
                kbps: Vec<u32>,
            }
            if let Ok(q) = api.send_json::<Qualities>(reqwest::Method::GET, "/api/qualities", None).await {
                *qualities.lock().unwrap() = Some(q.kbps);
                ctx.request_repaint();
            }
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView) {
        egui::Grid::new("sound").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            ui.label(RichText::new("Output").color(P.muted));
            let outputs = self.outputs.lock().unwrap().clone();
            let current = shell.settings.output.clone();
            let shown = current.clone().unwrap_or_else(|| "The system's default".into());
            egui::ComboBox::from_id_salt("output").selected_text(shown).width(320.0).show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "The system's default").clicked() {
                    shell.act(Action::SetOutput(None));
                }
                match &outputs {
                    None => {
                        ui.label(RichText::new("Looking for outputs…").color(P.faint));
                    }
                    Some(names) => {
                        for name in names {
                            if ui.selectable_label(current.as_deref() == Some(name.as_str()), name).clicked() {
                                shell.act(Action::SetOutput(Some(name.clone())));
                            }
                        }
                    }
                }
            });
            ui.end_row();

            ui.label(RichText::new("Volume").color(P.muted));
            ui.horizontal(|ui| {
                let mut volume = player.volume;
                let slider = egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false);
                if ui.add_sized([240.0, 20.0], slider).changed() {
                    shell.act(Action::SetVolume(volume));
                }
                ui.label(RichText::new(format!("{}%", (player.volume * 100.0).round())).color(P.muted));
                let mut muted = player.muted;
                if ui.checkbox(&mut muted, "Muted").changed() {
                    shell.act(Action::ToggleMute);
                }
            });
            ui.end_row();
        });
        egui::Grid::new("quality").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            ui.label(RichText::new("Streaming quality").color(P.muted));
            let current = shell.settings.stream_kbps;
            let qualities = self.qualities.lock().unwrap().clone();
            let shown = match current {
                None => "Automatic".to_string(),
                Some(k) => format!("{k} kbps copy"),
            };
            egui::ComboBox::from_id_salt("stream-quality").selected_text(shown).width(320.0).show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "Automatic: the original if you may have it, else the server's usual copy").clicked() {
                    shell.act(Action::SetStreamQuality(None));
                }
                for k in qualities.iter().flatten() {
                    if ui.selectable_label(current == Some(*k), format!("{k} kbps copy")).clicked() {
                        shell.act(Action::SetStreamQuality(Some(*k)));
                    }
                }
            });
            ui.end_row();
        });
        hint(ui, "Copies are Opus, made by the server (and kept there). A lower quality downloads less; it applies from the next track.");
        hint(ui, "9 and 0 turn it down and up, m mutes, anywhere outside a text field.");

        section(ui, "Equalizer");
        self.equalizer(ui, shell);
    }

    /// Ten bands, an octave apart; heard as they're moved.
    fn equalizer(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        use crate::playback::eq::{headroom_db, BANDS, RANGE_DB};
        let (mut enabled, mut gains) = (shell.settings.eq_enabled, shell.settings.eq_gains);
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut enabled, "On").changed();
            if ui.add_enabled(gains != [0.0; 10], egui::Button::new("Flat")).clicked() {
                gains = [0.0; 10];
                changed = true;
            }
        });
        ui.add_space(6.0);
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                for (gain, &hz) in gains.iter_mut().zip(&BANDS) {
                    ui.vertical(|ui| {
                        ui.set_width(52.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(format!("{:+.0}", *gain)).color(P.muted).small());
                            let slider = egui::Slider::new(gain, -RANGE_DB..=RANGE_DB).vertical().show_value(false);
                            if ui.add_sized([20.0, 140.0], slider).double_clicked() {
                                *gain = 0.0;
                            }
                            let name = if hz >= 1000.0 { format!("{}k", hz / 1000.0) } else { format!("{hz}") };
                            ui.label(RichText::new(name).color(P.faint).small());
                        });
                    });
                }
            });
        });
        if changed || (enabled, gains) != (shell.settings.eq_enabled, shell.settings.eq_gains) {
            shell.act(Action::SetEqualizer(enabled, gains));
        }
        let headroom = headroom_db(&gains);
        let note = if enabled && headroom < -0.05 { format!(" The sound is lowered by {:.0} dB to make room for the boosts, so they can't clip.", -headroom) } else { String::new() };
        hint(ui, &format!("Gain in dB per band (Hz); double-click a band to flatten it.{note}"));
    }
}
