use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{linear_color_stop, linear_gradient, px, Background, Pixels, Rgba, WindowAppearance};
use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;

const THEME_SETTINGS_CACHE_KEY: &str = "theme-settings-v1";
const UI_FONT_FAMILY: &str = ".AppleSystemUIFont";
const MONO_FONT_FAMILY: &str = "Fira Code";
const DISPLAY_SERIF_FONT_FAMILY: &str = "Instrument Serif";

const LIGHT_CANVAS: u32 = 0xf6f8fb;
const LIGHT_SURFACE: u32 = 0xffffff;
const LIGHT_ELEVATED: u32 = 0xffffff;
const LIGHT_INSET: u32 = 0xedf2f7;
const LIGHT_SUBTLE: u32 = 0xf1f5f9;
const LIGHT_EMPHASIS: u32 = 0xe7edf5;
const LIGHT_SELECTED: u32 = 0xf0f3f7;
const LIGHT_TEXT_EMPHASIS: u32 = 0x101828;
const LIGHT_TEXT: u32 = 0x344054;
const LIGHT_TEXT_MUTED: u32 = 0x667085;
const LIGHT_TEXT_SUBTLE: u32 = 0x98a2b3;
const LIGHT_BORDER: u32 = 0xd6dee8;
const LIGHT_BORDER_MUTED: u32 = 0xe5ebf2;

const DARK_CANVAS: u32 = 0x0b1118;
const DARK_SURFACE: u32 = 0x111821;
const DARK_ELEVATED: u32 = 0x151f2b;
const DARK_INSET: u32 = 0x080d14;
const DARK_SUBTLE: u32 = 0x151d28;
const DARK_EMPHASIS: u32 = 0x1d2939;
const DARK_SELECTED: u32 = 0x1a2431;
const DARK_TEXT_EMPHASIS: u32 = 0xf3f7fb;
const DARK_TEXT: u32 = 0xd6dee8;
const DARK_TEXT_MUTED: u32 = 0x9aa8ba;
const DARK_TEXT_SUBTLE: u32 = 0x728196;
const DARK_BORDER: u32 = 0x2b3848;
const DARK_BORDER_MUTED: u32 = 0x202b38;

const LIGHT_FOCUS: u32 = 0x2563eb;
const DARK_FOCUS: u32 = 0x7db4ff;
const LIGHT_SUCCESS: u32 = 0x16a34a;
const DARK_SUCCESS: u32 = 0x6ee7a5;
const LIGHT_WARNING: u32 = 0xd97706;
const DARK_WARNING: u32 = 0xfbbf24;
const LIGHT_DANGER: u32 = 0xdc2626;
const DARK_DANGER: u32 = 0xfca5a5;
const LIGHT_INFO: u32 = 0x0891b2;
const DARK_INFO: u32 = 0x67e8f9;

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

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(ActiveTheme::Light as u8);

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

pub fn ui_font_family() -> &'static str {
    UI_FONT_FAMILY
}

pub fn mono_font_family() -> &'static str {
    MONO_FONT_FAMILY
}

pub fn display_serif_font_family() -> &'static str {
    DISPLAY_SERIF_FONT_FAMILY
}

pub fn bg_canvas() -> Rgba {
    theme_hex(LIGHT_CANVAS, DARK_CANVAS)
}

pub fn bg_surface() -> Rgba {
    theme_hex(LIGHT_SURFACE, DARK_SURFACE)
}

pub fn bg_overlay() -> Rgba {
    theme_hex(LIGHT_ELEVATED, DARK_ELEVATED)
}

pub fn bg_inset() -> Rgba {
    theme_hex(LIGHT_INSET, DARK_INSET)
}

pub fn bg_subtle() -> Rgba {
    theme_hex(LIGHT_SUBTLE, DARK_SUBTLE)
}

pub fn bg_emphasis() -> Rgba {
    theme_hex(LIGHT_EMPHASIS, DARK_EMPHASIS)
}

pub fn bg_selected() -> Rgba {
    theme_hex(LIGHT_SELECTED, DARK_SELECTED)
}

pub fn focus() -> Rgba {
    theme_hex(LIGHT_FOCUS, DARK_FOCUS)
}

pub fn focus_muted() -> Rgba {
    theme_hex_alpha((LIGHT_FOCUS, 0.12), (DARK_FOCUS, 0.18))
}

pub fn focus_border() -> Rgba {
    theme_hex_alpha((LIGHT_FOCUS, 0.42), (DARK_FOCUS, 0.54))
}

pub fn fg_on_focus() -> Rgba {
    theme_hex(0xffffff, 0x07111f)
}

pub fn primary_action_bg() -> Rgba {
    theme_hex(0x05070c, 0xf3f7fb)
}

pub fn primary_action_hover() -> Rgba {
    theme_hex(0x1f2937, 0xd6dee8)
}

pub fn fg_on_primary_action() -> Rgba {
    theme_hex(0xffffff, 0x07111f)
}

pub fn accent() -> Rgba {
    focus()
}

pub fn accent_muted() -> Rgba {
    focus_muted()
}

pub fn warning() -> Rgba {
    theme_hex(LIGHT_WARNING, DARK_WARNING)
}

pub fn warning_muted() -> Rgba {
    theme_hex_alpha((LIGHT_WARNING, 0.13), (DARK_WARNING, 0.18))
}

pub fn info() -> Rgba {
    theme_hex(LIGHT_INFO, DARK_INFO)
}

pub fn info_muted() -> Rgba {
    theme_hex_alpha((LIGHT_INFO, 0.12), (DARK_INFO, 0.18))
}

pub fn brand_accent() -> Rgba {
    theme_hex(0x6d5dfc, 0xa7a2ff)
}

pub fn brand_accent_muted() -> Rgba {
    theme_hex_alpha((0x6d5dfc, 0.10), (0xa7a2ff, 0.16))
}

pub fn border_default() -> Rgba {
    theme_hex_alpha((LIGHT_BORDER, 0.92), (DARK_BORDER, 0.92))
}

pub fn border_muted() -> Rgba {
    theme_hex_alpha((LIGHT_BORDER_MUTED, 0.90), (DARK_BORDER_MUTED, 0.92))
}

pub fn diff_hunk_bg() -> Rgba {
    theme_hex(0xeaf2ff, 0x102033)
}

pub fn diff_hunk_fg() -> Rgba {
    focus()
}

pub fn diff_context_bg() -> Rgba {
    theme_hex(0xfbfdff, 0x0b1118)
}

pub fn diff_context_gutter_bg() -> Rgba {
    theme_hex(0xf1f5f9, 0x101720)
}

pub fn diff_meta_bg() -> Rgba {
    theme_hex(0xeaf2ff, 0x102033)
}

pub fn diff_add_bg() -> Rgba {
    theme_hex(0xecfdf3, 0x0c1f16)
}

pub fn diff_add_gutter_bg() -> Rgba {
    theme_hex(0xdcfce7, 0x123320)
}

pub fn diff_add_emphasis_bg() -> Rgba {
    theme_hex_alpha((LIGHT_SUCCESS, 0.16), (DARK_SUCCESS, 0.20))
}

pub fn diff_add_border() -> Rgba {
    theme_hex_alpha((LIGHT_SUCCESS, 0.28), (DARK_SUCCESS, 0.28))
}

pub fn diff_remove_bg() -> Rgba {
    theme_hex(0xfef2f2, 0x2a1216)
}

pub fn diff_remove_gutter_bg() -> Rgba {
    theme_hex(0xfee2e2, 0x3a171c)
}

pub fn diff_remove_emphasis_bg() -> Rgba {
    theme_hex_alpha((LIGHT_DANGER, 0.15), (DARK_DANGER, 0.20))
}

pub fn diff_remove_border() -> Rgba {
    theme_hex_alpha((LIGHT_DANGER, 0.26), (DARK_DANGER, 0.28))
}

pub fn fg_default() -> Rgba {
    theme_hex(LIGHT_TEXT, DARK_TEXT)
}

pub fn fg_muted() -> Rgba {
    theme_hex(LIGHT_TEXT_MUTED, DARK_TEXT_MUTED)
}

pub fn fg_subtle() -> Rgba {
    theme_hex(LIGHT_TEXT_SUBTLE, DARK_TEXT_SUBTLE)
}

pub fn fg_emphasis() -> Rgba {
    theme_hex(LIGHT_TEXT_EMPHASIS, DARK_TEXT_EMPHASIS)
}

pub fn success() -> Rgba {
    theme_hex(LIGHT_SUCCESS, DARK_SUCCESS)
}

pub fn success_muted() -> Rgba {
    theme_hex_alpha((LIGHT_SUCCESS, 0.12), (DARK_SUCCESS, 0.16))
}

pub fn danger() -> Rgba {
    theme_hex(LIGHT_DANGER, DARK_DANGER)
}

pub fn danger_muted() -> Rgba {
    theme_hex_alpha((LIGHT_DANGER, 0.12), (DARK_DANGER, 0.16))
}

pub fn waypoint_bg() -> Rgba {
    warning_muted()
}

pub fn waypoint_active_bg() -> Rgba {
    theme_hex_alpha((LIGHT_WARNING, 0.18), (DARK_WARNING, 0.24))
}

pub fn waypoint_border() -> Rgba {
    theme_hex_alpha((LIGHT_WARNING, 0.34), (DARK_WARNING, 0.38))
}

pub fn waypoint_fg() -> Rgba {
    warning()
}

pub fn waypoint_icon_bg() -> Rgba {
    warning_muted()
}

pub fn waypoint_icon_border() -> Rgba {
    waypoint_border()
}

pub fn waypoint_icon_core() -> Rgba {
    warning()
}

pub fn hover_bg() -> Rgba {
    theme_hex(0xf2f5f8, 0x1b2531)
}

pub fn material_gradient(seed: &str) -> Background {
    match material_index(seed) {
        0 => linear_gradient(
            126.0,
            linear_color_stop(theme_hex(0xd8d1ff, 0x352c72), 0.0),
            linear_color_stop(theme_hex(0x28f3e3, 0x04c4d7), 1.0),
        ),
        1 => linear_gradient(
            132.0,
            linear_color_stop(theme_hex(0xf6b6ff, 0x642c84), 0.0),
            linear_color_stop(theme_hex(0xff4f59, 0xe94b58), 1.0),
        ),
        _ => linear_gradient(
            102.0,
            linear_color_stop(theme_hex(0x37c9ff, 0x0b6fb6), 0.0),
            linear_color_stop(theme_hex(0xffb15c, 0xe57731), 1.0),
        ),
    }
}

pub fn material_glow(seed: &str) -> Background {
    match material_index(seed) {
        0 => linear_gradient(
            58.0,
            linear_color_stop(theme_hex_alpha((0x7cffd5, 0.84), (0x7cffd5, 0.46)), 0.0),
            linear_color_stop(theme_hex_alpha((0xf2ff7a, 0.72), (0xf2ff7a, 0.36)), 1.0),
        ),
        1 => linear_gradient(
            58.0,
            linear_color_stop(theme_hex_alpha((0xffd4f5, 0.76), (0xff8fd8, 0.36)), 0.0),
            linear_color_stop(theme_hex_alpha((0xff7b8a, 0.72), (0xff7b8a, 0.38)), 1.0),
        ),
        _ => linear_gradient(
            58.0,
            linear_color_stop(theme_hex_alpha((0x00f0ff, 0.70), (0x00c2ff, 0.42)), 0.0),
            linear_color_stop(theme_hex_alpha((0xffe173, 0.76), (0xffbd4a, 0.42)), 1.0),
        ),
    }
}

pub fn material_mark(seed: &str) -> Rgba {
    match material_index(seed) {
        0 => theme_hex(0x33f2d8, 0x6ee7f9),
        1 => theme_hex(0xff5d7c, 0xff8fb3),
        _ => theme_hex(0xffa43b, 0xffbd63),
    }
}

fn material_index(seed: &str) -> usize {
    let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
        acc.wrapping_mul(16777619) ^ byte as u32
    });
    (hash as usize) % 3
}

pub fn palette_backdrop() -> Rgba {
    theme_rgba((0.05, 0.09, 0.16, 0.18), (0.01, 0.02, 0.04, 0.72))
}

pub fn topbar_height() -> Pixels {
    px(48.0)
}

pub fn sidebar_width() -> Pixels {
    px(248.0)
}

pub fn file_tree_width() -> Pixels {
    px(268.0)
}

pub fn detail_side_width() -> Pixels {
    px(312.0)
}

pub fn radius() -> Pixels {
    px(12.0)
}

pub fn radius_sm() -> Pixels {
    px(8.0)
}

pub fn radius_lg() -> Pixels {
    px(18.0)
}

pub fn lane_accent_color(repo: &str) -> Rgba {
    let hash: u32 = repo.bytes().fold(5381u32, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as u32)
    });
    let palette = match active_theme() {
        ActiveTheme::Light => [
            hex(LIGHT_FOCUS),
            hex(LIGHT_INFO),
            hex(LIGHT_SUCCESS),
            hex(LIGHT_WARNING),
            hex(0x7c3aed),
            hex(0xdb2777),
            hex(0x4f46e5),
            hex(0x0f766e),
        ],
        ActiveTheme::Dark => [
            hex(DARK_FOCUS),
            hex(DARK_INFO),
            hex(DARK_SUCCESS),
            hex(DARK_WARNING),
            hex(0xc4b5fd),
            hex(0xf9a8d4),
            hex(0xaaa6ff),
            hex(0x5eead4),
        ],
    };
    palette[(hash as usize) % palette.len()]
}
