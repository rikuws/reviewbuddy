use std::f32::consts::TAU;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::*;
use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, px, Background, Bounds,
    ColorSpace, IntoElement, PathBuilder, Pixels, Point, Rgba, Window,
};

use crate::theme::bg_canvas;

const PERIOD_SECONDS: f32 = 5.0;
pub(super) const WELCOME_SHADER_RADIUS: f32 = 8.0;

pub(super) fn render_welcome_shader() -> impl IntoElement {
    div()
        .id("overview-welcome-shader")
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .left(px(0.0))
        .w_full()
        .h_full()
        .rounded(px(WELCOME_SHADER_RADIUS))
        .overflow_hidden()
        .child(
            canvas(
                |_bounds, window, _cx| {
                    window.request_animation_frame();
                    shader_time()
                },
                |bounds, time, window, _cx| paint_welcome_mesh_gradient(bounds, time, window),
            )
            .size_full(),
        )
}

fn paint_welcome_mesh_gradient(bounds: Bounds<Pixels>, time: f32, window: &mut Window) {
    let w = bounds.size.width / px(1.0);
    let h = bounds.size.height / px(1.0);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let phase = (time / PERIOD_SECONDS).fract();
    let t = phase * TAU;

    window.paint_quad(fill(bounds, rgba_hex(0x02040a, 0xff)));
    paint_gradient_layer(
        bounds,
        34.0 + 8.0 * t.sin(),
        rgba_hex(0xff5b24, 0x96),
        rgba_hex(0xff5b24, 0x00),
        window,
    );
    paint_gradient_layer(
        bounds,
        142.0 + 12.0 * (t * 0.8 + 1.3).cos(),
        rgba_hex(0x73a8c5, 0xa0),
        rgba_hex(0x203a78, 0x08),
        window,
    );
    paint_gradient_layer(
        bounds,
        228.0 + 10.0 * (t * 1.1 + 0.7).sin(),
        rgba_hex(0xf3eee0, 0x72),
        rgba_hex(0x08172c, 0x00),
        window,
    );
    paint_gradient_layer(
        bounds,
        312.0 + 14.0 * (t * 0.6 + 2.2).cos(),
        rgba_hex(0x203a78, 0xb0),
        rgba_hex(0x000105, 0x18),
        window,
    );
    paint_gradient_layer(
        bounds,
        82.0 + 8.0 * (t * 1.2 + 3.1).sin(),
        rgba_hex(0x7a2419, 0x54),
        rgba_hex(0x000105, 0x00),
        window,
    );

    paint_mesh_finish(bounds, window);
    paint_corner_masks(bounds, WELCOME_SHADER_RADIUS, window);
}

fn paint_gradient_layer(
    bounds: Bounds<Pixels>,
    angle: f32,
    from: Rgba,
    to: Rgba,
    window: &mut Window,
) {
    window.paint_quad(fill(bounds, gradient(angle.rem_euclid(360.0), from, to)));
}

fn paint_mesh_finish(bounds: Bounds<Pixels>, window: &mut Window) {
    window.paint_quad(fill(
        bounds,
        gradient(0.0, rgba_hex(0x000000, 0xb2), rgba_hex(0x000000, 0x00)),
    ));
    window.paint_quad(fill(
        bounds,
        gradient(180.0, rgba_hex(0x000000, 0x00), rgba_hex(0x000000, 0xaa)),
    ));
    window.paint_quad(fill(
        bounds,
        gradient(270.0, rgba_hex(0x000000, 0xa8), rgba_hex(0x000000, 0x00)),
    ));
    window.paint_quad(fill(
        bounds,
        gradient(40.0, rgba_hex(0xffffff, 0x16), rgba_hex(0xffffff, 0x00)),
    ));
}

fn paint_corner_masks(bounds: Bounds<Pixels>, radius: f32, window: &mut Window) {
    let w = bounds.size.width / px(1.0);
    let h = bounds.size.height / px(1.0);
    let r = radius.min(w * 0.5).min(h * 0.5);
    let background = bg_canvas();

    paint_corner_mask(
        window,
        &[
            point_xy(bounds, 0.0, 0.0),
            point_xy(bounds, r, 0.0),
            point_xy(bounds, r - r * QUARTER_ARC_KAPPA, 0.0),
            point_xy(bounds, 0.0, r - r * QUARTER_ARC_KAPPA),
            point_xy(bounds, 0.0, r),
            point_xy(bounds, 0.0, 0.0),
        ],
        background,
    );
    paint_corner_mask(
        window,
        &[
            point_xy(bounds, w, 0.0),
            point_xy(bounds, w - r, 0.0),
            point_xy(bounds, w - r + r * QUARTER_ARC_KAPPA, 0.0),
            point_xy(bounds, w, r - r * QUARTER_ARC_KAPPA),
            point_xy(bounds, w, r),
            point_xy(bounds, w, 0.0),
        ],
        background,
    );
    paint_corner_mask(
        window,
        &[
            point_xy(bounds, w, h),
            point_xy(bounds, w, h - r),
            point_xy(bounds, w, h - r + r * QUARTER_ARC_KAPPA),
            point_xy(bounds, w - r + r * QUARTER_ARC_KAPPA, h),
            point_xy(bounds, w - r, h),
            point_xy(bounds, w, h),
        ],
        background,
    );
    paint_corner_mask(
        window,
        &[
            point_xy(bounds, 0.0, h),
            point_xy(bounds, 0.0, h - r),
            point_xy(bounds, 0.0, h - r + r * QUARTER_ARC_KAPPA),
            point_xy(bounds, r - r * QUARTER_ARC_KAPPA, h),
            point_xy(bounds, r, h),
            point_xy(bounds, 0.0, h),
        ],
        background,
    );
}

const QUARTER_ARC_KAPPA: f32 = 0.552_284_8;

fn paint_corner_mask(window: &mut Window, points: &[Point<Pixels>; 6], background: Rgba) {
    let mut builder = PathBuilder::fill();
    builder.move_to(points[0]);
    builder.line_to(points[1]);
    builder.cubic_bezier_to(points[4], points[2], points[3]);
    builder.line_to(points[5]);
    builder.close();

    if let Ok(path) = builder.build() {
        window.paint_path(path, background);
    }
}

fn point_xy(bounds: Bounds<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(bounds.origin.x + px(x), bounds.origin.y + px(y))
}

fn gradient(angle: f32, from: Rgba, to: Rgba) -> Background {
    linear_gradient(
        angle,
        linear_color_stop(from, 0.0),
        linear_color_stop(to, 1.0),
    )
    .color_space(ColorSpace::Oklab)
}

fn rgba_hex(rgb: u32, alpha: u8) -> Rgba {
    gpui::rgba((rgb << 8) | alpha as u32)
}

fn shader_time() -> f32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            let seconds = (duration.as_secs() % 10_000) as f32;
            seconds + duration.subsec_nanos() as f32 / 1_000_000_000.0
        })
        .unwrap_or_default()
}
