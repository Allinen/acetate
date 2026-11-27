//! Files given to the app from outside: dropped on the window, pasted (Ctrl+V after
//! copying files in a file manager), or chosen in the system's file dialog.
//!
//! Dropping only works where the windowing library delivers it: X11, Windows and macOS.
//! On Wayland it never does (winit has no drag and drop there), so on Linux the app runs
//! through X11 (XWayland) unless `ACETATE_WAYLAND=1`; pasting copied files and the dialog
//! (the desktop's own, through its portal) work everywhere.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key};

/// Files dropped or pasted this frame (paths; not read yet).
pub fn given(ctx: &egui::Context) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
    let pasted = ctx.input(|i| {
        i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)) || matches!(e, egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command))
    });
    if pasted {
        files.extend(clipboard_files());
    }
    files.dedup();
    files
}

/// The files copied in a file manager (none: nothing copied, or not files).
pub fn clipboard_files() -> Vec<PathBuf> {
    match arboard::Clipboard::new().and_then(|mut c| c.get().file_list()) {
        Ok(list) => list.into_iter().map(without_line_end).collect(),
        Err(_) => Vec::new(),
    }
}

/// A copied path as the clipboard gives it: `text/uri-list` lines end in CRLF, and the `\r`
/// is left on (a file that can't be opened).
fn without_line_end(path: PathBuf) -> PathBuf {
    match path.to_str() {
        Some(text) if text.ends_with(['\r', '\n']) => PathBuf::from(text.trim_end_matches(['\r', '\n'])),
        _ => path,
    }
}

/// The system's file dialog, asked in the background; what was chosen arrives in `answer`.
/// `kinds`: what it offers (a name, and extensions without the dot; none: any file).
pub fn choose(shell: &crate::ui::Shell, title: &str, kinds: (&str, &[&str]), multiple: bool, answer: &Arc<Mutex<Vec<PathBuf>>>) {
    let (title, name, extensions) = (title.to_string(), kinds.0.to_string(), kinds.1.iter().map(|e| e.to_string()).collect::<Vec<_>>());
    let (answer, ctx) = (answer.clone(), shell.ctx.clone());
    shell.runtime.spawn(async move {
        let mut dialog = rfd::AsyncFileDialog::new().set_title(&title);
        if !extensions.is_empty() {
            dialog = dialog.add_filter(&name, &extensions);
        }
        let chosen: Vec<PathBuf> = if multiple {
            dialog.pick_files().await.unwrap_or_default().iter().map(|f| f.path().to_path_buf()).collect()
        } else {
            dialog.pick_file().await.map(|f| vec![f.path().to_path_buf()]).unwrap_or_default()
        };
        answer.lock().unwrap().extend(chosen);
        ctx.request_repaint();
    });
}

/// The system's folder dialog, asked in the background; the folder chosen arrives in
/// `answer`.
pub fn choose_folder(shell: &crate::ui::Shell, title: &str, answer: &Arc<Mutex<Vec<PathBuf>>>) {
    let (title, answer, ctx) = (title.to_string(), answer.clone(), shell.ctx.clone());
    shell.runtime.spawn(async move {
        if let Some(folder) = rfd::AsyncFileDialog::new().set_title(&title).pick_folder().await {
            answer.lock().unwrap().push(folder.path().to_path_buf());
            ctx.request_repaint();
        }
    });
}

/// A file's extension, lowercase ("" without one).
pub fn extension(path: &std::path::Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

/// Image files covers are made from.
pub const IMAGES: &[&str] = &["jpg", "jpeg", "png", "webp", "gif"];

/// A file to send, with the name it's kept under: its path under the folder that was given
/// (`day 1/a.png`, `/` between folders), or its own name when it was given itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub path: PathBuf,
    pub name: String,
}

/// What's left out of a folder: what file managers and operating systems put there.
fn left_out(name: &str) -> bool {
    name.starts_with('.') || ["thumbs.db", "desktop.ini"].contains(&name.to_lowercase().as_str())
}

/// The files given, with every file in a folder given (in its subfolders too, but not
/// through links), by path; a folder's files are named by where they are in it. Fails on a
/// folder that can't be read, before anything is sent.
pub fn expand(given: &[PathBuf]) -> std::io::Result<Vec<Named>> {
    fn walk(root: &std::path::Path, folder: &std::path::Path, into: &mut Vec<Named>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(folder)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let kind = entry.file_type()?;
            if left_out(&name) || kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                walk(root, &entry.path(), into)?;
            } else if kind.is_file() {
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let name = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                into.push(Named { path, name });
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    for path in given {
        if path.is_dir() {
            walk(path, path, &mut files)?;
        } else {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            files.push(Named { path: path.clone(), name });
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_paths_lose_the_line_end() {
        assert_eq!(without_line_end("/tmp/two words.txt\r".into()), PathBuf::from("/tmp/two words.txt"));
        assert_eq!(without_line_end("/tmp/a.wav".into()), PathBuf::from("/tmp/a.wav"));
    }

    /// Reads a file list copied to this desktop's clipboard (copy files in a file manager,
    /// or `printf 'file:///tmp/x.wav\r\n' | wl-copy -t text/uri-list`), as Ctrl+V does:
    /// `ACETATE_TEST_CLIPBOARD=1 cargo test copied_files -- --ignored --nocapture`.
    #[test]
    fn folders_are_given_with_their_files_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let shoot = dir.path().join("Shoot");
        std::fs::create_dir_all(shoot.join("day 1/raw")).unwrap();
        for file in ["b.png", "a.mp4", ".DS_Store", "Thumbs.db", "day 1/c.jpg", "day 1/raw/d.jpg"] {
            std::fs::write(shoot.join(file), b"x").unwrap();
        }
        let loose = dir.path().join("loose.txt");
        std::fs::write(&loose, b"x").unwrap();
        let names: Vec<String> = expand(&[shoot, loose]).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["a.mp4", "b.png", "day 1/c.jpg", "day 1/raw/d.jpg", "loose.txt"]);
    }

    #[test]
    #[ignore]
    fn copied_files_are_read_from_the_clipboard() {
        if std::env::var("ACETATE_TEST_CLIPBOARD").is_err() {
            return;
        }
        let list = arboard::Clipboard::new().and_then(|mut c| c.get().file_list()).expect("a file list on the clipboard");
        println!("{list:?}");
        assert!(!list.is_empty());
    }
}
