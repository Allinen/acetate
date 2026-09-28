//! What the app remembers about how you like it (`settings.json` in its data folder):
//! the volume and the sound output. Everything else it keeps (the library, covers, the
//! queue) is a copy of what the server has, not a setting.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 0 to 1, as the slider shows it (see `volume_amplitude`).
    pub volume: f32,
    pub muted: bool,
    /// The sound output's name; None: the system's default.
    pub output: Option<String>,
    /// The columns shown in each kind of track list ("album", "playlist"…), when changed
    /// from the list's own (right-click a column's name).
    pub table_columns: std::collections::HashMap<String, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { volume: 0.8, muted: false, output: None, table_columns: Default::default() }
    }
}

impl Settings {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.json")
    }

    pub fn load(data_dir: &Path) -> Settings {
        std::fs::read(Settings::path(data_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Best effort: without the file, the next start uses the defaults.
    pub fn save(&self, data_dir: &Path) {
        let _ = std::fs::create_dir_all(data_dir);
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(Settings::path(data_dir), bytes);
        }
    }
}

/// The slider's position as an amplitude factor. Loudness isn't linear in amplitude, so
/// the slider is cubic: equal steps sound like equal changes, and the low end is usable.
pub fn volume_amplitude(volume: f32, muted: bool) -> f32 {
    if muted { 0.0 } else { volume.clamp(0.0, 1.0).powi(3) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slider_is_cubic_and_mute_is_silent() {
        assert_eq!(volume_amplitude(1.0, false), 1.0);
        assert_eq!(volume_amplitude(0.5, false), 0.125);
        assert_eq!(volume_amplitude(0.8, true), 0.0);
        assert_eq!(volume_amplitude(3.0, false), 1.0, "clamped");
    }


    #[test]
    fn an_older_file_without_some_settings_reads() {
        let s: Settings = serde_json::from_str(r#"{"volume": 0.3}"#).unwrap();
        assert_eq!(s, Settings { volume: 0.3, ..Settings::default() });
    }
}
