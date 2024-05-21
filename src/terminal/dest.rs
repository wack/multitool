use console::{colors_enabled, colors_enabled_stderr, Term};
use dialoguer::theme::Theme;

use crate::Flags;

use super::theme::{colorful_theme, SIMPLE_THEME};

/// A TermDestination references an output file, usually stdout.
pub(super) struct TermDestination {
    term: Term,
    allow_color: bool,
    theme: &'static dyn Theme,
}

impl TermDestination {
    /// Getter for the terminal held by this object.
    pub(super) fn term(&self) -> &Term {
        &self.term
    }

    /// Getter for the theme held by this object.
    pub(super) fn theme(&self) -> &'static dyn Theme {
        self.theme
    }

    /// Getter for whether color is allowed.
    pub(super) fn allow_color(&self) -> bool {
        self.allow_color
    }

    pub(super) fn stdout(flags: &Flags) -> Self {
        let term = Term::stdout();
        // Respect the user's preference for color,
        // but fall back to inspecting the terminal for a tty
        // if no preference has been provided.
        let allow_color = flags
            .enable_colors()
            .color_preference()
            .unwrap_or_else(colors_enabled);
        // Pick the theme based on the user's color preference.
        let theme: &'static dyn Theme = if allow_color {
            colorful_theme()
        } else {
            SIMPLE_THEME
        };
        TermDestination {
            term,
            theme,
            allow_color,
        }
    }

    /// This function is very similar to stdout, but using stderr
    /// instead.
    pub(super) fn stderr(flags: &Flags) -> Self {
        let term = Term::stderr();
        // Respect the user's preference for color,
        // but fall back to inspecting the terminal for a tty
        // if no preference has been provided.
        let allow_color = flags
            .enable_colors()
            .color_preference()
            .unwrap_or_else(colors_enabled_stderr);
        // Pick the theme based on the user's color preference.
        let theme: &'static dyn Theme = if allow_color {
            colorful_theme()
        } else {
            SIMPLE_THEME
        };
        TermDestination {
            term,
            theme,
            allow_color,
        }
    }
}
