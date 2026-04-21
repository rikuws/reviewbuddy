use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{px, Pixels, Rgba, WindowAppearance};
use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;

const THEME_SETTINGS_CACHE_KEY: &str = "theme-settings-v1";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn label(&self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn all() -> &'static [ThemePreference] {
        &[
            ThemePreference::System,
            ThemePreference::Light,
            ThemePreference::Dark,
        ]
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActiveTheme {
    Light = 0,
    #[default]
    Dark = 1,
}

impl ActiveTheme {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSettings {
    #[serde(default)]
    pub preference: ThemePreference,
}

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(ActiveTheme::Dark as u8);

fn color(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

pub fn transparent() -> Rgba {
    color(0.0, 0.0, 0.0, 0.0)
}

fn hex(hex: u32) -> Rgba {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: 1.0 }
}

fn hex_alpha(value: u32, a: f32) -> Rgba {
    let mut rgba = hex(value);
    rgba.a = a;
    rgba
}

fn theme_hex(light: u32, dark: u32) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => hex(light),
        ActiveTheme::Dark => hex(dark),
    }
}

fn theme_hex_alpha(light: (u32, f32), dark: (u32, f32)) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => hex_alpha(light.0, light.1),
        ActiveTheme::Dark => hex_alpha(dark.0, dark.1),
    }
}

fn theme_rgba(light: (f32, f32, f32, f32), dark: (f32, f32, f32, f32)) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => color(light.0, light.1, light.2, light.3),
        ActiveTheme::Dark => color(dark.0, dark.1, dark.2, dark.3),
    }
}

pub fn load_theme_settings(cache: &CacheStore) -> Result<ThemeSettings, String> {
    Ok(cache
        .get::<ThemeSettings>(THEME_SETTINGS_CACHE_KEY)?
        .map(|document| document.value)
        .unwrap_or_default())
}

pub fn save_theme_settings(cache: &CacheStore, settings: &ThemeSettings) -> Result<(), String> {
    cache.put(THEME_SETTINGS_CACHE_KEY, settings, now_ms())
}

pub fn resolve_theme(preference: ThemePreference, appearance: WindowAppearance) -> ActiveTheme {
    match preference {
        ThemePreference::System => {
            if is_light_appearance(appearance) {
                ActiveTheme::Light
            } else {
                ActiveTheme::Dark
            }
        }
        ThemePreference::Light => ActiveTheme::Light,
        ThemePreference::Dark => ActiveTheme::Dark,
    }
}

pub fn set_active_theme(theme: ActiveTheme) {
    ACTIVE_THEME.store(theme as u8, Ordering::Relaxed);
}

pub fn active_theme() -> ActiveTheme {
    match ACTIVE_THEME.load(Ordering::Relaxed) {
        value if value == ActiveTheme::Light as u8 => ActiveTheme::Light,
        _ => ActiveTheme::Dark,
    }
}

pub fn appearance_label(appearance: WindowAppearance) -> &'static str {
    if is_light_appearance(appearance) {
        "Light"
    } else {
        "Dark"
    }
}

pub fn is_light_appearance(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Light | WindowAppearance::VibrantLight
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn bg_canvas() -> Rgba {
    theme_hex(0xf4f8fc, 0x191b1d)
}

pub fn bg_surface() -> Rgba {
    theme_hex(0xfcfdff, 0x1b1a1d)
}

pub fn bg_overlay() -> Rgba {
    theme_hex(0xffffff, 0x1d1f20)
}

pub fn bg_inset() -> Rgba {
    theme_hex(0xf1f5f9, 0x161819)
}

pub fn bg_subtle() -> Rgba {
    theme_hex(0xf5f8fc, 0x1e2021)
}

pub fn bg_emphasis() -> Rgba {
    theme_hex(0xe7eef7, 0x232526)
}

pub fn bg_selected() -> Rgba {
    theme_hex(0xe8f0fb, 0x202224)
}

pub fn accent() -> Rgba {
    theme_hex(0x4f6987, 0x8fa3b8)
}

pub fn accent_muted() -> Rgba {
    theme_hex_alpha((0x4f6987, 0.14), (0x8fa3b8, 0.18))
}

pub fn border_default() -> Rgba {
    theme_hex_alpha((0x7f8da0, 0.18), (0x828488, 0.24))
}

pub fn border_muted() -> Rgba {
    theme_hex_alpha((0x7f8da0, 0.10), (0x828488, 0.14))
}

pub fn diff_hunk_bg() -> Rgba {
    theme_hex(0xe9f0f8, 0x1a2026)
}

pub fn diff_hunk_fg() -> Rgba {
    theme_hex(0x41617f, 0xa4b6c8)
}

pub fn diff_context_bg() -> Rgba {
    theme_hex(0xf5f8fc, 0x191b1d)
}

pub fn diff_context_gutter_bg() -> Rgba {
    theme_hex(0xf0f5fb, 0x1b1a1d)
}

pub fn diff_meta_bg() -> Rgba {
    theme_hex(0xecf2f8, 0x1d1f20)
}

pub fn diff_add_bg() -> Rgba {
    theme_hex(0xebf7ef, 0x16201a)
}

pub fn diff_add_gutter_bg() -> Rgba {
    theme_hex(0xe0f0e6, 0x1a251e)
}

pub fn diff_add_border() -> Rgba {
    transparent()
}

pub fn diff_remove_bg() -> Rgba {
    theme_hex(0xffeef1, 0x22181b)
}

pub fn diff_remove_gutter_bg() -> Rgba {
    theme_hex(0xf8e0e6, 0x2a1d20)
}

pub fn diff_remove_border() -> Rgba {
    transparent()
}

pub fn fg_default() -> Rgba {
    theme_hex(0x455468, 0xc7cbcf)
}

pub fn fg_muted() -> Rgba {
    theme_hex(0x66768a, 0x9a9ea3)
}

pub fn fg_subtle() -> Rgba {
    theme_hex(0x90a0b2, 0x828488)
}

pub fn fg_emphasis() -> Rgba {
    theme_hex(0x1b2736, 0xf2f3f5)
}

pub fn success() -> Rgba {
    theme_hex(0x1f7a3f, 0x79be84)
}

pub fn success_muted() -> Rgba {
    theme_hex_alpha((0x1f7a3f, 0.12), (0x79be84, 0.14))
}

pub fn danger() -> Rgba {
    theme_hex(0xbe3c4d, 0xe1848d)
}

pub fn danger_muted() -> Rgba {
    theme_hex_alpha((0xbe3c4d, 0.12), (0xe1848d, 0.14))
}

pub fn purple() -> Rgba {
    theme_hex(0x7b57f6, 0xb396df)
}

pub fn waypoint_bg() -> Rgba {
    theme_hex_alpha((0x7b57f6, 0.10), (0xb396df, 0.16))
}

pub fn waypoint_active_bg() -> Rgba {
    theme_hex_alpha((0x7b57f6, 0.16), (0xb396df, 0.24))
}

pub fn waypoint_border() -> Rgba {
    theme_hex_alpha((0x7b57f6, 0.24), (0xb396df, 0.34))
}

pub fn waypoint_fg() -> Rgba {
    theme_hex(0x7b57f6, 0xe3d4fb)
}

pub fn waypoint_icon_bg() -> Rgba {
    theme_hex(0xf2ecff, 0x2b2338)
}

pub fn waypoint_icon_border() -> Rgba {
    theme_hex_alpha((0x7b57f6, 0.42), (0xb396df, 0.40))
}

pub fn waypoint_icon_core() -> Rgba {
    theme_hex(0x7b57f6, 0xd4bcff)
}

pub fn hover_bg() -> Rgba {
    theme_hex(0xecf3fb, 0x232526)
}

pub fn palette_backdrop() -> Rgba {
    theme_rgba((0.08, 0.13, 0.18, 0.14), (0.02, 0.02, 0.03, 0.58))
}

pub fn topbar_height() -> Pixels {
    px(48.0)
}

pub fn sidebar_width() -> Pixels {
    px(260.0)
}

pub fn file_tree_width() -> Pixels {
    px(252.0)
}

pub fn detail_side_width() -> Pixels {
    px(280.0)
}

pub fn radius() -> Pixels {
    px(6.0)
}

pub fn radius_sm() -> Pixels {
    px(4.0)
}

pub fn lane_accent_color(repo: &str) -> Rgba {
    let hash: u32 = repo.bytes().fold(5381u32, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as u32)
    });
    let palette = match active_theme() {
        ActiveTheme::Light => [
            hex(0x3f8a5b), // green
            hex(0xb46c2e), // orange
            hex(0x7a5dbf), // purple
            hex(0x447aa9), // blue
            hex(0xb45e78), // pink
            hex(0x826bb6), // lavender
            hex(0x2f8791), // teal
            hex(0xa17f28), // yellow
        ],
        ActiveTheme::Dark => [
            hex(0x7fbe89), // green
            hex(0xd19a68), // orange
            hex(0xb494e0), // purple
            hex(0x88a8c3), // blue
            hex(0xc98ca5), // pink
            hex(0xbaa3d8), // lavender
            hex(0x7fb5bb), // teal
            hex(0xd3bf6f), // yellow
        ],
    };
    palette[(hash as usize) % palette.len()]
}
