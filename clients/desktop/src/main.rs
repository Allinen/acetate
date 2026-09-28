//! Acetate's desktop client: drawn by egui on the GPU, no webview. It keeps the library
//! (as you may see it) locally, so browsing, sorting and filtering never wait on the
//! network, and plays natively and gapless.
//!
//! - `api`: talking to the server.
//! - `catalog`: the library kept locally, and its lookups.
//! - `details`: everything about one track, album or artist, for their pages.
//! - `lyrics`: synced lyrics (LRC).
//! - `playback`: the queue and the audio.
//! - `playlists`: playlists, kept locally like the catalog.
//! - `resume`: the queue kept between runs.
//! - `settings`: volume and sound output.
//! - `ui`: the window, its pages and the pieces they're drawn with.

mod api;
mod catalog;
mod details;
mod lyrics;
mod playback;
mod playlists;
mod resume;
mod settings;
mod ui;

fn main() -> eframe::Result {
    // The simple logo (the full one is for the README, used sparingly).
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/brand/acetate-simple.png")).expect("the app icon");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Acetate")
            .with_app_id("acetate")
            .with_icon(icon)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("Acetate", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))))
}
