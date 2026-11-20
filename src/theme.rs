//! The palette: a grey ramp and one accent.

use crate::canvas::Color;
use crate::config;

pub struct Theme {
    pub bg: Color,
    pub hover: Color,
    pub border: Color,
    pub fg: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    /// Text on a filled accent.
    pub on_accent: Color,
    pub urgent: Color,
}

impl Theme {
    pub fn new(config: &config::Theme) -> Self {
        let fg = Color::rgb(0xd8d8d4);
        let accent = config.accent.as_deref().and_then(Color::parse).unwrap_or(fg);
        Self {
            bg: Color::rgb(0x111112),
            hover: Color::rgb(0x222224),
            border: Color::rgb(0x3c3c40),
            fg,
            dim: Color::rgb(0x8a8a8e),
            faint: Color::rgb(0x2e2e31),
            accent,
            on_accent: Color::rgb(0x111112),
            urgent: Color::rgb(0xe0607e),
        }
    }
}
