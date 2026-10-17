//! Color themes, the same on every client: the built-in palettes and the ones a user made.
//! A client draws with a [`Palette`]; which one it uses (by name) is its own setting.

use serde::{Deserialize, Serialize};

/// The colors of the whole app, each as `0xRRGGBB`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    /// Errors.
    pub danger: u32,
}

/// The built-in palettes, the first the default.
pub const PRESETS: [(&str, Palette); 6] = [
    // Warm amber on near-black: acetate discs are lacquer on black, and the label is where
    // the color is.
    ("Amber", Palette {
        bg: 0x0d0e12,
        sidebar: 0x111217,
        surface: 0x171920,
        surface_hi: 0x20232c,
        line: 0x242731,
        text: 0xecedf1,
        muted: 0x8e93a3,
        faint: 0x555a68,
        accent: 0xf5a65b,
        accent_soft: 0x3a2b1f,
        danger: 0xff7878,
    }),
    ("Ocean", Palette {
        bg: 0x0b0f17,
        sidebar: 0x0e131d,
        surface: 0x151b28,
        surface_hi: 0x1e2636,
        line: 0x232c3e,
        text: 0xe8edf5,
        muted: 0x8c97ad,
        faint: 0x525d74,
        accent: 0x5cb8ff,
        accent_soft: 0x1a2c42,
        danger: 0xff7878,
    }),
    ("Forest", Palette {
        bg: 0x0d1210,
        sidebar: 0x111815,
        surface: 0x18201c,
        surface_hi: 0x212b26,
        line: 0x28332d,
        text: 0xebf0ec,
        muted: 0x8f9e95,
        faint: 0x55645b,
        accent: 0x6fd08c,
        accent_soft: 0x1d3326,
        danger: 0xff8278,
    }),
    ("Rose", Palette {
        bg: 0x130e11,
        sidebar: 0x181115,
        surface: 0x21171c,
        surface_hi: 0x2c2026,
        line: 0x36272f,
        text: 0xf3ecef,
        muted: 0xa38f98,
        faint: 0x6a555f,
        accent: 0xf2709c,
        accent_soft: 0x3d1f2c,
        danger: 0xff8c64,
    }),
    ("Paper", Palette {
        bg: 0xf6f4ef,
        sidebar: 0xedeae3,
        surface: 0xffffff,
        surface_hi: 0xe9e5dc,
        line: 0xd9d4c8,
        text: 0x1d1c1a,
        muted: 0x6b675e,
        faint: 0x9c978b,
        accent: 0xc2410c,
        accent_soft: 0xf9dcc8,
        danger: 0xc62828,
    }),
    ("High contrast", Palette {
        bg: 0x000000,
        sidebar: 0x0a0a0a,
        surface: 0x141414,
        surface_hi: 0x262626,
        line: 0x4d4d4d,
        text: 0xffffff,
        muted: 0xc4c4c4,
        faint: 0x8c8c8c,
        accent: 0xffd23f,
        accent_soft: 0x3d3300,
        danger: 0xff6b6b,
    }),
];

pub const DEFAULT_THEME: &str = PRESETS[0].0;

impl Palette {
    /// Each color with its name in Settings (the order they're saved in).
    pub fn colors_mut(&mut self) -> [(&'static str, &mut u32); 11] {
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

    /// Whether the background is light (a client then starts from its light widgets).
    pub const fn is_light(&self) -> bool {
        let (r, g, b) = ((self.bg >> 16) & 0xff, (self.bg >> 8) & 0xff, self.bg & 0xff);
        0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 128.0
    }
}

/// A palette the user made, as it's saved: colors as `#rrggbb` in the order of
/// [`Palette::colors_mut`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomTheme {
    pub name: String,
    pub colors: Vec<String>,
}

impl CustomTheme {
    pub fn new(name: String, mut palette: Palette) -> CustomTheme {
        let colors = palette.colors_mut().iter().map(|(_, c)| format!("#{:06x}", **c)).collect();
        CustomTheme { name, colors }
    }

    /// The saved colors; one missing or unreadable keeps the default's.
    pub fn palette(&self) -> Palette {
        let mut palette = PRESETS[0].1;
        for ((_, color), saved) in palette.colors_mut().into_iter().zip(&self.colors) {
            if let Some(hex) = saved.strip_prefix('#').filter(|h| h.len() == 6).and_then(|h| u32::from_str_radix(h, 16).ok()) {
                *color = hex;
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

/// Whether `name` is a built-in's.
pub fn is_preset(name: &str) -> bool {
    PRESETS.iter().any(|(n, _)| *n == name)
}

/// The user's themes as saved by a client that has no settings file of its own.
pub fn to_json(themes: &[CustomTheme]) -> String {
    serde_json::to_string(themes).unwrap_or_else(|_| "[]".into())
}

/// Unreadable: none.
pub fn from_json(json: &str) -> Vec<CustomTheme> {
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_theme_keeps_its_colors_and_unreadable_ones_fall_back() {
        let mut palette = PRESETS[1].1;
        palette.accent = 0x123456;
        let theme = CustomTheme::new("Mine".into(), palette);
        assert_eq!(theme.palette(), palette);
        let broken = CustomTheme { name: "Broken".into(), colors: vec!["nonsense".into(), "#000000".into()] };
        let read = broken.palette();
        assert_eq!(read.bg, PRESETS[0].1.bg, "unreadable: the default's");
        assert_eq!(read.sidebar, 0);
        assert_eq!(read.accent, PRESETS[0].1.accent, "missing: the default's");
        assert_eq!(find("gone", &[]), PRESETS[0].1);
    }

    #[test]
    fn saved_themes_come_back() {
        let themes = vec![CustomTheme::new("Mine".into(), PRESETS[2].1)];
        assert_eq!(from_json(&to_json(&themes)), themes);
        assert!(from_json("not json").is_empty());
    }

    #[test]
    fn only_the_light_palette_is_light() {
        let light: Vec<_> = PRESETS.iter().filter(|(_, p)| p.is_light()).map(|(n, _)| *n).collect();
        assert_eq!(light, ["Paper"]);
    }
}
