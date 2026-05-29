use opensessions_runtime::system_theme::{
    read_mac_system_appearance, theme_for_system_mode, SystemAppearance,
};

#[test]
fn theme_for_system_mode_maps_dark_and_light() {
    assert_eq!(
        theme_for_system_mode(SystemAppearance::Dark, "catppuccin-mocha", "catppuccin-latte"),
        "catppuccin-mocha"
    );
    assert_eq!(
        theme_for_system_mode(SystemAppearance::Light, "catppuccin-mocha", "catppuccin-latte"),
        "catppuccin-latte"
    );
}

#[test]
fn theme_for_system_mode_respects_custom_names() {
    assert_eq!(
        theme_for_system_mode(SystemAppearance::Dark, "tokyo-night-storm", "tango-adapted"),
        "tokyo-night-storm"
    );
    assert_eq!(
        theme_for_system_mode(SystemAppearance::Light, "tokyo-night-storm", "tango-adapted"),
        "tango-adapted"
    );
}

#[test]
fn read_appearance_is_total_and_returns_a_variant() {
    // Non-macOS: always Light. macOS: reads the real setting. Never panics.
    let mode = read_mac_system_appearance();
    assert!(matches!(mode, SystemAppearance::Dark | SystemAppearance::Light));
}
