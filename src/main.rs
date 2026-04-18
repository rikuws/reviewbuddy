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
mod selectable_text;
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
    blur_review_editor, close_palette, execute_palette_selection, move_palette_selection,
    toggle_palette, trigger_submit_review, RootView,
};

fn main() {
    Application::new()
        .with_assets(AppAssets::new())
        .run(|cx: &mut App| {
            apply_app_icon();

            let cache = CacheStore::new(cache_path()).expect("Failed to initialize cache");
            let app_state = cx.new(|_| AppState::new(cache));

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

                if is_platform_only && keystroke.key == "k" {
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

                let review_editor_active = app_state_for_keys.read(cx).review_editor_active;
                if !review_editor_active {
                    return;
                }

                if is_platform_only && keystroke.key == "enter" {
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
