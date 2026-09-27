//! The system's media controls: media keys, and what plays in the desktop's "now
//! playing" (MPRIS on Linux, the media overlay on Windows, Now Playing on macOS), through
//! the `souvlaki` crate.
//!
//! Events from the system are handled by the playback controller directly, not the
//! window, so the keys work while the window is hidden or minimized.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
use tokio::runtime::Handle;
use uuid::Uuid;

use super::{Controller, PlayerView};

pub struct MediaKeys {
    controls: Mutex<MediaControls>,
    /// What the system was last told: (track, playing, paused), so it's only told changes.
    told: Mutex<Option<(Option<Uuid>, bool, bool)>>,
    /// Where covers are kept on disk (the system shows them from there).
    covers: PathBuf,
}

impl MediaKeys {
    /// Connects to the system's media controls. None if there are none (or it failed:
    /// the app works without them).
    pub fn start(controller: &Arc<Controller>, runtime: Handle, covers: PathBuf) -> Option<MediaKeys> {
        let config = PlatformConfig { display_name: "Acetate", dbus_name: "acetate", hwnd: None };
        let mut controls = match MediaControls::new(config) {
            Ok(controls) => controls,
            Err(err) => {
                eprintln!("no media keys: {err:?}");
                return None;
            }
        };
        // Weak: the controller owns these controls, and they mustn't keep it alive.
        let controller = Arc::downgrade(controller);
        let attached = controls.attach(move |event| {
            let Some(c) = controller.upgrade() else { return };
            match event {
                MediaControlEvent::Toggle => c.toggle_pause(),
                MediaControlEvent::Play => c.set_paused(false),
                MediaControlEvent::Pause | MediaControlEvent::Stop => c.set_paused(true),
                MediaControlEvent::Next => {
                    runtime.spawn(async move { c.next().await });
                }
                MediaControlEvent::Previous => {
                    runtime.spawn(async move { c.previous().await });
                }
                MediaControlEvent::SetPosition(MediaPosition(at)) => c.seek(at),
                MediaControlEvent::Seek(direction) => c.seek_by(signed(direction, Duration::from_secs(10))),
                MediaControlEvent::SeekBy(direction, by) => c.seek_by(signed(direction, by)),
                _ => {}
            }
        });
        if let Err(err) = attached {
            eprintln!("no media keys: {err:?}");
            return None;
        }
        Some(MediaKeys { controls: Mutex::new(controls), told: Mutex::new(None), covers })
    }

    /// After a seek: the system is told the new position with the next update.
    pub fn seeked(&self) {
        let mut told = self.told.lock().unwrap();
        if let Some(t) = told.as_mut() {
            // Not a state that exists: the next update differs, and says where it is.
            t.1 = !t.1;
        }
    }

    /// Tells the system what plays, when it changed.
    pub fn update(&self, view: &PlayerView) {
        let current = view.current();
        let state = (current.map(|c| c.track_id), view.playing, view.paused);
        let mut told = self.told.lock().unwrap();
        if *told == Some(state) {
            return;
        }
        let track_changed = told.map(|t| t.0) != Some(state.0);
        *told = Some(state);
        let mut controls = self.controls.lock().unwrap();
        if track_changed {
            let cover = current
                .and_then(|c| c.cover_id)
                .map(|id| self.covers.join(format!("{id}.jpg")))
                .filter(|path| path.exists())
                .map(|path| format!("file://{}", path.display()));
            let _ = controls.set_metadata(MediaMetadata {
                title: current.map(|c| c.title.as_str()),
                artist: current.map(|c| c.artist.as_str()),
                album: current.and_then(|c| c.album.as_deref()),
                cover_url: cover.as_deref(),
                duration: current.and_then(|c| c.duration_ms).map(|ms| Duration::from_millis(ms.max(0) as u64)),
            });
        }
        let progress = Some(MediaPosition(view.position_now()));
        let playback = match (view.playing, view.paused) {
            (false, _) => MediaPlayback::Stopped,
            (true, true) => MediaPlayback::Paused { progress },
            (true, false) => MediaPlayback::Playing { progress },
        };
        let _ = controls.set_playback(playback);
    }
}

fn signed(direction: SeekDirection, by: Duration) -> f32 {
    match direction {
        SeekDirection::Forward => by.as_secs_f32(),
        SeekDirection::Backward => -by.as_secs_f32(),
    }
}
