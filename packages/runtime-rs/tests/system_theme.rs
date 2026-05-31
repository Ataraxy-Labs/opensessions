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

#[test]
fn watcher_handle_stop_is_idempotent() {
    use opensessions_runtime::system_theme::watch_mac_system_appearance;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let watcher = watch_mac_system_appearance(
        move |_mode| {
            counter.fetch_add(1, Ordering::SeqCst);
        },
        Some(60_000),
    );
    watcher.stop();
    watcher.stop(); // must not panic

    // On macOS the initial synchronous check fires once; on non-macOS, never.
    assert!(calls.load(Ordering::SeqCst) <= 1);
}

#[test]
fn resolve_auto_theme_uses_config_with_defaults() {
    use opensessions_runtime::config::OpensessionsConfig;
    use opensessions_runtime::system_theme::resolve_auto_theme;

    let mut config = OpensessionsConfig::default();
    assert_eq!(
        resolve_auto_theme(SystemAppearance::Dark, &config),
        "catppuccin-mocha"
    );
    assert_eq!(
        resolve_auto_theme(SystemAppearance::Light, &config),
        "catppuccin-latte"
    );

    config.dark_theme = Some("tokyo-night-storm".into());
    config.light_theme = Some("tango-adapted".into());
    assert_eq!(
        resolve_auto_theme(SystemAppearance::Dark, &config),
        "tokyo-night-storm"
    );
    assert_eq!(
        resolve_auto_theme(SystemAppearance::Light, &config),
        "tango-adapted"
    );
}

#[test]
fn manual_persist_slot_picks_per_appearance_when_following() {
    use opensessions_runtime::system_theme::{manual_persist_slot, ThemePersistSlot};

    // Not following → always the plain `theme` slot.
    assert_eq!(
        manual_persist_slot(false, Some(SystemAppearance::Dark)),
        ThemePersistSlot::Theme
    );
    assert_eq!(manual_persist_slot(false, None), ThemePersistSlot::Theme);

    // Following → per-appearance slot, so dark/light remember independently.
    assert_eq!(
        manual_persist_slot(true, Some(SystemAppearance::Dark)),
        ThemePersistSlot::DarkTheme
    );
    assert_eq!(
        manual_persist_slot(true, Some(SystemAppearance::Light)),
        ThemePersistSlot::LightTheme
    );
    // Following but appearance not observed yet → fall back to `theme`.
    assert_eq!(manual_persist_slot(true, None), ThemePersistSlot::Theme);
}
