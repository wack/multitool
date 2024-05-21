use std::sync::OnceLock;

use dialoguer::theme::{ColorfulTheme, SimpleTheme};

/// The globally available colorful theme, initiaulzied with the default
/// values. Use this function to create a ColorfulTheme; don't go through
/// the library to create a new instance.
pub(super) fn colorful_theme() -> &'static ColorfulTheme {
    // A lock ensuring the color theme is initialized at most once,
    // letting us reuse the value.
    static COLORFUL_THEME: OnceLock<ColorfulTheme> = OnceLock::new();
    // Initialize the theme using our brand colors.
    COLORFUL_THEME.get_or_init(|| {
        ColorfulTheme {
            // TODO:
            // Customizations and brand colors go here.
            ..ColorfulTheme::default()
        }
    })
}

pub(super) const SIMPLE_THEME: &SimpleTheme = &SimpleTheme;
