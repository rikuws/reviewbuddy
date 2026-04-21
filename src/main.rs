mod agents;
mod app_assets;
mod app_storage;
mod cache;
mod code_display;
mod code_tour;
mod code_tour_background;
mod diff;
mod gh;
mod github;
mod local_documents;
mod local_repo;
mod lsp;
mod managed_lsp;
mod markdown;
mod notifications;
mod platform_macos;
mod review_context;
mod review_graph;
mod review_queue;
mod review_routes;
mod review_session;
mod selectable_text;
mod semantic_diff;
mod source_browser;
mod state;
mod syntax;
mod theme;
mod views;

use gpui::*;

use app_assets::AppAssets;
use app_storage::cache_path;
use cache::CacheStore;
use platform_macos::apply_app_icon;
use state::AppState;
use views::{
    blur_review_editor, close_palette, close_review_line_action, close_waypoint_spotlight,
    execute_palette_selection, execute_waypoint_spotlight_selection, move_palette_selection,
    move_waypoint_spotlight_selection, toggle_palette, toggle_waypoint_spotlight,
    trigger_add_waypoint_shortcut, trigger_submit_inline_comment, trigger_submit_review, RootView,
};

fn main() {
    Application::new()
        .with_assets(AppAssets::new())
        .run(|cx: &mut App| {
            apply_app_icon();

            let cache = CacheStore::new(cache_path()).expect("Failed to initialize cache");
            let app_state = cx.new(|_| AppState::new(cache));
            let initial_window_appearance = cx.window_appearance();
            app_state.update(cx, |state, _| {
                state.set_window_appearance(initial_window_appearance);
            });

            let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("ReviewBuddy".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| RootView::new(app_state.clone(), window, cx)),
            )
            .unwrap();

            let app_state_for_keys = app_state.clone();
            cx.observe_keystrokes(move |event, window, cx| {
                let keystroke = &event.keystroke;
                let is_platform_only = keystroke.modifiers.platform
                    && !keystroke.modifiers.control
                    && !keystroke.modifiers.alt
                    && !keystroke.modifiers.function;
                let is_platform_plain = is_platform_only && !keystroke.modifiers.shift;
                let is_platform_shift = is_platform_only && keystroke.modifiers.shift;

                if is_platform_plain && keystroke.key == "k" {
                    toggle_palette(&app_state_for_keys, cx);
                    return;
                }

                let palette_open = app_state_for_keys.read(cx).palette_open;
                if palette_open {
                    match keystroke.key.as_str() {
                        "escape" => close_palette(&app_state_for_keys, cx),
                        "up" => move_palette_selection(&app_state_for_keys, -1, cx),
                        "down" => move_palette_selection(&app_state_for_keys, 1, cx),
                        "enter" => execute_palette_selection(&app_state_for_keys, window, cx),
                        _ => {}
                    }
                    return;
                }

                if is_platform_shift && keystroke.key == "j" {
                    trigger_add_waypoint_shortcut(&app_state_for_keys, cx);
                    return;
                }

                if is_platform_plain && keystroke.key == "j" {
                    toggle_waypoint_spotlight(&app_state_for_keys, cx);
                    return;
                }

                let waypoint_spotlight_open = app_state_for_keys.read(cx).waypoint_spotlight_open;
                if waypoint_spotlight_open {
                    match keystroke.key.as_str() {
                        "escape" => close_waypoint_spotlight(&app_state_for_keys, cx),
                        "up" => move_waypoint_spotlight_selection(&app_state_for_keys, -1, cx),
                        "down" => move_waypoint_spotlight_selection(&app_state_for_keys, 1, cx),
                        "enter" => {
                            execute_waypoint_spotlight_selection(&app_state_for_keys, window, cx)
                        }
                        _ => {}
                    }
                    return;
                }

                let line_action_active = app_state_for_keys
                    .read(cx)
                    .active_review_line_action
                    .is_some();
                let line_comment_mode = app_state_for_keys.read(cx).review_line_action_mode
                    == state::ReviewLineActionMode::Comment;

                if line_action_active {
                    if is_platform_plain && keystroke.key == "enter" && line_comment_mode {
                        trigger_submit_inline_comment(&app_state_for_keys, window, cx);
                        return;
                    }

                    if keystroke.key == "escape" {
                        close_review_line_action(&app_state_for_keys, cx);
                        return;
                    }
                }

                let review_editor_active = app_state_for_keys.read(cx).review_editor_active;
                if !review_editor_active {
                    return;
                }

                if is_platform_plain && keystroke.key == "enter" {
                    trigger_submit_review(&app_state_for_keys, window, cx);
                    return;
                }

                match keystroke.key.as_str() {
                    "escape" => blur_review_editor(&app_state_for_keys, cx),
                    _ => {}
                }
            })
            .detach();
        });
}
