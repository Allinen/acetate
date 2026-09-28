//! The look: colors, fonts, spacing. One place, so a theme can later come from a file
//! and be switched while the app runs (egui restyles the next frame).

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals,
};

/// The palette. Warm amber on near-black: acetate discs are lacquer on black, and the
/// label is where the color is.
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

pub const PALETTE: Palette = Palette {
    bg: Color32::from_rgb(0x0d, 0x0e, 0x12),
    sidebar: Color32::from_rgb(0x11, 0x12, 0x17),
    surface: Color32::from_rgb(0x17, 0x19, 0x20),
    surface_hi: Color32::from_rgb(0x20, 0x23, 0x2c),
    line: Color32::from_rgb(0x24, 0x27, 0x31),
    text: Color32::from_rgb(0xec, 0xed, 0xf1),
    muted: Color32::from_rgb(0x8e, 0x93, 0xa3),
    faint: Color32::from_rgb(0x55, 0x5a, 0x68),
    accent: Color32::from_rgb(0xf5, 0xa6, 0x5b),
    accent_soft: Color32::from_rgb(0x3a, 0x2b, 0x1f),
    danger: Color32::from_rgb(255, 120, 120),
};

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

/// The bundled Inter (all platforms look the same), with a system fallback for Chinese
/// and Japanese if there is one.
fn fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let inter = [
        ("inter", &include_bytes!("../../assets/fonts/Inter-Regular.ttf")[..]),
        ("inter-medium", &include_bytes!("../../assets/fonts/Inter-Medium.ttf")[..]),
        ("inter-semibold", &include_bytes!("../../assets/fonts/Inter-SemiBold.ttf")[..]),
        ("inter-bold", &include_bytes!("../../assets/fonts/Inter-Bold.ttf")[..]),
    ];
    for (name, bytes) in inter {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let cjk = ["/usr/share/fonts/google-droid-sans-fonts/DroidSansFallbackFull.ttf"]
        .into_iter()
        .find_map(|path| std::fs::read(path).ok());
    let has_cjk = cjk.is_some();
    if let Some(bytes) = cjk {
        fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
    }
    // egui's own fonts stay as fallbacks for symbols (▶, ⏸...).
    let fallbacks: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    let family = |first: &str| {
        let mut list = vec![first.to_string()];
        if has_cjk {
            list.push("cjk".into());
        }
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
    let p = &PALETTE;
    let mut visuals = Visuals::dark();
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
