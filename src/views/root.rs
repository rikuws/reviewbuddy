use gpui::prelude::*;
use gpui::*;

use crate::app_assets::APP_LOGO_ASSET;
use crate::github;
use crate::state::*;
use crate::theme::*;

use super::palette::render_palette;
use super::pr_detail::render_pr_workspace;
use super::sections::render_section_workspace;
use super::settings::ensure_managed_lsp_statuses_loaded;
use super::workspace_sync::{sync_workspace_flow, wait_for_workspace_poll_interval};

pub struct RootView {
    state: Entity<AppState>,
}

impl RootView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Bootstrap: load workspace from cache, then sync in background.
        let model = state.clone();
        cx.spawn_in(window, async move |_this, cx| {
            // Load bootstrap status
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };

            let result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move { github::load_workspace_snapshot(&cache) }
                })
                .await;

            model
                .update(cx, |state, cx| {
                    state.workspace_loading = false;
                    state.bootstrap_loading = false;
                    match &result {
                        Ok(ws) => {
                            state.gh_available = ws.auth.is_authenticated;
                            state.workspace = Some(ws.clone());
                        }
                        Err(e) => {
                            state.workspace_error = Some(e.clone());
                        }
                    }
                    cx.notify();
                })
                .ok();

            // Check gh version
            let gh_result = cx
                .background_executor()
                .spawn(async { crate::gh::run(&["--version"]) })
                .await;

            model
                .update(cx, |state, cx| {
                    if let Ok(output) = gh_result {
                        if output.exit_code == Some(0) {
                            state.gh_available = true;
                            state.gh_version = output.stdout.lines().next().map(str::to_string);
                        }
                    }
                    cx.notify();
                })
                .ok();

            // Now sync workspace in background.
            model
                .update(cx, |state, cx| {
                    state.workspace_syncing = true;
                    cx.notify();
                })
                .ok();

            sync_workspace_flow(model.clone(), cx).await;

            loop {
                wait_for_workspace_poll_interval(cx).await;

                let should_sync = model
                    .read_with(cx, |state, _| {
                        state.is_authenticated() && !state.workspace_syncing
                    })
                    .ok()
                    .unwrap_or(false);
                if !should_sync {
                    continue;
                }

                model
                    .update(cx, |state, cx| {
                        if state.workspace_syncing {
                            return;
                        }

                        state.workspace_syncing = true;
                        cx.notify();
                    })
                    .ok();

                sync_workspace_flow(model.clone(), cx).await;
            }
        })
        .detach();

        Self { state }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let palette_open = state.palette_open;

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg_canvas())
            .text_color(fg_default())
            .text_size(px(14.0))
            .font_family(".AppleSystemUIFont")
            .child(render_topbar(&self.state, cx))
            .child(render_workspace_area(&self.state, cx))
            .when(palette_open, |el| el.child(render_palette(&self.state, cx)))
    }
}

fn render_topbar(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let active_section = s.active_section;
    let active_pr_key = s.active_pr_key.clone();
    let tabs: Vec<_> = s.open_tabs.clone();

    let state_for_nav = state.clone();
    let state_for_tabs = state.clone();

    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(20.0))
        .h(topbar_height())
        .bg(bg_surface())
        .flex_shrink_0()
        // Brand mark
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .mr(px(16.0))
                .flex_shrink_0()
                .child(img(APP_LOGO_ASSET).size(px(28.0)))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("gh-ui"),
                ),
        )
        // Section nav
        .child(
            div()
                .flex()
                .gap(px(1.0))
                .items_center()
                .mr(px(8.0))
                .children(SectionId::all().iter().map(|section| {
                    let section = *section;
                    let is_active = active_section == section && active_pr_key.is_none();
                    let count = s.section_count(section);
                    let state = state_for_nav.clone();
                    nav_pill(section.label(), count, is_active, move |_, window, cx| {
                        if section == SectionId::Settings {
                            ensure_managed_lsp_statuses_loaded(&state, window, cx);
                        }
                        state.update(cx, |s, cx| {
                            s.active_section = section;
                            s.active_pr_key = None;
                            s.palette_open = false;
                            cx.notify();
                        });
                    })
                })),
        )
        // PR tabs
        .child(
            div()
                .flex()
                .gap(px(1.0))
                .items_center()
                .overflow_x_hidden()
                .pl(px(16.0))
                .ml(px(8.0))
                .children(tabs.into_iter().map(|tab| {
                    let key = pr_key(&tab.repository, tab.number);
                    let is_active = active_pr_key.as_deref() == Some(&key);
                    let state = state_for_tabs.clone();
                    pr_tab(
                        &tab.title,
                        tab.additions,
                        tab.deletions,
                        &tab.state,
                        is_active,
                        move |_, _, cx| {
                            state.update(cx, |s, cx| {
                                s.active_pr_key = Some(key.clone());
                                s.active_section = SectionId::Pulls;
                                s.palette_open = false;
                                cx.notify();
                            });
                        },
                    )
                })),
        )
}

fn render_workspace_area(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let has_active_pr = s.active_pr_key.is_some();

    div()
        .flex_grow()
        .min_h_0()
        .flex()
        .flex_col()
        .child(if has_active_pr {
            render_pr_workspace(state, cx).into_any_element()
        } else {
            render_section_workspace(state, cx).into_any_element()
        })
}

fn nav_pill(
    label: &str,
    count: i64,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .gap(px(6.0))
        .items_center()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(active, |el| el.bg(bg_selected()).text_color(fg_emphasis()))
        .when(!active, |el| el.text_color(fg_muted()))
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
        .when(count > 0, |el| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .text_color(if active { fg_default() } else { fg_subtle() })
                    .font_family("Fira Code")
                    .child(count.to_string()),
            )
        })
}

fn pr_tab(
    title: &str,
    additions: i64,
    deletions: i64,
    pr_state: &str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let dot_color = match pr_state {
        "MERGED" => purple(),
        "CLOSED" => danger(),
        _ => success(),
    };

    div()
        .flex()
        .gap(px(6.0))
        .items_center()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .text_size(px(12.0))
        .max_w(px(220.0))
        .cursor_pointer()
        .when(active, |el| el.bg(bg_selected()).text_color(fg_emphasis()))
        .when(!active, |el| el.text_color(fg_muted()))
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, on_click)
        // Status dot
        .child(
            div()
                .w(px(7.0))
                .h(px(7.0))
                .rounded(px(4.0))
                .bg(dot_color)
                .flex_shrink_0(),
        )
        // Title
        .child(
            div()
                .overflow_x_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(title.to_string()),
        )
        // Delta
        .child(
            div()
                .flex()
                .gap(px(3.0))
                .text_size(px(11.0))
                .font_family("Fira Code")
                .whitespace_nowrap()
                .flex_shrink_0()
                .child(div().text_color(success()).child(format!("+{additions}")))
                .child(div().text_color(fg_subtle()).child(format!("-{deletions}"))),
        )
}
