use gpui::prelude::*;
use gpui::*;

use crate::app_assets::APP_LOGO_ASSET;
use crate::github;
use crate::state::*;
use crate::theme::*;

use super::palette::render_palette;
use super::pr_detail::render_pr_workspace;
use super::sections::{badge, ghost_button, render_section_workspace};
use super::settings::prepare_settings_view;
use super::workspace_sync::{
    sync_workspace_flow, trigger_sync_workspace, wait_for_workspace_poll_interval,
};

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
    let is_authenticated = s.is_authenticated();
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let gh_version = s.gh_version.clone();

    let state_for_nav = state.clone();
    let state_for_tabs = state.clone();
    let state_for_sync = state.clone();

    div()
        .bg(bg_surface())
        .border_b(px(1.0))
        .border_color(border_default())
        .flex_shrink_0()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .px(px(20.0))
                .h(topbar_height())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(18.0))
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .flex_shrink_0()
                                .child(img(APP_LOGO_ASSET).size(px(24.0)))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child("ReviewBuddy"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_family("Fira Code")
                                                .text_color(fg_subtle())
                                                .child("desktop review workspace"),
                                        ),
                                ),
                        )
                        .child(
                            div().flex().gap(px(4.0)).items_center().min_w_0().children(
                                SectionId::all()
                                    .iter()
                                    .filter(|section| **section != SectionId::Issues)
                                    .map(|section| {
                                        let section = *section;
                                        let is_active =
                                            active_section == section && active_pr_key.is_none();
                                        let count = s.section_count(section);
                                        let state = state_for_nav.clone();
                                        nav_pill(
                                            section.label(),
                                            count,
                                            is_active,
                                            move |_, window, cx| {
                                                if section == SectionId::Settings {
                                                    prepare_settings_view(&state, window, cx);
                                                }
                                                state.update(cx, |s, cx| {
                                                    s.active_section = section;
                                                    s.active_pr_key = None;
                                                    s.palette_open = false;
                                                    s.palette_selected_index = 0;
                                                    cx.notify();
                                                });
                                            },
                                        )
                                    }),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .flex_wrap()
                        .justify_end()
                        .child(if workspace_syncing {
                            badge("syncing workspace").into_any_element()
                        } else if workspace_error.is_some() {
                            badge("sync issue").into_any_element()
                        } else if is_authenticated {
                            badge("github connected").into_any_element()
                        } else {
                            badge("gh needs auth").into_any_element()
                        })
                        .when_some(gh_version, |el, version| {
                            el.child(badge(version.split_whitespace().next().unwrap_or(&version)))
                        })
                        .child(ghost_button(
                            if workspace_syncing {
                                "Syncing..."
                            } else {
                                "Sync"
                            },
                            move |_, window, cx| {
                                trigger_sync_workspace(&state_for_sync, window, cx)
                            },
                        )),
                ),
        )
        .when(!tabs.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(20.0))
                    .py(px(8.0))
                    .border_t(px(1.0))
                    .border_color(border_muted())
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_family("Fira Code")
                            .text_color(fg_subtle())
                            .flex_shrink_0()
                            .child(format!("{} OPEN", tabs.len())),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.0))
                            .items_center()
                            .id("topbar-tabs-scroll")
                            .overflow_x_scroll()
                            .min_w_0()
                            .children(tabs.into_iter().map(|tab| {
                                let key = pr_key(&tab.repository, tab.number);
                                let is_active = active_pr_key.as_deref() == Some(&key);
                                let state = state_for_tabs.clone();
                                pr_tab(
                                    &tab.repository,
                                    tab.number,
                                    &tab.title,
                                    tab.additions,
                                    tab.deletions,
                                    &tab.state,
                                    tab.is_draft,
                                    is_active,
                                    move |_, _, cx| {
                                        state.update(cx, |s, cx| {
                                            s.active_pr_key = Some(key.clone());
                                            s.active_section = SectionId::Pulls;
                                            s.palette_open = false;
                                            s.palette_selected_index = 0;
                                            cx.notify();
                                        });
                                    },
                                )
                            })),
                    ),
            )
        })
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
        .px(px(12.0))
        .py(px(5.0))
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
    repository: &str,
    number: i64,
    title: &str,
    additions: i64,
    deletions: i64,
    pr_state: &str,
    is_draft: bool,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let dot_color = pr_tab_state_dot(pr_state, is_draft);
    let state_badge = pr_tab_state_badge(pr_state, is_draft);
    let repo_short = repository
        .split('/')
        .last()
        .unwrap_or(repository)
        .to_string();
    let tab_label = format!("#{number} {title}");

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            border_default()
        } else {
            border_muted()
        })
        .bg(if active { bg_canvas() } else { bg_surface() })
        .text_size(px(12.0))
        .max_w(px(320.0))
        .min_w_0()
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if active { bg_canvas() } else { hover_bg() })
                .border_color(border_default())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .min_w_0()
                .flex_grow()
                .child(
                    div()
                        .w(px(5.0))
                        .h(px(5.0))
                        .rounded(px(999.0))
                        .bg(dot_color)
                        .flex_shrink_0(),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(999.0))
                        .bg(if active { bg_surface() } else { bg_emphasis() })
                        .text_size(px(10.0))
                        .font_family("Fira Code")
                        .text_color(if active { fg_default() } else { fg_subtle() })
                        .flex_shrink_0()
                        .child(repo_short),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { fg_emphasis() } else { fg_default() })
                        .child(tab_label),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .items_center()
                .text_size(px(10.0))
                .font_family("Fira Code")
                .whitespace_nowrap()
                .flex_shrink_0()
                .when_some(state_badge, |el, badge| el.child(badge))
                .child(div().text_color(success()).child(format!("+{additions}")))
                .child(div().text_color(danger()).child(format!("-{deletions}"))),
        )
}

fn pr_tab_state_dot(pr_state: &str, is_draft: bool) -> Rgba {
    if is_draft {
        return fg_muted();
    }

    match pr_state {
        "MERGED" => purple(),
        "CLOSED" => danger(),
        _ => success(),
    }
}

fn pr_tab_state_badge(pr_state: &str, is_draft: bool) -> Option<AnyElement> {
    if is_draft {
        return Some(
            pr_tab_badge("Draft", fg_muted(), bg_emphasis(), border_muted()).into_any_element(),
        );
    }

    match pr_state {
        "MERGED" => {
            Some(pr_tab_badge("Merged", purple(), bg_emphasis(), purple()).into_any_element())
        }
        "CLOSED" => Some(
            pr_tab_badge("Closed", danger(), danger_muted(), diff_remove_border())
                .into_any_element(),
        ),
        _ => None,
    }
}

fn pr_tab_badge(label: &str, fg: Rgba, bg: Rgba, _border: Rgba) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg)
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.to_string())
}
