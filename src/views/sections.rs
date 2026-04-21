use gpui::prelude::*;
use gpui::*;

use crate::app_assets::{
    OVERVIEW_MY_PULL_REQUESTS_ASSET, OVERVIEW_OPEN_PULL_REQUESTS_ASSET,
    OVERVIEW_REVIEW_REQUESTS_ASSET,
};
use crate::github;
use crate::review_queue::default_review_file;
use crate::review_session::load_review_session;
use crate::state::*;
use crate::theme::*;

use super::settings::render_settings_view;
use super::workspace_sync::trigger_sync_workspace;
use std::collections::BTreeMap;

const DETAIL_AUTO_REFRESH_TTL_MS: i64 = 5 * 60 * 1000;

pub fn render_section_workspace(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    match s.active_section {
        SectionId::Overview => render_overview(state, cx).into_any_element(),
        SectionId::Pulls | SectionId::Reviews => render_pull_list(state, cx).into_any_element(),
        SectionId::Issues => render_issues(state, cx).into_any_element(),
        SectionId::Settings => render_settings_view(state, cx).into_any_element(),
    }
}

fn render_overview(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let viewer_name = s.viewer_name().to_string();
    let is_auth = s.is_authenticated();
    let review_count = s.review_queue().map(|q| q.total_count).unwrap_or(0);
    let authored_count = s.authored_queue().map(|q| q.total_count).unwrap_or(0);
    let open_tab_count = s.open_tabs.len() as i64;
    let review_items: Vec<_> = s
        .review_queue()
        .map(|q| q.items.clone())
        .unwrap_or_default();
    let workspace_loading = s.workspace_loading;
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let first_open_tab = s.open_tabs.first().cloned();

    let summary_title = if is_auth {
        format!("Welcome back, {viewer_name}")
    } else {
        "Connect GitHub".to_string()
    };
    let queue_copy = if workspace_loading {
        "Loading the latest review requests from the workspace snapshot.".to_string()
    } else if workspace_error.is_some() {
        "Workspace sync needs attention before the queue can refresh.".to_string()
    } else if is_auth {
        "Pull requests currently waiting on your attention.".to_string()
    } else {
        "Authenticate with gh to populate the review queue.".to_string()
    };

    let sync_state = state.clone();
    let state_for_open_pull_requests = state.clone();
    let state_for_authored = state.clone();
    let state_for_review_requests = state.clone();
    let state_for_items = state.clone();
    let state_for_review_board = state.clone();

    div()
        .p(px(28.0))
        .px(px(40.0))
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .id("overview-scroll")
        .overflow_y_scroll()
        .child(
            div().w_full().flex().justify_center().child(
                div()
                    .w_full()
                    .min_w_0()
                    .max_w(px(1040.0))
                    .flex()
                    .flex_col()
                    .gap(px(24.0))
                    .child(
                        div().w_full().min_w_0().child(
                            div()
                                .text_size(px(40.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(summary_title),
                        ),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_wrap()
                            .gap(px(12.0))
                            .child(stat_card(
                                OVERVIEW_OPEN_PULL_REQUESTS_ASSET,
                                "Open Pull Requests",
                                open_tab_count,
                                first_open_tab.is_some(),
                                {
                                    let state = state_for_open_pull_requests.clone();
                                    let summary = first_open_tab.clone();
                                    move |_, window, cx| {
                                        if let Some(summary) = summary.clone() {
                                            open_pull_request(&state, summary, window, cx);
                                        }
                                    }
                                },
                            ))
                            .child(stat_card(
                                OVERVIEW_MY_PULL_REQUESTS_ASSET,
                                "My Pull Requests",
                                authored_count,
                                is_auth,
                                {
                                    let state = state_for_authored.clone();
                                    move |_, _, cx| {
                                        activate_queue(&state, SectionId::Pulls, "authored", cx);
                                    }
                                },
                            ))
                            .child(stat_card(
                                OVERVIEW_REVIEW_REQUESTS_ASSET,
                                "Review Requests",
                                review_count,
                                is_auth,
                                {
                                    let state = state_for_review_requests.clone();
                                    move |_, _, cx| {
                                        activate_queue(
                                            &state,
                                            SectionId::Reviews,
                                            "reviewRequested",
                                            cx,
                                        );
                                    }
                                },
                            )),
                    )
                    .child(
                        panel().child(
                            div()
                                .p(px(20.0))
                                .px(px(24.0))
                                .flex()
                                .flex_col()
                                .gap(px(16.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_start()
                                        .justify_between()
                                        .gap(px(16.0))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(4.0))
                                                .min_w_0()
                                                .child(eyebrow("Needs your attention"))
                                                .child(
                                                    div()
                                                        .text_size(px(16.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(fg_emphasis())
                                                        .child("Recent Pull Requests"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(fg_muted())
                                                        .max_w(px(720.0))
                                                        .child(queue_copy),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .flex_wrap()
                                                .when(is_auth, |el| {
                                                    el.child(review_button("Open review board", {
                                                        let state = state_for_review_board.clone();
                                                        move |_, _, cx| {
                                                            activate_queue(
                                                                &state,
                                                                SectionId::Reviews,
                                                                "reviewRequested",
                                                                cx,
                                                            );
                                                        }
                                                    }))
                                                })
                                                .child(ghost_button(
                                                    if workspace_syncing {
                                                        "Syncing..."
                                                    } else {
                                                        "Sync workspace"
                                                    },
                                                    {
                                                        let state = sync_state.clone();
                                                        move |_, window, cx| {
                                                            trigger_sync_workspace(
                                                                &state, window, cx,
                                                            )
                                                        }
                                                    },
                                                )),
                                        ),
                                )
                                .when(workspace_loading, |el| {
                                    el.child(panel_state_text("Loading review requests..."))
                                })
                                .when_some(workspace_error.clone(), |el, err| {
                                    el.child(error_text(&err))
                                })
                                .when(
                                    !workspace_loading
                                        && workspace_error.is_none()
                                        && review_items.is_empty(),
                                    |el| {
                                        el.child(panel_state_text(if is_auth {
                                            "No pull requests are currently requesting your review."
                                        } else {
                                            "Authenticate with gh to populate the review queue."
                                        }))
                                    },
                                )
                                .child(div().flex().flex_col().gap(px(10.0)).children(
                                    review_items.into_iter().map(|item| {
                                        let state = state_for_items.clone();
                                        pr_list_row(item, move |summary, window, cx| {
                                            open_pull_request(&state, summary, window, cx);
                                        })
                                    }),
                                )),
                        ),
                    ),
            ),
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
    let state_for_lanes = state.clone();

    // Viewer login for mine/others split
    let viewer_login = s
        .workspace
        .as_ref()
        .and_then(|w| w.viewer.as_ref())
        .map(|v| v.login.clone())
        .unwrap_or_default();
    let muted_repos = s.muted_repos.clone();
    let is_authored_queue = current_queue
        .as_ref()
        .map(|q| q.id == "authored")
        .unwrap_or(false);

    // Group items into kanban lanes by repository
    let mut my_items: Vec<github::PullRequestSummary> = Vec::new();
    let mut repo_groups: BTreeMap<String, Vec<github::PullRequestSummary>> = BTreeMap::new();
    for item in &queue_items {
        if muted_repos.contains(&item.repository) {
            continue;
        }
        if !is_authored_queue && !viewer_login.is_empty() && item.author_login == viewer_login {
            my_items.push(item.clone());
        } else {
            repo_groups
                .entry(item.repository.clone())
                .or_default()
                .push(item.clone());
        }
    }

    let has_my_items = !my_items.is_empty();
    let has_any_lanes = has_my_items || !repo_groups.is_empty();
    let muted_list: Vec<String> = muted_repos.iter().cloned().collect::<Vec<_>>();
    let has_muted = !muted_list.is_empty();

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
                            "Review requests grouped by repository."
                        } else {
                            "Pull requests grouped into repo lanes."
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
                ))
                .when(has_muted, |el| {
                    el.child(
                        div()
                            .mt(px(24.0))
                            .flex()
                            .flex_col()
                            .child(eyebrow("Muted Repos"))
                            .child(div().flex().flex_col().gap(px(4.0)).children(
                                muted_list.into_iter().map(|repo| {
                                    let state = state.clone();
                                    let repo_for_unmute = repo.clone();
                                    muted_repo_pill(&repo, move |_, _, cx| {
                                        let r = repo_for_unmute.clone();
                                        state.update(cx, |s, cx| {
                                            s.muted_repos.remove(&r);
                                            cx.notify();
                                        });
                                    })
                                }),
                            )),
                    )
                }),
        )
        // Kanban board
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                // Board header
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(28.0))
                        .pt(px(24.0))
                        .pb(px(16.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(eyebrow(if loaded_from_cache {
                                    "Cached data"
                                } else {
                                    "Live data"
                                }))
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(if is_reviews {
                                            "Review Board".to_string()
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
                    el.child(
                        div()
                            .px(px(28.0))
                            .child(panel_state_text("Loading queue...")),
                    )
                })
                .when_some(workspace_error, |el, err| {
                    el.child(div().px(px(28.0)).child(error_text(&err)))
                })
                .when(!workspace_loading && !has_any_lanes, |el| {
                    el.child(div().px(px(28.0)).child(panel_state_text(if has_muted {
                        "All repositories in this queue are muted."
                    } else if is_auth {
                        "No pull requests matched this queue."
                    } else {
                        "Authenticate with gh to load live pull request queues."
                    })))
                })
                // Swim lanes
                .child(
                    div()
                        .flex_grow()
                        .min_h_0()
                        .id("kanban-board-hscroll")
                        .overflow_x_scroll()
                        .overflow_y_hidden()
                        .px(px(20.0))
                        .pb(px(20.0))
                        .child(
                            div()
                                .flex()
                                .gap(px(12.0))
                                .h_full()
                                .when(has_my_items, |el| {
                                    let state = state_for_lanes.clone();
                                    el.child(kanban_lane(
                                        "__mine__",
                                        "My Pull Requests",
                                        &format!("{} open", my_items.len()),
                                        my_items,
                                        accent(),
                                        true,
                                        state,
                                    ))
                                })
                                .children(repo_groups.into_iter().map(|(repo, items)| {
                                    let short_name =
                                        repo.split('/').last().unwrap_or(&repo).to_string();
                                    let count = items.len();
                                    let accent_color = lane_accent_color(&repo);
                                    let state = state_for_lanes.clone();
                                    kanban_lane(
                                        &repo,
                                        &short_name,
                                        &format!("{repo} \u{00b7} {count}"),
                                        items,
                                        accent_color,
                                        false,
                                        state,
                                    )
                                })),
                        ),
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
    div()
        .rounded(radius())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
        .overflow_hidden()
}

pub fn nested_panel() -> Div {
    div()
        .p(px(18.0))
        .rounded(radius())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
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
        .rounded(px(999.0))
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
        .rounded(px(999.0))
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
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .w(px(88.0))
                .flex_shrink_0()
                .text_color(fg_subtle())
                .font_family("Fira Code")
                .text_size(px(10.0))
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .px(px(10.0))
                .py(px(8.0))
                .rounded(radius_sm())
                .bg(bg_inset())
                .border_1()
                .border_color(border_muted())
                .text_color(fg_emphasis())
                .font_weight(FontWeight::MEDIUM)
                .font_family("Fira Code")
                .text_size(px(11.0))
                .whitespace_normal()
                .child(value.to_string()),
        )
}

fn stat_card(
    icon_asset: &str,
    label: &str,
    count: i64,
    interactive: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_1()
        .min_w(px(240.0))
        .items_center()
        .gap(px(14.0))
        .p(px(16.0))
        .px(px(18.0))
        .min_h(px(84.0))
        .rounded(px(18.0))
        .bg(bg_overlay())
        .when(interactive, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    svg()
                        .path(icon_asset.to_string())
                        .size(px(22.0))
                        .text_color(fg_muted()),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .min_w_0()
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(count.to_string()),
                )
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_default())
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

fn pill_badge(label: &str, fg: Rgba, bg: Rgba, _border: Rgba) -> impl IntoElement {
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

fn subtle_pill(label: &str) -> impl IntoElement {
    pill_badge(label, fg_muted(), bg_emphasis(), border_muted())
}

fn pull_request_state_badge(item: &github::PullRequestSummary) -> AnyElement {
    if item.is_draft {
        return pill_badge("Draft", fg_muted(), bg_emphasis(), border_muted()).into_any_element();
    }

    match item.state.as_str() {
        "MERGED" => pill_badge("Merged", purple(), bg_emphasis(), purple()).into_any_element(),
        "CLOSED" => {
            pill_badge("Closed", danger(), danger_muted(), diff_remove_border()).into_any_element()
        }
        _ => pill_badge("Open", success(), success_muted(), diff_add_border()).into_any_element(),
    }
}

fn review_decision_badge(decision: &str) -> AnyElement {
    match decision {
        "APPROVED" => {
            pill_badge("Approved", success(), success_muted(), diff_add_border()).into_any_element()
        }
        "CHANGES_REQUESTED" => {
            pill_badge("Changes", danger(), danger_muted(), diff_remove_border()).into_any_element()
        }
        "REVIEW_REQUIRED" => {
            pill_badge("Needs review", fg_muted(), bg_emphasis(), border_muted()).into_any_element()
        }
        "COMMENTED" => {
            pill_badge("Commented", accent(), accent_muted(), accent()).into_any_element()
        }
        _ => subtle_pill(decision).into_any_element(),
    }
}

fn render_diff_summary(additions: i64, deletions: i64) -> impl IntoElement {
    let additions = additions.max(0);
    let deletions = deletions.max(0);
    let total = additions + deletions;
    let segments = 8usize;
    let add_segments = if total > 0 {
        ((additions as f32 / total as f32) * segments as f32)
            .round()
            .clamp(0.0, segments as f32) as usize
    } else {
        0
    };
    let delete_segments = if total > 0 {
        segments.saturating_sub(add_segments)
    } else {
        0
    };

    div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .gap(px(4.0))
                .text_size(px(11.0))
                .font_family("Fira Code")
                .child(div().text_color(success()).child(format!("+{additions}")))
                .child(div().text_color(danger()).child(format!("-{deletions}"))),
        )
        .child(
            div()
                .flex()
                .gap(px(2.0))
                .children((0..segments).map(move |ix| {
                    let bg = if ix < add_segments {
                        success()
                    } else if ix < add_segments + delete_segments {
                        danger()
                    } else {
                        border_muted()
                    };

                    div().w(px(8.0)).h(px(4.0)).rounded(px(2.0)).bg(bg)
                })),
        )
}

fn pr_list_row(
    item: github::PullRequestSummary,
    on_click: impl Fn(github::PullRequestSummary, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = item.title.clone();
    let repo_ref = format!("{} #{}", item.repository, item.number);
    let meta = format!(
        "{} \u{2022} updated {}",
        item.author_login,
        format_relative_time(&item.updated_at)
    );
    let comments = item.comments_count;
    let changed_files = item.changed_files;
    let additions = item.additions;
    let deletions = item.deletions;
    let review_decision = item.review_decision.clone();
    let summary = item.clone();

    div()
        .w_full()
        .p(px(14.0))
        .rounded(radius_sm())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(summary.clone(), window, cx)
        })
        .child(
            div()
                .flex_grow()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(16.0))
                .child(
                    div()
                        .flex_grow()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_family("Fira Code")
                                .text_color(fg_subtle())
                                .child(repo_ref),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .text_size(px(14.0))
                                .line_clamp(2)
                                .child(title),
                        )
                        .child(div().text_size(px(12.0)).text_color(fg_muted()).child(meta))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .flex_wrap()
                                .child(pull_request_state_badge(&item))
                                .when_some(review_decision, |el, decision| {
                                    el.child(review_decision_badge(&decision))
                                })
                                .when(comments > 0, |el| {
                                    el.child(subtle_pill(&format!("{comments} comments")))
                                })
                                .child(subtle_pill(&format!("{changed_files} files"))),
                        ),
                )
                .child(render_diff_summary(additions, deletions)),
        )
}

fn kanban_lane(
    lane_id: &str,
    label: &str,
    subtitle: &str,
    items: Vec<github::PullRequestSummary>,
    accent: Rgba,
    is_mine: bool,
    state: Entity<AppState>,
) -> impl IntoElement {
    let label = label.to_string();
    let subtitle = subtitle.to_string();
    let count = items.len();
    let mute_state = state.clone();
    let mute_repo = lane_id.to_string();

    div()
        .w(px(286.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .min_h_0()
        .child(
            div()
                .flex()
                .flex_col()
                .min_h_0()
                .flex_grow()
                .rounded(radius())
                .bg(bg_surface())
                .border_1()
                .border_color(border_muted())
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .p(px(14.0))
                        .pb(px(8.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(div().w(px(6.0)).h(px(24.0)).rounded(radius_sm()).bg(accent))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .rounded(px(10.0))
                                        .bg(bg_emphasis())
                                        .text_size(px(11.0))
                                        .font_family("Fira Code")
                                        .text_color(fg_muted())
                                        .child(count.to_string()),
                                ),
                        )
                        .when(!is_mine, |el| {
                            el.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(radius_sm())
                                    .text_size(px(11.0))
                                    .text_color(fg_subtle())
                                    .cursor_pointer()
                                    .hover(|s| s.bg(hover_bg()).text_color(danger()))
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        mute_state.update(cx, |s, cx| {
                                            s.muted_repos.insert(mute_repo.clone());
                                            cx.notify();
                                        });
                                    })
                                    .child("Mute"),
                            )
                        }),
                )
                .child(
                    div()
                        .px(px(14.0))
                        .pb(px(10.0))
                        .text_size(px(11.0))
                        .text_color(fg_subtle())
                        .font_family("Fira Code")
                        .child(subtitle),
                )
                .child(
                    div()
                        .flex_grow()
                        .min_h_0()
                        .id(SharedString::from(format!("lane-scroll-{lane_id}")))
                        .overflow_y_scroll()
                        .px(px(8.0))
                        .pb(px(8.0))
                        .child(div().flex().flex_col().gap(px(4.0)).children(
                            items.into_iter().map(|item| {
                                let state = state.clone();
                                kanban_card(item, move |summary, window, cx| {
                                    open_pull_request(&state, summary, window, cx);
                                })
                            }),
                        )),
                ),
        )
}

fn kanban_card(
    item: github::PullRequestSummary,
    on_click: impl Fn(github::PullRequestSummary, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = item.title.clone();
    let repo_label = item
        .repository
        .split('/')
        .last()
        .unwrap_or(&item.repository)
        .to_string();
    let meta = format!(
        "{} #{} \u{00b7} {} \u{00b7} {}",
        repo_label,
        item.number,
        item.author_login,
        format_relative_time(&item.updated_at)
    );
    let additions = item.additions;
    let deletions = item.deletions;
    let comments = item.comments_count;
    let changed_files = item.changed_files;
    let review_decision = item.review_decision.clone();
    let summary = item.clone();

    div()
        .p(px(12.0))
        .rounded(radius_sm())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|s| s.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(summary.clone(), window, cx)
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_grow()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .line_clamp(2)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .font_family("Fira Code")
                                        .text_color(fg_muted())
                                        .child(meta),
                                ),
                        )
                        .child(render_diff_summary(additions, deletions)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(pull_request_state_badge(&item))
                        .when_some(review_decision, |el, decision| {
                            el.child(review_decision_badge(&decision))
                        })
                        .when(comments > 0, |el| {
                            el.child(subtle_pill(&format!("{comments} comments")))
                        })
                        .child(subtle_pill(&format!("{changed_files} files"))),
                ),
        )
}

fn muted_repo_pill(
    repo: &str,
    on_unmute: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let short_name = repo.split('/').last().unwrap_or(repo).to_string();

    div()
        .flex()
        .justify_between()
        .items_center()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .text_size(px(12.0))
        .text_color(fg_subtle())
        .child(
            div()
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_x_hidden()
                .child(short_name),
        )
        .child(
            div()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(radius_sm())
                .text_size(px(11.0))
                .text_color(fg_subtle())
                .cursor_pointer()
                .hover(|s| s.bg(hover_bg()).text_color(success()))
                .on_mouse_down(MouseButton::Left, on_unmute)
                .child("Unmute"),
        )
}

fn activate_queue(state: &Entity<AppState>, section: SectionId, queue_id: &str, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.active_section = section;
        s.active_surface = PullRequestSurface::Overview;
        s.active_queue_id = queue_id.to_string();
        s.active_pr_key = None;
        s.palette_open = false;
        s.palette_selected_index = 0;
        s.pr_header_compact = false;
        cx.notify();
    });
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
    let cached_review_session = {
        let cache = state.read(cx).cache.clone();
        load_review_session(cache.as_ref(), &key).ok().flatten()
    };
    let load_plan = {
        let s = state.read(cx);
        plan_pull_request_open(&s, &key)
    };

    state.update(cx, |s, cx| {
        if !s
            .open_tabs
            .iter()
            .any(|t| pr_key(&t.repository, t.number) == key)
        {
            s.open_tabs.insert(0, summary);
        }
        s.active_section = SectionId::Pulls;
        s.active_surface = PullRequestSurface::Files;
        s.active_pr_key = Some(key.clone());
        s.palette_open = false;
        s.palette_selected_index = 0;
        s.review_body.clear();
        s.review_editor_active = false;
        s.review_message = None;
        s.review_success = false;
        s.pr_header_compact = false;
        s.active_tour_outline_id = "overview".to_string();
        s.collapsed_tour_panels.clear();

        s.detail_states.entry(key.clone()).or_default();
        s.apply_review_session_document(&key, cached_review_session.clone());
        let detail_state = s.detail_states.entry(key.clone()).or_default();
        detail_state.loading = load_plan.show_loading;
        if load_plan.load_cached_snapshot || load_plan.sync_live {
            detail_state.error = None;
        }
        cx.notify();
    });

    if !load_plan.load_cached_snapshot && !load_plan.sync_live {
        return;
    }

    // Load PR detail in background
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let detail_key = pr_key(&repository, number);
            let mut should_sync = load_plan.sync_live;

            if load_plan.load_cached_snapshot {
                let cached_result = cx
                    .background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = repository.clone();
                        async move { github::load_pull_request_detail(&cache, &repository, number) }
                    })
                    .await;

                should_sync = match &cached_result {
                    Ok(snapshot) => detail_snapshot_needs_background_refresh(snapshot),
                    Err(_) => true,
                };

                model
                    .update(cx, |s, cx| {
                        let ds = s.detail_states.entry(detail_key.clone()).or_default();
                        match &cached_result {
                            Ok(snapshot) => {
                                ds.snapshot = Some(snapshot.clone());
                                ds.loading = snapshot.detail.is_none() && should_sync;
                                ds.error = None;
                            }
                            Err(error) => {
                                ds.loading = should_sync;
                                ds.error = Some(error.clone());
                            }
                        }
                        if s.selected_file_path.is_none() {
                            if let Some(detail) =
                                ds.snapshot.as_ref().and_then(|sn| sn.detail.as_ref())
                            {
                                s.selected_file_path = default_review_file(detail).or_else(|| {
                                    detail.files.first().map(|file| file.path.clone()).or_else(
                                        || detail.parsed_diff.first().map(|file| file.path.clone()),
                                    )
                                });
                            }
                        }
                        cx.notify();
                    })
                    .ok();
            }

            if !should_sync {
                return;
            }

            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = ds
                        .snapshot
                        .as_ref()
                        .and_then(|sn| sn.detail.as_ref())
                        .is_none();
                    ds.syncing = true;
                    ds.error = None;
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
                    ds.loading = false;
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
                            s.selected_file_path =
                                default_review_file(detail).or_else(|| {
                                    detail.files.first().map(|file| file.path.clone()).or_else(
                                        || detail.parsed_diff.first().map(|file| file.path.clone()),
                                    )
                                });
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

#[derive(Clone, Copy)]
struct PullRequestOpenPlan {
    load_cached_snapshot: bool,
    sync_live: bool,
    show_loading: bool,
}

fn plan_pull_request_open(state: &AppState, key: &str) -> PullRequestOpenPlan {
    let Some(detail_state) = state.detail_states.get(key) else {
        return PullRequestOpenPlan {
            load_cached_snapshot: true,
            sync_live: false,
            show_loading: true,
        };
    };

    let has_detail = detail_state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.detail.as_ref())
        .is_some();

    if detail_state.loading || detail_state.syncing {
        return PullRequestOpenPlan {
            load_cached_snapshot: false,
            sync_live: false,
            show_loading: !has_detail,
        };
    }

    if !has_detail {
        return PullRequestOpenPlan {
            load_cached_snapshot: true,
            sync_live: false,
            show_loading: true,
        };
    }

    PullRequestOpenPlan {
        load_cached_snapshot: false,
        sync_live: detail_state
            .snapshot
            .as_ref()
            .map(detail_snapshot_needs_background_refresh)
            .unwrap_or(true),
        show_loading: false,
    }
}

fn detail_snapshot_needs_background_refresh(snapshot: &github::PullRequestDetailSnapshot) -> bool {
    if snapshot.detail.is_none() {
        return true;
    }

    let Some(fetched_at_ms) = snapshot.fetched_at_ms else {
        return true;
    };

    current_time_ms().saturating_sub(fetched_at_ms) > DETAIL_AUTO_REFRESH_TTL_MS
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn format_relative_time(value: &str) -> String {
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
