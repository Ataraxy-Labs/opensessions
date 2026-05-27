use opensessions_sidebar_core::renderer::{palette_for_theme, Rgb, THEME_NAMES};

#[test]
fn gruvbox_light_theme_is_registered_and_resolves_palette() {
    assert!(THEME_NAMES.contains(&"gruvbox-light"));

    let palette = palette_for_theme(Some("gruvbox-light"));
    assert_eq!(palette.text, Rgb::new(60, 56, 54));
    assert_eq!(palette.sky, Rgb::new(69, 133, 136));
    assert_eq!(palette.surface1, Rgb::new(213, 196, 161));
    assert_ne!(palette, palette_for_theme(Some("gruvbox-dark")));
}
