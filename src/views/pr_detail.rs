use gpui::prelude::*;
use gpui::*;

use crate::github::{self, ReviewAction};
use crate::markdown::render_markdown;
use crate::state::*;
use crate::theme::*;

use super::diff_view::render_files_view;
use super::sections::{
    badge, badge_success, error_text, eyebrow, ghost_button, meta_row, nested_panel,
    panel_state_text, review_button, success_text,
};
use super::tour_view::{enter_tour_surface, refresh_active_tour_flow, render_tour_view};

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

    let state_for_surface = state.clone();
    let state_for_refresh = state.clone();

    div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        // Header (fixed, never scrolls)
        .child(
            div()
                .p(px(28.0))
                .px(px(32.0))
                .pb_0()
                .flex_shrink_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .mb(px(20.0))
                        .pb(px(20.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(eyebrow(&format!(
                                    "Pull Requests / {} / #{}",
                                    repository, number
                                )))
                                .child(
                                    div()
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(pr_title),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.0))
                                        .flex_wrap()
                                        .mt(px(10.0))
                                        .items_center()
                                        .text_size(px(13.0))
                                        .text_color(fg_muted())
                                        .child(badge_success(&pr_state))
                                        .child(author)
                                        .when_some(
                                            detail.map(|d| {
                                                (d.base_ref_name.clone(), d.head_ref_name.clone())
                                            }),
                                            |el, (base, head)| {
                                                el.child("wants to merge into")
                                                    .child(badge(&base))
                                                    .child("from")
                                                    .child(badge(&head))
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
                                        open_pull_request_in_browser(
                                            &repository,
                                            number,
                                            window,
                                            cx,
                                        )
                                    }
                                }))
                                .child(review_button(
                                    if syncing {
                                        "Refreshing..."
                                    } else {
                                        "Refresh PR"
                                    },
                                    {
                                        let state = state_for_refresh.clone();
                                        let repository = repository.clone();
                                        move |_, window, cx| {
                                            trigger_sync_pr(&state, &repository, number, window, cx)
                                        }
                                    },
                                )),
                        ),
                )
                // Surface nav
                .child(div().flex().gap(px(2.0)).pb(px(12.0)).children(
                    PullRequestSurface::all().iter().map(|s| {
                        let is_active = surface == *s;
                        let surf = *s;
                        let state = state_for_surface.clone();
                        surface_tab(s.label(), is_active, move |_, window, cx| {
                            if surf == PullRequestSurface::Tour {
                                enter_tour_surface(&state, window, cx);
                            } else {
                                state.update(cx, |st, cx| {
                                    st.active_surface = surf;
                                    cx.notify();
                                });
                            }
                        })
                    }),
                )),
        )
        // Content area (scrollable or flex-fill depending on surface)
        .when(loading, |el| {
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
    let fetched_at_ms = detail_state
        .and_then(|d| d.snapshot.as_ref())
        .and_then(|sn| sn.fetched_at_ms);

    let state_for_review = state.clone();

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
                                .child(badge(if loaded_from_cache { "cache" } else { "live" })),
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
                                .mb(px(8.0))
                                .child("Reviewers"),
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
                                .child(meta_row("gh", s.gh_version.as_deref().unwrap_or("unknown")))
                                .child(meta_row("Cache", &s.cache_path)),
                        ),
                ),
        )
        .into_any_element()
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
    let model = state.clone();
    let repo = repository.to_string();

    state.update(cx, |s, cx| {
        let ds = s.detail_states.entry(key.clone()).or_default();
        ds.syncing = true;
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
