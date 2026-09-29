//! Smaller copies of cover images, for clients that show many at once: `small` (200 px
//! on the longest side: rows, thumbnails) and `standard` (640 px: grids, a page's cover,
//! "now playing"). The original is always there too, unmodified.
//!
//! A copy is made the first time it's asked for (decoded, scaled, saved as JPEG) and kept
//! in the cache folder (`Config::cache_dir`, `images/<size>/<sha256>.jpg`); later requests
//! are a file read. An image never changes under its hash, so a copy is good forever, and
//! the folder can be emptied at any time (they're made again). An original that's already
//! that small is sent as it is.

use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Small,
    Standard,
}

impl Size {
    /// From the `size` query parameter.
    pub fn parse(text: &str) -> Option<Size> {
        match text {
            "small" => Some(Size::Small),
            "standard" => Some(Size::Standard),
            _ => None,
        }
    }

    /// The longest side, in pixels.
    pub fn pixels(self) -> u32 {
        match self {
            Size::Small => 200,
            Size::Standard => 640,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Standard => "standard",
        }
    }
}

/// What to send for a size.
pub enum Scaled {
    /// The copy, a JPEG.
    Resized(PathBuf),
    /// The original: it's that small already (or couldn't be read as an image).
    Original,
}

pub struct ImageCache {
    dir: PathBuf,
    /// Scaling is CPU work: this many at once (the rest wait), so a client asking for a
    /// whole grid of new covers doesn't starve everything else.
    working: Semaphore,
}

/// JPEG quality of the copies: sharp enough to not see at their size, a fraction of the
/// original's bytes.
const QUALITY: u8 = 88;

impl ImageCache {
    pub fn new(dir: PathBuf) -> ImageCache {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        ImageCache { dir: dir.join("images"), working: Semaphore::new(cores.div_ceil(2)) }
    }

    fn path(&self, size: Size, sha256: &str) -> PathBuf {
        self.dir.join(size.name()).join(format!("{sha256}.jpg"))
    }

    /// The copy of `original` (the blob with this hash) at `size`: made now if it isn't
    /// there yet.
    pub async fn sized(&self, original: &Path, sha256: &str, size: Size) -> anyhow::Result<Scaled> {
        let path = self.path(size, sha256);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(Scaled::Resized(path));
        }
        // Small enough already: no copy (the header says, without decoding it).
        if let Ok(dimensions) = imagesize::size(original)
            && dimensions.width.max(dimensions.height) <= size.pixels() as usize
        {
            return Ok(Scaled::Original);
        }
        let _permit = self.working.acquire().await?;
        // Made meanwhile, by a request that waited less.
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(Scaled::Resized(path));
        }
        let (original, target) = (original.to_path_buf(), path.clone());
        let made = tokio::task::spawn_blocking(move || make(&original, &target, size.pixels())).await?;
        match made {
            Ok(()) => Ok(Scaled::Resized(path)),
            Err(err) => {
                // Not an image this can read (or scale): the original is still good.
                tracing::warn!("couldn't make the {} copy of image {sha256}: {err:#}", size.name());
                Ok(Scaled::Original)
            }
        }
    }
}

/// Decodes `original`, scales it to fit `pixels` (keeping its shape) and writes it to
/// `target` as a JPEG (through a temporary file, so a half-written copy is never served).
fn make(original: &Path, target: &Path, pixels: u32) -> anyhow::Result<()> {
    let bytes = std::fs::read(original).context("reading the original")?;
    let image = image::load_from_memory(&bytes).context("decoding it")?;
    let scaled = image.resize(pixels, pixels, image::imageops::FilterType::CatmullRom).to_rgb8();
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY);
    scaled.write_with_encoder(encoder).context("encoding the copy")?;
    let dir = target.parent().context("the cache folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
    let temp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::write(temp.path(), &out)?;
    temp.persist(target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test image: `width`×`height`, a gradient (so scaling has something to do).
    fn png(dir: &Path, width: u32, height: u32) -> PathBuf {
        let image = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        let path = dir.join(format!("{width}x{height}.png"));
        image.save(&path).unwrap();
        path
    }

    #[tokio::test]
    async fn copies_are_made_once_and_small_originals_stay() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::new(dir.path().join("cache"));
        let big = png(dir.path(), 1200, 900);

        let Scaled::Resized(path) = cache.sized(&big, "abc", Size::Standard).await.unwrap() else { panic!("a copy") };
        let copy = image::open(&path).unwrap();
        assert_eq!((copy.width(), copy.height()), (640, 480), "fits 640, keeps its shape");
        let made = std::fs::metadata(&path).unwrap().modified().unwrap();
        let Scaled::Resized(again) = cache.sized(&big, "abc", Size::Standard).await.unwrap() else { panic!("a copy") };
        assert_eq!((again.clone(), std::fs::metadata(&again).unwrap().modified().unwrap()), (path, made), "kept, not made again");

        let Scaled::Resized(small) = cache.sized(&big, "abc", Size::Small).await.unwrap() else { panic!("a copy") };
        assert_eq!(image::open(small).unwrap().width(), 200);

        let tiny = png(dir.path(), 150, 150);
        assert!(matches!(cache.sized(&tiny, "def", Size::Small).await.unwrap(), Scaled::Original));

        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"not an image").unwrap();
        assert!(matches!(cache.sized(&broken, "ghi", Size::Small).await.unwrap(), Scaled::Original));
    }
}
