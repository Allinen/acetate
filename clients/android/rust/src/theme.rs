//! The color themes (`acetate_core::theme`): the palettes, and the ones the user made.

use acetate_core::theme as shared;

/// The colors of the whole app, each as `0xRRGGBB`.
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct Palette {
    pub bg: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub surface_hi: u32,
    pub line: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub accent_soft: u32,
    pub danger: u32,
    /// The background is light.
    pub light: bool,
}

impl From<shared::Palette> for Palette {
    fn from(p: shared::Palette) -> Palette {
        Palette {
            bg: p.bg,
            sidebar: p.sidebar,
            surface: p.surface,
            surface_hi: p.surface_hi,
            line: p.line,
            text: p.text,
            muted: p.muted,
            faint: p.faint,
            accent: p.accent,
            accent_soft: p.accent_soft,
            danger: p.danger,
            light: p.is_light(),
        }
    }
}

impl From<Palette> for shared::Palette {
    fn from(p: Palette) -> shared::Palette {
        shared::Palette {
            bg: p.bg,
            sidebar: p.sidebar,
            surface: p.surface,
            surface_hi: p.surface_hi,
            line: p.line,
            text: p.text,
            muted: p.muted,
            faint: p.faint,
            accent: p.accent,
            accent_soft: p.accent_soft,
            danger: p.danger,
        }
    }
}

/// A palette the user made, as it's saved.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CustomTheme {
    pub name: String,
    pub colors: Vec<String>,
}

impl From<CustomTheme> for shared::CustomTheme {
    fn from(t: CustomTheme) -> shared::CustomTheme {
        shared::CustomTheme { name: t.name, colors: t.colors }
    }
}

impl From<shared::CustomTheme> for CustomTheme {
    fn from(t: shared::CustomTheme) -> CustomTheme {
        CustomTheme { name: t.name, colors: t.colors }
    }
}

/// One of a palette's colors, named as Settings shows it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NamedColor {
    pub label: String,
    pub rgb: u32,
}

/// The built-in themes' names, the default first.
#[uniffi::export]
pub fn theme_presets() -> Vec<String> {
    shared::PRESETS.iter().map(|(n, _)| n.to_string()).collect()
}

/// Whether `name` is a built-in theme's.
#[uniffi::export]
pub fn theme_is_preset(name: String) -> bool {
    shared::is_preset(&name)
}

/// The palette named: one of `custom`, else a built-in, else the default.
#[uniffi::export]
pub fn theme_find(name: String, custom: Vec<CustomTheme>) -> Palette {
    let custom: Vec<shared::CustomTheme> = custom.into_iter().map(Into::into).collect();
    shared::find(&name, &custom).into()
}

/// `palette` as a theme to save under `name`.
#[uniffi::export]
pub fn theme_new(name: String, palette: Palette) -> CustomTheme {
    shared::CustomTheme::new(name, palette.into()).into()
}

/// Each of `palette`'s colors, in the order Settings lists them.
#[uniffi::export]
pub fn theme_colors(palette: Palette) -> Vec<NamedColor> {
    let mut palette: shared::Palette = palette.into();
    palette.colors_mut().into_iter().map(|(label, rgb)| NamedColor { label: label.to_string(), rgb: *rgb }).collect()
}

/// `palette` with its `index`th color (as `theme_colors` lists them) set to `rgb`.
#[uniffi::export]
pub fn theme_with_color(palette: Palette, index: u32, rgb: u32) -> Palette {
    let mut palette: shared::Palette = palette.into();
    if let Some((_, color)) = palette.colors_mut().into_iter().nth(index as usize) {
        *color = rgb & 0xff_ffff;
    }
    palette.into()
}

/// The user's themes, to keep on the phone.
#[uniffi::export]
pub fn themes_to_json(themes: Vec<CustomTheme>) -> String {
    let themes: Vec<shared::CustomTheme> = themes.into_iter().map(Into::into).collect();
    shared::to_json(&themes)
}

/// What `themes_to_json` made; unreadable: none.
#[uniffi::export]
pub fn themes_from_json(json: String) -> Vec<CustomTheme> {
    shared::from_json(&json).into_iter().map(Into::into).collect()
}
