//! Acetate's desktop client: drawn by egui on the GPU, no webview. It keeps the library
//! (as you may see it) locally, so browsing, sorting and filtering never wait on the
//! network, and plays natively and gapless.
//!
//! - `api`: talking to the server.
//! - `catalog`: the library kept locally, and its lookups.
//! - `playback`: the queue and the audio.
//! - `playlists`: playlists, kept locally like the catalog.
//! - `ui`: the window, its pages and the pieces they're drawn with.

mod api;
mod catalog;
mod playback;
mod playlists;
mod ui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Acetate")
            .with_app_id("acetate")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("Acetate", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))))
}
