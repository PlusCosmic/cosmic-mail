//! The active omarchy theme as UI colours.
//!
//! Every colour derives from the omarchy theme (`OmarchyTheme`); nothing is
//! hard-coded beyond the kanagawa fallback. Derived tones reproduce the old
//! stylesheet's `color-mix(in srgb, …)` recipes, so mixing happens on
//! sRGB-encoded channels, not in linear light.

use rune::color::{hex, srgb_decode, srgb_encode};
use rune::Color;

use crate::omarchy::{kanagawa_fallback, OmarchyTheme};

/// Resolved UI colours for one theme.
#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub cursor: Color,
    pub sel_bg: Color,
    pub sel_fg: Color,
    /// color0..color15.
    pub c: [Color; 16],
    pub border: Color,
    pub border_strong: Color,
    pub muted: Color,
    pub faint: Color,
    pub panel: Color,
    pub panel_2: Color,
    pub hover: Color,
    pub accent_dim: Color,
    pub rail: Color,
    pub border_soft: Color,
    pub muted_2: Color,
    pub accent_soft: Color,
    /// Error tone (toasts, failed sync).
    pub error: Color,
    /// Success tone (synced dot).
    pub ok: Color,
    /// Warning tone (unverified discovery).
    pub warn: Color,
}

/// `color-mix(in srgb, a p%, b)`: `p` of `a`, the rest `b`, on sRGB channels.
pub fn mix(a: Color, p: f32, b: Color) -> Color {
    let m = |x: f32, y: f32| srgb_decode(srgb_encode(x) * p + srgb_encode(y) * (1.0 - p));
    Color {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
        a: a.a * p + b.a * (1.0 - p),
    }
}

/// `color-mix(in srgb, a p%, transparent)`.
fn fade(a: Color, p: f32) -> Color {
    a.alpha(a.a * p)
}

fn parse(value: &str, fallback: &str) -> Color {
    hex(value.trim())
        .or_else(|| hex(fallback))
        .unwrap_or(Color::BLACK)
}

impl Palette {
    /// Resolve a theme, falling back to kanagawa for any unparsable colour.
    pub fn from_theme(t: &OmarchyTheme) -> Palette {
        let fb = kanagawa_fallback();
        let bg = parse(&t.background, &fb.background);
        let fg = parse(&t.foreground, &fb.foreground);
        let accent = parse(&t.accent, &fb.accent);
        let c: [Color; 16] = std::array::from_fn(|i| {
            parse(
                t.palette.get(i).map(String::as_str).unwrap_or(""),
                &fb.palette[i],
            )
        });
        Palette {
            bg,
            fg,
            accent,
            cursor: parse(&t.cursor, &fb.cursor),
            sel_bg: parse(&t.selection_background, &fb.selection_background),
            sel_fg: parse(&t.selection_foreground, &fb.selection_foreground),
            c,
            border: mix(fg, 0.16, bg),
            border_strong: mix(fg, 0.30, bg),
            muted: mix(fg, 0.55, bg),
            faint: mix(fg, 0.35, bg),
            panel: mix(fg, 0.04, bg),
            panel_2: mix(fg, 0.08, bg),
            hover: mix(fg, 0.07, bg),
            accent_dim: mix(accent, 0.40, bg),
            rail: mix(bg, 0.82, c[0]),
            border_soft: fade(c[8], 0.28),
            muted_2: mix(fg, 0.36, bg),
            accent_soft: fade(accent, 0.17),
            error: c[1],
            ok: c[2],
            warn: c[3],
        }
    }

    /// The kanagawa fallback palette.
    pub fn fallback() -> Palette {
        Palette::from_theme(&kanagawa_fallback())
    }

    /// An avatar background for a palette slot (see `format::avatar_slot`).
    pub fn slot(&self, i: usize) -> Color {
        self.c[i % 16]
    }

    /// Background for a primary button (`accent` 20% over `bg`).
    pub fn primary(&self) -> Color {
        mix(self.accent, 0.20, self.bg)
    }

    /// Hovered primary button.
    pub fn primary_hover(&self) -> Color {
        mix(self.accent, 0.30, self.bg)
    }

    /// Whether the theme is dark (drives Rune's light/dark hints).
    pub fn is_dark(&self) -> bool {
        self.bg.luminance() < 0.18
    }
}

impl Default for Palette {
    fn default() -> Self {
        Palette::fallback()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_matches_css_srgb_color_mix() {
        let a = hex("#ffffff").unwrap();
        let b = hex("#000000").unwrap();
        // color-mix(in srgb, #fff 50%, #000): 127.5/255 per encoded channel.
        let m = mix(a, 0.5, b);
        assert!((srgb_encode(m.r) * 255.0 - 127.5).abs() < 0.01);
        assert!((srgb_encode(m.g) * 255.0 - 127.5).abs() < 0.01);
        // Not linear-light mixing, which would give ~188/255.
        let m = mix(a, 0.16, b);
        assert!((srgb_encode(m.b) * 255.0 - 40.8).abs() < 0.01);
    }

    #[test]
    fn fallback_parses_every_colour() {
        let p = Palette::fallback();
        assert_eq!(p.bg.hex(), "#1f1f28ff");
        assert_eq!(p.fg.hex(), "#dcd7baff");
        assert_eq!(p.c[1].hex(), "#c34043ff");
        assert!(p.is_dark());
    }

    #[test]
    fn bad_colours_fall_back_to_kanagawa() {
        let mut t = kanagawa_fallback();
        t.background = "not a colour".into();
        t.palette.truncate(3);
        let p = Palette::from_theme(&t);
        assert_eq!(p.bg.hex(), "#1f1f28ff");
        assert_eq!(p.c[15].hex(), "#dcd7baff");
    }
}
