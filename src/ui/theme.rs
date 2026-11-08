//! The UI colours, from the UI foundation's tokens (`rune-theme-foundation`).
//!
//! The foundation owns every colour: five schemes, each light and dark, generated from
//! `@pluscosmic/ui`'s `tokens.css`. [`Palette`] names those tokens by the roles the views
//! use, so a view asks for `p.muted` and gets the scheme's `fg-soft`.

use rune::Color;
pub use rune_theme_foundation::mix;
use rune_theme_foundation::{look, ColorScheme, Look};

/// Whether the UI is light or dark: the desktop's choice, or one the user fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    /// Dark or not, given whether the desktop is dark.
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Appearance::System => system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }

    /// The name stored in the UI preferences.
    pub fn name(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    /// A stored name; anything unknown follows the system.
    pub fn from_name(name: &str) -> Appearance {
        Appearance::ALL
            .into_iter()
            .find(|a| a.name() == name)
            .unwrap_or_default()
    }
}

/// A scheme's name and one line about it, for the settings dialog.
pub fn scheme_words(scheme: ColorScheme) -> (String, String) {
    let (name, about) = match scheme {
        ColorScheme::Stacks => ("Stacks", "Green, on green-tinted neutrals"),
        ColorScheme::Oxblood => ("Oxblood", "Deep red, on warm neutrals"),
        ColorScheme::Ink => ("Ink", "Blue-black ink and cool slate"),
        ColorScheme::Walnut => ("Walnut", "Dark wood and brass"),
        ColorScheme::Graphite => ("Graphite", "No tint; the mail carries the colour"),
    };
    (name.into(), about.into())
}

/// Resolved UI colours for one scheme and appearance.
#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub struct Palette {
    /// The page (`--bg`).
    pub bg: Color,
    /// Text (`--fg`).
    pub fg: Color,
    /// The accent as text and thin marks (`--accent`).
    pub accent: Color,
    /// Behind selected text.
    pub sel_bg: Color,
    /// Borders (`--line`).
    pub border: Color,
    pub border_strong: Color,
    pub border_soft: Color,
    /// Secondary text (`--fg-soft`).
    pub muted: Color,
    /// Placeholders.
    pub muted_2: Color,
    /// The faintest marks.
    pub faint: Color,
    /// Raised cards and the selected row (`--surface`).
    pub panel: Color,
    /// Recessed fields, tracks and quiet buttons (`--bg-soft`).
    pub panel_2: Color,
    /// The account rail.
    pub rail: Color,
    /// The accent over the line, for the edges of accented controls.
    pub accent_dim: Color,
    /// A light tint of the accent, for highlighted rows and chips.
    pub accent_soft: Color,
    /// The primary button (`--fill`) and the text on it (`--fill-ink`).
    pub fill: Color,
    pub fill_ink: Color,
    /// Tones: errors (`--danger`), success (the accent), needs attention soon
    /// (`--warning`) and neutral information (`--info`).
    pub error: Color,
    pub ok: Color,
    pub warn: Color,
    pub info: Color,
    /// Toasts (`--tip-bg`, `--tip-ink`).
    pub tip_bg: Color,
    pub tip_ink: Color,
    /// The base of shadows and modal scrims.
    pub shadow: Color,
    /// Colours that tell accounts apart, each readable as text on `bg`.
    slots: [Color; 4],
    pub dark: bool,
}

impl Palette {
    /// The palette for one look.
    pub fn from_look(l: &Look) -> Palette {
        let t = &l.t;
        Palette {
            bg: t.bg,
            fg: t.fg,
            accent: t.accent,
            sel_bg: mix(t.accent, 0.24, t.bg),
            border: t.line,
            border_strong: mix(t.fg_soft, 0.35, t.line),
            border_soft: mix(t.line, 0.6, t.bg),
            muted: t.fg_soft,
            muted_2: t.fg_soft,
            faint: mix(t.fg_soft, 0.7, t.bg),
            panel: t.surface,
            panel_2: t.bg_soft,
            rail: t.bg_soft,
            accent_dim: mix(t.accent, 0.45, t.line),
            accent_soft: mix(t.accent, 0.12, t.bg),
            fill: t.fill,
            fill_ink: t.fill_ink,
            error: t.danger,
            ok: t.accent,
            warn: t.warning,
            info: t.info,
            tip_bg: t.tip_bg,
            tip_ink: t.tip_ink,
            shadow: t.shadow,
            slots: [t.accent, t.info, t.fg_soft, mix(t.accent, 0.5, t.info)],
            dark: l.dark,
        }
    }

    /// The palette for a scheme in light or dark.
    pub fn new(scheme: ColorScheme, dark: bool) -> Palette {
        Palette::from_look(look(scheme, dark))
    }

    /// An account's colour (see `format::avatar_slot`).
    pub fn slot(&self, i: usize) -> Color {
        self.slots[i % self.slots.len()]
    }

    /// Background for a primary button.
    pub fn primary(&self) -> Color {
        self.fill
    }
}

impl Default for Palette {
    fn default() -> Self {
        Palette::new(ColorScheme::default(), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color, b: Color) -> f32 {
        let (a, b) = (a.luminance(), b.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn every_scheme_reads_in_both_appearances() {
        for scheme in ColorScheme::ALL {
            for dark in [false, true] {
                let p = Palette::new(scheme, dark);
                assert_eq!(p.dark, dark);
                assert!(contrast(p.fg, p.bg) >= 4.5, "{scheme:?} {dark}: text");
                assert!(contrast(p.fg, p.panel) >= 4.5, "{scheme:?} {dark}: card");
                assert!(
                    contrast(p.fill_ink, p.fill) >= 4.5,
                    "{scheme:?} {dark}: button"
                );
                for i in 0..4 {
                    assert!(
                        contrast(p.slot(i), p.bg) >= 3.0,
                        "{scheme:?} {dark}: slot {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_palette_is_the_foundations_tokens() {
        let l = look(ColorScheme::Ink, true);
        let p = Palette::from_look(l);
        assert_eq!(p.bg, l.t.bg);
        assert_eq!(p.muted, l.t.fg_soft);
        assert_eq!(p.fill, l.t.fill);
        assert_eq!(p.error, l.t.danger);
    }

    #[test]
    fn appearance_follows_the_system_unless_fixed() {
        assert!(Appearance::System.is_dark(true));
        assert!(!Appearance::System.is_dark(false));
        assert!(!Appearance::Light.is_dark(true));
        assert!(Appearance::Dark.is_dark(false));
        assert_eq!(Appearance::from_name("dark"), Appearance::Dark);
        assert_eq!(Appearance::from_name("sepia"), Appearance::System);
    }
}
