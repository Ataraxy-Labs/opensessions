//! macOS system-appearance helpers.
//!
//! Parity with the TypeScript `system-theme` module: detect the macOS Appearance
//! (Light/Dark), map it to a configured theme name, and (see [`watch_mac_system_appearance`])
//! push changes to the consumer. All functions are total and macOS-gated; on
//! non-macOS platforms appearance is always [`SystemAppearance::Light`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Dark,
    Light,
}

/// Map a detected appearance + configured theme names to the theme to apply.
/// Pure and trivially testable.
pub fn theme_for_system_mode(
    mode: SystemAppearance,
    dark_theme: &str,
    light_theme: &str,
) -> String {
    match mode {
        SystemAppearance::Dark => dark_theme.to_string(),
        SystemAppearance::Light => light_theme.to_string(),
    }
}

/// Read the current macOS Appearance. `defaults read -g AppleInterfaceStyle`
/// prints "Dark" in dark mode and exits non-zero / empty in light mode (the key
/// is absent), so both absent and unreadable map to Light. Never panics.
#[cfg(target_os = "macos")]
pub fn read_mac_system_appearance() -> SystemAppearance {
    use std::process::Command;
    match Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
    {
        Ok(output) => {
            let value = String::from_utf8_lossy(&output.stdout);
            if value.trim() == "Dark" {
                SystemAppearance::Dark
            } else {
                SystemAppearance::Light
            }
        }
        Err(_) => SystemAppearance::Light,
    }
}

/// Non-macOS platforms have no system Appearance; always Light.
#[cfg(not(target_os = "macos"))]
pub fn read_mac_system_appearance() -> SystemAppearance {
    SystemAppearance::Light
}
