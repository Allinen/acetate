//! Where the app keeps its things: one folder, the *root*. Settings, the session, the
//! library kept locally, playlists, the queue, and the mirror (its blobs and index) all
//! live there. Scaled covers don't: they're a cache, kept in the system's cache folder
//! (`cache_dir`), where backups usually skip them.
//!
//! The root is the system's data folder for the app unless moved: then a small pointer
//! (`root.json`) is left in that folder saying where it went. `ACETATE_ROOT` overrides
//! both (for running the mirror headless against some other root).
//!
//! Moving the root moves everything in it (`plan_move`, then `move_root`): a rename when
//! it stays on the same filesystem; otherwise a copy, each file's hash checked against the
//! original's before the original is removed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// The pointer left in the default root when the root is elsewhere.
const POINTER: &str = "root.json";

#[derive(Serialize, Deserialize)]
struct Pointer {
    root: PathBuf,
}

/// The app's folder in `base` (the system's data or cache folder). One under the old name
/// is renamed to the new one.
fn app_dir(base: Option<PathBuf>) -> PathBuf {
    const NAME: &str = "allinen.acetate.desktop";
    const OLD: &str = "dev.acetate.desktop";
    let base = base.unwrap_or_default();
    let (dir, old) = (base.join(NAME), base.join(OLD));
    if !dir.exists() && old.exists()
        && let Err(err) = std::fs::rename(&old, &dir)
    {
        // It starts afresh; the old folder stays as it was.
        eprintln!("couldn't move {} to {}: {err}", old.display(), dir.display());
    }
    dir
}

/// The root when it hasn't been moved. Local: on Windows, not the roaming folder (the
/// library kept here is hundreds of MB, the mirror far more).
pub fn default_root() -> PathBuf {
    app_dir(dirs::data_local_dir())
}

/// Where copies of things are kept (scaled covers): the system's cache folder, whatever
/// the root.
pub fn cache_dir() -> PathBuf {
    app_dir(dirs::cache_dir())
}

/// The root: `ACETATE_ROOT`, else where the pointer says, else the default.
pub fn root() -> PathBuf {
    if let Some(root) = std::env::var_os("ACETATE_ROOT").filter(|r| !r.is_empty()) {
        return PathBuf::from(root);
    }
    let default = default_root();
    read_pointer(&default).unwrap_or(default)
}

fn read_pointer(default: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(default.join(POINTER)).ok()?;
    Some(serde_json::from_slice::<Pointer>(&bytes).ok()?.root)
}

/// Points the default root at `root` (or, if `root` is the default, removes the pointer).
fn write_pointer(default: &Path, root: &Path) -> anyhow::Result<()> {
    let pointer = default.join(POINTER);
    if root == default {
        match std::fs::remove_file(&pointer) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
            _ => return Ok(()),
        }
    }
    std::fs::create_dir_all(default)?;
    let bytes = serde_json::to_vec_pretty(&Pointer { root: root.to_path_buf() })?;
    let temp = default.join(format!("{POINTER}.new"));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, &pointer).with_context(|| format!("writing {}", pointer.display()))
}

/// One thing in the root, as a move would take it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: OsString,
    pub files: u64,
    pub bytes: u64,
}

/// What moving the root would do: shown before it's done.
#[derive(Debug, Clone)]
pub struct MovePlan {
    pub from: PathBuf,
    pub to: PathBuf,
    pub entries: Vec<Entry>,
    /// The same filesystem: each entry is renamed, instantly. Else everything is copied
    /// (and checked) first.
    pub same_filesystem: bool,
    /// Free space where it's going (None: couldn't tell).
    pub free_bytes: Option<u64>,
}

impl MovePlan {
    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }

    pub fn files(&self) -> u64 {
        self.entries.iter().map(|e| e.files).sum()
    }
}

/// What moving the root from `from` to `to` would do. Refuses a `to` inside `from` (or
/// the other way round), and one that already has something of the same name in it:
/// nothing is ever written over.
pub fn plan_move(from: &Path, to: &Path) -> anyhow::Result<MovePlan> {
    let (from_real, to_real) = (from.canonicalize().unwrap_or(from.to_path_buf()), absolute(to)?);
    if from_real == to_real {
        anyhow::bail!("that's where it is already");
    }
    if to_real.starts_with(&from_real) || from_real.starts_with(&to_real) {
        anyhow::bail!("one folder can't be inside the other");
    }
    let mut entries = Vec::new();
    let mut clashes = Vec::new();
    for item in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let item = item?;
        let name = item.file_name();
        if name == POINTER {
            continue;
        }
        if to.join(&name).symlink_metadata().is_ok() {
            clashes.push(name.to_string_lossy().into_owned());
        }
        let (files, bytes) = measure(&item.path())?;
        entries.push(Entry { name, files, bytes });
    }
    if !clashes.is_empty() {
        anyhow::bail!("{} already has {}: nothing was moved", to.display(), clashes.join(", "));
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let target = existing_ancestor(&to_real);
    Ok(MovePlan {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        entries,
        same_filesystem: same_filesystem(from, &target),
        free_bytes: fs4::available_space(&target).ok(),
    })
}

/// Moves the root as planned, then points the default root at it. `done(bytes)` hears how
/// far it got (copies only; renames are instant). An entry copied goes in under a staging
/// name (`.<name>.moving`) and gets its own name only once all of it is checked; then the
/// original is removed. Stops at the first thing that fails: what was moved so far is in
/// `to`, the rest still in `from`, and the pointer unchanged, so the app keeps using `from`.
/// Planning and moving again goes on from there (a staging copy left over is started over).
pub fn move_root(plan: &MovePlan, done: &dyn Fn(u64)) -> anyhow::Result<()> {
    std::fs::create_dir_all(&plan.to).with_context(|| format!("creating {}", plan.to.display()))?;
    let mut copied = 0;
    for entry in &plan.entries {
        let (source, target) = (plan.from.join(&entry.name), plan.to.join(&entry.name));
        match std::fs::rename(&source, &target) {
            Ok(()) => continue,
            Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {}
            Err(err) => return Err(err).with_context(|| format!("moving {}", source.display())),
        }
        let mut staging = OsString::from(".");
        staging.push(&entry.name);
        staging.push(".moving");
        let staging = plan.to.join(staging);
        if staging.symlink_metadata().is_ok() {
            remove_tree(&staging)?;
        }
        copy_checked(&source, &staging, &mut copied, done)?;
        std::fs::rename(&staging, &target)?;
        remove_tree(&source)?;
    }
    write_pointer(&default_root(), &plan.to)?;
    Ok(())
}

/// Copies `source` (a file or a folder) to `target`, checking each file's hash against
/// the original's.
fn copy_checked(source: &Path, target: &Path, copied: &mut u64, done: &dyn Fn(u64)) -> anyhow::Result<()> {
    let meta = source.symlink_metadata()?;
    if meta.is_dir() {
        std::fs::create_dir_all(target)?;
        for item in std::fs::read_dir(source)? {
            let item = item?;
            copy_checked(&item.path(), &target.join(item.file_name()), copied, done)?;
        }
        return Ok(());
    }
    if meta.is_symlink() {
        anyhow::bail!("{} is a link: not moving it", source.display());
    }
    std::fs::copy(source, target).with_context(|| format!("copying {}", source.display()))?;
    std::fs::File::open(target)?.sync_all()?;
    let (expected, _) = crate::mirror::store::hash_file(source)?;
    let (got, _) = crate::mirror::store::hash_file(target)?;
    if expected != got {
        anyhow::bail!("the copy of {} isn't the same as the original: nothing more was moved", source.display());
    }
    *copied += meta.len();
    done(*copied);
    Ok(())
}

/// Removes a file or folder, read-only files too (Windows won't remove those otherwise).
pub fn remove_tree(path: &Path) -> std::io::Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_dir() {
        for item in std::fs::read_dir(path)? {
            remove_tree(&item?.path())?;
        }
        return std::fs::remove_dir(path);
    }
    remove_file(path)
}

/// Removes a file, also a read-only one.
pub fn remove_file(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if let Ok(meta) = path.metadata() {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    std::fs::remove_file(path)
}

/// Files and bytes in a file or folder (links not followed).
fn measure(path: &Path) -> std::io::Result<(u64, u64)> {
    let meta = path.symlink_metadata()?;
    if !meta.is_dir() {
        return Ok((1, meta.len()));
    }
    let (mut files, mut bytes) = (0, 0);
    for item in std::fs::read_dir(path)? {
        let (f, b) = measure(&item?.path())?;
        files += f;
        bytes += b;
    }
    Ok((files, bytes))
}

fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    // Resolve what exists of it (links), keep the rest as given.
    let existing = existing_ancestor(&path);
    let rest = path.strip_prefix(&existing).unwrap_or(Path::new(""));
    Ok(existing.canonicalize().unwrap_or(existing).join(rest))
}

/// The nearest folder of `path` that exists (itself, if it does).
fn existing_ancestor(path: &Path) -> PathBuf {
    path.ancestors().find(|p| p.exists()).unwrap_or(Path::new("/")).to_path_buf()
}

#[cfg(unix)]
fn same_filesystem(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    matches!((a.metadata(), b.metadata()), (Ok(a), Ok(b)) if a.dev() == b.dev())
}

/// Elsewhere: can't tell cheaply; a rename across drives fails and falls back to copying.
#[cfg(not(unix))]
fn same_filesystem(a: &Path, b: &Path) -> bool {
    a.components().next() == b.components().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_takes_everything_and_refuses_to_write_over_anything() {
        let (from, base) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(from.path().join("settings.json"), b"{}").unwrap();
        std::fs::create_dir_all(from.path().join("blobs/ab/cd")).unwrap();
        std::fs::write(from.path().join("blobs/ab/cd/x"), b"12345").unwrap();
        std::fs::write(from.path().join(POINTER), b"{}").unwrap();
        let to = base.path().join("new root");

        let plan = plan_move(from.path(), &to).unwrap();
        let names: Vec<_> = plan.entries.iter().map(|e| e.name.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["blobs", "settings.json"], "not the pointer");
        assert_eq!((plan.files(), plan.bytes()), (2, 7));
        assert!(plan.same_filesystem || cfg!(not(unix)));

        // Inside itself, or onto something already there: refused.
        assert!(plan_move(from.path(), &from.path().join("sub")).is_err());
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join("settings.json"), b"theirs").unwrap();
        let clash = plan_move(from.path(), &to).unwrap_err().to_string();
        assert!(clash.contains("settings.json"), "{clash}");
        std::fs::remove_file(to.join("settings.json")).unwrap();

        // Copied and checked (as across filesystems), then the originals removed.
        let mut copied = 0;
        for entry in &plan.entries {
            copy_checked(&from.path().join(&entry.name), &to.join(&entry.name), &mut copied, &|_| {}).unwrap();
            remove_tree(&from.path().join(&entry.name)).unwrap();
        }
        assert_eq!(copied, 7);
        assert_eq!(std::fs::read(to.join("blobs/ab/cd/x")).unwrap(), b"12345");
        assert!(!from.path().join("blobs").exists());
        assert!(from.path().join(POINTER).exists());
    }

    #[test]
    fn the_pointer_says_where_the_root_went() {
        let default = tempfile::tempdir().unwrap();
        assert_eq!(read_pointer(default.path()), None);
        write_pointer(default.path(), Path::new("/somewhere/else")).unwrap();
        assert_eq!(read_pointer(default.path()), Some(PathBuf::from("/somewhere/else")));
        // Back home: no pointer.
        write_pointer(default.path(), default.path()).unwrap();
        assert_eq!(read_pointer(default.path()), None);
    }
}
