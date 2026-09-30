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
    told_volume: Mutex<Option<f32>>,
    /// The cover it was told (a file's address; None: none yet, as it wasn't on disk), and
    /// when the disk was last looked at for a better one.
    told_cover: Mutex<(Option<String>, std::time::Instant)>,
    /// The cache folder, where covers are kept on disk (the system shows them from there).
    covers: PathBuf,
}

impl MediaKeys {
    /// Connects to the system's media controls. None if there are none (or it failed:
    /// the app works without them).
    pub fn start(controller: &Arc<Controller>, runtime: Handle, covers: PathBuf) -> Option<MediaKeys> {
        // One name per running app (the MPRIS spec's way for several instances:
        // `org.mpris.MediaPlayer2.acetate.instance<pid>`). A plain "acetate" is taken
        // when Acetate is already running, and souvlaki then panics in its own thread.
        let dbus_name = format!("acetate.instance{}", std::process::id());
        let config = PlatformConfig { display_name: "Acetate", dbus_name: &dbus_name, hwnd: None };
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
                MediaControlEvent::SetVolume(volume) => c.set_volume(volume as f32, false),
                _ => {}
            }
        });
        if let Err(err) = attached {
            eprintln!("no media keys: {err:?}");
            return None;
        }
        Some(MediaKeys {
            controls: Mutex::new(controls),
            told: Mutex::new(None),
            told_volume: Mutex::new(None),
            told_cover: Mutex::new((None, std::time::Instant::now())),
            covers,
        })
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
        let volume = if view.muted { 0.0 } else { view.volume };
        let mut told_volume = self.told_volume.lock().unwrap();
        if *told_volume != Some(volume) {
            *told_volume = Some(volume);
            let _ = self.controls.lock().unwrap().set_volume(volume as f64);
        }
        drop(told_volume);
        let current = view.current();
        let state = (current.map(|c| c.track_id), view.playing, view.paused);
        let mut told = self.told.lock().unwrap();
        if *told == Some(state) {
            return;
        }
        // The cover is on disk only once it's been shown (loaded in the background): until
        // the bigger copy is there, look again now and then, and tell the system when it
        // is.
        if *told == Some(state) {
            let mut told_cover = self.told_cover.lock().unwrap();
            let best = current.and_then(|c| c.cover).map(|c| format!("file://{}", crate::ui::covers::path(&self.covers, c.key, crate::ui::covers::CoverSize::Standard).display()));
            if best.is_some() && told_cover.0 != best && told_cover.1.elapsed() > Duration::from_secs(1) {
                told_cover.1 = std::time::Instant::now();
                let cover = self.cover(current);
                if cover != told_cover.0 {
                    told_cover.0 = cover.clone();
                    self.tell_metadata(current, cover.as_deref());
                }
            }
            return;
        }
        let track_changed = told.map(|t| t.0) != Some(state.0);
        *told = Some(state);
        if track_changed {
            let cover = self.cover(current);
            *self.told_cover.lock().unwrap() = (cover.clone(), std::time::Instant::now());
            self.tell_metadata(current, cover.as_deref());
        }
        let mut controls = self.controls.lock().unwrap();
        let progress = Some(MediaPosition(view.position_now()));
        let playback = match (view.playing, view.paused) {
            (false, _) => MediaPlayback::Stopped,
            (true, true) => MediaPlayback::Paused { progress },
            (true, false) => MediaPlayback::Playing { progress },
        };
        let _ = controls.set_playback(playback);
    }
}

impl MediaKeys {
    /// The cover of what plays as the system reads it: the bigger copy on disk if it's
    /// there (it is once "now playing" was shown), else the small one.
    fn cover(&self, current: Option<&super::QueueItem>) -> Option<String> {
        current
            .and_then(|c| c.cover)
            .and_then(|c| {
                [crate::ui::covers::CoverSize::Standard, crate::ui::covers::CoverSize::Small]
                    .into_iter()
                    .map(|size| crate::ui::covers::path(&self.covers, c.key, size))
                    .find(|path| path.exists())
            })
            .map(|path| format!("file://{}", path.display()))
    }

    fn tell_metadata(&self, current: Option<&super::QueueItem>, cover: Option<&str>) {
        let _ = self.controls.lock().unwrap().set_metadata(MediaMetadata {
            title: current.map(|c| c.title.as_str()),
            artist: current.map(|c| c.artist.as_str()),
            album: current.and_then(|c| c.album.as_deref()),
            cover_url: cover,
            duration: current.and_then(|c| c.duration_ms).map(|ms| Duration::from_millis(ms.max(0) as u64)),
        });
    }
}

fn signed(direction: SeekDirection, by: Duration) -> f32 {
    match direction {
        SeekDirection::Forward => by.as_secs_f32(),
        SeekDirection::Backward => -by.as_secs_f32(),
    }
}
