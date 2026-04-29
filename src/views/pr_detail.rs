use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use crate::code_tour::review_thread_anchor;
use crate::github::{
    self, PullRequestComment, PullRequestReview, PullRequestReviewComment, PullRequestReviewThread,
    ReviewAction,
};
use crate::markdown::render_markdown;
use crate::notifications;
use crate::review_session::ReviewCenterMode;
use crate::selectable_text::{AppTextFieldKind, AppTextInput, SelectableText};
use crate::state::*;
use crate::theme::*;

use super::ai_tour::refresh_active_tour_flow;
use super::diff_view::{enter_files_surface, render_files_view};
use super::sections::{
    badge, error_text, eyebrow, format_relative_time, ghost_button, nested_panel, panel_state_text,
    review_button, success_text, user_avatar,
};

#[derive(Debug, Default, PartialEq, Eq)]
struct ReviewStatusSummary {
    approved: Vec<String>,
    changes_requested: Vec<String>,
    commented: Vec<String>,
    waiting: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnPrFeedbackItem {
    anchor: crate::code_tour::DiffAnchor,
    file_path: String,
    location_label: String,
    author_login: String,
    author_avatar_url: Option<String>,
    updated_at: String,
    preview: String,
    subject_type: String,
    feedback_count: usize,
    unread_count: usize,
    unread_comment_ids: Vec<String>,
    is_resolved: bool,
    is_outdated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThreadDigestItem {
    anchor: crate::code_tour::DiffAnchor,
    file_path: String,
    location_label: String,
    latest_author: String,
    latest_author_avatar_url: Option<String>,
    updated_at: String,
    preview: String,
    subject_type: String,
    comment_count: usize,
    unread_count: usize,
    unread_comment_ids: Vec<String>,
    is_resolved: bool,
    is_outdated: bool,
    resolved_by_login: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParticipantItem {
    login: String,
    avatar_url: Option<String>,
    is_author: bool,
    is_requested: bool,
    approved: bool,
    changes_requested: bool,
    commented: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActivityItemKind {
    Conversation,
    Review,
    Thread,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActivityItem {
    kind: ActivityItemKind,
    author_login: String,
    author_avatar_url: Option<String>,
    timestamp: String,
    title: String,
    preview: String,
    status_label: Option<String>,
    status_code: Option<String>,
    location_label: Option<String>,
    file_path: Option<String>,
    anchor: Option<crate::code_tour::DiffAnchor>,
}

fn summarize_review_status(
    reviewers: &[String],
    latest_reviews: &[PullRequestReview],
) -> ReviewStatusSummary {
    let mut latest_by_author = BTreeMap::<String, &PullRequestReview>::new();
    for review in latest_reviews {
        let author = review.author_login.trim();
        if author.is_empty() {
            continue;
        }
        latest_by_author.insert(author.to_string(), review);
    }

    let mut approved = BTreeSet::new();
    let mut changes_requested = BTreeSet::new();
    let mut commented = BTreeSet::new();

    for (author, review) in latest_by_author {
        match review.state.as_str() {
            "APPROVED" => {
                approved.insert(author);
            }
            "CHANGES_REQUESTED" => {
                changes_requested.insert(author);
            }
            _ => {
                commented.insert(author);
            }
        }
    }

    let mut waiting = BTreeSet::new();
    for reviewer in reviewers {
        let reviewer = reviewer.trim();
        if reviewer.is_empty() {
            continue;
        }
        if !approved.contains(reviewer)
            && !changes_requested.contains(reviewer)
            && !commented.contains(reviewer)
        {
            waiting.insert(reviewer.to_string());
        }
    }

    ReviewStatusSummary {
        approved: approved.into_iter().collect(),
        changes_requested: changes_requested.into_iter().collect(),
        commented: commented.into_iter().collect(),
        waiting: waiting.into_iter().collect(),
    }
}

fn summarize_own_pr_feedback(
    review_threads: &[PullRequestReviewThread],
    viewer_login: &str,
    unread_comment_ids: &BTreeSet<String>,
) -> Vec<OwnPrFeedbackItem> {
    let viewer_login = viewer_login.trim();
    let mut items = review_threads
        .iter()
        .filter_map(|thread| own_pr_feedback_item(thread, viewer_login, unread_comment_ids))
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        left.is_resolved
            .cmp(&right.is_resolved)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.location_label.cmp(&right.location_label))
    });
    items
}

fn own_pr_feedback_item(
    thread: &PullRequestReviewThread,
    viewer_login: &str,
    unread_comment_ids: &BTreeSet<String>,
) -> Option<OwnPrFeedbackItem> {
    let anchor = review_thread_anchor(thread)?;
    let latest_feedback = thread
        .comments
        .iter()
        .rev()
        .find(|comment| comment.author_login != viewer_login)?;
    let feedback_count = thread
        .comments
        .iter()
        .filter(|comment| comment.author_login != viewer_login)
        .count();
    let unread_comment_ids = thread_unread_comment_ids(thread, unread_comment_ids);

    Some(OwnPrFeedbackItem {
        file_path: thread.path.clone(),
        location_label: feedback_location_label(thread, &anchor),
        author_login: latest_feedback.author_login.clone(),
        author_avatar_url: latest_feedback.author_avatar_url.clone(),
        updated_at: latest_feedback
            .published_at
            .clone()
            .unwrap_or_else(|| latest_feedback.updated_at.clone()),
        preview: summarize_feedback_preview(latest_feedback),
        subject_type: thread.subject_type.clone(),
        feedback_count,
        unread_count: unread_comment_ids.len(),
        unread_comment_ids,
        is_resolved: thread.is_resolved,
        is_outdated: thread.is_outdated,
        anchor,
    })
}

fn feedback_location_label(
    thread: &PullRequestReviewThread,
    anchor: &crate::code_tour::DiffAnchor,
) -> String {
    match anchor.line.or(thread.line).or(thread.original_line) {
        Some(line) => format!("{}:{}", thread.path, line),
        None => thread.path.clone(),
    }
}

fn summarize_feedback_preview(comment: &PullRequestReviewComment) -> String {
    truncate_markdown_preview(&comment.body, 320)
}

fn truncate_markdown_preview(body: &str, limit: usize) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "No comment body.".to_string();
    }

    let mut collapsed = String::with_capacity(trimmed.len());
    let mut blank_run = 0usize;
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                collapsed.push('\n');
            }
        } else {
            blank_run = 0;
            if !collapsed.is_empty() && !collapsed.ends_with('\n') {
                collapsed.push('\n');
            }
            collapsed.push_str(line);
            collapsed.push('\n');
        }
    }
    let collapsed = collapsed.trim_end().to_string();

    if collapsed.chars().count() <= limit {
        collapsed
    } else {
        let mut out: String = collapsed.chars().take(limit).collect();
        out.push('…');
        out
    }
}

fn viewer_login(state: &AppState) -> Option<String> {
    state.workspace.as_ref().and_then(|workspace| {
        workspace
            .viewer
            .as_ref()
            .map(|viewer| viewer.login.clone())
            .or_else(|| workspace.auth.active_login.clone())
    })
}

pub fn render_pr_workspace(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let pr = s.active_pr();
    let detail = s.active_detail();
    let detail_state = s.active_detail_state();
    let surface = s.active_surface;

    let Some(pr) = pr else {
        return div()
            .child(panel_state_text("No pull request selected."))
            .into_any_element();
    };

    let pr_title = detail
        .map(|d| d.title.clone())
        .unwrap_or_else(|| pr.title.clone());
    let pr_state = detail
        .map(|d| d.state.clone())
        .unwrap_or_else(|| pr.state.clone());
    let is_draft = detail.map(|d| d.is_draft).unwrap_or(pr.is_draft);
    let author = detail
        .map(|d| d.author_login.clone())
        .unwrap_or_else(|| pr.author_login.clone());
    let author_avatar_url = detail
        .and_then(|d| d.author_avatar_url.clone())
        .or_else(|| pr.author_avatar_url.clone());
    let repository = pr.repository.clone();
    let number = pr.number;
    let loading = detail_state.map(|d| d.loading).unwrap_or(false);
    let syncing = detail_state.map(|d| d.syncing).unwrap_or(false);
    let error = detail_state.and_then(|d| d.error.clone());
    let show_loading_state = detail.is_none() && (loading || syncing);
    let header_compact = surface != PullRequestSurface::Overview || s.pr_header_compact;
    let unread_review_comment_ids = detail
        .map(|detail| s.unread_review_comment_ids_for_detail(detail))
        .unwrap_or_default();
    let unread_review_comment_count = unread_review_comment_ids.len();

    let state_for_surface = state.clone();
    let state_for_refresh = state.clone();

    div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        // Header (fixed, never scrolls)
        .child(render_pr_header(
            &repository,
            number,
            &pr_title,
            &pr_state,
            is_draft,
            &author,
            author_avatar_url.as_deref(),
            detail.map(|d| (d.base_ref_name.clone(), d.head_ref_name.clone())),
            syncing,
            surface,
            header_compact,
            unread_review_comment_count,
            unread_review_comment_ids,
            state_for_refresh,
            state_for_surface,
        ))
        // Content area (scrollable or flex-fill depending on surface)
        .when(show_loading_state, |el| {
            el.child(
                div()
                    .px(px(32.0))
                    .child(panel_state_text("Loading pull request...")),
            )
        })
        .when_some(error, |el, err| {
            el.child(div().px(px(32.0)).child(error_text(&err)))
        })
        .when(
            detail.is_some() && surface == PullRequestSurface::Overview,
            |el| {
                el.child(
                    div()
                        .px(px(32.0))
                        .flex_grow()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .id("pr-overview-scroll")
                        .overflow_y_scroll()
                        .pb(px(24.0))
                        .child(render_overview_surface(state, cx)),
                )
            },
        )
        .when(
            detail.is_some() && surface == PullRequestSurface::Files,
            |el| el.child(render_files_view(state, cx)),
        )
        .into_any_element()
}

fn render_pr_header(
    repository: &str,
    number: i64,
    pr_title: &str,
    pr_state: &str,
    is_draft: bool,
    author: &str,
    author_avatar_url: Option<&str>,
    refs: Option<(String, String)>,
    syncing: bool,
    surface: PullRequestSurface,
    compact: bool,
    unread_review_comment_count: usize,
    unread_review_comment_ids: Vec<String>,
    state_for_refresh: Entity<AppState>,
    state_for_surface: Entity<AppState>,
) -> impl IntoElement {
    let title = pr_title.to_string();
    let author = author.to_string();
    let author_avatar_url = author_avatar_url.map(str::to_string);
    let repository = repository.to_string();
    let breadcrumb = format!("Pull Requests / {} / #{}", repository, number).to_uppercase();
    let state_for_mark_read = state_for_refresh.clone();

    let header_copy = div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(if compact { px(0.0) } else { px(4.0) })
        .child(
            div()
                .h(if compact { px(0.0) } else { px(18.0) })
                .overflow_hidden()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(mono_font_family())
                .text_color(if compact { transparent() } else { fg_subtle() })
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_x_hidden()
                .child(breadcrumb)
                .with_animation(
                    ("pr-header-eyebrow", usize::from(compact)),
                    Animation::new(Duration::from_millis(240)).with_easing(ease_in_out),
                    move |el, delta| {
                        let progress = header_animation_progress(compact, delta);
                        el.h(lerp_px(18.0, 0.0, progress)).text_color(lerp_rgba(
                            fg_subtle(),
                            transparent(),
                            progress,
                        ))
                    },
                ),
        )
        .child(
            div()
                .text_size(if compact { px(16.0) } else { px(22.0) })
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .line_height(if compact { px(20.0) } else { px(28.0) })
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_x_hidden()
                .child(title)
                .with_animation(
                    ("pr-header-title", usize::from(compact)),
                    Animation::new(Duration::from_millis(240)).with_easing(ease_in_out),
                    move |el, delta| {
                        let progress = header_animation_progress(compact, delta);
                        el.text_size(lerp_px(22.0, 16.0, progress))
                            .line_height(lerp_px(28.0, 20.0, progress))
                            .text_color(fg_emphasis())
                    },
                ),
        )
        .child(
            div()
                .h(if compact { px(0.0) } else { px(28.0) })
                .overflow_hidden()
                .text_size(px(13.0))
                .text_color(if compact { transparent() } else { fg_muted() })
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .flex_wrap()
                        .items_center()
                        .child(pull_request_state_badge(pr_state, is_draft))
                        .child(user_avatar(
                            &author,
                            author_avatar_url.as_deref(),
                            18.0,
                            false,
                        ))
                        .child(author)
                        .when(syncing, |el| el.child(badge("Refreshing live")))
                        .when_some(refs, |el, (base, head)| {
                            el.child("wants to merge into")
                                .child(badge(&base))
                                .child("from")
                                .child(badge(&head))
                        }),
                )
                .with_animation(
                    ("pr-header-meta", usize::from(compact)),
                    Animation::new(Duration::from_millis(240)).with_easing(ease_in_out),
                    move |el, delta| {
                        let progress = header_animation_progress(compact, delta);
                        el.h(lerp_px(28.0, 0.0, progress)).text_color(lerp_rgba(
                            fg_muted(),
                            transparent(),
                            progress,
                        ))
                    },
                ),
        )
        .with_animation(
            ("pr-header-copy", usize::from(compact)),
            Animation::new(Duration::from_millis(240)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = header_animation_progress(compact, delta);
                el.gap(lerp_px(6.0, 0.0, progress))
            },
        );

    let top_row = div()
        .flex()
        .items_center()
        .justify_between()
        .mb(if compact { px(4.0) } else { px(14.0) })
        .pb(if compact { px(4.0) } else { px(14.0) })
        .gap(if compact { px(8.0) } else { px(14.0) })
        .child(
            div()
                .flex()
                .items_center()
                .gap(if compact { px(8.0) } else { px(12.0) })
                .min_w_0()
                .when(!compact, |el| el.child(header_copy))
                .when(compact, |el| {
                    el.child(render_pr_surface_tabs(
                        surface,
                        state_for_surface.clone(),
                        true,
                    ))
                }),
        )
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .flex_wrap()
                .when(unread_review_comment_count > 0, |el| {
                    let unread_review_comment_ids = unread_review_comment_ids.clone();
                    el.child(ghost_button(
                        &format!("Mark read ({unread_review_comment_count})"),
                        move |_, _, cx| {
                            state_for_mark_read.update(cx, |state, cx| {
                                state.mark_review_comments_read(unread_review_comment_ids.clone());
                                cx.notify();
                            });
                        },
                    ))
                })
                .child(ghost_button(
                    if compact {
                        "Browser"
                    } else {
                        "Open in browser"
                    },
                    {
                        let repository = repository.clone();
                        move |_, window, cx| {
                            open_pull_request_in_browser(&repository, number, window, cx)
                        }
                    },
                ))
                .child(if compact {
                    ghost_button("Refresh", {
                        let state = state_for_refresh.clone();
                        let repository = repository.clone();
                        move |_, window, cx| {
                            trigger_sync_pr(&state, &repository, number, window, cx)
                        }
                    })
                    .into_any_element()
                } else {
                    review_button("Refresh PR", {
                        let state = state_for_refresh.clone();
                        let repository = repository.clone();
                        move |_, window, cx| {
                            trigger_sync_pr(&state, &repository, number, window, cx)
                        }
                    })
                    .into_any_element()
                }),
        )
        .with_animation(
            ("pr-header-top-row", usize::from(compact)),
            Animation::new(Duration::from_millis(240)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = header_animation_progress(compact, delta);
                el.mb(lerp_px(14.0, 4.0, progress))
                    .pb(lerp_px(14.0, 4.0, progress))
                    .gap(lerp_px(14.0, 8.0, progress))
            },
        );

    div()
        .flex_shrink_0()
        .bg(bg_surface())
        .border_b(px(1.0))
        .border_color(border_muted())
        .child(top_row)
        .when(!compact, |el| {
            el.child(render_pr_surface_tabs(
                surface,
                state_for_surface.clone(),
                false,
            ))
        })
        .with_animation(
            ("pr-header-shell", usize::from(compact)),
            Animation::new(Duration::from_millis(240)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = header_animation_progress(compact, delta);
                el.pt(lerp_px(18.0, 4.0, progress)).px(px(18.0)).pb(px(0.0))
            },
        )
}

fn render_pr_surface_tabs(
    surface: PullRequestSurface,
    state_for_surface: Entity<AppState>,
    inline: bool,
) -> impl IntoElement {
    div()
        .flex()
        .gap(px(2.0))
        .when(!inline, |el| el.pb(px(10.0)))
        .children(PullRequestSurface::all().iter().map(|surface_id| {
            let is_active = surface == *surface_id;
            let target_surface = *surface_id;
            let state = state_for_surface.clone();
            surface_tab(surface_id.label(), is_active, move |_, window, cx| {
                if target_surface == PullRequestSurface::Files {
                    enter_files_surface(&state, window, cx);
                } else {
                    state.update(cx, |st, cx| {
                        st.active_surface = target_surface;
                        st.pr_header_compact = false;
                        cx.notify();
                    });
                }
            })
        }))
}

fn header_animation_progress(compact: bool, delta: f32) -> f32 {
    if compact {
        delta
    } else {
        1.0 - delta
    }
}

fn lerp_px(expanded: f32, compact: f32, progress: f32) -> Pixels {
    px(expanded + (compact - expanded) * progress)
}

fn lerp_rgba(expanded: Rgba, compact: Rgba, progress: f32) -> Rgba {
    Rgba {
        r: expanded.r + (compact.r - expanded.r) * progress,
        g: expanded.g + (compact.g - expanded.g) * progress,
        b: expanded.b + (compact.b - expanded.b) * progress,
        a: expanded.a + (compact.a - expanded.a) * progress,
    }
}

fn render_overview_surface(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();
    let detail_state = s.active_detail_state();

    let Some(detail) = detail else {
        return div().into_any_element();
    };

    let review_action = s.review_action;
    let review_body = s.review_body.clone();
    let review_loading = s.review_loading;
    let review_message = s.review_message.clone();
    let review_success = s.review_success;
    let loaded_from_cache = detail_state
        .and_then(|d| d.snapshot.as_ref())
        .map(|sn| sn.loaded_from_cache)
        .unwrap_or(false);
    let syncing = detail_state.map(|d| d.syncing).unwrap_or(false);
    let fetched_at_ms = detail_state
        .and_then(|d| d.snapshot.as_ref())
        .and_then(|sn| sn.fetched_at_ms);
    let viewer_login = viewer_login(&s);
    let is_own_pull_request = viewer_login
        .as_deref()
        .map(|viewer_login| detail.author_login == viewer_login)
        .unwrap_or(false);
    let review_status = summarize_review_status(&detail.reviewers, &detail.latest_reviews);
    let own_pr_feedback = viewer_login
        .as_deref()
        .filter(|_| is_own_pull_request)
        .map(|viewer_login| {
            summarize_own_pr_feedback(
                &detail.review_threads,
                viewer_login,
                &s.unread_review_comment_ids,
            )
        })
        .unwrap_or_default();
    let thread_digest =
        summarize_thread_activity(&detail.review_threads, &s.unread_review_comment_ids);
    let recent_activity = summarize_recent_activity(detail, &s.unread_review_comment_ids);
    let participants = summarize_participants(detail, &review_status);

    let state_for_review = state.clone();
    let state_for_threads = state.clone();
    let state_for_activity = state.clone();
    let state_for_files = state.clone();

    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .flex_wrap()
        .gap(px(20.0))
        .child(
            div()
                .flex_1()
                .min_w(px(460.0))
                .flex()
                .flex_col()
                .gap(px(16.0))
                .child(render_overview_summary_strip(
                    detail,
                    is_own_pull_request,
                    &state_for_files,
                ))
                .child(render_review_snapshot_panel(
                    detail,
                    &review_status,
                    &own_pr_feedback,
                    &thread_digest,
                    is_own_pull_request,
                    &state_for_threads,
                ))
                .child(render_pull_request_summary_panel(
                    detail,
                    loaded_from_cache,
                    syncing,
                ))
                .child(render_recent_activity_panel(
                    &recent_activity,
                    &state_for_activity,
                ))
                .when(!is_own_pull_request, |el| {
                    el.child(render_submit_review_panel(
                        review_action,
                        review_body,
                        s.review_editor_active,
                        review_loading,
                        review_message,
                        review_success,
                        &state_for_review,
                    ))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(240.0))
                .max_w(detail_side_width())
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(16.0))
                .child(render_details_panel(detail, fetched_at_ms))
                .child(render_reviewers_panel(detail, &review_status))
                .child(render_participants_panel(&participants))
                .child(render_labels_panel(&detail.labels)),
        )
        .into_any_element()
}

fn render_overview_summary_strip(
    detail: &github::PullRequestDetail,
    is_own_pull_request: bool,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let state = state.clone();
    let action_label = if is_own_pull_request {
        "Open review workspace"
    } else {
        "Start review"
    };

    nested_panel().child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .flex_wrap()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .min_w_0()
                    .child(eyebrow(if is_own_pull_request {
                        "Own pull request"
                    } else {
                        "Review overview"
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.0))
                            .items_center()
                            .flex_wrap()
                            .child(render_overview_metric(
                                detail.commits_count.to_string(),
                                "Commits",
                                fg_emphasis(),
                            ))
                            .child(render_overview_metric(
                                detail.changed_files.to_string(),
                                "Files changed",
                                fg_emphasis(),
                            ))
                            .child(render_overview_metric(
                                detail.comments_count.to_string(),
                                "Comments",
                                accent(),
                            ))
                            .child(render_change_meter(detail.additions, detail.deletions)),
                    ),
            )
            .child(review_button(action_label, move |_, window, cx| {
                enter_files_surface(&state, window, cx)
            })),
    )
}

fn render_overview_metric(value: String, label: &str, color: Rgba) -> impl IntoElement {
    div()
        .px(px(12.0))
        .py(px(10.0))
        .min_h(px(68.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .child(
            div()
                .text_size(px(13.0))
                .font_family(mono_font_family())
                .text_color(color)
                .child(value),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(10.0))
                .font_family(mono_font_family())
                .text_color(fg_subtle())
                .child(label.to_uppercase()),
        )
}

fn render_change_meter(additions: i64, deletions: i64) -> impl IntoElement {
    let additions = additions.max(0);
    let deletions = deletions.max(0);
    let total = additions + deletions;
    let segments = 10usize;
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
        .px(px(12.0))
        .py(px(10.0))
        .min_h(px(68.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .items_center()
                .font_family(mono_font_family())
                .text_size(px(12.0))
                .child(div().text_color(success()).child(format!("+{additions}")))
                .child(div().text_color(danger()).child(format!("-{deletions}"))),
        )
        .child(
            div()
                .mt(px(8.0))
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

                    div().w(px(10.0)).h(px(4.0)).rounded(px(2.0)).bg(bg)
                })),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(10.0))
                .font_family(mono_font_family())
                .text_color(fg_subtle())
                .child("DIFF".to_string()),
        )
}

fn render_review_snapshot_panel(
    detail: &github::PullRequestDetail,
    review_status: &ReviewStatusSummary,
    own_pr_feedback: &[OwnPrFeedbackItem],
    thread_digest: &[ThreadDigestItem],
    is_own_pull_request: bool,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let review_decision = detail.review_decision.clone();
    let highlight_count = if is_own_pull_request {
        format!("{} highlights", own_pr_feedback.len())
    } else {
        format!("{} threads", thread_digest.len())
    };
    let unresolved_feedback = own_pr_feedback
        .iter()
        .filter(|item| !item.is_resolved)
        .count();
    let unresolved_threads = thread_digest
        .iter()
        .filter(|item| !item.is_resolved)
        .count();
    let summary_text = if is_own_pull_request {
        build_own_pr_summary_text(review_status, own_pr_feedback)
    } else {
        build_review_snapshot_text(review_status, thread_digest, detail.comments_count as usize)
    };

    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
                .mb(px(14.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(if is_own_pull_request {
                            "Feedback Summary"
                        } else {
                            "Review Snapshot"
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(badge(&highlight_count))
                        .when_some(review_decision, |el, decision| {
                            el.child(review_decision_badge(&decision))
                        }),
                ),
        )
        .child(
            div()
                .max_w(px(760.0))
                .text_size(px(13.0))
                .line_height(px(20.0))
                .text_color(fg_default())
                .child(summary_text),
        )
        .child(
            div()
                .mt(px(16.0))
                .flex()
                .gap(px(10.0))
                .flex_wrap()
                .child(if is_own_pull_request {
                    render_snapshot_stat(
                        unresolved_feedback.to_string(),
                        "Needs reply",
                        "Reviewer threads still waiting on you.",
                        accent(),
                    )
                    .into_any_element()
                } else {
                    render_snapshot_stat(
                        unresolved_threads.to_string(),
                        "Open threads",
                        "Thread discussions still in progress.",
                        accent(),
                    )
                    .into_any_element()
                })
                .child(render_snapshot_stat(
                    review_status.waiting.len().to_string(),
                    "Waiting",
                    "Requested reviewers without a latest verdict.",
                    fg_muted(),
                ))
                .child(render_snapshot_stat(
                    review_status.approved.len().to_string(),
                    "Approved",
                    "Reviewers whose latest review is approval.",
                    success(),
                ))
                .child(render_snapshot_stat(
                    review_status.changes_requested.len().to_string(),
                    "Changes",
                    "Reviewers currently requesting updates.",
                    danger(),
                )),
        )
        .child(div().mt(px(18.0)).child(render_thread_focus_panel(
            own_pr_feedback,
            thread_digest,
            is_own_pull_request,
            state,
        )))
}

fn render_snapshot_stat(value: String, label: &str, hint: &str, color: Rgba) -> impl IntoElement {
    div()
        .p(px(14.0))
        .rounded(radius())
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .min_w(px(150.0))
        .max_w(px(188.0))
        .child(
            div()
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(mono_font_family())
                .text_color(color)
                .child(value),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg_emphasis())
                .child(label.to_string()),
        )
        .child(
            div()
                .mt(px(6.0))
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(fg_muted())
                .child(hint.to_string()),
        )
}

fn render_thread_focus_panel(
    own_pr_feedback: &[OwnPrFeedbackItem],
    thread_digest: &[ThreadDigestItem],
    is_own_pull_request: bool,
    state: &Entity<AppState>,
) -> AnyElement {
    if is_own_pull_request {
        let has_more = own_pr_feedback.len() > 4;

        div()
            .w_full()
            .min_w_0()
            .p(px(16.0))
            .rounded(radius())
            .bg(bg_subtle())
            .border_1()
            .border_color(border_muted())
            .child(eyebrow("Needs your attention"))
            .when(own_pr_feedback.is_empty(), |el| {
                el.child(panel_state_text("No reviewer comments yet."))
            })
            .child(
                div().flex().flex_col().gap(px(8.0)).children(
                    own_pr_feedback
                        .iter()
                        .take(4)
                        .map(|item| render_own_feedback_card(item, state)),
                ),
            )
            .when(has_more, |el| {
                el.child(
                    div()
                        .mt(px(10.0))
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(format!(
                            "{} more feedback thread{} in Files view.",
                            own_pr_feedback.len() - 4,
                            if own_pr_feedback.len() - 4 == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )),
                )
            })
            .into_any_element()
    } else {
        let has_more = thread_digest.len() > 4;

        div()
            .w_full()
            .min_w_0()
            .p(px(16.0))
            .rounded(radius())
            .bg(bg_subtle())
            .border_1()
            .border_color(border_muted())
            .child(eyebrow("Comment threads"))
            .when(thread_digest.is_empty(), |el| {
                el.child(panel_state_text("No review threads yet."))
            })
            .child(
                div().flex().flex_col().gap(px(8.0)).children(
                    thread_digest
                        .iter()
                        .take(4)
                        .map(|item| render_thread_digest_card(item, state)),
                ),
            )
            .when(has_more, |el| {
                el.child(
                    div()
                        .mt(px(10.0))
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(format!(
                            "{} more thread{} in Files view.",
                            thread_digest.len() - 4,
                            if thread_digest.len() - 4 == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )),
                )
            })
            .into_any_element()
    }
}

fn render_own_feedback_card(
    item: &OwnPrFeedbackItem,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let state = state.clone();
    let selected_file_path = item.file_path.clone();
    let selected_anchor = item.anchor.clone();
    let unread_comment_ids = item.unread_comment_ids.clone();
    let updated_at = format_relative_time(&item.updated_at);

    div()
        .min_w_0()
        .p(px(14.0))
        .rounded(radius_sm())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            state.update(cx, |state, cx| {
                state.mark_review_comments_read(unread_comment_ids.clone());
                state.selected_file_path = Some(selected_file_path.clone());
                state.selected_diff_anchor = Some(selected_anchor.clone());
                cx.notify();
            });
            enter_files_surface(&state, window, cx);
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.0))
                .min_w_0()
                .child(div().flex_grow().min_w_0().child(overflow_safe_code_label(
                    &item.location_label,
                    fg_emphasis(),
                )))
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .justify_end()
                        .flex_shrink_0()
                        .child(subtle_badge(&item.subject_type.to_lowercase()))
                        .when(item.is_resolved, |el| {
                            el.child(tone_badge(
                                "resolved",
                                success(),
                                success_muted(),
                                diff_add_border(),
                            ))
                        })
                        .when(item.is_outdated, |el| el.child(subtle_badge("outdated")))
                        .when(item.unread_count > 0, |el| {
                            el.child(tone_badge(
                                &format!("{} new", item.unread_count),
                                accent(),
                                accent_muted(),
                                accent(),
                            ))
                        })
                        .child(subtle_badge(&format!("{} feedback", item.feedback_count))),
                ),
        )
        .child(
            div()
                .mt(px(8.0))
                .text_size(px(13.0))
                .line_height(px(19.0))
                .text_color(fg_default())
                .child(render_markdown(
                    &format!(
                        "own-pr-feedback-preview-{}-{}",
                        item.file_path, item.updated_at
                    ),
                    &item.preview,
                )),
        )
        .child(
            div()
                .mt(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(user_avatar(
                    &item.author_login,
                    item.author_avatar_url.as_deref(),
                    18.0,
                    false,
                ))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_emphasis())
                        .child(item.author_login.clone()),
                )
                .child(format!("\u{2022} {updated_at}")),
        )
}

fn render_thread_digest_card(
    item: &ThreadDigestItem,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let state = state.clone();
    let selected_file_path = item.file_path.clone();
    let selected_anchor = item.anchor.clone();
    let unread_comment_ids = item.unread_comment_ids.clone();
    let updated_at = format_relative_time(&item.updated_at);
    let resolved_by = item.resolved_by_login.clone();

    div()
        .min_w_0()
        .p(px(14.0))
        .rounded(radius_sm())
        .bg(bg_overlay())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            state.update(cx, |state, cx| {
                state.mark_review_comments_read(unread_comment_ids.clone());
                state.selected_file_path = Some(selected_file_path.clone());
                state.selected_diff_anchor = Some(selected_anchor.clone());
                cx.notify();
            });
            enter_files_surface(&state, window, cx);
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.0))
                .min_w_0()
                .child(div().flex_grow().min_w_0().child(overflow_safe_code_label(
                    &item.location_label,
                    fg_emphasis(),
                )))
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .justify_end()
                        .flex_shrink_0()
                        .child(subtle_badge(&item.subject_type.to_lowercase()))
                        .when(item.is_resolved, |el| {
                            el.child(tone_badge(
                                resolved_by
                                    .as_deref()
                                    .map(|login| format!("resolved by {login}"))
                                    .unwrap_or_else(|| "resolved".to_string())
                                    .as_str(),
                                success(),
                                success_muted(),
                                diff_add_border(),
                            ))
                        })
                        .when(!item.is_resolved, |el| {
                            el.child(tone_badge("open", accent(), accent_muted(), accent()))
                        })
                        .when(item.is_outdated, |el| el.child(subtle_badge("outdated")))
                        .when(item.unread_count > 0, |el| {
                            el.child(tone_badge(
                                &format!("{} new", item.unread_count),
                                accent(),
                                accent_muted(),
                                accent(),
                            ))
                        })
                        .child(subtle_badge(&format!("{} comments", item.comment_count))),
                ),
        )
        .child(
            div()
                .mt(px(8.0))
                .text_size(px(13.0))
                .line_height(px(19.0))
                .text_color(fg_default())
                .child(render_markdown(
                    &format!(
                        "thread-digest-preview-{}-{}",
                        item.file_path, item.updated_at
                    ),
                    &item.preview,
                )),
        )
        .child(
            div()
                .mt(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(user_avatar(
                    &item.latest_author,
                    item.latest_author_avatar_url.as_deref(),
                    18.0,
                    false,
                ))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_emphasis())
                        .child(item.latest_author.clone()),
                )
                .child(format!("\u{2022} {updated_at}")),
        )
}

fn render_pull_request_summary_panel(
    detail: &github::PullRequestDetail,
    loaded_from_cache: bool,
    syncing: bool,
) -> impl IntoElement {
    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(16.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Summary"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(badge(if loaded_from_cache { "cache" } else { "live" }))
                        .when(syncing, |el| el.child(badge("refreshing"))),
                ),
        )
        .child(div().max_w(px(720.0)).child(if detail.body.is_empty() {
            div()
                .text_size(px(13.0))
                .text_color(fg_muted())
                .child("No PR description provided.")
                .into_any_element()
        } else {
            render_markdown("pr-summary-body", &detail.body).into_any_element()
        }))
}

fn render_recent_activity_panel(
    activity: &[ActivityItem],
    state: &Entity<AppState>,
) -> impl IntoElement {
    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(16.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Recent Activity"),
                )
                .child(badge(&activity.len().to_string())),
        )
        .when(activity.is_empty(), |el| {
            el.child(panel_state_text("No recent review or comment activity."))
        })
        .child(
            div().flex().flex_col().gap(px(8.0)).children(
                activity
                    .iter()
                    .take(10)
                    .map(|item| render_activity_card(item, state)),
            ),
        )
}

fn render_activity_card(item: &ActivityItem, state: &Entity<AppState>) -> impl IntoElement {
    let clickable = item.file_path.is_some() && item.anchor.is_some();
    let state = state.clone();
    let file_path = item.file_path.clone();
    let anchor = item.anchor.clone();
    let timestamp = format_relative_time(&item.timestamp);

    div()
        .min_w_0()
        .p(px(16.0))
        .rounded(radius())
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .when(clickable, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    state.update(cx, |state, cx| {
                        state.selected_file_path = file_path.clone();
                        state.selected_diff_anchor = anchor.clone();
                        cx.notify();
                    });
                    enter_files_surface(&state, window, cx);
                })
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.0))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .flex_grow()
                        .min_w_0()
                        .when(item.kind != ActivityItemKind::Thread, |el| {
                            el.child(activity_kind_badge(&item.kind))
                        })
                        .child(user_avatar(
                            &item.author_login,
                            item.author_avatar_url.as_deref(),
                            20.0,
                            false,
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(item.title.clone()),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(timestamp),
                ),
        )
        .child(
            div()
                .mt(px(8.0))
                .flex()
                .items_start()
                .gap(px(6.0))
                .flex_wrap()
                .min_w_0()
                .when_some(item.location_label.clone(), |el, location| {
                    el.child(
                        div()
                            .min_w_0()
                            .max_w(px(720.0))
                            .child(activity_location_text(&location)),
                    )
                })
                .when_some(item.status_label.clone(), |el, status| {
                    el.child(activity_status_badge(item, &status))
                }),
        )
        .when(!item.preview.is_empty(), |el| {
            el.child(
                div()
                    .mt(px(8.0))
                    .pl(px(10.0))
                    .border_l(px(2.0))
                    .border_color(transparent())
                    .text_size(px(14.0))
                    .line_height(px(21.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg_emphasis())
                    .child(SelectableText::new(
                        format!("activity-preview-{}-{}", item.author_login, item.timestamp),
                        item.preview.clone(),
                    )),
            )
        })
}

pub fn start_review_editor(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if s.review_loading {
            return;
        }
        s.review_editor_active = true;
        s.review_message = None;
        s.review_success = false;
        cx.notify();
    });
}

pub fn blur_review_editor(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if !s.review_editor_active {
            return;
        }
        s.review_editor_active = false;
        cx.notify();
    });
}

pub fn trigger_submit_review(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let Some((repository, number)) = state
        .read(cx)
        .active_pr()
        .map(|pr| (pr.repository.clone(), pr.number))
    else {
        return;
    };

    let (action, body, loading) = {
        let s = state.read(cx);
        (s.review_action, s.review_body.clone(), s.review_loading)
    };

    if loading {
        return;
    }

    if action == ReviewAction::Comment && body.trim().is_empty() {
        state.update(cx, |s, cx| {
            s.review_message = Some("Enter a review note before submitting a comment.".to_string());
            s.review_success = false;
            cx.notify();
        });
        return;
    }

    state.update(cx, |s, cx| {
        s.review_loading = true;
        s.review_message = None;
        s.review_success = false;
        cx.notify();
    });

    let model = state.clone();
    let repo = repository.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let submit_result = cx
                .background_executor()
                .spawn(async move {
                    github::submit_pull_request_review(&repository, number, action, &body)
                })
                .await;

            let (success, message) = match submit_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            model
                .update(cx, |s, cx| {
                    s.review_loading = false;
                    s.review_message = Some(message.clone());
                    s.review_success = success;
                    if success {
                        s.review_body.clear();
                        s.review_editor_active = false;
                    }
                    cx.notify();
                })
                .ok();

            if !success {
                return;
            }

            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let detail_key = pr_key(&repo, number);
            let repo_for_sync = repo.clone();
            let sync_result = cx
                .background_executor()
                .spawn(async move {
                    notifications::sync_pull_request_detail_with_read_state(
                        &cache,
                        &repo_for_sync,
                        number,
                    )
                })
                .await;

            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = false;
                    ds.syncing = false;
                    if let Ok((snapshot, unread_ids)) = sync_result {
                        ds.snapshot = Some(snapshot);
                        ds.error = None;
                        s.unread_review_comment_ids = unread_ids;
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

fn render_submit_review_panel(
    review_action: ReviewAction,
    review_body: String,
    review_editor_active: bool,
    review_loading: bool,
    review_message: Option<String>,
    review_success: bool,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let editor_state = state.clone();
    nested_panel()
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
                        .gap(px(4.0))
                        .child(eyebrow("Review action"))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Submit review"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child(
                                    "Write a review note here, then submit through gh without leaving the pull request.",
                                ),
                        ),
                )
                .child(badge(match review_action {
                    ReviewAction::Approve => "approve",
                    ReviewAction::Comment => "comment",
                    ReviewAction::RequestChanges => "request changes",
                })),
        )
        .child(
            div().flex().gap(px(4.0)).flex_wrap().children(
                [
                    (ReviewAction::Comment, "Comment"),
                    (ReviewAction::Approve, "Approve"),
                    (ReviewAction::RequestChanges, "Request changes"),
                ]
                .iter()
                .map(|(action, label)| {
                    let is_active = review_action == *action;
                    let action = *action;
                    let state = state.clone();
                    surface_tab(label, is_active, move |_, _, cx| {
                        state.update(cx, |s, cx| {
                            s.review_action = action;
                            cx.notify();
                        });
                    })
                }),
            ),
        )
        .child(
            div()
                .mt(px(12.0))
                .p(px(12.0))
                .px(px(14.0))
                .rounded(radius_sm())
                .border_1()
                .border_color(transparent())
                .bg(if review_editor_active {
                    bg_overlay()
                } else {
                    bg_subtle()
                })
                .cursor(CursorStyle::IBeam)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    start_review_editor(&editor_state, cx);
                })
                .text_color(if review_body.is_empty() {
                    fg_subtle()
                } else {
                    fg_default()
                })
                .text_size(px(14.0))
                .min_h(px(120.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family(mono_font_family())
                                .text_color(if review_editor_active {
                                    accent()
                                } else {
                                    fg_subtle()
                                })
                                .child(if review_editor_active {
                                    "EDITING"
                                } else {
                                    "CLICK TO EDIT"
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family(mono_font_family())
                                .text_color(fg_subtle())
                                .child("cmd-enter submit • esc blur"),
                        ),
                )
                .child(
                    div()
                        .mt(px(10.0))
                        .child(
                            AppTextInput::new(
                                "review-body-input",
                                state.clone(),
                                AppTextFieldKind::ReviewBody,
                                "Leave a review note...",
                            )
                            .autofocus(review_editor_active),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(10.0))
                .items_center()
                .justify_between()
                .flex_wrap()
                .mt(px(12.0))
                .child(review_button(
                    if review_loading {
                        "Submitting..."
                    } else {
                        "Submit review"
                    },
                    {
                        let state = state.clone();
                        move |_, window, cx| {
                            trigger_submit_review(&state, window, cx);
                        }
                    },
                ))
                .when_some(review_message, |el, msg| {
                    if review_success {
                        el.child(success_text(&msg))
                    } else {
                        el.child(error_text(&msg))
                    }
                }),
        )
}

fn render_details_panel(
    detail: &github::PullRequestDetail,
    fetched_at_ms: Option<i64>,
) -> impl IntoElement {
    let review_decision = detail.review_decision.as_deref().unwrap_or("PENDING");
    let completeness_warnings = if detail.data_completeness.is_complete() {
        Vec::new()
    } else {
        detail.data_completeness.warnings()
    };

    nested_panel()
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .mb(px(12.0))
                .child("Details"),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(detail_row(
                    "State",
                    detail_state_badge(&detail.state, detail.is_draft),
                ))
                .child(detail_row(
                    "Decision",
                    detail_review_decision_badge(review_decision),
                ))
                .child(detail_row(
                    "Created",
                    detail_value_text(&format_relative_time(&detail.created_at)),
                ))
                .child(detail_row(
                    "Updated",
                    detail_value_text(&format_relative_time(&detail.updated_at)),
                ))
                .child(detail_row(
                    "Comments",
                    detail_value_text(&detail.comments_count.to_string()),
                ))
                .child(detail_row(
                    "Threads",
                    detail_value_text(&detail.review_threads.len().to_string()),
                ))
                .child(detail_row(
                    "Files",
                    detail_value_text(&detail.changed_files.to_string()),
                ))
                .when_some(fetched_at_ms, |el, ms| {
                    el.child(detail_row("Cached at", detail_value_text(&format_ms(ms))))
                }),
        )
        .when(!completeness_warnings.is_empty(), |el| {
            el.child(
                div().mt(px(12.0)).flex().flex_col().gap(px(6.0)).children(
                    completeness_warnings
                        .into_iter()
                        .map(|warning| error_text(&warning)),
                ),
            )
        })
}

fn detail_row(label: &str, value: AnyElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
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
                .flex()
                .items_center()
                .child(value),
        )
}

fn detail_value_text(value: &str) -> AnyElement {
    div()
        .text_color(fg_emphasis())
        .font_weight(FontWeight::MEDIUM)
        .font_family(mono_font_family())
        .text_size(px(11.0))
        .whitespace_normal()
        .child(value.to_string())
        .into_any_element()
}

fn detail_badge(label: &str, fg: Rgba, bg: Rgba, _border: Rgba) -> AnyElement {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg)
        .text_size(px(11.0))
        .font_family(mono_font_family())
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.to_string())
        .into_any_element()
}

fn detail_state_badge(state: &str, is_draft: bool) -> AnyElement {
    let label = humanize_pull_request_state(state, is_draft);
    let (fg, bg, border) = pull_request_state_colors(state, is_draft);
    detail_badge(&label, fg, bg, border)
}

fn detail_review_decision_badge(decision: &str) -> AnyElement {
    let label = humanize_review_state(decision);
    let (fg, bg, border) = review_state_colors(decision);
    detail_badge(&label, fg, bg, border)
}

fn render_reviewers_panel(
    detail: &github::PullRequestDetail,
    review_status: &ReviewStatusSummary,
) -> impl IntoElement {
    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(12.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Reviewers"),
                )
                .child(badge(&detail.reviewers.len().to_string())),
        )
        .when(detail.reviewers.is_empty(), |el| {
            el.child(panel_state_text("No reviewers requested."))
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(detail.reviewers.iter().map(|reviewer| {
                    let avatar_url = detail
                        .reviewer_avatar_urls
                        .get(reviewer)
                        .map(String::as_str);
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .flex_grow()
                                .min_w_0()
                                .child(user_avatar(reviewer, avatar_url, 28.0, false))
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(participant_display_name(reviewer)),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .child(reviewer_status_badge(reviewer, review_status)),
                        )
                })),
        )
}

fn render_participants_panel(participants: &[ParticipantItem]) -> impl IntoElement {
    let has_more = participants.len() > 8;

    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(12.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Participants"),
                )
                .child(badge(&participants.len().to_string())),
        )
        .when(participants.is_empty(), |el| {
            el.child(panel_state_text("No participant activity yet."))
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(participants.iter().take(8).map(render_participant_row)),
        )
        .when(has_more, |el| {
            el.child(
                div()
                    .mt(px(10.0))
                    .text_size(px(12.0))
                    .text_color(fg_muted())
                    .child(format!("+{} more participants", participants.len() - 8)),
            )
        })
}

fn render_participant_row(participant: &ParticipantItem) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .min_w_0()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .flex_grow()
                .min_w_0()
                .child(user_avatar(
                    &participant.login,
                    participant.avatar_url.as_deref(),
                    28.0,
                    participant.is_author,
                ))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_emphasis())
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(participant_display_name(&participant.login)),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(4.0))
                .flex_wrap()
                .justify_end()
                .flex_shrink_0()
                .when(participant.is_author, |el| {
                    el.child(tone_badge("author", accent(), accent_muted(), accent()))
                })
                .when(
                    participant.is_requested
                        && !participant.approved
                        && !participant.changes_requested,
                    |el| el.child(subtle_badge("requested")),
                )
                .when(participant.approved, |el| {
                    el.child(tone_badge(
                        "approved",
                        success(),
                        success_muted(),
                        diff_add_border(),
                    ))
                })
                .when(participant.changes_requested, |el| {
                    el.child(tone_badge(
                        "changes",
                        danger(),
                        danger_muted(),
                        diff_remove_border(),
                    ))
                })
                .when(
                    participant.commented
                        && !participant.approved
                        && !participant.changes_requested,
                    |el| el.child(subtle_badge("commented")),
                ),
        )
}

fn render_labels_panel(labels: &[String]) -> impl IntoElement {
    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Labels"),
                )
                .child(badge(&labels.len().to_string())),
        )
        .when(labels.is_empty(), |el| {
            el.child(panel_state_text("No labels."))
        })
        .child(
            div()
                .flex()
                .gap(px(4.0))
                .flex_wrap()
                .mt(px(6.0))
                .children(labels.iter().map(|label| badge(label))),
        )
}

fn participant_display_name(login: &str) -> String {
    let max_chars = 18usize;
    if login.chars().count() <= max_chars {
        return login.to_string();
    }

    let segments = login
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if let (Some(first), Some(last)) = (segments.first(), segments.last()) {
        let compact = format!("{first}-{last}");
        if compact.chars().count() <= max_chars {
            return compact;
        }

        let compact_with_gap = format!("{first}-...-{last}");
        if compact_with_gap.chars().count() <= max_chars {
            return compact_with_gap;
        }
    }

    let mut shortened = login
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    shortened.push_str("...");
    shortened
}

fn overflow_safe_code_label(label: &str, color: Rgba) -> impl IntoElement {
    div()
        .min_w_0()
        .font_family(mono_font_family())
        .text_size(px(12.0))
        .text_color(color)
        .text_ellipsis()
        .whitespace_nowrap()
        .overflow_x_hidden()
        .child(label.to_string())
}

fn tone_badge(label: &str, fg: Rgba, bg: Rgba, border: Rgba) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.to_string())
}

fn subtle_badge(label: &str) -> impl IntoElement {
    tone_badge(label, fg_muted(), bg_emphasis(), border_muted())
}

fn activity_kind_badge(kind: &ActivityItemKind) -> AnyElement {
    match kind {
        ActivityItemKind::Conversation => {
            tone_badge("comment", accent(), accent_muted(), accent()).into_any_element()
        }
        ActivityItemKind::Review => {
            tone_badge("review", fg_emphasis(), bg_emphasis(), border_muted()).into_any_element()
        }
        ActivityItemKind::Thread => {
            tone_badge("thread", fg_muted(), bg_emphasis(), border_muted()).into_any_element()
        }
    }
}

fn activity_location_text(location: &str) -> AnyElement {
    overflow_safe_code_label(location, fg_subtle()).into_any_element()
}

fn activity_status_badge(item: &ActivityItem, status: &str) -> AnyElement {
    if let Some(code) = item.status_code.as_deref() {
        let (fg, bg, border) = review_state_colors(code);
        return tone_badge(status, fg, bg, border).into_any_element();
    }

    subtle_badge(status).into_any_element()
}

fn pull_request_state_badge(state: &str, is_draft: bool) -> AnyElement {
    let label = humanize_pull_request_state(state, is_draft);
    let (fg, bg, border) = pull_request_state_colors(state, is_draft);
    tone_badge(&label, fg, bg, border).into_any_element()
}

fn review_decision_badge(decision: &str) -> AnyElement {
    let label = humanize_review_state(decision);
    let (fg, bg, border) = review_state_colors(decision);
    tone_badge(&label, fg, bg, border).into_any_element()
}

fn reviewer_status_badge(login: &str, review_status: &ReviewStatusSummary) -> AnyElement {
    if review_status.approved.iter().any(|name| name == login) {
        return tone_badge("approved", success(), success_muted(), diff_add_border())
            .into_any_element();
    }
    if review_status
        .changes_requested
        .iter()
        .any(|name| name == login)
    {
        return tone_badge("changes", danger(), danger_muted(), diff_remove_border())
            .into_any_element();
    }
    if review_status.commented.iter().any(|name| name == login) {
        return tone_badge("commented", accent(), accent_muted(), accent()).into_any_element();
    }
    subtle_badge("waiting").into_any_element()
}

fn humanize_pull_request_state(state: &str, is_draft: bool) -> String {
    if is_draft {
        return "Draft".to_string();
    }
    match state {
        "MERGED" => "Merged".to_string(),
        "CLOSED" => "Closed".to_string(),
        "OPEN" => "Open".to_string(),
        _ => state.to_string(),
    }
}

fn humanize_review_state(state: &str) -> String {
    match state {
        "APPROVED" => "Approved".to_string(),
        "CHANGES_REQUESTED" => "Changes requested".to_string(),
        "COMMENTED" => "Commented".to_string(),
        "PENDING" => "Pending".to_string(),
        "REVIEW_REQUIRED" => "Needs review".to_string(),
        "DISMISSED" => "Dismissed".to_string(),
        _ => state.to_string(),
    }
}

fn pull_request_state_colors(state: &str, is_draft: bool) -> (Rgba, Rgba, Rgba) {
    if is_draft {
        return (fg_muted(), bg_emphasis(), border_muted());
    }

    match state {
        "MERGED" => (info(), info_muted(), info()),
        "CLOSED" => (danger(), danger_muted(), diff_remove_border()),
        _ => (success(), success_muted(), diff_add_border()),
    }
}

fn review_state_colors(state: &str) -> (Rgba, Rgba, Rgba) {
    match state {
        "APPROVED" => (success(), success_muted(), diff_add_border()),
        "CHANGES_REQUESTED" => (danger(), danger_muted(), diff_remove_border()),
        "COMMENTED" => (accent(), accent_muted(), accent()),
        "PENDING" => (fg_muted(), bg_emphasis(), border_muted()),
        "REVIEW_REQUIRED" => (fg_muted(), bg_emphasis(), border_muted()),
        _ => (fg_muted(), bg_emphasis(), border_muted()),
    }
}

fn build_own_pr_summary_text(
    review_status: &ReviewStatusSummary,
    own_pr_feedback: &[OwnPrFeedbackItem],
) -> String {
    let unresolved_feedback = own_pr_feedback
        .iter()
        .filter(|item| !item.is_resolved)
        .count();
    let waiting = review_status.waiting.len();
    let approvals = review_status.approved.len();
    let changes_requested = review_status.changes_requested.len();

    format!(
        "{} {}, {} {}, {} {}, and {} {}.",
        unresolved_feedback,
        count_copy(
            unresolved_feedback,
            "thread needs your reply",
            "threads need your reply"
        ),
        waiting,
        count_copy(
            waiting,
            "reviewer is still waiting",
            "reviewers are still waiting"
        ),
        approvals,
        count_copy(approvals, "approval is in", "approvals are in"),
        changes_requested,
        count_copy(
            changes_requested,
            "reviewer is requesting changes",
            "reviewers are requesting changes",
        ),
    )
}

fn build_review_snapshot_text(
    review_status: &ReviewStatusSummary,
    thread_digest: &[ThreadDigestItem],
    comments_count: usize,
) -> String {
    let unresolved_threads = thread_digest
        .iter()
        .filter(|item| !item.is_resolved)
        .count();
    let responded = review_status.approved.len()
        + review_status.changes_requested.len()
        + review_status.commented.len();

    format!(
        "{} {}, {} {}, and {} {} so far.",
        unresolved_threads,
        count_copy(
            unresolved_threads,
            "thread is still open",
            "threads are still open"
        ),
        comments_count,
        count_copy(
            comments_count,
            "conversation comment is on the PR",
            "conversation comments are on the PR",
        ),
        responded,
        count_copy(
            responded,
            "reviewer has responded",
            "reviewers have responded",
        ),
    )
}

fn summarize_thread_activity(
    review_threads: &[PullRequestReviewThread],
    unread_comment_ids: &BTreeSet<String>,
) -> Vec<ThreadDigestItem> {
    let mut items = review_threads
        .iter()
        .filter_map(|thread| thread_digest_item(thread, unread_comment_ids))
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        left.is_resolved
            .cmp(&right.is_resolved)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.location_label.cmp(&right.location_label))
    });
    items
}

fn thread_digest_item(
    thread: &PullRequestReviewThread,
    unread_comment_ids: &BTreeSet<String>,
) -> Option<ThreadDigestItem> {
    let anchor = review_thread_anchor(thread)?;
    let location_label = feedback_location_label(thread, &anchor);
    let latest_comment = thread.comments.iter().rev().find(|comment| {
        !comment.author_login.trim().is_empty() || !comment.body.trim().is_empty()
    })?;
    let unread_comment_ids = thread_unread_comment_ids(thread, unread_comment_ids);

    Some(ThreadDigestItem {
        anchor,
        file_path: thread.path.clone(),
        location_label,
        latest_author: latest_comment.author_login.clone(),
        latest_author_avatar_url: latest_comment.author_avatar_url.clone(),
        updated_at: latest_comment
            .published_at
            .clone()
            .unwrap_or_else(|| latest_comment.updated_at.clone()),
        preview: summarize_feedback_preview(latest_comment),
        subject_type: thread.subject_type.clone(),
        comment_count: thread.comments.len(),
        unread_count: unread_comment_ids.len(),
        unread_comment_ids,
        is_resolved: thread.is_resolved,
        is_outdated: thread.is_outdated,
        resolved_by_login: thread.resolved_by_login.clone(),
    })
}

fn thread_unread_comment_ids(
    thread: &PullRequestReviewThread,
    unread_comment_ids: &BTreeSet<String>,
) -> Vec<String> {
    thread
        .comments
        .iter()
        .filter(|comment| unread_comment_ids.contains(&comment.id))
        .map(|comment| comment.id.clone())
        .collect()
}

fn summarize_recent_activity(
    detail: &github::PullRequestDetail,
    unread_comment_ids: &BTreeSet<String>,
) -> Vec<ActivityItem> {
    let mut items = detail
        .comments
        .iter()
        .map(activity_item_for_comment)
        .collect::<Vec<_>>();

    items.extend(detail.latest_reviews.iter().map(activity_item_for_review));
    items.extend(
        detail
            .review_threads
            .iter()
            .filter_map(|thread| activity_item_for_thread(thread, unread_comment_ids)),
    );

    items.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.title.cmp(&right.title))
    });
    items
}

fn activity_item_for_comment(comment: &PullRequestComment) -> ActivityItem {
    ActivityItem {
        kind: ActivityItemKind::Conversation,
        author_login: comment.author_login.clone(),
        author_avatar_url: comment.author_avatar_url.clone(),
        timestamp: comment.updated_at.clone(),
        title: format!("{} commented on the pull request", comment.author_login),
        preview: summarize_text_preview(&comment.body, 220),
        status_label: None,
        status_code: None,
        location_label: None,
        file_path: None,
        anchor: None,
    }
}

fn activity_item_for_review(review: &PullRequestReview) -> ActivityItem {
    ActivityItem {
        kind: ActivityItemKind::Review,
        author_login: review.author_login.clone(),
        author_avatar_url: review.author_avatar_url.clone(),
        timestamp: review.submitted_at.clone().unwrap_or_default(),
        title: format!(
            "{} {}",
            review.author_login,
            match review.state.as_str() {
                "APPROVED" => "approved the changes",
                "CHANGES_REQUESTED" => "requested changes",
                _ => "left a review",
            }
        ),
        preview: review_activity_preview(review),
        status_label: Some(humanize_review_state(&review.state)),
        status_code: Some(review.state.clone()),
        location_label: None,
        file_path: None,
        anchor: None,
    }
}

fn activity_item_for_thread(
    thread: &PullRequestReviewThread,
    unread_comment_ids: &BTreeSet<String>,
) -> Option<ActivityItem> {
    let digest = thread_digest_item(thread, unread_comment_ids)?;
    let mut status_parts = Vec::new();
    if digest.unread_count > 0 {
        status_parts.push(format!("{} new", digest.unread_count));
    }
    if digest.is_resolved {
        status_parts.push("Resolved".to_string());
    }
    if digest.is_outdated {
        status_parts.push("Outdated".to_string());
    }

    Some(ActivityItem {
        kind: ActivityItemKind::Thread,
        author_login: digest.latest_author.clone(),
        author_avatar_url: digest.latest_author_avatar_url.clone(),
        timestamp: digest.updated_at.clone(),
        title: format!("{} commented", digest.latest_author),
        preview: digest.preview.clone(),
        status_label: if status_parts.is_empty() {
            Some(format!("{} comments", digest.comment_count))
        } else {
            Some(status_parts.join(" \u{2022} "))
        },
        status_code: None,
        location_label: Some(digest.location_label.clone()),
        file_path: Some(digest.file_path),
        anchor: Some(digest.anchor),
    })
}

fn review_activity_preview(review: &PullRequestReview) -> String {
    let body = review.body.trim();
    if body.is_empty() {
        return String::new();
    }

    summarize_text_preview(body, 220)
}

fn summarize_participants(
    detail: &github::PullRequestDetail,
    review_status: &ReviewStatusSummary,
) -> Vec<ParticipantItem> {
    let mut participants = BTreeMap::<String, ParticipantItem>::new();
    let review_avatar_urls = detail
        .latest_reviews
        .iter()
        .filter_map(|review| {
            Some((
                review.author_login.as_str(),
                review.author_avatar_url.as_deref()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();

    let mut upsert = |login: &str, avatar_url: Option<&str>, apply: fn(&mut ParticipantItem)| {
        if login.trim().is_empty() {
            return;
        }
        let entry = participants
            .entry(login.to_string())
            .or_insert_with(|| ParticipantItem {
                login: login.to_string(),
                avatar_url: None,
                is_author: false,
                is_requested: false,
                approved: false,
                changes_requested: false,
                commented: false,
            });
        if entry.avatar_url.is_none() {
            entry.avatar_url = avatar_url
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string);
        }
        apply(entry);
    };

    upsert(
        &detail.author_login,
        detail.author_avatar_url.as_deref(),
        |participant| participant.is_author = true,
    );

    for reviewer in &detail.reviewers {
        upsert(
            reviewer,
            detail
                .reviewer_avatar_urls
                .get(reviewer)
                .map(String::as_str),
            |participant| participant.is_requested = true,
        );
    }
    for login in &review_status.approved {
        upsert(
            login,
            review_avatar_urls.get(login.as_str()).copied(),
            |participant| participant.approved = true,
        );
    }
    for login in &review_status.changes_requested {
        upsert(
            login,
            review_avatar_urls.get(login.as_str()).copied(),
            |participant| participant.changes_requested = true,
        );
    }
    for login in &review_status.commented {
        upsert(
            login,
            review_avatar_urls.get(login.as_str()).copied(),
            |participant| participant.commented = true,
        );
    }
    for comment in &detail.comments {
        upsert(
            &comment.author_login,
            comment.author_avatar_url.as_deref(),
            |participant| participant.commented = true,
        );
    }
    for thread in &detail.review_threads {
        for comment in &thread.comments {
            upsert(
                &comment.author_login,
                comment.author_avatar_url.as_deref(),
                |participant| participant.commented = true,
            );
        }
    }

    let mut items = participants.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .is_author
            .cmp(&left.is_author)
            .then_with(|| right.changes_requested.cmp(&left.changes_requested))
            .then_with(|| right.approved.cmp(&left.approved))
            .then_with(|| right.is_requested.cmp(&left.is_requested))
            .then_with(|| left.login.cmp(&right.login))
    });
    items
}

fn summarize_text_preview(text: &str, limit: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "No comment body.".to_string();
    }

    let mut preview = collapsed.chars().take(limit).collect::<String>();
    if collapsed.chars().count() > limit {
        preview.push('…');
    }
    preview
}

fn count_copy(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        singular.to_string()
    } else {
        plural.to_string()
    }
}

pub fn surface_tab(
    label: &str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .text_size(px(12.0))
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
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
}

fn trigger_sync_pr(
    state: &Entity<AppState>,
    repository: &str,
    number: i64,
    window: &mut Window,
    cx: &mut App,
) {
    let key = pr_key(repository, number);
    let already_syncing = state
        .read(cx)
        .detail_states
        .get(&key)
        .map(|detail_state| detail_state.syncing)
        .unwrap_or(false);
    if already_syncing {
        return;
    }

    let model = state.clone();
    let repo = repository.to_string();

    state.update(cx, |s, cx| {
        let ds = s.detail_states.entry(key.clone()).or_default();
        ds.loading = ds
            .snapshot
            .as_ref()
            .and_then(|sn| sn.detail.as_ref())
            .is_none();
        ds.syncing = true;
        ds.error = None;
        cx.notify();
    });

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };

            let result = cx
                .background_executor()
                .spawn(async move {
                    notifications::sync_pull_request_detail_with_read_state(&cache, &repo, number)
                })
                .await;

            let detail_key = key;
            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = false;
                    ds.syncing = false;
                    match result {
                        Ok((snapshot, unread_ids)) => {
                            ds.snapshot = Some(snapshot);
                            ds.error = None;
                            s.unread_review_comment_ids = unread_ids;
                        }
                        Err(e) => ds.error = Some(e),
                    }
                    cx.notify();
                })
                .ok();

            let should_refresh_tour = model
                .read_with(cx, |s, _| {
                    s.active_surface == PullRequestSurface::Files
                        && s.active_pr_key.as_deref() == Some(&detail_key)
                        && s.active_review_session()
                            .map(|session| session.center_mode == ReviewCenterMode::AiTour)
                            .unwrap_or(false)
                })
                .ok()
                .unwrap_or(false);

            if should_refresh_tour {
                refresh_active_tour_flow(model.clone(), true, cx).await;
            }
        })
        .detach();
}

fn open_pull_request_in_browser(repository: &str, number: i64, window: &mut Window, cx: &mut App) {
    let repository = repository.to_string();

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    crate::gh::run_owned(vec![
                        "pr".to_string(),
                        "view".to_string(),
                        number.to_string(),
                        "--repo".to_string(),
                        repository,
                        "--web".to_string(),
                    ])
                })
                .await;
        })
        .detach();
}

fn format_ms(ms: i64) -> String {
    let secs = ms / 1000;
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    format!("{hours:02}:{minutes:02}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        humanize_review_state, participant_display_name, summarize_own_pr_feedback,
        summarize_participants, summarize_recent_activity, summarize_review_status,
        ActivityItemKind,
    };
    use crate::github::{
        PullRequestComment, PullRequestDetail, PullRequestFile, PullRequestReview,
        PullRequestReviewComment, PullRequestReviewThread,
    };

    #[test]
    fn summarize_review_status_groups_latest_outcomes() {
        let summary = summarize_review_status(
            &[
                "zoe".to_string(),
                "alice".to_string(),
                "bob".to_string(),
                "sam".to_string(),
            ],
            &[
                review("alice", "COMMENTED", None),
                review("alice", "APPROVED", None),
                review("bob", "CHANGES_REQUESTED", None),
                review("carol", "COMMENTED", None),
                review("", "APPROVED", None),
            ],
        );

        assert_eq!(summary.approved, vec!["alice".to_string()]);
        assert_eq!(summary.changes_requested, vec!["bob".to_string()]);
        assert_eq!(summary.commented, vec!["carol".to_string()]);
        assert_eq!(summary.waiting, vec!["sam".to_string(), "zoe".to_string()]);
    }

    #[test]
    fn summarize_own_pr_feedback_prioritizes_unresolved_external_threads() {
        let items = summarize_own_pr_feedback(
            &[
                line_thread(
                    "thread-1",
                    "src/main.rs",
                    24,
                    false,
                    false,
                    vec![
                        comment("author", "I think this is fine", "2026-04-14T08:00:00Z"),
                        comment(
                            "reviewer-a",
                            "Please add a null check before this branch.\n\nIt currently panics.",
                            "2026-04-14T09:00:00Z",
                        ),
                        comment("author", "Pushed a fix", "2026-04-14T09:10:00Z"),
                    ],
                ),
                file_thread(
                    "thread-2",
                    "README.md",
                    true,
                    true,
                    vec![comment(
                        "reviewer-b",
                        "The onboarding note is stale and should mention the managed checkout flow.",
                        "2026-04-14T10:30:00Z",
                    )],
                ),
                line_thread(
                    "thread-3",
                    "src/lib.rs",
                    8,
                    false,
                    false,
                    vec![comment(
                        "author",
                        "I already addressed this in a follow-up commit.",
                        "2026-04-14T11:00:00Z",
                    )],
                ),
            ],
            "author",
            &BTreeSet::new(),
        );

        assert_eq!(items.len(), 2);

        assert_eq!(items[0].file_path, "src/main.rs");
        assert_eq!(items[0].location_label, "src/main.rs:24");
        assert_eq!(items[0].author_login, "reviewer-a");
        assert_eq!(items[0].feedback_count, 1);
        assert_eq!(items[0].anchor.line, Some(24));
        assert_eq!(items[0].anchor.side.as_deref(), Some("RIGHT"));
        assert_eq!(
            items[0].preview,
            "Please add a null check before this branch.\n\nIt currently panics."
        );
        assert!(!items[0].is_resolved);
        assert!(!items[0].is_outdated);

        assert_eq!(items[1].file_path, "README.md");
        assert_eq!(items[1].location_label, "README.md");
        assert_eq!(items[1].author_login, "reviewer-b");
        assert_eq!(items[1].feedback_count, 1);
        assert_eq!(items[1].anchor.line, None);
        assert!(items[1].is_resolved);
        assert!(items[1].is_outdated);
    }

    #[test]
    fn summarize_recent_activity_sorts_conversation_reviews_and_threads() {
        let detail = detail_with_activity(
            vec![issue_comment(
                "alice",
                "Left a top-level conversation comment.",
                "2026-04-14T09:00:00Z",
            )],
            vec![review("bob", "APPROVED", Some("2026-04-14T10:00:00Z"))],
            vec![line_thread(
                "thread-activity",
                "src/main.rs",
                42,
                false,
                false,
                vec![comment(
                    "carol",
                    "Please rename this helper so the intent is clearer.",
                    "2026-04-14T11:00:00Z",
                )],
            )],
        );

        let items = summarize_recent_activity(&detail, &BTreeSet::new());

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, ActivityItemKind::Thread);
        assert_eq!(items[0].title, "carol commented");
        assert_eq!(items[0].location_label.as_deref(), Some("src/main.rs:42"));
        assert_eq!(items[1].kind, ActivityItemKind::Review);
        assert_eq!(items[1].status_code.as_deref(), Some("APPROVED"));
        assert!(items[1].preview.is_empty());
        assert_eq!(items[2].kind, ActivityItemKind::Conversation);
    }

    #[test]
    fn summarize_participants_marks_requested_reviewers_and_commenters() {
        let detail = detail_with_activity(
            vec![issue_comment(
                "erin",
                "Needs a follow-up note in the PR body.",
                "2026-04-14T09:00:00Z",
            )],
            vec![
                review("alice", "APPROVED", Some("2026-04-14T10:00:00Z")),
                review("bob", "CHANGES_REQUESTED", Some("2026-04-14T10:30:00Z")),
                review("dave", "COMMENTED", Some("2026-04-14T11:00:00Z")),
            ],
            vec![line_thread(
                "thread-participants",
                "src/lib.rs",
                8,
                false,
                false,
                vec![comment(
                    "frank",
                    "This branch still needs a guard clause.",
                    "2026-04-14T11:15:00Z",
                )],
            )],
        );

        let review_status = summarize_review_status(&detail.reviewers, &detail.latest_reviews);
        let participants = summarize_participants(&detail, &review_status);

        let author = participants
            .iter()
            .find(|participant| participant.login == "author");
        let alice = participants
            .iter()
            .find(|participant| participant.login == "alice");
        let bob = participants
            .iter()
            .find(|participant| participant.login == "bob");
        let erin = participants
            .iter()
            .find(|participant| participant.login == "erin");
        let frank = participants
            .iter()
            .find(|participant| participant.login == "frank");

        assert!(author.is_some_and(|participant| participant.is_author));
        assert!(alice.is_some_and(|participant| participant.is_requested && participant.approved));
        assert!(bob.is_some_and(|participant| {
            participant.is_requested && participant.changes_requested
        }));
        assert!(erin.is_some_and(|participant| participant.commented));
        assert!(frank.is_some_and(|participant| participant.commented));
    }

    #[test]
    fn humanize_review_state_formats_pending() {
        assert_eq!(humanize_review_state("PENDING"), "Pending");
    }

    #[test]
    fn participant_display_name_compacts_long_hyphenated_logins() {
        assert_eq!(
            participant_display_name("copilot-pull-request-reviewer"),
            "copilot-reviewer"
        );
    }

    fn review(author_login: &str, state: &str, submitted_at: Option<&str>) -> PullRequestReview {
        PullRequestReview {
            author_login: author_login.to_string(),
            author_avatar_url: None,
            state: state.to_string(),
            body: String::new(),
            submitted_at: submitted_at.map(str::to_string),
        }
    }

    fn issue_comment(author_login: &str, body: &str, timestamp: &str) -> PullRequestComment {
        PullRequestComment {
            id: format!("issue-comment-{author_login}-{timestamp}"),
            author_login: author_login.to_string(),
            author_avatar_url: None,
            body: body.to_string(),
            created_at: timestamp.to_string(),
            updated_at: timestamp.to_string(),
            url: "https://example.com/issue-comment".to_string(),
        }
    }

    fn comment(author_login: &str, body: &str, timestamp: &str) -> PullRequestReviewComment {
        PullRequestReviewComment {
            id: format!("comment-{author_login}-{timestamp}"),
            author_login: author_login.to_string(),
            author_avatar_url: None,
            body: body.to_string(),
            path: String::new(),
            line: None,
            original_line: None,
            start_line: None,
            original_start_line: None,
            state: "SUBMITTED".to_string(),
            created_at: timestamp.to_string(),
            updated_at: timestamp.to_string(),
            published_at: Some(timestamp.to_string()),
            reply_to_id: None,
            url: "https://example.com/comment".to_string(),
        }
    }

    fn line_thread(
        id: &str,
        path: &str,
        line: i64,
        is_resolved: bool,
        is_outdated: bool,
        comments: Vec<PullRequestReviewComment>,
    ) -> PullRequestReviewThread {
        PullRequestReviewThread {
            id: id.to_string(),
            path: path.to_string(),
            line: Some(line),
            original_line: Some(line),
            start_line: None,
            original_start_line: None,
            diff_side: "RIGHT".to_string(),
            start_diff_side: None,
            is_collapsed: false,
            is_outdated,
            is_resolved,
            subject_type: "LINE".to_string(),
            resolved_by_login: None,
            viewer_can_reply: true,
            viewer_can_resolve: true,
            viewer_can_unresolve: false,
            comments,
        }
    }

    fn file_thread(
        id: &str,
        path: &str,
        is_resolved: bool,
        is_outdated: bool,
        comments: Vec<PullRequestReviewComment>,
    ) -> PullRequestReviewThread {
        PullRequestReviewThread {
            id: id.to_string(),
            path: path.to_string(),
            line: None,
            original_line: None,
            start_line: None,
            original_start_line: None,
            diff_side: String::new(),
            start_diff_side: None,
            is_collapsed: false,
            is_outdated,
            is_resolved,
            subject_type: "FILE".to_string(),
            resolved_by_login: None,
            viewer_can_reply: true,
            viewer_can_resolve: true,
            viewer_can_unresolve: false,
            comments,
        }
    }

    fn detail_with_activity(
        comments: Vec<PullRequestComment>,
        latest_reviews: Vec<PullRequestReview>,
        review_threads: Vec<PullRequestReviewThread>,
    ) -> PullRequestDetail {
        PullRequestDetail {
            id: "detail-1".to_string(),
            repository: "acme/widgets".to_string(),
            number: 42,
            title: "Improve review summary".to_string(),
            body: String::new(),
            url: "https://example.com/pr/42".to_string(),
            author_login: "author".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature/review-summary".to_string(),
            base_ref_oid: None,
            head_ref_oid: None,
            additions: 24,
            deletions: 8,
            changed_files: 3,
            comments_count: comments.len() as i64,
            commits_count: 2,
            created_at: "2026-04-14T08:00:00Z".to_string(),
            updated_at: "2026-04-14T11:30:00Z".to_string(),
            labels: vec!["ui".to_string()],
            reviewers: vec!["alice".to_string(), "bob".to_string()],
            reviewer_avatar_urls: std::collections::BTreeMap::new(),
            comments,
            latest_reviews,
            review_threads,
            files: vec![PullRequestFile {
                path: "src/main.rs".to_string(),
                additions: 12,
                deletions: 4,
                change_type: "MODIFIED".to_string(),
            }],
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        }
    }
}
