use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use crate::code_tour::review_thread_anchor;
use crate::github::{
    self, PullRequestReview, PullRequestReviewComment, PullRequestReviewThread, ReviewAction,
};
use crate::markdown::render_markdown;
use crate::state::*;
use crate::theme::*;

use super::diff_view::{enter_files_surface, render_files_view};
use super::sections::{
    badge, badge_success, error_text, eyebrow, ghost_button, meta_row, nested_panel,
    panel_state_text, review_button, success_text,
};
use super::tour_view::{enter_tour_surface, refresh_active_tour_flow, render_tour_view};

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
    updated_at: String,
    preview: String,
    subject_type: String,
    feedback_count: usize,
    is_resolved: bool,
    is_outdated: bool,
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
) -> Vec<OwnPrFeedbackItem> {
    let viewer_login = viewer_login.trim();
    let mut items = review_threads
        .iter()
        .filter_map(|thread| own_pr_feedback_item(thread, viewer_login))
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

    Some(OwnPrFeedbackItem {
        file_path: thread.path.clone(),
        location_label: feedback_location_label(thread, &anchor),
        author_login: latest_feedback.author_login.clone(),
        updated_at: latest_feedback
            .published_at
            .clone()
            .unwrap_or_else(|| latest_feedback.updated_at.clone()),
        preview: summarize_feedback_preview(latest_feedback),
        subject_type: thread.subject_type.clone(),
        feedback_count,
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
    let collapsed = comment
        .body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        return "No comment body.".to_string();
    }

    let mut preview = collapsed.chars().take(160).collect::<String>();
    if collapsed.chars().count() > 160 {
        preview.push('…');
    }
    preview
}

fn viewer_login(state: &AppState) -> Option<String> {
    state
        .workspace
        .as_ref()
        .and_then(|workspace| {
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
    let author = detail
        .map(|d| d.author_login.clone())
        .unwrap_or_else(|| pr.author_login.clone());
    let repository = pr.repository.clone();
    let number = pr.number;
    let loading = detail_state.map(|d| d.loading).unwrap_or(false);
    let syncing = detail_state.map(|d| d.syncing).unwrap_or(false);
    let error = detail_state.and_then(|d| d.error.clone());
    let show_loading_state = detail.is_none() && (loading || syncing);
    let header_compact = surface != PullRequestSurface::Overview && s.pr_header_compact;

    let state_for_surface = state.clone();
    let state_for_refresh = state.clone();

    div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        // Header (fixed, never scrolls)
        .child(
            render_pr_header(
                &repository,
                number,
                &pr_title,
                &pr_state,
                &author,
                detail.map(|d| (d.base_ref_name.clone(), d.head_ref_name.clone())),
                syncing,
                surface,
                header_compact,
                state_for_refresh,
                state_for_surface,
            ),
        )
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
                        .child(render_overview_surface(state, cx)),
                )
            },
        )
        .when(
            detail.is_some() && surface == PullRequestSurface::Files,
            |el| el.child(render_files_view(state, cx)),
        )
        .when(
            detail.is_some() && surface == PullRequestSurface::Tour,
            |el| el.child(render_tour_view(state, cx)),
        )
        .into_any_element()
}

fn render_pr_header(
    repository: &str,
    number: i64,
    pr_title: &str,
    pr_state: &str,
    author: &str,
    refs: Option<(String, String)>,
    syncing: bool,
    surface: PullRequestSurface,
    compact: bool,
    state_for_refresh: Entity<AppState>,
    state_for_surface: Entity<AppState>,
) -> impl IntoElement {
    let title = pr_title.to_string();
    let author = author.to_string();
    let repository = repository.to_string();

    div()
        .flex_shrink_0()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(if compact { px(12.0) } else { px(20.0) })
                .pb(if compact { px(12.0) } else { px(20.0) })
                .gap(px(16.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .text_color(if compact { fg_muted() } else { fg_subtle() })
                                .child(eyebrow(&format!(
                                    "Pull Requests / {} / #{}",
                                    repository, number
                                ))),
                        )
                        .child(
                            div()
                                .text_size(if compact { px(20.0) } else { px(24.0) })
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(title)
                                .with_animation(
                                    "pr-header-title",
                                    Animation::new(Duration::from_millis(180))
                                        .with_easing(ease_in_out),
                                    move |el, delta| {
                                        let progress = header_animation_progress(compact, delta);
                                        el.text_size(lerp_px(24.0, 20.0, progress))
                                    },
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .flex_wrap()
                                .mt(if compact { px(6.0) } else { px(10.0) })
                                .items_center()
                                .text_size(if compact { px(12.0) } else { px(13.0) })
                                .text_color(fg_muted())
                                .child(badge_success(pr_state))
                                .child(author)
                                .when(syncing, |el| el.child(badge("Refreshing live")))
                                .when_some(refs, |el, (base, head)| {
                                    if compact {
                                        el.child(badge(&base)).child(badge(&head))
                                    } else {
                                        el.child("wants to merge into")
                                            .child(badge(&base))
                                            .child("from")
                                            .child(badge(&head))
                                    }
                                })
                                .with_animation(
                                    "pr-header-meta",
                                    Animation::new(Duration::from_millis(180))
                                        .with_easing(ease_in_out),
                                    move |el, delta| {
                                        let progress = header_animation_progress(compact, delta);
                                        el.mt(lerp_px(10.0, 6.0, progress))
                                            .text_size(lerp_px(13.0, 12.0, progress))
                                    },
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(ghost_button("Open in browser", {
                            let repository = repository.clone();
                            move |_, window, cx| {
                                open_pull_request_in_browser(&repository, number, window, cx)
                            }
                        }))
                        .child(review_button("Refresh PR", {
                            let state = state_for_refresh.clone();
                            let repository = repository.clone();
                            move |_, window, cx| {
                                trigger_sync_pr(&state, &repository, number, window, cx)
                            }
                        })),
                ),
        )
        .child(div().flex().gap(px(2.0)).pb(if compact { px(8.0) } else { px(12.0) }).children(
            PullRequestSurface::all().iter().map(|surface_id| {
                let is_active = surface == *surface_id;
                let target_surface = *surface_id;
                let state = state_for_surface.clone();
                surface_tab(surface_id.label(), is_active, move |_, window, cx| {
                    if target_surface == PullRequestSurface::Tour {
                        enter_tour_surface(&state, window, cx);
                    } else if target_surface == PullRequestSurface::Files {
                        enter_files_surface(&state, window, cx);
                    } else {
                        state.update(cx, |st, cx| {
                            st.active_surface = target_surface;
                            st.pr_header_compact = false;
                            cx.notify();
                        });
                    }
                })
            }),
        ))
        .with_animation(
            "pr-header-shell",
            Animation::new(Duration::from_millis(180)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = header_animation_progress(compact, delta);
                el.pt(lerp_px(28.0, 16.0, progress))
                    .px(px(32.0))
                    .pb(px(0.0))
            },
        )
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
        .map(|viewer_login| summarize_own_pr_feedback(&detail.review_threads, viewer_login))
        .unwrap_or_default();

    let state_for_review = state.clone();
    let state_for_feedback = state.clone();

    div()
        .flex()
        .gap(px(20.0))
        // Main column
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(16.0))
                // Summary bar
                .child(
                    nested_panel().child(
                        div()
                            .flex()
                            .gap(px(20.0))
                            .items_center()
                            .text_size(px(13.0))
                            .text_color(fg_muted())
                            .font_family("Fira Code")
                            .child(format!("{} commits", detail.commits_count))
                            .child(format!("{} files changed", detail.changed_files))
                            .child(
                                div()
                                    .text_color(success())
                                    .child(format!("+{}", detail.additions)),
                            )
                            .child(
                                div()
                                    .text_color(danger())
                                    .child(format!("-{}", detail.deletions)),
                            ),
                    ),
                )
                .when(is_own_pull_request, |el| {
                    el.child(
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
                                            .child("Feedback Summary"),
                                    )
                                    .child(badge(&own_pr_feedback.len().to_string())),
                            )
                            .when(own_pr_feedback.is_empty(), |el| {
                                el.child(panel_state_text("No review feedback yet."))
                            })
                            .child(
                                div().flex().flex_col().gap(px(8.0)).children(
                                    own_pr_feedback.iter().map(|item| {
                                        let state = state_for_feedback.clone();
                                        let selected_file_path = item.file_path.clone();
                                        let selected_anchor = item.anchor.clone();
                                        let updated_at = item.updated_at.clone();

                                        div()
                                            .p(px(16.0))
                                            .rounded(radius())
                                            .bg(bg_subtle())
                                            .cursor_pointer()
                                            .hover(|style| style.bg(hover_bg()))
                                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                                state.update(cx, |state, cx| {
                                                    state.selected_file_path =
                                                        Some(selected_file_path.clone());
                                                    state.selected_diff_anchor =
                                                        Some(selected_anchor.clone());
                                                    cx.notify();
                                                });
                                                enter_files_surface(&state, window, cx);
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(10.0))
                                                    .flex_wrap()
                                                    .child(
                                                        div()
                                                            .text_size(px(13.0))
                                                            .font_family("Fira Code")
                                                            .text_color(fg_emphasis())
                                                            .child(item.location_label.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .gap(px(6.0))
                                                            .flex_wrap()
                                                            .child(badge(&item.subject_type.to_lowercase()))
                                                            .when(item.is_resolved, |el| {
                                                                el.child(badge_success("resolved"))
                                                            })
                                                            .when(item.is_outdated, |el| {
                                                                el.child(badge("outdated"))
                                                            })
                                                            .child(badge(&format!(
                                                                "{} feedback",
                                                                item.feedback_count
                                                            ))),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .mt(px(8.0))
                                                    .text_size(px(13.0))
                                                    .text_color(fg_default())
                                                    .child(item.preview.clone()),
                                            )
                                            .child(
                                                div()
                                                    .mt(px(8.0))
                                                    .text_size(px(12.0))
                                                    .text_color(fg_muted())
                                                    .child(format!(
                                                        "{} \u{2022} {}",
                                                        item.author_login, updated_at
                                                    )),
                                            )
                                    }),
                                ),
                            ),
                    )
                })
                // PR body
                .child(
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
                                        .child(badge(if loaded_from_cache {
                                            "cache"
                                        } else {
                                            "live"
                                        }))
                                        .when(syncing, |el| el.child(badge("refreshing"))),
                                ),
                        )
                        .child(div().max_w(px(640.0)).child(if detail.body.is_empty() {
                            div()
                                .text_size(px(13.0))
                                .text_color(fg_muted())
                                .child("No PR description provided.")
                                .into_any_element()
                        } else {
                            render_markdown(&detail.body).into_any_element()
                        })),
                )
                // Reviews
                .child(
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
                                        .child("Latest Reviews"),
                                )
                                .child(badge(&detail.latest_reviews.len().to_string())),
                        )
                        .when(detail.latest_reviews.is_empty(), |el| {
                            el.child(panel_state_text("No reviews yet."))
                        })
                        .child(div().flex().flex_col().gap(px(8.0)).children(
                            detail.latest_reviews.iter().map(|review| {
                                div()
                                    .p(px(16.0))
                                    .px(px(20.0))
                                    .rounded(radius())
                                    .bg(bg_subtle())
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(fg_emphasis())
                                            .text_size(px(14.0))
                                            .child(format!(
                                                "{} \u{2022} {}",
                                                review.author_login, review.state
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(fg_muted())
                                            .mt(px(4.0))
                                            .child(
                                                review
                                                    .submitted_at
                                                    .as_deref()
                                                    .unwrap_or("No timestamp")
                                                    .to_string(),
                                            ),
                                    )
                                    .when(!review.body.is_empty(), |el| {
                                        el.child(
                                            div().mt(px(8.0)).child(render_markdown(&review.body)),
                                        )
                                    })
                            }),
                        )),
                )
                // Submit review form
                .child(
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
                                        .child("Submit Review"),
                                )
                                .child(badge(match review_action {
                                    ReviewAction::Approve => "approve",
                                    ReviewAction::Comment => "comment",
                                    ReviewAction::RequestChanges => "request changes",
                                })),
                        )
                        // Action selector
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
                                    let state = state_for_review.clone();
                                    surface_tab(label, is_active, move |_, _, cx| {
                                        state.update(cx, |s, cx| {
                                            s.review_action = action;
                                            cx.notify();
                                        });
                                    })
                                }),
                            ),
                        )
                        // Review body placeholder (text input not yet implemented)
                        .child(
                            div()
                                .mt(px(12.0))
                                .p(px(12.0))
                                .px(px(14.0))
                                .rounded(radius_sm())
                                .bg(bg_subtle())
                                .text_color(if review_body.is_empty() {
                                    fg_subtle()
                                } else {
                                    fg_default()
                                })
                                .text_size(px(14.0))
                                .min_h(px(120.0))
                                .child(if review_body.is_empty() {
                                    "Leave a review note... (text input coming soon)".to_string()
                                } else {
                                    review_body
                                }),
                        )
                        // Submit button
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
                                    |_, _, _cx| {
                                        // TODO: submit review
                                    },
                                ))
                                .when_some(review_message, |el, msg| {
                                    if review_success {
                                        el.child(success_text(&msg))
                                    } else {
                                        el.child(error_text(&msg))
                                    }
                                }),
                        ),
                ),
        )
        // Side column
        .child(
            div()
                .w(detail_side_width())
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(16.0))
                // Details
                .child(
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
                                .child(meta_row(
                                    "Review decision",
                                    detail.review_decision.as_deref().unwrap_or("none"),
                                ))
                                .child(meta_row("Comments", &detail.comments_count.to_string()))
                                .child(meta_row(
                                    "Review threads",
                                    &detail.review_threads.len().to_string(),
                                ))
                                .child(meta_row("Updated", &detail.updated_at))
                                .when_some(fetched_at_ms, |el, ms| {
                                    el.child(meta_row("Cached at", &format_ms(ms)))
                                }),
                        ),
                )
                // Reviewers
                .child(
                    nested_panel()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .mb(px(12.0))
                                .child("Review status"),
                        )
                        .when(
                            review_status.approved.is_empty()
                                && review_status.changes_requested.is_empty()
                                && review_status.commented.is_empty()
                                && review_status.waiting.is_empty(),
                            |el| el.child(panel_state_text("No review activity yet.")),
                        )
                        .when(!review_status.approved.is_empty(), |el| {
                            el.child(render_review_status_group(
                                "Approved",
                                &review_status.approved,
                                success(),
                            ))
                        })
                        .when(!review_status.changes_requested.is_empty(), |el| {
                            el.child(render_review_status_group(
                                "Changes requested",
                                &review_status.changes_requested,
                                danger(),
                            ))
                        })
                        .when(!review_status.waiting.is_empty(), |el| {
                            el.child(render_review_status_group(
                                "Waiting",
                                &review_status.waiting,
                                fg_subtle(),
                            ))
                        })
                        .when(!review_status.commented.is_empty(), |el| {
                            el.child(render_review_status_group(
                                "Commented",
                                &review_status.commented,
                                accent(),
                            ))
                        }),
                )
                // Requested reviewers
                .child(
                    nested_panel()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .mb(px(8.0))
                                .child("Requested reviewers"),
                        )
                        .when(detail.reviewers.is_empty(), |el| {
                            el.child(panel_state_text("No reviewers requested."))
                        })
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .flex_wrap()
                                .mt(px(6.0))
                                .children(detail.reviewers.iter().map(|r| badge(r))),
                        ),
                )
                // Labels
                .child(
                    nested_panel()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .mb(px(8.0))
                                .child("Labels"),
                        )
                        .when(detail.labels.is_empty(), |el| {
                            el.child(panel_state_text("No labels."))
                        })
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .flex_wrap()
                                .mt(px(6.0))
                                .children(detail.labels.iter().map(|l| badge(l))),
                        ),
                )
                // Backend
                .child(
                    nested_panel()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .mb(px(12.0))
                                .child("Backend"),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(10.0))
                                .child(meta_row("gh", s.gh_version.as_deref().unwrap_or("unknown"))),
                        ),
                ),
        )
        .into_any_element()
}

fn render_review_status_group(label: &str, names: &[String], color: Rgba) -> impl IntoElement {
    div()
        .mb(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .mb(px(6.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_emphasis())
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_family("Fira Code")
                        .text_color(color)
                        .child(names.len().to_string()),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(4.0))
                .flex_wrap()
                .children(names.iter().map(|name| {
                    div()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(999.0))
                        .bg(bg_emphasis())
                        .text_size(px(11.0))
                        .text_color(color)
                        .child(name.clone())
                })),
        )
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
        .cursor_pointer()
        .when(active, |el| el.bg(bg_selected()).text_color(fg_emphasis()))
        .when(!active, |el| el.text_color(fg_muted()))
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
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
                .spawn(async move { github::sync_pull_request_detail(&cache, &repo, number) })
                .await;

            let detail_key = key;
            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = false;
                    ds.syncing = false;
                    match result {
                        Ok(snapshot) => {
                            ds.snapshot = Some(snapshot);
                            ds.error = None;
                        }
                        Err(e) => ds.error = Some(e),
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
    use super::{summarize_own_pr_feedback, summarize_review_status};
    use crate::github::{PullRequestReview, PullRequestReviewComment, PullRequestReviewThread};

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
                review("alice", "COMMENTED"),
                review("alice", "APPROVED"),
                review("bob", "CHANGES_REQUESTED"),
                review("carol", "COMMENTED"),
                review("", "APPROVED"),
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
            "Please add a null check before this branch. It currently panics."
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

    fn review(author_login: &str, state: &str) -> PullRequestReview {
        PullRequestReview {
            author_login: author_login.to_string(),
            state: state.to_string(),
            body: String::new(),
            submitted_at: None,
        }
    }

    fn comment(author_login: &str, body: &str, timestamp: &str) -> PullRequestReviewComment {
        PullRequestReviewComment {
            id: format!("comment-{author_login}-{timestamp}"),
            author_login: author_login.to_string(),
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
}
