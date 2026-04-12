use gpui::prelude::*;
use gpui::*;

use crate::github;
use crate::state::*;
use crate::theme::*;

use super::tour_view::refresh_active_tour_flow;

pub fn render_section_workspace(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    match s.active_section {
        SectionId::Overview => render_overview(state, cx).into_any_element(),
        SectionId::Pulls | SectionId::Reviews => render_pull_list(state, cx).into_any_element(),
        SectionId::Issues => render_issues(state, cx).into_any_element(),
    }
}

fn render_overview(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let viewer_name = s.viewer_name().to_string();
    let is_auth = s.is_authenticated();
    let review_count = s.review_queue().map(|q| q.total_count).unwrap_or(0);
    let authored_count = s.authored_queue().map(|q| q.total_count).unwrap_or(0);
    let review_items: Vec<_> = s
        .review_queue()
        .map(|q| q.items.clone())
        .unwrap_or_default();
    let workspace_loading = s.workspace_loading;
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();

    let sync_state = state.clone();
    let state_for_items = state.clone();

    div()
        .p(px(40.0))
        .px(px(48.0))
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .id("overview-scroll")
        .overflow_y_scroll()
        .gap(px(24.0))
        .max_w(px(960.0))
        // Hero panel
        .child(
            panel().child(
                div()
                    .p(px(28.0))
                    .px(px(32.0))
                    .child(
                        div()
                            .text_size(px(24.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child(if is_auth {
                                format!("Welcome back, {viewer_name}")
                            } else {
                                "Connect GitHub through gh".to_string()
                            }),
                    )
                    // Stat cards
                    .child(
                        div()
                            .flex()
                            .gap(px(16.0))
                            .mt(px(24.0))
                            .child(stat_card(authored_count, "Open Pull Requests"))
                            .child(stat_card(review_count, "Review Requests")),
                    ),
            ),
        )
        // Recent PRs panel
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .mb(px(16.0))
                        .px(px(4.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Recent Pull Requests"),
                        )
                        .child(ghost_button(
                            if workspace_syncing {
                                "Syncing..."
                            } else {
                                "Sync workspace"
                            },
                            {
                                let state = sync_state.clone();
                                move |_, window, cx| trigger_sync_workspace(&state, window, cx)
                            },
                        )),
                )
                .when(workspace_loading, |el| {
                    el.child(panel_state_text("Loading workspace..."))
                })
                .when_some(workspace_error.clone(), |el, err| {
                    el.child(error_text(&err))
                })
                .when(
                    !workspace_loading && workspace_error.is_none() && review_items.is_empty(),
                    |el| {
                        el.child(panel_state_text(if is_auth {
                            "No PRs are currently requesting your review."
                        } else {
                            "No live review queue yet because gh is not authenticated."
                        }))
                    },
                )
                .child(div().flex().flex_col().gap(px(8.0)).children(
                    review_items.into_iter().map(|item| {
                        let state = state_for_items.clone();
                        pr_list_row(item, move |summary, window, cx| {
                            open_pull_request(&state, summary, window, cx);
                        })
                    }),
                )),
        )
}

fn render_pull_list(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let is_reviews = s.active_section == SectionId::Reviews;
    let workspace_loading = s.workspace_loading;
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let is_auth = s.is_authenticated();

    let available_queues: Vec<_> = if is_reviews {
        s.workspace
            .as_ref()
            .map(|w| {
                w.queues
                    .iter()
                    .filter(|q| q.id == "reviewRequested")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    } else {
        s.workspace
            .as_ref()
            .map(|w| w.queues.clone())
            .unwrap_or_default()
    };

    let current_queue = if is_reviews {
        available_queues.first().cloned()
    } else {
        available_queues
            .iter()
            .find(|q| q.id == s.active_queue_id)
            .or(available_queues.first())
            .cloned()
    };

    let queue_items: Vec<_> = current_queue
        .as_ref()
        .map(|q| q.items.clone())
        .unwrap_or_default();
    let queue_label = current_queue
        .as_ref()
        .map(|q| q.label.clone())
        .unwrap_or_else(|| "Pull Requests".to_string());
    let loaded_from_cache = s
        .workspace
        .as_ref()
        .map(|w| w.loaded_from_cache)
        .unwrap_or(false);

    let sync_state = state.clone();
    let state_for_items = state.clone();

    div()
        .flex()
        .min_h_0()
        .flex_grow()
        // Sidebar
        .child(
            div()
                .w(sidebar_width())
                .bg(bg_surface())
                .p(px(28.0))
                .px(px(32.0))
                .flex()
                .flex_col()
                .flex_shrink_0()
                .min_h_0()
                .id("pull-sidebar-scroll")
                .overflow_y_scroll()
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(if is_reviews {
                            "Reviews"
                        } else {
                            "Pull Requests"
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .mt(px(6.0))
                        .max_w(px(200.0))
                        .child(if is_reviews {
                            "Focused queue over pull requests that need your review."
                        } else {
                            "Queue filters backed by gh search plus a local cache."
                        }),
                )
                .child(div().flex().flex_col().gap(px(4.0)).mt(px(20.0)).children(
                    available_queues.iter().map(|queue| {
                        let is_active = current_queue
                            .as_ref()
                            .map(|c| c.id == queue.id)
                            .unwrap_or(false);
                        let queue_id = queue.id.clone();
                        let state = state.clone();
                        filter_pill(
                            &queue.label,
                            queue.total_count,
                            is_active,
                            move |_, _, cx| {
                                state.update(cx, |s, cx| {
                                    s.active_queue_id = queue_id.clone();
                                    cx.notify();
                                });
                            },
                        )
                    }),
                )),
        )
        // Main content
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .p(px(24.0))
                .px(px(28.0))
                .flex()
                .flex_col()
                .id("pull-list-scroll")
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .mb(px(16.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(eyebrow(if loaded_from_cache {
                                    "Showing cached queue data"
                                } else {
                                    "Showing live queue data"
                                }))
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(if is_reviews {
                                            "Review Queue".to_string()
                                        } else {
                                            queue_label
                                        }),
                                ),
                        )
                        .child(ghost_button(
                            if workspace_syncing {
                                "Syncing..."
                            } else {
                                "Refresh"
                            },
                            {
                                let state = sync_state.clone();
                                move |_, window, cx| trigger_sync_workspace(&state, window, cx)
                            },
                        )),
                )
                .when(workspace_loading, |el| {
                    el.child(panel_state_text("Loading queue..."))
                })
                .when_some(workspace_error, |el, err| el.child(error_text(&err)))
                .when(!workspace_loading && queue_items.is_empty(), |el| {
                    el.child(panel_state_text(if is_auth {
                        "No pull requests matched this queue."
                    } else {
                        "Authenticate with gh to load live pull request queues."
                    }))
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children(queue_items.into_iter().map(|item| {
                            let state = state_for_items.clone();
                            pr_list_row(item, move |summary, window, cx| {
                                open_pull_request(&state, summary, window, cx);
                            })
                        })),
                ),
        )
}

fn render_issues(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);

    div()
        .p(px(40.0))
        .px(px(48.0))
        .flex_grow()
        .min_h_0()
        .id("issues-scroll")
        .overflow_y_scroll()
        .max_w(px(960.0))
        .child(
            panel().child(
                div()
                    .p(px(28.0))
                    .px(px(32.0))
                    .child(eyebrow("Deferred"))
                    .child(
                        div()
                            .text_size(px(24.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Issues"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .mt(px(6.0))
                            .max_w(px(480.0))
                            .child("Issues remain intentionally secondary while the MVP concentrates on review flow, PR detail, and write actions."),
                    )
                    .child(
                        nested_panel()
                            .mt(px(16.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child("Backend status"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.0))
                                    .mt(px(12.0))
                                    .child(meta_row(
                                        "gh",
                                        if s.gh_available {
                                            "available"
                                        } else {
                                            "missing"
                                        },
                                    ))
                                    .child(meta_row("Cache", &s.cache_path)),
                            ),
                    ),
            ),
        )
}

// --- Shared components ---

pub fn panel() -> Div {
    div().rounded(radius()).bg(bg_surface()).overflow_hidden()
}

pub fn nested_panel() -> Div {
    div().p(px(20.0)).rounded(radius()).bg(bg_overlay())
}

pub fn eyebrow(text: &str) -> impl IntoElement {
    div()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg_subtle())
        .font_family("Fira Code")
        .mb(px(8.0))
        .child(text.to_string().to_uppercase())
}

pub fn ghost_button(
    label: &str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .text_color(fg_muted())
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
}

pub fn review_button(
    label: &str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(bg_selected())
        .text_color(fg_emphasis())
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
}

pub fn badge(text: &str) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(16.0))
        .bg(bg_subtle())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg_muted())
        .child(text.to_string())
}

pub fn badge_success(text: &str) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(16.0))
        .bg(success_muted())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(success())
        .child(text.to_string())
}

pub fn panel_state_text(text: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(fg_muted())
        .child(text.to_string())
}

pub fn error_text(text: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(danger())
        .child(text.to_string())
}

pub fn success_text(text: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(success())
        .child(text.to_string())
}

pub fn meta_row(label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.0))
        .justify_between()
        .text_size(px(13.0))
        .child(div().text_color(fg_muted()).child(label.to_string()))
        .child(
            div()
                .text_color(fg_emphasis())
                .font_weight(FontWeight::MEDIUM)
                .font_family("Fira Code")
                .text_size(px(11.0))
                .child(value.to_string()),
        )
}

fn stat_card(count: i64, label: &str) -> impl IntoElement {
    let abbrev: String = label
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();

    div()
        .flex()
        .items_center()
        .gap(px(16.0))
        .p(px(20.0))
        .px(px(24.0))
        .rounded(radius())
        .bg(bg_overlay())
        .flex_1()
        .child(
            div()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(radius())
                .bg(bg_emphasis())
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(fg_default())
                .text_size(px(14.0))
                .font_weight(FontWeight::BOLD)
                .font_family("Fira Code")
                .child(abbrev),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(24.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .font_family("Fira Code")
                        .child(count.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(label.to_string()),
                ),
        )
}

fn filter_pill(
    label: &str,
    count: i64,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .items_center()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(active, |el| el.bg(bg_selected()).text_color(fg_emphasis()))
        .when(!active, |el| el.text_color(fg_muted()))
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
        .child(
            div()
                .text_color(if active { fg_default() } else { fg_subtle() })
                .font_family("Fira Code")
                .text_size(px(12.0))
                .child(count.to_string()),
        )
}

fn pr_list_row(
    item: github::PullRequestSummary,
    on_click: impl Fn(github::PullRequestSummary, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let dot_color = match item.state.as_str() {
        "MERGED" => purple(),
        "CLOSED" => danger(),
        _ => success(),
    };
    let title = item.title.clone();
    let meta = format!(
        "{} #{} \u{00b7} {} \u{00b7} {}",
        item.repository,
        item.number,
        item.author_login,
        format_relative_time(&item.updated_at)
    );
    let comments = item.comments_count;
    let summary = item.clone();

    div()
        .flex()
        .gap(px(12.0))
        .items_center()
        .justify_between()
        .px(px(20.0))
        .py(px(14.0))
        .rounded(radius())
        .bg(bg_surface())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(summary.clone(), window, cx)
        })
        // Status dot
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .rounded(px(4.0))
                .bg(dot_color)
                .flex_shrink_0(),
        )
        // Body
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_emphasis())
                        .text_size(px(14.0))
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .mt(px(4.0))
                        .child(meta),
                ),
        )
        // Trailing
        .child(
            div()
                .flex()
                .gap(px(5.0))
                .items_center()
                .text_color(fg_muted())
                .text_size(px(13.0))
                .flex_shrink_0()
                .child(comments.to_string()),
        )
}

pub fn open_pull_request(
    state: &Entity<AppState>,
    summary: github::PullRequestSummary,
    window: &mut Window,
    cx: &mut App,
) {
    let key = pr_key(&summary.repository, summary.number);
    let repository = summary.repository.clone();
    let number = summary.number;

    state.update(cx, |s, cx| {
        if !s
            .open_tabs
            .iter()
            .any(|t| pr_key(&t.repository, t.number) == key)
        {
            s.open_tabs.insert(0, summary);
        }
        s.active_section = SectionId::Pulls;
        s.active_surface = PullRequestSurface::Overview;
        s.active_pr_key = Some(key.clone());
        s.palette_open = false;
        s.selected_diff_anchor = None;
        s.review_body.clear();
        s.review_message = None;
        s.review_success = false;
        s.active_tour_outline_id = "overview".to_string();
        s.collapsed_tour_panels.clear();

        // Start loading if we don't have data
        if !s.detail_states.contains_key(&key) {
            s.detail_states.insert(
                key.clone(),
                DetailState {
                    loading: true,
                    ..Default::default()
                },
            );
        }
        cx.notify();
    });

    // Load PR detail in background
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };

            // Load from cache first
            let cached_result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let repository = repository.clone();
                    async move { github::load_pull_request_detail(&cache, &repository, number) }
                })
                .await;

            let detail_key = pr_key(&repository, number);

            model
                .update(cx, |s, cx| {
                    if let Ok(snapshot) = &cached_result {
                        let ds = s.detail_states.entry(detail_key.clone()).or_default();
                        ds.snapshot = Some(snapshot.clone());
                        ds.loading = false;
                    }
                    cx.notify();
                })
                .ok();

            let should_refresh_tour = model
                .read_with(cx, |s, _| {
                    s.active_surface == PullRequestSurface::Tour
                        && s.active_pr_key.as_deref() == Some(&detail_key)
                })
                .ok()
                .unwrap_or(false);

            if should_refresh_tour {
                refresh_active_tour_flow(model.clone(), true, cx).await;
            }

            // Then sync from GitHub
            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.syncing = true;
                    cx.notify();
                })
                .ok();

            let sync_result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let repository = repository.clone();
                    async move { github::sync_pull_request_detail(&cache, &repository, number) }
                })
                .await;

            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.syncing = false;
                    match sync_result {
                        Ok(snapshot) => {
                            ds.snapshot = Some(snapshot);
                            ds.error = None;
                        }
                        Err(e) => {
                            ds.error = Some(e);
                        }
                    }
                    // Set default selected file
                    if s.selected_file_path.is_none() {
                        if let Some(detail) = ds.snapshot.as_ref().and_then(|sn| sn.detail.as_ref())
                        {
                            s.selected_file_path = detail
                                .files
                                .first()
                                .map(|f| f.path.clone())
                                .or_else(|| detail.parsed_diff.first().map(|f| f.path.clone()));
                        }
                    }
                    cx.notify();
                })
                .ok();

            let should_refresh_tour = model
                .read_with(cx, |s, _| {
                    s.active_surface == PullRequestSurface::Tour
                        && s.active_pr_key.as_deref() == Some(&detail_key)
                })
                .ok()
                .unwrap_or(false);

            if should_refresh_tour {
                refresh_active_tour_flow(model.clone(), true, cx).await;
            }
        })
        .detach();
}

pub fn trigger_sync_workspace(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let model = state.clone();

    state.update(cx, |s, cx| {
        s.workspace_syncing = true;
        cx.notify();
    });

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };

            let result = cx
                .background_executor()
                .spawn(async move { github::sync_workspace_snapshot(&cache) })
                .await;

            model
                .update(cx, |s, cx| {
                    s.workspace_syncing = false;
                    match result {
                        Ok(ws) => {
                            s.gh_available = ws.auth.is_authenticated;
                            s.workspace = Some(ws);
                            s.workspace_error = None;
                        }
                        Err(e) => {
                            s.workspace_error = Some(e);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

fn format_relative_time(value: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(ts) = parse_iso_timestamp(value) {
        let diff = now.saturating_sub(ts);
        let minutes = diff / 60;
        let hours = diff / 3600;
        let days = diff / 86400;

        if minutes < 1 {
            return "just now".to_string();
        }
        if minutes < 60 {
            return format!("{minutes}m ago");
        }
        if hours < 24 {
            return format!("{hours}h ago");
        }
        if days < 30 {
            return format!("{days}d ago");
        }
    }

    if value.len() > 10 {
        value[..10].to_string()
    } else {
        value.to_string()
    }
}

fn parse_iso_timestamp(value: &str) -> Option<u64> {
    let parts: Vec<&str> = value.split('T').collect();
    if parts.len() < 2 {
        return None;
    }
    let date_parts: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let time_str = parts[1].trim_end_matches('Z');
    let time_parts: Vec<u64> = time_str.split(':').filter_map(|p| p.parse().ok()).collect();
    if time_parts.len() < 2 {
        return None;
    }

    let year = date_parts[0];
    let month = date_parts[1];
    let day = date_parts[2];
    let hour = time_parts[0];
    let minute = time_parts[1];
    let second = if time_parts.len() > 2 {
        time_parts[2]
    } else {
        0
    };

    let mut days_total: u64 = 0;
    for y in 1970..year {
        days_total += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    for m in 1..month {
        days_total += month_days[m as usize];
        if m == 2 && is_leap {
            days_total += 1;
        }
    }
    days_total += day - 1;

    Some(days_total * 86400 + hour * 3600 + minute * 60 + second)
}
