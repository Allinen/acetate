//! Downloads with the library's current tags: a copy of the stored file with the track's
//! metadata (as the database has it now) written into its tags. The stored file is never
//! touched, and the copy isn't kept: it's made for each download.
//!
//! The fields below replace what the file has; one the database has nothing for is removed
//! from the copy (a stale value would contradict the library). Everything else in the
//! file's tags is left as it was.

use std::path::Path;

use anyhow::Context;
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};

/// What the database says about a track, as far as tags go.
#[derive(Debug, Default)]
pub struct TrackTags {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    /// As recorded: "2024", "2024-05" or "2024-05-17".
    pub date: Option<String>,
    pub genres: Vec<String>,
    /// Plain (not synced) lyrics.
    pub lyrics: Option<String>,
    /// The main cover: its bytes and content type.
    pub cover: Option<(Vec<u8>, Option<String>)>,
}

/// Writes `tags` into the file at `path`. Blocking.
pub fn write(path: &Path, tags: &TrackTags) -> anyhow::Result<()> {
    let mut file = lofty::read_from_path(path).context("reading the file's tags")?;
    if file.primary_tag().is_none() {
        let kind = file.primary_tag_type();
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().context("this kind of file has no tags to write")?;

    tag.set_title(tags.title.clone());
    tag.set_artist(tags.artist.clone());
    set_or_remove(tag, ItemKey::AlbumTitle, tags.album.as_deref());
    set_or_remove(tag, ItemKey::AlbumArtist, tags.album_artist.as_deref());
    set_or_remove(tag, ItemKey::RecordingDate, tags.date.as_deref());
    set_or_remove(tag, ItemKey::Lyrics, tags.lyrics.as_deref());
    let genres = tags.genres.join("; ");
    set_or_remove(tag, ItemKey::Genre, (!genres.is_empty()).then_some(genres.as_str()));
    match tags.track_number {
        Some(n) => tag.set_track(n),
        None => tag.remove_track(),
    }
    match tags.disc_number {
        Some(n) => tag.set_disk(n),
        None => tag.remove_disk(),
    }
    if let Some((bytes, mime)) = &tags.cover {
        // The library's main cover replaces the file's own front cover(s).
        tag.remove_picture_type(PictureType::CoverFront);
        let mime = mime.as_deref().map(MimeType::from_str);
        tag.push_picture(Picture::unchecked(bytes.clone()).pic_type(PictureType::CoverFront).mime_type(mime.unwrap_or(MimeType::Jpeg)).build());
    }
    tag.save_to_path(path, WriteOptions::default()).context("writing the tags")?;
    Ok(())
}

fn set_or_remove(tag: &mut Tag, key: ItemKey, value: Option<&str>) {
    match value {
        Some(text) => {
            tag.insert_text(key, text.to_string());
        }
        None => tag.remove_key(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::file::AudioFile;

    fn copy_of(name: &str, dir: &Path) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(name), &path).unwrap();
        path
    }

    #[test]
    fn current_tags_replace_the_files_and_the_audio_is_the_same() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["01 - Glass Harbor.flac", "lantern demo.mp3", "salt-and-static.m4a"] {
            let path = copy_of(name, dir.path());
            let before = lofty::read_from_path(&path).unwrap().properties().duration();
            let tags = TrackTags {
                title: "New Title".into(),
                artist: "Someone, Other".into(),
                album: Some("An Album".into()),
                album_artist: Some("Someone".into()),
                track_number: Some(3),
                disc_number: None,
                date: Some("2024-05-17".into()),
                genres: vec!["Ambient".into(), "Drone".into()],
                lyrics: Some("la la".into()),
                cover: None,
            };
            write(&path, &tags).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let file = lofty::read_from_path(&path).unwrap();
            let tag = file.primary_tag().unwrap();
            assert_eq!(tag.title().as_deref(), Some("New Title"), "{name}");
            assert_eq!(tag.artist().as_deref(), Some("Someone, Other"), "{name}");
            assert_eq!(tag.album().as_deref(), Some("An Album"), "{name}");
            assert_eq!(tag.track(), Some(3), "{name}");
            assert_eq!(tag.disk(), None, "{name}");
            assert_eq!(tag.get_string(ItemKey::AlbumArtist), Some("Someone"), "{name}");
            assert_eq!(file.properties().duration(), before, "{name}: same audio");
        }
    }
}
