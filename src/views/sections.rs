use gpui::prelude::*;
use gpui::*;

use crate::app_assets::{
    OVERVIEW_MY_PULL_REQUESTS_ASSET, OVERVIEW_OPEN_PULL_REQUESTS_ASSET,
    OVERVIEW_REVIEW_REQUESTS_ASSET,
};
use crate::review_queue::default_review_file;
use crate::review_session::{load_review_session, location_label};
use crate::shader_surface::{
    opengl_shader_surface_variant_with_corner_mask, opengl_shader_surface_with_corner_mask,
    OverviewShaderVariant, ShaderCornerMask,
};
use crate::state::*;
use crate::theme::*;
use crate::{github, notifications};

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
    let workspace_error = s.workspace_error.clone();
    let first_open_tab = s.open_tabs.first().cloned();
    let overview_greeting_index = s.overview_greeting_index;
    let review_comment_items = overview_review_comment_items(&s, &review_items);

    let welcome_greeting =
        overview_welcome_greeting(&viewer_name, is_auth, overview_greeting_index).to_string();
    let queue_copy = if workspace_loading {
        "Loading the latest review requests from the workspace snapshot.".to_string()
    } else if workspace_error.is_some() {
        "Workspace sync needs attention before the queue can refresh.".to_string()
    } else if is_auth {
        "Pull requests currently waiting on your attention.".to_string()
    } else {
        "Authenticate with gh to populate the review queue.".to_string()
    };
    let state_for_open_pull_requests = state.clone();
    let state_for_authored = state.clone();
    let state_for_review_requests = state.clone();
    let state_for_items = state.clone();
    let state_for_comment_items = state.clone();
    let show_empty_state = is_auth
        && !workspace_loading
        && workspace_error.is_none()
        && review_items.is_empty()
        && review_comment_items.is_empty();

    div()
        .p(px(28.0))
        .px(px(40.0))
        .flex()
        .flex_col()
        .gap(px(18.0))
        .flex_grow()
        .min_h_0()
        .h_full()
        .overflow_hidden()
        .child(overview_ambient_strip(welcome_greeting))
        .child(
            div()
                .id("overview-scroll")
                .flex()
                .flex_col()
                .gap(px(18.0))
                .flex_grow()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_wrap()
                        .gap(px(12.0))
                        .child(overview_metric_card(
                            OVERVIEW_OPEN_PULL_REQUESTS_ASSET,
                            "Open Pull Requests",
                            open_tab_count,
                            OverviewShaderVariant::Bands,
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
                        .child(overview_metric_card(
                            OVERVIEW_MY_PULL_REQUESTS_ASSET,
                            "My Pull Requests",
                            authored_count,
                            OverviewShaderVariant::Flow,
                            is_auth,
                            {
                                let state = state_for_authored.clone();
                                move |_, _, cx| {
                                    activate_queue(&state, SectionId::Pulls, "authored", cx);
                                }
                            },
                        ))
                        .child(overview_metric_card(
                            OVERVIEW_REVIEW_REQUESTS_ASSET,
                            "Review Requests",
                            review_count,
                            OverviewShaderVariant::Bands,
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
                .when(show_empty_state, |el| {
                    el.child(overview_empty_state_panel())
                })
                .when(!show_empty_state, |el| {
                    el.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_wrap()
                            .items_start()
                            .gap(px(18.0))
                            .child(div().flex_1().min_w(px(480.0)).child(
                                overview_review_comment_briefing_panel(
                                    review_comment_items.clone(),
                                    workspace_loading,
                                    workspace_error.clone(),
                                    is_auth,
                                    state_for_comment_items.clone(),
                                ),
                            ))
                            .child(div().flex_1().min_w(px(380.0)).child(
                                overview_review_requests_panel(
                                    review_items,
                                    review_count,
                                    queue_copy,
                                    workspace_loading,
                                    workspace_error.clone(),
                                    is_auth,
                                    state_for_items.clone(),
                                ),
                            )),
                    )
                }),
        )
}

#[derive(Clone)]
struct OverviewReviewCommentItem {
    summary: github::PullRequestSummary,
    author_login: String,
    author_avatar_url: Option<String>,
    location: String,
    preview: String,
    timestamp: String,
    unread: bool,
    is_resolved: bool,
    is_outdated: bool,
}

fn overview_ambient_strip(welcome_greeting: String) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .min_h(px(128.0))
        .flex()
        .flex_shrink_0()
        .rounded(radius())
        .border_1()
        .border_color(border_muted())
        .overflow_hidden()
        .child(
            opengl_shader_surface_with_corner_mask(
                "overview-attention",
                radius(),
                bg_canvas(),
                ShaderCornerMask::LEFT,
            )
            .w(px(188.0))
            .h_full()
            .flex_shrink_0(),
        )
        .child(
            div()
                .relative()
                .min_h(px(128.0))
                .h_full()
                .flex_grow()
                .bg(bg_overlay())
                .rounded_r(radius())
                .p(px(24.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(24.0))
                .child(
                    div().min_w_0().flex().flex_col().gap(px(0.0)).child(
                        div()
                            .text_size(px(28.0))
                            .line_height(px(32.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .line_clamp(2)
                            .child(welcome_greeting),
                    ),
                ),
        )
}

fn overview_metric_card(
    icon_asset: &str,
    label: &str,
    count: i64,
    shader_variant: OverviewShaderVariant,
    interactive: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let material_key = format!("metric-{label}");

    div()
        .flex_1()
        .min_w(px(190.0))
        .min_h(px(84.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(0.0))
        .rounded(radius())
        .overflow_hidden()
        .border_1()
        .border_color(border_muted())
        .when(interactive, |el| {
            el.cursor_pointer()
                .hover(|style| style.border_color(border_muted()))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(
            opengl_shader_surface_variant_with_corner_mask(
                material_key.clone(),
                shader_variant,
                radius(),
                bg_canvas(),
                ShaderCornerMask::LEFT,
            )
            .w(px(72.0))
            .h_full()
            .flex_shrink_0()
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(25.0))
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded(px(999.0))
                    .bg(white().opacity(0.72))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(icon_asset.to_string())
                            .size(px(17.0))
                            .text_color(fg_emphasis()),
                    ),
            ),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .p(px(14.0))
                .px(px(16.0))
                .flex_grow()
                .bg(bg_surface())
                .rounded_r(radius())
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_subtle())
                        .child(label.to_uppercase()),
                )
                .child(
                    div()
                        .text_size(px(27.0))
                        .line_height(px(29.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(count.to_string()),
                ),
        )
}

fn overview_empty_state_panel() -> impl IntoElement {
    panel().child(
        div()
            .p(px(28.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(20.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(eyebrow("All clear"))
                    .child(
                        div()
                            .text_size(px(22.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("No review work needs attention."),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(fg_muted())
                            .child(
                                "No unread review comments or review requests were found in the current workspace snapshot.",
                            ),
                    ),
            )
    )
}

fn overview_review_comment_briefing_panel(
    items: Vec<OverviewReviewCommentItem>,
    workspace_loading: bool,
    workspace_error: Option<String>,
    is_auth: bool,
    state: Entity<AppState>,
) -> impl IntoElement {
    let has_unread = items.iter().any(|item| item.unread);
    let copy = if workspace_loading {
        "Checking the current review queue for new thread replies."
    } else if workspace_error.is_some() {
        "Workspace sync needs attention before the comment briefing can refresh."
    } else if has_unread {
        "Newest unread replies on review threads in your queue."
    } else {
        "Latest review-thread activity from the current review queue."
    };

    panel().child(
        div()
            .p(px(20.0))
            .px(px(22.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .min_w_0()
                    .child(eyebrow("Newest comments"))
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Review Comment Briefing"),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(fg_muted())
                            .child(copy),
                    ),
            )
            .when(workspace_loading, |el| {
                el.child(panel_state_text("Loading review comments..."))
            })
            .when_some(workspace_error.clone(), |el, err| {
                el.child(error_text(&err))
            })
            .when(
                !workspace_loading && workspace_error.is_none() && items.is_empty(),
                |el| {
                    el.child(panel_state_text(if is_auth {
                        "No new review comments are waiting in the current queue."
                    } else {
                        "Authenticate with gh to populate review comments."
                    }))
                },
            )
            .child(
                div().flex().flex_col().gap(px(10.0)).children(
                    items
                        .into_iter()
                        .map(|item| overview_review_comment_row(item, state.clone())),
                ),
            ),
    )
}

fn overview_review_requests_panel(
    review_items: Vec<github::PullRequestSummary>,
    review_count: i64,
    queue_copy: String,
    workspace_loading: bool,
    workspace_error: Option<String>,
    is_auth: bool,
    state: Entity<AppState>,
) -> impl IntoElement {
    let visible_count = review_items.len().min(5);
    let remaining_count = review_count.saturating_sub(visible_count as i64);

    panel().child(
        div()
            .p(px(20.0))
            .px(px(22.0))
            .flex()
            .flex_col()
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
                            .text_size(px(18.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Review Requests"),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(fg_muted())
                            .child(queue_copy),
                    ),
            )
            .when(workspace_loading, |el| {
                el.child(panel_state_text("Loading review requests..."))
            })
            .when_some(workspace_error.clone(), |el, err| {
                el.child(error_text(&err))
            })
            .when(
                !workspace_loading && workspace_error.is_none() && review_items.is_empty(),
                |el| {
                    el.child(panel_state_text(if is_auth {
                        "No pull requests are currently requesting your review."
                    } else {
                        "Authenticate with gh to populate the review queue."
                    }))
                },
            )
            .child(div().flex().flex_col().gap(px(10.0)).children(
                review_items.into_iter().take(5).map(|item| {
                    let state = state.clone();
                    pr_list_row(item, move |summary, window, cx| {
                        open_pull_request(&state, summary, window, cx);
                    })
                }),
            ))
            .when(remaining_count > 0, |el| {
                el.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(fg_subtle())
                        .child(format!("{remaining_count} more in the review board")),
                )
            }),
    )
}

fn overview_review_comment_row(
    item: OverviewReviewCommentItem,
    state: Entity<AppState>,
) -> impl IntoElement {
    let summary = item.summary.clone();
    let repo_ref = format!("{} #{}", summary.repository, summary.number);
    let title = summary.title.clone();
    let author_login = item.author_login.clone();
    let author_avatar_url = item.author_avatar_url.clone();
    let meta = format!("commented {}", format_relative_time(&item.timestamp));
    let location = item.location.clone();
    let preview = item.preview.clone();

    div()
        .w_full()
        .p(px(16.0))
        .rounded(radius())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(hover_bg())
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_pull_request(&state, summary.clone(), window, cx);
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(14.0))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(repo_ref),
                                )
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .text_size(px(14.0))
                                        .line_clamp(1)
                                        .child(title),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .flex_wrap()
                                .justify_end()
                                .when(item.unread, |el| {
                                    el.child(pill_badge("new", accent(), accent_muted(), accent()))
                                })
                                .when(item.is_resolved, |el| el.child(subtle_pill("resolved")))
                                .when(item.is_outdated, |el| el.child(subtle_pill("outdated"))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(user_avatar(
                            &author_login,
                            author_avatar_url.as_deref(),
                            18.0,
                            false,
                        ))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .child(author_login),
                        )
                        .child(meta),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .line_height(px(19.0))
                        .text_color(fg_default())
                        .line_clamp(2)
                        .child(preview),
                )
                .child(
                    div()
                        .min_w_0()
                        .font_family(mono_font_family())
                        .text_size(px(10.0))
                        .text_color(fg_subtle())
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(location),
                ),
        )
}

fn overview_review_comment_items(
    state: &AppState,
    review_items: &[github::PullRequestSummary],
) -> Vec<OverviewReviewCommentItem> {
    let mut summaries = BTreeMap::new();
    for item in review_items {
        summaries.insert(pr_key(&item.repository, item.number), item.clone());
    }

    let viewer_login = state
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.viewer.as_ref())
        .map(|viewer| viewer.login.as_str())
        .unwrap_or_default();
    let mut unread_items = Vec::new();
    let mut latest_items = Vec::new();

    for (key, detail_state) in &state.detail_states {
        let Some(summary) = summaries.get(key) else {
            continue;
        };
        let Some(detail) = detail_state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.detail.as_ref())
        else {
            continue;
        };

        for thread in &detail.review_threads {
            let mut latest_foreign_comment = None;
            for comment in &thread.comments {
                let unread = state.unread_review_comment_ids.contains(&comment.id);
                if unread {
                    unread_items.push(overview_comment_item_for_comment(
                        summary, thread, comment, true,
                    ));
                }

                if !thread.is_resolved
                    && !comment.body.trim().is_empty()
                    && (viewer_login.is_empty() || comment.author_login != viewer_login)
                {
                    latest_foreign_comment = Some(comment);
                }
            }

            if unread_items.is_empty() {
                if let Some(comment) = latest_foreign_comment {
                    latest_items.push(overview_comment_item_for_comment(
                        summary, thread, comment, false,
                    ));
                }
            }
        }
    }

    let mut items = if unread_items.is_empty() {
        latest_items
    } else {
        unread_items
    };
    items.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.location.cmp(&right.location))
    });
    items.truncate(5);
    items
}

fn overview_comment_item_for_comment(
    summary: &github::PullRequestSummary,
    thread: &github::PullRequestReviewThread,
    comment: &github::PullRequestReviewComment,
    unread: bool,
) -> OverviewReviewCommentItem {
    OverviewReviewCommentItem {
        summary: summary.clone(),
        author_login: comment.author_login.clone(),
        author_avatar_url: comment.author_avatar_url.clone(),
        location: overview_comment_location(thread, comment),
        preview: summarize_overview_comment(&comment.body),
        timestamp: comment
            .published_at
            .clone()
            .unwrap_or_else(|| comment.updated_at.clone()),
        unread,
        is_resolved: thread.is_resolved,
        is_outdated: thread.is_outdated,
    }
}

fn overview_comment_location(
    thread: &github::PullRequestReviewThread,
    comment: &github::PullRequestReviewComment,
) -> String {
    let path = if comment.path.trim().is_empty() {
        thread.path.as_str()
    } else {
        comment.path.as_str()
    };
    let line = comment
        .line
        .or(comment.original_line)
        .or(thread.line)
        .or(thread.original_line)
        .and_then(|line| usize::try_from(line).ok());

    location_label(path, line)
}

fn summarize_overview_comment(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "No comment body.".to_string();
    }

    let limit = 180usize;
    let mut preview = collapsed.chars().take(limit).collect::<String>();
    if collapsed.chars().count() > limit {
        preview.push_str("...");
    }
    preview
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
    let queue_truncation_message = current_queue.as_ref().and_then(|queue| {
        if queue.is_complete {
            None
        } else {
            Some(queue.truncated_reason.clone().unwrap_or_else(|| {
                format!(
                    "Loaded {} of {} pull requests.",
                    queue.items.len(),
                    queue.total_count
                )
            }))
        }
    });
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
    let board_shader_offset = if is_authored_queue {
        2
    } else if is_reviews {
        1
    } else {
        0
    };

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
                .bg(bg_overlay())
                .border_r(px(1.0))
                .border_color(border_muted())
                .p(px(24.0))
                .px(px(28.0))
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
                        .text_size(px(13.0))
                        .text_color(fg_muted())
                        .mt(px(6.0))
                        .max_w(px(200.0))
                        .child(if is_reviews {
                            "Review requests grouped by repository."
                        } else {
                            "Pull requests grouped into repo lanes."
                        }),
                )
                .child(div().flex().flex_col().gap(px(6.0)).mt(px(22.0)).children(
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
                .when_some(queue_truncation_message, |el, message| {
                    el.child(div().px(px(28.0)).pb(px(12.0)).child(error_text(&message)))
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
                        .px(px(24.0))
                        .pb(px(24.0))
                        .child(
                            div()
                                .flex()
                                .gap(px(16.0))
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
                                        board_shader_offset + 2,
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
                                        board_shader_offset,
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

pub fn material_surface(seed: &str) -> Div {
    shader_material_surface(
        seed,
        0,
        ShaderCornerMask::default(),
        transparent(),
        radius(),
    )
}

fn material_surface_with_corner_radius(
    seed: &str,
    variant_offset: usize,
    corners: ShaderCornerMask,
    mask_color: Rgba,
    corner_radius: Pixels,
) -> Div {
    shader_material_surface(seed, variant_offset, corners, mask_color, corner_radius)
}

fn shader_material_surface(
    seed: &str,
    variant_offset: usize,
    corners: ShaderCornerMask,
    mask_color: Rgba,
    corner_radius: Pixels,
) -> Div {
    let seed = seed.to_string();
    let shader_seed = format!("review-material-{seed}");
    let variant = material_shader_variant(&seed, variant_offset);

    opengl_shader_surface_variant_with_corner_mask(
        shader_seed,
        variant,
        corner_radius,
        mask_color,
        corners,
    )
}

fn material_shader_variant(seed: &str, offset: usize) -> OverviewShaderVariant {
    match (material_seed_index(seed) + offset) % 3 {
        0 => OverviewShaderVariant::Ember,
        1 => OverviewShaderVariant::Lagoon,
        _ => OverviewShaderVariant::Aurora,
    }
}

fn material_seed_index(seed: &str) -> usize {
    let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
        acc.wrapping_mul(16777619) ^ byte as u32
    });
    (hash as usize) % 3
}

pub fn panel() -> Div {
    div()
        .rounded(radius())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
        .shadow_sm()
        .overflow_hidden()
}

pub fn nested_panel() -> Div {
    div()
        .p(px(20.0))
        .rounded(radius())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
}

pub fn eyebrow(text: &str) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg_subtle())
        .mb(px(8.0))
        .child(text.to_string().to_uppercase())
}

pub fn ghost_button(
    label: &str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(7.0))
        .rounded(radius_sm())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
        .text_color(fg_default())
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(hover_bg())
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
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
        .bg(primary_action_bg())
        .text_color(fg_on_primary_action())
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.bg(primary_action_hover()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
}

pub fn badge(text: &str) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg_muted())
        .child(text.to_string())
}

pub fn user_avatar(
    login: &str,
    avatar_url: Option<&str>,
    size: f32,
    emphasized: bool,
) -> AnyElement {
    let login = login.to_string();
    let avatar_url = avatar_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);

    match avatar_url {
        Some(url) => {
            let url = avatar_image_url(&url, size);
            let inner_size = avatar_inner_size(size);
            let loading_login = login.clone();
            let fallback_login = login.clone();
            div()
                .w(px(size))
                .h(px(size))
                .rounded(px(size / 2.0))
                .overflow_hidden()
                .border_1()
                .border_color(if emphasized { accent() } else { border_muted() })
                .bg(if emphasized {
                    accent_muted()
                } else {
                    bg_emphasis()
                })
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    img(url)
                        .size(px(inner_size))
                        .rounded(px(inner_size / 2.0))
                        .overflow_hidden()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(move || {
                            avatar_placeholder(&loading_login, inner_size, emphasized)
                                .into_any_element()
                        })
                        .with_fallback(move || {
                            avatar_placeholder(&fallback_login, inner_size, emphasized)
                                .into_any_element()
                        }),
                )
                .into_any_element()
        }
        None => avatar_placeholder(&login, size, emphasized).into_any_element(),
    }
}

fn avatar_inner_size(size: f32) -> f32 {
    (size - 2.0).max(1.0)
}

fn avatar_image_url(url: &str, display_size: f32) -> String {
    if !url.contains("avatars.githubusercontent.com") {
        return url.to_string();
    }

    let image_size = ((display_size * 3.0).ceil() as usize).clamp(96, 256);
    let (url_without_fragment, fragment) = url
        .split_once('#')
        .map(|(url, fragment)| (url, Some(fragment)))
        .unwrap_or((url, None));
    let (base, query) = url_without_fragment
        .split_once('?')
        .unwrap_or((url_without_fragment, ""));
    let mut params = query
        .split('&')
        .filter(|param| !param.is_empty() && !param.starts_with("s="))
        .map(str::to_string)
        .collect::<Vec<_>>();
    params.push(format!("s={image_size}"));

    let mut output = if params.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", params.join("&"))
    };
    if let Some(fragment) = fragment {
        output.push('#');
        output.push_str(fragment);
    }
    output
}

fn avatar_placeholder(login: &str, size: f32, emphasized: bool) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .border_1()
        .border_color(if emphasized { accent() } else { border_muted() })
        .bg(if emphasized {
            accent_muted()
        } else {
            bg_emphasis()
        })
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .text_size(px((size * 0.38).max(9.0)))
        .font_family(mono_font_family())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if emphasized { accent() } else { fg_emphasis() })
        .child(login_monogram(login))
}

fn login_monogram(login: &str) -> String {
    let mut monogram = login
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if monogram.is_empty() {
        monogram.push('?');
    }
    monogram
}

pub fn badge_success(text: &str) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(success_muted())
        .border_1()
        .border_color(diff_add_border())
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
                .font_family(mono_font_family())
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
                .font_family(mono_font_family())
                .text_size(px(11.0))
                .whitespace_normal()
                .child(value.to_string()),
        )
}

fn welcome_greeting_count(is_authenticated: bool) -> usize {
    if is_authenticated {
        4
    } else {
        3
    }
}

fn overview_welcome_greeting(
    viewer_name: &str,
    is_authenticated: bool,
    greeting_index: usize,
) -> String {
    let index = greeting_index % welcome_greeting_count(is_authenticated);
    let viewer_name = viewer_name.trim();
    let viewer_name = if viewer_name.is_empty() {
        "there"
    } else {
        viewer_name
    };

    if is_authenticated {
        match index {
            0 => format!("Welcome back, {viewer_name}"),
            1 => format!("Ready for review, {viewer_name}?"),
            2 => "The review queue is live.".to_string(),
            _ => "Let's clear what needs attention.".to_string(),
        }
    } else {
        match index {
            0 => "Connect GitHub".to_string(),
            1 => "Authenticate with gh".to_string(),
            _ => "Bring your reviews into focus.".to_string(),
        }
    }
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
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(active, |el| el.bg(bg_selected()).text_color(fg_emphasis()))
        .when(!active, |el| el.text_color(fg_muted()))
        .hover(|style| {
            style
                .bg(hover_bg())
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
        .child(
            div()
                .text_color(if active { fg_default() } else { fg_subtle() })
                .font_family(mono_font_family())
                .text_size(px(12.0))
                .child(count.to_string()),
        )
}

fn pill_badge(label: &str, fg: Rgba, bg: Rgba, border: Rgba) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(border)
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
        "MERGED" => pill_badge("Merged", info(), info_muted(), info()).into_any_element(),
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
                .font_family(mono_font_family())
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
    let material_key = format!("{}-{}", item.repository, item.number);
    let author_login = item.author_login.clone();
    let author_avatar_url = item.author_avatar_url.clone();
    let meta = format!("updated {}", format_relative_time(&item.updated_at));
    let comments = item.comments_count;
    let changed_files = item.changed_files;
    let additions = item.additions;
    let deletions = item.deletions;
    let review_decision = item.review_decision.clone();
    let summary = item.clone();

    div()
        .w_full()
        .rounded(radius())
        .bg(bg_overlay())
        .border_1()
        .border_color(transparent())
        .shadow_sm()
        .overflow_hidden()
        .flex()
        .cursor_pointer()
        .hover(|style| style.bg(bg_surface()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(summary.clone(), window, cx)
        })
        .child(material_surface(&material_key).w(px(10.0)).flex_shrink_0())
        .child(
            div()
                .flex_grow()
                .p(px(14.0))
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
                                .font_family(mono_font_family())
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
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child(user_avatar(
                                    &author_login,
                                    author_avatar_url.as_deref(),
                                    18.0,
                                    false,
                                ))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .child(author_login),
                                )
                                .child(format!("\u{2022} {meta}")),
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
                .child(render_diff_summary(additions, deletions)),
        )
}

fn kanban_lane(
    lane_id: &str,
    label: &str,
    subtitle: &str,
    items: Vec<github::PullRequestSummary>,
    _accent: Rgba,
    is_mine: bool,
    shader_offset: usize,
    state: Entity<AppState>,
) -> impl IntoElement {
    let label = label.to_string();
    let subtitle = subtitle.to_string();
    let count = items.len();
    let mute_state = state.clone();
    let mute_repo = lane_id.to_string();

    div()
        .w(px(320.0))
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
                .rounded(radius_lg())
                .bg(transparent())
                .shadow_md()
                .child(
                    material_surface_with_corner_radius(
                        lane_id,
                        shader_offset + 1,
                        ShaderCornerMask::TOP,
                        bg_canvas(),
                        radius_lg(),
                    )
                    .h(px(70.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .text_color(fg_emphasis())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
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
                                    .rounded(px(999.0))
                                    .bg(white().opacity(0.62))
                                    .text_size(px(11.0))
                                    .font_family(mono_font_family())
                                    .text_color(fg_emphasis())
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
                                .text_color(fg_emphasis())
                                .cursor_pointer()
                                .bg(white().opacity(0.46))
                                .hover(|s| s.bg(white().opacity(0.68)).text_color(danger()))
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
                        .flex()
                        .flex_col()
                        .flex_grow()
                        .min_h_0()
                        .bg(bg_overlay())
                        .rounded_b(radius_lg())
                        .overflow_hidden()
                        .child(
                            div()
                                .px(px(14.0))
                                .py(px(10.0))
                                .text_size(px(11.0))
                                .text_color(fg_subtle())
                                .font_family(mono_font_family())
                                .child(subtitle),
                        )
                        .child(
                            div()
                                .flex_grow()
                                .min_h_0()
                                .id(SharedString::from(format!("lane-scroll-{lane_id}")))
                                .overflow_y_scroll()
                                .px(px(10.0))
                                .pb(px(10.0))
                                .child(div().flex().flex_col().gap(px(8.0)).children(
                                    items.into_iter().map(|item| {
                                        let state = state.clone();
                                        kanban_card(item, move |summary, window, cx| {
                                            open_pull_request(&state, summary, window, cx);
                                        })
                                    }),
                                )),
                        ),
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
    let author_login = item.author_login.clone();
    let author_avatar_url = item.author_avatar_url.clone();
    let meta = format!(
        "{} #{} \u{00b7} {}",
        repo_label,
        item.number,
        format_relative_time(&item.updated_at)
    );
    let additions = item.additions;
    let deletions = item.deletions;
    let comments = item.comments_count;
    let changed_files = item.changed_files;
    let review_decision = item.review_decision.clone();
    let summary = item.clone();

    div()
        .rounded(radius())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
        .shadow_sm()
        .p(px(14.0))
        .cursor_pointer()
        .hover(|s| s.bg(bg_overlay()).shadow_md())
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
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .min_w_0()
                                        .text_size(px(10.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_muted())
                                        .child(user_avatar(
                                            &author_login,
                                            author_avatar_url.as_deref(),
                                            16.0,
                                            false,
                                        ))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(fg_emphasis())
                                                .child(author_login),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .overflow_x_hidden()
                                                .child(format!("\u{00b7} {meta}")),
                                        ),
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
        s.set_active_section(section);
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
        s.set_active_section(SectionId::Pulls);
        s.active_surface = PullRequestSurface::Files;
        s.active_pr_key = Some(key.clone());
        s.palette_open = false;
        s.palette_selected_index = 0;
        s.review_body.clear();
        s.review_editor_active = false;
        s.review_message = None;
        s.review_success = false;
        s.pr_header_compact = false;

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
                    async move {
                        notifications::sync_pull_request_detail_with_read_state(
                            &cache,
                            &repository,
                            number,
                        )
                    }
                })
                .await;

            model
                .update(cx, |s, cx| {
                    let mut next_unread_ids = None;
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = false;
                    ds.syncing = false;
                    match sync_result {
                        Ok((snapshot, unread_ids)) => {
                            ds.snapshot = Some(snapshot);
                            ds.error = None;
                            next_unread_ids = Some(unread_ids);
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
                    if let Some(unread_ids) = next_unread_ids {
                        s.unread_review_comment_ids = unread_ids;
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
