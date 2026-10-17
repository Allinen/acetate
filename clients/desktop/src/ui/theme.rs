//! The look: colors, fonts, spacing. One place; the palette can be switched while the app
//! runs (egui restyles the next frame).

use std::ops::Deref;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

use acetate_core::theme as shared;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals,
};

/// The palette: what every color in the app is read from (`PALETTE`, which is the one
/// chosen in Settings, Appearance).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub sidebar: Color32,
    pub surface: Color32,
    pub surface_hi: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    /// Errors.
    pub danger: Color32,
    /// A light palette gets egui's light widgets as the base.
    pub light: bool,
}

pub use shared::{CustomTheme, DEFAULT_THEME, PRESETS, find};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Palette {
    const fn from_shared(p: shared::Palette) -> Palette {
        Palette {
            bg: rgb(p.bg),
            sidebar: rgb(p.sidebar),
            surface: rgb(p.surface),
            surface_hi: rgb(p.surface_hi),
            line: rgb(p.line),
            text: rgb(p.text),
            muted: rgb(p.muted),
            faint: rgb(p.faint),
            accent: rgb(p.accent),
            accent_soft: rgb(p.accent_soft),
            danger: rgb(p.danger),
            light: p.is_light(),
        }
    }
}

impl From<shared::Palette> for Palette {
    fn from(p: shared::Palette) -> Palette {
        Palette::from_shared(p)
    }
}

/// The palette used until one is chosen.
static DEFAULT: Palette = Palette::from_shared(shared::PRESETS[0].1);

/// The palette in use, as `PALETTE`. Changed rarely (a palette is leaked per change, a few
/// bytes), so reading it is a pointer load and `P.accent` stays what it looks like.
static CURRENT: AtomicPtr<Palette> = AtomicPtr::new(std::ptr::null_mut());

pub struct Current;

pub const PALETTE: Current = Current;

impl Deref for Current {
    type Target = Palette;

    fn deref(&self) -> &Palette {
        let current = CURRENT.load(Ordering::Acquire);
        // SAFETY: only ever null or a leaked, never freed, Palette.
        if current.is_null() { &DEFAULT } else { unsafe { &*current } }
    }
}

/// Use `palette` from the next frame on.
pub fn set_palette(ctx: &egui::Context, palette: Palette) {
    if *PALETTE != palette {
        CURRENT.store(Box::leak(Box::new(palette)), Ordering::Release);
    }
    restyle(ctx);
    ctx.request_repaint();
}

/// Font families by weight (egui picks fonts by family, not by weight).
pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn font(size: f32, family: FontFamily) -> FontId {
    FontId::new(size, family)
}

/// The bundled Inter (all platforms look the same), with system fallbacks for the scripts
/// it lacks (CJK, Arabic, Hebrew, Indic, Thai) where the system has them.
fn fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let inter = [
        ("inter", &include_bytes!("../../../assets/fonts/Inter-Regular.ttf")[..]),
        ("inter-medium", &include_bytes!("../../../assets/fonts/Inter-Medium.ttf")[..]),
        ("inter-semibold", &include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf")[..]),
        ("inter-bold", &include_bytes!("../../../assets/fonts/Inter-Bold.ttf")[..]),
    ];
    for (name, bytes) in inter {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // System fonts for what Inter lacks, in order (first one with the glyph wins). Each
    // entry is the paths to try for one script; the first that exists is loaded.
    let scripts: [&[&str]; 6] = [
        // Chinese and Japanese. Windows: Microsoft YaHei, else Yu Gothic.
        &["/usr/share/fonts/google-droid-sans-fonts/DroidSansFallbackFull.ttf", r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\YuGothR.ttc"],
        // Arabic and Hebrew (Segoe UI has both).
        &["/usr/share/fonts/google-noto-vf/NotoSansArabic[wght].ttf", "/usr/share/fonts/noto/NotoSansArabic-Regular.ttf", r"C:\Windows\Fonts\segoeui.ttf"],
        &["/usr/share/fonts/google-noto-vf/NotoSansHebrew[wght].ttf", "/usr/share/fonts/noto/NotoSansHebrew-Regular.ttf"],
        // Indic scripts (Nirmala UI has them all).
        &["/usr/share/fonts/google-noto-vf/NotoSansDevanagari[wght].ttf", "/usr/share/fonts/noto/NotoSansDevanagari-Regular.ttf", r"C:\Windows\Fonts\Nirmala.ttc"],
        &["/usr/share/fonts/google-noto-vf/NotoSansThai[wght].ttf", "/usr/share/fonts/noto/NotoSansThai-Regular.ttf", r"C:\Windows\Fonts\LeelawUI.ttf"],
        // Korean.
        &["/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", r"C:\Windows\Fonts\malgun.ttf"],
    ];
    let mut system = Vec::new();
    for (i, paths) in scripts.iter().enumerate() {
        if let Some(bytes) = paths.iter().find_map(|path| std::fs::read(path).ok()) {
            let name = format!("system-{i}");
            fonts.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            system.push(name);
        }
    }
    // egui's own fonts stay as fallbacks for symbols (▶, ⏸...).
    let fallbacks: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    let family = |first: &str| {
        let mut list = vec![first.to_string()];
        list.extend(system.iter().cloned());
        list.extend(fallbacks.iter().cloned());
        list
    };
    fonts.families.insert(FontFamily::Proportional, family("inter"));
    fonts.families.insert(medium(), family("inter-medium"));
    fonts.families.insert(semibold(), family("inter-semibold"));
    fonts.families.insert(bold(), family("inter-bold"));
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    fonts(ctx);
    restyle(ctx);
}

/// Everything that follows the palette.
fn restyle(ctx: &egui::Context) {
    let p = &*PALETTE;
    let mut visuals = if p.light { Visuals::light() } else { Visuals::dark() };
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.surface;
    visuals.extreme_bg_color = p.surface;
    visuals.faint_bg_color = p.surface;
    visuals.override_text_color = Some(p.text);
    visuals.hyperlink_color = p.accent;
    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0, p.accent);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.window_stroke = Stroke::new(1.0, p.line);
    for (widget, fill) in [
        (&mut visuals.widgets.noninteractive, p.bg),
        (&mut visuals.widgets.inactive, p.surface),
        (&mut visuals.widgets.hovered, p.surface_hi),
        (&mut visuals.widgets.active, p.surface_hi),
        (&mut visuals.widgets.open, p.surface_hi),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.corner_radius = CornerRadius::same(8);
        widget.bg_stroke = Stroke::NONE;
        widget.fg_stroke.color = p.text;
    }
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.line);
    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.text_styles = [
            (TextStyle::Heading, font(30.0, bold())),
            (TextStyle::Body, font(14.0, FontFamily::Proportional)),
            (TextStyle::Button, font(14.0, medium())),
            (TextStyle::Small, font(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, font(13.0, FontFamily::Monospace)),
        ]
        .into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every symbol the app draws as text is in its fonts (else it's drawn as a box).
    #[test]
    fn the_symbols_used_exist_in_the_fonts() {
        let ctx = egui::Context::default();
        apply(&ctx);
        // A frame, so the fonts are loaded.
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        // (Not "✕" (U+2715): it isn't there. "×" is.)
        let missing: Vec<char> = "→↓↑←▶−×✓✗♪●⏸…·“”’"
            .chars()
            .filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&font(14.0, medium()), *c)))
            .collect();
        assert!(missing.is_empty(), "not in the fonts: {missing:?}");
    }
}
