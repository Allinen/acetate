//! ID3v2 text frames that are in a tag more than once, e.g. two TPE1 frames for two
//! artists. The standard doesn't allow that, but some taggers write several values that way
//! in ID3v2.3 (which has no separator for them). lofty and the id3 crate keep only one
//! frame per id, so the other names would be lost; this reads the frames directly.
//!
//! Only what's needed for that: the tag is found (at the start of the file, or as a WAV or
//! AIFF chunk), its frames are walked, and text frames (T…, but not TXXX) that repeat are
//! decoded. Anything unusual (unsynchronisation, compressed or encrypted frames) is left
//! alone: then lofty's reading stands.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// For each text frame id given more than once: all its values, in the file's order
/// (a frame holding several NUL-separated values gives each). Empty if none repeats.
pub fn repeated_text_frames(path: &Path) -> std::io::Result<Vec<(String, Vec<String>)>> {
    let mut file = File::open(path)?;
    let Some(tag) = find_tag(&mut file)? else {
        return Ok(Vec::new());
    };
    Ok(repeated_in_tag(&tag))
}

/// The ID3v2 tag's bytes (header included), wherever it is.
fn find_tag(file: &mut File) -> std::io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 12];
    let read = read_up_to(file, &mut head)?;
    if read >= 10 && &head[..3] == b"ID3" {
        return read_tag_at(file, 0).map(Some);
    }
    if read < 12 {
        return Ok(None);
    }
    // WAV: "RIFF" <size, little-endian> "WAVE", then chunks; AIFF: "FORM" <size, big-endian>
    // "AIFF"/"AIFC". The tag is the chunk named "id3 " or "ID3 ".
    let big_endian = match (&head[..4], &head[8..12]) {
        (b"RIFF", b"WAVE") => false,
        (b"FORM", b"AIFF") | (b"FORM", b"AIFC") => true,
        _ => return Ok(None),
    };
    let length = file.metadata()?.len();
    let mut pos = 12u64;
    while pos + 8 <= length {
        file.seek(SeekFrom::Start(pos))?;
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)?;
        let size_bytes = [chunk[4], chunk[5], chunk[6], chunk[7]];
        let size = if big_endian { u32::from_be_bytes(size_bytes) } else { u32::from_le_bytes(size_bytes) } as u64;
        if chunk[..4].eq_ignore_ascii_case(b"id3 ") {
            return read_tag_at(file, pos + 8).map(Some);
        }
        // Chunks are padded to an even size.
        pos += 8 + size + (size % 2);
    }
    Ok(None)
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = file.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

/// The tag starting at `offset`: its 10-byte header and the size it gives.
fn read_tag_at(file: &mut File, offset: u64) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0u8; 10];
    file.read_exact(&mut header)?;
    if &header[..3] != b"ID3" {
        return Ok(Vec::new());
    }
    let size = syncsafe(&header[6..10]) as usize;
    let mut tag = header.to_vec();
    tag.resize(10 + size, 0);
    let read = read_up_to(file, &mut tag[10..])?;
    tag.truncate(10 + read);
    Ok(tag)
}

fn syncsafe(b: &[u8]) -> u32 {
    (u32::from(b[0]) << 21) | (u32::from(b[1]) << 14) | (u32::from(b[2]) << 7) | u32::from(b[3])
}

/// See `repeated_text_frames`; `tag` is the whole tag, header included.
fn repeated_in_tag(tag: &[u8]) -> Vec<(String, Vec<String>)> {
    if tag.len() < 10 {
        return Vec::new();
    }
    let version = tag[3];
    let flags = tag[5];
    // Unsynchronised tags (rare) would need undoing first: leave them to lofty.
    if !(version == 3 || version == 4) || flags & 0x80 != 0 {
        return Vec::new();
    }
    let mut pos = 10;
    if flags & 0x40 != 0 && tag.len() >= 14 {
        // An extended header: v2.3 gives its size without these 4 bytes, v2.4 with them.
        let size = if version == 3 {
            u32::from_be_bytes([tag[10], tag[11], tag[12], tag[13]]) as usize + 4
        } else {
            syncsafe(&tag[10..14]) as usize
        };
        pos += size;
    }

    // (frame id, values) for every text frame, in order.
    let mut frames: Vec<(String, Vec<String>)> = Vec::new();
    while pos + 10 <= tag.len() {
        let id = &tag[pos..pos + 4];
        if id[0] == 0 {
            break; // padding
        }
        let size = if version == 3 {
            u32::from_be_bytes([tag[pos + 4], tag[pos + 5], tag[pos + 6], tag[pos + 7]]) as usize
        } else {
            syncsafe(&tag[pos + 4..pos + 8]) as usize
        };
        let format_flags = tag[pos + 9];
        let body_start = pos + 10;
        let body_end = body_start.saturating_add(size).min(tag.len());
        pos = body_start.saturating_add(size);
        let Ok(id) = std::str::from_utf8(id) else { break };
        // Compressed, encrypted, grouped (v2.3: 0x80/0x40/0x20) or unsynchronised / with a
        // length indicator (v2.4: 0x02/0x01, plus 0x08/0x04/0x40): not plain text.
        let unusual = if version == 3 { format_flags & 0xE0 != 0 } else { format_flags & 0x4F != 0 };
        if !id.starts_with('T') || id == "TXXX" || unusual || body_start >= body_end {
            continue;
        }
        frames.push((id.to_string(), decode_text(&tag[body_start..body_end])));
    }

    let mut repeated: Vec<(String, Vec<String>)> = Vec::new();
    for (id, _) in &frames {
        let count = frames.iter().filter(|(other, _)| other == id).count();
        if count > 1 && !repeated.iter().any(|(r, _)| r == id) {
            let values = frames.iter().filter(|(other, _)| other == id).flat_map(|(_, v)| v.clone()).collect();
            repeated.push((id.clone(), values));
        }
    }
    repeated
}

/// A text frame's body: an encoding byte, then the text. Several values are separated
/// by NULs; empty ones are dropped.
fn decode_text(body: &[u8]) -> Vec<String> {
    let (encoding, bytes) = (body[0], &body[1..]);
    let text = match encoding {
        0 => bytes.iter().map(|&b| b as char).collect::<String>(), // ISO-8859-1
        1 | 2 => {
            // UTF-16: with a BOM (1), or big-endian without one (2).
            let mut big_endian = encoding == 2;
            let mut data = bytes;
            if encoding == 1 && data.len() >= 2 {
                match (data[0], data[1]) {
                    (0xFF, 0xFE) => {
                        big_endian = false;
                        data = &data[2..];
                    }
                    (0xFE, 0xFF) => {
                        big_endian = true;
                        data = &data[2..];
                    }
                    _ => {}
                }
            }
            let units: Vec<u16> = data
                .chunks_exact(2)
                .map(|p| if big_endian { u16::from_be_bytes([p[0], p[1]]) } else { u16::from_le_bytes([p[0], p[1]]) })
                .collect();
            // Each value may start with its own BOM.
            String::from_utf16_lossy(&units).replace('\u{FEFF}', "")
        }
        _ => String::from_utf8_lossy(bytes).to_string(),
    };
    text.split('\0').map(str::to_string).filter(|v| !v.trim().is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v2.3 tag with these (id, latin-1 text) frames.
    fn tag(frames: &[(&str, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, text) in frames {
            body.extend(id.as_bytes());
            body.extend(((text.len() + 1) as u32).to_be_bytes());
            body.extend([0, 0, 0]); // flags, then the encoding byte (latin-1)
            body.extend(text.as_bytes());
        }
        let size = body.len() as u32;
        let mut tag = b"ID3\x03\x00\x00".to_vec();
        tag.extend([(size >> 21) as u8 & 0x7F, (size >> 14) as u8 & 0x7F, (size >> 7) as u8 & 0x7F, size as u8 & 0x7F]);
        tag.extend(body);
        tag
    }

    #[test]
    fn repeated_frames_keep_every_value() {
        let t = tag(&[("TIT2", "song"), ("TPE1", "5'4"), ("TPE1", "Afourteen"), ("TPE2", "5'4"), ("TCOM", "A\0B")]);
        assert_eq!(repeated_in_tag(&t), vec![("TPE1".to_string(), vec!["5'4".to_string(), "Afourteen".to_string()])]);
        // Nothing repeats: nothing to say.
        assert!(repeated_in_tag(&tag(&[("TPE1", "a"), ("TPE2", "b")])).is_empty());
    }

    #[test]
    fn utf16_values() {
        // "Ä" in UTF-16LE with a BOM.
        assert_eq!(decode_text(&[1, 0xFF, 0xFE, 0xC4, 0x00]), vec!["Ä".to_string()]);
        assert_eq!(decode_text(&[3, b'a', 0, b'b']), vec!["a".to_string(), "b".to_string()]);
    }
}
