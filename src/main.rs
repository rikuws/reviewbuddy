mod app_assets;
mod cache;
mod code_tour;
mod diff;
mod gh;
mod github;
mod local_repo;
mod markdown;
mod platform_macos;
mod syntax;
mod state;
mod theme;
mod views;

use gpui::*;

use app_assets::AppAssets;
use cache::CacheStore;
use platform_macos::apply_app_icon;
use state::{cache_path, AppState};
use views::RootView;

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
                        title: Some("gh-ui".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| RootView::new(app_state, window, cx)),
            )
            .unwrap();
        });
}
