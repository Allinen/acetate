//! Files given to the app from outside: dropped on the window, pasted (Ctrl+V after
//! copying files in a file manager), or chosen in the system's file dialog.
//!
//! Dropping only works where the windowing library delivers it: X11, Windows and macOS.
//! On Wayland it never does (winit has no drag and drop there), so a drop does nothing;
//! pasting copied files and the dialog (the desktop's own, through its portal) work
//! everywhere. `ACETATE_X11=1` runs the app through XWayland, where dropping works too.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key};

/// Files dropped or pasted this frame (paths; not read yet).
pub fn given(ctx: &egui::Context) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
    let pasted = ctx.input(|i| {
        i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)) || matches!(e, egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command))
    });
    if pasted && let Ok(list) = arboard::Clipboard::new().and_then(|mut c| c.get().file_list()) {
        files.extend(list.into_iter().map(without_line_end));
    }
    files.dedup();
    files
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
/// `kinds`: what it offers (a name, and extensions without the dot).
pub fn choose(shell: &crate::ui::Shell, title: &str, kinds: (&str, &[&str]), multiple: bool, answer: &Arc<Mutex<Vec<PathBuf>>>) {
    let (title, name, extensions) = (title.to_string(), kinds.0.to_string(), kinds.1.iter().map(|e| e.to_string()).collect::<Vec<_>>());
    let (answer, ctx) = (answer.clone(), shell.ctx.clone());
    shell.runtime.spawn(async move {
        let dialog = rfd::AsyncFileDialog::new().set_title(&title).add_filter(&name, &extensions);
        let chosen: Vec<PathBuf> = if multiple {
            dialog.pick_files().await.unwrap_or_default().iter().map(|f| f.path().to_path_buf()).collect()
        } else {
            dialog.pick_file().await.map(|f| vec![f.path().to_path_buf()]).unwrap_or_default()
        };
        answer.lock().unwrap().extend(chosen);
        ctx.request_repaint();
    });
}

/// A file's extension, lowercase ("" without one).
pub fn extension(path: &std::path::Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

/// Image files covers are made from.
pub const IMAGES: &[&str] = &["jpg", "jpeg", "png", "webp", "gif"];

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
