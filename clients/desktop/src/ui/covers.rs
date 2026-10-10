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
//!
//! Not in the cache, a cover is looked for on this computer before it's asked of the
//! server (so it shows offline too): among the covers the downloads keep (the same two
//! sizes), then among the mirror's files, where the original is (scaled here as the server
//! scales it, and kept in the cache).

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
/// At most this many bytes of textures (a standard cover is 1.6 MB): scrolling fast
/// through a big grid would otherwise load hundreds within `EVICT_AFTER`. Past it, the
/// ones shown longest ago go first (never ones shown in the last second).
const BUDGET: usize = 256 * 1024 * 1024;

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

/// Where covers are on this computer, besides the cache.
#[derive(Clone, Default)]
pub struct Here {
    /// The downloads (their kept covers).
    pub downloads: Option<Arc<acetate_core::offline::Offline>>,
    /// The mirror's root (its index and blobs), if this computer keeps one.
    pub mirror: Option<PathBuf>,
}

impl Here {
    /// A kept copy at `size`: its bytes, as the server made them.
    fn kept(&self, key: CoverKey, size: CoverSize) -> Option<Vec<u8>> {
        let path = self.downloads.as_ref()?.cover_path(&format!("{:016x}", key.0), size.query())?;
        std::fs::read(path).ok()
    }

    /// The original, from the mirror.
    fn original(&self, key: CoverKey) -> Option<Vec<u8>> {
        let root = self.mirror.as_ref()?;
        let index = root.join("mirror.sqlite");
        if !index.is_file() {
            return None;
        }
        let sha256 = crate::mirror::index::Index::open_reader(&index).ok()?.find_by_start(&format!("{:016x}", key.0)).ok()??;
        std::fs::read(root.join("blobs").join(sha256.get(0..2)?).join(sha256.get(2..4)?).join(&sha256)).ok()
    }
}

/// An image scaled to fit `pixels` on its longest side, as the server does it (a JPEG); one
/// that fits already is as it is.
fn scaled(original: &[u8], pixels: u32) -> Option<Vec<u8>> {
    let image = image::load_from_memory(original).ok()?;
    if image.width().max(image.height()) <= pixels {
        return Some(original.to_vec());
    }
    let scaled = image.resize(pixels, pixels, image::imageops::FilterType::CatmullRom).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88).encode_image(&scaled).ok()?;
    Some(out)
}

pub struct Covers {
    states: HashMap<(CoverKey, CoverSize), State>,
    /// Decoded images from the background, waiting to be uploaded.
    done: Arc<Mutex<Vec<((CoverKey, CoverSize), Option<ColorImage>)>>>,
    loading: usize,
    dir: Arc<PathBuf>,
    api: Api,
    runtime: Handle,
    here: Here,
}

impl Covers {
    pub fn new(api: Api, runtime: Handle, cache_dir: PathBuf) -> Covers {
        Covers { states: HashMap::new(), done: Arc::default(), loading: 0, dir: Arc::new(cache_dir), api, runtime, here: Here::default() }
    }

    /// Where covers are on this computer (for whoever is logged in).
    pub fn set_here(&mut self, here: Here) {
        self.here = here;
    }

    pub fn here(&self) -> Here {
        self.here.clone()
    }

    /// Covers that didn't load (the server was away) are tried again when next shown.
    pub fn forget_failed(&mut self) {
        self.states.retain(|_, state| !matches!(state, State::Failed));
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
        let before = self.states.len();
        self.states.retain(|_, state| !matches!(state, State::Ready { shown, .. } if now - *shown > EVICT_AFTER));
        let (_, mut bytes) = self.textures();
        if bytes > BUDGET {
            let mut ready: Vec<((CoverKey, CoverSize), f64, usize)> = self
                .states
                .iter()
                .filter_map(|(key, state)| match state {
                    State::Ready { texture, shown, .. } if now - *shown > 1.0 => Some((*key, *shown, texture_bytes(texture))),
                    _ => None,
                })
                .collect();
            ready.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (key, _, size) in ready {
                if bytes <= BUDGET {
                    break;
                }
                self.states.remove(&key);
                bytes -= size;
            }
        }
        if self.states.len() < before {
            crate::memory::trim();
        }
    }

    /// The textures held: how many, and their bytes.
    pub fn textures(&self) -> (usize, usize) {
        self.states.values().fold((0, 0), |(n, bytes), state| match state {
            State::Ready { texture, .. } => (n + 1, bytes + texture_bytes(texture)),
            _ => (n, bytes),
        })
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
                    let (done, api, dir, ctx, here) = (self.done.clone(), self.api.clone(), self.dir.clone(), ctx.clone(), self.here.clone());
                    self.runtime.spawn(async move {
                        let image = load(&api, &dir, &here, cover.key, size).await;
                        done.lock().unwrap().push((key, image));
                        ctx.request_repaint();
                    });
                }
                None
            }
        }
    }
}

/// A texture's size in memory (4 bytes a pixel).
fn texture_bytes(texture: &TextureHandle) -> usize {
    let [w, h] = texture.size();
    w * h * 4
}

/// Where an image's copy at `size` is kept (`cache_dir`: the app's cache folder).
pub fn path(cache_dir: &Path, key: CoverKey, size: CoverSize) -> PathBuf {
    cache_dir.join(format!("covers-{}", size.query())).join(format!("{:016x}", key.0))
}

/// The cover at `size`: from the cache, else this computer (`here`), else the server.
async fn load(api: &Api, dir: &Path, here: &Here, key: CoverKey, size: CoverSize) -> Option<ColorImage> {
    let path = path(dir, key, size);
    let found = {
        let (path, here) = (path.clone(), here.clone());
        // (Reading, and scaling an original: off the async threads.)
        tokio::task::spawn_blocking(move || {
            if let Ok(bytes) = std::fs::read(&path) {
                return Some((bytes, true));
            }
            if let Some(bytes) = here.kept(key, size) {
                return Some((bytes, true));
            }
            let pixels = match size {
                CoverSize::Small => 200,
                CoverSize::Standard => 640,
            };
            here.original(key).and_then(|original| scaled(&original, pixels)).map(|bytes| (bytes, false))
        })
        .await
        .ok()
        .flatten()
    };
    let (bytes, from_disk) = match found {
        Some(found) => found,
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
    /// `here`: the mirror has it, perhaps (then the server isn't asked).
    pub fn open(api: &Api, runtime: &Handle, ctx: &egui::Context, key: CoverKey, here: Here) -> Original {
        let answer: Arc<Mutex<Option<Result<(ColorImage, usize), String>>>> = Arc::default();
        let (api, sink, ctx) = (api.clone(), answer.clone(), ctx.clone());
        runtime.spawn(async move {
            let mirrored = tokio::task::spawn_blocking(move || here.original(key)).await.ok().flatten();
            let fetched = match mirrored {
                Some(bytes) => Ok(bytes),
                None => api.image(&format!("{:016x}", key.0), None).await,
            };
            let result = match fetched {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn an_original_is_scaled_to_fit_like_the_server_does() {
        let big = scaled(&png(1000, 500), 640).unwrap();
        let image = image::load_from_memory(&big).unwrap();
        assert_eq!((image.width(), image.height()), (640, 320));
        assert_eq!(image::guess_format(&big).unwrap(), image::ImageFormat::Jpeg);
        // One that fits already: as it is.
        let small = png(100, 80);
        assert_eq!(scaled(&small, 200).unwrap(), small);
    }
}
