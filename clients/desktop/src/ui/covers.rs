//! Cover images: loaded in the background, scaled down to at most `SIZE`, uploaded to the
//! GPU once and kept. Each remembers when it arrived, so it can fade in instead of popping.
//!
//! Everything is per image, not per cover: the tracks of an album often each have their
//! own cover with the same image, and it's loaded, kept and shown once (the same texture,
//! so moving between those tracks doesn't flicker).
//!
//! Scaled copies are kept on disk (`covers/<key>.jpg` in the cache folder): an image
//! never changes under its key, so the copy is good forever, and the next start shows it
//! without the network.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use tokio::runtime::Handle;

use crate::api::Api;
use crate::catalog::model::{Cover, CoverKey};

/// Longest side of a scaled cover (sharp in the grid and on "now playing").
const SIZE: u32 = 600;
/// Loads at once.
const PARALLEL: usize = 8;

enum State {
    Loading,
    Ready { texture: TextureHandle, arrived: f64 },
    Failed,
}

pub struct Covers {
    states: HashMap<CoverKey, State>,
    /// Decoded images from the background, waiting to be uploaded.
    done: Arc<Mutex<Vec<(CoverKey, Option<ColorImage>)>>>,
    loading: usize,
    dir: Arc<PathBuf>,
    api: Api,
    runtime: Handle,
}

impl Covers {
    pub fn new(api: Api, runtime: Handle, cache_dir: PathBuf) -> Covers {
        Covers {
            states: HashMap::new(),
            done: Arc::default(),
            loading: 0,
            dir: Arc::new(cache_dir.join("covers")),
            api,
            runtime,
        }
    }

    /// Takes what finished loading. Call once per frame, before `get`.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for (key, image) in std::mem::take(&mut *self.done.lock().unwrap()) {
            self.loading -= 1;
            let state = match image {
                Some(image) => State::Ready {
                    texture: ctx.load_texture(format!("cover-{:016x}", key.0), image, TextureOptions::LINEAR),
                    arrived: now,
                },
                None => State::Failed,
            };
            self.states.insert(key, state);
        }
    }

    /// Whether this image couldn't be loaded (there's nothing to wait for).
    pub fn failed(&self, key: CoverKey) -> bool {
        matches!(self.states.get(&key), Some(State::Failed))
    }

    /// The cover's texture and how far it has faded in (0 to 1), starting a load if it
    /// isn't there yet. None while loading (or if it failed).
    pub fn get(&mut self, ctx: &egui::Context, cover: Cover) -> Option<(TextureHandle, f32)> {
        let key = cover.key;
        match self.states.get(&key) {
            Some(State::Ready { texture, arrived }) => {
                let age = (ctx.input(|i| i.time) - arrived) as f32;
                let fade = (age / 0.25).clamp(0.0, 1.0);
                if fade < 1.0 {
                    ctx.request_repaint();
                }
                Some((texture.clone(), ease_out(fade)))
            }
            Some(_) => None,
            None => {
                if self.loading < PARALLEL {
                    self.loading += 1;
                    self.states.insert(key, State::Loading);
                    let (done, api, dir, ctx) = (self.done.clone(), self.api.clone(), self.dir.clone(), ctx.clone());
                    self.runtime.spawn(async move {
                        let image = load(&api, &dir, key).await;
                        done.lock().unwrap().push((key, image));
                        ctx.request_repaint();
                    });
                }
                None
            }
        }
    }
}

/// Where an image's scaled copy is kept.
pub fn path(dir: &std::path::Path, key: CoverKey) -> std::path::PathBuf {
    dir.join(format!("{:016x}.jpg", key.0))
}

async fn load(api: &Api, dir: &std::path::Path, key: CoverKey) -> Option<ColorImage> {
    let path = path(dir, key);
    let cached = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || std::fs::read(path).ok()).await.ok().flatten()
    };
    let (bytes, from_disk) = match cached {
        Some(bytes) => (bytes, true),
        None => (api.image(&format!("{:016x}", key.0)).await.ok()?, false),
    };
    let dir = dir.to_path_buf();
    // Decoding and scaling are CPU work: off the async threads.
    tokio::task::spawn_blocking(move || {
        let image = image::load_from_memory(&bytes).ok()?;
        let image = if image.width() > SIZE || image.height() > SIZE { image.thumbnail(SIZE, SIZE) } else { image };
        let rgb = image.to_rgb8();
        if !from_disk {
            // Best effort: without a copy on disk it's just loaded again next time.
            let _ = std::fs::create_dir_all(&dir);
            let mut out = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
            if rgb.write_with_encoder(encoder).is_ok() {
                let temp = path.with_extension("part");
                if std::fs::write(&temp, &out).is_ok() {
                    let _ = std::fs::rename(&temp, &path);
                }
            }
        }
        Some(ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw()))
    })
    .await
    .ok()
    .flatten()
}

/// Fast start, gentle stop.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
