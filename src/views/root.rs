use gpui::prelude::*;
use gpui::*;

use crate::app_assets::{
    APP_MARK_ASSET, SIDEBAR_COLLAPSE_ASSET, SIDEBAR_DARK_ASSET, SIDEBAR_EXPAND_ASSET,
    SIDEBAR_LIGHT_ASSET, SIDEBAR_OVERVIEW_ASSET, SIDEBAR_PULLS_ASSET, SIDEBAR_REVIEWS_ASSET,
    SIDEBAR_SETTINGS_ASSET, SIDEBAR_SYNC_ASSET, SIDEBAR_SYSTEM_ASSET,
};
use crate::branding::{APP_NAME, APP_TAGLINE_LABEL};
use crate::github;
use crate::review_session::load_review_session;
use crate::state::*;
use crate::theme::*;

use super::palette::render_palette;
use super::pr_detail::render_pr_workspace;
use super::sections::render_section_workspace;
use super::settings::{prepare_settings_view, update_theme_preference};
use super::workspace_sync::{
    sync_workspace_flow, trigger_sync_workspace, wait_for_workspace_poll_interval,
};

pub struct RootView {
    state: Entity<AppState>,
}

const APP_SIDEBAR_EXPANDED_WIDTH: f32 = 216.0;
const APP_SIDEBAR_COLLAPSED_WIDTH: f32 = 68.0;

impl RootView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_appearance = window.appearance();
        state.update(cx, |state, _| {
            state.set_window_appearance(initial_appearance);
        });
        cx.observe_window_appearance(window, {
            let state = state.clone();
            move |_, window, cx| {
                let appearance = window.appearance();
                state.update(cx, |state, cx| {
                    state.set_window_appearance(appearance);
                    cx.notify();
                });
            }
        })
        .detach();

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

            maybe_bootstrap_debug_pull_request(&model, cache.as_ref(), cx).await;

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

async fn maybe_bootstrap_debug_pull_request(
    model: &Entity<AppState>,
    cache: &crate::cache::CacheStore,
    cx: &mut AsyncWindowContext,
) {
    let Some(debug_target) = std::env::var("REMISS_DEBUG_OPEN_PR")
        .or_else(|_| std::env::var("REVIEWBUDDY_DEBUG_OPEN_PR"))
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };

    let Some((repository, number)) = parse_debug_pull_request_target(&debug_target) else {
        return;
    };

    let snapshot = match cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = repository.clone();
            async move { github::load_pull_request_detail(&cache, &repository, number) }
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(_) => return,
    };

    let Some(detail) = snapshot.detail.clone() else {
        return;
    };

    let review_session = load_review_session(cache, &pr_key(&repository, number))
        .ok()
        .flatten();
    let summary = github::PullRequestSummary {
        repository: detail.repository.clone(),
        number: detail.number,
        title: detail.title.clone(),
        author_login: detail.author_login.clone(),
        author_avatar_url: detail.author_avatar_url.clone(),
        is_draft: detail.is_draft,
        comments_count: detail.comments_count,
        additions: detail.additions,
        deletions: detail.deletions,
        changed_files: detail.changed_files,
        state: detail.state.clone(),
        review_decision: detail.review_decision.clone(),
        updated_at: detail.updated_at.clone(),
        url: detail.url.clone(),
    };
    let detail_key = pr_key(&repository, number);

    model
        .update(cx, |state, cx| {
            if !state
                .open_tabs
                .iter()
                .any(|tab| pr_key(&tab.repository, tab.number) == detail_key)
            {
                state.open_tabs.insert(0, summary);
            }

            state.set_active_section(SectionId::Pulls);
            state.active_surface = PullRequestSurface::Files;
            state.active_pr_key = Some(detail_key.clone());
            state.pr_header_compact = false;
            state.review_body.clear();
            state.review_editor_active = false;
            state.review_message = None;
            state.review_success = false;

            let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
            detail_state.snapshot = Some(snapshot.clone());
            detail_state.loading = false;
            detail_state.syncing = false;
            detail_state.error = None;

            state.apply_review_session_document(&detail_key, review_session.clone());
            cx.notify();
        })
        .ok();
}

fn parse_debug_pull_request_target(target: &str) -> Option<(String, i64)> {
    let (repository, number) = target.trim().rsplit_once('#')?;
    let number = number.parse::<i64>().ok()?;
    Some((repository.to_string(), number))
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let palette_open = state.palette_open;

        div()
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(bg_canvas())
            .text_color(fg_default())
            .text_size(px(14.0))
            .font_family(ui_font_family())
            .child(render_app_sidebar(&self.state, cx))
            .child(render_main_column(&self.state, cx))
            .when(palette_open, |el| el.child(render_palette(&self.state, cx)))
    }
}

fn render_app_sidebar(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let collapsed = s.app_sidebar_collapsed;
    let active_section = s.active_section;
    let is_authenticated = s.is_authenticated();
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let theme_preference = s.theme_preference;
    let sidebar_width = if collapsed {
        APP_SIDEBAR_COLLAPSED_WIDTH
    } else {
        APP_SIDEBAR_EXPANDED_WIDTH
    };
    let sync_label = if workspace_syncing {
        "Syncing workspace"
    } else {
        "Sync workspace"
    };
    let status_label = if workspace_syncing {
        "Syncing now"
    } else if workspace_error.is_some() {
        "Sync issue"
    } else if is_authenticated {
        "GitHub connected"
    } else {
        "gh needs auth"
    };
    let sync_color = if workspace_syncing {
        accent()
    } else if workspace_error.is_some() {
        danger()
    } else if is_authenticated {
        success()
    } else {
        fg_muted()
    };

    let state_for_nav = state.clone();
    let state_for_toggle = state.clone();
    let state_for_sync = state.clone();
    let state_for_theme = state.clone();

    div()
        .w(px(sidebar_width))
        .flex_shrink_0()
        .min_h_0()
        .bg(bg_surface())
        .border_r(px(1.0))
        .border_color(border_default())
        .child(
            div()
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .justify_between()
                .child(
                    div()
                        .p(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(if collapsed {
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(10.0))
                                .pb(px(8.0))
                                .child(
                                    img(APP_MARK_ASSET)
                                        .size(px(30.0))
                                        .object_fit(ObjectFit::Contain),
                                )
                                .child(sidebar_utility_button(SIDEBAR_EXPAND_ASSET, false, true, {
                                    let state = state_for_toggle.clone();
                                    move |_, _, cx| {
                                        state.update(cx, |state, cx| {
                                            state.app_sidebar_collapsed = false;
                                            cx.notify();
                                        });
                                    }
                                }))
                                .into_any_element()
                        } else {
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(10.0))
                                .pb(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .min_w_0()
                                        .child(
                                            img(APP_MARK_ASSET)
                                                .size(px(30.0))
                                                .object_fit(ObjectFit::Contain),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(1.0))
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .text_size(px(20.0))
                                                        .line_height(px(21.0))
                                                        .font_family(display_serif_font_family())
                                                        .font_weight(FontWeight::NORMAL)
                                                        .text_color(fg_emphasis())
                                                        .child(APP_NAME),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .font_family("Fira Code")
                                                        .text_color(ochre())
                                                        .text_ellipsis()
                                                        .whitespace_nowrap()
                                                        .overflow_x_hidden()
                                                        .child(APP_TAGLINE_LABEL),
                                                ),
                                        ),
                                )
                                .child(sidebar_utility_button(
                                    SIDEBAR_COLLAPSE_ASSET,
                                    false,
                                    true,
                                    {
                                        let state = state_for_toggle.clone();
                                        move |_, _, cx| {
                                            state.update(cx, |state, cx| {
                                                state.app_sidebar_collapsed = true;
                                                cx.notify();
                                            });
                                        }
                                    },
                                ))
                                .into_any_element()
                        })
                        .children(
                            SectionId::all()
                                .iter()
                                .filter(|section| **section != SectionId::Issues)
                                .map(|section| {
                                    let section = *section;
                                    let count = s.section_count(section);
                                    let state = state_for_nav.clone();
                                    sidebar_nav_button(
                                        section.label(),
                                        sidebar_icon_for_section(section),
                                        count,
                                        active_section == section,
                                        collapsed,
                                        move |_, window, cx| {
                                            if section == SectionId::Settings {
                                                prepare_settings_view(&state, window, cx);
                                            }
                                            state.update(cx, |s, cx| {
                                                s.set_active_section(section);
                                                s.active_pr_key = None;
                                                s.palette_open = false;
                                                s.palette_selected_index = 0;
                                                cx.notify();
                                            });
                                        },
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .p(px(10.0))
                        .pt(px(12.0))
                        .border_t(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .when(!collapsed, |el| {
                            el.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(7.0))
                                    .rounded(radius_sm())
                                    .bg(bg_overlay())
                                    .text_size(px(11.0))
                                    .font_family("Fira Code")
                                    .text_color(sync_color)
                                    .child(status_label),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .when(!collapsed, |el| {
                                    el.child(
                                        div()
                                            .px(px(6.0))
                                            .text_size(px(10.0))
                                            .font_family("Fira Code")
                                            .text_color(fg_subtle())
                                            .child("THEME"),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .flex_col()
                                        .when(!collapsed, |el| el.flex_row())
                                        .children(ThemePreference::all().iter().map(|candidate| {
                                            let candidate = *candidate;
                                            let state = state_for_theme.clone();
                                            sidebar_theme_button(
                                                theme_icon_asset(candidate),
                                                theme_preference == candidate,
                                                collapsed,
                                                move |_, window, cx| {
                                                    update_theme_preference(
                                                        &state, candidate, window, cx,
                                                    );
                                                },
                                            )
                                        })),
                                ),
                        )
                        .child(sidebar_action_button(
                            SIDEBAR_SYNC_ASSET,
                            sync_label,
                            collapsed,
                            sync_color,
                            move |_, window, cx| {
                                trigger_sync_workspace(&state_for_sync, window, cx)
                            },
                        )),
                ),
        )
}

fn render_main_column(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let has_tabs = !state.read(cx).open_tabs.is_empty();

    div()
        .flex_grow()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .when(has_tabs, |el| {
            el.child(render_workspace_tabs_strip(state, cx))
        })
        .child(render_workspace_body(state, cx))
}

fn render_workspace_tabs_strip(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let active_pr_key = s.active_pr_key.clone();
    let tabs: Vec<_> = s.open_tabs.clone();
    let state_for_tabs = state.clone();

    div()
        .bg(bg_surface())
        .border_b(px(1.0))
        .border_color(border_default())
        .flex_shrink_0()
        .child(
            div()
                .px(px(8.0))
                .pt(px(6.0))
                .flex()
                .items_end()
                .gap(px(4.0))
                .id("workspace-tabs-scroll")
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
                                s.set_active_section(SectionId::Pulls);
                                s.palette_open = false;
                                s.palette_selected_index = 0;
                                cx.notify();
                            });
                        },
                    )
                })),
        )
}

fn render_workspace_body(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
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

fn sidebar_icon_for_section(section: SectionId) -> &'static str {
    match section {
        SectionId::Overview => SIDEBAR_OVERVIEW_ASSET,
        SectionId::Pulls => SIDEBAR_PULLS_ASSET,
        SectionId::Reviews => SIDEBAR_REVIEWS_ASSET,
        SectionId::Settings => SIDEBAR_SETTINGS_ASSET,
        SectionId::Issues => SIDEBAR_OVERVIEW_ASSET,
    }
}

fn theme_icon_asset(preference: ThemePreference) -> &'static str {
    match preference {
        ThemePreference::System => SIDEBAR_SYSTEM_ASSET,
        ThemePreference::Light => SIDEBAR_LIGHT_ASSET,
        ThemePreference::Dark => SIDEBAR_DARK_ASSET,
    }
}

fn sidebar_nav_button(
    label: &str,
    icon_asset: &str,
    count: i64,
    active: bool,
    collapsed: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(38.0))
        .px(px(10.0))
        .when(collapsed, |el| el.px(px(0.0)))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            border_default()
        } else {
            transparent()
        })
        .when(active, |el| el.bg(bg_selected()))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .justify_center()
                .when(!collapsed, |el| el.justify_start())
                .flex_grow()
                .min_w_0()
                .child(
                    svg()
                        .path(icon_asset.to_string())
                        .size(px(18.0))
                        .text_color(if active { fg_emphasis() } else { fg_muted() }),
                )
                .when(!collapsed, |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if active { fg_emphasis() } else { fg_default() })
                            .child(label.to_string()),
                    )
                }),
        )
        .when(!collapsed && count > 0, |el| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .font_family("Fira Code")
                    .text_color(if active { fg_default() } else { fg_subtle() })
                    .child(count.to_string()),
            )
        })
        .when(collapsed, |el| el.justify_center())
}

fn sidebar_theme_button(
    icon_asset: &str,
    active: bool,
    collapsed: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(34.0))
        .when(collapsed, |el| el.w_full())
        .when(!collapsed, |el| el.flex_1())
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            border_default()
        } else {
            border_muted()
        })
        .bg(if active { bg_selected() } else { bg_overlay() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            svg()
                .path(icon_asset.to_string())
                .size(px(16.0))
                .text_color(if active { fg_emphasis() } else { fg_muted() }),
        )
}

fn sidebar_utility_button(
    icon_asset: &str,
    active: bool,
    bordered: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if bordered {
            border_muted()
        } else {
            transparent()
        })
        .bg(if active { bg_selected() } else { transparent() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            svg()
                .path(icon_asset.to_string())
                .size(px(16.0))
                .text_color(if active { fg_emphasis() } else { fg_muted() }),
        )
}

fn sidebar_action_button(
    icon_asset: &str,
    label: &str,
    collapsed: bool,
    icon_color: Rgba,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(36.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_overlay())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .when(!collapsed, |el| el.px(px(10.0)).justify_start())
        .when(collapsed, |el| el.w_full())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            svg()
                .path(icon_asset.to_string())
                .size(px(16.0))
                .text_color(icon_color),
        )
        .when(!collapsed, |el| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg_default())
                    .child(label.to_string()),
            )
        })
}

fn pr_tab(
    repository: &str,
    number: i64,
    title: &str,
    _additions: i64,
    _deletions: i64,
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
        .py(px(5.0))
        .rounded_t(radius_sm())
        .border_1()
        .border_color(if active {
            border_default()
        } else {
            border_muted()
        })
        .bg(if active { bg_canvas() } else { bg_surface() })
        .text_size(px(11.0))
        .max_w(px(280.0))
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
        .when_some(state_badge, |el, badge| el.child(badge))
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
