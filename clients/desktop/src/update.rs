//! Updating the app to the server's release: downloads the release from GitHub (the
//! archive `package-linux.sh` / `package-windows.sh` make) and puts its program over the
//! running one. It never goes back: a release that's older than the app is only said to
//! be (an older server can't read what a newer app keeps, like the catalog's file).
//!
//! The running program is replaced on disk and keeps running; the new one starts the next
//! time the app does.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use acetate_core::account::ServerVersion;
use acetate_core::Waker;

/// What the update is doing, for Settings → General.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Progress {
    #[default]
    Idle,
    /// Bytes so far, and in all if the server said.
    Downloading(u64, Option<u64>),
    Installing,
    /// The version now installed (it starts with the next run).
    Done(String),
    Failed(String),
}

#[derive(Clone, Default)]
pub struct Update(Arc<Mutex<Progress>>);

/// The system the archive is for (the releases have these two).
const OS: &str = if cfg!(windows) { "windows" } else { "linux" };
/// The program in the archive, and what else to put next to it.
const PROGRAM: &str = if cfg!(windows) { "Acetate.exe" } else { "acetate-desktop" };
const ALONGSIDE: &[&str] = if cfg!(windows) { &["libmpv-2.dll"] } else { &[] };

impl Update {
    pub fn progress(&self) -> Progress {
        self.0.lock().unwrap().clone()
    }

    pub fn busy(&self) -> bool {
        matches!(self.progress(), Progress::Downloading(..) | Progress::Installing)
    }

    /// Downloads and installs the server's release, in the background.
    pub fn start(&self, runtime: &tokio::runtime::Handle, server: &ServerVersion, waker: &Waker) {
        if self.busy() {
            return;
        }
        let (state, waker, url, version) = (self.0.clone(), waker.clone(), server.desktop_url(OS), server.version.clone());
        let set = move |p: Progress| {
            *state.lock().unwrap() = p;
            waker.changed();
        };
        let state = Arc::new(State { set: Box::new(set) });
        state.set(Progress::Downloading(0, None));
        runtime.spawn(async move {
            let result = async {
                // The release's checksum for it (release.yml makes `<file>.sha256`), fetched
                // first: no checksum, no update.
                let sum = fetch_checksum(&url).await?;
                let archive = download(&url, &state).await?;
                verify(&archive, &sum)?;
                state.set(Progress::Installing);
                tokio::task::spawn_blocking(move || {
                    let exe = std::env::current_exe().map_err(|e| format!("Can't find where the app is: {e}"))?;
                    install(&archive, url.ends_with(".zip"), &exe)
                })
                .await
                .map_err(|e| e.to_string())?
            }
            .await;
            state.set(match result {
                Ok(()) => Progress::Done(version),
                Err(err) => Progress::Failed(err),
            });
        });
    }
}

struct State {
    set: Box<dyn Fn(Progress) + Send + Sync>,
}

impl State {
    fn set(&self, p: Progress) {
        (self.set)(p)
    }
}

async fn fetch_checksum(url: &str) -> Result<String, String> {
    let url = format!("{url}.sha256");
    let response = reqwest::get(&url).await.map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("The release has no checksum to check the download against ({url}: {}).", response.status()));
    }
    response.text().await.map_err(|e| format!("Couldn't read the checksum: {e}"))
}

/// Whether the download is what the checksum file (`sha256sum`'s: the hash, then the
/// name) says.
fn verify(archive: &[u8], checksum_file: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let expected = checksum_file.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if hex::encode(Sha256::digest(archive)) == expected {
        Ok(())
    } else {
        Err("The download doesn't match its checksum, so it wasn't installed.".into())
    }
}

async fn download(url: &str, state: &State) -> Result<Vec<u8>, String> {
    let mut response = reqwest::get(url).await.map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GitHub has no {url} ({})", response.status()));
    }
    let total = response.content_length();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(1 << 30) as usize);
    let mut last = std::time::Instant::now();
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("The download stopped: {e}"))? {
        bytes.extend_from_slice(&chunk);
        // (A repaint per chunk would be a lot.)
        if last.elapsed().as_millis() > 100 {
            state.set(Progress::Downloading(bytes.len() as u64, total));
            last = std::time::Instant::now();
        }
    }
    if let Some(total) = total
        && total != bytes.len() as u64
    {
        return Err("The download was cut short.".into());
    }
    Ok(bytes)
}

/// The files of the archive that go on disk, by name (they're in a folder, `Acetate/`).
fn unpack(archive: &[u8], zip: bool) -> Result<Vec<(String, Vec<u8>)>, String> {
    let wanted = |name: &str| (name == PROGRAM || ALONGSIDE.contains(&name)).then(|| name.to_string());
    let mut files = Vec::new();
    if zip {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|e| format!("The download isn't a zip: {e}"))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let Some(name) = entry.enclosed_name().and_then(|p| wanted(p.file_name()?.to_str()?)) else { continue };
            let mut data = Vec::new();
            entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
            files.push((name, data));
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        for entry in tar.entries().map_err(|e| format!("The download isn't a tar.gz: {e}"))? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let Some(name) = entry.path().ok().and_then(|p| wanted(p.file_name()?.to_str()?)) else { continue };
            let mut data = Vec::new();
            entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
            files.push((name, data));
        }
    }
    if !files.iter().any(|(name, _)| name == PROGRAM) {
        return Err(format!("The download has no {PROGRAM} in it."));
    }
    Ok(files)
}

/// Puts the archive's program in place of `exe` (whatever it's called), and its other
/// files next to it. Each is written beside its place first and then renamed over it, so
/// a failure half-way leaves the old file, not half a new one. Windows won't overwrite a
/// running program but will rename it: the old one is set aside (`…old`, removed by
/// `clean_up` at the next start).
fn install(archive: &[u8], zip: bool, exe: &Path) -> Result<(), String> {
    let files = unpack(archive, zip)?;
    let dir = exe.parent().ok_or("Can't find the app's folder.")?;
    for (name, data) in files {
        let place = if name == PROGRAM { exe.to_path_buf() } else { dir.join(&name) };
        let new = with_suffix(&place, "new");
        let cant = |e: std::io::Error| format!("Can't write {}: {e}", place.display());
        std::fs::write(&new, &data).map_err(cant)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(cant)?;
        }
        if cfg!(windows) && place.exists() {
            let old = with_suffix(&place, "old");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&place, &old).map_err(cant)?;
        }
        if let Err(err) = std::fs::rename(&new, &place) {
            let _ = std::fs::remove_file(&new);
            return Err(cant(err));
        }
    }
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{suffix}"));
    path.with_file_name(name)
}

/// Windows: removes what an update set aside, now that nothing runs from it.
pub fn clean_up() {
    let Ok(exe) = std::env::current_exe() else { return };
    for name in std::iter::once(PROGRAM).chain(ALONGSIDE.iter().copied()) {
        let _ = std::fs::remove_file(with_suffix(&exe.with_file_name(name), "old"));
    }
    let _ = std::fs::remove_file(with_suffix(&exe, "old"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("Acetate/{name}"), *data).unwrap();
        }
        let tar = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn a_download_must_match_its_checksum() {
        use sha2::{Digest, Sha256};
        let sum = hex::encode(Sha256::digest(b"archive"));
        assert!(verify(b"archive", &format!("{sum}  Acetate-v1.0.0-linux-x86_64.tar.gz\n")).is_ok());
        assert!(verify(b"archive", &sum.to_uppercase()).is_ok());
        assert!(verify(b"tampered", &format!("{sum}  x\n")).is_err());
        assert!(verify(b"archive", "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_program_replaces_the_running_one_and_nothing_else_does() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("renamed-by-the-user");
        std::fs::write(&exe, b"old").unwrap();
        let archive = tar_gz(&[("acetate-desktop", b"new"), ("acetate.desktop", b"entry"), ("assets/acetate-512.jpg", b"icon")]);
        install(&archive, false, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        let mut left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        left.sort();
        assert_eq!(left, ["renamed-by-the-user"], "no leftovers, no other files");
    }

    #[cfg(unix)]
    #[test]
    fn an_archive_without_the_program_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("acetate-desktop");
        std::fs::write(&exe, b"old").unwrap();
        assert!(install(&tar_gz(&[("other", b"x")]), false, &exe).is_err());
        assert!(install(b"not an archive", false, &exe).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }
}
