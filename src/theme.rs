use gpui::{px, Pixels, Rgba};

fn color(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

fn hex(hex: u32) -> Rgba {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: 1.0 }
}

// GitHub dark-inspired background colors
pub fn bg_canvas() -> Rgba {
    hex(0x0d1117)
}

pub fn bg_surface() -> Rgba {
    hex(0x161b22)
}

pub fn bg_overlay() -> Rgba {
    hex(0x1c2128)
}

pub fn bg_inset() -> Rgba {
    hex(0x010409)
}

pub fn bg_subtle() -> Rgba {
    hex(0x1a1f26)
}

pub fn bg_emphasis() -> Rgba {
    hex(0x21262d)
}

pub fn bg_selected() -> Rgba {
    hex(0x2d333b)
}

pub fn accent() -> Rgba {
    hex(0x2f81f7)
}

pub fn accent_muted() -> Rgba {
    color(0.184, 0.506, 0.969, 0.18)
}

pub fn border_default() -> Rgba {
    hex(0x30363d)
}

pub fn border_muted() -> Rgba {
    hex(0x21262d)
}

pub fn diff_hunk_bg() -> Rgba {
    hex(0x111d2f)
}

pub fn diff_hunk_fg() -> Rgba {
    hex(0x79c0ff)
}

pub fn diff_context_bg() -> Rgba {
    hex(0x0d1117)
}

pub fn diff_context_gutter_bg() -> Rgba {
    hex(0x161b22)
}

pub fn diff_meta_bg() -> Rgba {
    hex(0x161b22)
}

// Subtle diff tints — barely perceptible so syntax highlighting can dominate
pub fn diff_add_bg() -> Rgba {
    hex(0x0f1a14)
}

pub fn diff_add_gutter_bg() -> Rgba {
    hex(0x122016)
}

pub fn diff_add_border() -> Rgba {
    hex(0x1e2e25)
}

pub fn diff_remove_bg() -> Rgba {
    hex(0x1a0f10)
}

pub fn diff_remove_gutter_bg() -> Rgba {
    hex(0x201214)
}

pub fn diff_remove_border() -> Rgba {
    hex(0x2e1e21)
}

// Foreground colors
pub fn fg_default() -> Rgba {
    hex(0xc9d1d9)
}

pub fn fg_muted() -> Rgba {
    hex(0x8b949e)
}

pub fn fg_subtle() -> Rgba {
    hex(0x6e7681)
}

pub fn fg_emphasis() -> Rgba {
    hex(0xf0f6fc)
}

pub fn success() -> Rgba {
    hex(0x3fb950)
}

pub fn success_muted() -> Rgba {
    color(0.247, 0.725, 0.314, 0.12)
}

pub fn danger() -> Rgba {
    hex(0xf85149)
}

pub fn danger_muted() -> Rgba {
    color(0.973, 0.318, 0.286, 0.12)
}

pub fn purple() -> Rgba {
    hex(0xa371f7)
}

pub fn hover_bg() -> Rgba {
    hex(0x262c36)
}

pub fn palette_backdrop() -> Rgba {
    color(0.0, 0.0, 0.0, 0.7)
}

// Sizes
pub fn topbar_height() -> Pixels {
    px(48.0)
}

pub fn sidebar_width() -> Pixels {
    px(260.0)
}

pub fn file_tree_width() -> Pixels {
    px(240.0)
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
