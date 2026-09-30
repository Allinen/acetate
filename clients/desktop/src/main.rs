//! Acetate's desktop client: drawn by egui on the GPU, no webview. It keeps the library
//! (as you may see it) locally, so browsing, sorting and filtering never wait on the
//! network, and plays natively and gapless.
//!
//! - `api`: talking to the server; `account`: who's logged in, what they may do.
//! - `catalog`: the library kept locally, and its lookups.
//! - `details`: everything about one track, album or artist, for their pages.
//! - `links`: links to share, and opening the app from one (`acetate://`).
//! - `lyrics`: synced lyrics (LRC).
//! - `playback`: the queue and the audio.
//! - `playlists`: playlists, kept locally like the catalog.
//! - `resume`: the queue kept between runs.
//! - `settings`: volume and sound output.
//! - `ui`: the window, its pages and the pieces they're drawn with.

// Answers from background work arrive in shared slots (`Arc<Mutex<Option<…>>>`), which
// clippy counts as "very complex types"; drawing code takes many plain arguments.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

mod account;
mod api;
mod catalog;
mod details;
mod links;
mod lyrics;
mod playback;
mod playlists;
mod resume;
mod review;
mod settings;
mod video;
mod ui;

fn main() -> eframe::Result {
    // Started for a link (`acetate://…`, see `links`): the app that's running already
    // opens it, if there is one; else this start does, once it's up.
    let link = std::env::args().skip(1).find(|a| a.starts_with("acetate://"));
    if let Some(link) = &link
        && links::hand_over(link)
    {
        return Ok(());
    }
    // The simple logo (the full one is for the README, used sparingly).
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/brand/acetate-simple.jpg")).expect("the app icon");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Acetate")
            .with_app_id("acetate")
            .with_icon(icon)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0]),
        event_loop_builder: x11_unless_asked_not_to(),
        ..Default::default()
    };
    eframe::run_native("Acetate", options, Box::new(move |cc| Ok(Box::new(ui::App::new(cc, link)))))
}

/// Linux: through X11 (XWayland on a Wayland desktop) unless `ACETATE_WAYLAND=1`: winit has
/// no drag and drop on Wayland (see `ui::files`), and through XWayland it works, with no
/// other difference noticed. `ACETATE_WAYLAND=1` runs natively on Wayland (no dropping
/// files; pasting copied ones and the file dialog still work).
fn x11_unless_asked_not_to() -> Option<eframe::EventLoopBuilderHook> {
    #[cfg(target_os = "linux")]
    if !std::env::var("ACETATE_WAYLAND").is_ok_and(|v| !v.is_empty() && v != "0") {
        return Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }));
    }
    None
}
