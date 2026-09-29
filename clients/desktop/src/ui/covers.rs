//! Cover images: loaded in the background at the size they're shown, uploaded to the GPU,
//! and let go again when they haven't been shown for a while (`EVICT_AFTER`), so memory
//! doesn't grow with every cover ever scrolled past. Each remembers when it arrived, so it
//! can fade in instead of popping.
//!
//! Two sizes, made by the server (`GET /api/images/{hash}?size=`): small (200 px on the
//! longest side) for rows and thumbnails, standard (640 px) for grids, a page's cover and
//! "now playing". Which one is picked from how big it's drawn, in pixels on the screen
//! (`CoverSize::for_rect`). The original, unchanged, is only fetched to be looked at on its
//! own (`Original`: a click on a big cover).
//!
//! Everything is per image, not per cover: the tracks of an album often each have their
//! own cover with the same image, and it's loaded, kept and shown once (the same texture,
//! so moving between those tracks doesn't flicker).
//!
//! What the server sends is kept on disk as it is (`covers-small/<key>`,
//! `covers-standard/<key>` in the cache folder): an image never changes under its key, so
//! the copy is good forever; the next start, or a cover shown again after being let go,
//! comes from there without the network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use tokio::runtime::Handle;

use crate::api::Api;
use crate::catalog::model::{Cover, CoverKey};

/// Loads at once.
const PARALLEL: usize = 8;
/// A texture not shown for this long (seconds) is let go (its copy on disk stays).
const EVICT_AFTER: f64 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoverSize {
    Small,
    Standard,
}

impl CoverSize {
    /// Small up to this many pixels on the screen (the small copy's own size, a little
    /// over: scaled up by a few percent is still sharp).
    const SMALL_UP_TO: f32 = 210.0;

    /// The size for a cover drawn in `rect` (on this screen: a HiDPI one has more pixels).
    pub fn for_rect(ctx: &egui::Context, rect: egui::Rect) -> CoverSize {
        let pixels = rect.width().max(rect.height()) * ctx.pixels_per_point();
        if pixels <= Self::SMALL_UP_TO { CoverSize::Small } else { CoverSize::Standard }
    }

    fn query(self) -> &'static str {
        match self {
            CoverSize::Small => "small",
            CoverSize::Standard => "standard",
        }
    }
}

enum State {
    Loading,
    Ready { texture: TextureHandle, arrived: f64, shown: f64 },
    Failed,
}

pub struct Covers {
    states: HashMap<(CoverKey, CoverSize), State>,
    /// Decoded images from the background, waiting to be uploaded.
    done: Arc<Mutex<Vec<((CoverKey, CoverSize), Option<ColorImage>)>>>,
    loading: usize,
    dir: Arc<PathBuf>,
    api: Api,
    runtime: Handle,
}

impl Covers {
    pub fn new(api: Api, runtime: Handle, cache_dir: PathBuf) -> Covers {
        Covers { states: HashMap::new(), done: Arc::default(), loading: 0, dir: Arc::new(cache_dir), api, runtime }
    }

    /// Takes what finished loading, and lets go of what hasn't been shown for a while.
    /// Call once per frame, before `get`.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for ((key, size), image) in std::mem::take(&mut *self.done.lock().unwrap()) {
            self.loading -= 1;
            let state = match image {
                Some(image) => State::Ready {
                    texture: ctx.load_texture(format!("cover-{:016x}-{}", key.0, size.query()), image, TextureOptions::LINEAR),
                    arrived: now,
                    shown: now,
                },
                None => State::Failed,
            };
            self.states.insert((key, size), state);
        }
        // (Dropping the texture's handle frees it.)
        self.states.retain(|_, state| !matches!(state, State::Ready { shown, .. } if now - *shown > EVICT_AFTER));
    }

    /// Whether this image couldn't be loaded (there's nothing to wait for).
    pub fn failed(&self, key: CoverKey, size: CoverSize) -> bool {
        matches!(self.states.get(&(key, size)), Some(State::Failed))
    }

    /// The cover's texture at `size` and how far it has faded in (0 to 1), starting a load
    /// if it isn't there yet. None while loading (or if it failed).
    pub fn get(&mut self, ctx: &egui::Context, cover: Cover, size: CoverSize) -> Option<(TextureHandle, f32)> {
        let key = (cover.key, size);
        let now = ctx.input(|i| i.time);
        match self.states.get_mut(&key) {
            Some(State::Ready { texture, arrived, shown }) => {
                *shown = now;
                let fade = (((now - *arrived) as f32) / 0.25).clamp(0.0, 1.0);
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
                        let image = load(&api, &dir, cover.key, size).await;
                        done.lock().unwrap().push((key, image));
                        ctx.request_repaint();
                    });
                }
                None
            }
        }
    }
}

/// Where an image's copy at `size` is kept (`cache_dir`: the app's cache folder).
pub fn path(cache_dir: &Path, key: CoverKey, size: CoverSize) -> PathBuf {
    cache_dir.join(format!("covers-{}", size.query())).join(format!("{:016x}", key.0))
}

async fn load(api: &Api, dir: &Path, key: CoverKey, size: CoverSize) -> Option<ColorImage> {
    let path = path(dir, key, size);
    let cached = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || std::fs::read(path).ok()).await.ok().flatten()
    };
    let (bytes, from_disk) = match cached {
        Some(bytes) => (bytes, true),
        None => (api.image(&format!("{:016x}", key.0), Some(size.query())).await.ok()?, false),
    };
    // Decoding is CPU work: off the async threads.
    tokio::task::spawn_blocking(move || {
        let image = image::load_from_memory(&bytes).ok()?.to_rgb8();
        if !from_disk {
            // Best effort: without a copy on disk it's just loaded again next time.
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let temp = path.with_extension("part");
            if std::fs::write(&temp, &bytes).is_ok() {
                let _ = std::fs::rename(&temp, &path);
            }
        }
        Some(ColorImage::from_rgb([image.width() as usize, image.height() as usize], image.as_raw()))
    })
    .await
    .ok()
    .flatten()
}

/// The original of an image, fetched to be looked at on its own (a popup), kept only
/// while it's open.
pub struct Original {
    pub key: CoverKey,
    answer: Arc<Mutex<Option<Result<(ColorImage, usize), String>>>>,
    /// The texture, its size in pixels, and the file's size in bytes.
    pub loaded: Option<Result<(TextureHandle, [usize; 2], usize), String>>,
}

impl Original {
    pub fn open(api: &Api, runtime: &Handle, ctx: &egui::Context, key: CoverKey) -> Original {
        let answer: Arc<Mutex<Option<Result<(ColorImage, usize), String>>>> = Arc::default();
        let (api, sink, ctx) = (api.clone(), answer.clone(), ctx.clone());
        runtime.spawn(async move {
            let result = match api.image(&format!("{:016x}", key.0), None).await {
                Ok(bytes) => tokio::task::spawn_blocking(move || {
                    let image = image::load_from_memory(&bytes).map_err(|err| format!("It isn't an image this can show ({err})."))?.to_rgba8();
                    Ok((ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()), bytes.len()))
                })
                .await
                .unwrap_or_else(|err| Err(err.to_string())),
                Err(err) => Err(err.message),
            };
            *sink.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
        Original { key, answer, loaded: None }
    }

    /// Takes the loaded image, once it's there.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.loaded = Some(result.map(|(image, bytes)| {
                let size = image.size;
                (ctx.load_texture(format!("original-{:016x}", self.key.0), image, TextureOptions::LINEAR), size, bytes)
            }));
        }
    }
}

/// Fast start, gentle stop.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
