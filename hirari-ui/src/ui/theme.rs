//! Display theme: OS dark/light detection and the dark/light palettes.
//!
//! The UI reads every color through the Slint `Style` global, so switching
//! the theme is pushing 33 colors once at startup. The dark palette is
//! byte-identical to the `Style` defaults (a test parses `style.slint` and
//! compares), which means a dark-OS machine renders exactly what it always
//! did; only a light OS (or an explicit override) changes anything.
//!
//! Resolution mirrors the language logic: explicit Settings override
//! (`"dark"`/`"light"`), otherwise the OS (`"system"`, the default),
//! otherwise dark. Only the two shipped palettes exist.

use crate::slint_ui::Style;

/// Theme override values stored in settings.
pub(crate) const THEME_SYSTEM: &str = "system";
pub(crate) const THEME_DARK: &str = "dark";
pub(crate) const THEME_LIGHT: &str = "light";

/// The settings default: follow the OS appearance.
pub(crate) fn default_theme() -> String {
    THEME_SYSTEM.to_owned()
}

/// Returns the OS appearance (`"dark"`/`"light"`). `HIRARI_THEME` overrides
/// the OS for tests and troubleshooting. macOS reads the global
/// `AppleInterfaceStyle` default (`Dark` present means dark; the key is
/// absent on a light machine). Other platforms keep the historical dark
/// look unless the override is set.
pub(crate) fn system_theme() -> String {
    if let Ok(forced) = std::env::var("HIRARI_THEME") {
        match forced.trim().to_lowercase().as_str() {
            THEME_LIGHT => return THEME_LIGHT.to_owned(),
            THEME_DARK => return THEME_DARK.to_owned(),
            _ => {}
        }
    }
    #[cfg(target_os = "macos")]
    {
        let dark = std::process::Command::new("defaults")
            .arg("read")
            .arg("-g")
            .arg("AppleInterfaceStyle")
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .eq_ignore_ascii_case("dark")
            });
        if dark {
            THEME_DARK.to_owned()
        } else {
            THEME_LIGHT.to_owned()
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        THEME_DARK.to_owned()
    }
}

/// Resolves the display theme: the stored override wins, `"system"` (or
/// anything unrecognized) follows the OS.
pub(crate) fn resolve_theme(stored: &str) -> String {
    match stored.trim().to_lowercase().as_str() {
        THEME_DARK => THEME_DARK.to_owned(),
        THEME_LIGHT => THEME_LIGHT.to_owned(),
        _ => system_theme(),
    }
}

type Rgba = (u8, u8, u8, u8);

fn rgba(pixel: Rgba) -> slint::Color {
    slint::Color::from_argb_u8(pixel.0, pixel.1, pixel.2, pixel.3)
}

/// Every `in-out property <color>` in `ui/style.slint`, in one list. Each
/// row is the Style property name, the dark value, the light value, and the
/// generated setter. One list drives both the file-parity test (via
/// `THEME_COLORS_META`) and the application below, so a color cannot be
/// added to one and forgotten in the other. The dark column repeats the
/// Slint defaults exactly (proven by test); the light column is a
/// conservative light-editor ramp that keeps the brand and status hues
/// recognizable in both modes.
macro_rules! theme_colors {
    ($( $key:expr, $dark:expr, $light:expr, $setter:ident );* $(;)?) => {
        pub(crate) const THEME_COLORS_META: &[(&str, Rgba, Rgba)] =
            &[ $( ($key, $dark, $light) ),* ];
        /// Pushes a resolved theme into the Slint `Style` global. Called
        /// once at startup, before the UI is shown; Slint bindings update
        /// from the values.
        pub(crate) fn apply_to_ui(ui: &crate::slint_ui::AppWindow, theme: &str) {
            use slint::ComponentHandle;
            let style = ui.global::<crate::slint_ui::Style>();
            let light = theme == THEME_LIGHT;
            $( style.$setter(rgba(if light { $light } else { $dark })); )*
        }
    };
}

theme_colors! {
    "base", (255, 15, 20, 27), (255, 235, 239, 244), set_base;
    "panel", (255, 26, 34, 44), (255, 255, 255, 255), set_panel;
    "panel-h", (255, 36, 49, 64), (255, 224, 231, 239), set_panel_h;
    "surface", (255, 20, 27, 36), (255, 244, 247, 250), set_surface;
    "surface-0", (255, 9, 13, 18), (255, 216, 222, 230), set_surface_0;
    "surface-1", (255, 16, 23, 31), (255, 228, 233, 239), set_surface_1;
    "surface-2", (255, 24, 33, 43), (255, 238, 242, 247), set_surface_2;
    "surface-3", (255, 34, 46, 58), (255, 224, 231, 239), set_surface_3;
    "border", (255, 42, 55, 68), (255, 201, 210, 220), set_border;
    "line-subtle", (255, 41, 45, 51), (255, 213, 220, 228), set_line_subtle;
    "line-strong", (255, 58, 65, 74), (255, 179, 189, 201), set_line_strong;
    "bezel-light", (255, 56, 56, 60), (255, 192, 200, 210), set_bezel_light;
    "shadow-bottom", (107, 0, 0, 0), (107, 0, 0, 0), set_shadow_bottom;
    "highlight-top", (20, 255, 255, 255), (20, 255, 255, 255), set_highlight_top;
    "brand", (255, 139, 124, 255), (255, 139, 124, 255), set_brand;
    "brand-glow", (41, 139, 124, 255), (41, 139, 124, 255), set_brand_glow;
    "selection", (77, 139, 124, 255), (77, 139, 124, 255), set_selection;
    "ok", (255, 53, 211, 154), (255, 31, 157, 102), set_ok;
    "solo", (255, 211, 154, 58), (255, 185, 122, 31), set_solo;
    "alert", (255, 217, 74, 74), (255, 201, 58, 58), set_alert;
    "record", (255, 217, 74, 74), (255, 201, 58, 58), set_record;
    "lcd-bg", (255, 10, 10, 12), (255, 10, 10, 12), set_lcd_bg;
    "lcd-text", (255, 99, 199, 197), (255, 99, 199, 197), set_lcd_text;
    "cycle", (255, 162, 224, 72), (255, 95, 168, 50), set_cycle;
    "grid-bar", (255, 58, 58, 60), (255, 212, 218, 225), set_grid_bar;
    "grid-beat", (255, 36, 36, 38), (255, 230, 234, 239), set_grid_beat;
    "grid-subbeat", (255, 31, 31, 35), (255, 239, 242, 246), set_grid_subbeat;
    "playhead", (255, 48, 209, 88), (255, 31, 163, 74), set_playhead;
    "region-border", (51, 255, 255, 255), (64, 26, 37, 48), set_region_border;
    "text", (255, 244, 247, 251), (255, 20, 26, 33), set_text;
    "text-dim", (255, 154, 168, 184), (255, 74, 85, 96), set_text_dim;
    "text-muted", (255, 102, 117, 136), (255, 138, 150, 163), set_text_muted;
    "text-primary", (255, 237, 242, 248), (255, 16, 21, 27), set_text_primary;
    "text-secondary", (255, 170, 183, 198), (255, 61, 72, 84), set_text_secondary;
    "text-tertiary", (255, 113, 128, 147), (255, 104, 117, 135), set_text_tertiary;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_override_wins_over_the_os_theme() {
        assert_eq!(resolve_theme("dark"), "dark");
        assert_eq!(resolve_theme("light"), "light");
        assert_eq!(resolve_theme("DARK"), "dark");
        assert_eq!(resolve_theme("  light  "), "light");
    }

    #[test]
    fn unknown_overrides_follow_the_os_without_crashing() {
        for stored in ["system", "SYSTEM", "xx", ""] {
            let resolved = resolve_theme(stored);
            assert!(
                resolved == "dark" || resolved == "light",
                "{stored} resolved to {resolved}"
            );
        }
    }

    #[test]
    fn system_theme_is_always_a_supported_value() {
        let system = system_theme();
        assert!(system == "dark" || system == "light", "got {system}");
    }

    /// Parses `in-out property <color> name: default;` defaults from
    /// `ui/style.slint` and asserts the dark palette repeats them exactly.
    /// A dark-OS machine must render pixel-identically with and without the
    /// theme switch, so any drift here is a visible regression on the
    /// shipping appearance.
    #[test]
    fn dark_palette_matches_the_slint_defaults() {
        fn parse_hex(token: &str) -> Option<Rgba> {
            let token = token.trim().trim_start_matches('#');
            if token.len() == 6 {
                Some((
                    255,
                    u8::from_str_radix(&token[0..2], 16).ok()?,
                    u8::from_str_radix(&token[2..4], 16).ok()?,
                    u8::from_str_radix(&token[4..6], 16).ok()?,
                ))
            } else {
                None
            }
        }
        let source =
            std::fs::read_to_string("ui/style.slint").expect("style source must be readable");
        let mut defaults = std::collections::HashMap::new();
        for line in source.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("in-out property <color> ") else {
                continue;
            };
            let (name, value) = rest
                .split_once(':')
                .expect("color property must have a default");
            let value = value.trim().trim_end_matches(';').trim();
            let rgba = if let Some((hex, alpha)) = value.split_once(".with-alpha(") {
                let mut pixel = parse_hex(hex)
                    .unwrap_or_else(|| panic!("unparseable color default for {name}: {value}"));
                let amount: f32 = alpha
                    .trim_end_matches(')')
                    .parse()
                    .unwrap_or_else(|_| panic!("unparseable alpha for {name}: {value}"));
                pixel.0 = (amount.clamp(0.0, 1.0) * 255.0).round() as u8;
                pixel
            } else {
                parse_hex(value)
                    .unwrap_or_else(|| panic!("unparseable color default for {name}: {value}"))
            };
            defaults.insert(name.trim().to_owned(), rgba);
        }
        assert!(!defaults.is_empty());
        for (key, dark, _light) in THEME_COLORS_META {
            assert_eq!(
                Some(dark),
                defaults.get(*key),
                "dark palette drifted from the Slint default for {key}"
            );
        }
        assert_eq!(
            defaults.len(),
            THEME_COLORS_META.len(),
            "every Style color must be in the theme table"
        );
    }

    #[test]
    fn palettes_are_well_formed_and_distinct_where_it_matters() {
        let mut keys = std::collections::HashSet::new();
        for (key, dark, light) in THEME_COLORS_META {
            assert!(keys.insert(*key), "duplicate theme key {key}");
            for pixel in [*dark, *light] {
                assert!(pixel.0 > 0, "{key} must not be fully transparent");
            }
        }
        // Surfaces and body text must actually invert between modes;
        // accents intentionally stay recognizable in both.
        for key in ["base", "surface", "panel", "text", "text-primary"] {
            let (_, dark, light) = THEME_COLORS_META
                .iter()
                .find(|entry| entry.0 == key)
                .expect("key exists");
            assert_ne!(dark, light, "{key} must differ between themes");
        }
    }
}
