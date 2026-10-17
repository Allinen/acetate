//! The look: colors, fonts, spacing. One place; the palette can be switched while the app
//! runs (egui restyles the next frame).

use std::ops::Deref;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

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
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The built-in palettes, the first the default.
pub const PRESETS: [(&str, Palette); 6] = [
    // Warm amber on near-black: acetate discs are lacquer on black, and the label is where
    // the color is.
    ("Amber", Palette {
        bg: rgb(0x0d0e12),
        sidebar: rgb(0x111217),
        surface: rgb(0x171920),
        surface_hi: rgb(0x20232c),
        line: rgb(0x242731),
        text: rgb(0xecedf1),
        muted: rgb(0x8e93a3),
        faint: rgb(0x555a68),
        accent: rgb(0xf5a65b),
        accent_soft: rgb(0x3a2b1f),
        danger: rgb(0xff7878),
    }),
    ("Ocean", Palette {
        bg: rgb(0x0b0f17),
        sidebar: rgb(0x0e131d),
        surface: rgb(0x151b28),
        surface_hi: rgb(0x1e2636),
        line: rgb(0x232c3e),
        text: rgb(0xe8edf5),
        muted: rgb(0x8c97ad),
        faint: rgb(0x525d74),
        accent: rgb(0x5cb8ff),
        accent_soft: rgb(0x1a2c42),
        danger: rgb(0xff7878),
    }),
    ("Forest", Palette {
        bg: rgb(0x0d1210),
        sidebar: rgb(0x111815),
        surface: rgb(0x18201c),
        surface_hi: rgb(0x212b26),
        line: rgb(0x28332d),
        text: rgb(0xebf0ec),
        muted: rgb(0x8f9e95),
        faint: rgb(0x55645b),
        accent: rgb(0x6fd08c),
        accent_soft: rgb(0x1d3326),
        danger: rgb(0xff8278),
    }),
    ("Rose", Palette {
        bg: rgb(0x130e11),
        sidebar: rgb(0x181115),
        surface: rgb(0x21171c),
        surface_hi: rgb(0x2c2026),
        line: rgb(0x36272f),
        text: rgb(0xf3ecef),
        muted: rgb(0xa38f98),
        faint: rgb(0x6a555f),
        accent: rgb(0xf2709c),
        accent_soft: rgb(0x3d1f2c),
        danger: rgb(0xff8c64),
    }),
    ("Paper", Palette {
        bg: rgb(0xf6f4ef),
        sidebar: rgb(0xedeae3),
        surface: rgb(0xffffff),
        surface_hi: rgb(0xe9e5dc),
        line: rgb(0xd9d4c8),
        text: rgb(0x1d1c1a),
        muted: rgb(0x6b675e),
        faint: rgb(0x9c978b),
        accent: rgb(0xc2410c),
        accent_soft: rgb(0xf9dcc8),
        danger: rgb(0xc62828),
    }),
    ("High contrast", Palette {
        bg: rgb(0x000000),
        sidebar: rgb(0x0a0a0a),
        surface: rgb(0x141414),
        surface_hi: rgb(0x262626),
        line: rgb(0x4d4d4d),
        text: rgb(0xffffff),
        muted: rgb(0xc4c4c4),
        faint: rgb(0x8c8c8c),
        accent: rgb(0xffd23f),
        accent_soft: rgb(0x3d3300),
        danger: rgb(0xff6b6b),
    }),
];

pub const DEFAULT_THEME: &str = PRESETS[0].0;

impl Palette {
    /// Each color with its name in Settings (the order they're saved in).
    pub fn colors_mut(&mut self) -> [(&'static str, &mut Color32); 11] {
        [
            ("Background", &mut self.bg),
            ("Sidebar", &mut self.sidebar),
            ("Panels", &mut self.surface),
            ("Raised panels", &mut self.surface_hi),
            ("Lines", &mut self.line),
            ("Text", &mut self.text),
            ("Secondary text", &mut self.muted),
            ("Faint text", &mut self.faint),
            ("Accent", &mut self.accent),
            ("Accent, soft", &mut self.accent_soft),
            ("Errors", &mut self.danger),
        ]
    }

    /// A light palette gets egui's light widgets as the base.
    fn is_light(&self) -> bool {
        let [r, g, b, _] = self.bg.to_array();
        0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 128.0
    }
}

/// A palette the user made: saved in the settings, colors as `#rrggbb` in the order of
/// `Palette::colors_mut`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomTheme {
    pub name: String,
    pub colors: Vec<String>,
}

impl CustomTheme {
    pub fn new(name: String, mut palette: Palette) -> CustomTheme {
        let colors = palette.colors_mut().iter().map(|(_, c)| format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())).collect();
        CustomTheme { name, colors }
    }

    /// The saved colors; one missing or unreadable keeps the default's.
    pub fn palette(&self) -> Palette {
        let mut palette = PRESETS[0].1;
        for ((_, color), saved) in palette.colors_mut().into_iter().zip(&self.colors) {
            if let Some(hex) = saved.strip_prefix('#').and_then(|h| u32::from_str_radix(h, 16).ok()).filter(|_| saved.len() == 7) {
                *color = rgb(hex);
            }
        }
        palette
    }
}

/// The palette named: one of the user's, else a built-in, else the default.
pub fn find(name: &str, custom: &[CustomTheme]) -> Palette {
    custom
        .iter()
        .find(|t| t.name == name)
        .map(CustomTheme::palette)
        .or_else(|| PRESETS.iter().find(|(n, _)| *n == name).map(|(_, p)| *p))
        .unwrap_or(PRESETS[0].1)
}

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
        if current.is_null() { &PRESETS[0].1 } else { unsafe { &*current } }
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
    let mut visuals = if p.is_light() { Visuals::light() } else { Visuals::dark() };
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

    #[test]
    fn a_custom_theme_keeps_its_colors_and_unreadable_ones_fall_back() {
        let mut palette = PRESETS[1].1;
        palette.accent = rgb(0x123456);
        let theme = CustomTheme::new("Mine".into(), palette);
        assert_eq!(theme.palette(), palette);
        let broken = CustomTheme { name: "Broken".into(), colors: vec!["nonsense".into(), "#000000".into()] };
        let read = broken.palette();
        assert_eq!(read.bg, PRESETS[0].1.bg, "unreadable: the default's");
        assert_eq!(read.sidebar, rgb(0x000000));
        assert_eq!(read.accent, PRESETS[0].1.accent, "missing: the default's");
        assert_eq!(find("gone", &[]), PRESETS[0].1);
    }

    #[test]
    fn only_the_light_palette_is_light() {
        let light: Vec<_> = PRESETS.iter().filter(|(_, p)| p.is_light()).map(|(n, _)| *n).collect();
        assert_eq!(light, ["Paper"]);
    }

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
