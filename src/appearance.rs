//! Detection of the operating system's light/dark appearance. This is used to
//! resolve adaptive themes (see [`crate::theme::ThemeConfig`]).

/// Returns `true` if the system is currently using a dark appearance.
///
/// This uses the `dark-light` crate, which reads the OS appearance setting on
/// macOS, Windows, and Linux (the latter via the XDG desktop portal). If the
/// appearance cannot be determined, this returns `false` so adaptive themes
/// fall back to their light variant.
pub fn is_dark_mode() -> bool {
    matches!(dark_light::detect(), Ok(dark_light::Mode::Dark))
}
